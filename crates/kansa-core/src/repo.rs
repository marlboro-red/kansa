//! GitHub repo access (`ui~repo-register~1`, `ui~gh-required~1`).
//!
//! The GitHub CLI is a hard requirement for github.com repos: `gh` handles auth
//! (`gh auth setup-git`), repo metadata (`gh repo view`), PRs (`gh pr list`, `gh pr view`),
//! and the system `git` it fronts does fetches. libgit2 is used only for *local* reads
//! (blobs, trees, diffs) and for non-GitHub URLs such as `file://` test repos.
//! Clones are bare: kansa never checks out or edits the PM's HLD.

use anyhow::{anyhow, bail, Context, Result};
use git2::{Cred, FetchOptions, RemoteCallbacks, Repository};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const REFSPECS: [&str; 2] = [
    "+refs/heads/*:refs/remotes/origin/*",
    "+refs/pull/*/head:refs/pull/*/head",
];

/// Is `gh` installed and authenticated? Cached per process.
pub fn gh_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        if std::env::var("KANSA_NO_GH").is_ok() {
            return false;
        }
        crate::proc::command("gh")
            .args(["auth", "status"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Fail with an actionable message when the GitHub CLI is missing or logged out.
pub fn require_gh() -> Result<()> {
    if gh_available() {
        return Ok(());
    }
    let installed = crate::proc::command("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if installed {
        bail!("GitHub CLI is not logged in — run `gh auth login` and try again")
    } else {
        bail!("kansa needs the GitHub CLI: install `gh` (https://cli.github.com) and run `gh auth login`")
    }
}

/// Run `gh <args>` and return stdout; error carries stderr.
pub fn gh(args: &[&str]) -> Result<String> {
    let out = crate::proc::command("gh")
        .args(args)
        .output()
        .with_context(|| format!("running gh {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `git -C <dir> <args>` (system git, credentials via `gh auth setup-git`).
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = crate::proc::command("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Make sure git can authenticate to github.com through gh (idempotent, best-effort).
fn ensure_gh_git_auth() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let _ = crate::proc::command("gh")
            .args(["auth", "setup-git", "--hostname", "github.com"])
            .output();
    });
}

fn callbacks<'a>() -> RemoteCallbacks<'a> {
    let mut cb = RemoteCallbacks::new();
    cb.credentials(|_url, username, allowed| {
        if allowed.is_ssh_key() {
            if let Some(u) = username {
                if let Ok(c) = Cred::ssh_key_from_agent(u) {
                    return Ok(c);
                }
            }
        }
        if allowed.is_default() {
            return Cred::default();
        }
        Err(git2::Error::from_str(
            "no credentials for non-GitHub remote",
        ))
    });
    cb
}

fn fetch_opts<'a>() -> FetchOptions<'a> {
    let mut fo = FetchOptions::new();
    fo.remote_callbacks(callbacks());
    fo.download_tags(git2::AutotagOption::None);
    fo
}

/// `owner/name` → clone URL.
pub fn github_url(github: &str) -> Result<String> {
    let parts: Vec<&str> = github
        .trim()
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .split('/')
        .collect();
    let (owner, name) = match parts.as_slice() {
        [owner, name] => (*owner, *name),
        // accept full URLs too
        p if p.len() >= 2 && github.contains("github.com") => (p[p.len() - 2], p[p.len() - 1]),
        _ => bail!("expected `owner/name`, got `{github}`"),
    };
    Ok(format!("https://github.com/{owner}/{name}.git"))
}

/// Normalize to `owner/name`.
pub fn github_slug(github: &str) -> Result<String> {
    let url = github_url(github)?;
    let s = url
        .trim_start_matches("https://github.com/")
        .trim_end_matches(".git");
    Ok(s.to_string())
}

/// Clone (or open, if already present) a bare clone at `dest` with branch + PR-head refspecs.
pub fn clone_or_open(url: &str, dest: &Path) -> Result<Repository> {
    if dest.join("HEAD").exists() {
        return Repository::open_bare(dest).with_context(|| format!("opening {}", dest.display()));
    }
    std::fs::create_dir_all(dest)?;
    let repo = Repository::init_bare(dest)?;
    {
        let mut remote = repo.remote("origin", url)?;
        // `remote()` already adds the heads refspec; add PR heads too.
        repo.remote_add_fetch("origin", REFSPECS[1])?;
        if is_github(url) {
            require_gh()?;
            ensure_gh_git_auth();
            git(dest, &["fetch", "--no-tags", "origin"])
                .with_context(|| format!("cloning {url} via gh/git"))?;
        } else {
            remote
                .fetch(&REFSPECS, Some(&mut fetch_opts()), None)
                .with_context(|| format!("cloning {url}"))?;
        }
    }
    Ok(repo)
}

/// Fetch branches and PR heads from origin.
pub fn fetch(repo: &Repository) -> Result<()> {
    let mut remote = repo.find_remote("origin")?;
    let url = remote.url().unwrap_or_default().to_string();
    if is_github(&url) {
        require_gh()?;
        ensure_gh_git_auth();
        git(repo.path(), &["fetch", "--no-tags", "origin"])
            .context("fetching origin via gh/git")?;
    } else {
        remote
            .fetch(&REFSPECS, Some(&mut fetch_opts()), None)
            .context("fetching origin")?;
    }
    Ok(())
}

/// github.com URLs go through gh/git; local `file://` and other hosts go through libgit2.
fn is_github(url: &str) -> bool {
    url.contains("github.com")
}

/// `owner/name` for a github URL, if it is one.
pub fn slug_of_url(url: &str) -> Option<String> {
    github_slug(url).ok().filter(|_| url.contains("github.com"))
}

/// Default branch name: `gh repo view` when available, else `refs/remotes/origin/HEAD`, else
/// ask the remote, else main/master heuristics.
pub fn default_branch(repo: &Repository) -> Result<String> {
    if let Some(slug) = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().and_then(slug_of_url))
    {
        if gh_available() {
            if let Ok(out) = gh(&[
                "repo",
                "view",
                &slug,
                "--json",
                "defaultBranchRef",
                "-q",
                ".defaultBranchRef.name",
            ]) {
                let b = out.trim();
                if !b.is_empty() {
                    return Ok(b.to_string());
                }
            }
        }
    }
    if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(t) = r.symbolic_target() {
            return Ok(t.trim_start_matches("refs/remotes/origin/").to_string());
        }
    }
    // Ask the remote.
    if let Ok(mut remote) = repo.find_remote("origin") {
        if remote
            .connect_auth(git2::Direction::Fetch, Some(callbacks()), None)
            .is_ok()
        {
            if let Ok(buf) = remote.default_branch() {
                if let Some(s) = buf.as_str() {
                    let _ = remote.disconnect();
                    return Ok(s.trim_start_matches("refs/heads/").to_string());
                }
            }
            let _ = remote.disconnect();
        }
    }
    for b in ["main", "master"] {
        if repo
            .find_reference(&format!("refs/remotes/origin/{b}"))
            .is_ok()
        {
            return Ok(b.into());
        }
    }
    bail!("could not determine default branch")
}

/// Resolve `refs/remotes/origin/<branch>` or `refs/pull/<n>/head` or a raw sha to a commit.
pub fn resolve_commit<'r>(repo: &'r Repository, refname: &str) -> Result<git2::Commit<'r>> {
    let candidates = [
        refname.to_string(),
        format!("refs/remotes/origin/{refname}"),
        format!("refs/pull/{refname}/head"),
    ];
    for c in &candidates {
        if let Ok(r) = repo.find_reference(c) {
            return Ok(r.peel_to_commit()?);
        }
    }
    if let Ok(oid) = git2::Oid::from_str(refname) {
        if let Ok(c) = repo.find_commit(oid) {
            return Ok(c);
        }
    }
    Err(anyhow!("cannot resolve `{refname}`"))
}

