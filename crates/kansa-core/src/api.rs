//! JSON command surface shared by the Tauri app and the `kansa serve` dev bridge.
//! One dispatch table → identical behaviour in the webview and in a browser (`ui~core-parity~1`).

use crate::id::Id;
use crate::model::{Context, Level, Pattern, Rating, Status};
use crate::ops::{self, Workspace};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};

fn arg<'a, T: Deserialize<'a>>(args: &'a Value, key: &str) -> Result<T> {
    let v = args
        .get(key)
        .ok_or_else(|| anyhow!("missing argument `{key}`"))?;
    T::deserialize(v).map_err(|e| anyhow!("argument `{key}`: {e}"))
}

fn arg_opt<'a, T: Deserialize<'a>>(args: &'a Value, key: &str) -> Result<Option<T>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => T::deserialize(v)
            .map(Some)
            .map_err(|e| anyhow!("argument `{key}`: {e}")),
    }
}

fn ws(args: &Value) -> Result<Workspace> {
    let github: String = arg(args, "github")?;
    Workspace::open_github(&github)
}

fn ctx_of(ws: &Workspace, args: &Value) -> Result<Context> {
    match arg_opt::<Context>(args, "context")? {
        Some(c) => Ok(c),
        None => ws.default_context(),
    }
}

fn by(args: &Value) -> String {
    arg_opt::<String>(args, "by")
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "user".into())
        })
}

fn j<T: serde::Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

pub const COMMANDS: &[&str] = &[
    "kansa_home",
    "list_repos",
    "register_repo",
    "list_docs",
    "track_doc",
    "untrack_doc",
    "refresh_repo",
    "repo_status",
    "doc_view",
    "mark_non_normative",
    "unmark",
    "create_req",
    "attach_req",
    "detach_req",
    "update_req",
    "bump_req",
    "flag_question",
    "close_round",
    "list_reqs",
    "export",
    "inventory",
    "list_groups",
    "create_group",
    "assign_group",
    "unassign_group",
    "update_group",
    "decide_verdict",
    "confirm_reconciliation",
    "list_questions",
    "answer_question",
    "resolve_held_answer",
    "withdraw_question",
    "list_prs",
    "open_pr",
    "rounds",
];

