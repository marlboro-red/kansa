import { createResource, createSignal, ErrorBoundary, For, Show, type Component } from "solid-js";
import { api, type RepoSummary } from "./api";
import { Classifier, type Toast } from "./Classifier";
import { InventoryView } from "./InventoryView";
import { ReviewView } from "./ReviewView";
import type { Context, PrSummary } from "./api";

type Route =
  | { kind: "home" }
  | { kind: "repo"; github: string }
  | { kind: "inventory"; github: string }
  | { kind: "review"; github: string }
  | { kind: "pr"; github: string; pr: PrSummary }
  | { kind: "doc"; github: string; doc: string; span?: string; context?: Context };

const App: Component = () => {
  const [repos, { refetch: refetchRepos }] = createResource(api.listRepos);
  const [route, setRouteRaw] = createSignal<Route>(loadRoute());
  const setRoute = (r: Route) => { setRouteRaw(r); try { localStorage.setItem("kansa.route", JSON.stringify(r)); } catch {} };
  const [toast, setToast] = createSignal<Toast | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  let toastTimer: number | undefined;

  function showToast(t: Toast | null) {
    setToast(t);
    if (toastTimer) window.clearTimeout(toastTimer);
    if (t && t.kind === "info") toastTimer = window.setTimeout(() => setToast(null), 3500);
  }

  async function run<T>(label: string, f: () => Promise<T>): Promise<T | undefined> {
    setBusy(label);
    try {
      return await f();
    } catch (e) {
      showToast({ kind: "error", text: String(e) });
    } finally {
      setBusy(null);
    }
  }

  const currentGithub = () => { const r = route(); return r.kind === "home" ? null : r.github; };
  const currentDoc = () => { const r = route(); return r.kind === "doc" ? r.doc : null; };

  return (
    <div class="shell">
      <aside class="sidebar">
        <header class="brand"><span class="logo" /><span>kansa</span></header>
        <RepoAdd
          disabled={busy() !== null}
          onAdd={async (gh) => {
            const r = await run(`cloning ${gh}`, () => api.registerRepo(gh));
            if (r) { await refetchRepos(); setRoute({ kind: "repo", github: r.github }); }
          }}
        />
        <nav class="repos">
          <Show when={repos()} fallback={<p class="muted" style={{ padding: "0 10px" }}>loading…</p>}>
            <For each={repos()} fallback={<p class="muted" style={{ padding: "0 10px", "font-size": "12px" }}>No repos yet. Add one above.</p>}>
              {(r) => (
                <>
                  <button class="repo" classList={{ active: currentGithub() === r.github && !currentDoc() }} onClick={() => setRoute({ kind: "repo", github: r.github })}>
                    <span class="name" title={r.github}><span class="owner">{r.github.split("/")[0]}/</span>{r.github.split("/").slice(1).join("/")}</span>
                    <span class="meta">{r.default_branch} · {r.tracked.length} tracked</span>
                  </button>
                  <Show when={currentGithub() === r.github}>
                    <DocNav repo={r} current={currentDoc()} onOpen={(d) => setRoute({ kind: "doc", github: r.github, doc: d })} />
                    <div class="docnav">
                      <button classList={{ active: route().kind === "inventory" }} onClick={() => setRoute({ kind: "inventory", github: r.github })}>
                        <span class="dot inv-dot" /><span>Inventory</span>
                      </button>
                      <button classList={{ active: route().kind === "review" }} onClick={() => setRoute({ kind: "review", github: r.github })}>
                        <span class="dot inv-dot" /><span>Review</span>
                      </button>
                    </div>
                  </Show>
                </>
              )}
            </For>
          </Show>
        </nav>
        <footer class="status-line">
          <Show when={busy()} fallback={<span>ready</span>}><span class="spinner" /> {busy()}</Show>
        </footer>
      </aside>

      <main class="content">
        <Show when={toast()}>
          {(t) => <div class={`toast ${t().kind}`} onClick={() => setToast(null)}>{t().text}</div>}
        </Show>
        <Show when={route().kind === "home"}>
          <div class="empty">
            <h1>Register a GitHub repo to begin</h1>
            <p class="muted">kansa reads markdown HLDs from a repo and never writes into it. Classification state lives in your local kansa home.</p>
          </div>
        </Show>
        <Show when={route().kind === "repo" && currentGithub()}>
          {(gh) => (
            <RepoPane
              github={gh()}
              repo={repos()?.find((r) => r.github === gh())}
              run={run}
              onChanged={refetchRepos}
              onOpenDoc={(d) => setRoute({ kind: "doc", github: gh(), doc: d })}
              onOpenPr={(pr) => setRoute({ kind: "pr", github: gh(), pr })}
            />
          )}
        </Show>
        <Show when={route().kind === "inventory" && currentGithub()}>
          {(gh) => (
            <InventoryView
              github={gh()}
              onOpenAnchor={(doc, span) => setRoute({ kind: "doc", github: gh(), doc, span })}
              toast={showToast}
            />
          )}
        </Show>
        <Show when={route().kind === "review" && currentGithub()}>
          {(gh) => (
            <ReviewView
              github={gh()}
              onOpenAnchor={(doc, span) => setRoute({ kind: "doc", github: gh(), doc, span })}
              onOpenDoc={(doc) => setRoute({ kind: "doc", github: gh(), doc })}
              toast={showToast}
            />
          )}
        </Show>
        <Show when={route().kind === "pr" && route()} keyed>
          {(r) => r.kind === "pr" && (
            <PrPane
              github={r.github}
              pr={r.pr}
              run={run}
              onBack={() => setRoute({ kind: "repo", github: r.github })}
              onOpenDoc={async (doc) => {
                const ctx = await run(`opening PR #${r.pr.number}`, () => api.openPr(r.github, r.pr.number, doc));
                if (ctx) setRoute({ kind: "doc", github: r.github, doc, context: ctx });
              }}
              onChanged={refetchRepos}
            />
          )}
        </Show>
        <Show when={route().kind === "doc" && route()} keyed>
          {(r) => r.kind === "doc" && (
            <ErrorBoundary fallback={(e) => <div class="empty"><h1>Something broke</h1><pre class="mono" style={{ "white-space": "pre-wrap" }}>{String(e?.stack ?? e)}</pre></div>}>
              <Classifier github={r.github} doc={r.doc} initialSpan={r.span} context={r.context} onBack={() => setRoute({ kind: "repo", github: r.github })} toast={showToast} />
            </ErrorBoundary>
          )}
        </Show>
      </main>
    </div>
  );
};

