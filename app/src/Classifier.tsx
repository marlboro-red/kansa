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
import { createStore, produce, reconcile } from "solid-js/store";
import { Dynamic } from "solid-js/web";
import { api, type DocView, type Req, type Span, type SpanState, type SpanStatus, type Pattern, type Level } from "./api";
import { ReqPalette } from "./ReqPalette";
import { QuestionDialog } from "./QuestionDialog";
import { Inventory } from "./Inventory";
import { GroupPalette } from "./GroupPalette";
import { ReconcilePanel } from "./ReconcilePanel";
import { ProposalsPanel } from "./ProposalsPanel";
import { ByteSource, codeHtml, inlineHtml, stripBlockPrefix, tableCells } from "./md";
import { createCached } from "./swr";
import type { Proposal } from "./api";
import type { Context, Decision } from "./api";

export type Toast = { kind: "error" | "info"; text: string };

type Props = {
  github: string;
  /** Display name (folder name for local repos). */
  label?: string;
  doc: string;
  /** Land on this span (click-through from the inventory). */
  initialSpan?: string;
  /** PR context; default branch when absent. */
  context?: Context;
  onBack: () => void;
  toast: (t: Toast | null) => void;
};

/** Everything the classifier knows about one span, merged with optimistic patches. */
export type SpanRow = { id: string; span: Span; status: SpanStatus; pending: boolean };

