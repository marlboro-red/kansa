import { makeDraggable } from "./drag";
import { createSignal, For, onMount, type Component } from "solid-js";
import { createStore } from "solid-js/store";
import type { Level } from "./api";

/** `q` — flag the selection as a question: quote, materiality, candidate readings, default. */
export const QuestionDialog: Component<{
  quote: string;
  onClose: () => void;
  onSubmit: (q: { quote: string; materiality: Level; readings: { key: string; text: string }[]; default?: string }) => void;
}> = (p) => {
  const [quote, setQuote] = createSignal(p.quote);
  const [materiality, setMateriality] = createSignal<Level>("M");
  const [readings, setReadings] = createStore<{ key: string; text: string }[]>([
    { key: "a", text: "" },
    { key: "b", text: "" },
  ]);
  const [def, setDef] = createSignal<string | undefined>(undefined);
  let first: HTMLInputElement | undefined;
  let dlg: HTMLDivElement | undefined;
  let head: HTMLDivElement | undefined;
  onMount(() => { first?.focus(); if (dlg && head) makeDraggable(dlg, head, "question"); });

  const canSubmit = () => quote().trim().length > 0;
  function submit() {
    if (!canSubmit()) return;
    const rs = readings.filter((r) => r.text.trim()).map((r) => ({ key: r.key, text: r.text.trim() }));
    p.onSubmit({ quote: quote().trim(), materiality: materiality(), readings: rs, default: rs.some((r) => r.key === def()) ? def() : undefined });
  }
  function addReading() {
    const key = String.fromCharCode(97 + readings.length);
    setReadings(readings.length, { key, text: "" });
  }

  return (
    <div class="overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div
        class="dialog"
        role="dialog"
        aria-modal="true"
        ref={dlg}
        aria-label="Flag as question"
        onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submit(); } }}
      >
        <div class="dhead" ref={head} title="Drag to move · double-click to reset"><span class="title">Flag as question</span></div>
        <div class="dbody">
          <div class="field">
            <label>Quoted prose</label>
            <textarea name="quote" aria-label="Quoted prose" value={quote()} onInput={(e) => setQuote(e.currentTarget.value)} rows={2} />
          </div>
          <div class="field">
            <label>Materiality — how much does the answer change the inventory?</label>
            <div class="seg" role="radiogroup">
              <For each={["H", "M", "L"] as Level[]}>
                {(l) => <button type="button" classList={{ on: materiality() === l }} onClick={() => setMateriality(l)}>{l === "H" ? "High" : l === "M" ? "Medium" : "Low"}</button>}
              </For>
            </div>
          </div>
          <div class="field">
            <label>Possible readings <span class="muted" style={{ "text-transform": "none", "letter-spacing": 0 }}>— pick the default you'd assume if unanswered</span></label>
            <div class="readings">
              <For each={readings}>
                {(r, i) => (
                  <div class="reading">
                    <input type="radio" name="def" aria-label={`Reading ${r.key} is the default`} checked={def() === r.key} onChange={() => setDef(r.key)} title="default reading" disabled={!r.text.trim()} />
                    <span class="key">{r.key}</span>
                    <input
                      ref={i() === 0 ? first : undefined}
                      name={`reading-${r.key}`}
                      aria-label={`Reading ${r.key}`}
                      value={r.text}
                      placeholder={i() === 0 ? "e.g. 10-minute sliding window" : "another interpretation"}
                      onInput={(e) => setReadings(i(), "text", e.currentTarget.value)}
                    />
                  </div>
                )}
              </For>
              <div><button type="button" class="ghost" onClick={addReading}>+ reading</button></div>
            </div>
          </div>
        </div>
        <div class="dfoot">
          <span class="hint"><span><kbd>⌘⏎</kbd> raise</span><span><kbd>esc</kbd> cancel</span></span>
          <button onClick={p.onClose}>Cancel</button>
          <button class="primary" onClick={submit} disabled={!canSubmit()}>Raise question</button>
        </div>
      </div>
    </div>
  );
};
