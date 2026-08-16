import {
  createEffect,
  createMemo,
  createResource,
  createSignal,
  For,
  Index,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { createStore, produce } from "solid-js/store";
import { Dynamic } from "solid-js/web";
import { api, type DocView, type Req, type Span, type SpanState, type SpanStatus, type Pattern, type Level } from "./api";
import { ReqPalette } from "./ReqPalette";
import { QuestionDialog } from "./QuestionDialog";
import { Inventory } from "./Inventory";
import { GroupPalette } from "./GroupPalette";
import { ReconcilePanel } from "./ReconcilePanel";
import type { Context, Decision } from "./api";

export type Toast = { kind: "error" | "info"; text: string };

type Props = {
  github: string;
  doc: string;
  /** Land on this span (click-through from the inventory). */
  initialSpan?: string;
  /** PR context; default branch when absent. */
  context?: Context;
  onBack: () => void;
  toast: (t: Toast | null) => void;
};

/** Everything the classifier knows about one span, merged with optimistic patches. */
export type SpanRow = { span: Span; status: SpanStatus; pending: boolean };

export const Classifier: Component<Props> = (p) => {
  const ctx = () => p.context;
  const isPr = () => !!p.context && "pr" in p.context;
  const [view, { mutate }] = createResource(() => [p.github, p.doc, JSON.stringify(p.context ?? null)] as const, ([g, d]) => api.docView(g, d, ctx()));
  const reload = async () => mutate(await api.docView(p.github, p.doc, ctx()));
  // Reconciliation review: render the incoming snapshot instead of the current one.
  const [reviewing, setReviewing] = createSignal(false);
  const [incoming, { refetch: refetchIncoming }] = createResource(
    () => (reviewing() && view()?.pending ? [p.github, p.doc, view()!.pending!.to] as const : null),
    ([g, d, sha]) => api.docView(g, d, ctx(), sha),
  );
  const [picking, setPicking] = createSignal<string | null>(null);
  const [focusSpan, setFocusSpan] = createSignal<string | null>(null);
  const active = () => (reviewing() ? incoming() : view());
  const [reqs, { refetch: refetchReqs }] = createResource(() => p.github, api.listReqs);
  const [groups, { refetch: refetchGroups }] = createResource(() => p.github, api.listGroups);
  const groupsByReq = createMemo(() => {
    const m = new Map<string, string[]>();
    for (const g of groups() ?? []) for (const mem of g.group.members) { const k = slugOf(mem); m.set(k, [...(m.get(k) ?? []), g.group.title]); }
    return m;
  });
  const [groupLens, setGroupLens] = createSignal<string | null>(null); // group slug filter (`ui~grp-lens~1`)
  const [groupTargets, setGroupTargets] = createSignal<string[]>([]);

  // ---- optimistic patches: span id → status override
  const [patches, setPatches] = createStore<Record<string, SpanStatus>>({});

  const rows = createMemo<SpanRow[]>(() => {
    const v = active();
    if (!v) return [];
    const statusById = new Map(v.coverage.spans);
    return v.snapshot.spans.map((span) => {
      const base = statusById.get(span.id)!;
      const patch = patches[span.id];
      return { span, status: patch ?? base, pending: !!patch };
    });
  });

  const meter = createMemo(() => {
    let total = 0, residue = 0, mapped = 0, nn = 0, q = 0;
    for (const r of rows()) {
      const counts = !r.status.structural || r.status.state !== "unclassified";
      if (!counts) continue;
      total++;
      if (r.status.state === "unclassified") residue++;
      else if (r.status.state === "mapped") mapped++;
      else if (r.status.state === "non-normative") nn++;
      else q++;
    }
    return { total, residue, mapped, non_normative: nn, questioned: q, classified: total - residue };
  });

  // ---- selection: cursor index + optional anchor for ranges
  const [cursor, setCursor] = createSignal(0);
  const [anchor, setAnchor] = createSignal<number | null>(null);
  const [linkedReq, setLinkedReq] = createSignal<string | null>(null); // slug highlighted from inventory
  const [dialog, setDialog] = createSignal<"req" | "question" | "help" | "group" | null>(null);
  const [busy, setBusy] = createSignal(false);

  const selRange = createMemo<[number, number]>(() => {
    const a = anchor();
    const c = cursor();
    return a === null ? [c, c] : [Math.min(a, c), Math.max(a, c)];
  });
  const selectedIds = createMemo(() => {
    const [lo, hi] = selRange();
    return rows().slice(lo, hi + 1).map((r) => r.span.id);
  });
  const selectedText = createMemo(() => {
    const [lo, hi] = selRange();
    return rows().slice(lo, hi + 1).map((r) => r.span.text).join(" ");
  });
  const current = createMemo(() => rows()[cursor()]);

  // Slug → req for margin/inventory lookups
  const reqBySlug = createMemo(() => {
    const m = new Map<string, Req>();
    for (const r of reqs() ?? []) m.set(slugOf(r.id), r);
    return m;
  });
  const lensSpanIds = createMemo(() => {
    const slug = groupLens();
    if (!slug) return null;
    const g = (groups() ?? []).find((x) => slugOf(x.group.id) === slug);
    const members = new Set((g?.group.members ?? []).map(slugOf));
    const ids = new Set<string>();
    for (const r of reqs() ?? []) if (members.has(slugOf(r.id))) for (const a of r.anchors) if (a.doc === p.doc) ids.add(a.span);
    return ids;
  });
  const linkedSpanIds = createMemo(() => {
    const slug = linkedReq();
    if (!slug) return new Set<string>();
    const r = reqBySlug().get(slug);
    return new Set((r?.anchors ?? []).filter((a) => a.doc === p.doc).map((a) => a.span));
  });

  // ---- navigation
  let docEl: HTMLDivElement | undefined;
  function scrollTo(i: number) {
    const id = rows()[i]?.span.id;
    if (!id) return;
    const el = docEl?.querySelector<HTMLElement>(`[data-sid="${CSS.escape(id)}"]`);
    el?.scrollIntoView({ block: "center", behavior: "auto" });
  }
  function move(i: number, extend = false) {
    const n = rows().length;
    if (!n) return;
    const clamped = Math.max(0, Math.min(n - 1, i));
    if (extend) {
      if (anchor() === null) setAnchor(cursor());
    } else {
      setAnchor(null);
    }
    setCursor(clamped);
    setLinkedReq(null);
    scrollTo(clamped);
  }
  function nextUnclassified(from = cursor()) {
    const rs = rows();
    for (let k = 1; k <= rs.length; k++) {
      const i = (from + k) % rs.length;
      const r = rs[i];
      if (r.status.state === "unclassified" && !r.status.structural) {
        move(i);
        return true;
      }
    }
    return false;
  }

  // ---- mutations (optimistic → api → refetch)
  async function mutateSpans(ids: string[], optimistic: (prev: SpanStatus) => SpanStatus, call: () => Promise<unknown>) {
    const rs = rows();
    const before = new Map(ids.map((id) => [id, rs.find((r) => r.span.id === id)!.status]));
    setPatches(produce((s) => { for (const id of ids) s[id] = optimistic(before.get(id)!); }));
    setBusy(true);
    try {
      await call();
      await reload();
      if (reviewing()) refetchIncoming();
      refetchReqs();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    } finally {
      setPatches(produce((s) => { for (const id of ids) delete s[id]; }));
      setBusy(false);
    }
  }

  function markNonNormative() {
    const ids = selectedIds();
    mutateSpans(ids, (s) => ({ ...s, state: "non-normative" }), () => api.markNonNormative(p.github, p.doc, ids, ctx())).then(() => afterClassify());
  }
  function clearClassification() {
    const rs = rows();
    const ids = selectedIds();
    const work: Promise<unknown>[] = [];
    for (const id of ids) {
      const st = rs.find((r) => r.span.id === id)!.status;
      if (st.state === "non-normative") work.push(api.unmark(p.github, p.doc, [id], ctx()));
      for (const rid of st.reqs) work.push(api.detachReq(p.github, p.doc, [id], slugOf(rid)));
    }
    if (!work.length) return;
    mutateSpans(ids, (s) => ({ ...s, state: "unclassified", reqs: [] }), () => Promise.all(work));
  }
  function afterClassify() {
    // Advance to next unclassified span — the systematic pass (`ui~residue-nav~1`).
    setAnchor(null);
    if (!nextUnclassified(selRange()[1])) p.toast({ kind: "info", text: "Residue is zero — close the round when you're ready." });
  }

  async function createOrAttach(r: { mode: "attach"; slug: string } | { mode: "create"; statement: string; slug?: string; pattern?: Pattern; rating?: [Level, Level] }) {
    const ids = selectedIds();
    setDialog(null);
    const label = r.mode === "attach" ? `req~${r.slug}` : "req~…";
    await mutateSpans(
      ids,
      (s) => ({ ...s, state: "mapped", reqs: [...s.reqs, label] }),
      () => (r.mode === "attach" ? api.attachReq(p.github, p.doc, ids, r.slug, ctx()) : api.createReq(p.github, p.doc, ids, r, ctx())),
    );
    afterClassify();
  }

  async function raiseQuestion(q: { quote: string; materiality: Level; readings: { key: string; text: string }[]; default?: string }) {
    const ids = selectedIds();
    setDialog(null);
    await mutateSpans(ids, (s) => ({ ...s, state: "question", questions: [...s.questions, "qst~…"] }), () => api.flagQuestion(p.github, p.doc, ids, q, ctx()));
    afterClassify();
  }

  async function closeRound() {
    setBusy(true);
    try {
      const r = await api.closeRound(p.github, p.doc, ctx());
      p.toast({ kind: "info", text: `Round #${r.n} closed.` });
      await reload();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    } finally {
      setBusy(false);
    }
  }

  async function doExport() {
    setBusy(true);
    try {
      const r = await api.export(p.github);
      const v = r.validate ? (r.validate.code === 0 ? " · reqtrace validate OK" : ` · reqtrace validate FAILED (${r.validate.code})`) : "";
      p.toast({ kind: r.validate && r.validate.code !== 0 ? "error" : "info", text: `Exported ${r.items} item(s) → ${r.inventory}${v}` });
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    } finally {
      setBusy(false);
    }
  }

  function openGroupPalette(slugs?: string[]) {
    const targets = slugs ?? (linkedReq() ? [linkedReq()!] : (current()?.status.reqs ?? []).map(slugOf));
    if (!targets.length) { p.toast({ kind: "info", text: "Select a mapped sentence (or a requirement in the panel) first, then press g." }); return; }
    setGroupTargets(targets);
    setDialog("group");
  }
  async function assignToGroup(groupSlug: string) {
    setDialog(null);
    try {
      await api.assignGroup(p.github, groupSlug, groupTargets());
      refetchGroups();
      p.toast({ kind: "info", text: `Added ${groupTargets().length === 1 ? `req~${groupTargets()[0]}` : `${groupTargets().length} requirements`} to group.` });
    } catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  async function createAndAssign(title: string) {
    setDialog(null);
    try {
      const g = await api.createGroup(p.github, title);
      await api.assignGroup(p.github, slugOf(g.id), groupTargets());
      refetchGroups();
      p.toast({ kind: "info", text: `Created group “${g.title}” and added ${groupTargets().length} requirement(s).` });
    } catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }

  // ---- reconciliation
  async function decide(from: string, d: Decision) {
    setBusy(true);
    try {
      await api.decideVerdict(p.github, p.doc, from, d, ctx());
      await reload();
    } catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); setPicking(null); }
  }
  async function confirmRecon() {
    setBusy(true);
    try {
      const r = await api.confirmReconciliation(p.github, p.doc, ctx());
      setReviewing(false);
      await reload();
      refetchReqs();
      p.toast({ kind: "info", text: `Adopted ${r.to.slice(0, 7)} — round closed, ${r.added.length} new sentence(s) to classify.` });
      landed = false;
    } catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); }
  }
  function pickSpan(i: number) {
    const from = picking();
    if (!from) return false;
    const id = rows()[i]?.span.id;
    if (id) decide(from, { kind: "reanchor", span: id });
    return true;
  }

  // ---- keyboard
  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (dialog()) {
      if (e.key === "Escape") { e.preventDefault(); setDialog(null); }
      return;
    }
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key;
    const ext = e.shiftKey;
    if (reviewing() && "rcqxg".includes(k) && k.length === 1) {
      e.preventDefault();
      p.toast({ kind: "info", text: "Decide the changed sentences and confirm first — then classify the new text." });
      return;
    }
    switch (k) {
      case "u": case "U": e.preventDefault(); nextUnclassified(); break;
      case "n": case "j": case "ArrowDown": e.preventDefault(); move(cursor() + 1, ext); break;
      case "N": case "J": e.preventDefault(); move(cursor() + 1, true); break;
      case "p": case "k": case "ArrowUp": e.preventDefault(); move(cursor() - 1, ext); break;
      case "P": case "K": e.preventDefault(); move(cursor() - 1, true); break;
      case "r": e.preventDefault(); setDialog("req"); break;
      case "c": e.preventDefault(); markNonNormative(); break;
      case "q": e.preventDefault(); setDialog("question"); break;
      case "x": case "Backspace": e.preventDefault(); clearClassification(); break;
      case "e": e.preventDefault(); { const s = current()?.status.reqs[0]; if (s) setLinkedReq(slugOf(s)); } break;
      case "g": e.preventDefault(); openGroupPalette(); break;
      case "G": e.preventDefault(); move(rows().length - 1); break;
      case "Home": e.preventDefault(); move(0); break;
      case "End": e.preventDefault(); move(rows().length - 1); break;
      case "?": e.preventDefault(); setDialog(dialog() === "help" ? null : "help"); break;
      case "Escape": setAnchor(null); setLinkedReq(null); setPicking(null); break;
    }
  }
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  // On first load land on the first unclassified prose span.
  let landed = false;
  createEffect(() => { reviewing(); landed = false; });
  createEffect(() => {
    if (!landed && view() && rows().length) {
      landed = true;
      const target = p.initialSpan ? rows().findIndex((r) => r.span.id === p.initialSpan) : -1;
      queueMicrotask(() => { if (target >= 0) move(target); else nextUnclassified(-1); });
    }
  });

  return (
    <div class="classifier">
      <header class="topbar">
        <div class="crumb">
          <button class="ghost" onClick={p.onBack} title="Back to repo">←</button>
          <span>{p.github}</span>
          <span class="sep">/</span>
          <span class="doc">{p.doc}</span>
          <Show when={isPr()}><span class="chip pr">PR #{(p.context as { pr: number }).pr}</span></Show>
        </div>
        <span class="spacer" />
        <Show when={(groups() ?? []).length}>
          <select class="lens" value={groupLens() ?? ""} onChange={(e) => setGroupLens(e.currentTarget.value || null)} title="Group lens — dim sentences outside a group">
            <option value="">all groups</option>
            <For each={groups()}>{(g) => <option value={slugOf(g.group.id)}>{g.group.title} ({g.group.members.length})</option>}</For>
          </select>
        </Show>
        <Show when={view()?.round} fallback={<span class="round">no open round</span>}>
          {(r) => <span class="round">round <b>#{r().n}</b> · {shortSha(r().snapshot)}</span>}
        </Show>
        <Show when={view()?.pending}>
          {(pend) => (
            <button class="banner" classList={{ pr: isPr() }} onClick={() => setReviewing(!reviewing())}>
              {isPr() ? "▲ " : "⚠ "}
              {isPr() ? `${pend().verdicts.filter((v) => v.kind !== "unchanged").length} changed · ${pend().added.length} new vs base` : `changed upstream · ${pend().verdicts.filter((v) => v.kind !== "unchanged" && !v.decision).length} to decide`}
              {reviewing() ? " · reviewing" : " · review"}
            </button>
          )}
        </Show>
        <button onClick={doExport} disabled={busy()}>Export</button>
        <button class="primary" onClick={closeRound} disabled={busy() || !view()?.round || meter().residue > 0} title="Requires residue = 0">
          Close round
        </button>
      </header>

      <div class="work">
        <div class="docwrap">
          <div class="docscroll" ref={docEl} onScroll={() => setTick((n) => n + 1)}>
            <Show when={view()} fallback={<div class="empty muted">loading…</div>}>
              {(v) => (
                <DocBody
                  view={v()}
                  rows={rows()}
                  cursor={cursor()}
                  selRange={selRange()}
                  linked={linkedSpanIds()}
                  lens={lensSpanIds()}
                  dimOthers={!!linkedReq()}
                  onPick={(i, shift) => { if (!pickSpan(i)) move(i, shift); }}
                  focus={focusSpan()}
                  added={reviewing() ? new Set(view()?.pending?.added ?? []) : EMPTY}
                  onPickReq={(slug) => setLinkedReq(slug)}
                  tick={tick()}
                />
              )}
            </Show>
          </div>
          <Rail rows={rows()} cursor={cursor()} onJump={(i) => move(i)} docEl={docEl} tick={tick()} />
        </div>

        <Show when={reviewing() && view()?.pending} fallback={<Inventory
          doc={p.doc}
          reqs={reqs() ?? []}
          groupsByReq={groupsByReq()}
          linked={linkedReq()}
          currentReqIds={current()?.status.reqs ?? []}
          onSelect={(slug) => {
            setLinkedReq(slug);
            const first = reqBySlug().get(slug)?.anchors.find((a) => a.doc === p.doc)?.span;
            const i = rows().findIndex((r) => r.span.id === first);
            if (i >= 0) { setCursor(i); setAnchor(null); scrollTo(i); }
          }}
          onJumpSpan={(id) => { const i = rows().findIndex((r) => r.span.id === id); if (i >= 0) move(i); }}
          onChanged={async () => { refetchReqs(); refetchGroups(); await reload(); }}
          onGroup={(slug) => openGroupPalette([slug])}
          github={p.github}
          toast={p.toast}
        />}>
          {(pend) => (
            <ReconcilePanel
              recon={pend()}
              addedText={(id) => incoming()?.snapshot.spans.find((s) => s.id === id)?.text}
              readOnly={isPr()}
              picking={picking()}
              focus={focusSpan()}
              busy={busy()}
              onDecide={decide}
              onPick={setPicking}
              onFocus={setFocusSpan}
              onConfirm={confirmRecon}
              onExit={() => { setReviewing(false); setPicking(null); }}
            />
          )}
        </Show>
      </div>

      <footer class="statusbar">
        <div class="cov">
          <span class="bar">
            <i class="m" style={{ width: pct(meter().mapped, meter().total) }} />
            <i class="q" style={{ width: pct(meter().questioned, meter().total) }} />
            <i class="n" style={{ width: pct(meter().non_normative, meter().total) }} />
          </span>
          <span>{meter().classified}/{meter().total} classified</span>
          <span>· residue <span class="n-res" classList={{ zero: meter().residue === 0 }}>{meter().residue}</span></span>
          <span class="muted">· {meter().mapped} req · {meter().non_normative} ctx · {meter().questioned} q</span>
        </div>
        <div class="keys">
          <span><kbd>u</kbd> next</span>
          <span><kbd>r</kbd> requirement</span>
          <span><kbd>c</kbd> context</span>
          <span><kbd>q</kbd> question</span>
          <span><kbd>?</kbd> all keys</span>
        </div>
      </footer>

      <Show when={dialog() === "req"}>
        <ReqPalette
          github={p.github}
          reqs={reqs() ?? []}
          selectionText={selectedText()}
          onClose={() => setDialog(null)}
          onSubmit={createOrAttach}
        />
      </Show>
      <Show when={dialog() === "question"}>
        <QuestionDialog quote={selectedText()} onClose={() => setDialog(null)} onSubmit={raiseQuestion} />
      </Show>
      <Show when={dialog() === "group"}>
        <GroupPalette groups={groups() ?? []} targets={groupTargets()} onClose={() => setDialog(null)} onAssign={assignToGroup} onCreate={createAndAssign} />
      </Show>
      <Show when={dialog() === "help"}>
        <div class="help">
          <table>
            <tbody>
              <tr><td><kbd>u</kbd></td><td>next unclassified sentence</td></tr>
              <tr><td><kbd>n</kbd> / <kbd>p</kbd></td><td>next / previous sentence (also j/k, arrows)</td></tr>
              <tr><td><kbd>⇧</kbd>+move</td><td>extend selection to classify a run</td></tr>
              <tr><td><kbd>r</kbd></td><td>map to a requirement (attach or create)</td></tr>
              <tr><td><kbd>c</kbd></td><td>mark as context (non-normative)</td></tr>
              <tr><td><kbd>q</kbd></td><td>flag as a question</td></tr>
              <tr><td><kbd>x</kbd></td><td>clear classification</td></tr>
              <tr><td><kbd>e</kbd></td><td>show linked requirement</td></tr>
              <tr><td><kbd>g</kbd></td><td>add the linked requirement to a group</td></tr>
              <tr><td><kbd>Home</kbd> / <kbd>End</kbd></td><td>top / bottom</td></tr>
              <tr><td><kbd>esc</kbd></td><td>clear selection / highlight</td></tr>
            </tbody>
          </table>
        </div>
      </Show>
    </div>
  );
};

