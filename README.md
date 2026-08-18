# kansa

Turns a PM's prose HLD (markdown in a GitHub repo) into a structured, traceable requirement
inventory — without ever editing the HLD. Every sentence is classified as requirement-mapped,
context, or question until **residue = 0**; the inventory exports to
[reqtrace](https://github.com/marlboro-red/reqtrace) so downstream repos can be coverage-checked
in CI. See [spec.md](spec.md).

```
crates/kansa-core   state store, objects, segmentation, reconciliation, agent, export — all logic
crates/kansa-cli    `kansa` CLI (thin) + `kansa serve` dev bridge
app/                Tauri 2 + SolidJS desktop app (thin)
```

## What it does

- **Classifier** — serif doc pane with margin marks and a residue rail; drive the whole loop from
  the keyboard (`u/n/p/r/c/q/x/g`, command palette, question dialog).
- **Inventory** — repo-wide requirement table with filters, group-by, bulk status/retire; groups
  as umbrella labels with quick-assign and a group lens.
- **Reconciliation** — on fetch, changed docs get per-anchor verdicts (unchanged / reworded /
  missing) to confirm or adopt; meaning changes mark requirements *suspect*, never silently
  rewritten. PR heads can be classified as their own context.
- **Questions** — flag ambiguous sentences with readings and a default; review queue and round
  timeline in the review view.
- **Agent pre-fill** — a background `claude -p` job proposes classifications; accept/reject with
  `⏎`/`x` (or accept all). Accepted marks record `by: agent, accepted-by: you`.
- **Export** — `requirements.yaml` + `not-in-scope.yaml` in reqtrace format, validated with
  `reqtrace validate` when available.

State lives in a local store outside the repo (atomic writes, full history per object); the bare
clone is never checked out. The app and CLI are skins over the same core — same state, same ops.

Not yet: virtualization for >5k-sentence docs, PM read-only mode, multi-user, packaging/signing.

## Run

```sh
# prerequisites: rust stable, node 22, `git` + GitHub CLI `gh` logged in (required), optionally reqtrace + claude on PATH
# windows: Git for Windows, WebView2 runtime (Tauri installs it), MSVC build tools for building from source
cargo test                                   # core + cli
cd app && npm install && npm run tauri dev   # desktop app
```

Browser dev loop — the frontend in Chrome against the real core:

```sh
cargo run -p kansa-cli -- serve              # http://127.0.0.1:1430/api/<command>
cd app && npx vite                           # http://localhost:1420
```

CLI mirrors the app (same core ops):

```sh
kansa repo add owner/name                    # clone (bare, via gh/git) + create store
kansa repo add-local /path/to/folder         # a plain folder of markdown — no GitHub, no gh needed
kansa repo add acme/local --url file:///path # local git repo, e.g. for testing
kansa doc list -r owner/name
kansa doc track -r owner/name docs/hld.md
kansa spans -r owner/name -d docs/hld.md --residue
kansa classify req -r owner/name -d docs/hld.md <span-id>... -s "The system shall …" --pattern ubiquitous
kansa classify non-normative … | kansa classify question …
kansa req note <slug> -r owner/name -m "PM confirmed the window on the 12 Aug call."
kansa group add -r owner/name "Lockout"        # then: group assign|unassign|list|update
kansa group assign -r owner/name lockout <req-slug>...
kansa status -r owner/name
kansa close -r owner/name -d docs/hld.md
kansa export -r owner/name                   # requirements.yaml + not-in-scope.yaml, then `reqtrace validate`
kansa repo refresh -r owner/name             # fetch; changed docs go to reconciliation
```

All commands take `--json` for scripting.

## Configuration

| Env | Meaning |
|---|---|
| `KANSA_HOME` | state root (default: OS config dir `/kansa`); repos under `repos/`, bare clones under `clones/` |
| `KANSA_USER` | attribution in history (default `$USER`) |
| `KANSA_NO_GH` | pretend `gh` is absent (CI; only `file://` repos work then) |
| `KANSA_AGENT_MODEL` | model for `claude -p` |
| `KANSA_AGENT_CMD` | replace `claude -p` with any command reading the prompt on stdin and printing the JSON array |
| `REQTRACE_BIN` | reqtrace binary for post-export validate (else searched on PATH) |

## Keyboard (classifier)

`u` next unclassified · `n`/`p` (or `j`/`k`, arrows) move · `⇧`+move extend · `r` requirement (attach/create) · `c` context ·
`q` question · `x` clear / reject proposal · `⏎` accept proposal · `g` group · `e` show linked requirement · `?` all keys ·
`ctrl`/`⌘`+scroll (or `ctrl`/`⌘` `+`/`-`/`0`) zoom the page text
