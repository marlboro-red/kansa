import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import type { Req } from "./api";
import { slugOf } from "./Classifier";
import type { Toast } from "./Classifier";
import { ReqDrawer } from "./ReqDrawer";

/** Right pane of the classifier: this doc's requirements, search, and a detail drawer. */
export const Inventory: Component<{
  github: string;
  doc: string;
  reqs: Req[];
  groupsByReq: Map<string, string[]>;
  linked: string | null;
  currentReqIds: string[];
  onSelect: (slug: string) => void;
  onJumpSpan: (spanId: string) => void;
  onChanged: () => void;
  onGroup: (slug: string) => void;
  toast: (t: Toast | null) => void;
}> = (p) => {
  const [q, setQ] = createSignal("");
  const [showAll, setShowAll] = createSignal(false);

  const docReqs = createMemo(() => {
    const inDoc = (r: Req) => r.anchors.some((a) => a.doc === p.doc);
    const base = showAll() ? p.reqs : p.reqs.filter((r) => inDoc(r) || r.anchors.length === 0);
    const query = q().trim().toLowerCase();
    const filtered = query ? base.filter((r) => r.id.toLowerCase().includes(query) || r.statement.toLowerCase().includes(query)) : base;
    return [...filtered].sort((a, b) => (b.history[0]?.at ?? "").localeCompare(a.history[0]?.at ?? ""));
  });
  const selected = createMemo(() => (p.linked ? p.reqs.find((r) => slugOf(r.id) === p.linked) ?? null : null));

  return (
    <aside class="inv">
      <div class="inv-head">
        <input placeholder="Filter requirements…" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
        <button class="ghost" onClick={() => setShowAll(!showAll())} title={showAll() ? "Showing every requirement in the repo" : "Showing this doc's requirements"}>
          {showAll() ? "repo" : "doc"}
        </button>
        <span class="count">{docReqs().length}</span>
      </div>
      <div class="inv-list">
        <For each={docReqs()} fallback={<div class="inv-empty">No requirements yet.<br />Select a sentence and press <kbd>r</kbd>.</div>}>
          {(r) => (
            <button
              class="req"
              classList={{ active: p.linked === slugOf(r.id), linked: p.currentReqIds.includes(r.id) }}
              onClick={() => p.onSelect(slugOf(r.id))}
            >
              <div class="row1">
                <span class="id">{r.id}</span>
                <Show when={r.suspect}><span class="chip suspect" title={r.suspect!}>suspect</span></Show>
                <span class={`chip status-${r.status}`}>{r.status}</span>
              </div>
              <div class="stmt">{r.statement}</div>
              <div class="row3">
                <Show when={r.pattern}><span>{r.pattern}</span></Show>
                <span>{r.anchors.length} anchor{r.anchors.length === 1 ? "" : "s"}</span>
                <Show when={p.groupsByReq.get(slugOf(r.id))?.length}><span>{p.groupsByReq.get(slugOf(r.id))!.join(", ")}</span></Show>
                <Show when={r.questions.length}><span class="qbadge">? {r.questions.length}</span></Show>
                <Show when={r.notes?.length}><span class="nbadge" title="notes">✎ {r.notes!.length}</span></Show>
              </div>
            </button>
          )}
        </For>
      </div>
      <Show when={selected()}>
        {(r) => (
          <ReqDrawer
            github={p.github}
            req={r()}
            groups={p.groupsByReq.get(slugOf(r().id))}
            onJumpAnchor={(_doc, span) => p.onJumpSpan(span)}
            onChanged={p.onChanged}
            onClose={() => p.onSelect("")}
            onGroup={() => p.onGroup(slugOf(r().id))}
            toast={p.toast}
          />
        )}
      </Show>
    </aside>
  );
};
