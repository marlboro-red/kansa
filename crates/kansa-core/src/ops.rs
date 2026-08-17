//! High-level operations. Every mutation the CLI or app performs goes through here
//! (`ui~core-parity~1`); each takes the store lock for its duration (`ui~lock-scope~1`).

use crate::coverage::{doc_coverage, DocCoverage};
use crate::id::{slugify, Id};
use crate::model::*;
use crate::repo;
use crate::snapshot::Snapshot;
use crate::store::{clone_dir_for, kansa_home, list_repos, store_dir_for, Store};
use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A registered repo: its store + bare clone.
pub struct Workspace {
    pub store: Store,
    pub git: git2::Repository,
}

impl Workspace {
    pub fn open(store_dir: &Path) -> Result<Workspace> {
        let store = Store::open(store_dir)?;
        let cfg = store.repo()?;
        let git = git2::Repository::open_bare(&cfg.local_path)
            .with_context(|| format!("opening clone at {}", cfg.local_path))?;
        Ok(Workspace { store, git })
    }

    /// Open by `owner/name` under the default home.
    pub fn open_github(github: &str) -> Result<Workspace> {
        let home = kansa_home()?;
        Workspace::open(&store_dir_for(&home, &repo::github_slug(github)?))
    }

    pub fn default_context(&self) -> Result<Context> {
        Ok(Context::Branch {
            branch: self.store.repo()?.default_branch,
        })
    }

    pub fn refname(&self, ctx: &Context) -> String {
        match ctx {
            Context::Branch { branch } => repo::branch_ref(branch),
            Context::Pr { pr, .. } => repo::pr_ref(*pr),
        }
    }
}

// ---------- repos ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoSummary {
    pub github: String,
    pub default_branch: String,
    pub store_dir: String,
    pub tracked: Vec<String>,
    pub last_fetch: Option<String>,
    #[serde(default)]
    pub kind: RepoKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dir: Option<String>,
}

pub fn list_registered() -> Result<Vec<RepoSummary>> {
    let home = kansa_home()?;
    Ok(list_repos(&home)?
        .into_iter()
        .map(|c| RepoSummary {
            store_dir: store_dir_for(&home, &c.github)
                .to_string_lossy()
                .into_owned(),
            kind: c.kind,
            source_dir: c.source_dir,
            github: c.github,
            default_branch: c.default_branch,
            tracked: c.tracked.into_iter().map(|t| t.path).collect(),
            last_fetch: c.last_fetch.map(|t| t.to_string()),
        })
        .collect())
}

/// Register a GitHub repo: clone it (bare) and create its store. Idempotent.
pub fn register_repo(github: &str) -> Result<Workspace> {
    let home = kansa_home()?;
    register_repo_in(&home, github)
}

pub fn register_repo_in(home: &Path, github: &str) -> Result<Workspace> {
    let slug = repo::github_slug(github)?;
    let url = repo::github_url(github)?;
    register_repo_from_url(home, &slug, &url)
}

/// Same as `register_repo_in` but with an explicit clone URL (tests use `file://`).
pub fn register_repo_from_url(home: &Path, slug: &str, url: &str) -> Result<Workspace> {
    let store_dir = store_dir_for(home, slug);
    let clone_dir = clone_dir_for(home, slug);
    let git = repo::clone_or_open(url, &clone_dir)?;
    let store = if store_dir.join("repo.yaml").exists() {
        Store::open(&store_dir)?
    } else {
        let default_branch = repo::default_branch(&git)?;
        Store::init(
            &store_dir,
            &RepoConfig {
                github: slug.to_string(),
                remote: url.to_string(),
                kind: Default::default(),
                source_dir: None,
                default_branch,
                local_path: clone_dir.to_string_lossy().replace('\\', "/"),
                tracked: vec![],
                registered_at: now(),
                last_fetch: Some(now()),
            },
        )?
    };
    Ok(Workspace { store, git })
}

/// Register a local folder (no GitHub): kansa keeps a private git history of its markdown
/// files under `clones/`, so snapshots, refresh and reconciliation work exactly as for repos.
pub fn register_local(dir: &Path) -> Result<Workspace> {
    register_local_in(&kansa_home()?, dir)
}

pub fn register_local_in(home: &Path, dir: &Path) -> Result<Workspace> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("folder {} not found", dir.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "folder".into());
    // Stable, unique slug: local/<name>-<hash of path>
    let h = crate::snapshot::content_hash(&dir.to_string_lossy());
    let slug = format!("local/{}-{}", slugify(&name, 24), &h[..6]);
    let store_dir = store_dir_for(home, &slug);
    let clone_dir = clone_dir_for(home, &slug);
    let git = repo::open_local_backing(&clone_dir)?;
    repo::import_folder(&git, &dir, "main")?;
    let store = if store_dir.join("repo.yaml").exists() {
        Store::open(&store_dir)?
    } else {
        Store::init(
            &store_dir,
            &RepoConfig {
                github: slug.clone(),
                remote: format!("file://{}", dir.to_string_lossy().replace('\\', "/")),
                kind: RepoKind::Local,
                source_dir: Some(dir.to_string_lossy().replace('\\', "/")),
                default_branch: "main".into(),
                local_path: clone_dir.to_string_lossy().replace('\\', "/"),
                tracked: vec![],
                registered_at: now(),
                last_fetch: Some(now()),
            },
        )?
    };
    Ok(Workspace { store, git })
}

// ---------- docs ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    pub path: String,
    pub tracked: bool,
}

/// Markdown docs on the default branch, flagged tracked/untracked (`ui~repo-docs~1`).
pub fn list_docs(ws: &Workspace) -> Result<Vec<DocEntry>> {
    let cfg = ws.store.repo()?;
    let all = repo::list_markdown(&ws.git, &repo::branch_ref(&cfg.default_branch))?;
    Ok(all
        .into_iter()
        .map(|p| DocEntry {
            tracked: cfg.tracked.iter().any(|t| t.path == p),
            path: p,
        })
        .collect())
}

/// Start tracking a doc: records it and builds its first snapshot on the default branch.
pub fn track_doc(ws: &Workspace, path: &str) -> Result<Snapshot> {
    let _l = ws.store.lock()?;
    let mut cfg = ws.store.repo()?;
    let path = path.replace('\\', "/");
    let ctx = Context::Branch {
        branch: cfg.default_branch.clone(),
    };
    let on_default = repo::read_blob(&ws.git, &repo::branch_ref(&cfg.default_branch), &path)?;
    let snap = match on_default {
        Some((content, _)) => {
            let snap = Snapshot::build(&path, &content);
            ws.store.save_snapshot(&snap)?;
            if ws.store.current_sha(&ctx, &path)?.is_none() {
                ws.store.set_current_sha(&ctx, &path, &snap.sha)?;
            }
            snap
        }
        None => {
            // Not on the default branch yet (e.g. a file added in a PR): track it anyway; the
            // default-branch snapshot arrives on the next refresh after merge. Return a snapshot
            // from any fetched PR head that has the file so callers get something to show.
            let mut found = None;
            for r in ws.git.references_glob("refs/pull/*/head")? {
                let r = r?;
                if let Some(name) = r.name() {
                    if let Some((c, _)) = repo::read_blob(&ws.git, name, &path)? {
                        found = Some(Snapshot::build(&path, &c));
                        break;
                    }
                }
            }
            found.ok_or_else(|| {
                anyhow!(
                    "{path} not found on {} or any fetched PR",
                    cfg.default_branch
                )
            })?
        }
    };
    if !cfg.tracked.iter().any(|t| t.path == path) {
        cfg.tracked.push(TrackedDoc { path: path.clone() });
        ws.store.save_repo(&cfg)?;
    }
    Ok(snap)
}

