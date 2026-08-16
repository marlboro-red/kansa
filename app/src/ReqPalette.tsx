import { createMemo, createSignal, For, onMount, Show, type Component } from "solid-js";
import type { Level, Pattern, Req } from "./api";
import { slugOf } from "./Classifier";

type Submit =
  | { mode: "attach"; slug: string }
  | { mode: "create"; statement: string; slug?: string; pattern?: Pattern; rating?: [Level, Level] };

const PATTERNS: { v: Pattern; label: string; hint: string }[] = [
  { v: "ubiquitous", label: "Ubiquitous", hint: "The <system> shall <response>" },
  { v: "event-driven", label: "Event-driven", hint: "When <trigger>, the <system> shall <response>" },
  { v: "state-driven", label: "State-driven", hint: "While <state>, the <system> shall <response>" },
  { v: "unwanted", label: "Unwanted", hint: "If <condition>, then the <system> shall <response>" },
  { v: "optional", label: "Optional", hint: "Where <feature>, the <system> shall <response>" },
  { v: "complex", label: "Complex", hint: "Combination of the above" },
];

/** `r` — attach the selection to an existing requirement, or create a new one from it. */
export const ReqPalette: Component<{
  github: string;
  reqs: Req[];
  selectionText: string;
  onClose: () => void;
  onSubmit: (s: Submit) => void;
}> = (p) => {
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const [creating, setCreating] = createSignal(false);
  const [statement, setStatement] = createSignal(p.selectionText);
  const [slug, setSlug] = createSignal("");
  const [pattern, setPattern] = createSignal<Pattern | undefined>(guessPattern(p.selectionText));
  let input: HTMLInputElement | undefined;
  let stmt: HTMLTextAreaElement | undefined;

  const matches = createMemo(() => {
    const q = query().trim().toLowerCase();
    const live = p.reqs.filter((r) => r.status !== "retired");
    if (!q) return live.slice(0, 40);
    return live
      .map((r) => ({ r, s: score(q, r) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .slice(0, 40)
      .map((x) => x.r);
  });
  const total = () => matches().length + 1; // + create row

  onMount(() => input?.focus());

  function choose(i: number) {
    if (i < matches().length) p.onSubmit({ mode: "attach", slug: slugOf(matches()[i].id) });
    else openCreate();
  }
  function openCreate() {
    setCreating(true);
    if (query().trim() && !p.selectionText.trim()) setStatement(query());
    queueMicrotask(() => { stmt?.focus(); stmt?.select(); });
  }
  function submitCreate() {
    const s = statement().trim();
    if (!s) return;
    p.onSubmit({ mode: "create", statement: s, slug: slug().trim() || undefined, pattern: pattern() });
  }

  function onListKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((a) => (a + 1) % total()); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive((a) => (a - 1 + total()) % total()); }
    else if (e.key === "Enter") { e.preventDefault(); choose(active()); }
    else if (e.key === "Tab") { e.preventDefault(); openCreate(); }
  }

  return (
    <div class="overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div class="dialog" role="dialog" aria-label="Map to requirement">
        <div class="dhead">
          <span class="title">{creating() ? "New requirement" : "Map to requirement"}</span>
          <span class="quote" title={p.selectionText}>“{p.selectionText}”</span>
        </div>
        <Show
          when={creating()}
          fallback={
            <>
              <div class="dbody">
                <input
                  ref={input}
                  placeholder="Search requirements by statement or id… (Tab to create new)"
                  value={query()}
                  onInput={(e) => { setQuery(e.currentTarget.value); setActive(0); }}
                  onKeyDown={onListKey}
                />
                <div class="matches">
                  <For each={matches()}>
                    {(r, i) => (
                      <div class="match" classList={{ active: active() === i() }} onMouseEnter={() => setActive(i())} onClick={() => choose(i())}>
                        <span class="id">{r.id}</span>
                        <span class="stmt">{r.statement}</span>
                        <span class={`chip status-${r.status}`}>{r.status}</span>
                      </div>
                    )}
                  </For>
                  <div class="match create" classList={{ active: active() === matches().length }} onMouseEnter={() => setActive(matches().length)} onClick={openCreate}>
                    + Create a new requirement from this selection
                  </div>
                </div>
              </div>
              <div class="dfoot">
                <span class="hint"><span><kbd>↑↓</kbd> choose</span><span><kbd>⏎</kbd> attach</span><span><kbd>tab</kbd> create</span><span><kbd>esc</kbd> cancel</span></span>
                <button onClick={p.onClose}>Cancel</button>
              </div>
            </>
          }
        >
          <div class="dbody">
            <div class="field">
              <label>EARS statement</label>
              <textarea
                ref={stmt}
                value={statement()}
                onInput={(e) => setStatement(e.currentTarget.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submitCreate(); } }}
                rows={3}
              />
              <span class="muted" style={{ "font-size": "11.5px" }}>{PATTERNS.find((x) => x.v === pattern())?.hint ?? "Pick a pattern to see its shape"}</span>
            </div>
            <div class="field">
              <label>Pattern</label>
              <div class="seg" role="radiogroup">
                <For each={PATTERNS}>
                  {(pt) => <button type="button" classList={{ on: pattern() === pt.v }} onClick={() => setPattern(pattern() === pt.v ? undefined : pt.v)}>{pt.label}</button>}
                </For>
              </div>
            </div>
            <div class="field">
              <label>Id slug <span class="muted" style={{ "text-transform": "none", "letter-spacing": 0 }}>(optional — derived from the statement)</span></label>
              <input value={slug()} placeholder={slugPreview(statement())} onInput={(e) => setSlug(e.currentTarget.value.toLowerCase().replace(/[^a-z0-9-]+/g, "-"))} spellcheck={false} />
            </div>
          </div>
          <div class="dfoot">
            <span class="hint"><span><kbd>⌘⏎</kbd> create</span><span><kbd>esc</kbd> cancel</span></span>
            <button onClick={() => setCreating(false)}>Back</button>
            <button class="primary" onClick={submitCreate} disabled={!statement().trim()}>Create &amp; map</button>
          </div>
        </Show>
      </div>
    </div>
  );
};

