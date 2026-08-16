/**
 * Transport-agnostic command client. In the Tauri webview it uses `invoke("call")`;
 * in a plain browser (dev/testing) it POSTs to the `kansa serve` bridge. Same core dispatch
 * either way (`ui~core-parity~1`).
 */

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const BRIDGE = (import.meta.env.VITE_KANSA_BRIDGE as string | undefined) ?? "http://127.0.0.1:1430";

export const transport: "tauri" | "http" = inTauri ? "tauri" : "http";

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
};

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
  anchors: { doc: string; span: string }[];
  questions: string[];
  history: { at: string; by: string; op: string; note?: string }[];
};

export type Question = {
  id: string;
  status: "open" | "answered" | "withdrawn";
  quote: string;
  anchors: { doc: string; span: string }[];
  materiality: Level;
  readings: { key: string; text: string; default?: boolean }[];
  affects: string[];
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
  listDocs: (github: string) => call<DocEntry[]>("list_docs", { github }),
  trackDoc: (github: string, path: string) =>
    call<{ doc: string; sha: string; spans: number }>("track_doc", { github, path }),
  untrackDoc: (github: string, path: string) => call<void>("untrack_doc", { github, path }),
  refreshRepo: (github: string) => call<DocChange[]>("refresh_repo", { github }),
  repoStatus: (github: string) => call<RepoStatus>("repo_status", { github }),
  docView: (github: string, doc: string) => call<DocView>("doc_view", { github, doc }),
  listReqs: (github: string) => call<Req[]>("list_reqs", { github }),

  markNonNormative: (github: string, doc: string, spans: string[]) =>
    call<void>("mark_non_normative", { github, doc, spans }),
  unmark: (github: string, doc: string, spans: string[]) => call<void>("unmark", { github, doc, spans }),
  createReq: (
    github: string,
    doc: string,
    spans: string[],
    req: { statement: string; slug?: string; pattern?: Pattern; rating?: [Level, Level]; owner?: string },
  ) => call<Req>("create_req", { github, doc, spans, ...req }),
  attachReq: (github: string, doc: string, spans: string[], slug: string) =>
    call<Req>("attach_req", { github, doc, spans, slug }),
  detachReq: (github: string, doc: string, spans: string[], slug: string) =>
    call<void>("detach_req", { github, doc, spans, slug }),
  updateReq: (
    github: string,
    slug: string,
    patch: { statement?: string; pattern?: Pattern | null; status?: Status; rating?: [Level, Level] | null; owner?: string | null; reason?: string },
  ) => call<Req>("update_req", { github, slug, ...patch }),
  bumpReq: (github: string, slug: string, statement: string) => call<Req>("bump_req", { github, slug, statement }),
  flagQuestion: (
    github: string,
    doc: string,
    spans: string[],
    q: { quote: string; materiality?: Level; readings?: { key: string; text: string }[]; default?: string; affects?: string[] },
  ) => call<Question>("flag_question", { github, doc, spans, ...q }),
  closeRound: (github: string, doc: string) => call<Round>("close_round", { github, doc }),
  export: (github: string, out?: string) => call<ExportResult>("export", { github, out }),

  inventory: (github: string) => call<InventoryRow[]>("inventory", { github }),
  listGroups: (github: string) => call<GroupRollup[]>("list_groups", { github }),
  createGroup: (github: string, title: string, description?: string) => call<Group>("create_group", { github, title, description }),
  assignGroup: (github: string, group: string, reqs: string[]) => call<Group>("assign_group", { github, group, reqs }),
  unassignGroup: (github: string, group: string, reqs: string[]) => call<Group>("unassign_group", { github, group, reqs }),
  updateGroup: (github: string, group: string, patch: { title?: string; description?: string | null }) =>
    call<Group>("update_group", { github, group, ...patch }),
};
