import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { api, type RepoSummary } from "./api";

const App: Component = () => {
  const [repos, { refetch: refetchRepos }] = createResource(api.listRepos);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);

  async function run<T>(label: string, f: () => Promise<T>): Promise<T | undefined> {
    setBusy(label);
    setError(null);
    try {
      return await f();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div class="shell">
      <aside class="sidebar">
        <header class="brand">
          <span class="mark" />
          <span>kansa</span>
        </header>
        <RepoAdd
          disabled={busy() !== null}
          onAdd={async (gh) => {
            const r = await run(`cloning ${gh}`, () => api.registerRepo(gh));
            if (r) {
              await refetchRepos();
              setSelected(r.github);
            }
          }}
        />
        <nav class="repos">
          <Show when={repos()} fallback={<p class="muted">loading…</p>}>
            <For each={repos()} fallback={<p class="muted">No repos yet. Add one above.</p>}>
              {(r) => (
                <button
                  class="repo"
                  classList={{ active: selected() === r.github }}
                  onClick={() => setSelected(r.github)}
                >
                  <span class="name">{r.github}</span>
                  <span class="meta">
                    {r.default_branch} · {r.tracked.length} tracked
                  </span>
                </button>
              )}
            </For>
          </Show>
        </nav>
        <footer class="status-line">
          <Show when={busy()} fallback={<span class="muted">ready</span>}>
            <span class="spinner" /> {busy()}
          </Show>
        </footer>
      </aside>

      <main class="content">
        <Show when={error()}>
          <div class="toast error" onClick={() => setError(null)}>
            {error()}
          </div>
        </Show>
        <Show
          when={selected()}
          fallback={
            <div class="empty">
              <h1>Register a GitHub repo to begin</h1>
              <p>kansa reads markdown HLDs from a repo and never writes into it. State lives in your local kansa home.</p>
            </div>
          }
        >
          {(gh) => (
            <RepoPane
              github={gh()}
              repo={repos()?.find((r) => r.github === gh())}
              run={run}
              onChanged={refetchRepos}
            />
          )}
        </Show>
      </main>
    </div>
  );
};

const RepoAdd: Component<{ disabled: boolean; onAdd: (gh: string) => void }> = (p) => {
  const [value, setValue] = createSignal("");
  return (
    <form
      class="repo-add"
      onSubmit={(e) => {
        e.preventDefault();
        const v = value().trim();
        if (v) {
          p.onAdd(v);
          setValue("");
        }
      }}
    >
      <input
        placeholder="owner/name"
        value={value()}
        onInput={(e) => setValue(e.currentTarget.value)}
        disabled={p.disabled}
        spellcheck={false}
      />
      <button type="submit" disabled={p.disabled || !value().trim()}>
        Add
      </button>
    </form>
  );
};

const RepoPane: Component<{
  github: string;
  repo?: RepoSummary;
  run: <T>(label: string, f: () => Promise<T>) => Promise<T | undefined>;
  onChanged: () => void;
}> = (p) => {
  const [docs, { refetch: refetchDocs }] = createResource(() => p.github, api.listDocs);
  const [status, { refetch: refetchStatus }] = createResource(() => p.github, api.repoStatus);

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
      await Promise.all([refetchDocs(), refetchStatus()]);
      p.onChanged();
    }
  }

  return (
    <div class="repo-pane">
      <header class="pane-head">
        <div>
          <h1>{p.github}</h1>
          <p class="muted">
            {p.repo?.default_branch} · last fetch {p.repo?.last_fetch ?? "—"}
          </p>
        </div>
        <button onClick={refresh}>Refresh</button>
      </header>

      <section>
        <h2>Tracked HLDs</h2>
        <Show when={status()} fallback={<p class="muted">loading…</p>}>
          {(s) => (
            <table class="docs">
              <thead>
                <tr>
                  <th>Doc</th>
                  <th>Coverage</th>
                  <th>Residue</th>
                  <th>Questions</th>
                  <th>Round</th>
                </tr>
              </thead>
              <tbody>
                <For each={s().docs} fallback={<tr><td colSpan={5} class="muted">Nothing tracked yet — pick docs below.</td></tr>}>
                  {(d) => (
                    <tr>
                      <td class="mono">{d.doc}</td>
                      <td>
                        <Show when={d.meter} fallback="—">
                          {(m) => (
                            <span class="meter">
                              <span class="bar">
                                <span style={{ width: `${m().total ? (100 * m().classified) / m().total : 0}%` }} />
                              </span>
                              {m().classified}/{m().total}
                            </span>
                          )}
                        </Show>
                      </td>
                      <td classList={{ loud: (d.meter?.residue ?? 0) > 0 }}>{d.meter?.residue ?? "—"}</td>
                      <td>{d.meter?.open_questions ?? "—"}</td>
                      <td>{d.open_round ? `#${d.open_round} open` : `${d.rounds_closed} closed`}</td>
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
              reqs:{" "}
              {Object.entries(s().rollup.reqs_by_status)
                .map(([k, v]) => `${k} ${v}`)
                .join(" · ")}{" "}
              · open questions {s().rollup.open_questions} · groups {s().rollup.groups}
              <Show when={s().rollup.unexported_changes}>
                {" "}
                · <span class="loud">unexported changes</span>
              </Show>
            </p>
          )}
        </Show>
      </section>

      <section>
        <h2>Markdown docs on {p.repo?.default_branch}</h2>
        <Show when={docs()} fallback={<p class="muted">loading…</p>}>
          <ul class="doclist">
            <For each={docs()}>
              {(d) => (
                <li>
                  <label>
                    <input type="checkbox" checked={d.tracked} onChange={() => toggle(d.path, d.tracked)} />
                    <span class="mono">{d.path}</span>
                  </label>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </section>
    </div>
  );
};

export default App;