const STOP = new Set("the a an and or of to in on for with by at as is are be been shall should must will may can system tool it its this that these those when while if then where which who from into than not no any all each every per via so such only also both either neither we user users".split(" "));
/** Same rules as kansa-core::id::slugify(text, 32) — shown as the placeholder so the derived id is no surprise. */
export function slugPreview(text: string, max = 32): string {
  const words = text.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
  const pick = (ws: string[]) => { let out = ""; for (const w of ws) { const need = out ? out.length + 1 + w.length : w.length; if (need > max) break; out = out ? `${out}-${w}` : w; } return out; };
  return pick(words.filter((w) => !STOP.has(w))) || pick(words) || (words[0] ?? "").slice(0, max) || "item";
}

function score(q: string, r: Req): number {
  const id = r.id.toLowerCase();
  const st = r.statement.toLowerCase();
  if (id.includes(q)) return 100 - id.indexOf(q);
  if (st.includes(q)) return 50 - Math.min(40, st.indexOf(q) / 4);
  // all query words present somewhere
  const words = q.split(/\s+/).filter(Boolean);
  if (words.length > 1 && words.every((w) => st.includes(w) || id.includes(w))) return 20;
  return 0;
}

function guessPattern(text: string): Pattern | undefined {
  const t = text.trim().toLowerCase();
  if (/^when\b/.test(t)) return "event-driven";
  if (/^while\b/.test(t)) return "state-driven";
  if (/^if\b/.test(t)) return "unwanted";
  if (/^where\b/.test(t)) return "optional";
  if (/\bshall\b/.test(t)) return "ubiquitous";
  return undefined;
}
