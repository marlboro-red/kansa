import { createMemo, createSignal, For, onMount, type Component } from "solid-js";
import type { GroupRollup } from "./api";
import { slugOf } from "./Classifier";

/**
 * `g` — group quick-assign: fuzzy-find an existing group or create one inline.
 * One keystroke plus a name, not a form (`ui~grp-assign~1`).
 */
export const GroupPalette: Component<{
  groups: GroupRollup[];
  /** Requirement slugs being assigned (for the header). */
  targets: string[];
  onClose: () => void;
  onAssign: (groupSlug: string) => void;
  onCreate: (title: string) => void;
}> = (p) => {
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;
  onMount(() => input?.focus());

  const matches = createMemo(() => {
    const q = query().trim().toLowerCase();
    const gs = p.groups;
    if (!q) return gs;
    return gs
      .map((g) => ({ g, s: g.group.title.toLowerCase().includes(q) ? 2 : g.group.id.includes(q) ? 1 : 0 }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.g);
  });
  const canCreate = () => query().trim().length > 0 && !matches().some((g) => g.group.title.toLowerCase() === query().trim().toLowerCase());
  const total = () => matches().length + (canCreate() ? 1 : 0);

  function choose(i: number) {
    if (i < matches().length) p.onAssign(slugOf(matches()[i].group.id));
    else if (canCreate()) p.onCreate(query().trim());
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((a) => (a + 1) % Math.max(1, total())); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive((a) => (a - 1 + total()) % Math.max(1, total())); }
    else if (e.key === "Enter") { e.preventDefault(); choose(active()); }
  }

  return (
    <div class="overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div class="dialog" role="dialog" aria-label="Assign to group" style={{ width: "min(520px, 92vw)" }}>
        <div class="dhead">
          <span class="title">Add to group</span>
          <span class="quote mono">{p.targets.length === 1 ? `req~${p.targets[0]}` : `${p.targets.length} requirements`}</span>
        </div>
        <div class="dbody">
          <input ref={input} placeholder="Type a group name — pick one, or press Enter to create it" value={query()} onInput={(e) => { setQuery(e.currentTarget.value); setActive(0); }} onKeyDown={onKey} />
          <div class="matches">
            <For each={matches()}>
              {(g, i) => (
                <div class="match" classList={{ active: active() === i() }} onMouseEnter={() => setActive(i())} onClick={() => choose(i())}>
                  <span class="stmt" style={{ "font-family": "var(--sans)" }}>{g.group.title}</span>
                  <span class="id">{g.group.members.length} member{g.group.members.length === 1 ? "" : "s"}</span>
                </div>
              )}
            </For>
            {canCreate() && (
              <div class="match create" classList={{ active: active() === matches().length }} onMouseEnter={() => setActive(matches().length)} onClick={() => choose(matches().length)}>
                + Create group “{query().trim()}”
              </div>
            )}
            {matches().length === 0 && !canCreate() && <div class="match muted">No groups yet — type a name to create one.</div>}
          </div>
        </div>
        <div class="dfoot">
          <span class="hint"><span><kbd>↑↓</kbd> choose</span><span><kbd>⏎</kbd> assign</span><span><kbd>esc</kbd> cancel</span></span>
          <button onClick={p.onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
};
