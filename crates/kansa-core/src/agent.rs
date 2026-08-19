//! Agent pre-fill (spec `ui~agent-prefill~1`, `ui~perf-agent-async~1`, `ui~grp-agent~1`).
//!
//! Runs `claude -p` (or `$KANSA_AGENT_CMD`) over the unclassified spans of a doc in batches,
//! parses JSON proposals, and stores them in a *proposed* state. Nothing touches the inventory
//! until a human accepts each proposal (`by: agent, accepted-by: <user>` in history).
//! Runs on a background thread; progress is written to the store so any client can poll.

use crate::model::{Context, Level, Pattern};
use crate::snapshot::Snapshot;
use crate::store::Store;
use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

pub const BATCH: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Proposed {
    Req {
        statement: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<Pattern>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slug: Option<String>,
        /// Suggested group titles.
        #[serde(default)]
        groups: Vec<String>,
        /// Attach to an existing requirement slug instead of creating.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attach: Option<String>,
    },
    Context,
    Question {
        quote: String,
        #[serde(default)]
        materiality: Option<Level>,
        #[serde(default)]
        readings: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Proposed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Proposal {
    pub id: String,
    pub spans: Vec<String>,
    pub proposed: Proposed,
    pub status: ProposalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Idle,
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub state: JobState,
    pub doc: String,
    pub snapshot: String,
    pub done: usize,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub started: time::OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub finished: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Proposals {
    #[serde(default)]
    pub items: Vec<Proposal>,
}

// ---------- storage ----------

fn proposals_path(store: &Store, ctx: &Context, doc: &str) -> PathBuf {
    store
        .root()
        .join("proposals")
        .join(ctx.key())
        .join(format!("{}.yaml", crate::model::doc_key(doc)))
}
fn job_path(store: &Store, ctx: &Context, doc: &str) -> PathBuf {
    store
        .root()
        .join("jobs")
        .join(ctx.key())
        .join(format!("{}.yaml", crate::model::doc_key(doc)))
}

pub fn load_proposals(store: &Store, ctx: &Context, doc: &str) -> Result<Proposals> {
    let p = proposals_path(store, ctx, doc);
    if !p.exists() {
        return Ok(Proposals::default());
    }
    store.read_yaml(&p)
}
pub fn save_proposals(store: &Store, ctx: &Context, doc: &str, ps: &Proposals) -> Result<()> {
    store.write_yaml(&proposals_path(store, ctx, doc), ps)
}
pub fn load_job(store: &Store, ctx: &Context, doc: &str) -> Result<Option<Job>> {
    let p = job_path(store, ctx, doc);
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(store.read_yaml(&p)?))
}
fn save_job(store: &Store, ctx: &Context, doc: &str, j: &Job) -> Result<()> {
    store.write_yaml(&job_path(store, ctx, doc), j)
}

// ---------- agent invocation ----------

/// Is an agent backend available? (`claude` on PATH, or an override command.)
pub fn agent_available() -> bool {
    if std::env::var("KANSA_AGENT_CMD")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    claude_cmd()
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `claude` is an npm shim (`claude.cmd`) on Windows, which `CreateProcess` won't run
/// directly — go through `cmd /C` there. Elsewhere it's a plain executable.
fn claude_cmd() -> Command {
    if cfg!(windows) {
        let mut c = crate::proc::command("cmd");
        c.args(["/C", "claude"]);
        c
    } else {
        crate::proc::command("claude")
    }
}

pub fn agent_model() -> Option<String> {
    std::env::var("KANSA_AGENT_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Wire shape the model must return, one entry per proposal.
#[derive(Debug, Deserialize)]
struct WireProposal {
    spans: Vec<String>,
    kind: String,
    #[serde(default)]
    statement: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    attach: Option<String>,
    #[serde(default)]
    quote: Option<String>,
    #[serde(default)]
    materiality: Option<String>,
    #[serde(default)]
    readings: Vec<String>,
    #[serde(default)]
    rationale: Option<String>,
}

pub struct BatchInput<'a> {
    pub doc: &'a str,
    pub spans: Vec<(&'a str, &'a str, Option<&'a str>)>, // (id, text, section)
    pub existing_reqs: Vec<(String, String)>,            // (slug, statement)
    pub existing_groups: Vec<String>,
}

pub fn build_prompt(b: &BatchInput<'_>) -> String {
    let mut s = String::new();
    s.push_str("You are helping a requirements analyst classify sentences of a high-level design document into a requirement inventory.\n");
    s.push_str("For EVERY sentence id below, decide exactly one of:\n");
    s.push_str("- \"req\": the sentence states (or clearly implies) a requirement. Rewrite it as ONE EARS-formed statement (Ubiquitous: \"The <system> shall <response>\"; Event-driven: \"When <trigger>, the <system> shall <response>\"; State-driven: \"While <state>, ...\"; Unwanted: \"If <condition>, then the <system> shall ...\"; Optional: \"Where <feature>, ...\"). Give `pattern` (ubiquitous|event-driven|state-driven|unwanted|optional|complex), a short kebab-case `slug` (2-4 words), and 0-2 `groups` (short umbrella labels like \"validation\", \"lockout\"). If it restates an EXISTING requirement listed below, set `attach` to that slug instead of a new statement. Adjacent sentences that form ONE requirement may share a single entry with several `spans`.\n");
    s.push_str("- \"context\": non-normative prose (purpose, rationale, examples, non-goals, commentary, headings-as-prose).\n");
    s.push_str("- \"question\": genuinely ambiguous or undecided text that a PM must resolve before it can be a requirement. Give `quote`, `materiality` (H|M|L) and 2-3 candidate `readings`.\n");
    s.push_str("Return ONLY a JSON array (no prose, no code fences). Each element: {\"spans\":[ids], \"kind\":\"req\"|\"context\"|\"question\", \"statement\"?, \"pattern\"?, \"slug\"?, \"groups\"?, \"attach\"?, \"quote\"?, \"materiality\"?, \"readings\"?, \"rationale\"?}. Every input id must appear in exactly one element.\n\n");
    if !b.existing_reqs.is_empty() {
        s.push_str("Existing requirements (slug: statement):\n");
        for (slug, st) in &b.existing_reqs {
            s.push_str(&format!("- {slug}: {st}\n"));
        }
        s.push('\n');
    }
    if !b.existing_groups.is_empty() {
        s.push_str(&format!(
            "Existing groups: {}\n\n",
            b.existing_groups.join(", ")
        ));
    }
    s.push_str(&format!("Document: {}\nSentences:\n", b.doc));
    let mut last_section: Option<&str> = None;
    for (id, text, section) in &b.spans {
        if *section != last_section {
            if let Some(sec) = section {
                s.push_str(&format!("## {sec}\n"));
            }
            last_section = *section;
        }
        s.push_str(&format!("[{id}] {text}\n"));
    }
    s
}

/// Run the agent once and return raw stdout.
pub fn run_agent(prompt: &str) -> Result<String> {
    let out = if let Ok(cmd) = std::env::var("KANSA_AGENT_CMD").and_then(|c| {
        if c.is_empty() {
            Err(std::env::VarError::NotPresent)
        } else {
            Ok(c)
        }
    }) {
        // Test/override hook: shell command receiving the prompt on stdin.
        let mut child = crate::proc::command(if cfg!(windows) { "cmd" } else { "sh" })
            .args(if cfg!(windows) {
                ["/C", cmd.as_str()]
            } else {
                ["-c", cmd.as_str()]
            })
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawning KANSA_AGENT_CMD")?;
        use std::io::Write;
        child.stdin.take().unwrap().write_all(prompt.as_bytes())?;
        child.wait_with_output()?
    } else {
        // The prompt goes on stdin, never argv: it is tens of KB with newlines and quotes,
        // which Windows' `cmd /C` mangles or truncates (8K line limit) — claude then answers
        // a garbled prompt with exit 0 and no JSON array.
        let mut c = claude_cmd();
        c.arg("-p").arg("--output-format").arg("text");
        if let Some(m) = agent_model() {
            c.arg("--model").arg(m);
        }
        c.env_remove("CLAUDECODE"); // allow nesting when launched from inside Claude Code
        c.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = c
            .spawn()
            .context("running `claude -p` (is Claude Code installed and logged in?)")?;
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin piped")
            .write_all(prompt.as_bytes())
            .context("writing prompt to `claude -p`")?;
        child.wait_with_output()?
    };
    if !out.status.success() {
        bail!(
            "agent exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Extract the JSON array from model output (tolerates prose/fences around it).
pub fn parse_output(raw: &str) -> Result<Vec<Proposal>> {
    // Show what the agent actually said — "no JSON array" alone is undebuggable.
    let snippet = || {
        let t = raw.trim();
        let head: String = t.chars().take(200).collect();
        if head.is_empty() { "(empty output)".into() } else { format!("agent said: {head}…") }
    };
    let start = raw
        .find('[')
        .ok_or_else(|| anyhow!("no JSON array in agent output — {}", snippet()))?;
    let end = raw
        .rfind(']')
        .ok_or_else(|| anyhow!("no JSON array end in agent output — {}", snippet()))?;
    let json = &raw[start..=end];
    let wire: Vec<WireProposal> = serde_json::from_str(json)
        .with_context(|| format!("parsing agent JSON: {}", &json[..json.len().min(200)]))?;
    let mut out = vec![];
    for (i, w) in wire.into_iter().enumerate() {
        if w.spans.is_empty() {
            continue;
        }
        let proposed = match w.kind.as_str() {
            "req" | "requirement" => Proposed::Req {
                statement: w.statement.clone().unwrap_or_default(),
                pattern: w.pattern.as_deref().and_then(parse_pattern),
                slug: w.slug.map(|s| crate::id::slugify(&s, 32)),
                groups: w.groups,
                attach: w.attach,
            },
            "context" | "non-normative" => Proposed::Context,
            "question" => Proposed::Question {
                quote: w.quote.unwrap_or_default(),
                materiality: w.materiality.as_deref().and_then(|m| {
                    match m.to_ascii_uppercase().as_str() {
                        "H" => Some(Level::H),
                        "M" => Some(Level::M),
                        "L" => Some(Level::L),
                        _ => None,
                    }
                }),
                readings: w.readings,
            },
            other => bail!("unknown proposal kind `{other}`"),
        };
        if let Proposed::Req {
            statement, attach, ..
        } = &proposed
        {
            if statement.trim().is_empty() && attach.is_none() {
                continue;
            }
        }
        out.push(Proposal {
            id: format!("p{}-{}", i, &w.spans[0]),
            spans: w.spans,
            proposed,
            status: ProposalStatus::Proposed,
            rationale: w.rationale,
            result: None,
        });
    }
    Ok(out)
}

fn parse_pattern(s: &str) -> Option<Pattern> {
    match s.to_ascii_lowercase().replace('_', "-").as_str() {
        "ubiquitous" => Some(Pattern::Ubiquitous),
        "event-driven" | "event" => Some(Pattern::EventDriven),
        "state-driven" | "state" => Some(Pattern::StateDriven),
        "unwanted" | "unwanted-behaviour" | "unwanted-behavior" => Some(Pattern::Unwanted),
        "optional" => Some(Pattern::Optional),
        "complex" => Some(Pattern::Complex),
        _ => None,
    }
}

/// The blocking pre-fill worker. Called on a background thread by `ops::start_prefill`.
pub fn run_prefill(
    store: Store,
    ctx: Context,
    snap: Snapshot,
    unclassified: Vec<String>,
    existing_reqs: Vec<(String, String)>,
    existing_groups: Vec<String>,
) {
    let doc = snap.doc.clone();
    let total = unclassified.len().div_ceil(BATCH).max(1);
    let mut job = Job {
        state: JobState::Running,
        doc: doc.clone(),
        snapshot: snap.sha.clone(),
        done: 0,
        total,
        error: None,
        model: agent_model(),
        started: crate::model::now(),
        finished: None,
    };
    let _ = save_job(&store, &ctx, &doc, &job);
    let index = snap.index();
    let mut all = load_proposals(&store, &ctx, &doc).unwrap_or_default();
    // drop stale proposals for spans that are no longer unclassified
    let uncl: std::collections::HashSet<&str> = unclassified.iter().map(String::as_str).collect();
    all.items.retain(|p| {
        p.status != ProposalStatus::Proposed || p.spans.iter().all(|s| uncl.contains(s.as_str()))
    });
    let already: std::collections::HashSet<String> = all
        .items
        .iter()
        .filter(|p| p.status == ProposalStatus::Proposed)
        .flat_map(|p| p.spans.clone())
        .collect();
    let todo: Vec<&String> = unclassified
        .iter()
        .filter(|s| !already.contains(*s))
        .collect();
    for chunk in todo.chunks(BATCH) {
        let spans: Vec<(&str, &str, Option<&str>)> = chunk
            .iter()
            .filter_map(|id| {
                index
                    .get(id.as_str())
                    .map(|s| (s.id.as_str(), s.text.as_str(), s.section.as_deref()))
            })
            .collect();
        if spans.is_empty() {
            job.done += 1;
            let _ = save_job(&store, &ctx, &doc, &job);
            continue;
        }
        let prompt = build_prompt(&BatchInput {
            doc: &doc,
            spans,
            existing_reqs: existing_reqs.clone(),
            existing_groups: existing_groups.clone(),
        });
        match run_agent(&prompt).and_then(|raw| parse_output(&raw)) {
            Ok(mut ps) => {
                // keep only proposals whose spans are all in this batch
                let batch: std::collections::HashSet<&str> =
                    chunk.iter().map(|s| s.as_str()).collect();
                ps.retain(|p| p.spans.iter().all(|s| batch.contains(s.as_str())));
                for p in ps.iter_mut() {
                    p.id = format!("{}-{}", &snap.sha[..6], p.id);
                }
                all.items.extend(ps);
                let _ = store
                    .lock()
                    .and_then(|_l| save_proposals(&store, &ctx, &doc, &all));
            }
            Err(e) => {
                job.state = JobState::Error;
                job.error = Some(format!("{e:#}"));
                job.finished = Some(crate::model::now());
                let _ = save_job(&store, &ctx, &doc, &job);
                return;
            }
        }
        job.done += 1;
        let _ = save_job(&store, &ctx, &doc, &job);
    }
    job.state = JobState::Done;
    job.finished = Some(crate::model::now());
    let _ = save_job(&store, &ctx, &doc, &job);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tolerant_output() {
        let raw = r#"Sure! Here you go:
```json
[
 {"spans":["s-1"],"kind":"req","statement":"The system shall X.","pattern":"Ubiquitous","slug":"System X!","groups":["Core"]},
 {"spans":["s-2","s-3"],"kind":"context","rationale":"purpose"},
 {"spans":["s-4"],"kind":"question","quote":"maybe","materiality":"h","readings":["a","b"]},
 {"spans":["s-5"],"kind":"req","attach":"email-format"}
]
```"#;
        let ps = parse_output(raw).unwrap();
        assert_eq!(ps.len(), 4);
        match &ps[0].proposed {
            Proposed::Req {
                statement,
                pattern,
                slug,
                groups,
                ..
            } => {
                assert_eq!(statement, "The system shall X.");
                assert_eq!(*pattern, Some(Pattern::Ubiquitous));
                assert_eq!(slug.as_deref(), Some("x"));
                assert_eq!(groups, &vec!["Core".to_string()]);
            }
            _ => panic!(),
        }
        assert_eq!(ps[1].spans.len(), 2);
        assert!(matches!(
            ps[2].proposed,
            Proposed::Question {
                materiality: Some(Level::H),
                ..
            }
        ));
        assert!(
            matches!(&ps[3].proposed, Proposed::Req { attach: Some(a), .. } if a == "email-format")
        );
        assert!(parse_output("nothing here").is_err());
    }

    #[test]
    fn prompt_lists_every_span() {
        let p = build_prompt(&BatchInput {
            doc: "d.md",
            spans: vec![
                ("s-1", "Hello.", Some("Intro")),
                ("s-2", "World.", Some("Intro")),
                ("s-3", "Bye.", None),
            ],
            existing_reqs: vec![("x".into(), "The x.".into())],
            existing_groups: vec!["G".into()],
        });
        assert!(p.contains("[s-1] Hello."));
        assert!(p.contains("## Intro"));
        assert!(p.contains("- x: The x."));
        assert!(p.contains("Existing groups: G"));
    }
}