const [tick, setTick] = createSignal(0);
const EMPTY = new Set<string>();

// ---------------------------------------------------------------------------
// Document body: renders spans grouped into paragraphs / lists / tables / code.

type BlockGroup =
  | { kind: "p"; items: number[] }
  | { kind: "h"; item: number; level: number }
  | { kind: "ul"; items: number[] }
  | { kind: "table"; items: number[] }
  | { kind: "code"; item: number }
  | { kind: "html"; item: number };

function groupBlocks(rows: SpanRow[], source: string): BlockGroup[] {
  const out: BlockGroup[] = [];
  let i = 0;
  const gapHasBlank = (a: Span, b: Span) => /\n[ \t]*\n/.test(source.slice(a.end, b.start));
  while (i < rows.length) {
    const s = rows[i].span;
    if (s.block === "heading") { out.push({ kind: "h", item: i, level: Math.min(4, Math.max(1, s.depth ?? 1)) }); i++; continue; }
    if (s.block === "code") { out.push({ kind: "code", item: i }); i++; continue; }
    if (s.block === "html") { out.push({ kind: "html", item: i }); i++; continue; }
    if (s.block === "row") { const items = [i]; while (rows[i + 1]?.span.block === "row") items.push(++i); out.push({ kind: "table", items }); i++; continue; }
    if (s.block === "li") { const items = [i]; while (rows[i + 1]?.span.block === "li" && !gapHasBlank(rows[i].span, rows[i + 1].span)) items.push(++i); out.push({ kind: "ul", items }); i++; continue; }
    // para: consecutive para spans without a blank line between them
    const items = [i];
    while (rows[i + 1]?.span.block === "para" && !gapHasBlank(rows[i].span, rows[i + 1].span)) items.push(++i);
    out.push({ kind: "p", items });
    i++;
  }
  return out;
}

