//! The state store: one directory per registered repo, outside the repo (spec §0).
//!
//! ```text
//! <store>/
//!   repo.yaml
//!   snapshots/<doc-key>/<sha>.yaml
//!   current/<ctx-key>/<doc-key>      # contains the sha of the current snapshot for (context, doc)
//!   reqs/<slug>.yaml
//!   questions/<slug>.yaml
//!   groups/<slug>.yaml
//!   rounds/<ctx-key>/<doc-key>/<n>.yaml
//!   marks/<ctx-key>/<doc-key>.yaml
//!   pending/<ctx-key>/<doc-key>.yaml   # reconciliation awaiting confirmation
//!   exports/last.yaml
//!   .lock
//! ```

use crate::id::{valid_slug, Id, Key};
use crate::model::*;
use anyhow::{anyhow, bail, Context as _, Result};
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// v2: image/link spans carry the full `![alt](url)` / `[text](url)` source range (text and
// therefore span ids are unchanged from v1 — old snapshots refresh in place, see doc_view_at).
pub const SEGMENTER_VERSION: u32 = 2;

// ---------- process-wide read caches ----------
//
// Objects are re-read on every operation (each API call opens a fresh Workspace). Files are
// small but numerous; parsing YAML for hundreds of requirements per keypress adds up. Cache
// parsed files keyed by path + mtime + len — a metadata syscall replaces a parse. Snapshots
// are immutable, so they are cached by path alone.

use std::sync::{Arc, Mutex, OnceLock};

type Stamp = (std::time::SystemTime, u64);
struct FileCache<T> {
    map: Mutex<std::collections::HashMap<PathBuf, (Stamp, Arc<T>)>>,
}
impl<T> FileCache<T> {
    fn new() -> Self {
        FileCache {
            map: Mutex::new(Default::default()),
        }
    }
    fn get_or_load(&self, path: &Path, load: impl FnOnce() -> Result<T>) -> Result<Arc<T>> {
        let meta = fs::metadata(path)?;
        let stamp: Stamp = (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len());
        if let Some((st, v)) = self.map.lock().unwrap().get(path) {
            if *st == stamp {
                return Ok(v.clone());
            }
        }
        let v = Arc::new(load()?);
        self.map
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), (stamp, v.clone()));
        Ok(v)
    }
    fn invalidate(&self, path: &Path) {
        self.map.lock().unwrap().remove(path);
    }
}
static REQ_CACHE: OnceLock<FileCache<Vec<ReqRev>>> = OnceLock::new();
static QST_CACHE: OnceLock<FileCache<Vec<Question>>> = OnceLock::new();
static GRP_CACHE: OnceLock<FileCache<Vec<Group>>> = OnceLock::new();
static SNAP_CACHE: OnceLock<
    Mutex<std::collections::HashMap<PathBuf, Arc<crate::snapshot::Snapshot>>>,
> = OnceLock::new();
fn req_cache() -> &'static FileCache<Vec<ReqRev>> {
    REQ_CACHE.get_or_init(FileCache::new)
}
fn qst_cache() -> &'static FileCache<Vec<Question>> {
    QST_CACHE.get_or_init(FileCache::new)
}
fn grp_cache() -> &'static FileCache<Vec<Group>> {
    GRP_CACHE.get_or_init(FileCache::new)
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Held for the duration of one operation (`ui~lock-scope~1`).
pub struct Lock {
    _file: fs::File,
}

/// Root of all kansa state on this machine: `<config_dir>/kansa/`.
pub fn kansa_home() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("KANSA_HOME") {
        return Ok(PathBuf::from(p));
    }
    let base = dirs::config_dir().ok_or_else(|| anyhow!("no config dir on this platform"))?;
    Ok(base.join("kansa"))
}

/// `<home>/repos/<owner>__<name>/`
pub fn store_dir_for(home: &Path, github: &str) -> PathBuf {
    home.join("repos").join(github.replace('/', "__"))
}

/// `<home>/clones/<owner>__<name>/`
pub fn clone_dir_for(home: &Path, github: &str) -> PathBuf {
    home.join("clones").join(github.replace('/', "__"))
}

/// List all registered repos under a home dir (by store dir presence).
pub fn list_repos(home: &Path) -> Result<Vec<RepoConfig>> {
    let repos = home.join("repos");
    let mut out = vec![];
    if !repos.exists() {
        return Ok(out);
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&repos)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    dirs.sort();
    for d in dirs {
        if d.join("repo.yaml").exists() {
            out.push(Store::open(&d)?.repo()?);
        }
    }
    Ok(out)
}