export const Classifier: Component<Props> = (p) => {
  const ctx = () => p.context;
  const isPr = () => !!p.context && "pr" in p.context;
  const viewKey = () => `doc:${p.github}:${p.doc}:${JSON.stringify(p.context ?? null)}`;
  const [view, { mutate }] = createCached(viewKey, () => api.docView(p.github, p.doc, ctx()));
  const reload = async () => mutate(await api.docView(p.github, p.doc, ctx()));
  /** Cheap refresh after a classification: coverage/round/pending only; full reload if the snapshot moved. */
  let inflight = 0; // mutations awaiting their state refresh
  const reloadState = async () => {
    const v = view();
    if (!v) return reload();
    const st = await api.docState(p.github, p.doc, ctx());
    if (st.snapshot !== v.snapshot.sha) return reload();
    // If newer mutations are still in flight, this state is already stale — applying it would
    // briefly revert their optimistic paint. The last one to land applies.
    if (inflight > 1) return;
    mutate({ ...v, coverage: st.coverage, round: st.round, pending: st.pending, tracked: st.tracked });
  };
  // Reconciliation review: render the incoming snapshot instead of the current one.
  const [reviewing, setReviewing] = createSignal(false);
  const [incoming, { refetch: refetchIncoming }] = createResource(
    () => (reviewing() && view()?.pending ? [p.github, p.doc, view()!.pending!.to] as const : null),
    ([g, d, sha]) => api.docView(g, d, ctx(), sha),
  );
  const [picking, setPicking] = createSignal<string | null>(null);
  // ---- agent pre-fill: proposals live beside the doc; polled while a job runs
  const [prefill, { refetch: refetchPrefill }] = createResource(() => [p.github, p.doc, JSON.stringify(p.context ?? null)] as const, ([g, d]) => api.prefillStatus(g, d, ctx()));
  const [showProposals, setShowProposals] = createSignal(false);
  const openProposals = createMemo(() => (prefill()?.proposals ?? []).filter((x) => x.status === "proposed"));
  const proposalBySpan = createMemo(() => {
    const m = new Map<string, Proposal>();
    for (const x of openProposals()) for (const sid of x.spans) m.set(sid, x);
    return m;
  });
  createEffect(() => {
    const j = prefill()?.job;
    if (j?.state === "running") {
      const t = window.setTimeout(() => refetchPrefill(), 1500);
      onCleanup(() => window.clearTimeout(t));
    }
  });
  const [focusSpan, setFocusSpan] = createSignal<string | null>(null);
  const active = () => (reviewing() ? incoming() : view());
  const [reqs, { refetch: refetchReqs }] = createCached(() => `reqs:${p.github}`, () => api.listReqs(p.github));
  const [groups, { refetch: refetchGroups }] = createCached(() => `groups:${p.github}`, () => api.listGroups(p.github));
  const groupsByReq = createMemo(() => {
    const m = new Map<string, string[]>();
    for (const g of groups() ?? []) for (const mem of g.group.members) { const k = slugOf(mem); m.set(k, [...(m.get(k) ?? []), g.group.title]); }
    return m;
  });
  const [groupLens, setGroupLens] = createSignal<string | null>(null); // group slug filter (`ui~grp-lens~1`)
  const [groupTargets, setGroupTargets] = createSignal<string[]>([]);

  const computedRows = createMemo<SpanRow[]>(() => {
    const v = active();
    if (!v) return [];
    const statusById = new Map(v.coverage.spans);
    return v.snapshot.spans.map((span) => ({ id: span.id, span, status: statusById.get(span.id)!, pending: false }));
  });
  // Fine-grained store: `reconcile` only touches rows whose status actually changed, so a
  // keypress re-runs a handful of span effects instead of all of them (`ui~perf-classify~1`).
  const [rowStore, setRowStore] = createStore<{ list: SpanRow[] }>({ list: [] });
  createEffect(() => setRowStore("list", reconcile(computedRows(), { key: "id", merge: true })));
  const rows = () => rowStore.list;
  const rowIndex = createMemo(() => { const m = new Map<string, number>(); active()?.snapshot.spans.forEach((sp, i) => m.set(sp.id, i)); return m; });

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
  /** Optimistic update: paint the new state on exactly the touched rows, persist, then sync
   *  from core (which reconciles only what differs). On failure, resync = rollback + toast. */
  async function mutateSpans(ids: string[], optimistic: (prev: SpanStatus) => SpanStatus, call: () => Promise<unknown>, touchesReqs = true) {
    const idx = rowIndex();
    const touched = ids.map((id) => idx.get(id)).filter((i): i is number => i !== undefined);
    setRowStore(produce((st) => { for (const i of touched) { st.list[i].status = optimistic(st.list[i].status); st.list[i].pending = true; } }));
    setBusy(true);
    inflight++;
    try {
      await call();
      await reloadState();
      if (reviewing()) refetchIncoming();
      if (touchesReqs) refetchReqs();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
      await reloadState().catch(() => {});
    } finally {
      inflight--;
      setRowStore(produce((st) => { for (const i of touched) st.list[i].pending = false; }));
      setBusy(false);
    }
  }

  function markNonNormative() {
    const ids = selectedIds();
    mutateSpans(ids, (s) => ({ ...s, state: "non-normative" }), () => api.markNonNormative(p.github, p.doc, ids, ctx()), false).then(() => afterClassify());
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
  async function unassignFromGroup(groupSlug: string) {
    setDialog(null);
    try {
      await api.unassignGroup(p.github, groupSlug, groupTargets());
      refetchGroups();
      p.toast({ kind: "info", text: `Removed ${groupTargets().length === 1 ? `req~${groupTargets()[0]}` : `${groupTargets().length} requirements`} from group.` });
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

  // ---- agent pre-fill
  async function startPrefill() {
    setBusy(true);
    try {
      await api.prefillStart(p.github, p.doc, ctx());
      setShowProposals(true);
      refetchPrefill();
    } catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); }
  }
  async function acceptProposal(id: string) {
    const x = openProposals().find((y) => y.id === id);
    if (!x) return;
    // optimistic: paint the spans in their proposed state
    const state = x.proposed.kind === "req" ? "mapped" : x.proposed.kind === "context" ? "non-normative" : "question";
    await mutateSpans(x.spans, (s) => ({ ...s, state: state as SpanState }), () => api.acceptProposal(p.github, p.doc, id, ctx()));
    refetchPrefill();
    refetchGroups();
    if (x.spans.includes(current()?.span.id ?? "")) afterClassify();
  }
  async function rejectProposal(id: string) {
    try { await api.rejectProposal(p.github, p.doc, id, ctx()); refetchPrefill(); } catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  async function acceptAll() {
    setBusy(true);
    try {
      const n = await api.acceptAllProposals(p.github, p.doc, ctx());
      await reload(); refetchReqs(); refetchGroups(); refetchPrefill();
      p.toast({ kind: "info", text: `Accepted ${n} proposal(s).` });
    } catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); }
  }
  async function clearProposals() {
    try { await api.clearProposals(p.github, p.doc, ctx()); refetchPrefill(); } catch (e) { p.toast({ kind: "error", text: String(e) }); }
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

  // ---- doc zoom: ctrl/⌘ + wheel over the page, ctrl/⌘ + `=`/`-`/`0` (`ui~doc-zoom~1`).
  // Only the prose scales — the margin rail keeps its size so marks stay legible; the rail
  // re-measures itself through its ResizeObserver when the text reflows.
  const [zoom, setZoomSignal] = createSignal(storedZoom());
  const setZoom = (z: number) => {
    const v = Math.min(2.5, Math.max(0.6, Math.round(z * 1000) / 1000));
    if (v === zoom()) return;
    setZoomSignal(v);
    try { v === 1 ? localStorage.removeItem(ZOOM_KEY) : localStorage.setItem(ZOOM_KEY, String(v)); } catch { /* ignore */ }
    setTick((n) => n + 1); // the residue rail's viewport box is derived from scrollHeight
    // Reflow moves everything; keep the sentence under the cursor where the reader left it.
    requestAnimationFrame(() => scrollTo(cursor()));
  };
  function onWheel(e: WheelEvent) {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault(); // otherwise the webview zooms the whole app
    setZoom(zoom() * Math.exp(-e.deltaY * 0.002));
  }

  // ---- keyboard
  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (dialog()) {
      if (e.key === "Escape") { e.preventDefault(); setDialog(null); }
      return;
    }
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
    if ((e.metaKey || e.ctrlKey) && !e.altKey && ["=", "+", "-", "_", "0"].includes(e.key)) {
      e.preventDefault();
      if (e.key === "0") setZoom(1);
      else setZoom(zoom() * (e.key === "-" || e.key === "_" ? 1 / 1.1 : 1.1));
      return;
    }
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
      case "Enter": {
        const pr = proposalBySpan().get(current()?.span.id ?? "");
        if (pr) { e.preventDefault(); acceptProposal(pr.id); }
        break;
      }
      case "x": case "Backspace": {
        e.preventDefault();
        const pr = proposalBySpan().get(current()?.span.id ?? "");
        if (pr) rejectProposal(pr.id); else clearClassification();
        break;
      }
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
  onMount(() => {
    // Solid's JSX handler would be registered passive by the browser for wheel; this is not.
    docEl?.addEventListener("wheel", onWheel, { passive: false });
    onCleanup(() => docEl?.removeEventListener("wheel", onWheel));
  });

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
          <span>{p.label ?? p.github}</span>
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
        <Show when={view() && !view()!.tracked}>
          <button class="banner" onClick={async () => { try { await api.trackDoc(p.github, p.doc); await reload(); p.toast({ kind: "info", text: `Tracking ${p.doc}.` }); } catch (e) { p.toast({ kind: "error", text: String(e) }); } }} title="This file is not a tracked HLD yet — track it to classify">
            ○ not tracked · click to track
          </button>
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
        <Show when={prefill()?.available && !isPr()}>
          <Show
            when={prefill()?.job?.state === "running"}
            fallback={
              <Show when={openProposals().length} fallback={<button class="agent" onClick={startPrefill} disabled={busy() || meter().residue === 0 || !!view()?.pending} title="Ask the agent to draft classifications for every unclassified sentence">✦ Pre-fill</button>}>
                <button class="agent on" onClick={() => setShowProposals(!showProposals())}>✦ {openProposals().length} proposal{openProposals().length === 1 ? "" : "s"}</button>
              </Show>
            }
          >
            <button class="agent on" onClick={() => setShowProposals(true)}><span class="spinner" style={{ display: "inline-block", "vertical-align": "middle", "margin-right": "6px" }} />pre-filling {prefill()!.job!.done}/{prefill()!.job!.total}</button>
          </Show>
        </Show>
        <button onClick={doExport} disabled={busy()}>Export</button>
        <button class="primary" onClick={closeRound} disabled={busy() || !view()?.round || meter().residue > 0} title="Requires residue = 0">
          Close round
        </button>
      </header>

      <div class="work">
        <div class="docwrap">
          <div class="docscroll" ref={docEl} style={{ "--doc-zoom": String(zoom()) }} onScroll={() => setTick((n) => n + 1)}>
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
                  proposals={reviewing() ? EMPTY_MAP : proposalBySpan()}
                  onPickReq={(slug) => setLinkedReq(slug)}
                />
              )}
            </Show>
          </div>
          <Rail rows={rows()} cursor={cursor()} onJump={(i) => move(i)} docEl={docEl} tick={tick()} />
        </div>

        <Show when={showProposals() && !reviewing()} fallback={<Show when={reviewing() && view()?.pending} fallback={<Inventory
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
        </Show>}>
          <ProposalsPanel
            job={prefill()?.job ?? null}
            proposals={prefill()?.proposals ?? []}
            currentSpan={current()?.span.id ?? null}
            spanText={(id) => view()?.snapshot.spans.find((s) => s.id === id)?.text}
            busy={busy()}
            onAccept={acceptProposal}
            onReject={rejectProposal}
            onAcceptAll={acceptAll}
            onClear={clearProposals}
            onFocus={setFocusSpan}
            onJump={(id) => { const i = rows().findIndex((r) => r.span.id === id); if (i >= 0) move(i); }}
            onExit={() => setShowProposals(false)}
          />
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
          <Show when={zoom() !== 1}>
            <button class="zoomchip" onClick={() => setZoom(1)} title="Ctrl/⌘ + scroll to zoom · click to reset (Ctrl/⌘ 0)">{Math.round(zoom() * 100)}%</button>
          </Show>
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
        <GroupPalette groups={groups() ?? []} targets={groupTargets()} onClose={() => setDialog(null)} onAssign={assignToGroup} onUnassign={unassignFromGroup} onCreate={createAndAssign} />
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
              <tr><td><kbd>ctrl</kbd>+scroll</td><td>zoom the page text (also ctrl <kbd>+</kbd> / <kbd>-</kbd> / <kbd>0</kbd>)</td></tr>
              <tr><td><kbd>esc</kbd></td><td>clear selection / highlight</td></tr>
            </tbody>
          </table>
        </div>
      </Show>
    </div>
  );
};

