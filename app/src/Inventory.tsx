import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import { api, type Req, type Status } from "./api";
import { slugOf } from "./Classifier";
import type { Toast } from "./Classifier";

const STATUSES: Status[] = ["extracted", "assumed", "confirmed", "disputed", "retired"];

/** Right pane: this doc's requirements, search, and a detail drawer for the selected one. */
export const Inventory: Component<{
  github: string;
  doc: string;
  reqs: Req[];
  linked: string | null;
  currentReqIds: string[];
  onSelect: (slug: string) => void;
  onJumpSpan: (spanId: string) => void;
  onChanged: () => void;
  toast: (t: Toast | null) => void;
}> = (p) => {
  const [q, setQ] = createSignal("");
  const [showAll, setShowAll] = createSignal(false);
  const [retireReason, setRetireReason] = createSignal("");
  const [retiring, setRetiring] = createSignal(false);

  const docReqs = createMemo(() => {
    const inDoc = (r: Req) => r.anchors.some((a) => a.doc === p.doc);
    const base = showAll() ? p.reqs : p.reqs.filter(inDoc);
    const query = q().trim().toLowerCase();
    const filtered = query ? base.filter((r) => r.id.toLowerCase().includes(query) || r.statement.toLowerCase().includes(query)) : base;
    // newest first by first history entry
    return [...filtered].sort((a, b) => (b.history[0]?.at ?? "").localeCompare(a.history[0]?.at ?? ""));
  });
  const selected = createMemo(() => (p.linked ? p.reqs.find((r) => slugOf(r.id) === p.linked) ?? null : null));

  async function setStatus(r: Req, status: Status) {
    try {
      if (status === "retired") {
        if (!retireReason().trim()) { setRetiring(true); return; }
        await api.updateReq(p.github, slugOf(r.id), { status, reason: retireReason().trim() });
        setRetiring(false);
        setRetireReason("");
      } else {
        await api.updateReq(p.github, slugOf(r.id), { status });
      }
      p.onChanged();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    }
  }

  return (
    <aside class="inv">
      <div class="inv-head">
        <input placeholder="Filter requirements…" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
        <button class="ghost" classList={{ on: showAll() }} onClick={() => setShowAll(!showAll())} title={showAll() ? "Showing all repo requirements" : "Showing this doc's requirements"}>
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
                <span class={`chip status-${r.status}`}>{r.status}</span>
              </div>
              <div class="stmt">{r.statement}</div>
              <div class="row3">
                <Show when={r.pattern}><span>{r.pattern}</span></Show>
                <span>{r.anchors.length} anchor{r.anchors.length === 1 ? "" : "s"}</span>
                <Show when={r.questions.length}><span class="qbadge">? {r.questions.length}</span></Show>
              </div>
            </button>
          )}
        </For>
      </div>
      <Show when={selected()}>
        {(r) => (
          <div class="drawer">
            <div class="row1" style={{ display: "flex", gap: "8px", "align-items": "center" }}>
              <span class="mono">{r().id}</span>
              <span class={`chip status-${r().status}`}>{r().status}</span>
              <span style={{ flex: 1 }} />
              <button class="ghost" onClick={() => p.onSelect("")} title="close">✕</button>
            </div>
            <div class="stmt">{r().statement}</div>
            <dl class="kv">
              <dt>pattern</dt><dd>{r().pattern ?? <span class="muted">—</span>}</dd>
              <dt>anchors</dt>
              <dd>
                <For each={r().anchors}>
                  {(a) => <span class="anchor mono" onClick={() => p.onJumpSpan(a.span)} title={a.doc}>{a.span}{" "}</span>}
                </For>
              </dd>
              <Show when={r().reason}><dt>reason</dt><dd>{r().reason}</dd></Show>
            </dl>
            <div class="actions">
              <For each={STATUSES.filter((s) => s !== r().status)}>
                {(s) => <button onClick={() => setStatus(r(), s)}>{s === "retired" ? "retire…" : s}</button>}
              </For>
            </div>
            <Show when={retiring()}>
              <div class="field" style={{ "margin-top": "8px" }}>
                <label>Why retire? (required)</label>
                <div style={{ display: "flex", gap: "6px" }}>
                  <input value={retireReason()} onInput={(e) => setRetireReason(e.currentTarget.value)} placeholder="e.g. superseded by req~x~2" onKeyDown={(e) => { if (e.key === "Enter") setStatus(r(), "retired"); }} />
                  <button class="primary" disabled={!retireReason().trim()} onClick={() => setStatus(r(), "retired")}>Retire</button>
                </div>
              </div>
            </Show>
            <ul class="hist">
              <For each={r().history.slice(-5).reverse()}>{(h) => <li>{h.at.slice(0, 16).replace("T", " ")} · {h.by} · {h.op}{h.note ? ` · ${h.note}` : ""}</li>}</For>
            </ul>
          </div>
        )}
      </Show>
    </aside>
  );
};