pub fn untrack_doc(ws: &Workspace, path: &str) -> Result<()> {
    let _l = ws.store.lock()?;
    let mut cfg = ws.store.repo()?;
    cfg.tracked.retain(|t| t.path != path);
    ws.store.save_repo(&cfg)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocChange {
    pub doc: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// True if the current pointer was advanced (no classification existed yet).
    pub advanced: bool,
}

/// Fetch origin; for each tracked doc build a snapshot of the new blob if it changed
/// (`ui~repo-refresh~1`). Pointers advance only when nothing is classified yet; otherwise a
/// reconciliation is computed and stored as pending (`obj~anchor~1`).
pub fn refresh(ws: &Workspace) -> Result<Vec<DocChange>> {
    let cfg0 = ws.store.repo()?;
    match (cfg0.kind, &cfg0.source_dir) {
        (RepoKind::Local, Some(dir)) => {
            repo::import_folder(&ws.git, Path::new(dir), &cfg0.default_branch)?;
        }
        _ => repo::fetch(&ws.git)?,
    }
    let _l = ws.store.lock()?;
    let mut cfg = ws.store.repo()?;
    cfg.last_fetch = Some(now());
    ws.store.save_repo(&cfg)?;
    let ctx = Context::Branch {
        branch: cfg.default_branch.clone(),
    };
    detect_changes(ws, &ctx, &cfg)
}

/// Compare each tracked doc at the context's ref against its current snapshot.
fn detect_changes(ws: &Workspace, ctx: &Context, cfg: &RepoConfig) -> Result<Vec<DocChange>> {
    let refname = ws.refname(ctx);
    let mut changes = vec![];
    for t in &cfg.tracked {
        let cur = ws.store.current_sha(ctx, &t.path)?;
        match repo::read_blob(&ws.git, &refname, &t.path)? {
            None => {
                if cur.is_some() {
                    changes.push(DocChange {
                        doc: t.path.clone(),
                        from: cur,
                        to: None,
                        advanced: false,
                    });
                }
            }
            Some((content, sha)) => {
                if cur.as_deref() == Some(sha.as_str()) {
                    continue;
                }
                if let Some(p) = ws.store.pending(ctx, &t.path)? {
                    if p.to == sha {
                        continue; // already pending against this exact blob
                    }
                }
                let snap = Snapshot::build(&t.path, &content);
                ws.store.save_snapshot(&snap)?;
                let advanced = match &cur {
                    None => true,
                    Some(cur_sha) => {
                        let old = ws.store.load_snapshot(&t.path, cur_sha)?;
                        let cov = doc_coverage(&ws.store, ctx, &old)?;
                        let classified = cov.meter.classified > 0;
                        if classified {
                            let recon = build_reconciliation(ws, ctx, &old, &snap, &cov)?;
                            ws.store.save_pending(ctx, &recon)?;
                            false
                        } else {
                            true
                        }
                    }
                };
                if advanced {
                    ws.store.set_current_sha(ctx, &t.path, &snap.sha)?;
                }
                changes.push(DocChange {
                    doc: t.path.clone(),
                    from: cur,
                    to: Some(snap.sha),
                    advanced,
                });
            }
        }
    }
    Ok(changes)
}

fn build_reconciliation(
    ws: &Workspace,
    ctx: &Context,
    old: &Snapshot,
    new: &Snapshot,
    cov: &DocCoverage,
) -> Result<crate::reconcile::Reconciliation> {
    let marks = ws.store.marks(ctx, &old.doc)?;
    let by_id: std::collections::HashMap<&str, &crate::coverage::SpanStatus> =
        cov.spans.iter().map(|(id, st)| (id.as_str(), st)).collect();
    Ok(crate::reconcile::reconcile(&old.doc, old, new, |id| {
        let st = by_id.get(id)?;
        let nn = marks.spans.contains_key(id);
        if st.reqs.is_empty() && st.questions.is_empty() && !nn {
            return None;
        }
        Some((st.reqs.clone(), st.questions.clone(), nn))
    }))
}

/// Record the human decision on one verdict.
pub fn decide_verdict(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    from_span: &str,
    decision: crate::reconcile::Decision,
) -> Result<crate::reconcile::Reconciliation> {
    use crate::reconcile::{Decision, VerdictKind};
    let _l = ws.store.lock()?;
    let mut r = ws
        .store
        .pending(ctx, doc)?
        .ok_or_else(|| anyhow!("no pending reconciliation for {doc}"))?;
    let new = ws.store.load_snapshot(doc, &r.to)?;
    let v = r
        .verdicts
        .iter_mut()
        .find(|v| v.from == from_span)
        .ok_or_else(|| anyhow!("no verdict for span {from_span}"))?;
    match &decision {
        Decision::Accept | Decision::MeaningChanged if v.to.is_none() => {
            bail!("cannot accept a missing span — re-anchor, drop, or retire")
        }
        Decision::Reanchor { span } => {
            let sp = new
                .span(span)
                .ok_or_else(|| anyhow!("span {span} not in new snapshot"))?;
            v.to = Some(sp.id.clone());
            v.to_text = Some(sp.text.clone());
            v.similarity = crate::reconcile::similarity(&v.from_text, &sp.text);
            if v.kind == VerdictKind::Missing {
                v.kind = VerdictKind::Reworded;
            }
        }
        Decision::MeaningChanged => v.kind = VerdictKind::MeaningChanged,
        Decision::Retire { reason } if reason.trim().is_empty() => {
            bail!("retiring requires a reason")
        }
        _ => {}
    }
    v.decision = Some(decision);
    ws.store.save_pending(ctx, &r)?;
    Ok(r)
}

/// Apply all decisions: remap anchors, close the old round, advance the pointer
/// (`obj~round-supersede~1`). Fails if any verdict is still undecided.
pub fn confirm_reconciliation(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    by: &str,
) -> Result<crate::reconcile::Reconciliation> {
    use crate::reconcile::Decision;
    let _l = ws.store.lock()?;
    let r = ws
        .store
        .pending(ctx, doc)?
        .ok_or_else(|| anyhow!("no pending reconciliation for {doc}"))?;
    if r.unconfirmed() > 0 {
        bail!("{} verdict(s) still need a decision", r.unconfirmed());
    }
    let mut marks_new = Marks::default();
    let old_marks = ws.store.marks(ctx, doc)?;
    for v in &r.verdicts {
        let decision = v.decision.clone().unwrap_or(Decision::Accept);
        let target: Option<String> = match &decision {
            Decision::Accept | Decision::MeaningChanged => v.to.clone(),
            Decision::Reanchor { span } => Some(span.clone()),
            Decision::Drop | Decision::Retire { .. } => None,
        };
        // marks
        if let (Some(m), Some(t)) = (old_marks.spans.get(&v.from), &target) {
            marks_new.spans.insert(t.clone(), m.clone());
        }
        // requirements
        for rid in &v.reqs {
            let id: Id = rid.parse()?;
            let mut revs = ws.store.req_revs(&id.slug)?;
            let Some(cur) = revs.last_mut() else { continue };
            let before = cur.anchors.clone();
            cur.anchors.retain(|a| !(a.doc == doc && a.span == v.from));
            if let Some(t) = &target {
                let a = Anchor {
                    doc: doc.into(),
                    span: t.clone(),
                };
                if !cur.anchors.contains(&a) {
                    cur.anchors.push(a);
                }
            }
            match &decision {
                Decision::MeaningChanged => {
                    cur.suspect = Some(format!(
                        "source changed meaning in {} — review statement",
                        &r.to[..8.min(r.to.len())]
                    ));
                    cur.history.push(
                        History::new(by, "meaning-changed")
                            .change(Some(&before), Some(&cur.anchors))
                            .note(v.to_text.clone().unwrap_or_default()),
                    );
                }
                Decision::Retire { reason } => {
                    cur.status = Status::Retired;
                    cur.reason = Some(reason.clone());
                    cur.history.push(History::new(by, "retire").note(format!(
                        "source missing after {}: {reason}",
                        &r.to[..8.min(r.to.len())]
                    )));
                }
                _ => cur
                    .history
                    .push(History::new(by, "reconcile").change(Some(&before), Some(&cur.anchors))),
            }
            ws.store.save_req_revs(&id.slug, &revs)?;
        }
        // questions
        for qid in &v.questions {
            let id: Id = qid.parse()?;
            let mut revs = ws.store.qst_revs(&id.slug)?;
            let Some(cur) = revs.last_mut() else { continue };
            cur.anchors.retain(|a| !(a.doc == doc && a.span == v.from));
            if let Some(t) = &target {
                cur.anchors.push(Anchor {
                    doc: doc.into(),
                    span: t.clone(),
                });
            }
            cur.history.push(History::new(by, "reconcile"));
            ws.store.save_qst_revs(&id.slug, &revs)?;
        }
    }
    ws.store.save_marks(ctx, doc, &marks_new)?;
    // close the old round (superseded), advance, clear pending
    if let Some(mut round) = ws.store.open_round(ctx, doc)? {
        let mut summary = round.summary.clone().unwrap_or_default();
        let counts = r.counts();
        summary.verdicts = Some(serde_yaml::to_value(
            counts
                .iter()
                .map(|(k, v)| (format!("{k:?}").to_lowercase(), *v))
                .collect::<std::collections::BTreeMap<_, _>>(),
        )?);
        summary.retired = r
            .verdicts
            .iter()
            .filter(|v| matches!(v.decision, Some(Decision::Retire { .. })))
            .flat_map(|v| v.reqs.iter().filter_map(|x| x.parse().ok()))
            .collect();
        round.summary = Some(summary);
        round.closed = Some(now());
        ws.store.save_round(&round)?;
    }
    ws.store.set_current_sha(ctx, doc, &r.to)?;
    ws.store.clear_pending(ctx, doc)?;
    Ok(r)
}

/// Everything the classifier needs to render a doc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocView {
    pub doc: String,
    pub context: Context,
    pub source: String,
    pub snapshot: Snapshot,
    pub coverage: DocCoverage,
    pub round: Option<Round>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<crate::reconcile::Reconciliation>,
    pub tracked: bool,
}

pub fn doc_view(ws: &Workspace, ctx: &Context, doc: &str) -> Result<DocView> {
    doc_view_at(ws, ctx, doc, None)
}

/// Render a specific snapshot (e.g. the incoming one during reconciliation) with the
/// context's classification projected onto it.
pub fn doc_view_at(ws: &Workspace, ctx: &Context, doc: &str, sha: Option<&str>) -> Result<DocView> {
    let snap = match sha {
        Some(sha) => ws.store.load_snapshot_arc(doc, sha)?,
        None => ws
            .store
            .current_snapshot_arc(ctx, doc)?
            .ok_or_else(|| anyhow!("{doc} has no snapshot in this context — track it first"))?,
    };
    // Source: prefer the exact blob by sha; fall back to the ref.
    let source = match ws.git.find_blob(git2::Oid::from_str(&snap.sha)?) {
        Ok(b) => String::from_utf8_lossy(b.content()).into_owned(),
        Err(_) => repo::read_blob(&ws.git, &ws.refname(ctx), doc)?
            .map(|(c, _)| c)
            .unwrap_or_default(),
    };
    let coverage = doc_coverage(&ws.store, ctx, &snap)?;
    let round = ws.store.open_round(ctx, doc)?;
    let pending = ws.store.pending(ctx, doc)?;
    let tracked = ws.store.repo()?.tracked.iter().any(|t| t.path == doc);
    Ok(DocView {
        doc: doc.into(),
        context: ctx.clone(),
        source,
        snapshot: (*snap).clone(),
        coverage,
        round,
        pending,
        tracked,
    })
}

/// The cheap part of `DocView`: everything that changes on classification, without the
/// snapshot or source. Clients refetch this after each mutation (`ui~perf-classify~1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocState {
    pub doc: String,
    pub snapshot: String,
    pub coverage: DocCoverage,
    pub round: Option<Round>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<crate::reconcile::Reconciliation>,
    pub tracked: bool,
}

pub fn doc_state(ws: &Workspace, ctx: &Context, doc: &str) -> Result<DocState> {
    let snap = ws
        .store
        .current_snapshot_arc(ctx, doc)?
        .ok_or_else(|| anyhow!("{doc} has no snapshot in this context"))?;
    let coverage = doc_coverage(&ws.store, ctx, &snap)?;
    Ok(DocState {
        doc: doc.into(),
        snapshot: snap.sha.clone(),
        coverage,
        round: ws.store.open_round(ctx, doc)?,
        pending: ws.store.pending(ctx, doc)?,
        tracked: ws.store.repo()?.tracked.iter().any(|t| t.path == doc),
    })
}

// ---------- classification (core loop) ----------

