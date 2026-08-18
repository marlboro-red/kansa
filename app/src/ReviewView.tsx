import { createEffect, createMemo, createResource, createSignal, For, Show, type Component } from "solid-js";

/** "show resolved" toggle per repo (session-lived). */
const showClosedMem = new Map<string, boolean>();
import { api, type Question, type RepoStatus, type Round } from "./api";
import { slugOf } from "./Classifier";
import type { Toast } from "./Classifier";
import { createCached } from "./swr";

/** Review view (spec §4.4): question queue, round timeline per doc, pending reconciliations. */
export const ReviewView: Component<{
  github: string;
  label?: string;
  onOpenAnchor: (doc: string, span: string) => void;
  onOpenDoc: (doc: string) => void;
  toast: (t: Toast | null) => void;
}> = (p) => {
  const [questions, { refetch: refetchQ }] = createCached(() => `questions:${p.github}`, () => api.listQuestions(p.github));
  const [status] = createCached(() => `status:${p.github}`, () => api.repoStatus(p.github));
  const [showClosed, setShowClosed] = createSignal(showClosedMem.get(p.github) ?? false);
  createEffect(() => showClosedMem.set(p.github, showClosed()));

  const open = createMemo(() => (questions() ?? []).filter((q) => q.status === "open").sort((a, b) => lvl(b.materiality) - lvl(a.materiality)));
  const closed = createMemo(() => (questions() ?? []).filter((q) => q.status !== "open"));

  return (
    <div class="review">
      <header class="invview-head">
        <div>
          <h1>Review <span class="muted">· {p.label ?? p.github}</span></h1>
          <p class="rollup muted">{open().length} open question{open().length === 1 ? "" : "s"} · {closed().length} resolved</p>
        </div>
      </header>
      <div class="review-body">
        <section class="qcol">
          <h2>Question queue</h2>
          <For each={open()} fallback={<div class="inv-empty">No open questions. Flag ambiguous prose with <kbd>q</kbd> in a doc.</div>}>
            {(q) => <QuestionCard q={q} github={p.github} onChanged={refetchQ} onOpenAnchor={p.onOpenAnchor} toast={p.toast} />}
          </For>
          <Show when={closed().length}>
            <button class="ghost" onClick={() => setShowClosed(!showClosed())}>{showClosed() ? "hide" : "show"} {closed().length} resolved</button>
            <Show when={showClosed()}>
              <For each={closed()}>{(q) => <QuestionCard q={q} github={p.github} onChanged={refetchQ} onOpenAnchor={p.onOpenAnchor} toast={p.toast} />}</For>
            </Show>
          </Show>
        </section>
        <section class="rcol">
          <h2>Rounds</h2>
          <Show when={status()} fallback={<p class="muted">loading…</p>}>
            {(s) => <For each={s().docs}>{(d) => <DocRounds github={p.github} doc={d.doc} status={s()} onOpenDoc={p.onOpenDoc} />}</For>}
          </Show>
        </section>
      </div>
    </div>
  );
};

function lvl(l: string) { return l === "H" ? 3 : l === "M" ? 2 : 1; }