/// Dispatch a command by name with JSON args. Blocking; callers run it off the UI thread.
pub fn call(name: &str, args: &Value) -> Result<Value> {
    match name {
        "kansa_home" => j(crate::store::kansa_home()?.to_string_lossy().into_owned()),
        "list_repos" => j(ops::list_registered()?),
        "register_repo" => {
            let github: String = arg(args, "github")?;
            let ws = ops::register_repo(&github)?;
            let cfg = ws.store.repo()?;
            j(ops::RepoSummary {
                store_dir: ws.store.root().to_string_lossy().into_owned(),
                github: cfg.github,
                default_branch: cfg.default_branch,
                tracked: cfg.tracked.into_iter().map(|t| t.path).collect(),
                last_fetch: cfg.last_fetch.map(|t| t.to_string()),
            })
        }
        "list_docs" => j(ops::list_docs(&ws(args)?)?),
        "track_doc" => {
            let s = ops::track_doc(&ws(args)?, &arg::<String>(args, "path")?)?;
            j(json!({"doc": s.doc, "sha": s.sha, "spans": s.spans.len()}))
        }
        "untrack_doc" => j(ops::untrack_doc(&ws(args)?, &arg::<String>(args, "path")?)?),
        "refresh_repo" => j(ops::refresh(&ws(args)?)?),
        "repo_status" => j(ops::status(&ws(args)?)?),
        "doc_view" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            j(ops::doc_view(&w, &c, &arg::<String>(args, "doc")?)?)
        }
        "mark_non_normative" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            ops::mark_non_normative(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &arg::<Vec<String>>(args, "spans")?,
                &by(args),
            )?;
            j(())
        }
        "unmark" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            ops::unmark(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &arg::<Vec<String>>(args, "spans")?,
            )?;
            j(())
        }
        "create_req" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            let statement: String = arg(args, "statement")?;
            let slug: Option<String> = arg_opt(args, "slug")?;
            let owner: Option<String> = arg_opt(args, "owner")?;
            let r = ops::create_req(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &arg::<Vec<String>>(args, "spans")?,
                ops::NewReq {
                    statement: &statement,
                    slug: slug.as_deref(),
                    pattern: arg_opt::<Pattern>(args, "pattern")?,
                    rating: arg_opt::<Rating>(args, "rating")?,
                    owner: owner.as_deref(),
                },
                &by(args),
            )?;
            j(r)
        }
        "attach_req" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            j(ops::attach_req(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &arg::<Vec<String>>(args, "spans")?,
                &arg::<String>(args, "slug")?,
                &by(args),
            )?)
        }
        "detach_req" => {
            let w = ws(args)?;
            ops::detach_req(
                &w,
                &arg::<String>(args, "doc")?,
                &arg::<Vec<String>>(args, "spans")?,
                &arg::<String>(args, "slug")?,
                &by(args),
            )?;
            j(())
        }
        "update_req" => {
            let w = ws(args)?;
            let statement: Option<String> = arg_opt(args, "statement")?;
            let rating: Option<Option<Rating>> = if args.get("rating").is_some() {
                Some(arg_opt(args, "rating")?)
            } else {
                None
            };
            let pattern: Option<Option<Pattern>> = if args.get("pattern").is_some() {
                Some(arg_opt(args, "pattern")?)
            } else {
                None
            };
            let reason: Option<String> = arg_opt(args, "reason")?;
            j(ops::update_req(
                &w,
                &arg::<String>(args, "slug")?,
                ops::ReqPatch {
                    statement: statement.as_deref(),
                    pattern,
                    status: arg_opt::<Status>(args, "status")?,
                    rating,
                    owner: if args.get("owner").is_some() {
                        Some(owner_ref(args))
                    } else {
                        None
                    },
                    reason: reason.as_deref(),
                },
                &by(args),
            )?)
        }
        "bump_req" => {
            let w = ws(args)?;
            j(ops::bump_req(
                &w,
                &arg::<String>(args, "slug")?,
                &arg::<String>(args, "statement")?,
                &by(args),
            )?)
        }
        "flag_question" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            let quote: String = arg(args, "quote")?;
            let slug: Option<String> = arg_opt(args, "slug")?;
            #[derive(Deserialize)]
            struct R {
                key: String,
                text: String,
            }
            let readings: Vec<R> = arg_opt(args, "readings")?.unwrap_or_default();
            let affects: Vec<String> = arg_opt(args, "affects")?.unwrap_or_default();
            let affects = affects
                .iter()
                .map(|s| Id::new("req", s, 1))
                .collect::<Result<Vec<_>, _>>()?;
            j(ops::flag_question(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &arg::<Vec<String>>(args, "spans")?,
                ops::NewQuestion {
                    quote: &quote,
                    materiality: arg_opt::<Level>(args, "materiality")?.unwrap_or(Level::M),
                    readings: readings.into_iter().map(|r| (r.key, r.text)).collect(),
                    default: arg_opt(args, "default")?,
                    affects,
                    slug: slug.as_deref(),
                },
                &by(args),
            )?)
        }
        "close_round" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            j(ops::close_round(&w, &c, &arg::<String>(args, "doc")?)?)
        }
        "list_reqs" => {
            let w = ws(args)?;
            j(w.store.current_reqs()?)
        }
        "export" => {
            let w = ws(args)?;
            let out: Option<String> = arg_opt(args, "out")?;
            let res = ops::export(&w, out.as_deref().map(std::path::Path::new))?;
            let validate = if arg_opt::<bool>(args, "validate")?.unwrap_or(true) {
                match crate::export::find_reqtrace() {
                    Some(bin) => {
                        let (code, text) = crate::export::run_reqtrace_validate(
                            &bin,
                            res.inventory.parent().unwrap(),
                        )?;
                        Some(json!({"code": code, "output": text}))
                    }
                    None => None,
                }
            } else {
                None
            };
            j(json!({
                "inventory": res.inventory.to_string_lossy(),
                "exceptions": res.exceptions.to_string_lossy(),
                "items": res.items,
                "exception_count": res.exception_count,
                "validate": validate,
            }))
        }
        "inventory" => j(ops::inventory(&ws(args)?)?),
        "list_groups" => j(ops::group_rollups(&ws(args)?)?),
        "create_group" => {
            let w = ws(args)?;
            let desc: Option<String> = arg_opt(args, "description")?;
            j(ops::create_group(
                &w,
                &arg::<String>(args, "title")?,
                desc.as_deref(),
                &by(args),
            )?)
        }
        "assign_group" => {
            let w = ws(args)?;
            j(ops::assign_group(
                &w,
                &arg::<String>(args, "group")?,
                &arg::<Vec<String>>(args, "reqs")?,
                &by(args),
            )?)
        }
        "unassign_group" => {
            let w = ws(args)?;
            j(ops::unassign_group(
                &w,
                &arg::<String>(args, "group")?,
                &arg::<Vec<String>>(args, "reqs")?,
                &by(args),
            )?)
        }
        "update_group" => {
            let w = ws(args)?;
            let title: Option<String> = arg_opt(args, "title")?;
            let desc: Option<Option<String>> = if args.get("description").is_some() {
                Some(arg_opt(args, "description")?)
            } else {
                None
            };
            j(ops::update_group(
                &w,
                &arg::<String>(args, "group")?,
                title.as_deref(),
                desc.as_ref().map(|d| d.as_deref()),
                &by(args),
            )?)
        }
        "decide_verdict" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            let decision: crate::reconcile::Decision = arg(args, "decision")?;
            j(ops::decide_verdict(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &arg::<String>(args, "span")?,
                decision,
            )?)
        }
        "confirm_reconciliation" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            j(ops::confirm_reconciliation(
                &w,
                &c,
                &arg::<String>(args, "doc")?,
                &by(args),
            )?)
        }
        "list_questions" => j(ws(args)?.store.current_qsts()?),
        "answer_question" => {
            let w = ws(args)?;
            let note: Option<String> = arg_opt(args, "note")?;
            j(ops::answer_question(
                &w,
                &arg::<String>(args, "slug")?,
                &arg::<String>(args, "reading")?,
                note.as_deref(),
                &by(args),
            )?)
        }
        "resolve_held_answer" => {
            let w = ws(args)?;
            j(ops::resolve_held_answer(
                &w,
                &arg::<String>(args, "slug")?,
                arg::<bool>(args, "apply")?,
                &by(args),
            )?)
        }
        "withdraw_question" => {
            let w = ws(args)?;
            j(ops::withdraw_question(
                &w,
                &arg::<String>(args, "slug")?,
                &by(args),
            )?)
        }
        "list_prs" => j(ops::list_prs(&ws(args)?)?),
        "open_pr" => {
            let w = ws(args)?;
            j(ops::open_pr(
                &w,
                arg::<u64>(args, "pr")?,
                &arg::<String>(args, "doc")?,
            )?)
        }
        "rounds" => {
            let w = ws(args)?;
            let c = ctx_of(&w, args)?;
            j(w.store.rounds(&c, &arg::<String>(args, "doc")?)?)
        }
        _ => bail!("unknown command `{name}`"),
    }
}

/// `owner: null` clears; `owner: "x"` sets. Returns the inner Option for the patch.
fn owner_ref(args: &Value) -> Option<&str> {
    args.get("owner").and_then(|v| v.as_str())
}
