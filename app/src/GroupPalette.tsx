import { makeDraggable } from "./drag";
import { createMemo, createSignal, For, onMount, type Component } from "solid-js";
import type { GroupRollup } from "./api";
import { slugOf } from "./Classifier";

/**
 * `g` — group quick-assign: fuzzy-find an existing group or create one inline.
 * One keystroke plus a name, not a form (`ui~grp-assign~1`). Picking a group every target
 * already belongs to removes them instead — same key, both directions.
 */
export const GroupPalette: Component<{
  groups: GroupRollup[];
  /** Requirement slugs being assigned (for the header). */
  targets: string[];
  onClose: () => void;
  onAssign: (groupSlug: string) => void;
  onUnassign: (groupSlug: string) => void;
  onCreate: (title: string) => void;
}> = (p) => {
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;
  let dlg: HTMLDivElement | undefined;
  let head: HTMLDivElement | undefined;
  onMount(() => { input?.focus(); if (dlg && head) makeDraggable(dlg, head, "group"); });

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
  /** How many of the selected requirements are already in this group. */
  const memberCount = (g: GroupRollup) => {
    const inGroup = new Set(g.group.members.map(slugOf));
    return p.targets.filter((t) => inGroup.has(t)).length;
  };
  const isFullMember = (g: GroupRollup) => p.targets.length > 0 && memberCount(g) === p.targets.length;

  const canCreate = () => query().trim().length > 0 && !matches().some((g) => g.group.title.toLowerCase() === query().trim().toLowerCase());
  const total = () => matches().length + (canCreate() ? 1 : 0);

  function choose(i: number) {
    if (i < matches().length) {
      const g = matches()[i];
      (isFullMember(g) ? p.onUnassign : p.onAssign)(slugOf(g.group.id));
    } else if (canCreate()) p.onCreate(query().trim());
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((a) => (a + 1) % Math.max(1, total())); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive((a) => (a - 1 + total()) % Math.max(1, total())); }
    else if (e.key === "Enter") { e.preventDefault(); choose(active()); }
  }

  return (
    <div class="overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div class="dialog" role="dialog" aria-modal="true" ref={dlg} aria-label="Assign to group" style={{ width: "min(520px, 92vw)" }}>
        <div class="dhead" ref={head} title="Drag to move · double-click to reset">
          <span class="title">Groups</span>
          <span class="quote mono">{p.targets.length === 1 ? `req~${p.targets[0]}` : `${p.targets.length} requirements`}</span>
        </div>
        <div class="dbody">
          <input ref={input} name="group-search" aria-label="Group name — pick to add, pick again to remove, Enter to create" placeholder="Type a group name — pick to add, pick again to remove, Enter to create" value={query()} onInput={(e) => { setQuery(e.currentTarget.value); setActive(0); }} onKeyDown={onKey} />
          <div class="matches">
            <For each={matches()}>
              {(g, i) => (
                <div class="match" classList={{ active: active() === i(), remove: isFullMember(g) }} onMouseEnter={() => setActive(i())} onClick={() => choose(i())}
                  title={isFullMember(g) ? "Already in this group — pick to remove" : undefined}>
                  <span class="stmt" style={{ "font-family": "var(--sans)" }}>{g.group.title}</span>
                  {isFullMember(g)
                    ? <span class="id memberstate">remove</span>
                    : memberCount(g) > 0 && <span class="id memberstate">{memberCount(g)} of {p.targets.length} in</span>}
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
          <span class="hint"><span><kbd>↑↓</kbd> choose</span><span><kbd>⏎</kbd> add / remove</span><span><kbd>esc</kbd> cancel</span></span>
          <button onClick={p.onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
};