/// Ensure an open round exists for (ctx, doc) — called on first mutation (`obj~round-open~1`).
fn ensure_round(store: &Store, ctx: &Context, doc: &str) -> Result<Round> {
    if !store.repo()?.tracked.iter().any(|t| t.path == doc) {
        bail!("{doc} is not tracked — track it first to classify");
    }
    if let Some(r) = store.open_round(ctx, doc)? {
        return Ok(r);
    }
    let sha = store
        .current_sha(ctx, doc)?
        .ok_or_else(|| anyhow!("{doc} has no current snapshot"))?;
    let n = store.rounds(ctx, doc)?.last().map(|r| r.n + 1).unwrap_or(1);
    let r = Round {
        doc: doc.into(),
        n,
        snapshot: sha,
        context: ctx.clone(),
        opened: now(),
        closed: None,
        summary: None,
    };
    store.save_round(&r)?;
    Ok(r)
}

fn check_spans(
    store: &Store,
    ctx: &Context,
    doc: &str,
    spans: &[String],
) -> Result<std::sync::Arc<Snapshot>> {
    let snap = store
        .current_snapshot_arc(ctx, doc)?
        .ok_or_else(|| anyhow!("{doc} has no current snapshot"))?;
    let index = snap.index();
    for s in spans {
        if !index.contains_key(s.as_str()) {
            bail!("span `{s}` not in current snapshot of {doc}");
        }
    }
    Ok(snap)
}

/// `c` — mark spans non-normative.
pub fn mark_non_normative(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    spans: &[String],
    by: &str,
) -> Result<()> {
    let _l = ws.store.lock()?;
    check_spans(&ws.store, ctx, doc, spans)?;
    ensure_round(&ws.store, ctx, doc)?;
    let mut marks = ws.store.marks(ctx, doc)?;
    for s in spans {
        marks.spans.insert(
            s.clone(),
            Mark {
                kind: MarkKind::NonNormative,
                by: by.into(),
                at: now(),
                note: None,
            },
        );
    }
    ws.store.save_marks(ctx, doc, &marks)
}

/// Remove a non-normative mark (back to unclassified).
pub fn unmark(ws: &Workspace, ctx: &Context, doc: &str, spans: &[String]) -> Result<()> {
    let _l = ws.store.lock()?;
    let mut marks = ws.store.marks(ctx, doc)?;
    for s in spans {
        marks.spans.remove(s);
    }
    ws.store.save_marks(ctx, doc, &marks)
}

pub struct NewReq<'a> {
    pub statement: &'a str,
    pub slug: Option<&'a str>,
    pub pattern: Option<Pattern>,
    pub rating: Option<Rating>,
    pub owner: Option<&'a str>,
}

/// `r` — create a requirement anchored to spans.
pub fn create_req(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    spans: &[String],
    new: NewReq<'_>,
    by: &str,
) -> Result<ReqRev> {
    let _l = ws.store.lock()?;
    check_spans(&ws.store, ctx, doc, spans)?;
    ensure_round(&ws.store, ctx, doc)?;
    let base = new
        .slug
        .map(|s| s.to_string())
        .unwrap_or_else(|| slugify(new.statement, 32));
    let slug = ws.store.free_req_slug(&base)?;
    let id = Id::new("req", &slug, 1)?;
    let mut r = ReqRev::new(id, new.statement, by);
    r.pattern = new.pattern;
    r.rating = new.rating;
    r.owner = new.owner.map(String::from);
    r.anchors = spans
        .iter()
        .map(|s| Anchor {
            doc: doc.into(),
            span: s.clone(),
        })
        .collect();
    ws.store.save_req_revs(&slug, &[r.clone()])?;
    Ok(r)
}

/// `r` (attach) — anchor an existing requirement's current rev to more spans.
pub fn attach_req(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    spans: &[String],
    slug: &str,
    by: &str,
) -> Result<ReqRev> {
    let _l = ws.store.lock()?;
    check_spans(&ws.store, ctx, doc, spans)?;
    ensure_round(&ws.store, ctx, doc)?;
    let mut revs = ws.store.req_revs(slug)?;
    let cur = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no requirement `{slug}`"))?;
    let before = cur.anchors.clone();
    for s in spans {
        let a = Anchor {
            doc: doc.into(),
            span: s.clone(),
        };
        if !cur.anchors.contains(&a) {
            cur.anchors.push(a);
        }
    }
    cur.history
        .push(History::new(by, "anchor").change(Some(&before), Some(&cur.anchors)));
    let out = cur.clone();
    ws.store.save_req_revs(slug, &revs)?;
    Ok(out)
}

/// Remove anchors from a requirement's current rev.
pub fn detach_req(ws: &Workspace, doc: &str, spans: &[String], slug: &str, by: &str) -> Result<()> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.req_revs(slug)?;
    let cur = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no requirement `{slug}`"))?;
    let before = cur.anchors.clone();
    cur.anchors
        .retain(|a| !(a.doc == doc && spans.contains(&a.span)));
    cur.history
        .push(History::new(by, "unanchor").change(Some(&before), Some(&cur.anchors)));
    ws.store.save_req_revs(slug, &revs)
}

/// Update editable fields of the current rev without bumping (no meaning change).
pub struct ReqPatch<'a> {
    pub statement: Option<&'a str>,
    pub pattern: Option<Option<Pattern>>,
    pub status: Option<Status>,
    pub rating: Option<Option<Rating>>,
    pub owner: Option<Option<&'a str>>,
    pub reason: Option<&'a str>,
}

pub fn update_req(ws: &Workspace, slug: &str, patch: ReqPatch<'_>, by: &str) -> Result<ReqRev> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.req_revs(slug)?;
    let cur = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no requirement `{slug}`"))?;
    let before = cur.clone();
    if let Some(s) = patch.statement {
        cur.statement = s.into();
    }
    if let Some(p) = patch.pattern {
        cur.pattern = p;
    }
    if let Some(st) = patch.status {
        if st == Status::Retired
            && patch
                .reason
                .map(|r| r.trim().is_empty())
                .unwrap_or(cur.reason.is_none())
        {
            bail!("retiring requires a non-empty reason");
        }
        cur.status = st;
    }
    if let Some(r) = patch.rating {
        cur.rating = r;
    }
    if let Some(o) = patch.owner {
        cur.owner = o.map(String::from);
    }
    if let Some(r) = patch.reason {
        cur.reason = Some(r.into());
    }
    if before.status != cur.status || before.statement != cur.statement {
        cur.suspect = None;
    }
    let mut h = History::new(by, "update");
    if before.status != cur.status {
        h = h.change(Some(&before.status), Some(&cur.status));
        h.op = "status".into();
    } else if before.statement != cur.statement {
        h = h.change(Some(&before.statement), Some(&cur.statement));
        h.op = "statement".into();
    }
    cur.history.push(h);
    let out = cur.clone();
    ws.store.save_req_revs(slug, &revs)?;
    Ok(out)
}

/// Bump a requirement to a new rev (meaning changed). Prior rev is kept (`obj~req-revs~1`).
pub fn bump_req(ws: &Workspace, slug: &str, statement: &str, by: &str) -> Result<ReqRev> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.req_revs(slug)?;
    let cur = revs
        .last()
        .ok_or_else(|| anyhow!("no requirement `{slug}`"))?
        .clone();
    let mut next = cur.clone();
    next.id = cur.id.with_rev(cur.id.rev + 1);
    next.statement = statement.into();
    next.history =
        vec![History::new(by, "rev").change(Some(&cur.statement), Some(&next.statement))];
    revs.push(next.clone());
    ws.store.save_req_revs(slug, &revs)?;
    Ok(next)
}

pub struct NewQuestion<'a> {
    pub quote: &'a str,
    pub materiality: Level,
    pub readings: Vec<(String, String)>,
    pub default: Option<String>,
    pub affects: Vec<Id>,
    pub slug: Option<&'a str>,
}

/// `q` — flag spans as a question.
pub fn flag_question(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    spans: &[String],
    new: NewQuestion<'_>,
    by: &str,
) -> Result<Question> {
    let _l = ws.store.lock()?;
    check_spans(&ws.store, ctx, doc, spans)?;
    ensure_round(&ws.store, ctx, doc)?;
    let base = new
        .slug
        .map(|s| s.to_string())
        .unwrap_or_else(|| slugify(new.quote, 32));
    let mut slug = base.clone();
    let mut i = 1;
    while !ws.store.qst_revs(&slug)?.is_empty() {
        i += 1;
        slug = format!("{base}-{i}");
    }
    let id = Id::new("qst", &slug, 1)?;
    let mut affects_revs = vec![];
    for a in &new.affects {
        let cur = ws
            .store
            .current_req(&a.slug)?
            .ok_or_else(|| anyhow!("no requirement `{}`", a.slug))?;
        affects_revs.push(cur.id.clone());
    }
    let q = Question {
        id: id.clone(),
        status: QstStatus::Open,
        quote: new.quote.into(),
        anchors: spans
            .iter()
            .map(|s| Anchor {
                doc: doc.into(),
                span: s.clone(),
            })
            .collect(),
        materiality: new.materiality,
        readings: new
            .readings
            .into_iter()
            .map(|(key, text)| Reading {
                default: new.default.as_deref() == Some(key.as_str()),
                key,
                text,
            })
            .collect(),
        affects: new.affects.clone(),
        answer: None,
        pending: None,
        affects_revs,
        history: vec![History::new(by, "raise")],
    };
    ws.store.save_qst_revs(&slug, std::slice::from_ref(&q))?;
    // link back from affected reqs
    for a in &new.affects {
        let mut revs = ws.store.req_revs(&a.slug)?;
        if let Some(cur) = revs.last_mut() {
            if !cur.questions.contains(&id) {
                cur.questions.push(id.clone());
                cur.history
                    .push(History::new(by, "question").note(id.to_string()));
                ws.store.save_req_revs(&a.slug, &revs)?;
            }
        }
    }
    Ok(q)
}