const DocBody: Component<{
  view: DocView;
  rows: SpanRow[];
  cursor: number;
  selRange: [number, number];
  linked: Set<string>;
  lens: Set<string> | null;
  focus: string | null;
  added: Set<string>;
  dimOthers: boolean;
  onPick: (i: number, shift: boolean) => void;
  onPickReq: (slug: string) => void;
  tick: number;
}> = (p) => {
  const groups = createMemo(() => groupBlocks(p.rows, p.view.source));
  let inner: HTMLDivElement | undefined;
  const [marks, setMarks] = createSignal<{ top: number; height: number; row: SpanRow }[]>([]);

  // Measure margin-mark positions after render / scroll / resize.
  function measure() {
    if (!inner) return;
    const base = inner.getBoundingClientRect().top;
    const out: { top: number; height: number; row: SpanRow }[] = [];
    let lastTop = -1e9;
    for (const r of p.rows) {
      if (r.status.state === "unclassified") continue;
      const el = inner.querySelector<HTMLElement>(`[data-sid="${CSS.escape(r.span.id)}"]`);
      if (!el) continue;
      const rect = el.getBoundingClientRect();
      const top = rect.top - base;
      // Merge marks that would overlap the previous one (same line): stack below.
      const adj = top < lastTop + 16 ? lastTop + 16 : top;
      out.push({ top: adj, height: rect.height, row: r });
      lastTop = adj;
    }
    setMarks(out);
  }
  createEffect(() => { p.rows; p.tick; groups(); requestAnimationFrame(measure); });
  onMount(() => {
    const ro = new ResizeObserver(() => measure());
    if (inner) ro.observe(inner);
    onCleanup(() => ro.disconnect());
  });

  const spanEl = (i: number, block = false) => {
    const r = () => p.rows[i];
    return (
      <span
        class="sp"
        classList={{
          block,
          [`state-${r().status.state}`]: true,
          structural: r().status.structural,
          current: p.cursor === i,
          selected: i >= p.selRange[0] && i <= p.selRange[1] && p.selRange[0] !== p.selRange[1],
          linked: p.linked.has(r().span.id) || p.focus === r().span.id,
          added: p.added.has(r().span.id),
          dim: (p.dimOthers && !p.linked.has(r().span.id)) || (!!p.lens && !p.lens.has(r().span.id) && !r().status.structural),
          pending: r().pending,
        }}
        data-sid={r().span.id}
        onMouseDown={(e) => { e.preventDefault(); p.onPick(i, e.shiftKey); }}
        title={r().span.id}
      >
        {r().span.text}
      </span>
    );
  };

  return (
    <div class="docinner" ref={inner}>
      <div class="margin">
        <For each={marks()}>
          {(m) => (
            <div class={`mark state-${m.row.status.state}`} classList={{ pending: m.row.pending }} style={{ top: `${m.top}px` }}>
              <div class="ids">
                <For each={m.row.status.reqs}>
                  {(id) => <span class="id" onClick={() => p.onPickReq(slugOf(id))} title={id}>{id.replace(/^req~/, "").replace(/~\d+$/, "")}</span>}
                </For>
                <For each={m.row.status.questions}>{(id) => <span class="id qid" title={id}>?{id.replace(/^qst~/, "").replace(/~\d+$/, "").slice(0, 14)}</span>}</For>
                <Show when={m.row.status.state === "non-normative"}><span class="nn">context</span></Show>
              </div>
              <span class="rule" />
            </div>
          )}
        </For>
      </div>
      <For each={groups()}>
        {(g) => {
          switch (g.kind) {
            case "h":
              return <Dynamic component={`h${g.level}`} class="h">{spanEl(g.item, true)}</Dynamic>;
            case "p":
              return <p><Index each={g.items}>{(i, k) => <>{k > 0 ? " " : ""}{spanEl(i())}</>}</Index></p>;
            case "ul":
              return (
                <ul>
                  <For each={g.items}>{(i) => <li style={{ "margin-left": `${((p.rows[i].span.depth ?? 1) - 1) * 1.3}em` }}>{spanEl(i)}</li>}</For>
                </ul>
              );
            case "table":
              return (
                <table class="md"><tbody>
                  <For each={g.items}>{(i) => <tr><td>{spanEl(i, true)}</td></tr>}</For>
                </tbody></table>
              );
            case "code":
              return <pre class="md">{spanEl(g.item, true)}</pre>;
            case "html":
              return <div class="html">{spanEl(g.item, true)}</div>;
          }
        }}
      </For>
    </div>
  );
};