pub fn branch_ref(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}
pub fn pr_ref(n: u64) -> String {
    format!("refs/pull/{n}/head")
}

/// Read a file's content and blob sha at a ref. Returns None if the path is absent.
pub fn read_blob(repo: &Repository, refname: &str, path: &str) -> Result<Option<(String, String)>> {
    let commit = resolve_commit(repo, refname)?;
    let tree = commit.tree()?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(e) => e,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let obj = entry.to_object(repo)?;
    let blob = obj
        .as_blob()
        .ok_or_else(|| anyhow!("{path} is not a file"))?;
    let content = String::from_utf8_lossy(blob.content()).into_owned();
    Ok(Some((content, blob.id().to_string())))
}

/// Raw bytes of a blob at a ref — images and other binary assets. Resolves git-LFS pointers:
/// the object is read from the clone's `lfs/objects` store, after a targeted `git lfs fetch`
/// if it isn't there yet (requires `git-lfs`; the error says so when it's missing).
pub fn read_blob_raw(repo: &Repository, refname: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let commit = resolve_commit(repo, refname)?;
    let tree = commit.tree()?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(e) => e,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let obj = entry.to_object(repo)?;
    let blob = obj
        .as_blob()
        .ok_or_else(|| anyhow!("{path} is not a file"))?;
    let bytes = blob.content();
    if let Some(oid) = lfs_pointer_oid(bytes) {
        return Ok(Some(read_lfs_object(repo, &commit.id().to_string(), path, &oid)?));
    }
    Ok(Some(bytes.to_vec()))
}

