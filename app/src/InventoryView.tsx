import { createMemo, createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { api, type ExportResult, type GroupRollup, type InventoryRow, type Status } from "./api";
import { slugOf } from "./Classifier";
import type { Toast } from "./Classifier";
import { ReqDrawer } from "./ReqDrawer";
import { GroupPalette } from "./GroupPalette";
import { createCached } from "./swr";

const STATUSES: Status[] = ["extracted", "assumed", "confirmed", "disputed", "retired"];
type GroupBy = "none" | "group" | "doc" | "status";

/** Repo-wide inventory (spec §4.2): every requirement, filters, group lens, bulk actions, export. */
export const InventoryView: Component<{
  github: string;
  label?: string;
  onOpenAnchor: (doc: string, spanId: string) => void;
  toast: (t: Toast | null) => void;
}> = (p) => {
  const [rows, { refetch: refetchRows }] = createCached(() => `inventory:${p.github}`, () => api.inventory(p.github));
  const [groups, { refetch: refetchGroups }] = createCached(() => `groups:${p.github}`, () => api.listGroups(p.github));
  const [status, { refetch: refetchStatus }] = createCached(() => `status:${p.github}`, () => api.repoStatus(p.github));
  const refetchAll = () => { refetchRows(); refetchGroups(); refetchStatus(); };

  const [q, setQ] = createSignal("");
  const [statusFilter, setStatusFilter] = createSignal<Set<Status>>(new Set());
  const [docFilter, setDocFilter] = createSignal<string>("");
  const [groupFilter, setGroupFilter] = createSignal<string>("");
  const [groupBy, setGroupBy] = createSignal<GroupBy>("none");
  const [selected, setSelected] = createSignal<Set<string>>(new Set()); // slugs
  const [open, setOpen] = createSignal<string | null>(null); // drawer slug
  const [palette, setPalette] = createSignal(false);
  const [exportResult, setExportResult] = createSignal<ExportResult | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [retireReason, setRetireReason] = createSignal("");
  const [retiring, setRetiring] = createSignal(false);
  const [newGroup, setNewGroup] = createSignal("");
  let search: HTMLInputElement | undefined;

  const docs = createMemo(() => Array.from(new Set((rows() ?? []).flatMap((r) => r.docs))).sort());

  const filtered = createMemo(() => {
    const query = q().trim().toLowerCase();
    const sf = statusFilter();
    const df = docFilter();
    const gf = groupFilter();
    return (rows() ?? []).filter((r) => {
      if (sf.size && !sf.has(r.status)) return false;
      if (df && !r.docs.includes(df)) return false;
      if (gf && !r.groups.includes(gf)) return false;
      if (query && !(r.id.toLowerCase().includes(query) || r.statement.toLowerCase().includes(query) || (r.owner ?? "").toLowerCase().includes(query))) return false;
      return true;
    });
  });

  const sections = createMemo<{ title: string; rows: InventoryRow[] }[]>(() => {
    const rs = filtered();
    switch (groupBy()) {
      case "none": return [{ title: "", rows: rs }];
      case "status": return STATUSES.map((s) => ({ title: s, rows: rs.filter((r) => r.status === s) })).filter((x) => x.rows.length);
      case "doc": {
        const m = new Map<string, InventoryRow[]>();
        for (const r of rs) for (const d of r.docs.length ? r.docs : ["(no anchors)"]) m.set(d, [...(m.get(d) ?? []), r]);
        return [...m.entries()].sort().map(([title, rows]) => ({ title, rows }));
      }
      case "group": {
        const m = new Map<string, InventoryRow[]>();
        for (const r of rs) for (const g of r.groups.length ? r.groups : ["(ungrouped)"]) m.set(g, [...(m.get(g) ?? []), r]);
        return [...m.entries()].sort(([a], [b]) => (a === "(ungrouped)" ? 1 : b === "(ungrouped)" ? -1 : a.localeCompare(b))).map(([title, rows]) => ({ title, rows }));
      }
    }
  });

  const openRow = createMemo(() => (open() ? (rows() ?? []).find((r) => slugOf(r.id) === open()) ?? null : null));

  function toggleSel(slug: string, e?: MouseEvent) {
    const s = new Set(selected());
    if (e?.shiftKey && lastSel) {
      const list = filtered().map((r) => slugOf(r.id));
      const a = list.indexOf(lastSel), b = list.indexOf(slug);
      if (a >= 0 && b >= 0) for (let i = Math.min(a, b); i <= Math.max(a, b); i++) s.add(list[i]);
    } else if (s.has(slug)) s.delete(slug); else s.add(slug);
    lastSel = slug;
    setSelected(s);
  }
  let lastSel: string | null = null;
  const selectAllVisible = () => setSelected(new Set(filtered().map((r) => slugOf(r.id))));

  async function bulkStatus(status: Status) {
    const slugs = [...selected()];
    if (!slugs.length) return;
    if (status === "retired" && !retireReason().trim()) { setRetiring(true); return; }
    setBusy(true);
    try {
      for (const s of slugs) await api.updateReq(p.github, s, status === "retired" ? { status, reason: retireReason().trim() } : { status });
      p.toast({ kind: "info", text: `${slugs.length} requirement(s) → ${status}` });
      setRetiring(false); setRetireReason("");
      refetchAll();
    } catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); }
  }
  async function assign(groupSlug: string) {
    setPalette(false);
    try { await api.assignGroup(p.github, groupSlug, [...selected()]); refetchAll(); p.toast({ kind: "info", text: `Added ${selected().size} to group.` }); }
    catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  async function unassign(groupSlug: string) {
    setPalette(false);
    const n = selected().size;
    try { await api.unassignGroup(p.github, groupSlug, [...selected()]); refetchAll(); p.toast({ kind: "info", text: `Removed ${n} from group.` }); }
    catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  /** The drawer knows group titles, not slugs; groups are unique by title in the picker. */
  async function unassignByTitle(slug: string, title: string) {
    const g = (groups() ?? []).find((x) => x.group.title === title);
    if (!g) return;
    try { await api.unassignGroup(p.github, slugOf(g.group.id), [slug]); refetchAll(); }
    catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  async function createAndAssign(title: string) {
    setPalette(false);
    try { const g = await api.createGroup(p.github, title); await api.assignGroup(p.github, slugOf(g.id), [...selected()]); refetchAll(); p.toast({ kind: "info", text: `Created “${g.title}”.` }); }
    catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  async function createGroup() {
    const t = newGroup().trim();
    if (!t) return;
    try { await api.createGroup(p.github, t); setNewGroup(""); refetchGroups(); } catch (e) { p.toast({ kind: "error", text: String(e) }); }
  }
  async function doExport() {
    setBusy(true);
    try { const r = await api.export(p.github); setExportResult(r); refetchStatus(); }
    catch (e) { p.toast({ kind: "error", text: String(e) }); } finally { setBusy(false); }
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (palette()) return;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) { if (e.key === "Escape") (t as HTMLElement).blur(); return; }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "/") { e.preventDefault(); search?.focus(); }
    else if (e.key === "g" && selected().size) { e.preventDefault(); setPalette(true); }
    else if (e.key.toLowerCase() === "a" && e.shiftKey) { e.preventDefault(); selectAllVisible(); }
    else if (e.key === "Escape") { setSelected(new Set<string>()); setOpen(null); setExportResult(null); }
  }
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  const total = () => (rows() ?? []).length;

  return (
    <div class="invview">
      <header class="invview-head">
        <div>
          <h1>Inventory <span class="muted">· {p.label ?? p.github}</span></h1>
          <Show when={status()}>
            {(s) => (
              <p class="rollup">
                <For each={STATUSES}>{(st) => <span class={`chip status-${st}`}>{st} {s().rollup.reqs_by_status[st] ?? 0}</span>}</For>
                <span class="muted"> · {s().rollup.open_questions} open question{s().rollup.open_questions === 1 ? "" : "s"} · {s().rollup.groups} group{s().rollup.groups === 1 ? "" : "s"}</span>
                <For each={s().docs}>{(d) => <Show when={d.meter}><span class="muted"> · {d.doc.split("/").pop()} residue <b classList={{ loud: d.meter!.residue > 0 }}>{d.meter!.residue}</b></span></Show>}</For>
                <Show when={s().rollup.unexported_changes}><span class="loud"> · unexported changes</span></Show>
              </p>
            )}
          </Show>
        </div>
        <button class="primary" onClick={doExport} disabled={busy()} title="export --format reqtrace">Export to reqtrace</button>
      </header>

      <Show when={exportResult()}>
        {(r) => (
          <div class="export-result" classList={{ bad: !!r().validate && r().validate!.code !== 0 }}>
            <div><b>Exported</b> {r().items} item(s) → <span class="mono" title={r().inventory}>{shortPath(r().inventory)}</span> · {r().exception_count} exception(s) → <span class="mono">{r().exceptions.split("/").pop()}</span></div>
            <Show when={r().validate} fallback={<div class="muted">reqtrace not found on PATH — validate skipped.</div>}>
              {(v) => <pre>{v().code === 0 ? "✓ " : "✗ "}reqtrace validate (exit {v().code}){"\n"}{v().output.trim()}</pre>}
            </Show>
            <button class="ghost" onClick={() => setExportResult(null)}>dismiss</button>
          </div>
        )}
      </Show>

      <div class="invview-body">
        <div class="invview-main">
          <div class="filters">
            <input ref={search} placeholder="Search id, statement, owner…  ( / )" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
            <div class="seg small">
              <For each={STATUSES}>
                {(st) => (
                  <button type="button" classList={{ on: statusFilter().has(st) }} onClick={() => { const s = new Set(statusFilter()); s.has(st) ? s.delete(st) : s.add(st); setStatusFilter(s); }}>
                    {st}
                  </button>
                )}
              </For>
            </div>
            <select value={docFilter()} onChange={(e) => setDocFilter(e.currentTarget.value)}>
              <option value="">all docs</option>
              <For each={docs()}>{(d) => <option value={d}>{d}</option>}</For>
            </select>
            <select value={groupFilter()} onChange={(e) => setGroupFilter(e.currentTarget.value)}>
              <option value="">all groups</option>
              <For each={groups() ?? []}>{(g) => <option value={g.group.title}>{g.group.title}</option>}</For>
            </select>
            <div class="seg small">
              <For each={["none", "group", "doc", "status"] as GroupBy[]}>
                {(g) => <button type="button" classList={{ on: groupBy() === g }} onClick={() => setGroupBy(g)}>{g === "none" ? "flat" : `by ${g}`}</button>}
              </For>
            </div>
            <span class="count muted">{filtered().length}/{total()}</span>
          </div>

          <Show when={selected().size}>
            <div class="bulk">
              <span><b>{selected().size}</b> selected</span>
              <button onClick={() => bulkStatus("assumed")} disabled={busy()}>assumed</button>
              <button onClick={() => bulkStatus("confirmed")} disabled={busy()}>confirmed</button>
              <button onClick={() => bulkStatus("disputed")} disabled={busy()}>disputed</button>
              <button onClick={() => bulkStatus("retired")} disabled={busy()}>retire…</button>
              <button onClick={() => setPalette(true)} disabled={busy()}>+ group <kbd>g</kbd></button>
              <span style={{ flex: 1 }} />
              <button class="ghost" onClick={() => setSelected(new Set<string>())}>clear</button>
              <Show when={retiring()}>
                <input style={{ width: "260px" }} placeholder="reason (required)" value={retireReason()} onInput={(e) => setRetireReason(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") bulkStatus("retired"); }} />
                <button class="primary" disabled={!retireReason().trim()} onClick={() => bulkStatus("retired")}>Retire {selected().size}</button>
              </Show>
            </div>
          </Show>

          <div class="tablewrap">
            <Show when={rows()} fallback={<p class="muted" style={{ padding: "16px" }}>loading…</p>}>
              <Show when={filtered().length} fallback={<div class="inv-empty">{total() ? "Nothing matches these filters." : "No requirements yet — open a doc and classify."}</div>}>
                <For each={sections()}>
                  {(sec) => (
                    <>
                      <Show when={sec.title}><h2 class="sec">{sec.title} <span class="muted">{sec.rows.length}</span></h2></Show>
                      <table class="inv-table">
                        <thead>
                          <tr>
                            <th class="c-chk"><input type="checkbox" checked={sec.rows.every((r) => selected().has(slugOf(r.id))) && sec.rows.length > 0} onChange={(e) => { const s = new Set(selected()); for (const r of sec.rows) e.currentTarget.checked ? s.add(slugOf(r.id)) : s.delete(slugOf(r.id)); setSelected(s); }} /></th>
                            <th class="c-id">id</th>
                            <th>statement</th>
                            <th class="c-status">status</th>
                            <th class="c-docs">docs</th>
                            <th class="c-groups">groups</th>
                            <th class="c-q">?</th>
                          </tr>
                        </thead>
                        <tbody>
                          <For each={sec.rows}>
                            {(r) => (
                              <tr classList={{ sel: selected().has(slugOf(r.id)), open: open() === slugOf(r.id) }} onClick={(e) => { if ((e.target as HTMLElement).tagName === "INPUT") return; setOpen(slugOf(r.id)); }}>
                                <td class="c-chk" onClick={(e) => e.stopPropagation()}><input type="checkbox" checked={selected().has(slugOf(r.id))} onClick={(e) => toggleSel(slugOf(r.id), e)} onChange={() => {}} /></td>
                                <td class="mono id c-id" title={r.id}>{r.id.replace(/^req~/, "")}</td>
                                <td class="stmt">
                                  <div class="text" classList={{ clamp: open() !== slugOf(r.id) }}>{r.statement}</div>
                                  <div class="meta">
                                    <Show when={r.pattern}><span>{r.pattern}</span></Show>
                                    <Show when={r.rating}><span class="mono">[{r.rating![0]},{r.rating![1]}]</span></Show>
                                    <Show when={r.owner}><span>{r.owner}</span></Show>
                                    <span class="narrow-only mono">{r.docs.map((d) => d.split("/").pop()).join(", ")}</span>
                                    <Show when={r.groups.length}><span class="narrow-only">{r.groups.join(", ")}</span></Show>
                                  </div>
                                </td>
                                <td class="c-status"><span class={`chip status-${r.status}`}>{r.status}</span><Show when={r.suspect}><span class="chip suspect" title={r.suspect!}>suspect</span></Show></td>
                                <td class="mono small c-docs" title={r.docs.join(", ")}>{r.docs.map((d) => d.split("/").pop()).join(", ")}</td>
                                <td class="small c-groups" title={r.groups.join(", ")}>{r.groups.join(", ")}</td>
                                <td class="qbadge c-q">{r.open_questions ? `?${r.open_questions}` : ""}</td>
                              </tr>
                            )}
                          </For>
                        </tbody>
                      </table>
                    </>
                  )}
                </For>
              </Show>
            </Show>
          </div>
        </div>

        <aside class="invview-side">
          <Show when={openRow()}>
            {(r) => (
              <ReqDrawer
                github={p.github}
                req={r()}
                groups={r().groups}
                onJumpAnchor={(doc, span) => p.onOpenAnchor(doc, span)}
                onChanged={refetchAll}
                onClose={() => setOpen(null)}
                onGroup={() => { setSelected(new Set([slugOf(r().id)])); setPalette(true); }}
                onUngroup={(title) => unassignByTitle(slugOf(r().id), title)}
                toast={p.toast}
              />
            )}
          </Show>
          <div class="groups">
            <div class="groups-head">
              <h2 style={{ margin: 0 }}>Groups</h2>
              <span class="muted">{(groups() ?? []).length}</span>
            </div>
            <div class="newgroup">
              <input placeholder="New group title…" value={newGroup()} onInput={(e) => setNewGroup(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") createGroup(); }} />
              <button onClick={createGroup} disabled={!newGroup().trim()}>Add</button>
            </div>
            <For each={groups() ?? []} fallback={<p class="muted" style={{ "font-size": "12px" }}>Groups are umbrella labels for oversight — “validation”, “lockout”… Select requirements and press <kbd>g</kbd>.</p>}>
              {(g) => <GroupCard g={g} active={groupFilter() === g.group.title} onClick={() => setGroupFilter(groupFilter() === g.group.title ? "" : g.group.title)} />}
            </For>
          </div>
        </aside>
      </div>

      <Show when={palette()}>
        <GroupPalette groups={groups() ?? []} targets={[...selected()]} onClose={() => setPalette(false)} onAssign={assign} onUnassign={unassign} onCreate={createAndAssign} />
      </Show>
    </div>
  );
};

function shortPath(p: string) {
  const parts = p.split("/");
  return parts.length > 4 ? `…/${parts.slice(-3).join("/")}` : p;
}

const GroupCard: Component<{ g: GroupRollup; active: boolean; onClick: () => void }> = (p) => {
  const total = () => Object.values(p.g.members_by_status).reduce((a, b) => a + b, 0);
  return (
    <button class="groupcard" classList={{ active: p.active }} onClick={p.onClick}>
      <div class="row1"><b>{p.g.group.title}</b><span class="muted mono">{p.g.group.members.length}</span></div>
      <Show when={p.g.group.description}><div class="muted desc">{p.g.group.description}</div></Show>
      <div class="bar">
        <For each={STATUSES}>{(s) => <i class={`seg-${s}`} style={{ width: total() ? `${(100 * (p.g.members_by_status[s] ?? 0)) / total()}%` : "0" }} />}</For>
      </div>
      <div class="row3 muted">
        <span>{p.g.open_questions ? `? ${p.g.open_questions}` : "no open questions"}</span>
        <Show when={p.g.findings.length}><span class="loud">{p.g.findings.length} finding{p.g.findings.length === 1 ? "" : "s"}: {p.g.findings.map((f) => `${f.member} ${f.kind}`).join(", ")}</span></Show>
      </div>
    </button>
  );
};
