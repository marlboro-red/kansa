//! GitHub repo access (`ui~repo-register~1`, `ui~gh-primary~1`).
//!
//! Network operations go through the GitHub CLI: `gh` handles auth (`gh auth setup-git`),
//! repo metadata (`gh repo view`, `gh pr list`) and the system `git` it fronts does fetches.
//! libgit2 is used for *local* reads (blobs, trees) and as a network fallback when `gh` is
//! not installed. Clones are bare: kansa never checks out or edits the PM's HLD.

use anyhow::{anyhow, bail, Context, Result};
use git2::{Cred, FetchOptions, RemoteCallbacks, Repository};
use std::path::Path;
use std::process::Command;
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
        Command::new("gh")
            .args(["auth", "status"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Run `gh <args>` and return stdout; error carries stderr.
pub fn gh(args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
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
    let out = Command::new("git")
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
        let _ = Command::new("gh")
            .args(["auth", "setup-git", "--hostname", "github.com"])
            .output();
    });
}

/// Resolve a GitHub token: `$GITHUB_TOKEN`, `$GH_TOKEN`, else `gh auth token` if `gh` is installed.
pub fn github_token() -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(k) {
            if !t.trim().is_empty() {
                return Some(t.trim().to_string());
            }
        }
    }
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if out.status.success() {
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

fn callbacks<'a>(token: Option<String>) -> RemoteCallbacks<'a> {
    let mut cb = RemoteCallbacks::new();
    cb.credentials(move |url, username, allowed| {
        if allowed.is_user_pass_plaintext() {
            if let Some(t) = &token {
                return Cred::userpass_plaintext("x-access-token", t);
            }
            if let Ok(cfg) = git2::Config::open_default() {
                if let Ok(c) = Cred::credential_helper(&cfg, url, username) {
                    return Ok(c);
                }
            }
        }
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
            "no usable credentials (set GITHUB_TOKEN or run `gh auth login`)",
        ))
    });
    cb
}

fn fetch_opts<'a>(token: Option<String>) -> FetchOptions<'a> {
    let mut fo = FetchOptions::new();
    fo.remote_callbacks(callbacks(token));
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
        if use_gh_for(url) {
            ensure_gh_git_auth();
            git(dest, &["fetch", "--no-tags", "origin"])
                .with_context(|| format!("cloning {url} via gh/git"))?;
        } else {
            remote
                .fetch(&REFSPECS, Some(&mut fetch_opts(github_token())), None)
                .with_context(|| format!("cloning {url}"))?;
        }
    }
    Ok(repo)
}

/// Fetch branches and PR heads from origin.
pub fn fetch(repo: &Repository) -> Result<()> {
    let mut remote = repo.find_remote("origin")?;
    let url = remote.url().unwrap_or_default().to_string();
    if use_gh_for(&url) {
        ensure_gh_git_auth();
        git(repo.path(), &["fetch", "--no-tags", "origin"])
            .context("fetching origin via gh/git")?;
    } else {
        remote
            .fetch(&REFSPECS, Some(&mut fetch_opts(github_token())), None)
            .context("fetching origin")?;
    }
    Ok(())
}

/// gh/git path is used for github.com URLs when gh is available; local `file://` and other
/// hosts go through libgit2.
fn use_gh_for(url: &str) -> bool {
    url.contains("github.com") && gh_available()
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
            .connect_auth(
                git2::Direction::Fetch,
                Some(callbacks(github_token())),
                None,
            )
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
    if !gh_available() {
        bail!("listing PRs requires the GitHub CLI (`gh`) — install it and run `gh auth login`");
    }
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
        // make sure the branch is named main
        let head = r.head().unwrap().peel_to_commit().unwrap();
        let _ = r.branch("main", &head, true);

        let dest = tempfile::tempdir().unwrap();
        let url = format!("file://{}", src.path().display());
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