/// `sha256` oid of a git-LFS pointer file, or None for regular content.
pub fn lfs_pointer_oid(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(b"version https://git-lfs.github.com/spec/") {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("oid sha256:"))
        .map(|o| o.trim().to_string())
}

fn lfs_object_path(repo: &Repository, oid: &str) -> PathBuf {
    // Bare clone: the LFS store lives under the git dir itself.
    repo.path().join("lfs").join("objects").join(&oid[..2]).join(&oid[2..4]).join(oid)
}

fn read_lfs_object(repo: &Repository, commit: &str, path: &str, oid: &str) -> Result<Vec<u8>> {
    let obj = lfs_object_path(repo, oid);
    if !obj.exists() {
        // Targeted fetch of just this path at this commit. Uses the system git — same policy
        // as every other network operation here.
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["lfs", "fetch", "origin", commit, "-I", path])
            .output()
            .map_err(|e| anyhow!("running git lfs: {e} — is git-lfs installed?"))?;
        if !out.status.success() {
            bail!(
                "git lfs fetch failed for {path}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    std::fs::read(&obj)
        .with_context(|| format!("LFS object for {path} not present after fetch ({oid})"))
}

/// All markdown paths at a ref, forward-slash, sorted.
pub fn list_markdown(repo: &Repository, refname: &str) -> Result<Vec<String>> {
    let commit = resolve_commit(repo, refname)?;
    let tree = commit.tree()?;
    let mut out = vec![];
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let Some(name) = entry.name() {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".md") || lower.ends_with(".markdown") {
                    out.push(format!("{dir}{name}"));
                }
            }
        }
        git2::TreeWalkResult::Ok
    })?;
    out.sort();
    Ok(out)
}