// ---------------------------------------------------------------------------
// Residue rail: one tick per span, coloured by state; click to jump.

const Rail: Component<{ rows: SpanRow[]; cursor: number; onJump: (i: number) => void; docEl?: HTMLDivElement; tick: number }> = (p) => {
  let el: HTMLDivElement | undefined;
  const n = () => Math.max(1, p.rows.length);
  const [viewport, setViewport] = createSignal<{ top: number; height: number }>({ top: 0, height: 0 });
  createEffect(() => {
    p.tick;
    const d = p.docEl;
    if (!d) return;
    const total = d.scrollHeight || 1;
    setViewport({ top: (d.scrollTop / total) * 100, height: (d.clientHeight / total) * 100 });
  });
  return (
    <div
      class="rail"
      ref={el}
      onClick={(e) => {
        const r = el!.getBoundingClientRect();
        const i = Math.floor(((e.clientY - r.top) / r.height) * n());
        p.onJump(Math.max(0, Math.min(n() - 1, i)));
      }}
      title="Residue rail — click to jump"
    >
      <div class="view" style={{ top: `${viewport().top}%`, height: `${viewport().height}%` }} />
      <For each={p.rows}>
        {(r, i) => (
          <div
            class={`tick state-${r.status.state}`}
            classList={{ structural: r.status.structural && r.status.state === "unclassified" }}
            style={{ top: `calc(${(i() / n()) * 100}% )` }}
          />
        )}
      </For>
      <div class="cursor" style={{ top: `${(p.cursor / n()) * 100}%` }} />
    </div>
  );
};

// ---------------------------------------------------------------------------

export function slugOf(id: string) {
  return id.split("~")[1] ?? id;
}
export function shortSha(s: string) {
  return s.slice(0, 8);
}
function pct(a: number, b: number) {
  return b ? `${(100 * a) / b}%` : "0%";
}
export type { SpanState };