const [tick, setTick] = createSignal(0);
const ZOOM_KEY = "kansa.docZoom";
function storedZoom(): number {
  try {
    const z = Number(localStorage.getItem(ZOOM_KEY));
    return Number.isFinite(z) && z >= 0.6 && z <= 2.5 ? z : 1;
  } catch { return 1; }
}
const EMPTY = new Set<string>();
const HTML_CACHE = new Map<string, string>();
const EMPTY_MAP = new Map<string, Proposal>();

// ---------------------------------------------------------------------------
// Document body: renders spans grouped into paragraphs / lists / tables / code.

type BlockGroup =
  | { kind: "p"; items: number[] }
  | { kind: "h"; item: number; level: number }
  | { kind: "ul"; items: number[] }
  | { kind: "table"; items: number[] }
  | { kind: "code"; item: number }
  | { kind: "html"; item: number };

function groupBlocks(spans: Span[], source: string): BlockGroup[] {
  const out: BlockGroup[] = [];
  let i = 0;
  const rows = spans.map((span) => ({ span }));
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
  proposals: Map<string, Proposal>;
  dimOthers: boolean;
  onPick: (i: number, shift: boolean) => void;
  onPickReq: (slug: string) => void;
}> = (p) => {
  // Block structure depends only on the snapshot (stable across classification) — never rebuild
  // the document DOM on a keypress.
  // `view` is replaced on every refresh but snapshot/source keep their identity; memoizing them
  // means the block structure (and the whole doc DOM) only rebuilds when the snapshot changes.
  const snap = createMemo(() => p.view.snapshot);
  const source = createMemo(() => p.view.source);
  const groups = createMemo(() => groupBlocks(snap().spans, source()));
  const bytes = createMemo(() => new ByteSource(source()));
  const srcOf = (sp: Span) => bytes().slice(sp.start, sp.end);
  /** Inline HTML for a prose/list/heading span: its own markdown slice, formatting kept. */
  const inline = (sp: Span) => {
    const src = bytes().sliceInline(sp.start, sp.end);
    // Fall back to plain text if the slice doesn't look like this span (defensive).
    if (!src.trim()) return inlineHtml(sp.text);
    return inlineHtml(stripBlockPrefix(src).replace(/\s*\n\s*/g, " "));
  };
  let inner: HTMLDivElement | undefined;
  const [marks, setMarks] = createSignal<{ top: number; height: number; row: SpanRow; prop?: Proposal }[]>([]);

  // Measure margin-mark positions after render / scroll / resize.
  let elById: Map<string, HTMLElement> | null = null;
  const elementIndex = () => {
    if (!inner) return new Map<string, HTMLElement>();
    if (!elById) {
      elById = new Map();
      inner.querySelectorAll<HTMLElement>("[data-sid]").forEach((el) => elById!.set(el.dataset.sid!, el));
    }
    return elById;
  };
  function measure() {
    if (!inner) return;
    const idx = elementIndex();
    const base = inner.getBoundingClientRect().top;
    const out: { top: number; height: number; row: SpanRow; prop?: Proposal }[] = [];
    let lastTop = -1e9;
    // Hoist prop reads out of the loop (each `p.x` access is a getter).
    const rowsArr = p.rows;
    const props = p.proposals;
    for (const r of rowsArr) {
      const prop = r.status.state === "unclassified" ? props.get(r.span.id) : undefined;
      if (r.status.state === "unclassified" && !prop) continue;
      const el = idx.get(r.span.id);
      if (!el) continue;
      const rect = el.getBoundingClientRect();
      const top = rect.top - base;
      // Merge marks that would overlap the previous one (same line): stack below.
      const adj = top < lastTop + 16 ? lastTop + 16 : top;
      out.push({ top: adj, height: rect.height, row: r, prop });
      lastTop = adj;
    }
    setMarks(out);
  }
  // Re-measure when classification/proposals change or the pane resizes — never on scroll
  // (marks are absolutely positioned inside the scrolling content, so they move for free).
  let raf = 0;
  const schedule = () => { cancelAnimationFrame(raf); raf = requestAnimationFrame(measure); };
  createEffect(() => { p.rows; p.proposals; groups(); elById = null; schedule(); });
  onMount(() => {
    const ro = new ResizeObserver(() => schedule());
    if (inner) ro.observe(inner);
    onCleanup(() => { ro.disconnect(); cancelAnimationFrame(raf); });
  });

  // Rendered HTML per span is a pure function of the snapshot — compute once per process
  // (module-level cache keyed by snapshot sha), so reopening a doc skips the markdown pass.
  const spanHtml = (sp: Span) => {
    const key = `${snap().sha}:${sp.id}`;
    let h = HTML_CACHE.get(key);
    if (h === undefined) {
      if (HTML_CACHE.size > 50_000) HTML_CACHE.clear();
      h = inline(sp);
      HTML_CACHE.set(key, h);
    }
    return h;
  };
  const spanEl = (i: number, block = false) => {
    const sp = snap().spans[i]; // static content
    const st = () => p.rows[i]?.status; // reactive state
    const pend = () => p.rows[i]?.pending ?? false;
    const isText = sp.block !== "code" && sp.block !== "html" && sp.block !== "row";
    return (
      <span
        class="sp"
        classList={{
          block,
          "state-unclassified": st()?.state === "unclassified",
          "state-mapped": st()?.state === "mapped",
          "state-non-normative": st()?.state === "non-normative",
          "state-question": st()?.state === "question",
          structural: st()?.structural ?? false,
          current: p.cursor === i,
          selected: i >= p.selRange[0] && i <= p.selRange[1] && p.selRange[0] !== p.selRange[1],
          linked: p.linked.has(sp.id) || p.focus === sp.id,
          added: p.added.has(sp.id),
          proposed: p.proposals.has(sp.id) && st()?.state === "unclassified",
          dim: (p.dimOthers && !p.linked.has(sp.id)) || (!!p.lens && !p.lens.has(sp.id) && !(st()?.structural ?? false)),
          pending: pend(),
        }}
        data-sid={sp.id}
        onMouseDown={(e) => { e.preventDefault(); p.onPick(i, e.shiftKey); }}
        title={sp.id}
        innerHTML={isText ? spanHtml(sp) : undefined}
      >
        {sp.block === "row" ? rowCells(i) : sp.block === "code" ? codeBlock(i) : sp.block === "html" ? sp.text : undefined}
      </span>
    );
  };
  const rowCells = (i: number) => {
    const cells = tableCells(srcOf(snap().spans[i]));
    return <For each={cells}>{(c) => <span class="cell" innerHTML={inlineHtml(c)} />}</For>;
  };
  const codeBlock = (i: number) => {
    const { lang, html } = codeHtml(srcOf(snap().spans[i]));
    return <code class={lang ? `hljs language-${lang}` : "hljs"} innerHTML={html} />;
  };
  // Column alignment/widths: table rows render as a CSS grid with N equal-ish columns.
  const tableCols = (items: number[]) => Math.max(1, ...items.map((i) => tableCells(srcOf(snap().spans[i])).length));

  return (
    <div class="docinner" ref={inner}>
      <div class="margin">
        <Index each={marks()}>
          {(m) => (
            <div class="mark" classList={{ [`state-${m().row.status.state}`]: true, pending: m().row.pending, proposed: !!m().prop }} style={{ top: `${m().top}px` }}>
              <div class="ids">
                <Show when={m().prop}>{(pr) => <span class="id prop" title={pr().proposed.kind === "req" && "statement" in pr().proposed ? (pr().proposed as { statement: string }).statement : pr().proposed.kind}>✦ {pr().proposed.kind === "req" ? ((pr().proposed as { slug?: string | null }).slug ?? "requirement") : pr().proposed.kind}</span>}</Show>
                <For each={m().row.status.reqs}>
                  {(id) => <span class="id" onClick={() => p.onPickReq(slugOf(id))} title={id}>{id.replace(/^req~/, "").replace(/~\d+$/, "")}</span>}
                </For>
                <For each={m().row.status.questions}>{(id) => <span class="id qid" title={id}>?{id.replace(/^qst~/, "").replace(/~\d+$/, "").slice(0, 14)}</span>}</For>
                <Show when={m().row.status.state === "non-normative"}><span class="nn">context</span></Show>
              </div>
              <span class="rule" />
            </div>
          )}
        </Index>
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
                  <For each={g.items}>{(i) => <li style={{ "margin-left": `${((snap().spans[i].depth ?? 1) - 1) * 1.3}em` }}>{spanEl(i)}</li>}</For>
                </ul>
              );
            case "table":
              return (
                <div class="md-table" style={{ "--cols": tableCols(g.items) }}>
                  <For each={g.items}>{(i, k) => <div class="trow" classList={{ head: k() === 0 }}>{spanEl(i, true)}</div>}</For>
                </div>
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
  let canvas: HTMLCanvasElement | undefined;
  const n = () => Math.max(1, p.rows.length);
  const [viewport, setViewport] = createSignal<{ top: number; height: number }>({ top: 0, height: 0 });
  createEffect(() => {
    p.tick;
    const d = p.docEl;
    if (!d) return;
    const total = d.scrollHeight || 1;
    setViewport({ top: (d.scrollTop / total) * 100, height: (d.clientHeight / total) * 100 });
  });
  // One canvas draw per classification change instead of thousands of DOM ticks.
  function draw() {
    if (!canvas || !el) return;
    const cs = getComputedStyle(el);
    const col = { coral: cs.getPropertyValue("--coral").trim(), jade: cs.getPropertyValue("--jade").trim(), violet: cs.getPropertyValue("--violet").trim(), line: cs.getPropertyValue("--line-2").trim() };
    const dpr = window.devicePixelRatio || 1;
    const w = el.clientWidth, h = el.clientHeight;
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) { canvas.width = w * dpr; canvas.height = h * dpr; canvas.style.width = `${w}px`; canvas.style.height = `${h}px`; }
    const g = canvas.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    const rows = p.rows, N = n();
    const th = Math.max(1, Math.min(2, h / N));
    for (let i = 0; i < rows.length; i++) {
      const s = rows[i].status;
      if (s.structural && s.state === "unclassified") continue;
      g.fillStyle = s.state === "unclassified" ? col.coral : s.state === "mapped" ? col.jade : s.state === "question" ? col.violet : col.line;
      g.fillRect(3, (i / N) * h, w - 6, th);
    }
  }
  let raf = 0;
  createEffect(() => { p.rows; cancelAnimationFrame(raf); raf = requestAnimationFrame(draw); });
  onMount(() => {
    const ro = new ResizeObserver(() => draw());
    if (el) ro.observe(el);
    onCleanup(() => { ro.disconnect(); cancelAnimationFrame(raf); });
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
      <canvas ref={canvas} class="ticks" />
      <div class="view" style={{ top: `${viewport().top}%`, height: `${viewport().height}%` }} />
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