/// Markdown files changed by `head_ref` relative to its merge-base with `base_ref`:
/// (path, status) with status one of added|modified|deleted|renamed.
pub fn changed_markdown(
    repo: &Repository,
    base_ref: &str,
    head_ref: &str,
) -> Result<Vec<(String, String)>> {
    let base = resolve_commit(repo, base_ref)?;
    let head = resolve_commit(repo, head_ref)?;
    let mb = repo.merge_base(base.id(), head.id()).unwrap_or(base.id());
    let old_tree = repo.find_commit(mb)?.tree()?;
    let new_tree = head.tree()?;
    let mut opts = git2::DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(&mut opts))?;
    let mut out = vec![];
    for d in diff.deltas() {
        let path = d
            .new_file()
            .path()
            .or(d.old_file().path())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let lower = path.to_ascii_lowercase();
        if !(lower.ends_with(".md") || lower.ends_with(".markdown")) {
            continue;
        }
        let status = match d.status() {
            git2::Delta::Added => "added",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Renamed => "renamed",
            _ => "modified",
        };
        out.push((path, status.to_string()));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Files changed by a PR according to GitHub (`gh pr view --json files`).
pub fn pr_changed_files(slug: &str, pr: u64) -> Result<Vec<String>> {
    require_gh()?;
    let out = gh(&[
        "pr",
        "view",
        &pr.to_string(),
        "--repo",
        slug,
        "--json",
        "files",
        "-q",
        ".files[].path",
    ])?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Head sha of a ref.
pub fn head_sha(repo: &Repository, refname: &str) -> Result<String> {
    Ok(resolve_commit(repo, refname)?.id().to_string())
}

/// Open PR metadata via `gh pr list` (`ui~pr-view~1`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    #[serde(rename = "headRefOid")]
    pub head: String,
    #[serde(rename = "headRefName")]
    pub head_ref: String,
    #[serde(rename = "baseRefName")]
    pub base_ref: String,
    #[serde(default)]
    pub author: serde_json::Value,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "isDraft", default)]
    pub draft: bool,
    #[serde(default)]
    pub files: Vec<serde_json::Value>,
}

pub fn list_prs(slug: &str) -> Result<Vec<PrInfo>> {
    require_gh()?;
    let out = gh(&[
        "pr",
        "list",
        "--repo",
        slug,
        "--state",
        "open",
        "--limit",
        "100",
        "--json",
        "number,title,headRefOid,headRefName,baseRefName,author,updatedAt,isDraft,files",
    ])?;
    serde_json::from_str(&out).context("parsing gh pr list output")
}

/// PR numbers we have heads for (from a fetch).
pub fn fetched_prs(repo: &Repository) -> Result<Vec<u64>> {
    let mut v = vec![];
    for r in repo.references_glob("refs/pull/*/head")? {
        let r = r?;
        if let Some(name) = r.name() {
            if let Some(n) = name
                .strip_prefix("refs/pull/")
                .and_then(|s| s.strip_suffix("/head"))
            {
                if let Ok(n) = n.parse() {
                    v.push(n);
                }
            }
        }
    }
    v.sort_unstable();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls() {
        assert_eq!(
            github_url("marlboro-red/reqtrace").unwrap(),
            "https://github.com/marlboro-red/reqtrace.git"
        );
        assert_eq!(
            github_url("https://github.com/marlboro-red/reqtrace/").unwrap(),
            "https://github.com/marlboro-red/reqtrace.git"
        );
        assert_eq!(github_slug("https://github.com/a/b.git").unwrap(), "a/b");
        assert!(github_url("nope").is_err());
    }

    /// Build a tiny local repo, "clone" it (file:// URL), read blobs and list docs.
    #[test]
    fn clone_read_list_local() {
        let src = tempfile::tempdir().unwrap();
        let r = Repository::init(src.path()).unwrap();
        std::fs::create_dir_all(src.path().join("docs")).unwrap();
        std::fs::write(src.path().join("docs/hld.md"), "# H\n\nA sentence.\n").unwrap();
        std::fs::write(src.path().join("README.md"), "readme\n").unwrap();
        std::fs::write(src.path().join("code.rs"), "fn main(){}\n").unwrap();
        let mut idx = r.index().unwrap();
        idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        idx.write().unwrap();
        let tree = r.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        r.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // make sure the branch is named main — and HEAD points at it, whatever the machine's
        // `init.defaultBranch` says (the clone reads the source's HEAD symref)
        let head = r.head().unwrap().peel_to_commit().unwrap();
        let _ = r.branch("main", &head, true);
        r.set_head("refs/heads/main").unwrap();

        let dest = tempfile::tempdir().unwrap();
        let url = crate::ops::tests::file_url(src.path());
        let bare = clone_or_open(&url, &dest.path().join("clone")).unwrap();
        assert_eq!(default_branch(&bare).unwrap(), "main");
        let docs = list_markdown(&bare, "main").unwrap();
        assert_eq!(docs, vec!["README.md", "docs/hld.md"]);
        let (content, sha) = read_blob(&bare, "main", "docs/hld.md").unwrap().unwrap();
        assert_eq!(content, "# H\n\nA sentence.\n");
        assert_eq!(sha, crate::snapshot::blob_sha(content.as_bytes()));
        assert!(read_blob(&bare, "main", "missing.md").unwrap().is_none());
        // reopen path
        let again = clone_or_open(&url, &dest.path().join("clone")).unwrap();
        fetch(&again).unwrap();
        assert!(head_sha(&again, "main").is_ok());
    }
}