const QuestionCard: Component<{ q: Question; github: string; onChanged: () => void; onOpenAnchor: (doc: string, span: string) => void; toast: (t: Toast | null) => void }> = (p) => {
  const [reading, setReading] = createSignal(p.q.readings.find((r) => r.default)?.key ?? p.q.readings[0]?.key ?? "");
  const [free, setFree] = createSignal("");
  const [note, setNote] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const answerKey = () => (p.q.readings.length ? reading() : free().trim());

  async function run(f: () => Promise<unknown>) {
    setBusy(true);
    try { await f(); p.onChanged(); } catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); }
  }

  return (
    <div class="qcard" classList={{ resolved: p.q.status !== "open", held: !!p.q.pending }}>
      <div class="row1">
        <span class="mono muted">{p.q.id}</span>
        <span class={`chip mat-${p.q.materiality}`}>{p.q.materiality === "H" ? "high" : p.q.materiality === "M" ? "medium" : "low"} materiality</span>
        <Show when={p.q.status !== "open"}><span class="chip">{p.q.status}</span></Show>
        <span style={{ flex: 1 }} />
        <For each={p.q.anchors}>{(a) => <span class="anchor mono" onClick={() => p.onOpenAnchor(a.doc, a.span)}>{a.doc.split("/").pop()}:{a.span.slice(2, 8)}</span>}</For>
      </div>
      <blockquote class="quote">“{p.q.quote}”</blockquote>
      <Show when={p.q.affects.length}><div class="small muted">affects <For each={p.q.affects}>{(a) => <span class="chip">{a}</span>}</For></div></Show>
      <Show when={p.q.status === "open"}>
        <Show when={p.q.pending} fallback={
          <div class="answer">
            <Show when={p.q.readings.length} fallback={<input name="answer" aria-label="Answer" placeholder="Answer…" value={free()} onInput={(e) => setFree(e.currentTarget.value)} />}>
              <div class="readings">
                <For each={p.q.readings}>
                  {(r) => (
                    <label class="reading">
                      <input type="radio" name={`ans-${p.q.id}`} checked={reading() === r.key} onChange={() => setReading(r.key)} />
                      <span class="key">{r.key}</span>
                      <span>{r.text}</span>
                      <Show when={r.default}><span class="muted small">(default)</span></Show>
                    </label>
                  )}
                </For>
              </div>
            </Show>
            <div class="row">
              <input name="answer-note" aria-label="Answer note (optional) — who decided, where" placeholder="Note (optional) — who decided, where" value={note()} onInput={(e) => setNote(e.currentTarget.value)} />
              <button
                class="primary"
                disabled={busy() || !answerKey()}
                onClick={() => run(async () => {
                  await api.answerQuestion(p.github, slugOf(p.q.id), answerKey(), note().trim() || undefined);
                  p.toast({ kind: "info", text: "Answered. The flagged sentence is residue again — classify it in the doc to reflect the answer." });
                })}
              >Answer</button>
              <button disabled={busy()} onClick={() => run(() => api.withdrawQuestion(p.github, slugOf(p.q.id)))}>Withdraw</button>
            </div>
          </div>
        }>
          {(held) => (
            <div class="heldbox">
              <b>Answer held</b> — a linked requirement changed rev since this question was raised. Held answer: <span class="mono">{held().reading}</span>{held().note ? ` · ${held().note}` : ""}.
              <div class="row">
                <button class="primary" disabled={busy()} onClick={() => run(() => api.resolveHeldAnswer(p.github, slugOf(p.q.id), true))}>Apply anyway</button>
                <button disabled={busy()} onClick={() => run(() => api.resolveHeldAnswer(p.github, slugOf(p.q.id), false))}>Discard</button>
              </div>
            </div>
          )}
        </Show>
      </Show>
      <Show when={p.q.answer}>{(a) => <div class="small">answered <b>{a().reading}</b>{a().note ? ` — ${a().note}` : ""} · {a().by} · {a().at.slice(0, 16).replace("T", " ")}</div>}</Show>
    </div>
  );
};

const DocRounds: Component<{ github: string; doc: string; status: RepoStatus; onOpenDoc: (doc: string) => void }> = (p) => {
  const [rounds] = createResource(() => [p.github, p.doc] as const, ([g, d]) => api.rounds(g, d));
  const ds = () => p.status.docs.find((d) => d.doc === p.doc);
  return (
    <div class="docrounds">
      <div class="row1">
        <span class="mono">{p.doc}</span>
        <span style={{ flex: 1 }} />
        <button class="ghost" onClick={() => p.onOpenDoc(p.doc)}>open</button>
      </div>
      <Show when={ds()?.meter}>{(m) => <div class="small muted">{m().classified}/{m().total} classified · residue <b classList={{ loud: m().residue > 0 }}>{m().residue}</b></div>}</Show>
      <ol class="timeline">
        <For each={[...(rounds() ?? [])].reverse()} fallback={<li class="muted small">no rounds yet</li>}>
          {(r) => <RoundItem r={r} />}
        </For>
      </ol>
    </div>
  );
};

const RoundItem: Component<{ r: Round & { summary?: { created?: string[]; changed?: string[]; retired?: string[]; verdicts?: Record<string, number> } | null } }> = (p) => (
  <li classList={{ open: !p.r.closed }}>
    <div class="row1">
      <b>#{p.r.n}</b>
      <span class="mono muted">{p.r.snapshot.slice(0, 7)}</span>
      <span class="muted small">{p.r.opened.slice(0, 16).replace("T", " ")}{p.r.closed ? ` → ${p.r.closed.slice(0, 16).replace("T", " ")}` : " · open"}</span>
    </div>
    <Show when={p.r.summary}>
      {(s) => (
        <div class="small muted">
          created {s().created?.length ?? 0} · changed {s().changed?.length ?? 0} · retired {s().retired?.length ?? 0}
          <Show when={s().verdicts}>{(v) => <span> · verdicts {Object.entries(v()).map(([k, n]) => `${k} ${n}`).join(", ")}</span>}</Show>
        </div>
      )}
    </Show>
  </li>
);