/// Answer a question (`obj~qst-apply~1`, `obj~qst-conflict~1`).
pub fn answer_question(
    ws: &Workspace,
    slug: &str,
    reading: &str,
    note: Option<&str>,
    by: &str,
) -> Result<Question> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.qst_revs(slug)?;
    let q = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no question `{slug}`"))?;
    if q.status != QstStatus::Open {
        bail!("question is {:?}", q.status);
    }
    if !q.readings.is_empty() && !q.readings.iter().any(|r| r.key == reading) {
        bail!("unknown reading `{reading}`");
    }
    let ans = Answer {
        reading: reading.into(),
        note: note.map(String::from).filter(|s| !s.trim().is_empty()),
        by: by.into(),
        at: now(),
    };
    // conflict: any affected requirement's current rev moved since the question was raised
    let mut conflicts = vec![];
    for (i, a) in q.affects.iter().enumerate() {
        let cur = ws.store.current_req(&a.slug)?;
        let raised_rev = q.affects_revs.get(i).map(|r| r.rev).unwrap_or(a.rev);
        match cur {
            Some(c) if c.id.rev != raised_rev => conflicts.push(format!(
                "{} is now rev {} (was {})",
                a.slug, c.id.rev, raised_rev
            )),
            None => conflicts.push(format!("{} no longer exists", a.slug)),
            _ => {}
        }
    }
    if !conflicts.is_empty() {
        q.pending = Some(ans);
        q.history
            .push(History::new(by, "answer-held").note(conflicts.join("; ")));
        let out = q.clone();
        ws.store.save_qst_revs(slug, &revs)?;
        return Ok(out);
    }
    q.answer = Some(ans);
    q.pending = None;
    q.status = QstStatus::Answered;
    q.history
        .push(History::new(by, "answer").note(reading.to_string()));
    let out = q.clone();
    ws.store.save_qst_revs(slug, &revs)?;
    for a in &out.affects {
        let mut rr = ws.store.req_revs(&a.slug)?;
        if let Some(cur) = rr.last_mut() {
            cur.history.push(
                History::new(by, "question-answered").note(format!("{} → {reading}", out.id)),
            );
            ws.store.save_req_revs(&a.slug, &rr)?;
        }
    }
    Ok(out)
}

/// Resolve a held answer after the human has looked at the conflict: apply it anyway or discard.
pub fn resolve_held_answer(ws: &Workspace, slug: &str, apply: bool, by: &str) -> Result<Question> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.qst_revs(slug)?;
    let q = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no question `{slug}`"))?;
    let held = q.pending.take().ok_or_else(|| anyhow!("no held answer"))?;
    if apply {
        q.answer = Some(held);
        q.status = QstStatus::Answered;
        // re-pin affects revs to current so the conflict is acknowledged
        let mut pinned = vec![];
        for a in &q.affects {
            pinned.push(
                ws.store
                    .current_req(&a.slug)?
                    .map(|c| c.id)
                    .unwrap_or(a.clone()),
            );
        }
        q.affects_revs = pinned;
        q.history
            .push(History::new(by, "answer").note("applied after conflict review"));
    } else {
        q.history.push(History::new(by, "answer-discarded"));
    }
    let out = q.clone();
    ws.store.save_qst_revs(slug, &revs)?;
    Ok(out)
}

pub fn withdraw_question(ws: &Workspace, slug: &str, by: &str) -> Result<Question> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.qst_revs(slug)?;
    let q = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no question `{slug}`"))?;
    q.status = QstStatus::Withdrawn;
    q.history.push(History::new(by, "withdraw"));
    let out = q.clone();
    ws.store.save_qst_revs(slug, &revs)?;
    Ok(out)
}

/// Close the open round if residue == 0 (`obj~round-close~1`). Verdict confirmation arrives in UM3.
pub fn close_round(ws: &Workspace, ctx: &Context, doc: &str) -> Result<Round> {
    let _l = ws.store.lock()?;
    let mut r = ws
        .store
        .open_round(ctx, doc)?
        .ok_or_else(|| anyhow!("no open round for {doc}"))?;
    let snap = ws.store.load_snapshot_arc(doc, &r.snapshot)?;
    let cov = doc_coverage(&ws.store, ctx, &snap)?;
    if cov.meter.residue > 0 {
        bail!(
            "cannot close: {} unclassified span(s) remain",
            cov.meter.residue
        );
    }
    let mut summary = RoundSummary::default();
    for req in ws.store.current_reqs()? {
        if req.anchors.iter().any(|a| a.doc == doc) && req.history.iter().any(|h| h.at >= r.opened)
        {
            match req.status {
                Status::Retired => summary.retired.push(req.id.clone()),
                _ if req.id.rev == 1
                    && req
                        .history
                        .first()
                        .map(|h| h.at >= r.opened)
                        .unwrap_or(false) =>
                {
                    summary.created.push(req.id.clone())
                }
                _ => summary.changed.push(req.id.clone()),
            }
        }
    }
    r.closed = Some(now());
    r.summary = Some(summary);
    ws.store.save_round(&r)?;
    Ok(r)
}

// ---------- PR contexts (`ui~pr-view~1`) ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub head: String,
    pub head_ref: String,
    pub base_ref: String,
    pub author: String,
    pub updated_at: String,
    pub draft: bool,
    /// Markdown files this PR changes.
    pub files: Vec<String>,
    /// Which of those are tracked docs.
    pub touches: Vec<String>,
}