// ---------- local folders (no GitHub) ----------

/// Import a folder's markdown files as a commit on `refs/remotes/origin/<branch>` (and
/// `refs/heads/<branch>`) of the bare repo, without a worktree or system git. Returns true if a
/// new commit was created (something changed). Skips `.git`, `node_modules`, `target`, and
/// hidden directories.
pub fn import_folder(repo: &Repository, dir: &Path, branch: &str) -> Result<bool> {
    let mut files: Vec<(String, Vec<u8>)> = vec![];
    collect_markdown(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    // Build a nested tree from the flat path list.
    let tree_oid = build_tree(repo, &files)?;
    let refname = branch_ref(branch);
    let parent = repo
        .find_reference(&refname)
        .ok()
        .and_then(|r| r.peel_to_commit().ok());
    if let Some(p) = &parent {
        if p.tree_id() == tree_oid {
            return Ok(false);
        }
    }
    let tree = repo.find_tree(tree_oid)?;
    let sig =
        git2::Signature::now("kansa", "kansa@local").map_err(|e| anyhow!("signature: {e}"))?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let msg = format!(
        "import {} markdown file(s) from {}",
        files.len(),
        dir.display()
    );
    let oid = repo.commit(None, &sig, &sig, &msg, &tree, &parents)?;
    repo.reference(&refname, oid, true, "kansa import")?;
    repo.reference(&format!("refs/heads/{branch}"), oid, true, "kansa import")?;
    if repo.find_reference("refs/remotes/origin/HEAD").is_err() {
        let _ = repo.reference_symbolic("refs/remotes/origin/HEAD", &refname, true, "kansa import");
    }
    Ok(true)
}

fn collect_markdown(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let ft = e.file_type()?;
        if ft.is_dir() {
            if name.starts_with('.') || name == "node_modules" || name == "target" || name == "dist"
            {
                continue;
            }
            collect_markdown(root, &path, out)?;
        } else if ft.is_file() {
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".md") || lower.ends_with(".markdown") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&path)?));
            }
        }
    }
    Ok(())
}

fn build_tree(repo: &Repository, files: &[(String, Vec<u8>)]) -> Result<git2::Oid> {
    // Recursive builder over path components.
    fn build(repo: &Repository, prefix: &str, files: &[(String, Vec<u8>)]) -> Result<git2::Oid> {
        let mut tb = repo.treebuilder(None)?;
        let mut i = 0;
        while i < files.len() {
            let rest = &files[i].0[prefix.len()..];
            match rest.find('/') {
                None => {
                    let oid = repo.blob(&files[i].1)?;
                    tb.insert(rest, oid, 0o100644)?;
                    i += 1;
                }
                Some(k) => {
                    let dirname = &rest[..k];
                    let sub_prefix = format!("{prefix}{dirname}/");
                    let mut j = i;
                    while j < files.len() && files[j].0.starts_with(&sub_prefix) {
                        j += 1;
                    }
                    let sub = build(repo, &sub_prefix, &files[i..j])?;
                    tb.insert(dirname, sub, 0o040000)?;
                    i = j;
                }
            }
        }
        Ok(tb.write()?)
    }
    build(repo, "", files)
}

/// Create (or open) the private bare repo backing a local folder.
pub fn open_local_backing(dest: &Path) -> Result<Repository> {
    if dest.join("HEAD").exists() {
        return Repository::open_bare(dest).with_context(|| format!("opening {}", dest.display()));
    }
    std::fs::create_dir_all(dest)?;
    let repo = Repository::init_bare(dest)?;
    // A nominal origin so code that inspects the remote keeps working.
    let _ = repo.remote("origin", "local://folder");
    Ok(repo)
}
