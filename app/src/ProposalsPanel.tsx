import { createMemo, For, Show, type Component } from "solid-js";
import type { PrefillJob, Proposal } from "./api";

/** Agent proposals (spec `ui~agent-prefill~1`): visually distinct, each needs accept/reject. */
export const ProposalsPanel: Component<{
  job: PrefillJob | null;
  proposals: Proposal[];
  currentSpan: string | null;
  spanText: (id: string) => string | undefined;
  busy: boolean;
  onAccept: (id: string) => void;
  onReject: (id: string) => void;
  onAcceptAll: () => void;
  onClear: () => void;
  onFocus: (span: string | null) => void;
  onJump: (span: string) => void;
  onExit: () => void;
}> = (p) => {
  const open = createMemo(() => p.proposals.filter((x) => x.status === "proposed"));
  const decided = createMemo(() => p.proposals.filter((x) => x.status !== "proposed"));
  const kinds = createMemo(() => {
    let r = 0, c = 0, q = 0;
    for (const x of open()) x.proposed.kind === "req" ? r++ : x.proposed.kind === "context" ? c++ : q++;
    return { r, c, q };
  });
  return (
    <aside class="inv proposals">
      <div class="recon-head">
        <div class="row1">
          <b>✦ Agent proposals</b>
          <Show when={p.job?.state === "running"}><span class="muted small">batch {p.job!.done}/{p.job!.total} <span class="spinner" style={{ display: "inline-block", "vertical-align": "middle" }} /></span></Show>
          <Show when={p.job?.state === "error"}><span class="loud small" title={p.job!.error ?? ""}>failed</span></Show>
        </div>
        <div class="counts">
          <span class="vk reworded">{kinds().r} requirements</span>
          <span class="vk unchanged">{kinds().c} context</span>
          <span class="vk missing" style={{ background: "transparent", color: "var(--violet)", "border-color": "var(--violet)" }}>{kinds().q} questions</span>
          <span class="muted small">· {decided().length} decided</span>
        </div>
        <Show when={p.job?.state === "error"}><p class="loud small">{p.job!.error}</p></Show>
        <p class="muted small">Proposals are drafts. <kbd>⏎</kbd> accepts the proposal on the current sentence, <kbd>x</kbd> rejects it. Accepted items record <span class="mono">by: agent, accepted-by: you</span>.</p>
      </div>
      <div class="inv-list">
        <For each={open()} fallback={<div class="inv-empty">{p.job?.state === "running" ? "Waiting for the first batch…" : "No open proposals."}</div>}>
          {(x) => (
            <div class="proposal" classList={{ current: !!p.currentSpan && x.spans.includes(p.currentSpan) }} onMouseEnter={() => p.onFocus(x.spans[0])} onMouseLeave={() => p.onFocus(null)} onClick={() => p.onJump(x.spans[0])}>
              <div class="row1">
                <span class={`vk kind-${x.proposed.kind}`}>{x.proposed.kind === "req" ? (("attach" in x.proposed && x.proposed.attach) ? `attach → ${x.proposed.attach}` : "requirement") : x.proposed.kind}</span>
                <Show when={x.spans.length > 1}><span class="muted small">{x.spans.length} sentences</span></Show>
                <span style={{ flex: 1 }} />
                <button class="ghost small-btn" title="reject (x)" onClick={(e) => { e.stopPropagation(); p.onReject(x.id); }} disabled={p.busy}>✕</button>
                <button class="primary small-btn" title="accept (⏎)" onClick={(e) => { e.stopPropagation(); p.onAccept(x.id); }} disabled={p.busy}>Accept</button>
              </div>
              <div class="src">{x.spans.map((s) => p.spanText(s) ?? s).join(" ")}</div>
              <Show when={x.proposed.kind === "req" && "statement" in x.proposed && x.proposed.statement}>
                <div class="stmt">{(x.proposed as { statement: string }).statement}</div>
                <div class="meta muted small">
                  {(x.proposed as { pattern?: string | null }).pattern ?? ""}{(x.proposed as { slug?: string | null }).slug ? ` · req~${(x.proposed as { slug?: string }).slug}` : ""}
                  {(x.proposed as { groups: string[] }).groups.length ? ` · groups: ${(x.proposed as { groups: string[] }).groups.join(", ")}` : ""}
                </div>
              </Show>
              <Show when={x.proposed.kind === "question"}>
                <div class="meta small">{(x.proposed as { readings: string[] }).readings.map((r, i) => `${String.fromCharCode(97 + i)}) ${r}`).join("  ")}</div>
              </Show>
              <Show when={x.rationale}><div class="muted small">— {x.rationale}</div></Show>
            </div>
          )}
        </For>
      </div>
      <div class="recon-foot">
        <button onClick={p.onExit}>Back to inventory</button>
        <span style={{ flex: 1 }} />
        <button onClick={p.onClear} disabled={p.busy || !open().length} title="Discard all open proposals">Clear</button>
        <button class="primary" onClick={p.onAcceptAll} disabled={p.busy || !open().length}>Accept all {open().length}</button>
      </div>
    </aside>
  );
};
