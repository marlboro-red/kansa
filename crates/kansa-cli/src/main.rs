//! `kansa` — thin CLI over kansa-core (scripting, CI). Every mutation goes through core ops.

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use kansa_core::model::{Context, Level, Pattern, Status};
use kansa_core::ops::{self, Workspace};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kansa",
    version,
    about = "Prose HLD → traceable requirement inventory"
)]
struct Cli {
    /// Emit JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args)]
struct RepoArg {
    /// GitHub repo as `owner/name` (or URL).
    #[arg(long, short = 'r', env = "KANSA_REPO")]
    repo: String,
}

#[derive(Args)]
struct DocArg {
    #[command(flatten)]
    repo: RepoArg,
    /// Tracked doc path (forward slashes), e.g. docs/hld.md
    #[arg(long, short = 'd')]
    doc: String,
    /// Span ids to act on.
    #[arg(required = true)]
    spans: Vec<String>,
    /// Who is acting (defaults to $USER).
    #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
    by: String,
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage registered repos.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Manage tracked docs.
    Doc {
        #[command(subcommand)]
        cmd: DocCmd,
    },
    /// Show coverage and inventory rollups.
    Status(RepoArg),
    /// Export the inventory in reqtrace format.
    Export {
        #[command(flatten)]
        repo: RepoArg,
        /// Output directory (default: <store>/exports/reqtrace).
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        #[arg(long, default_value = "reqtrace")]
        format: String,
        /// Skip running `reqtrace validate` even if the binary is available.
        #[arg(long)]
        no_validate: bool,
    },
    /// Print a doc's spans with their classification state.
    Spans {
        #[command(flatten)]
        repo: RepoArg,
        #[arg(long, short = 'd')]
        doc: String,
        /// Only unclassified spans.
        #[arg(long, short = 'u')]
        residue: bool,
    },
    /// Classification actions (the core loop, scriptable).
    Classify {
        #[command(subcommand)]
        cmd: ClassifyCmd,
    },
    /// Close the open round for a doc (requires residue = 0).
    Close {
        #[command(flatten)]
        repo: RepoArg,
        #[arg(long, short = 'd')]
        doc: String,
    },
    /// Requirement edits.
    Req {
        #[command(subcommand)]
        cmd: ReqCmd,
    },
    /// Requirement groups — umbrella labels for oversight.
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
    /// Dev bridge: serve the app's command surface over HTTP so the frontend can run in a
    /// browser (same core ops as the Tauri app). Binds 127.0.0.1 only. No auth — use `kansa ui`
    /// for the tokened, self-contained mode.
    Serve {
        #[arg(long, default_value_t = 1430)]
        port: u16,
    },
    /// Open the kansa UI: serves the embedded frontend and the command surface on 127.0.0.1
    /// with a per-session token, in a standalone app window (falls back to your browser).
    Ui {
        /// Port to bind (default: 1424, falling back to a free port if taken; 0 = always free).
        #[arg(long)]
        port: Option<u16>,
        /// Print the URL instead of opening anything.
        #[arg(long)]
        no_open: bool,
        /// Open a normal browser tab instead of a standalone app window.
        #[arg(long)]
        browser: bool,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Register a GitHub repo (clones it under the kansa home).
    Add {
        github: String,
        /// Clone from this URL instead of github.com (e.g. file:///path — local testing).
        #[arg(long)]
        url: Option<String>,
    },
    /// Register a local folder of markdown (no GitHub needed).
    AddLocal { path: PathBuf },
    /// List registered repos.
    List,
    /// Fetch origin and snapshot changed tracked docs.
    Refresh(RepoArg),
}

#[derive(Subcommand)]
enum DocCmd {
    /// List markdown docs on the default branch (tracked ones marked).
    List(RepoArg),
    /// Track a doc as an HLD (builds its first snapshot).
    Track {
        #[command(flatten)]
        repo: RepoArg,
        path: String,
    },
    Untrack {
        #[command(flatten)]
        repo: RepoArg,
        path: String,
    },
}

#[derive(Subcommand)]
enum ClassifyCmd {
    /// Mark spans non-normative (`c`).
    NonNormative(DocArg),
    /// Clear a non-normative mark.
    Unmark(DocArg),
    /// Create a requirement anchored to spans (`r`).
    Req {
        #[command(flatten)]
        doc: DocArg,
        #[arg(long, short = 's')]
        statement: String,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long, value_enum)]
        pattern: Option<PatternArg>,
        #[arg(long)]
        owner: Option<String>,
    },
    /// Attach spans to an existing requirement (`r` → attach).
    Attach {
        #[command(flatten)]
        doc: DocArg,
        #[arg(long)]
        slug: String,
    },
    /// Flag spans as a question (`q`).
    Question {
        #[command(flatten)]
        doc: DocArg,
        #[arg(long, short = 'q')]
        quote: String,
        #[arg(long, value_enum, default_value = "m")]
        materiality: LevelArg,
        /// Readings as key=text (repeatable).
        #[arg(long = "reading")]
        readings: Vec<String>,
        #[arg(long)]
        default: Option<String>,
        /// Requirement slugs the answer affects.
        #[arg(long = "affects")]
        affects: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ReqCmd {
    /// Set status (retire requires --reason).
    Status {
        #[command(flatten)]
        repo: RepoArg,
        slug: String,
        #[arg(value_enum)]
        status: StatusArg,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Bump to a new rev with a new statement (meaning change).
    Bump {
        #[command(flatten)]
        repo: RepoArg,
        slug: String,
        #[arg(long, short = 's')]
        statement: String,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Hard-delete a mistake: only extracted, single-rev, question-free requirements created
    /// after the last export. Anything older: retire with a reason instead.
    Delete {
        #[command(flatten)]
        repo: RepoArg,
        slug: String,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Attach a free-text note (commentary; does not bump the rev, not exported).
    Note {
        #[command(flatten)]
        repo: RepoArg,
        slug: String,
        #[arg(long, short = 'm')]
        text: String,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Remove note N (0-based, oldest first) from a requirement.
    NoteRm {
        #[command(flatten)]
        repo: RepoArg,
        slug: String,
        index: usize,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
}

#[derive(Subcommand)]
enum GroupCmd {
    /// List groups with their rollups.
    List {
        #[command(flatten)]
        repo: RepoArg,
    },
    /// Create a group (the slug is derived from the title).
    Add {
        #[command(flatten)]
        repo: RepoArg,
        title: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Add requirements to a group.
    Assign {
        #[command(flatten)]
        repo: RepoArg,
        group: String,
        #[arg(required = true)]
        reqs: Vec<String>,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Remove requirements from a group.
    Unassign {
        #[command(flatten)]
        repo: RepoArg,
        group: String,
        #[arg(required = true)]
        reqs: Vec<String>,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
    /// Retitle a group or set its description (`--description ""` clears it).
    Update {
        #[command(flatten)]
        repo: RepoArg,
        group: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, env = "KANSA_USER", default_value_t = whoami())]
        by: String,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum PatternArg {
    Ubiquitous,
    EventDriven,
    StateDriven,
    Unwanted,
    Optional,
    Complex,
}
impl From<PatternArg> for Pattern {
    fn from(p: PatternArg) -> Self {
        match p {
            PatternArg::Ubiquitous => Pattern::Ubiquitous,
            PatternArg::EventDriven => Pattern::EventDriven,
            PatternArg::StateDriven => Pattern::StateDriven,
            PatternArg::Unwanted => Pattern::Unwanted,
            PatternArg::Optional => Pattern::Optional,
            PatternArg::Complex => Pattern::Complex,
        }
    }
}
#[derive(Clone, Copy, clap::ValueEnum)]
enum LevelArg {
    H,
    M,
    L,
}
impl From<LevelArg> for Level {
    fn from(l: LevelArg) -> Self {
        match l {
            LevelArg::H => Level::H,
            LevelArg::M => Level::M,
            LevelArg::L => Level::L,
        }
    }
}
#[derive(Clone, Copy, clap::ValueEnum)]
enum StatusArg {
    Extracted,
    Assumed,
    Confirmed,
    Disputed,
    Retired,
}
impl From<StatusArg> for Status {
    fn from(s: StatusArg) -> Self {
        match s {
            StatusArg::Extracted => Status::Extracted,
            StatusArg::Assumed => Status::Assumed,
            StatusArg::Confirmed => Status::Confirmed,
            StatusArg::Disputed => Status::Disputed,
            StatusArg::Retired => Status::Retired,
        }
    }
}

fn open(r: &RepoArg) -> Result<Workspace> {
    Workspace::open_github(&r.repo).map_err(|e| anyhow!("{e}\nhint: `kansa repo add {}`", r.repo))
}

fn ctx(ws: &Workspace) -> Result<Context> {
    ws.default_context()
}

fn out<T: serde::Serialize>(json: bool, v: &T, human: impl FnOnce(&T) -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(v)?);
    } else {
        println!("{}", human(v));
    }
    Ok(())
}

/// Frontend build output, embedded at compile time (build.rs creates the dir so fresh clones
/// compile; an empty dir just means `kansa ui` reports the assets are missing).
static UI_ASSETS: include_dir::Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../app/dist");

fn respond_json(req: tiny_http::Request, code: u16, body: String) {
    respond_raw(req, code, body.into_bytes(), "application/json");
}

fn respond_raw(req: tiny_http::Request, code: u16, body: Vec<u8>, ctype: &str) {
    let mut r = tiny_http::Response::from_data(body).with_status_code(code);
    for (k, v) in [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Headers", "content-type, x-kansa-token"),
        ("Access-Control-Allow-Methods", "POST, GET, OPTIONS"),
        ("Content-Type", ctype),
    ] {
        r.add_header(tiny_http::Header::from_bytes(k, v).unwrap());
    }
    let _ = req.respond(r);
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// Serve the command surface (and, with `ui`, the embedded frontend) on 127.0.0.1.
/// `token`: when set, every /api call must carry it in `x-kansa-token` — static assets stay
/// open (they contain no state; the token arrives via the URL the browser was opened with).
/// `port: None` — prefer the stable default (a stable origin keeps the browser's app identity,
/// icons and bookmarks working across launches) but fall back to any free port rather than fail.
fn serve(port: Option<u16>, token: Option<String>, ui: bool, open: Open) -> Result<()> {
    const DEFAULT_UI_PORT: u16 = 1424;
    let server = match port {
        Some(p) => tiny_http::Server::http(("127.0.0.1", p))
            .map_err(|e| anyhow!("bind 127.0.0.1:{p}: {e}"))?,
        None => tiny_http::Server::http(("127.0.0.1", DEFAULT_UI_PORT)).or_else(|_| {
            tiny_http::Server::http(("127.0.0.1", 0)).map_err(|e| anyhow!("bind 127.0.0.1: {e}"))
        })?,
    };
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port.unwrap_or(DEFAULT_UI_PORT));
    if ui {
        let url = format!(
            "http://127.0.0.1:{bound}/?token={}",
            token.as_deref().unwrap_or("")
        );
        eprintln!("kansa ui on {url}");
        if UI_ASSETS.get_file("index.html").is_none() {
            eprintln!("warning: no embedded UI assets — build with `cd app && npm run build`, then rebuild kansa");
        }
        match open {
            Open::AppWindow => {
                if !open_app_window(&url) {
                    open_browser(&url);
                }
            }
            Open::Browser => open_browser(&url),
            Open::None => {}
        }
    } else {
        eprintln!("kansa dev bridge on http://127.0.0.1:{bound}/api/<command>  (POST JSON args)");
    }
    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        if *req.method() == tiny_http::Method::Options {
            respond_json(req, 204, String::new());
            continue;
        }
        if let Some(rest) = url.strip_prefix("/api") {
            if let Some(t) = &token {
                let ok = req
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("x-kansa-token") && h.value.as_str() == t);
                if !ok {
                    respond_json(req, 401, "{\"error\":\"missing or bad token\"}".into());
                    continue;
                }
            }
            let name = rest.trim_start_matches('/').trim_end_matches('/').to_string();
            if name.is_empty() {
                respond_json(req, 200, serde_json::to_string(kansa_core::api::COMMANDS)?);
                continue;
            }
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            let args: serde_json::Value = if body.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&body).unwrap_or(serde_json::json!({}))
            };
            match kansa_core::api::call(&name, &args) {
                Ok(v) => respond_json(req, 200, serde_json::to_string(&v)?),
                Err(e) => respond_json(
                    req,
                    400,
                    serde_json::to_string(&serde_json::json!({"error": format!("{e:#}")}))?,
                ),
            }
            continue;
        }
        if ui && *req.method() == tiny_http::Method::Get {
            let path = url.split('?').next().unwrap_or("/").trim_start_matches('/');
            let path = if path.is_empty() { "index.html" } else { path };
            match UI_ASSETS.get_file(path) {
                Some(f) => respond_raw(req, 200, f.contents().to_vec(), content_type(path)),
                // SPA-style fallback for extensionless paths; real missing assets 404.
                None if !path.contains('.') => match UI_ASSETS.get_file("index.html") {
                    Some(f) => respond_raw(req, 200, f.contents().to_vec(), content_type("index.html")),
                    None => respond_raw(req, 503, MISSING_UI.as_bytes().to_vec(), "text/html; charset=utf-8"),
                },
                None => respond_json(req, 404, "{\"error\":\"not found\"}".into()),
            }
            continue;
        }
        respond_json(req, 404, "{\"error\":\"not found\"}".into());
    }
    Ok(())
}

const MISSING_UI: &str = "<!doctype html><meta charset=utf-8><title>kansa</title><body style=\"font:14px system-ui;padding:40px\"><h1>UI assets not embedded</h1><p>This binary was built without the frontend. Build it with:</p><pre>cd app &amp;&amp; npm install &amp;&amp; npm run build\ncargo build --release -p kansa-cli</pre>";

#[derive(Clone, Copy, PartialEq)]
enum Open {
    /// Chromium `--app` window (chromeless, own taskbar entry); browser tab if none found.
    AppWindow,
    Browser,
    None,
}

/// Best-effort standalone window via a Chromium browser's `--app` mode. Returns false when no
/// candidate browser could be launched — the caller falls back to a normal tab.
fn open_app_window(url: &str) -> bool {
    use std::process::Command;
    let app_arg = format!("--app={url}");
    match std::env::consts::OS {
        "macos" => {
            // `open -na <App> --args --app=…` — a running instance picks the args up too.
            for app in ["Google Chrome", "Microsoft Edge", "Chromium", "Brave Browser"] {
                let ok = Command::new("open")
                    .args(["-na", app, "--args", &app_arg])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok {
                    return true;
                }
            }
            false
        }
        "windows" => {
            // Browsers are not on PATH; `start` resolves them via the App Paths registry.
            for exe in ["msedge", "chrome"] {
                let ok = Command::new("cmd")
                    .args(["/c", "start", "", exe, &app_arg])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok {
                    return true;
                }
            }
            false
        }
        _ => {
            for exe in ["google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"] {
                if Command::new(exe).arg(&app_arg).spawn().is_ok() {
                    return true;
                }
            }
            false
        }
    }
}

fn open_browser(url: &str) {
    use std::process::Command;
    let r = match std::env::consts::OS {
        "macos" => Command::new("open").arg(url).spawn(),
        "windows" => Command::new("cmd").args(["/c", "start", "", url]).spawn(),
        _ => Command::new("xdg-open").arg(url).spawn(),
    };
    if r.is_err() {
        eprintln!("open this URL in your browser: {url}");
    }
}

fn session_token() -> Result<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).map_err(|e| anyhow!("random token: {e}"))?;
    Ok(hex::encode(buf))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let json = cli.json;
    match cli.cmd {
        Cmd::Serve { port } => serve(Some(port), None, false, Open::None),
        Cmd::Ui { port, no_open, browser } => {
            let token = session_token()?;
            let open = if no_open { Open::None } else if browser { Open::Browser } else { Open::AppWindow };
            serve(port, Some(token), true, open)
        }
        Cmd::Repo { cmd } => match cmd {
            RepoCmd::Add { github, url } => {
                let ws = match url {
                    Some(u) => {
                        ops::register_repo_from_url(&kansa_core::store::kansa_home()?, &github, &u)?
                    }
                    None => ops::register_repo(&github)?,
                };
                let cfg = ws.store.repo()?;
                out(json, &cfg, |c| {
                    format!(
                        "registered {} (default branch {})\nstore: {}",
                        c.github,
                        c.default_branch,
                        ws.store.root().display()
                    )
                })
            }
            RepoCmd::AddLocal { path } => {
                let ws = ops::register_local(&path)?;
                let cfg = ws.store.repo()?;
                out(json, &cfg, |c| {
                    format!(
                        "registered local folder {} as {}\nstore: {}",
                        c.source_dir.clone().unwrap_or_default(),
                        c.github,
                        ws.store.root().display()
                    )
                })
            }
            RepoCmd::List => {
                let repos = ops::list_registered()?;
                out(json, &repos, |rs| {
                    if rs.is_empty() {
                        return "no repos registered — try `kansa repo add owner/name`".into();
                    }
                    rs.iter()
                        .map(|r| {
                            format!(
                                "{}  [{}]  {} tracked doc(s)",
                                r.github,
                                r.default_branch,
                                r.tracked.len()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            RepoCmd::Refresh(r) => {
                let ws = open(&r)?;
                let changes = ops::refresh(&ws)?;
                out(json, &changes, |cs| {
                    if cs.is_empty() {
                        "up to date".into()
                    } else {
                        cs.iter()
                            .map(|c| match (&c.to, c.advanced) {
                                (None, _) => format!("{}: removed upstream", c.doc),
                                (Some(_), true) => {
                                    format!("{}: updated (no classification yet — advanced)", c.doc)
                                }
                                (Some(_), false) => {
                                    format!("{}: changed upstream — reconciliation needed", c.doc)
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                })
            }
        },
        Cmd::Doc { cmd } => match cmd {
            DocCmd::List(r) => {
                let ws = open(&r)?;
                let docs = ops::list_docs(&ws)?;
                out(json, &docs, |ds| {
                    ds.iter()
                        .map(|d| format!("{} {}", if d.tracked { "*" } else { " " }, d.path))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            DocCmd::Track { repo, path } => {
                let ws = open(&repo)?;
                let snap = ops::track_doc(&ws, &path)?;
                out(json, &snap, |s| {
                    format!(
                        "tracking {} — {} spans @ {}",
                        s.doc,
                        s.spans.len(),
                        &s.sha[..8]
                    )
                })
            }
            DocCmd::Untrack { repo, path } => {
                let ws = open(&repo)?;
                ops::untrack_doc(&ws, &path)?;
                println!("untracked {path}");
                Ok(())
            }
        },
        Cmd::Status(r) => {
            let ws = open(&r)?;
            let s = ops::status(&ws)?;
            out(json, &s, |s| {
                let mut lines = vec![format!("{} [{}]", s.github, s.default_branch)];
                for d in &s.docs {
                    match &d.meter {
                        Some(m) => lines.push(format!(
                            "  {:<40} {}/{} classified · residue {} · mapped {} · non-normative {} · questions {} · round {}",
                            d.doc,
                            m.classified,
                            m.total,
                            m.residue,
                            m.mapped,
                            m.non_normative,
                            m.questioned,
                            d.open_round.map(|n| format!("#{n} open")).unwrap_or_else(|| format!("{} closed", d.rounds_closed))
                        )),
                        None => lines.push(format!("  {:<40} (no snapshot)", d.doc)),
                    }
                }
                let r = &s.rollup;
                lines.push(format!(
                    "  reqs: {}  · open questions {} · groups {}{}",
                    r.reqs_by_status
                        .iter()
                        .map(|(k, v)| format!("{k} {v}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    r.open_questions,
                    r.groups,
                    if r.unexported_changes {
                        " · UNEXPORTED CHANGES"
                    } else {
                        ""
                    }
                ));
                lines.join("\n")
            })
        }
        Cmd::Export {
            repo,
            out: out_dir,
            format,
            no_validate,
        } => {
            if format != "reqtrace" {
                return Err(anyhow!(
                    "unknown export format `{format}` (only `reqtrace`)"
                ));
            }
            let ws = open(&repo)?;
            let res = ops::export(&ws, out_dir.as_deref())?;
            println!(
                "wrote {} ({} items)\nwrote {} ({} entries)",
                res.inventory.display(),
                res.items,
                res.exceptions.display(),
                res.exception_count
            );
            if !no_validate {
                match kansa_core::export::find_reqtrace() {
                    Some(bin) => {
                        let dir = res.inventory.parent().unwrap();
                        let (code, text) = kansa_core::export::run_reqtrace_validate(&bin, dir)?;
                        print!("reqtrace validate: {}", text);
                        if code != 0 {
                            std::process::exit(code);
                        }
                    }
                    None => println!("(reqtrace not found on PATH — skipping validate)"),
                }
            }
            Ok(())
        }
        Cmd::Spans { repo, doc, residue } => {
            let ws = open(&repo)?;
            let c = ctx(&ws)?;
            let view = ops::doc_view(&ws, &c, &doc)?;
            let items: Vec<_> = view
                .snapshot
                .spans
                .iter()
                .zip(view.coverage.spans.iter())
                .filter(|(_, (_, st))| !residue || st.state == kansa_core::coverage::SpanState::Unclassified)
                .map(|(s, (_, st))| {
                    serde_json::json!({"id": s.id, "ord": s.ord, "block": s.block, "state": st.state, "reqs": st.reqs, "questions": st.questions, "structural": st.structural, "text": s.text})
                })
                .collect();
            let meter = view.coverage.meter.clone();
            out(
                json,
                &serde_json::json!({"meter": meter, "spans": items}),
                |_| {
                    let mut lines: Vec<String> = items
                        .iter()
                        .map(|i| {
                            let mark = match i["state"].as_str().unwrap_or("") {
                                "unclassified" => "·",
                                "mapped" => "R",
                                "non-normative" => "-",
                                "question" => "?",
                                _ => " ",
                            };
                            let text = i["text"].as_str().unwrap_or("");
                            let text = if text.chars().count() > 96 {
                                format!("{}…", text.chars().take(95).collect::<String>())
                            } else {
                                text.to_string()
                            };
                            format!(
                                "{} {:<14} {:<7} {}",
                                mark,
                                i["id"].as_str().unwrap_or(""),
                                i["block"].as_str().unwrap_or(""),
                                text
                            )
                        })
                        .collect();
                    lines.push(format!(
                        "{}/{} classified · residue {}",
                        meter.classified, meter.total, meter.residue
                    ));
                    lines.join("\n")
                },
            )
        }
        Cmd::Classify { cmd } => match cmd {
            ClassifyCmd::NonNormative(d) => {
                let ws = open(&d.repo)?;
                let c = ctx(&ws)?;
                ops::mark_non_normative(&ws, &c, &d.doc, &d.spans, &d.by)?;
                println!("marked {} span(s) non-normative", d.spans.len());
                Ok(())
            }
            ClassifyCmd::Unmark(d) => {
                let ws = open(&d.repo)?;
                let c = ctx(&ws)?;
                ops::unmark(&ws, &c, &d.doc, &d.spans)?;
                println!("unmarked {} span(s)", d.spans.len());
                Ok(())
            }
            ClassifyCmd::Req {
                doc: d,
                statement,
                slug,
                pattern,
                owner,
            } => {
                let ws = open(&d.repo)?;
                let c = ctx(&ws)?;
                let r = ops::create_req(
                    &ws,
                    &c,
                    &d.doc,
                    &d.spans,
                    ops::NewReq {
                        statement: &statement,
                        slug: slug.as_deref(),
                        pattern: pattern.map(Into::into),
                        rating: None,
                        owner: owner.as_deref(),
                    },
                    &d.by,
                )?;
                out(json, &r, |r| {
                    format!("created {} anchored to {} span(s)", r.id, r.anchors.len())
                })
            }
            ClassifyCmd::Attach { doc: d, slug } => {
                let ws = open(&d.repo)?;
                let c = ctx(&ws)?;
                let r = ops::attach_req(&ws, &c, &d.doc, &d.spans, &slug, &d.by)?;
                out(json, &r, |r| {
                    format!("{} now anchored to {} span(s)", r.id, r.anchors.len())
                })
            }
            ClassifyCmd::Question {
                doc: d,
                quote,
                materiality,
                readings,
                default,
                affects,
            } => {
                let ws = open(&d.repo)?;
                let c = ctx(&ws)?;
                let readings = readings
                    .iter()
                    .map(|r| {
                        r.split_once('=')
                            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                            .ok_or_else(|| anyhow!("reading must be key=text: `{r}`"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let affects = affects
                    .iter()
                    .map(|s| kansa_core::id::Id::new("req", s, 1))
                    .collect::<Result<Vec<_>, _>>()?;
                let q = ops::flag_question(
                    &ws,
                    &c,
                    &d.doc,
                    &d.spans,
                    ops::NewQuestion {
                        quote: &quote,
                        materiality: materiality.into(),
                        readings,
                        default,
                        affects,
                        slug: None,
                    },
                    &d.by,
                )?;
                out(json, &q, |q| {
                    format!("raised {} on {} span(s)", q.id, q.anchors.len())
                })
            }
        },
        Cmd::Close { repo, doc } => {
            let ws = open(&repo)?;
            let c = ctx(&ws)?;
            let r = ops::close_round(&ws, &c, &doc)?;
            out(json, &r, |r| {
                let s = r.summary.clone().unwrap_or_default();
                format!(
                    "closed round #{} for {} — created {} · changed {} · retired {}",
                    r.n,
                    r.doc,
                    s.created.len(),
                    s.changed.len(),
                    s.retired.len()
                )
            })
        }
        Cmd::Req { cmd } => match cmd {
            ReqCmd::Status {
                repo,
                slug,
                status,
                reason,
                by,
            } => {
                let ws = open(&repo)?;
                let r = ops::update_req(
                    &ws,
                    &slug,
                    ops::ReqPatch {
                        statement: None,
                        pattern: None,
                        status: Some(status.into()),
                        rating: None,
                        owner: None,
                        reason: reason.as_deref(),
                    },
                    &by,
                )?;
                out(json, &r, |r| format!("{} → {}", r.id, r.status.as_str()))
            }
            ReqCmd::Bump {
                repo,
                slug,
                statement,
                by,
            } => {
                let ws = open(&repo)?;
                let r = ops::bump_req(&ws, &slug, &statement, &by)?;
                out(json, &r, |r| format!("bumped to {}", r.id))
            }
            ReqCmd::Delete { repo, slug, by } => {
                let ws = open(&repo)?;
                ops::delete_req(&ws, &slug, &by)?;
                out(json, &serde_json::json!({"deleted": slug}), |_| {
                    format!("deleted req~{slug} — its sentences are residue again")
                })
            }
            ReqCmd::Note {
                repo,
                slug,
                text,
                by,
            } => {
                let ws = open(&repo)?;
                let r = ops::add_req_note(&ws, &slug, &text, &by)?;
                out(json, &r, |r| {
                    format!("{} now has {} note(s)", r.id, r.notes.len())
                })
            }
            ReqCmd::NoteRm {
                repo,
                slug,
                index,
                by,
            } => {
                let ws = open(&repo)?;
                let r = ops::delete_req_note(&ws, &slug, index, &by)?;
                out(json, &r, |r| {
                    format!("{} now has {} note(s)", r.id, r.notes.len())
                })
            }
        },
        Cmd::Group { cmd } => match cmd {
            GroupCmd::List { repo } => {
                let ws = open(&repo)?;
                let gs = ops::group_rollups(&ws)?;
                out(json, &gs, |gs| {
                    if gs.is_empty() {
                        return "no groups".into();
                    }
                    gs.iter()
                        .map(|g| {
                            let by_status = g
                                .members_by_status
                                .iter()
                                .filter(|(_, n)| **n > 0)
                                .map(|(s, n)| format!("{s} {n}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let findings = if g.findings.is_empty() {
                                String::new()
                            } else {
                                format!("  ⚠ {} finding(s)", g.findings.len())
                            };
                            format!(
                                "{:<24} {:>3} member(s)  [{}]{}",
                                g.group.id.slug,
                                g.group.members.len(),
                                by_status,
                                findings
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(
                            "
",
                        )
                })
            }
            GroupCmd::Add {
                repo,
                title,
                description,
                by,
            } => {
                let ws = open(&repo)?;
                let g = ops::create_group(&ws, &title, description.as_deref(), &by)?;
                out(json, &g, |g| format!("created {}", g.id))
            }
            GroupCmd::Assign {
                repo,
                group,
                reqs,
                by,
            } => {
                let ws = open(&repo)?;
                let g = ops::assign_group(&ws, &group, &reqs, &by)?;
                out(json, &g, |g| {
                    format!("{} now has {} member(s)", g.id, g.members.len())
                })
            }
            GroupCmd::Unassign {
                repo,
                group,
                reqs,
                by,
            } => {
                let ws = open(&repo)?;
                let g = ops::unassign_group(&ws, &group, &reqs, &by)?;
                out(json, &g, |g| {
                    format!("{} now has {} member(s)", g.id, g.members.len())
                })
            }
            GroupCmd::Update {
                repo,
                group,
                title,
                description,
                by,
            } => {
                let ws = open(&repo)?;
                let g = ops::update_group(
                    &ws,
                    &group,
                    title.as_deref(),
                    description
                        .as_deref()
                        .map(|d| if d.is_empty() { None } else { Some(d) }),
                    &by,
                )?;
                out(json, &g, |g| format!("{} — {}", g.id, g.title))
            }
        },
    }
}
