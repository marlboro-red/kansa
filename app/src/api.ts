/**
 * Transport-agnostic command client. In the Tauri webview it uses `invoke("call")`;
 * in a plain browser (dev/testing) it POSTs to the `kansa serve` bridge. Same core dispatch
 * either way (`ui~core-parity~1`).
 */

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const BRIDGE = (import.meta.env.VITE_KANSA_BRIDGE as string | undefined) ?? "http://127.0.0.1:1430";

export const transport: "tauri" | "http" = inTauri ? "tauri" : "http";

/** Folder picker: native dialog in Tauri; a plain path prompt in the browser harness. */
export async function pickFolder(): Promise<string | null> {
  if (inTauri) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const r = await open({ directory: true, multiple: false, title: "Choose a folder of markdown" });
    return typeof r === "string" ? r : null;
  }
  return window.prompt("Folder path (browser dev mode):") || null;
}

async function call<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("call", { name, args });
  }
  const res = await fetch(`${BRIDGE}/api/${name}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  const body = await res.json();
  if (!res.ok) throw new Error(body?.error ?? `${name} failed (${res.status})`);
  return body as T;
}

// ---------- types (mirror kansa-core serde shapes) ----------

export type RepoSummary = {
  github: string;
  default_branch: string;
  store_dir: string;
  tracked: string[];
  last_fetch: string | null;
  kind?: "github" | "local";
  source_dir?: string | null;
};

export type DocEntry = { path: string; tracked: boolean };

export type Meter = {
  total: number;
  classified: number;
  residue: number;
  mapped: number;
  non_normative: number;
  questioned: number;
  open_questions: number;
};

export type DocStatus = {
  doc: string;
  snapshot: string | null;
  meter: Meter | null;
  open_round: number | null;
  rounds_closed: number;
};

export type RepoStatus = {
  github: string;
  default_branch: string;
  docs: DocStatus[];
  rollup: {
    reqs_by_status: Record<string, number>;
    open_questions: number;
    groups: number;
    unexported_changes: boolean;
  };
};

export type DocChange = { doc: string; from: string | null; to: string | null; advanced: boolean };

export type Block = "para" | "heading" | "li" | "row" | "code" | "html";
export type SpanState = "unclassified" | "mapped" | "non-normative" | "question";

export type Span = {
  id: string;
  ord: number;
  block: Block;
  text: string;
  h: string;
  start: number;
  end: number;
  section?: string;
  depth?: number;
};

export type SpanStatus = { state: SpanState; reqs: string[]; questions: string[]; structural: boolean };

export type Context = { branch: string } | { pr: number; head: string };

export type Round = {
  doc: string;
  n: number;
  snapshot: string;
  context: Context;
  opened: string;
  closed?: string | null;
};

export type DocView = {
  doc: string;
  context: Context;
  source: string;
  snapshot: { doc: string; sha: string; segmenter: number; spans: Span[] };
  coverage: { doc: string; snapshot: string; meter: Meter; spans: [string, SpanStatus][] };
  round: Round | null;
  pending?: Reconciliation | null;
  tracked: boolean;
};

export type DocState = { doc: string; snapshot: string; coverage: DocView["coverage"]; round: Round | null; pending?: Reconciliation | null; tracked: boolean };

export type Status = "extracted" | "assumed" | "confirmed" | "disputed" | "retired";
export type Pattern = "ubiquitous" | "event-driven" | "state-driven" | "unwanted" | "optional" | "complex";
export type Level = "H" | "M" | "L";

export type Req = {
  id: string;
  statement: string;
  pattern?: Pattern | null;
  status: Status;
  rating?: [Level, Level] | null;
  owner?: string | null;
  reason?: string | null;
  suspect?: string | null;
  anchors: { doc: string; span: string }[];
  questions: string[];
  notes?: ReqNote[];
  history: { at: string; by: string; op: string; note?: string }[];
};

/** Free-form commentary on a requirement — never bumps the rev, never exported. */
export type ReqNote = { at: string; by: string; text: string };

export type Question = {
  id: string;
  status: "open" | "answered" | "withdrawn";
  quote: string;
  anchors: { doc: string; span: string }[];
  materiality: Level;
  readings: { key: string; text: string; default?: boolean }[];
  affects: string[];
  answer?: { reading: string; note?: string | null; by: string; at: string } | null;
  pending?: { reading: string; note?: string | null; by: string; at: string } | null;
  history: { at: string; by: string; op: string; note?: string }[];
};

export type Group = {
  id: string;
  title: string;
  description?: string | null;
  members: string[];
  history: { at: string; by: string; op: string }[];
};

export type GroupRollup = {
  group: Group;
  members_by_status: Record<string, number>;
  open_questions: number;
  anchors: number;
  findings: { member: string; kind: "missing" | "retired" | "stale-rev" }[];
};

export type InventoryRow = Req & { groups: string[]; docs: string[]; open_questions: number };

export type VerdictKind = "unchanged" | "reworded" | "meaning-changed" | "missing";
export type Decision =
  | { kind: "accept" }
  | { kind: "meaning-changed" }
  | { kind: "reanchor"; span: string }
  | { kind: "drop" }
  | { kind: "retire"; reason: string };
export type Verdict = {
  from: string;
  from_text: string;
  to?: string | null;
  to_text?: string | null;
  kind: VerdictKind;
  similarity: number;
  reqs: string[];
  questions: string[];
  non_normative: boolean;
  decision?: Decision | null;
};
export type Reconciliation = { doc: string; from: string; to: string; verdicts: Verdict[]; added: string[] };

export type PrSummary = {
  number: number;
  title: string;
  head: string;
  head_ref: string;
  base_ref: string;
  author: string;
  updated_at: string;
  draft: boolean;
  files: string[];
  touches: string[];
};

export type Proposed =
  | { kind: "req"; statement: string; pattern?: Pattern | null; slug?: string | null; groups: string[]; attach?: string | null }
  | { kind: "context" }
  | { kind: "question"; quote: string; materiality?: Level | null; readings: string[] };
export type Proposal = { id: string; spans: string[]; proposed: Proposed; status: "proposed" | "accepted" | "rejected"; rationale?: string | null; result?: string | null };
export type PrefillJob = { state: "idle" | "running" | "done" | "error"; doc: string; snapshot: string; done: number; total: number; error?: string | null; model?: string | null; started: string; finished?: string | null };
export type PrefillStatus = { available: boolean; job: PrefillJob | null; proposals: Proposal[] };

export type Cached<T> = { data: T; fetched_at: string; refreshing: boolean };

export type PrDoc = { path: string; status: "added" | "modified" | "deleted" | "renamed"; tracked: boolean };

export type ExportResult = {
  inventory: string;
  exceptions: string;
  items: number;
  exception_count: number;
  validate: { code: number; output: string } | null;
};

// ---------- commands ----------

export const api = {
  kansaHome: () => call<string>("kansa_home"),
  listRepos: () => call<RepoSummary[]>("list_repos"),
  registerRepo: (github: string) => call<RepoSummary>("register_repo", { github }),
  registerLocal: (path: string) => call<RepoSummary>("register_local", { path }),
  listDocs: (github: string) => call<DocEntry[]>("list_docs", { github }),
  trackDoc: (github: string, path: string) =>
    call<{ doc: string; sha: string; spans: number }>("track_doc", { github, path }),
  untrackDoc: (github: string, path: string) => call<void>("untrack_doc", { github, path }),
  refreshRepo: (github: string) => call<DocChange[]>("refresh_repo", { github }),
  repoStatus: (github: string) => call<RepoStatus>("repo_status", { github }),
  docView: (github: string, doc: string, context?: Context, sha?: string) => call<DocView>("doc_view", { github, doc, context, sha }),
  docState: (github: string, doc: string, context?: Context) => call<DocState>("doc_state", { github, doc, context }),
  listReqs: (github: string) => call<Req[]>("list_reqs", { github }),

  markNonNormative: (github: string, doc: string, spans: string[], context?: Context) =>
    call<void>("mark_non_normative", { github, doc, spans, context }),
  unmark: (github: string, doc: string, spans: string[], context?: Context) => call<void>("unmark", { github, doc, spans, context }),
  createReq: (
    github: string,
    doc: string,
    spans: string[],
    req: { statement: string; slug?: string; pattern?: Pattern; rating?: [Level, Level]; owner?: string },
    context?: Context,
  ) => call<Req>("create_req", { github, doc, spans, ...req, context }),
  attachReq: (github: string, doc: string, spans: string[], slug: string, context?: Context) =>
    call<Req>("attach_req", { github, doc, spans, slug, context }),
  detachReq: (github: string, doc: string, spans: string[], slug: string) =>
    call<void>("detach_req", { github, doc, spans, slug }),
  updateReq: (
    github: string,
    slug: string,
    patch: { statement?: string; pattern?: Pattern | null; status?: Status; rating?: [Level, Level] | null; owner?: string | null; reason?: string },
  ) => call<Req>("update_req", { github, slug, ...patch }),
  bumpReq: (github: string, slug: string, statement: string) => call<Req>("bump_req", { github, slug, statement }),
  addReqNote: (github: string, slug: string, text: string) => call<Req>("add_req_note", { github, slug, text }),
  deleteReqNote: (github: string, slug: string, index: number) => call<Req>("delete_req_note", { github, slug, index }),
  flagQuestion: (
    github: string,
    doc: string,
    spans: string[],
    q: { quote: string; materiality?: Level; readings?: { key: string; text: string }[]; default?: string; affects?: string[] },
    context?: Context,
  ) => call<Question>("flag_question", { github, doc, spans, ...q, context }),
  closeRound: (github: string, doc: string, context?: Context) => call<Round>("close_round", { github, doc, context }),
  decideVerdict: (github: string, doc: string, span: string, decision: Decision, context?: Context) =>
    call<Reconciliation>("decide_verdict", { github, doc, span, decision, context }),
  confirmReconciliation: (github: string, doc: string, context?: Context) =>
    call<Reconciliation>("confirm_reconciliation", { github, doc, context }),
  rounds: (github: string, doc: string, context?: Context) => call<Round[]>("rounds", { github, doc, context }),
  listQuestions: (github: string) => call<Question[]>("list_questions", { github }),
  answerQuestion: (github: string, slug: string, reading: string, note?: string) =>
    call<Question>("answer_question", { github, slug, reading, note }),
  resolveHeldAnswer: (github: string, slug: string, apply: boolean) => call<Question>("resolve_held_answer", { github, slug, apply }),
  withdrawQuestion: (github: string, slug: string) => call<Question>("withdraw_question", { github, slug }),
  listPrs: (github: string, force = false) => call<Cached<PrSummary[]>>("list_prs", { github, force }),
  prefillStart: (github: string, doc: string, context?: Context) => call<PrefillJob>("prefill_start", { github, doc, context }),
  prefillStatus: (github: string, doc: string, context?: Context) => call<PrefillStatus>("prefill_status", { github, doc, context }),
  acceptProposal: (github: string, doc: string, id: string, context?: Context) => call<Proposal>("accept_proposal", { github, doc, id, context }),
  rejectProposal: (github: string, doc: string, id: string, context?: Context) => call<Proposal>("reject_proposal", { github, doc, id, context }),
  acceptAllProposals: (github: string, doc: string, context?: Context) => call<number>("accept_all_proposals", { github, doc, context }),
  clearProposals: (github: string, doc: string, context?: Context) => call<void>("clear_proposals", { github, doc, context }),
  openPr: (github: string, pr: number, doc: string) => call<Context>("open_pr", { github, pr, doc }),
  prDocs: (github: string, pr: number, force = false) => call<Cached<PrDoc[]>>("pr_docs", { github, pr, force }),
  export: (github: string, out?: string) => call<ExportResult>("export", { github, out }),

  inventory: (github: string) => call<InventoryRow[]>("inventory", { github }),
  listGroups: (github: string) => call<GroupRollup[]>("list_groups", { github }),
  createGroup: (github: string, title: string, description?: string) => call<Group>("create_group", { github, title, description }),
  assignGroup: (github: string, group: string, reqs: string[]) => call<Group>("assign_group", { github, group, reqs }),
  unassignGroup: (github: string, group: string, reqs: string[]) => call<Group>("unassign_group", { github, group, reqs }),
  updateGroup: (github: string, group: string, patch: { title?: string; description?: string | null }) =>
    call<Group>("update_group", { github, group, ...patch }),
};