pub fn list_prs(ws: &Workspace) -> Result<Vec<PrSummary>> {
    let cfg = ws.store.repo()?;
    if cfg.kind == RepoKind::Local {
        return Ok(vec![]);
    }
    let prs = repo::list_prs(&cfg.github)?;
    Ok(prs
        .into_iter()
        .map(|p| {
            let files: Vec<String> = p
                .files
                .iter()
                .filter_map(|f| f.get("path").and_then(|x| x.as_str()).map(String::from))
                .filter(|f| {
                    let l = f.to_ascii_lowercase();
                    l.ends_with(".md") || l.ends_with(".markdown")
                })
                .collect();
            PrSummary {
                files: files.clone(),
                touches: cfg
                    .tracked
                    .iter()
                    .filter(|t| files.contains(&t.path))
                    .map(|t| t.path.clone())
                    .collect(),
                number: p.number,
                title: p.title,
                head: p.head,
                head_ref: p.head_ref,
                base_ref: p.base_ref,
                author: p
                    .author
                    .get("login")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                updated_at: p.updated_at,
                draft: p.draft,
            }
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrDoc {
    pub path: String,
    /// added | modified | deleted | renamed
    pub status: String,
    pub tracked: bool,
}

/// Markdown files changed by a PR. GitHub is the source of truth (`gh pr view --json files`);
/// the local diff vs merge-base supplies add/modify/delete status. Non-GitHub (`file://`)
/// repos use the local diff alone.
pub fn pr_docs(ws: &Workspace, pr: u64) -> Result<Vec<PrDoc>> {
    let cfg = ws.store.repo()?;
    let _ = repo::fetch(&ws.git);
    let local = repo::changed_markdown(
        &ws.git,
        &repo::branch_ref(&cfg.default_branch),
        &repo::pr_ref(pr),
    )
    .unwrap_or_default();
    let status_of: std::collections::HashMap<&str, &str> = local
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let is_md = |f: &str| {
        let l = f.to_ascii_lowercase();
        l.ends_with(".md") || l.ends_with(".markdown")
    };
    let paths: Vec<String> = if cfg.remote.contains("github.com") {
        repo::pr_changed_files(&cfg.github, pr)?
            .into_iter()
            .filter(|f| is_md(f))
            .collect()
    } else {
        local.iter().map(|(p, _)| p.clone()).collect()
    };
    Ok(paths
        .into_iter()
        .map(|path| PrDoc {
            tracked: cfg.tracked.iter().any(|t| t.path == path),
            status: status_of
                .get(path.as_str())
                .unwrap_or(&"changed")
                .to_string(),
            path,
        })
        .collect())
}

/// Open a PR context for a doc: fetch, snapshot the doc at the PR head, and (if the PR text
/// differs from the base snapshot) compute the verdict list against the base classification.
/// Returns the context to use with `doc_view`.
pub fn open_pr(ws: &Workspace, pr: u64, doc: &str) -> Result<Context> {
    let cfg = ws.store.repo()?;
    let _ = repo::fetch(&ws.git);
    let head = repo::head_sha(&ws.git, &repo::pr_ref(pr))
        .with_context(|| format!("PR #{pr} head not fetched"))?;
    let ctx = Context::Pr {
        pr,
        head: head.clone(),
    };
    let _l = ws.store.lock()?;
    let (content, _) = repo::read_blob(&ws.git, &repo::pr_ref(pr), doc)?
        .ok_or_else(|| anyhow!("{doc} not present on PR #{pr}"))?;
    let snap = Snapshot::build(doc, &content);
    ws.store.save_snapshot(&snap)?;
    ws.store.set_current_sha(&ctx, doc, &snap.sha)?;
    // Verdicts vs the base classification (base = default branch's current snapshot).
    let base_ctx = Context::Branch {
        branch: cfg.default_branch.clone(),
    };
    if let Some(base) = ws.store.current_snapshot(&base_ctx, doc)? {
        if base.sha != snap.sha {
            let cov = doc_coverage(&ws.store, &base_ctx, &base)?;
            let recon = build_reconciliation(ws, &base_ctx, &base, &snap, &cov)?;
            ws.store.save_pending(&ctx, &recon)?;
        } else {
            ws.store.clear_pending(&ctx, doc)?;
        }
    }
    Ok(ctx)
}

// ---------- status ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocStatus {
    pub doc: String,
    pub snapshot: Option<String>,
    pub meter: Option<crate::coverage::Meter>,
    pub open_round: Option<u32>,
    pub rounds_closed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub github: String,
    pub default_branch: String,
    pub docs: Vec<DocStatus>,
    pub rollup: crate::coverage::RepoRollup,
}

pub fn status(ws: &Workspace) -> Result<RepoStatus> {
    let cfg = ws.store.repo()?;
    let ctx = Context::Branch {
        branch: cfg.default_branch.clone(),
    };
    let mut docs = vec![];
    for t in &cfg.tracked {
        let snap = ws.store.current_snapshot_arc(&ctx, &t.path)?;
        let meter = match &snap {
            Some(s) => Some(doc_coverage(&ws.store, &ctx, s)?.meter),
            None => None,
        };
        let rounds = ws.store.rounds(&ctx, &t.path)?;
        docs.push(DocStatus {
            doc: t.path.clone(),
            snapshot: snap.map(|s| s.sha.clone()),
            meter,
            open_round: rounds.iter().find(|r| r.closed.is_none()).map(|r| r.n),
            rounds_closed: rounds.iter().filter(|r| r.closed.is_some()).count(),
        });
    }
    Ok(RepoStatus {
        github: cfg.github,
        default_branch: cfg.default_branch,
        docs,
        rollup: crate::coverage::repo_rollup(&ws.store)?,
    })
}

/// Default export location: `<store>/exports/reqtrace/`, or a caller-provided dir.
pub fn export(ws: &Workspace, out_dir: Option<&Path>) -> Result<crate::export::ExportResult> {
    let _l = ws.store.lock()?;
    let dir: PathBuf = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ws.store.root().join("exports").join("reqtrace"));
    crate::export::export_reqtrace(&ws.store, &dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a local git repo with a doc, register it under a temp home, track the doc.
    pub(crate) fn fixture() -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("docs")).unwrap();
        let r = git2::Repository::init(&src).unwrap();
        std::fs::write(
            src.join("docs/hld.md"),
            include_str!("../tests/fixtures/sample-hld.md"),
        )
        .unwrap();
        std::fs::write(src.join("README.md"), "# readme\n").unwrap();
        let mut idx = r.index().unwrap();
        idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = r.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        r.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let head = r.head().unwrap().peel_to_commit().unwrap();
        let _ = r.branch("main", &head, true);
        let home = tmp.path().join("home");
        let url = format!("file://{}", src.display());
        let ws = register_repo_from_url(&home, "o/n", &url).unwrap();
        track_doc(&ws, "docs/hld.md").unwrap();
        (tmp, ws)
    }

    #[test]
    fn register_track_status_export() {
        let (_tmp, ws) = fixture();
        let docs = list_docs(&ws).unwrap();
        assert_eq!(docs.len(), 2);
        assert!(docs.iter().any(|d| d.path == "docs/hld.md" && d.tracked));
        assert!(docs.iter().any(|d| d.path == "README.md" && !d.tracked));

        let ctx = ws.default_context().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert!(view.snapshot.spans.len() > 20);
        let m = &view.coverage.meter;
        assert_eq!(m.classified, 0);
        assert!(
            m.total < view.snapshot.spans.len(),
            "headings/code are structural"
        );
        assert!(view.round.is_none());

        // classify a couple of spans
        let first_para = view
            .snapshot
            .spans
            .iter()
            .find(|s| s.block == crate::segment::Block::Para)
            .unwrap()
            .id
            .clone();
        let li = view
            .snapshot
            .spans
            .iter()
            .find(|s| s.block == crate::segment::Block::Li)
            .unwrap()
            .id
            .clone();
        mark_non_normative(&ws, &ctx, "docs/hld.md", &[first_para.clone()], "cj").unwrap();
        let req = create_req(
            &ws,
            &ctx,
            "docs/hld.md",
            &[li.clone()],
            NewReq {
                statement: "The email address shall match RFC 5322.",
                slug: Some("email-format"),
                pattern: Some(Pattern::Ubiquitous),
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        assert_eq!(req.id.to_string(), "req~email-format~1");
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert_eq!(view.coverage.meter.classified, 2);
        assert_eq!(view.coverage.meter.mapped, 1);
        assert_eq!(view.round.as_ref().unwrap().n, 1);
        let st = view
            .coverage
            .spans
            .iter()
            .find(|(id, _)| id == &li)
            .unwrap();
        assert_eq!(st.1.reqs, vec!["req~email-format~1"]);

        // bad span rejected
        assert!(mark_non_normative(&ws, &ctx, "docs/hld.md", &["s-nope".into()], "cj").is_err());
        // can't close with residue
        assert!(close_round(&ws, &ctx, "docs/hld.md").is_err());

        // status + export
        let s = status(&ws).unwrap();
        assert_eq!(s.docs[0].open_round, Some(1));
        assert_eq!(s.rollup.reqs_by_status["extracted"], 1);
        assert!(s.rollup.unexported_changes);
        let ex = export(&ws, None).unwrap();
        assert_eq!(ex.items, 1);
        assert!(!status(&ws).unwrap().rollup.unexported_changes);

        // retire requires reason
        assert!(update_req(
            &ws,
            "email-format",
            ReqPatch {
                statement: None,
                pattern: None,
                status: Some(Status::Retired),
                rating: None,
                owner: None,
                reason: None
            },
            "cj"
        )
        .is_err());
        let r = update_req(
            &ws,
            "email-format",
            ReqPatch {
                statement: None,
                pattern: None,
                status: Some(Status::Retired),
                rating: None,
                owner: None,
                reason: Some("dup"),
            },
            "cj",
        )
        .unwrap();
        assert_eq!(r.status, Status::Retired);
        // retired anchors no longer count as mapped
        assert_eq!(
            doc_view(&ws, &ctx, "docs/hld.md")
                .unwrap()
                .coverage
                .meter
                .mapped,
            0
        );

        // bump keeps prior rev
        let b = bump_req(&ws, "email-format", "New meaning.", "cj").unwrap();
        assert_eq!(b.id.rev, 2);
        assert_eq!(ws.store.req_revs("email-format").unwrap().len(), 2);
    }

    #[test]
    fn full_pass_closes_round() {
        let (_tmp, ws) = fixture();
        let ctx = ws.default_context().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        let ids: Vec<String> = view
            .coverage
            .spans
            .iter()
            .filter(|(_, s)| !s.structural)
            .map(|(id, _)| id.clone())
            .collect();
        mark_non_normative(&ws, &ctx, "docs/hld.md", &ids, "cj").unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert_eq!(view.coverage.meter.residue, 0);
        let r = close_round(&ws, &ctx, "docs/hld.md").unwrap();
        assert!(r.closed.is_some());
        assert!(ws.store.open_round(&ctx, "docs/hld.md").unwrap().is_none());
        // next mutation opens round 2
        unmark(&ws, &ctx, "docs/hld.md", &ids[..1]).unwrap();
        mark_non_normative(&ws, &ctx, "docs/hld.md", &ids[..1], "cj").unwrap();
        assert_eq!(
            ws.store.open_round(&ctx, "docs/hld.md").unwrap().unwrap().n,
            2
        );
    }

    #[test]
    fn question_flow() {
        let (_tmp, ws) = fixture();
        let ctx = ws.default_context().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        let span = view
            .snapshot
            .spans
            .iter()
            .find(|s| s.text.starts_with("Is a per-partner"))
            .unwrap()
            .id
            .clone();
        let q = flag_question(
            &ws,
            &ctx,
            "docs/hld.md",
            &[span.clone()],
            NewQuestion {
                quote: "Is a per-partner override needed in v1?",
                materiality: Level::M,
                readings: vec![("a".into(), "yes".into()), ("b".into(), "no".into())],
                default: Some("b".into()),
                affects: vec![],
                slug: None,
            },
            "cj",
        )
        .unwrap();
        assert_eq!(q.id.ty, "qst");
        assert!(q.readings[1].default);
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert_eq!(view.coverage.meter.questioned, 1);
        assert_eq!(view.coverage.meter.open_questions, 1);
    }
}

// ---------- groups (spec §4.3) ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRollup {
    pub group: Group,
    pub members_by_status: std::collections::BTreeMap<String, usize>,
    pub open_questions: usize,
    pub anchors: usize,
    /// Members that no longer resolve or are retired (`obj~grp-integrity~1`).
    pub findings: Vec<GroupFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupFinding {
    pub member: Id,
    pub kind: String, // "missing" | "retired" | "stale-rev"
}

pub fn group_rollups(ws: &Workspace) -> Result<Vec<GroupRollup>> {
    let reqs = ws.store.current_reqs()?;
    let by_slug: std::collections::HashMap<&str, &ReqRev> =
        reqs.iter().map(|r| (r.id.slug.as_str(), r)).collect();
    let qsts = ws.store.current_qsts()?;
    let mut out = vec![];
    for g in ws.store.current_grps()? {
        let mut r = GroupRollup {
            group: g.clone(),
            members_by_status: Default::default(),
            open_questions: 0,
            anchors: 0,
            findings: vec![],
        };
        for m in &g.members {
            match by_slug.get(m.slug.as_str()) {
                None => r.findings.push(GroupFinding {
                    member: m.clone(),
                    kind: "missing".into(),
                }),
                Some(req) => {
                    *r.members_by_status
                        .entry(req.status.as_str().into())
                        .or_default() += 1;
                    r.anchors += req.anchors.len();
                    if req.status == Status::Retired {
                        r.findings.push(GroupFinding {
                            member: m.clone(),
                            kind: "retired".into(),
                        });
                    } else if req.id.rev != m.rev {
                        r.findings.push(GroupFinding {
                            member: m.clone(),
                            kind: "stale-rev".into(),
                        });
                    }
                    r.open_questions += qsts
                        .iter()
                        .filter(|q| {
                            q.status == QstStatus::Open
                                && q.affects.iter().any(|a| a.slug == req.id.slug)
                        })
                        .count();
                }
            }
        }
        out.push(r);
    }
    out.sort_by(|a, b| {
        a.group
            .title
            .to_lowercase()
            .cmp(&b.group.title.to_lowercase())
    });
    Ok(out)
}

pub fn create_group(
    ws: &Workspace,
    title: &str,
    description: Option<&str>,
    by: &str,
) -> Result<Group> {
    let _l = ws.store.lock()?;
    let title = title.trim();
    if title.is_empty() {
        bail!("group title is required");
    }
    let base = slugify(title, 32);
    let mut slug = base.clone();
    let mut i = 1;
    while !ws.store.grp_revs(&slug)?.is_empty() {
        i += 1;
        slug = format!("{base}-{i}");
    }
    let g = Group {
        id: Id::new("grp", &slug, 1)?,
        title: title.into(),
        description: description
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        members: vec![],
        history: vec![History::new(by, "create")],
    };
    ws.store.save_grp_revs(&slug, std::slice::from_ref(&g))?;
    Ok(g)
}

/// Add requirements (by slug, at their current rev) to a group. Idempotent.
pub fn assign_group(
    ws: &Workspace,
    group_slug: &str,
    req_slugs: &[String],
    by: &str,
) -> Result<Group> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.grp_revs(group_slug)?;
    let g = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no group `{group_slug}`"))?;
    let before = g.members.clone();
    for s in req_slugs {
        let cur = ws
            .store
            .current_req(s)?
            .ok_or_else(|| anyhow!("no requirement `{s}`"))?;
        if let Some(existing) = g.members.iter_mut().find(|m| m.slug == cur.id.slug) {
            *existing = cur.id.clone();
        } else {
            g.members.push(cur.id.clone());
        }
    }
    if g.members != before {
        g.history
            .push(History::new(by, "members").change(Some(&before), Some(&g.members)));
    }
    let out = g.clone();
    ws.store.save_grp_revs(group_slug, &revs)?;
    Ok(out)
}

pub fn unassign_group(
    ws: &Workspace,
    group_slug: &str,
    req_slugs: &[String],
    by: &str,
) -> Result<Group> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.grp_revs(group_slug)?;
    let g = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no group `{group_slug}`"))?;
    let before = g.members.clone();
    g.members.retain(|m| !req_slugs.contains(&m.slug));
    if g.members != before {
        g.history
            .push(History::new(by, "members").change(Some(&before), Some(&g.members)));
    }
    let out = g.clone();
    ws.store.save_grp_revs(group_slug, &revs)?;
    Ok(out)
}

pub fn update_group(
    ws: &Workspace,
    group_slug: &str,
    title: Option<&str>,
    description: Option<Option<&str>>,
    by: &str,
) -> Result<Group> {
    let _l = ws.store.lock()?;
    let mut revs = ws.store.grp_revs(group_slug)?;
    let g = revs
        .last_mut()
        .ok_or_else(|| anyhow!("no group `{group_slug}`"))?;
    if let Some(t) = title {
        if !t.trim().is_empty() {
            g.title = t.trim().into();
        }
    }
    if let Some(d) = description {
        g.description = d.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    g.history.push(History::new(by, "update"));
    let out = g.clone();
    ws.store.save_grp_revs(group_slug, &revs)?;
    Ok(out)
}

/// Repo-wide inventory row: current rev + derived group titles + doc list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryRow {
    #[serde(flatten)]
    pub req: ReqRev,
    pub groups: Vec<String>,
    pub docs: Vec<String>,
    pub open_questions: usize,
}

pub fn inventory(ws: &Workspace) -> Result<Vec<InventoryRow>> {
    let groups = ws.store.groups_by_req()?;
    let qsts = ws.store.current_qsts()?;
    let mut rows = vec![];
    for req in ws.store.current_reqs()? {
        let mut docs: Vec<String> = req.anchors.iter().map(|a| a.doc.clone()).collect();
        docs.sort();
        docs.dedup();
        let open_questions = qsts
            .iter()
            .filter(|q| {
                q.status == QstStatus::Open && q.affects.iter().any(|a| a.slug == req.id.slug)
            })
            .count();
        rows.push(InventoryRow {
            groups: groups.get(&req.id.key()).cloned().unwrap_or_default(),
            docs,
            open_questions,
            req,
        });
    }
    rows.sort_by(|a, b| a.req.id.slug.cmp(&b.req.id.slug));
    Ok(rows)
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn groups_roundtrip() {
        let (_tmp, ws) = tests::fixture();
        let ctx = ws.default_context().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        let li = view
            .snapshot
            .spans
            .iter()
            .find(|s| s.block == crate::segment::Block::Li)
            .unwrap()
            .id
            .clone();
        create_req(
            &ws,
            &ctx,
            "docs/hld.md",
            &[li],
            NewReq {
                statement: "The email address shall match RFC 5322.",
                slug: Some("email-format"),
                pattern: None,
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        let g = create_group(&ws, "Validation", Some("input rules"), "cj").unwrap();
        assert_eq!(g.id.to_string(), "grp~validation~1");
        let g2 = create_group(&ws, "Validation", None, "cj").unwrap();
        assert_eq!(g2.id.slug, "validation-2");
        assign_group(&ws, "validation", &["email-format".into()], "cj").unwrap();
        assign_group(&ws, "validation", &["email-format".into()], "cj").unwrap(); // idempotent
        let r = group_rollups(&ws).unwrap();
        let v = r.iter().find(|x| x.group.id.slug == "validation").unwrap();
        assert_eq!(v.members_by_status["extracted"], 1);
        assert!(v.findings.is_empty());
        let inv = inventory(&ws).unwrap();
        assert_eq!(inv[0].groups, vec!["Validation"]);
        assert_eq!(inv[0].docs, vec!["docs/hld.md"]);
        // retire → finding
        update_req(
            &ws,
            "email-format",
            ReqPatch {
                statement: None,
                pattern: None,
                status: Some(Status::Retired),
                rating: None,
                owner: None,
                reason: Some("x"),
            },
            "cj",
        )
        .unwrap();
        let r = group_rollups(&ws).unwrap();
        assert_eq!(
            r.iter()
                .find(|x| x.group.id.slug == "validation")
                .unwrap()
                .findings[0]
                .kind,
            "retired"
        );
        unassign_group(&ws, "validation", &["email-format".into()], "cj").unwrap();
        assert!(ws.store.grp_revs("validation").unwrap()[0]
            .members
            .is_empty());
        assert!(assign_group(&ws, "validation", &["nope".into()], "cj").is_err());
    }
}

#[cfg(test)]
mod reconcile_tests {
    use super::*;
    use crate::reconcile::{Decision, VerdictKind};

    fn commit_file(src: &Path, rel: &str, content: &str, msg: &str) {
        let r = git2::Repository::open(src).unwrap();
        std::fs::write(src.join(rel), content).unwrap();
        let mut idx = r.index().unwrap();
        idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = r.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let parent = r.head().unwrap().peel_to_commit().unwrap();
        r.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
            .unwrap();
    }

    #[test]
    fn edit_upstream_then_reconcile() {
        let (tmp, ws) = tests::fixture();
        let src = tmp.path().join("src");
        let ctx = ws.default_context().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        let find = |t: &str| {
            view.snapshot
                .spans
                .iter()
                .find(|s| s.text.starts_with(t))
                .unwrap()
                .id
                .clone()
        };
        let email = find("The email address shall match");
        let abn = find("The ABN, when supplied");
        let guardian = find("Where the applicant is under 18");
        let intro = find("The intake service accepts");
        create_req(
            &ws,
            &ctx,
            "docs/hld.md",
            &[email.clone()],
            NewReq {
                statement: "Email shall match RFC 5322.",
                slug: Some("email-format"),
                pattern: None,
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        create_req(
            &ws,
            &ctx,
            "docs/hld.md",
            &[abn.clone()],
            NewReq {
                statement: "ABN shall pass checksum.",
                slug: Some("abn-checksum"),
                pattern: None,
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        create_req(
            &ws,
            &ctx,
            "docs/hld.md",
            &[guardian.clone()],
            NewReq {
                statement: "Minors need a guardian.",
                slug: Some("guardian"),
                pattern: None,
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        mark_non_normative(&ws, &ctx, "docs/hld.md", &[intro.clone()], "cj").unwrap();

        // Edit upstream: reword the email rule, delete the guardian rule, keep ABN, add a sentence.
        let original = include_str!("../tests/fixtures/sample-hld.md");
        let edited = original
            .replace(
                "The email address shall match RFC 5322 and shall have a resolvable MX record.",
                "The email address shall match RFC 5322 and must have a resolvable MX record.",
            )
            .replace(
                "Where the applicant is under 18, the system shall require a guardian's details. ",
                "",
            )
            .replace(
                "## 5. Non-goals",
                "A brand new paragraph appears here.\n\n## 5. Non-goals",
            );
        assert_ne!(original, edited);
        commit_file(&src, "docs/hld.md", &edited, "edit hld");

        let changes = refresh(&ws).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].advanced, "classified doc must not auto-advance");
        let old_sha = changes[0].from.clone().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert_eq!(view.snapshot.sha, old_sha, "still rendering old snapshot");
        let pend = view.pending.clone().expect("pending reconciliation");
        let kinds: std::collections::HashMap<&str, VerdictKind> = pend
            .verdicts
            .iter()
            .map(|v| (v.from.as_str(), v.kind))
            .collect();
        assert_eq!(kinds[email.as_str()], VerdictKind::Reworded);
        assert_eq!(kinds[abn.as_str()], VerdictKind::Unchanged);
        assert_eq!(kinds[guardian.as_str()], VerdictKind::Missing);
        assert_eq!(kinds[intro.as_str()], VerdictKind::Unchanged);
        assert_eq!(pend.added.len(), 1);
        assert_eq!(pend.unconfirmed(), 2);

        // Round can't close while pending.
        assert!(close_round(&ws, &ctx, "docs/hld.md").is_err());
        // Confirm fails until decided.
        assert!(confirm_reconciliation(&ws, &ctx, "docs/hld.md", "cj").is_err());
        // Missing can't be accepted.
        assert!(decide_verdict(&ws, &ctx, "docs/hld.md", &guardian, Decision::Accept).is_err());

        decide_verdict(&ws, &ctx, "docs/hld.md", &email, Decision::MeaningChanged).unwrap();
        decide_verdict(
            &ws,
            &ctx,
            "docs/hld.md",
            &guardian,
            Decision::Retire {
                reason: "removed from HLD".into(),
            },
        )
        .unwrap();
        let r = confirm_reconciliation(&ws, &ctx, "docs/hld.md", "cj").unwrap();
        assert_eq!(r.unconfirmed(), 0);

        // After: pointer advanced, round closed, anchors remapped, mark migrated, guardian retired.
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert_ne!(view.snapshot.sha, old_sha);
        assert!(view.pending.is_none());
        assert!(view.round.is_none(), "old round closed by supersede");
        let rounds = ws.store.rounds(&ctx, "docs/hld.md").unwrap();
        assert!(rounds[0].closed.is_some());
        assert!(rounds[0].summary.as_ref().unwrap().verdicts.is_some());
        let email_req = ws.store.current_req("email-format").unwrap().unwrap();
        assert!(email_req.suspect.is_some());
        let new_email_span = view
            .snapshot
            .spans
            .iter()
            .find(|s| s.text.contains("must have a resolvable"))
            .unwrap();
        assert_eq!(email_req.anchors[0].span, new_email_span.id);
        assert_eq!(
            ws.store.current_req("guardian").unwrap().unwrap().status,
            Status::Retired
        );
        let cov = &view.coverage;
        let st = |t: &str| {
            cov.spans
                .iter()
                .find(|(id, _)| view.snapshot.span(id).unwrap().text.starts_with(t))
                .unwrap()
                .1
                .state
        };
        assert_eq!(
            st("The intake service accepts"),
            crate::coverage::SpanState::NonNormative
        );
        assert_eq!(
            st("The email address shall match"),
            crate::coverage::SpanState::Mapped
        );
        assert_eq!(
            st("A brand new paragraph"),
            crate::coverage::SpanState::Unclassified
        );
        // suspect clears on edit
        let r = update_req(
            &ws,
            "email-format",
            ReqPatch {
                statement: Some("Email must match RFC 5322."),
                pattern: None,
                status: None,
                rating: None,
                owner: None,
                reason: None,
            },
            "cj",
        )
        .unwrap();
        assert!(r.suspect.is_none());
    }

    #[test]
    fn question_answer_and_conflict() {
        let (_tmp, ws) = tests::fixture();
        let ctx = ws.default_context().unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        let span = view
            .snapshot
            .spans
            .iter()
            .find(|s| s.text.starts_with("Rate limits are per partner"))
            .unwrap()
            .id
            .clone();
        create_req(
            &ws,
            &ctx,
            "docs/hld.md",
            &[span.clone()],
            NewReq {
                statement: "Rate limits shall be per partner key.",
                slug: Some("rate-limit"),
                pattern: None,
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        let q = flag_question(
            &ws,
            &ctx,
            "docs/hld.md",
            &[span.clone()],
            NewQuestion {
                quote: "burst 200",
                materiality: Level::H,
                readings: vec![
                    ("a".into(), "per minute".into()),
                    ("b".into(), "per second".into()),
                ],
                default: Some("a".into()),
                affects: vec![Id::new("req", "rate-limit", 1).unwrap()],
                slug: Some("burst-unit"),
            },
            "cj",
        )
        .unwrap();
        assert_eq!(q.affects_revs[0].rev, 1);
        // bump the req → conflict on answer
        bump_req(
            &ws,
            "rate-limit",
            "Rate limits shall be per partner key, burst 200/min.",
            "cj",
        )
        .unwrap();
        let held = answer_question(&ws, "burst-unit", "a", None, "cj").unwrap();
        assert_eq!(held.status, QstStatus::Open);
        assert!(held.pending.is_some());
        let applied = resolve_held_answer(&ws, "burst-unit", true, "cj").unwrap();
        assert_eq!(applied.status, QstStatus::Answered);
        assert_eq!(applied.answer.unwrap().reading, "a");
        // no-conflict path
        let q2 = flag_question(
            &ws,
            &ctx,
            "docs/hld.md",
            &[span],
            NewQuestion {
                quote: "x",
                materiality: Level::L,
                readings: vec![],
                default: None,
                affects: vec![],
                slug: Some("plain"),
            },
            "cj",
        )
        .unwrap();
        let a = answer_question(&ws, "plain", "free-text", Some("whatever"), "cj").unwrap();
        assert_eq!(a.status, QstStatus::Answered);
        assert!(answer_question(&ws, "plain", "a", None, "cj").is_err());
        let w = withdraw_question(&ws, &q2.id.slug, "cj");
        assert!(w.is_ok());
    }
}

// ---------- agent pre-fill (`ui~agent-prefill~1`) ----------

use crate::agent::{self, Job, Proposal, ProposalStatus, Proposals, Proposed};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefillStatus {
    pub available: bool,
    pub job: Option<Job>,
    pub proposals: Vec<Proposal>,
}

/// Start a background pre-fill over the doc's unclassified prose spans. Returns immediately.
pub fn start_prefill(ws: &Workspace, ctx: &Context, doc: &str) -> Result<Job> {
    if !agent::agent_available() {
        bail!(
            "no agent backend: install Claude Code (`claude`) and log in, or set KANSA_AGENT_CMD"
        );
    }
    if let Some(j) = agent::load_job(&ws.store, ctx, doc)? {
        if j.state == agent::JobState::Running {
            bail!("a pre-fill is already running for {doc}");
        }
    }
    let snap = ws
        .store
        .current_snapshot(ctx, doc)?
        .ok_or_else(|| anyhow!("{doc} has no snapshot"))?;
    let cov = doc_coverage(&ws.store, ctx, &snap)?;
    let unclassified: Vec<String> = cov
        .spans
        .iter()
        .filter(|(_, st)| st.state == crate::coverage::SpanState::Unclassified && !st.structural)
        .map(|(id, _)| id.clone())
        .collect();
    let existing_reqs: Vec<(String, String)> = ws
        .store
        .current_reqs()?
        .into_iter()
        .filter(|r| r.status != Status::Retired)
        .map(|r| (r.id.slug.clone(), r.statement.clone()))
        .collect();
    let existing_groups: Vec<String> = ws
        .store
        .current_grps()?
        .into_iter()
        .map(|g| g.title)
        .collect();
    let job = Job {
        state: agent::JobState::Running,
        doc: doc.into(),
        snapshot: snap.sha.clone(),
        done: 0,
        total: unclassified.len().div_ceil(agent::BATCH).max(1),
        error: None,
        model: agent::agent_model(),
        started: now(),
        finished: None,
    };
    let store = ws.store.clone();
    let ctx2 = ctx.clone();
    std::thread::Builder::new()
        .name(format!("kansa-prefill-{}", doc_key(doc)))
        .spawn(move || {
            agent::run_prefill(
                store,
                ctx2,
                snap,
                unclassified,
                existing_reqs,
                existing_groups,
            )
        })
        .context("spawning pre-fill thread")?;
    Ok(job)
}

pub fn prefill_status(ws: &Workspace, ctx: &Context, doc: &str) -> Result<PrefillStatus> {
    Ok(PrefillStatus {
        available: agent::agent_available(),
        job: agent::load_job(&ws.store, ctx, doc)?,
        proposals: agent::load_proposals(&ws.store, ctx, doc)?.items,
    })
}

/// Stamp `accepted_by` on the newest history entry of an object the agent proposed.
fn attribute(h: &mut [History], user: &str) {
    if let Some(last) = h.last_mut() {
        last.by = "agent".into();
        last.accepted_by = Some(user.into());
    }
}

/// Accept one proposal: perform the corresponding core op with `by: agent, accepted-by: user`.
pub fn accept_proposal(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    proposal_id: &str,
    user: &str,
) -> Result<Proposal> {
    let mut all = agent::load_proposals(&ws.store, ctx, doc)?;
    let idx = all
        .items
        .iter()
        .position(|p| p.id == proposal_id)
        .ok_or_else(|| anyhow!("no proposal `{proposal_id}`"))?;
    let p = all.items[idx].clone();
    if p.status != ProposalStatus::Proposed {
        bail!("proposal already {:?}", p.status);
    }
    let result = match &p.proposed {
        Proposed::Context => {
            mark_non_normative(ws, ctx, doc, &p.spans, "agent")?;
            {
                let _l = ws.store.lock()?;
                let mut m = ws.store.marks(ctx, doc)?;
                for s in &p.spans {
                    if let Some(mk) = m.spans.get_mut(s) {
                        mk.by = format!("agent (accepted by {user})");
                    }
                }
                ws.store.save_marks(ctx, doc, &m)?;
            }
            "context".to_string()
        }
        Proposed::Req {
            statement,
            pattern,
            slug,
            groups,
            attach,
        } => {
            let r = if let Some(a) = attach {
                let r = attach_req(ws, ctx, doc, &p.spans, a, "agent")?;
                let _l = ws.store.lock()?;
                let mut revs = ws.store.req_revs(a)?;
                if let Some(cur) = revs.last_mut() {
                    attribute(&mut cur.history, user);
                }
                ws.store.save_req_revs(a, &revs)?;
                r
            } else {
                let r = create_req(
                    ws,
                    ctx,
                    doc,
                    &p.spans,
                    NewReq {
                        statement,
                        slug: slug.as_deref(),
                        pattern: *pattern,
                        rating: None,
                        owner: None,
                    },
                    "agent",
                )?;
                let _l = ws.store.lock()?;
                let mut revs = ws.store.req_revs(&r.id.slug)?;
                if let Some(cur) = revs.last_mut() {
                    attribute(&mut cur.history, user);
                }
                ws.store.save_req_revs(&r.id.slug, &revs)?;
                r
            };
            // groups: create on acceptance only (`ui~grp-agent~1`)
            for title in groups {
                let existing = ws
                    .store
                    .current_grps()?
                    .into_iter()
                    .find(|g| g.title.eq_ignore_ascii_case(title.trim()));
                let g = match existing {
                    Some(g) => g,
                    None => create_group(ws, title, None, "agent")?,
                };
                assign_group(ws, &g.id.slug, std::slice::from_ref(&r.id.slug), "agent")?;
                let _l = ws.store.lock()?;
                let mut revs = ws.store.grp_revs(&g.id.slug)?;
                if let Some(cur) = revs.last_mut() {
                    attribute(&mut cur.history, user);
                }
                ws.store.save_grp_revs(&g.id.slug, &revs)?;
            }
            r.id.to_string()
        }
        Proposed::Question {
            quote,
            materiality,
            readings,
        } => {
            let q = flag_question(
                ws,
                ctx,
                doc,
                &p.spans,
                NewQuestion {
                    quote: if quote.trim().is_empty() { "?" } else { quote },
                    materiality: materiality.unwrap_or(Level::M),
                    readings: readings
                        .iter()
                        .enumerate()
                        .map(|(i, t)| (((b'a' + i as u8) as char).to_string(), t.clone()))
                        .collect(),
                    default: None,
                    affects: vec![],
                    slug: None,
                },
                "agent",
            )?;
            let _l = ws.store.lock()?;
            let mut revs = ws.store.qst_revs(&q.id.slug)?;
            if let Some(cur) = revs.last_mut() {
                attribute(&mut cur.history, user);
            }
            ws.store.save_qst_revs(&q.id.slug, &revs)?;
            q.id.to_string()
        }
    };
    let _l = ws.store.lock()?;
    let mut all2: Proposals = agent::load_proposals(&ws.store, ctx, doc)?;
    if let Some(p) = all2.items.iter_mut().find(|p| p.id == proposal_id) {
        p.status = ProposalStatus::Accepted;
        p.result = Some(result);
    }
    all = all2;
    agent::save_proposals(&ws.store, ctx, doc, &all)?;
    Ok(all.items.into_iter().find(|p| p.id == proposal_id).unwrap())
}

pub fn reject_proposal(
    ws: &Workspace,
    ctx: &Context,
    doc: &str,
    proposal_id: &str,
) -> Result<Proposal> {
    let _l = ws.store.lock()?;
    let mut all = agent::load_proposals(&ws.store, ctx, doc)?;
    let p = all
        .items
        .iter_mut()
        .find(|p| p.id == proposal_id)
        .ok_or_else(|| anyhow!("no proposal `{proposal_id}`"))?;
    p.status = ProposalStatus::Rejected;
    let out = p.clone();
    agent::save_proposals(&ws.store, ctx, doc, &all)?;
    Ok(out)
}

/// Accept every still-proposed item (per-group accept, `ui~agent-prefill~1`).
pub fn accept_all_proposals(ws: &Workspace, ctx: &Context, doc: &str, user: &str) -> Result<usize> {
    let ids: Vec<String> = agent::load_proposals(&ws.store, ctx, doc)?
        .items
        .into_iter()
        .filter(|p| p.status == ProposalStatus::Proposed)
        .map(|p| p.id)
        .collect();
    let mut n = 0;
    for id in ids {
        if accept_proposal(ws, ctx, doc, &id, user).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

pub fn clear_proposals(ws: &Workspace, ctx: &Context, doc: &str) -> Result<()> {
    let _l = ws.store.lock()?;
    let mut all = agent::load_proposals(&ws.store, ctx, doc)?;
    all.items.retain(|p| p.status != ProposalStatus::Proposed);
    agent::save_proposals(&ws.store, ctx, doc, &all)
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    /// A fake agent: reads the prompt, answers "context" for every id it sees except ones
    /// containing "shall", which become requirements.
    fn fake_agent_cmd() -> String {
        // Runs with sh -c; python reads stdin.
        r#"python3 -c '
import sys,re,json
p=sys.stdin.read()
out=[]
for m in re.finditer(r"^\[(s-[0-9a-f-]+)\] (.*)$", p, re.M):
    sid,text=m.group(1),m.group(2)
    if "shall" in text: out.append({"spans":[sid],"kind":"req","statement":"The system "+text[text.index("shall"):],"pattern":"ubiquitous","groups":["Rules"]})
    elif "?" in text: out.append({"spans":[sid],"kind":"question","quote":text,"materiality":"M","readings":["yes","no"]})
    else: out.append({"spans":[sid],"kind":"context"})
print(json.dumps(out))
'"#.to_string()
    }

    #[test]
    #[cfg(unix)] // the fake agent is a python3/sh one-liner
    fn prefill_roundtrip_with_fake_agent() {
        std::env::set_var("KANSA_AGENT_CMD", fake_agent_cmd());
        let (_tmp, ws) = tests::fixture();
        let ctx = ws.default_context().unwrap();
        let job = start_prefill(&ws, &ctx, "docs/hld.md").unwrap();
        assert_eq!(job.state, agent::JobState::Running);
        // wait for the thread
        let mut st = prefill_status(&ws, &ctx, "docs/hld.md").unwrap();
        for _ in 0..200 {
            if matches!(
                st.job.as_ref().map(|j| j.state),
                Some(agent::JobState::Done | agent::JobState::Error)
            ) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            st = prefill_status(&ws, &ctx, "docs/hld.md").unwrap();
        }
        let j = st.job.unwrap();
        assert_eq!(j.state, agent::JobState::Done, "{:?}", j.error);
        assert!(st.proposals.len() > 10, "{}", st.proposals.len());
        assert!(st
            .proposals
            .iter()
            .any(|p| matches!(p.proposed, Proposed::Req { .. })));
        assert!(st
            .proposals
            .iter()
            .any(|p| matches!(p.proposed, Proposed::Question { .. })));
        // accept one req and one context, reject one
        let req = st
            .proposals
            .iter()
            .find(|p| matches!(p.proposed, Proposed::Req { .. }))
            .unwrap()
            .clone();
        let acc = accept_proposal(&ws, &ctx, "docs/hld.md", &req.id, "cj").unwrap();
        assert_eq!(acc.status, ProposalStatus::Accepted);
        let rid: Id = acc.result.clone().unwrap().parse().unwrap();
        let r = ws.store.current_req(&rid.slug).unwrap().unwrap();
        assert_eq!(r.history[0].by, "agent");
        assert_eq!(r.history[0].accepted_by.as_deref(), Some("cj"));
        assert_eq!(ws.store.groups_by_req().unwrap()[&rid.key()], vec!["Rules"]);
        let ctxp = st
            .proposals
            .iter()
            .find(|p| matches!(p.proposed, Proposed::Context))
            .unwrap()
            .clone();
        accept_proposal(&ws, &ctx, "docs/hld.md", &ctxp.id, "cj").unwrap();
        assert!(accept_proposal(&ws, &ctx, "docs/hld.md", &ctxp.id, "cj").is_err());
        let q = st
            .proposals
            .iter()
            .find(|p| matches!(p.proposed, Proposed::Question { .. }))
            .unwrap()
            .clone();
        reject_proposal(&ws, &ctx, "docs/hld.md", &q.id).unwrap();
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert!(view.coverage.meter.mapped >= 1 && view.coverage.meter.non_normative >= 1);
        let n = accept_all_proposals(&ws, &ctx, "docs/hld.md", "cj").unwrap();
        assert!(n > 5);
        let view = doc_view(&ws, &ctx, "docs/hld.md").unwrap();
        assert_eq!(
            view.coverage.meter.residue, 1,
            "only the rejected proposal remains"
        );
        std::env::remove_var("KANSA_AGENT_CMD");
    }
}

// ---------- cached GitHub data (stale-while-revalidate) ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cached<T> {
    pub data: T,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: time::OffsetDateTime,
    /// True when a background refresh was started by this call; poll again shortly.
    #[serde(default)]
    pub refreshing: bool,
}

fn cache_path(store: &Store, key: &str) -> PathBuf {
    store.root().join("cache").join(format!("{key}.yaml"))
}

fn cache_read<T: serde::de::DeserializeOwned>(store: &Store, key: &str) -> Option<Cached<T>> {
    let p = cache_path(store, key);
    if !p.exists() {
        return None;
    }
    store.read_yaml(&p).ok()
}

fn cache_write<T: Serialize>(store: &Store, key: &str, data: &T) -> Result<()> {
    store.write_yaml(
        &cache_path(store, key),
        &Cached {
            data,
            fetched_at: now(),
            refreshing: false,
        },
    )
}

/// Serve `key` from the disk cache immediately (if present) and refresh in the background;
/// otherwise fetch synchronously. `max_age` controls whether a background refresh is kicked.
fn swr<T>(
    ws: &Workspace,
    key: &str,
    max_age: time::Duration,
    force: bool,
    fetch: impl Fn(&Workspace) -> Result<T> + Send + 'static,
) -> Result<Cached<T>>
where
    T: Serialize + serde::de::DeserializeOwned + Clone + Send + 'static,
{
    if !force {
        if let Some(mut c) = cache_read::<T>(&ws.store, key) {
            let stale = now() - c.fetched_at > max_age;
            if stale && refresh_guard(key) {
                let store = ws.store.clone();
                let key = key.to_string();
                std::thread::spawn(move || {
                    if let Ok(w) = Workspace::open(store.root()) {
                        if let Ok(fresh) = fetch(&w) {
                            let _ = cache_write(&store, &key, &fresh);
                        }
                    }
                    refresh_done(&key);
                });
                c.refreshing = true;
            } else if stale {
                c.refreshing = true; // someone else is already refreshing
            }
            return Ok(c);
        }
    }
    let data = fetch(ws)?;
    cache_write(&ws.store, key, &data)?;
    Ok(Cached {
        data,
        fetched_at: now(),
        refreshing: false,
    })
}

/// One in-flight refresh per cache key per process.
fn refresh_guard(key: &str) -> bool {
    let set = REFRESHING.get_or_init(Default::default);
    let mut g = set.lock().unwrap();
    if g.contains(key) {
        false
    } else {
        g.insert(key.to_string());
        true
    }
}
fn refresh_done(key: &str) {
    if let Some(set) = REFRESHING.get() {
        set.lock().unwrap().remove(key);
    }
}
static REFRESHING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

/// Open PRs, cached; background refresh when older than 60 s (`force` = fetch now).
pub fn list_prs_cached(ws: &Workspace, force: bool) -> Result<Cached<Vec<PrSummary>>> {
    swr(ws, "prs", time::Duration::seconds(60), force, list_prs)
}

/// Changed markdown files of a PR, cached per PR; background refresh when older than 60 s.
pub fn pr_docs_cached(ws: &Workspace, pr: u64, force: bool) -> Result<Cached<Vec<PrDoc>>> {
    swr(
        ws,
        &format!("pr-{pr}-docs"),
        time::Duration::seconds(60),
        force,
        move |w| pr_docs(w, pr),
    )
}

#[cfg(test)]
mod local_tests {
    use super::*;

    #[test]
    fn local_folder_register_track_edit_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("my docs");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(
            folder.join("hld.md"),
            "# H\n\nThe system shall do A. Context here.\n",
        )
        .unwrap();
        std::fs::write(folder.join("sub/notes.md"), "Some notes.\n").unwrap();
        std::fs::write(folder.join("ignore.txt"), "not markdown").unwrap();
        let home = tmp.path().join("home");
        let ws = register_local_in(&home, &folder).unwrap();
        let cfg = ws.store.repo().unwrap();
        assert_eq!(cfg.kind, RepoKind::Local);
        assert!(cfg.github.starts_with("local/my-docs-"));
        let docs = list_docs(&ws).unwrap();
        assert_eq!(
            docs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            vec!["hld.md", "sub/notes.md"]
        );
        track_doc(&ws, "hld.md").unwrap();
        let ctx = ws.default_context().unwrap();
        let v = doc_view(&ws, &ctx, "hld.md").unwrap();
        let a = v
            .snapshot
            .spans
            .iter()
            .find(|s| s.text.starts_with("The system shall do A"))
            .unwrap()
            .id
            .clone();
        create_req(
            &ws,
            &ctx,
            "hld.md",
            &[a],
            NewReq {
                statement: "The system shall do A.",
                slug: Some("do-a"),
                pattern: None,
                rating: None,
                owner: None,
            },
            "cj",
        )
        .unwrap();
        // no change → no import commit, refresh reports nothing
        assert!(refresh(&ws).unwrap().is_empty());
        // edit the file → refresh detects change and produces a pending reconciliation
        std::fs::write(
            folder.join("hld.md"),
            "# H\n\nThe system shall do A and B. Context here.\n",
        )
        .unwrap();
        let ch = refresh(&ws).unwrap();
        assert_eq!(ch.len(), 1);
        assert!(!ch[0].advanced);
        assert!(doc_view(&ws, &ctx, "hld.md").unwrap().pending.is_some());
        // re-registering the same folder reuses the store
        let ws2 = register_local_in(&home, &folder).unwrap();
        assert_eq!(ws2.store.repo().unwrap().github, cfg.github);
        assert!(list_prs(&ws2).unwrap().is_empty());
        assert_eq!(list_repos(&home).unwrap().len(), 1);
    }
}