impl Store {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Open an existing store (must contain `repo.yaml`).
    pub fn open(root: &Path) -> Result<Store> {
        if !root.join("repo.yaml").exists() {
            bail!("no kansa store at {}", root.display());
        }
        Ok(Store {
            root: root.to_path_buf(),
        })
    }

    /// Create a store directory and write `repo.yaml`.
    pub fn init(root: &Path, repo: &RepoConfig) -> Result<Store> {
        for sub in [
            "snapshots",
            "current",
            "reqs",
            "questions",
            "groups",
            "rounds",
            "marks",
            "exports",
        ] {
            fs::create_dir_all(root.join(sub))?;
        }
        let s = Store {
            root: root.to_path_buf(),
        };
        s.write_yaml(&s.root.join("repo.yaml"), repo)?;
        Ok(s)
    }

    // ----- lock + atomic io -----

    pub fn lock(&self) -> Result<Lock> {
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.root.join(".lock"))?;
        f.lock_exclusive()?;
        Ok(Lock { _file: f })
    }

    /// write-temp + rename (`obj~store-atomic~1`), Windows-safe retry (`ui~windows-paths~1`).
    pub fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, bytes)?;
        let mut attempt = 0;
        loop {
            match fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < 10 => {
                    attempt += 1;
                    let _ = e;
                    std::thread::sleep(std::time::Duration::from_millis(20 * attempt));
                }
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(e).with_context(|| {
                        format!("renaming {} -> {}", tmp.display(), path.display())
                    });
                }
            }
        }
    }

    pub fn write_yaml<T: Serialize>(&self, path: &Path, v: &T) -> Result<()> {
        let s = serde_yaml::to_string(v)?;
        self.write_atomic(path, s.as_bytes())
    }

    pub fn read_yaml<T: DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", path.display()))
    }

    fn read_yaml_opt<T: DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
        if path.exists() {
            Ok(Some(self.read_yaml(path)?))
        } else {
            Ok(None)
        }
    }

    fn list_yaml(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut v = vec![];
        if !dir.exists() {
            return Ok(v);
        }
        for e in fs::read_dir(dir)? {
            let p = e?.path();
            if p.extension().and_then(|s| s.to_str()) == Some("yaml") {
                v.push(p);
            }
        }
        v.sort();
        Ok(v)
    }

    // ----- repo -----

    pub fn repo(&self) -> Result<RepoConfig> {
        self.read_yaml(&self.root.join("repo.yaml"))
    }

    pub fn save_repo(&self, r: &RepoConfig) -> Result<()> {
        self.write_yaml(&self.root.join("repo.yaml"), r)
    }

    // ----- generic slug-keyed object files (reqs / questions / groups) -----

    fn slug_path(&self, dir: &str, slug: &str) -> Result<PathBuf> {
        if !valid_slug(slug) {
            bail!("invalid slug `{slug}`");
        }
        Ok(self.root.join(dir).join(format!("{slug}.yaml")))
    }

    fn save_revs<T: Serialize>(&self, dir: &str, slug: &str, revs: &[T]) -> Result<()> {
        self.write_yaml(&self.slug_path(dir, slug)?, &revs)
    }

    // reqs
    pub fn req_revs(&self, slug: &str) -> Result<Vec<ReqRev>> {
        let p = self.slug_path("reqs", slug)?;
        if !p.exists() {
            return Ok(vec![]);
        }
        Ok((*req_cache().get_or_load(&p, || self.read_yaml(&p))?).clone())
    }
    pub fn save_req_revs(&self, slug: &str, revs: &[ReqRev]) -> Result<()> {
        let p = self.slug_path("reqs", slug)?;
        req_cache().invalidate(&p);
        self.save_revs("reqs", slug, revs)
    }
    /// Remove a requirement file outright — only `ops::delete_req` (gated) calls this.
    pub fn delete_req_file(&self, slug: &str) -> Result<()> {
        let p = self.slug_path("reqs", slug)?;
        req_cache().invalidate(&p);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    }
    /// Every requirement file, as its list of revs (newest last).
    pub fn all_reqs(&self) -> Result<Vec<Vec<ReqRev>>> {
        let mut out = vec![];
        for p in self.list_yaml(&self.root.join("reqs"))? {
            out.push((*req_cache().get_or_load(&p, || self.read_yaml(&p))?).clone());
        }
        Ok(out)
    }
    /// Current rev of every requirement slug.
    pub fn current_reqs(&self) -> Result<Vec<ReqRev>> {
        Ok(self
            .all_reqs()?
            .into_iter()
            .filter_map(|revs| revs.into_iter().last())
            .collect())
    }
    pub fn current_req(&self, slug: &str) -> Result<Option<ReqRev>> {
        Ok(self.req_revs(slug)?.pop())
    }

    // questions
    pub fn qst_revs(&self, slug: &str) -> Result<Vec<Question>> {
        let p = self.slug_path("questions", slug)?;
        if !p.exists() {
            return Ok(vec![]);
        }
        Ok((*qst_cache().get_or_load(&p, || self.read_yaml(&p))?).clone())
    }
    pub fn save_qst_revs(&self, slug: &str, revs: &[Question]) -> Result<()> {
        let p = self.slug_path("questions", slug)?;
        qst_cache().invalidate(&p);
        self.save_revs("questions", slug, revs)
    }
    pub fn current_qsts(&self) -> Result<Vec<Question>> {
        let mut out = vec![];
        for p in self.list_yaml(&self.root.join("questions"))? {
            if let Some(q) = qst_cache().get_or_load(&p, || self.read_yaml(&p))?.last() {
                out.push(q.clone());
            }
        }
        Ok(out)
    }

    // groups
    pub fn grp_revs(&self, slug: &str) -> Result<Vec<Group>> {
        let p = self.slug_path("groups", slug)?;
        if !p.exists() {
            return Ok(vec![]);
        }
        Ok((*grp_cache().get_or_load(&p, || self.read_yaml(&p))?).clone())
    }
    pub fn save_grp_revs(&self, slug: &str, revs: &[Group]) -> Result<()> {
        let p = self.slug_path("groups", slug)?;
        grp_cache().invalidate(&p);
        self.save_revs("groups", slug, revs)
    }
    pub fn current_grps(&self) -> Result<Vec<Group>> {
        let mut out = vec![];
        for p in self.list_yaml(&self.root.join("groups"))? {
            if let Some(g) = grp_cache().get_or_load(&p, || self.read_yaml(&p))?.last() {
                out.push(g.clone());
            }
        }
        Ok(out)
    }

    /// Group titles per requirement key, derived (`obj~req-groups-derived~1`).
    pub fn groups_by_req(&self) -> Result<std::collections::BTreeMap<Key, Vec<String>>> {
        let mut m: std::collections::BTreeMap<Key, Vec<String>> = Default::default();
        for g in self.current_grps()? {
            for mem in &g.members {
                m.entry(mem.key()).or_default().push(g.title.clone());
            }
        }
        for v in m.values_mut() {
            v.sort();
            v.dedup();
        }
        Ok(m)
    }

    // ----- snapshots & current pointer -----

    pub fn snapshot_path(&self, doc: &str, sha: &str) -> PathBuf {
        self.root
            .join("snapshots")
            .join(doc_key(doc))
            .join(format!("{sha}.yaml"))
    }
    pub fn has_snapshot(&self, doc: &str, sha: &str) -> bool {
        self.snapshot_path(doc, sha).exists()
    }
    pub fn save_snapshot(&self, snap: &crate::snapshot::Snapshot) -> Result<()> {
        let p = self.snapshot_path(&snap.doc, &snap.sha);
        if p.exists() {
            return Ok(()); // immutable
        }
        self.write_yaml(&p, snap)
    }
    /// Overwrite a snapshot with a re-derived one after a segmenter upgrade. Callers must have
    /// verified every span id is preserved — anchors resolve by id (`obj~span-id~1`).
    pub fn replace_snapshot(&self, snap: &crate::snapshot::Snapshot) -> Result<()> {
        let p = self.snapshot_path(&snap.doc, &snap.sha);
        if let Some(c) = SNAP_CACHE.get() {
            c.lock().unwrap().remove(&p);
        }
        self.write_yaml(&p, snap)
    }
    pub fn load_snapshot(&self, doc: &str, sha: &str) -> Result<crate::snapshot::Snapshot> {
        Ok((*self.load_snapshot_arc(doc, sha)?).clone())
    }
    /// Snapshots are immutable: cached forever by path (shared, no clone).
    pub fn load_snapshot_arc(
        &self,
        doc: &str,
        sha: &str,
    ) -> Result<Arc<crate::snapshot::Snapshot>> {
        let p = self.snapshot_path(doc, sha);
        let cache = SNAP_CACHE.get_or_init(Default::default);
        if let Some(s) = cache.lock().unwrap().get(&p) {
            return Ok(s.clone());
        }
        let s: Arc<crate::snapshot::Snapshot> = Arc::new(self.read_yaml(&p)?);
        cache.lock().unwrap().insert(p, s.clone());
        Ok(s)
    }
    pub fn current_snapshot_arc(
        &self,
        ctx: &Context,
        doc: &str,
    ) -> Result<Option<Arc<crate::snapshot::Snapshot>>> {
        match self.current_sha(ctx, doc)? {
            Some(sha) => Ok(Some(self.load_snapshot_arc(doc, &sha)?)),
            None => Ok(None),
        }
    }

    fn current_ptr(&self, ctx: &Context, doc: &str) -> PathBuf {
        self.root.join("current").join(ctx.key()).join(doc_key(doc))
    }
    /// sha of the current snapshot for (context, doc), if any (`obj~snapshot-current~1`).
    pub fn current_sha(&self, ctx: &Context, doc: &str) -> Result<Option<String>> {
        let p = self.current_ptr(ctx, doc);
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(p)?.trim().to_string()))
    }
    pub fn set_current_sha(&self, ctx: &Context, doc: &str, sha: &str) -> Result<()> {
        self.write_atomic(&self.current_ptr(ctx, doc), sha.as_bytes())
    }
    pub fn current_snapshot(
        &self,
        ctx: &Context,
        doc: &str,
    ) -> Result<Option<crate::snapshot::Snapshot>> {
        match self.current_sha(ctx, doc)? {
            Some(sha) => Ok(Some(self.load_snapshot(doc, &sha)?)),
            None => Ok(None),
        }
    }

    // ----- marks -----

    fn marks_path(&self, ctx: &Context, doc: &str) -> PathBuf {
        self.root
            .join("marks")
            .join(ctx.key())
            .join(format!("{}.yaml", doc_key(doc)))
    }
    pub fn marks(&self, ctx: &Context, doc: &str) -> Result<Marks> {
        Ok(self
            .read_yaml_opt(&self.marks_path(ctx, doc))?
            .unwrap_or_default())
    }
    pub fn save_marks(&self, ctx: &Context, doc: &str, m: &Marks) -> Result<()> {
        self.write_yaml(&self.marks_path(ctx, doc), m)
    }

    // ----- rounds -----

    fn rounds_dir(&self, ctx: &Context, doc: &str) -> PathBuf {
        self.root.join("rounds").join(ctx.key()).join(doc_key(doc))
    }
    pub fn rounds(&self, ctx: &Context, doc: &str) -> Result<Vec<Round>> {
        let mut v: Vec<Round> = vec![];
        for p in self.list_yaml(&self.rounds_dir(ctx, doc))? {
            v.push(self.read_yaml(&p)?);
        }
        v.sort_by_key(|r| r.n);
        Ok(v)
    }
    pub fn save_round(&self, r: &Round) -> Result<()> {
        self.write_yaml(
            &self
                .rounds_dir(&r.context, &r.doc)
                .join(format!("{:04}.yaml", r.n)),
            r,
        )
    }
    pub fn open_round(&self, ctx: &Context, doc: &str) -> Result<Option<Round>> {
        Ok(self
            .rounds(ctx, doc)?
            .into_iter()
            .find(|r| r.closed.is_none()))
    }

    // ----- pending reconciliation -----

    fn pending_path(&self, ctx: &Context, doc: &str) -> PathBuf {
        self.root
            .join("pending")
            .join(ctx.key())
            .join(format!("{}.yaml", doc_key(doc)))
    }
    pub fn pending(
        &self,
        ctx: &Context,
        doc: &str,
    ) -> Result<Option<crate::reconcile::Reconciliation>> {
        self.read_yaml_opt(&self.pending_path(ctx, doc))
    }
    pub fn save_pending(&self, ctx: &Context, r: &crate::reconcile::Reconciliation) -> Result<()> {
        self.write_yaml(&self.pending_path(ctx, &r.doc), r)
    }
    pub fn clear_pending(&self, ctx: &Context, doc: &str) -> Result<()> {
        let p = self.pending_path(ctx, doc);
        if p.exists() {
            fs::remove_file(p)?;
        }
        Ok(())
    }

    // ----- exports -----

    pub fn last_export(&self) -> Result<Option<ExportRecord>> {
        self.read_yaml_opt(&self.root.join("exports").join("last.yaml"))
    }
    pub fn save_last_export(&self, e: &ExportRecord) -> Result<()> {
        self.write_yaml(&self.root.join("exports").join("last.yaml"), e)
    }

    // ----- helpers -----

    /// Next free rev for a slug in a dir (1 if none).
    pub fn next_req_rev(&self, slug: &str) -> Result<u32> {
        Ok(self
            .req_revs(slug)?
            .last()
            .map(|r| r.id.rev + 1)
            .unwrap_or(1))
    }

    /// Ensure the slug is unused across reqs; else append `-2`, `-3`, …
    pub fn free_req_slug(&self, base: &str) -> Result<String> {
        if !valid_slug(base) {
            bail!("invalid slug `{base}`");
        }
        if self.req_revs(base)?.is_empty() {
            return Ok(base.to_string());
        }
        for i in 2..1000 {
            let s = format!("{base}-{i}");
            if self.req_revs(&s)?.is_empty() {
                return Ok(s);
            }
        }
        bail!("could not find a free slug for `{base}`")
    }

    pub fn resolve_req(&self, id: &Id) -> Result<Option<ReqRev>> {
        Ok(self
            .req_revs(&id.slug)?
            .into_iter()
            .find(|r| r.id.rev == id.rev))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{now, RepoConfig};

    fn cfg() -> RepoConfig {
        RepoConfig {
            github: "o/n".into(),
            remote: "https://github.com/o/n.git".into(),
            kind: Default::default(),
            source_dir: None,
            default_branch: "main".into(),
            local_path: "/tmp/x".into(),
            tracked: vec![],
            registered_at: now(),
            last_fetch: None,
        }
    }

    #[test]
    fn init_open_and_req_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        let s = Store::init(&root, &cfg()).unwrap();
        assert!(Store::open(&root).is_ok());
        assert!(Store::open(dir.path()).is_err());

        let _l = s.lock().unwrap();
        let id: Id = "req~a~1".parse().unwrap();
        let r = ReqRev::new(id.clone(), "s", "t");
        s.save_req_revs("a", &[r.clone()]).unwrap();
        assert_eq!(s.req_revs("a").unwrap(), vec![r.clone()]);
        assert_eq!(s.current_reqs().unwrap().len(), 1);
        assert_eq!(s.next_req_rev("a").unwrap(), 2);
        assert_eq!(s.next_req_rev("b").unwrap(), 1);
        assert_eq!(s.free_req_slug("a").unwrap(), "a-2");
        assert_eq!(s.resolve_req(&id).unwrap(), Some(r));
        assert!(s.req_revs("nope").unwrap().is_empty());
        assert!(s.save_req_revs("Bad Slug", &[]).is_err());
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::init(&dir.path().join("s"), &cfg()).unwrap();
        let p = s.root().join("x.txt");
        s.write_atomic(&p, b"one").unwrap();
        s.write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        assert!(fs::read_dir(s.root()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("tmp-")));
    }

    #[test]
    fn groups_by_req_derived() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::init(&dir.path().join("s"), &cfg()).unwrap();
        let g = Group {
            id: "grp~validation~1".parse().unwrap(),
            title: "Validation".into(),
            description: None,
            members: vec!["req~a~1".parse().unwrap(), "req~b~2".parse().unwrap()],
            history: vec![],
        };
        s.save_grp_revs("validation", &[g]).unwrap();
        let m = s.groups_by_req().unwrap();
        assert_eq!(
            m[&Key {
                ty: "req".into(),
                slug: "a".into()
            }],
            vec!["Validation"]
        );
    }

    #[test]
    fn current_pointer_and_rounds() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::init(&dir.path().join("s"), &cfg()).unwrap();
        let ctx = Context::Branch {
            branch: "main".into(),
        };
        assert_eq!(s.current_sha(&ctx, "docs/hld.md").unwrap(), None);
        s.set_current_sha(&ctx, "docs/hld.md", "abc").unwrap();
        assert_eq!(
            s.current_sha(&ctx, "docs/hld.md").unwrap().as_deref(),
            Some("abc")
        );
        let r = Round {
            doc: "docs/hld.md".into(),
            n: 1,
            snapshot: "abc".into(),
            context: ctx.clone(),
            opened: now(),
            closed: None,
            summary: None,
        };
        s.save_round(&r).unwrap();
        assert_eq!(s.open_round(&ctx, "docs/hld.md").unwrap().unwrap().n, 1);
        assert!(s.rounds_dir(&ctx, "docs/hld.md").join("0001.yaml").exists());
    }
}
