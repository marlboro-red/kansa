import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import type { Decision, Reconciliation, Verdict, VerdictKind } from "./api";

const ORDER: Record<VerdictKind, number> = { missing: 0, "meaning-changed": 1, reworded: 2, unchanged: 3 };

/**
 * Reconciliation review (spec §4.4): one row per classified span of the old snapshot with the
 * proposed mapping into the new one. Humans decide `reworded`/`missing`; `unchanged` is
 * accepted automatically. In a PR context the list is read-only ("what merging would change").
 */
export const ReconcilePanel: Component<{
  recon: Reconciliation;
  /** id → text for `recon.added` (from the incoming snapshot). */
  addedText: (id: string) => string | undefined;
  readOnly: boolean;
  picking: string | null;
  focus: string | null;
  onDecide: (from: string, d: Decision) => void;
  onPick: (from: string | null) => void;
  onFocus: (span: string | null) => void;
  onConfirm: () => void;
  onExit: () => void;
  busy: boolean;
}> = (p) => {
  const [showUnchanged, setShowUnchanged] = createSignal(false);
  const [retireFor, setRetireFor] = createSignal<string | null>(null);
  const [reason, setReason] = createSignal("");

  const sorted = createMemo(() =>
    [...p.recon.verdicts].sort((a, b) => ORDER[a.kind] - ORDER[b.kind]).filter((v) => showUnchanged() || v.kind !== "unchanged"),
  );
  const counts = createMemo(() => {
    const c: Record<VerdictKind, number> = { unchanged: 0, reworded: 0, "meaning-changed": 0, missing: 0 };
    for (const v of p.recon.verdicts) c[v.kind]++;
    return c;
  });
  const undecided = () => p.recon.verdicts.filter((v) => v.kind !== "unchanged" && !v.decision).length;

  return (
    <aside class="inv recon">
      <div class="recon-head">
        <div class="row1">
          <b>{p.readOnly ? "Changes vs base" : "Doc changed upstream"}</b>
          <span class="mono muted">{p.recon.from.slice(0, 7)} → {p.recon.to.slice(0, 7)}</span>
        </div>
        <div class="counts">
          <span class="vk unchanged">{counts().unchanged} unchanged</span>
          <span class="vk reworded">{counts().reworded} reworded</span>
          <span class="vk meaning-changed">{counts()["meaning-changed"]} meaning</span>
          <span class="vk missing">{counts().missing} missing</span>
          <span class="vk added">{p.recon.added.length} new</span>
        </div>
        <Show when={!p.readOnly} fallback={<p class="muted small">Read-only in a PR: anchors follow sentence content, so unchanged text keeps its classification when merged; new text shows as residue.</p>}>
          <p class="muted small">You are viewing the incoming text. Decide each changed sentence, then confirm to close the current round and adopt it. Nothing is rewritten until you confirm.</p>
        </Show>
        <label class="small toggle"><input type="checkbox" checked={showUnchanged()} onChange={(e) => setShowUnchanged(e.currentTarget.checked)} /> show unchanged</label>
      </div>
      <div class="inv-list">
        <Show when={p.recon.added.length}>
          <div class="addedlist">
            <div class="small muted" style={{ "margin-bottom": "4px" }}>New sentences (become residue)</div>
            <For each={p.recon.added}>
              {(id) => <div class="added" onMouseEnter={() => p.onFocus(id)} onMouseLeave={() => p.onFocus(null)}>{p.addedText(id) ?? id}</div>}
            </For>
          </div>
        </Show>
        <For each={sorted()} fallback={<div class="inv-empty">Nothing changed for classified sentences.</div>}>
          {(v) => (
            <div class="verdict" classList={{ decided: !!v.decision && v.kind !== "unchanged", focus: p.focus === (v.to ?? v.from), picking: p.picking === v.from }} onMouseEnter={() => p.onFocus(v.to ?? null)} onMouseLeave={() => p.onFocus(null)}>
              <div class="row1">
                <span class={`vk ${v.kind}`}>{v.kind}</span>
                <Show when={v.kind !== "unchanged" && v.kind !== "missing"}><span class="muted mono">{Math.round(v.similarity * 100)}%</span></Show>
                <span style={{ flex: 1 }} />
                <Show when={v.decision}>{(d) => <span class="chip decision">{label(d())}</span>}</Show>
              </div>
              <div class="from">{v.from_text}</div>
              <Show when={v.to_text} fallback={<div class="to missing-to">— no matching sentence in the new text —</div>}>
                <div class="to">{v.to_text}</div>
              </Show>
              <div class="links">
                <For each={v.reqs}>{(r) => <span class="chip">{r}</span>}</For>
                <For each={v.questions}>{(q) => <span class="chip qchip">{q}</span>}</For>
                <Show when={v.non_normative}><span class="chip">context</span></Show>
              </div>
              <Show when={!p.readOnly && v.kind !== "unchanged"}>
                <div class="actions">
                  <Show when={v.to}>
                    <button classList={{ on: v.decision?.kind === "accept" }} onClick={() => p.onDecide(v.from, { kind: "accept" })} disabled={p.busy}>Same meaning</button>
                    <button classList={{ on: v.decision?.kind === "meaning-changed" }} onClick={() => p.onDecide(v.from, { kind: "meaning-changed" })} disabled={p.busy}>Meaning changed</button>
                  </Show>
                  <button classList={{ on: p.picking === v.from || v.decision?.kind === "reanchor" }} onClick={() => p.onPick(p.picking === v.from ? null : v.from)} disabled={p.busy}>
                    {p.picking === v.from ? "click a sentence…" : "Re-anchor"}
                  </button>
                  <button classList={{ on: v.decision?.kind === "drop" }} onClick={() => p.onDecide(v.from, { kind: "drop" })} disabled={p.busy}>Drop anchor</button>
                  <Show when={v.reqs.length}>
                    <button classList={{ on: v.decision?.kind === "retire" }} onClick={() => setRetireFor(retireFor() === v.from ? null : v.from)} disabled={p.busy}>Retire…</button>
                  </Show>
                </div>
                <Show when={retireFor() === v.from}>
                  <div class="retire">
                    <input placeholder="reason (required)" value={reason()} onInput={(e) => setReason(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter" && reason().trim()) { p.onDecide(v.from, { kind: "retire", reason: reason().trim() }); setRetireFor(null); setReason(""); } }} />
                    <button class="primary" disabled={!reason().trim()} onClick={() => { p.onDecide(v.from, { kind: "retire", reason: reason().trim() }); setRetireFor(null); setReason(""); }}>Retire</button>
                  </div>
                </Show>
              </Show>
            </div>
          )}
        </For>
      </div>
      <div class="recon-foot">
        <button onClick={p.onExit}>{p.readOnly ? "Close" : "Back to current text"}</button>
        <Show when={!p.readOnly}>
          <span style={{ flex: 1 }} />
          <span class="muted small">{undecided() ? `${undecided()} to decide` : "all decided"}</span>
          <button class="primary" disabled={undecided() > 0 || p.busy} onClick={p.onConfirm} title={undecided() ? "Decide every changed sentence first" : "Close the current round and adopt the new text"}>
            Confirm &amp; adopt
          </button>
        </Show>
      </div>
    </aside>
  );
};

function label(d: Decision) {
  switch (d.kind) {
    case "accept": return "same meaning";
    case "meaning-changed": return "meaning changed";
    case "reanchor": return "re-anchored";
    case "drop": return "dropped";
    case "retire": return "retire";
  }
}
export type { Verdict };
