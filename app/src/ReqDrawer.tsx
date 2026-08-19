import { createSignal, For, Show, type Component } from "solid-js";
import { api, type Pattern, type Req, type Status } from "./api";
import { slugOf } from "./Classifier";
import type { Toast } from "./Classifier";
import { PATTERNS } from "./ReqPalette";

const STATUSES: Status[] = ["extracted", "assumed", "confirmed", "disputed", "retired"];

/** Requirement detail: statement, anchors (click-through), status actions, retire-with-reason, history. */
export const ReqDrawer: Component<{
  github: string;
  req: Req;
  groups?: string[];
  onJumpAnchor?: (doc: string, spanId: string) => void;
  onChanged: () => void;
  onClose: () => void;
  onGroup?: () => void;
  /** Remove this requirement from the named group (title, as shown). */
  onUngroup?: (title: string) => void;
  toast: (t: Toast | null) => void;
}> = (p) => {
  const [retireReason, setRetireReason] = createSignal("");
  const [retiring, setRetiring] = createSignal(false);
  const [editing, setEditing] = createSignal(false);
  const [draft, setDraft] = createSignal("");
  const [noteDraft, setNoteDraft] = createSignal("");
  const [savingNote, setSavingNote] = createSignal(false);

  async function setStatus(status: Status) {
    try {
      if (status === "retired") {
        if (!retireReason().trim()) { setRetiring(true); return; }
        await api.updateReq(p.github, slugOf(p.req.id), { status, reason: retireReason().trim() });
        setRetiring(false);
        setRetireReason("");
      } else {
        await api.updateReq(p.github, slugOf(p.req.id), { status });
      }
      p.onChanged();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    }
  }
  async function saveStatement(bump: boolean) {
    const s = draft().trim();
    if (!s || s === p.req.statement) { setEditing(false); return; }
    try {
      if (bump) await api.bumpReq(p.github, slugOf(p.req.id), s);
      else await api.updateReq(p.github, slugOf(p.req.id), { statement: s });
      setEditing(false);
      p.onChanged();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    }
  }

  async function setPattern(v: Pattern | null) {
    if (v === (p.req.pattern ?? null)) return;
    try {
      await api.updateReq(p.github, slugOf(p.req.id), { pattern: v });
      p.onChanged();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    }
  }

  async function addNote() {
    const text = noteDraft().trim();
    if (!text || savingNote()) return;
    setSavingNote(true);
    try {
      await api.addReqNote(p.github, slugOf(p.req.id), text);
      setNoteDraft("");
      p.onChanged();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    } finally {
      setSavingNote(false);
    }
  }
  async function removeNote(i: number) {
    try {
      await api.deleteReqNote(p.github, slugOf(p.req.id), i);
      p.onChanged();
    } catch (e) {
      p.toast({ kind: "error", text: String(e) });
    }
  }

  return (
    <div class="drawer">
      <div style={{ display: "flex", gap: "8px", "align-items": "center" }}>
        <span class="mono" style={{ overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>{p.req.id}</span>
        <span class={`chip status-${p.req.status}`}>{p.req.status}</span>
        <span style={{ flex: 1 }} />
        <button class="ghost" onClick={p.onClose} title="close" aria-label="Close details">✕</button>
      </div>
      <Show
        when={editing()}
        fallback={<div class="stmt" onDblClick={() => { setDraft(p.req.statement); setEditing(true); }} title="Double-click to edit">{p.req.statement}</div>}
      >
        <div class="field" style={{ margin: "6px 0 10px" }}>
          <textarea name="statement-edit" aria-label="Edit statement" value={draft()} onInput={(e) => setDraft(e.currentTarget.value)} rows={3} />
          <div style={{ display: "flex", gap: "6px", "align-items": "center" }}>
            <span class="muted" style={{ "font-size": "11.5px", flex: 1 }}>Rewording keeps the rev; a meaning change should bump it.</span>
            <button onClick={() => setEditing(false)}>Cancel</button>
            <button onClick={() => saveStatement(false)}>Save (reword)</button>
            <button class="primary" onClick={() => saveStatement(true)}>Bump rev</button>
          </div>
        </div>
      </Show>
      <dl class="kv">
        <dt>pattern</dt>
        <dd>
          <select
            name="pattern"
            aria-label="EARS pattern"
            class="inline-select"
            onChange={(e) => setPattern((e.currentTarget.value || null) as Pattern | null)}
          >
            <option value="" selected={!p.req.pattern}>—</option>
            <For each={PATTERNS}>{(pt) => <option value={pt.v} selected={p.req.pattern === pt.v} title={pt.hint}>{pt.label}</option>}</For>
          </select>
        </dd>
        <Show when={p.req.rating}><dt>rating</dt><dd class="mono">[{p.req.rating![0]}, {p.req.rating![1]}]</dd></Show>
        <Show when={p.req.owner}><dt>owner</dt><dd>{p.req.owner}</dd></Show>
        <Show when={p.groups && p.groups.length}>
          <dt>groups</dt>
          <dd class="grouplist">
            <For each={p.groups!}>
              {(g) => (
                <span class="chip grp">
                  {g}
                  <Show when={p.onUngroup}><button class="x" title={`Remove from “${g}”`} onClick={() => p.onUngroup!(g)}>✕</button></Show>
                </span>
              )}
            </For>
          </dd>
        </Show>
        <dt>anchors</dt>
        <dd>
          <Show when={p.req.anchors.length} fallback={<span class="muted">none — this requirement has no source sentence</span>}>
            <For each={p.req.anchors}>
              {(a) => (
                <span class="anchor mono" onClick={() => p.onJumpAnchor?.(a.doc, a.span)} title={`${a.doc} · ${a.span}`}>
                  {a.doc.split("/").pop()}:{a.span.slice(2, 8)}{" "}
                </span>
              )}
            </For>
          </Show>
        </dd>
        <Show when={p.req.reason}><dt>reason</dt><dd>{p.req.reason}</dd></Show>
        <Show when={p.req.suspect}><dt>suspect</dt><dd class="loud">{p.req.suspect} — edit the statement or set a status to clear.</dd></Show>
      </dl>
      <div class="actions">
        <For each={STATUSES.filter((s) => s !== p.req.status)}>
          {(s) => <button onClick={() => setStatus(s)}>{s === "retired" ? "retire…" : s}</button>}
        </For>
        <Show when={p.onGroup}><button onClick={p.onGroup}>+ group</button></Show>
      </div>
      <Show when={retiring()}>
        <div class="field" style={{ "margin-top": "8px" }}>
          <label>Why retire? (required)</label>
          <div style={{ display: "flex", gap: "6px" }}>
            <input name="retire-reason" aria-label="Why retire? (required)" value={retireReason()} onInput={(e) => setRetireReason(e.currentTarget.value)} placeholder="e.g. superseded by req~x~2" onKeyDown={(e) => { if (e.key === "Enter") setStatus("retired"); }} />
            <button class="primary" disabled={!retireReason().trim()} onClick={() => setStatus("retired")}>Retire</button>
          </div>
        </div>
      </Show>
      <div class="notes">
        <div class="small muted">notes — commentary only; they never change the rev and are not exported</div>
        <For each={p.req.notes ?? []}>
          {(n, i) => (
            <div class="note">
              <div class="ntext">{n.text}</div>
              <div class="nmeta">
                <span>{n.by} · {n.at.slice(0, 16).replace("T", " ")}</span>
                <button class="ghost" title="delete note" onClick={() => removeNote(i())}>✕</button>
              </div>
            </div>
          )}
        </For>
        <div class="field">
          <textarea
            rows={2}
            name="note"
            aria-label="Add a note"
            placeholder="Add a note — ⌘/Ctrl+⏎ to save"
            value={noteDraft()}
            onInput={(e) => setNoteDraft(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); addNote(); } }}
          />
          <div style={{ display: "flex", "justify-content": "flex-end" }}>
            <button disabled={!noteDraft().trim() || savingNote()} onClick={addNote}>Add note</button>
          </div>
        </div>
      </div>
      <ul class="hist">
        <For each={p.req.history.slice(-5).reverse()}>{(h) => <li>{h.at.slice(0, 16).replace("T", " ")} · {h.by} · {h.op}{h.note ? ` · ${h.note}` : ""}</li>}</For>
      </ul>
    </div>
  );
};