function loadRoute(): Route {
  try { const r = JSON.parse(localStorage.getItem("kansa.route") ?? ""); if (r && typeof r.kind === "string") return r as Route; } catch {}
  return { kind: "home" };
}

const DocNav: Component<{ repo: RepoSummary; current: string | null; onOpen: (doc: string) => void }> = (p) => {
  const [status] = createResource(() => p.repo.github, api.repoStatus);
  return (
    <div class="docnav">
      <For each={p.repo.tracked}>
        {(d) => {
          const st = () => status()?.docs.find((x) => x.doc === d);
          return (
            <button classList={{ active: p.current === d }} onClick={() => p.onOpen(d)} title={d}>
              <span class="dot" classList={{ done: (st()?.meter?.residue ?? 1) === 0 }} />
              <span style={{ overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>{d.split("/").pop()}</span>
              <Show when={(st()?.meter?.residue ?? 0) > 0}><span class="residue">{st()?.meter?.residue}</span></Show>
            </button>
          );
        }}
      </For>
    </div>
  );
};

/** A pull request as a place to work: its markdown files, open any at the PR head, track/untrack. */
const PrPane: Component<{
  github: string;
  pr: PrSummary;
  run: <T>(label: string, f: () => Promise<T>) => Promise<T | undefined>;
  onBack: () => void;
  onOpenDoc: (doc: string) => void;
  onChanged: () => void;
}> = (p) => {
  const [docs, { refetch }] = createResource(() => [p.github, p.pr.number] as const, ([g, n]) => api.prDocs(g, n));
  async function toggle(path: string, tracked: boolean) {
    await p.run<unknown>(tracked ? `untracking ${path}` : `tracking ${path}`, () => (tracked ? api.untrackDoc(p.github, path) : api.trackDoc(p.github, path)));
    refetch();
    p.onChanged();
  }
  const sorted = () => [...(docs() ?? [])].sort((a, b) => Number(b.tracked) - Number(a.tracked) || a.path.localeCompare(b.path));
  return (
    <div class="repo-pane">
      <header class="pane-head">
        <div>
          <p class="muted" style={{ margin: "0 0 4px" }}><button class="ghost" onClick={p.onBack}>← {p.github}</button></p>
          <h1><span class="mono muted">#{p.pr.number}</span> {p.pr.title}</h1>
          <p class="muted">{p.pr.author} · {p.pr.head_ref} → {p.pr.base_ref} · updated {p.pr.updated_at.slice(0, 10)}{p.pr.draft ? " · draft" : ""}</p>
        </div>
      </header>
      <section>
        <h2>Markdown files changed in this PR</h2>
        <p class="muted small">Open a file to read it at the PR head with the base classification projected onto it; track a file to classify it.</p>
        <Show when={docs()} fallback={<p class="muted">loading…</p>}>
          <ul class="prfiles">
            <For each={sorted()} fallback={<li class="muted">This PR doesn't change any markdown files.</li>}>
              {(d) => (
                <li classList={{ changed: true }}>
                  <span class="mono path">{d.path}</span>
                  <span class="chip pr">{d.status}</span>
                  <span style={{ flex: 1 }} />
                  <Show when={d.status !== "deleted"} fallback={<span class="muted small">removed by this PR{d.tracked ? " — reconcile after merge" : ""}</span>}>
                    <label class="small muted" style={{ display: "flex", gap: "6px", "align-items": "center" }}>
                      <input type="checkbox" checked={d.tracked} onChange={() => toggle(d.path, d.tracked)} /> tracked
                    </label>
                    <button class="primary" onClick={() => p.onOpenDoc(d.path)}>Open at PR head</button>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </section>
    </div>
  );
};

const RepoAdd: Component<{ disabled: boolean; onAdd: (gh: string) => void }> = (p) => {
  const [value, setValue] = createSignal("");
  return (
    <form
      class="repo-add"
      onSubmit={(e) => { e.preventDefault(); const v = value().trim(); if (v) { p.onAdd(v); setValue(""); } }}
    >
      <input placeholder="owner/name" value={value()} onInput={(e) => setValue(e.currentTarget.value)} disabled={p.disabled} spellcheck={false} />
      <button type="submit" disabled={p.disabled || !value().trim()}>Add</button>
    </form>
  );
};

const RepoPane: Component<{
  github: string;
  repo?: RepoSummary;
  run: <T>(label: string, f: () => Promise<T>) => Promise<T | undefined>;
  onChanged: () => void;
  onOpenDoc: (doc: string) => void;
  onOpenPr: (pr: PrSummary) => void;
}> = (p) => {
  const [docs, { refetch: refetchDocs }] = createResource(() => p.github, api.listDocs);
  const [status, { refetch: refetchStatus }] = createResource(() => p.github, api.repoStatus);
  const [prErr, setPrErr] = createSignal("");
  const [prs, { refetch: refetchPrs }] = createResource(() => p.github, (g) => api.listPrs(g).catch((e) => { setPrErr(String(e)); return [] as PrSummary[]; }));
  const [reconDocs, setReconDocs] = createSignal<string[]>([]);
  const [pick, setPick] = createSignal("");
  const [pickOpen, setPickOpen] = createSignal(false);
  const pickList = () => {
    const q = pick().trim().toLowerCase();
    return (docs() ?? []).filter((d) => !d.tracked && (!q || d.path.toLowerCase().includes(q))).slice(0, 12);
  };
  const [prQuery, setPrQuery] = createSignal("");
  const [prOnlyTracked, setPrOnlyTracked] = createSignal(false);
  const [prHideDrafts, setPrHideDrafts] = createSignal(false);
  const prsFiltered = () => {
    const q = prQuery().trim().toLowerCase();
    return (prs() ?? [])
      .filter((pr) => !prOnlyTracked() || pr.touches.length)
      .filter((pr) => !prHideDrafts() || !pr.draft)
      .filter((pr) => !q || `#${pr.number} ${pr.title} ${pr.author} ${pr.head_ref} ${pr.files.join(" ")}`.toLowerCase().includes(q))
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  };

  async function toggle(path: string, tracked: boolean) {
    await p.run<unknown>(tracked ? `untracking ${path}` : `snapshotting ${path}`, () =>
      tracked ? api.untrackDoc(p.github, path) : api.trackDoc(p.github, path),
    );
    await Promise.all([refetchDocs(), refetchStatus()]);
    p.onChanged();
  }
  async function refresh() {
    const changes = await p.run(`fetching ${p.github}`, () => api.refreshRepo(p.github));
    if (changes) {
      await Promise.all([refetchDocs(), refetchStatus(), refetchPrs()]);
      p.onChanged();
      setReconDocs(changes.filter((c) => c.to && !c.advanced).map((c) => c.doc));
    }
  }

  return (
    <div class="repo-pane">
      <Show when={reconDocs().length}>
        <div class="export-result bad" style={{ margin: "0 0 12px" }}>
          <div>Changed upstream: <b>{reconDocs().join(", ")}</b> — open the doc to review the changes before classifying further.</div>
          <button class="ghost" onClick={() => setReconDocs([])}>dismiss</button>
        </div>
      </Show>
      <header class="pane-head">
        <div>
          <h1>{p.github}</h1>
          <p class="muted">{p.repo?.default_branch} · last fetch {p.repo?.last_fetch?.slice(0, 16) ?? "—"}</p>
        </div>
        <button onClick={refresh}>Refresh</button>
      </header>

      <section>
        <div style={{ display: "flex", "align-items": "baseline", gap: "12px" }}>
          <h2>Tracked HLDs</h2>
          <span style={{ flex: 1 }} />
          <div class="trackpick">
            <input placeholder="Track a doc… (search markdown files)" value={pick()} onInput={(e) => setPick(e.currentTarget.value)} onFocus={() => setPickOpen(true)} onBlur={() => setTimeout(() => setPickOpen(false), 150)} onKeyDown={(e) => { if (e.key === "Escape") setPickOpen(false); if (e.key === "Enter" && pickList()[0]) toggle(pickList()[0].path, false); }} />
            <Show when={pickOpen() && pickList().length}>
              <div class="matches pickmenu">
                <For each={pickList()}>{(d) => <div class="match" onMouseDown={() => { toggle(d.path, false); setPick(""); }}><span class="mono">{d.path}</span></div>}</For>
              </div>
            </Show>
          </div>
        </div>
        <Show when={status()} fallback={<p class="muted">loading…</p>}>
          {(s) => (
            <table class="docs">
              <thead><tr><th>Doc</th><th>Coverage</th><th>Residue</th><th>Questions</th><th>Round</th><th></th></tr></thead>
              <tbody>
                <For each={s().docs} fallback={<tr><td colSpan={6} class="muted">Nothing tracked yet — use “Track a doc…” to pick an HLD, then open it to classify.</td></tr>}>
                  {(d) => (
                    <tr class="clickable" onClick={() => p.onOpenDoc(d.doc)}>
                      <td class="mono">{d.doc}</td>
                      <td>
                        <Show when={d.meter} fallback="—">
                          {(m) => (
                            <span class="meter">
                              <span class="bar"><span style={{ width: `${m().total ? (100 * m().classified) / m().total : 0}%` }} /></span>
                              {m().classified}/{m().total}
                            </span>
                          )}
                        </Show>
                      </td>
                      <td classList={{ loud: (d.meter?.residue ?? 0) > 0 }}>{d.meter?.residue ?? "—"}</td>
                      <td>{d.meter?.open_questions ?? "—"}</td>
                      <td>{d.open_round ? `#${d.open_round} open` : `${d.rounds_closed} closed`}</td>
                      <td class="c-untrack" onClick={(e) => e.stopPropagation()}><button class="ghost" title="Stop tracking (keeps existing requirements)" onClick={() => toggle(d.doc, true)}>untrack</button></td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          )}
        </Show>
        <Show when={status()}>
          {(s) => (
            <p class="rollup muted">
              reqs: {Object.entries(s().rollup.reqs_by_status).map(([k, v]) => `${k} ${v}`).join(" · ")} · open questions {s().rollup.open_questions} · groups {s().rollup.groups}
              <Show when={s().rollup.unexported_changes}> · <span class="loud">unexported changes</span></Show>
            </p>
          )}
        </Show>
      </section>

      <section>
        <h2>Open pull requests <span class="muted" style={{ "text-transform": "none", "letter-spacing": 0 }}>{prs() ? `${prsFiltered().length}/${prs()!.length}` : ""}</span></h2>
        <Show when={(prs() ?? []).length}>
          <div class="prfilters">
            <input placeholder="Search PRs — number, title, author, branch, file…" value={prQuery()} onInput={(e) => setPrQuery(e.currentTarget.value)} />
            <label class="small muted"><input type="checkbox" checked={prOnlyTracked()} onChange={(e) => setPrOnlyTracked(e.currentTarget.checked)} /> touches tracked docs</label>
            <label class="small muted"><input type="checkbox" checked={prHideDrafts()} onChange={(e) => setPrHideDrafts(e.currentTarget.checked)} /> hide drafts</label>
          </div>
        </Show>
        <Show when={prs()} fallback={<p class="muted">loading…</p>}>
          <Show when={(prs() ?? []).length} fallback={<p class="muted" style={{ "font-size": "12px" }}>{prErr() || "No open pull requests."}</p>}>
            <ul class="prlist">
              <For each={prsFiltered()} fallback={<li class="muted" style={{ "font-size": "12px" }}>No pull requests match.</li>}>
                {(pr) => (
                  <li class="clickable" onClick={() => p.onOpenPr(pr)}>
                    <div class="row1">
                      <span class="mono muted">#{pr.number}</span>
                      <b>{pr.title}</b>
                      <Show when={pr.draft}><span class="chip">draft</span></Show>
                      <span class="muted small">{pr.author} · {pr.head_ref} → {pr.base_ref}</span>
                      <span style={{ flex: 1 }} />
                      <span class="muted small">{pr.files.length} markdown file{pr.files.length === 1 ? "" : "s"}{pr.touches.length ? ` · ${pr.touches.length} tracked` : ""}</span>
                    </div>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Show>
      </section>

    </div>
  );
};

export default App;
