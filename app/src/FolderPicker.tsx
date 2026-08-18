import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { api } from "./api";

/**
 * Browser-mode folder picker: navigates the server's filesystem via `list_dirs` (the server
 * only ever binds 127.0.0.1, so this browses the machine kansa runs on). Tauri builds use the
 * native dialog instead.
 */
export const FolderPicker: Component<{ onClose: () => void; onPick: (path: string) => void }> = (p) => {
  const [path, setPath] = createSignal<string | undefined>(undefined);
  const [typed, setTyped] = createSignal("");
  const [listing] = createResource(
    () => path() ?? "",
    (cur) => api.listDirs(cur || undefined),
  );
  const go = (next: string) => { setPath(next); setTyped(""); };

  return (
    <div class="overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div
        class="dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Open a local folder"
        style={{ width: "min(560px, 92vw)" }}
        onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); p.onClose(); } }}
      >
        <div class="dhead"><span class="title">Open a local folder</span></div>
        <div class="dbody">
          <input
            name="folder-path"
            aria-label="Folder path"
            class="mono"
            placeholder={listing()?.path ?? "type a path and press Enter"}
            value={typed()}
            spellcheck={false}
            onInput={(e) => setTyped(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && typed().trim()) go(typed().trim()); }}
          />
          <Show when={listing.error}>
            <p class="loud small" style={{ margin: 0 }}>{String(listing.error)}</p>
          </Show>
          <Show when={listing()} fallback={<p class="muted">loading…</p>}>
            {(l) => (
              <>
                <div class="mono muted small" style={{ "overflow-wrap": "anywhere" }}>{l().path}</div>
                <div class="matches" style={{ "max-height": "260px" }}>
                  <Show when={l().parent}>
                    <div class="match" onClick={() => go(l().parent!)}><span class="mono">..</span></div>
                  </Show>
                  <For each={l().dirs} fallback={<div class="match muted">no subfolders</div>}>
                    {(d) => <div class="match" onClick={() => go(d.path)}><span class="mono">{d.name}/</span></div>}
                  </For>
                </div>
              </>
            )}
          </Show>
        </div>
        <div class="dfoot">
          <span class="hint">
            <Show when={listing()}>{(l) => <span>{l().markdown_files} markdown file{l().markdown_files === 1 ? "" : "s"} here</span>}</Show>
          </span>
          <button onClick={p.onClose}>Cancel</button>
          <button class="primary" disabled={!listing()} onClick={() => listing() && p.onPick(listing()!.path)}>
            Use this folder
          </button>
        </div>
      </div>
    </div>
  );
};
