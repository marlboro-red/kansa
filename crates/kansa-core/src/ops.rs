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

    fn refname(&self, ctx: &Context) -> String {
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
}

pub fn list_registered() -> Result<Vec<RepoSummary>> {
    let home = kansa_home()?;
    Ok(list_repos(&home)?
        .into_iter()
        .map(|c| RepoSummary {
            store_dir: store_dir_for(&home, &c.github)
                .to_string_lossy()
                .into_owned(),
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
    let (content, _sha) = repo::read_blob(&ws.git, &repo::branch_ref(&cfg.default_branch), &path)?
        .ok_or_else(|| anyhow!("{path} not found on {}", cfg.default_branch))?;
    let snap = Snapshot::build(&path, &content);
    ws.store.save_snapshot(&snap)?;
    ws.store.set_current_sha(&ctx, &path, &snap.sha)?;
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
/// (`ui~repo-refresh~1`). Pointers advance only when nothing is classified yet; otherwise the
/// change is reported for reconciliation (UM3).
pub fn refresh(ws: &Workspace) -> Result<Vec<DocChange>> {
    repo::fetch(&ws.git)?;
    let _l = ws.store.lock()?;
    let mut cfg = ws.store.repo()?;
    cfg.last_fetch = Some(now());
    ws.store.save_repo(&cfg)?;
    let ctx = Context::Branch {
        branch: cfg.default_branch.clone(),
    };
    let refname = repo::branch_ref(&cfg.default_branch);
    let mut changes = vec![];
    for t in &cfg.tracked {
        let cur = ws.store.current_sha(&ctx, &t.path)?;
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
                let snap = Snapshot::build(&t.path, &content);
                ws.store.save_snapshot(&snap)?;
                let untouched = ws.store.open_round(&ctx, &t.path)?.is_none()
                    && ws.store.rounds(&ctx, &t.path)?.is_empty();
                if untouched || cur.is_none() {
                    ws.store.set_current_sha(&ctx, &t.path, &snap.sha)?;
                }
                changes.push(DocChange {
                    doc: t.path.clone(),
                    from: cur,
                    to: Some(snap.sha),
                    advanced: untouched,
                });
            }
        }
    }
    Ok(changes)
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
}

pub fn doc_view(ws: &Workspace, ctx: &Context, doc: &str) -> Result<DocView> {
    let snap = ws
        .store
        .current_snapshot(ctx, doc)?
        .ok_or_else(|| anyhow!("{doc} has no snapshot in this context — track it first"))?;
    // Source: prefer the exact blob by sha; fall back to the ref.
    let source = match ws.git.find_blob(git2::Oid::from_str(&snap.sha)?) {
        Ok(b) => String::from_utf8_lossy(b.content()).into_owned(),
        Err(_) => repo::read_blob(&ws.git, &ws.refname(ctx), doc)?
            .map(|(c, _)| c)
            .unwrap_or_default(),
    };
    let coverage = doc_coverage(&ws.store, ctx, &snap)?;
    let round = ws.store.open_round(ctx, doc)?;
    Ok(DocView {
        doc: doc.into(),
        context: ctx.clone(),
        source,
        snapshot: snap,
        coverage,
        round,
    })
}

// ---------- classification (core loop) ----------

/// Ensure an open round exists for (ctx, doc) — called on first mutation (`obj~round-open~1`).
fn ensure_round(store: &Store, ctx: &Context, doc: &str) -> Result<Round> {
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

fn check_spans(store: &Store, ctx: &Context, doc: &str, spans: &[String]) -> Result<Snapshot> {
    let snap = store
        .current_snapshot(ctx, doc)?
        .ok_or_else(|| anyhow!("{doc} has no current snapshot"))?;
    for s in spans {
        if snap.span(s).is_none() {
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

/// Close the open round if residue == 0 (`obj~round-close~1`). Verdict confirmation arrives in UM3.
pub fn close_round(ws: &Workspace, ctx: &Context, doc: &str) -> Result<Round> {
    let _l = ws.store.lock()?;
    let mut r = ws
        .store
        .open_round(ctx, doc)?
        .ok_or_else(|| anyhow!("no open round for {doc}"))?;
    let snap = ws.store.load_snapshot(doc, &r.snapshot)?;
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
        let snap = ws.store.current_snapshot(&ctx, &t.path)?;
        let meter = match &snap {
            Some(s) => Some(doc_coverage(&ws.store, &ctx, s)?.meter),
            None => None,
        };
        let rounds = ws.store.rounds(&ctx, &t.path)?;
        docs.push(DocStatus {
            doc: t.path.clone(),
            snapshot: snap.map(|s| s.sha),
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
