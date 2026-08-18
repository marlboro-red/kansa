# `kansa` desktop app — UI spec v0.1

kansa turns a PM's prose HLD (markdown, living in a GitHub repo) into a structured, traceable requirement inventory — without ever editing the HLD. The core loop is **classification**: every sentence in the doc is accounted for as requirement-mapped, non-normative, or question-flagged; anything else is **residue**, and a doc is done when residue = 0. The inventory exports to [reqtrace](https://github.com/marlboro-red/reqtrace) format so downstream repos can be coverage-checked in CI.

The desktop app is the **primary human surface** for classification, inventory oversight, and question review. A thin CLI exists alongside it (scripting, CI). Same state store, same objects, same guarantees — the app is a skin over the file state, never a second source of truth.

Requirement IDs use the `<type>~<slug>~<rev>` grammar (reqtrace's `inv~id-grammar~2`). Types used in this spec: `ui` (app requirements), `obj` (object model), `out` (export). Object types in the store: `req` (requirement), `qst` (question), `grp` (group).

---

## 0. Object model

Everything below is owned by `kansa-core` and lives in the state store — a local directory **outside** the registered repo (e.g. `~/.kansa/repos/<owner>__<name>/`), so kansa never writes into the PM's repo. Promoting the store into the repo is a possible later change; nothing here depends on its location.

```
<store>/                              # <config-dir>/kansa/repos/<owner>__<name>/
  repo.yaml                           # remote, default branch, tracked docs
  snapshots/<doc-key>/<sha>.yaml      # segmented doc snapshots (immutable)
  current/<ctx>/<doc-key>             # sha of the current snapshot per context (branch or pr-N)
  reqs/<slug>.yaml                    # all revs of one requirement slug
  questions/<slug>.yaml
  groups/<slug>.yaml
  rounds/<ctx>/<doc-key>/<n>.yaml
  marks/<ctx>/<doc-key>.yaml          # non-normative marks (mapped/question states derive from anchors)
  pending/<ctx>/<doc-key>.yaml        # reconciliation awaiting confirmation
  proposals/<ctx>/<doc-key>.yaml      # agent proposals (proposed/accepted/rejected)
  jobs/<ctx>/<doc-key>.yaml           # background job status (pre-fill)
  exports/last.yaml                   # what was last exported (for "unexported changes")
  .lock                               # per-operation lock
```
The bare git clone lives beside it under `<config-dir>/kansa/clones/<owner>__<name>/`; kansa never checks it out.

- `obj~store-atomic~1` — Every mutation shall be a single atomic write (write-temp + rename) under the store lock, and shall append a history entry `{at, by, accepted-by?, op, from?, to?}` to the object it changes.
- `obj~store-shape~1` — All objects shall be YAML files, one per slug, holding every rev of that slug (newest last); the highest rev is *current*, matching reqtrace's `inv~current-rev~1`.

### 0.1 Snapshots and span identity

A **snapshot** is a tracked doc at one commit, segmented into sentences by the core segmenter. Snapshots are immutable and are the only thing anchors point into.

```yaml
doc: docs/hld.md
sha: 3f9c…            # git blob sha of the doc content
segmenter: 1          # segmenter version — bumping it produces new snapshots, not silently shifted spans
spans:
  - {id: s-8a1f, ord: 41, block: para, text: "When a user fails login…", h: 8a1f2c…}
  - {id: s-c04e, ord: 42, block: li,   text: "…", h: c04e…}
```

- `obj~span-id~1` — A span's `id` shall be derived from a content hash of its normalized text plus a disambiguator for repeated identical sentences (`h` + occurrence index), never from position alone; `ord` is display order only.
- `obj~span-blocks~1` — The segmenter shall treat list items, table rows, headings, and fenced code blocks as single spans (`block` records the kind); only prose paragraphs are sentence-split.
- `obj~span-structural~1` — Headings, code blocks and HTML blocks are *structural*: they are addressable and may be mapped, but they do not count toward the coverage denominator unless the user classifies them (`u` skips them).
- `obj~anchor~1` — An anchor is `{doc, span: <span-id>}` and resolves through the doc's *current* snapshot; when a fetch produces a new snapshot, reconciliation (§4.4) maps every anchor from the old snapshot to the new one and records the verdict; anchors are never rewritten silently.
- `obj~snapshot-current~1` — Each tracked doc shall have exactly one *current* snapshot per context (default branch, or a PR head when classifying a PR); the classifier always renders the current snapshot.

### 0.2 `req~` — requirement

Follows reqtrace's item schema so export is a projection, not a translation. reqtrace reads `id/statement/status/rating/owner` and preserves the rest.

```yaml
- id: req~login-throttling~2       # type~slug~rev; rev bumps when the statement's meaning changes
  statement: "When a user fails login 5 times in 10 min, the system shall …"   # EARS-formed
  pattern: event-driven            # ubiquitous | event-driven | state-driven | unwanted | optional | complex
  status: extracted                # extracted | assumed | confirmed | disputed | retired
  rating: [H, M]                   # optional [value, risk], H|M|L
  owner: pm-jane                   # optional
  reason: null                     # required when status: retired
  anchors: [{doc: docs/hld.md, span: s-8a1f}]   # n:m with spans
  questions: [qst~throttle-window~1]
  notes:                           # optional free-text commentary, oldest first
    - {at: 2026-08-18T09:14:00Z, by: cj, text: "PM confirmed the 10-min window on the 12 Aug call."}
  history: [...]
```

- `obj~req-revs~1` — Bumping a rev shall keep the prior rev in the file (status frozen as it was); export emits all revs so reqtrace's `stale` check works downstream.
- `obj~req-suspect~1` — When reconciliation confirms a `meaning-changed` verdict, the affected requirement's current rev gets `suspect: <reason>` (surfaced as a badge and in oversight); the next human edit of statement or status clears it.
- `obj~req-note~1` — A requirement's current rev may carry free-text `notes` (append + delete, `{at, by, text}`): commentary a human writes for themselves. Notes never bump the rev, never clear `suspect`, and are not exported to reqtrace; they carry forward when a rev is bumped, and each add/remove is recorded in `history`.
- `obj~req-groups-derived~1` — A requirement's group membership is *not* stored on the requirement; it is derived from `grp.members` at read time (single source of truth, §4.3).

### 0.3 `qst~` — question

Raised when prose is unclassifiable as-is: ambiguous, contradictory, or missing a decision.

```yaml
- id: qst~throttle-window~1
  status: open                     # open | answered | withdrawn
  quote: "…within a reasonable window…"          # the prose that triggered it
  anchors: [{doc: docs/hld.md, span: s-8a1f}]
  materiality: H                   # H|M|L — how much the answer changes the inventory
  readings:                        # candidate interpretations
    - {key: a, text: "10-minute sliding window", default: true}
    - {key: b, text: "per-session count, no window"}
  affects: [req~login-throttling~2]   # requirements whose statement/status depend on the answer
  answer: null                     # {reading: a, note, by, at} once answered
  history: [...]
```

- `obj~qst-apply~1` — Answering shall, in one atomic write, record the answer, set status `answered`, and append a history entry on every `affects` requirement pointing at the question; edits to those requirements' statements are then made by the human (agent-drafted when available), never auto-rewritten.
- `obj~qst-conflict~1` — If any `affects` requirement's current rev changed after the question was raised, the answer shall be held (`status` stays `open`, answer stored as `pending`) and the conflict surfaced for manual resolution.

### 0.4 Rounds

A **round** is the unit of classification work over one doc snapshot.

```yaml
doc: docs/hld.md
n: 3
snapshot: 3f9c…
context: {branch: main} | {pr: 42, head: 9b2e…}
opened: 2026-08-16T…
closed: null
summary: {created: [...], changed: [...], retired: [...], verdicts: {...}}   # filled at close
```

- `obj~round-open~1` — A round shall open automatically on the first mutation against a snapshot that has no open round; there is at most one open round per (doc, context).
- `obj~round-close~1` — A round shall close only by explicit user action, and only when residue = 0 and every reconciliation verdict for that snapshot is confirmed; closing writes the summary and freezes the round file.
- `obj~round-supersede~1` — If a new snapshot arrives while a round is open, the open round stays open against the old snapshot until reconciliation is confirmed, then closes and a new round opens against the new snapshot.

### 0.5 Reconciliation verdicts

When a fetch (or PR) yields a new snapshot of a classified doc, core maps every classified old span onto the new snapshot and stores a **pending reconciliation**: `unchanged` (same content hash — auto-accepted), `reworded` (best fuzzy match ≥ 0.55 similarity, same block kind — human decides *same meaning* or *meaning changed*), `missing` (no match — human must re-anchor, drop, or retire-with-reason), plus the list of `added` new spans (they become residue). Confirming applies the decisions in one pass: anchors and marks are remapped, `meaning-changed` sets `suspect`, the open round is closed with verdict counts, and the current pointer advances (`obj~round-supersede~1`). Until then the classifier keeps rendering the old snapshot and refuses to close the round.

---

## 1. Stack decision: Tauri 2

**Yes — Tauri 2, with the responsiveness budget enforced by architecture, not hoped for.**

- **Why Tauri fits:** the domain logic lives in a Rust core crate (greenfield — this is a fresh repo) — Tauri lets the app link it directly: no server, no serialization of the state store over HTTP, direct file access, single distributable binary per OS. System webview keeps the bundle small.
- **Why not pure-native (egui):** the UI is fundamentally rich-text annotation — rendered markdown with thousands of interactive spans. That's what DOM engines are best at and immediate-mode GUIs are worst at.
- **Why not a local web server + browser:** loses single-app feel, file dialogs, and OS integration; gains nothing Tauri doesn't have.
- **Frontend:** a fine-grained-reactivity framework — **SolidJS or Svelte 5** (TypeScript) — not React. Classifying a sentence must repaint one span, not diff a tree containing every span in the document.
- **Workspace shape:** `kansa-core` (state store, objects, segmentation, reconciliation, export — everything), `kansa-cli` (thin), `kansa-app` (Tauri; commands are thin wrappers over core ops). One behavior, three entry points.

- `ui~core-parity~1` — Every mutation the app performs shall go through the same `kansa-core` operations the CLI uses (same validation, same atomic writes, same history entries); the app shall contain no state-mutation logic of its own.
- `ui~lock-scope~1` — The app shall take the state lock per operation, not for the session, so CLI and app can be used interleaved on the same repo.
- `ui~offline~1` — The app shall be fully functional with no agent backend available: manual classification, requirement authoring, question answering. Agent features degrade to hidden/disabled, never to broken.

## 2. Performance requirements (the "highly responsive" contract)

- `ui~perf-classify~1` — A classification action (keypress → span recolored, inventory updated, next sentence focused) shall render its visual result within one frame (≤16 ms perceived); state persistence happens off the UI thread (optimistic UI, rollback on write failure with an error toast).
- `ui~perf-open~1` — Opening a registered repo shall reach interactive (doc rendered, spans live) in ≤1.5 s for a 200 KB HLD with 2,000 sentences on commodity hardware.
- `ui~perf-virtualize~1` — Documents beyond 5,000 sentences shall use windowed/virtualized rendering; scrolling shall not drop below 60 fps on such documents.
- `ui~perf-search~1` — Inventory/prose search shall return results within 50 ms for 1,000 inventory items (in-memory index, rebuilt on state change).
- `ui~perf-agent-async~1` — Agent calls shall never block the UI; pre-fill runs as a background job with visible progress, and its proposals appear incrementally as each stage validates.

## 3. Repos and sources

HLDs live as markdown in GitHub repos. The app registers repos, not loose files.

- `ui~repo-register~1` — The user shall register a GitHub repo (owner/name, authenticated through `gh`); the app shall clone/fetch it locally and keep kansa state for that repo in a local state store keyed by the repo, never written into the repo itself.
- `ui~repo-local~1` — A local folder (no GitHub) can be registered too: kansa imports its markdown files into a private bare git repo under `clones/` (commits built with libgit2, no worktree, no `git`/`gh`), so snapshots, rounds, refresh ("Re-import") and reconciliation behave identically; PR features are hidden for local repos.
- `ui~repo-docs~1` — For a registered repo the app shall list its markdown docs on the default branch and let the user pick which are HLDs (tracked docs); untracked docs are ignored by classification and oversight.
- `ui~pr-view~1` — The app shall list open PRs (via `gh pr list`) on a registered repo; a PR is a place to work: it lists the markdown files at the PR head (changed ones first), any file can be opened there, and files can be tracked/untracked from inside the PR (a file added by the PR can be tracked before it exists on the default branch). Opening a tracked doc at the PR head shows the PR text rendered with the base classification projected onto it (anchors are content-addressed, so unchanged sentences keep their state), new sentences highlighted, and a read-only verdict list against the base snapshot. Classifying new sentences against a PR head is allowed (the round is tagged with PR number and head SHA); verdicts are not confirmable in a PR context — that happens on the default branch after merge.
- `ui~repo-refresh~1` — Fetching is explicit (button / `R`), never on a timer; a fetch that changes a tracked doc opens the reconciliation flow.

## 4. The views

### 4.1 Classifier (the main screen)

Two panes + a status bar.

**Left — the document.** Rendered markdown; every sentence an addressable span, color-coded by state:

- **unclassified** (loud — this is the residue),
- **requirement-mapped** (one or more `req~` links),
- **non-normative** (context/rationale/example),
- **question-flagged** (unclassifiable → `qst~` object).

**Right — the inventory panel.** The requirement list for this doc: ID, EARS statement, pattern, status chip (extracted/assumed/confirmed/disputed/retired), open-question badge. Includes filter/search and a detail drawer (anchors, history, assumptions, linked questions).

**Status bar:** the coverage meter — `classified / total sentences`, residue count, open questions, current round. Doc is completable when residue = 0.

- `ui~spans~1` — Sentence segmentation shall be computed deterministically by `kansa-core` (same segmenter the agent pipeline uses) and persisted with the snapshot; the UI shall never re-segment independently.
- `ui~bidirectional~1` — Selecting a requirement shall highlight all its source spans in the document (scrolling to the first); selecting a span shall highlight its linked requirement(s) in the panel. Provenance is one click in both directions, always.
- `ui~grouping~1` — The user shall be able to select a contiguous run of sentences (shift-click / shift+arrows) and classify the group as one unit — one requirement may anchor to multiple sentences and one sentence to multiple requirements (n:m).
- `ui~residue-nav~1` — A single key (`u`) shall jump to the next unclassified sentence; the systematic pass is: `u`, classify, repeat until the meter reads zero.

**Keyboard map (primary; mouse is secondary):**
`u` next unclassified · `n`/`p` next/prev sentence · `r` map to requirement (opens quick-create/attach with agent-drafted EARS statement when available) · `c` mark non-normative · `q` flag as question (materiality + readings dialog) · `e` edit linked requirement · `g` group quick-assign · `enter` confirm agent proposal · `x` reject agent proposal · `/` search.

### 4.2 Inventory view

Full-repo table across docs: every requirement with status, rating, owner, doc, open questions, rev; filterable; row → detail drawer with full history and anchors (click-through opens the classifier at that span). Bulk actions: confirm, dispute, retire-with-reason (the dialog requires a non-empty reason; it is recorded in history). Export button = `export --format reqtrace`, writing two files: `requirements.yaml` (all reqs, all revs, `groups:` titles added) and `not-in-scope.yaml` (retired reqs with their `reason` as `justification`); a post-export `reqtrace validate` run is surfaced inline when the binary is available, and `exports/last.yaml` is updated so "unexported changes" can be counted.

- `ui~oversight~1` — The inventory view shall show, per doc and per repo, the counts that constitute oversight: total requirements by status, residue remaining, open questions, suspect links (post-reconciliation), unexported changes since last export — and the same rollups per group (`ui~grp-lens~1`).

### 4.3 Requirement groups

Groups are umbrella labels over requirements — "validation", "defaulting logic", "lockout" — for oversight and navigation. They are objects in the state store (`groups/*.yaml`), not UI decoration.

```yaml
id: grp~validation~1
title: "Validation"
description: "Input validation rules across the intake forms"
members: [req~email-format~1, req~abn-checksum~2, req~required-fields~1]
```

- `obj~grp-membership~1` — Group membership shall be n:m (a requirement may belong to several groups; a group holds any number of requirements) and flat in v1 (no nested groups). `grp.members` is the **only** place membership is stored (`obj~req-groups-derived~1`); membership changes append history on the group object.
- `obj~grp-integrity~1` — A group member that no longer resolves to an inventory requirement (retired or missing) shall be surfaced as a group finding, never silently dropped from the member list.
- `ui~grp-assign~1` — In the classifier and inventory views, `g` on a selected requirement (or multi-selection) shall open group quick-assign (fuzzy-find existing groups, create inline); assignment shall be one keystroke plus a name, not a form. The palette toggles: picking a group that *every* target already belongs to removes them from it (rows show `n of m in` for a partial selection), and the drawer's group chips carry a ✕ for the single-requirement case.
- `ui~grp-lens~1` — The inventory view shall offer group-by-group display with per-group rollups (member count by status, open questions, residue-linked spans, suspect links), and a group filter in the classifier that dims spans not anchored to the selected group's members.
- `ui~grp-agent~1` — Agent pre-fill may propose group assignments (same proposed/accept flow as `ui~agent-prefill~1`); proposed groups are created only on acceptance.
- `out~groups~1` — `export --format reqtrace` shall include each requirement's group titles as a `groups: [..]` field on inventory items — reqtrace ignores unknown keys by spec, so downstream tooling can use groups without a format break.

### 4.4 Review view (rounds & questions)

- Round timeline per doc: each round's diff summary (created/changed/retired, reconciliation verdicts), immutable once closed.
- Question queue: open questions with quoted prose, readings, default; answering applies atomically to every requirement the question is linked to (status/statement update + history entry in one write); if a linked requirement changed since the question was raised, the answer is held and the conflict shown for manual resolution.
- Reconciliation review: after an HLD edit, the verdict list (unchanged / reworded / meaning-changed / new / missing) presented for human confirmation *before* the round closes; `missing` items block closure until retired-with-reason or re-anchored.

- `ui~agent-prefill~1` — On opening an unclassified/changed doc, the app shall offer (not auto-run) an agent pre-fill pass; proposals shall land in a visually distinct "proposed" state requiring per-item or per-group accept (`enter`) / reject (`x`), and accepted items shall record `by: agent, accepted-by: <user>` in history.
- `ui~agent-backend~1` — The agent backend is the Claude Code CLI (`claude -p`, model overridable with `KANSA_AGENT_MODEL`); `KANSA_AGENT_CMD` substitutes any command that reads the prompt on stdin and prints the JSON array (used by tests and for other providers). Pre-fill runs on a background thread in batches of 40 sentences; progress and proposals are files in the store so the CLI and app see the same job.

## 5. Milestones

Greenfield. Order is chosen so the core loop (open doc → classify → residue 0 → export) is real end-to-end as early as possible; everything else layers on.

1. **UM0 — core + shells (~2–3 days):** cargo workspace (`kansa-core`, `kansa-cli`, `kansa-app`); core: store layout, atomic writes + lock, `req`/`qst`/`grp`/round objects, snapshot + segmenter (§0.1) with golden tests, reqtrace export; CLI: `repo add`, `doc track`, `status`, `export`; Tauri shell: registers a GitHub repo (clone/fetch), lists tracked docs. CI on macOS + Windows + Linux. *Done when: `kansa export` on a hand-written store passes `reqtrace validate`, and the same core tests pass from CLI and app.*
2. **UM1 — manual classifier (~1 week):** doc rendering from snapshot spans, `u`/`n`/`p`/`r`/`c`/`q` keys, contiguous-run selection, requirement quick-create, coverage meter, optimistic persistence through core ops, round opens on first mutation. No agent, no groups. *Done when: a real HLD can be classified by hand to residue 0, the round closes, and CLI `status`/`export` see the result.*
3. **UM2 — inventory, groups + bidirectional nav (~4–5 days):** inventory view, detail drawer, click-through both directions, groups (`g` quick-assign, group lens, rollups), retire-with-reason, export button (both files, `groups:` field), unexported-changes count. *Done when: `ui~bidirectional~1`, `ui~oversight~1`, and `ui~grp-lens~1` demo cleanly.*
4. **UM3 — reconciliation, PRs & questions (~1.5 weeks):** new-snapshot detection on fetch, anchor mapping + verdicts, verdict review before round close, round supersede; PR list + open a PR's doc with diff and verdicts; question dialog/queue, answer apply + conflict hold. *Done when: editing the HLD in a PR and opening it in the app walks the user through a correct diff-round.*
5. **UM4 — agent pre-fill (~3–5 days):** background `claude -p` jobs per stage, proposed-state UI, accept/reject flow, agent-drafted EARS on `r`, agent-proposed groups. *Done when: pre-fill + human pass over a fresh HLD is measurably faster than UM1 manual-only.*

Deferred: PM-facing read-only mode, multi-user, in-app HLD editing (never — `the tool never edits the PM's HLD` stands), packaging/signing polish, promoting the state store into the repo.

## 5a. Platform

- `ui~platforms~1` — The app and CLI shall run on macOS, Windows, and Linux from one codebase; CI shall build and run core tests on all three from UM0.
- `ui~windows-paths~1` — Store paths, doc keys, and anchors shall be platform-neutral: forward-slash doc paths as stored in git, store root under the OS config dir (`dirs`), no case-sensitivity assumptions in slugs/doc keys, and atomic rename done with a Windows-safe strategy (retry on sharing violation).
- `ui~dev-bridge~1` — `kansa serve` exposes the app's command surface (`kansa_core::api::call`) over local HTTP so the frontend runs in an ordinary browser for development and automated UI testing; the Tauri app and the bridge dispatch through the same table (`ui~core-parity~1`).
- `ui~gh-required~1` — The GitHub CLI (`gh`, logged in) is a hard requirement for GitHub repos: auth (`gh auth setup-git`), repo metadata (`gh repo view`), PRs (`gh pr list`, `gh pr view --json files`), and fetches via the `git` it fronts. Local reads (blobs, trees, diffs) use libgit2 and never touch the network. Without `gh` the app fails fast with install/login instructions; only local `file://` repos (tests) work.

## 6. Risks specific to the app

1. **Segmentation quality is now user-visible.** Bad sentence splits were invisible in the CLI; in the UI every wrong split is friction. Budget real time for the segmenter (markdown-aware: lists, tables, code blocks are units, not sentence-split).
2. **Webview variance.** System webviews differ (WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux); test the span-heavy DOM on all three early — UM1, not UM4. Windows is a first-class target, so WebView2 gets tested in UM1, not "eventually".
3. **Scope gravity.** A desktop app invites feature creep (themes, tabs, editors). The views above are v1; anything else goes to the deferred list.
