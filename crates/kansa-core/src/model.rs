//! Store objects (spec §0): requirements, questions, groups, rounds, marks, repo config.

use crate::id::Id;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

// ---------- shared ----------

/// One append-only history entry (`obj~store-atomic~1`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct History {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_by: Option<String>,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<serde_yaml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<serde_yaml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl History {
    pub fn new(by: &str, op: &str) -> Self {
        History {
            at: now(),
            by: by.into(),
            accepted_by: None,
            op: op.into(),
            from: None,
            to: None,
            note: None,
        }
    }
    pub fn change<T: Serialize>(mut self, from: Option<&T>, to: Option<&T>) -> Self {
        self.from = from.and_then(|v| serde_yaml::to_value(v).ok());
        self.to = to.and_then(|v| serde_yaml::to_value(v).ok());
        self
    }
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// `{doc, span}` — resolves through the doc's current snapshot (`obj~anchor~1`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Anchor {
    pub doc: String,
    pub span: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    H,
    M,
    L,
}

/// `[value, risk]` — serialized as a two-element list to match reqtrace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rating {
    pub value: Level,
    pub risk: Level,
}

impl Serialize for Rating {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.value, self.risk].serialize(s)
    }
}
impl<'de> Deserialize<'de> for Rating {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<Level> = Vec::deserialize(d)?;
        if v.len() != 2 {
            return Err(serde::de::Error::custom("rating must be [value, risk]"));
        }
        Ok(Rating {
            value: v[0],
            risk: v[1],
        })
    }
}

// ---------- req ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Extracted,
    Assumed,
    Confirmed,
    Disputed,
    Retired,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Extracted => "extracted",
            Status::Assumed => "assumed",
            Status::Confirmed => "confirmed",
            Status::Disputed => "disputed",
            Status::Retired => "retired",
        }
    }
    pub fn all() -> [Status; 5] {
        [
            Status::Extracted,
            Status::Assumed,
            Status::Confirmed,
            Status::Disputed,
            Status::Retired,
        ]
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Pattern {
    Ubiquitous,
    EventDriven,
    StateDriven,
    Unwanted,
    Optional,
    Complex,
}

/// One rev of a requirement. A `reqs/<slug>.yaml` file is a list of these, newest last (`obj~store-shape~1`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReqRev {
    pub id: Id,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<Pattern>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Required when `status == Retired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    #[serde(default)]
    pub questions: Vec<Id>,
    #[serde(default)]
    pub history: Vec<History>,
}

impl ReqRev {
    pub fn new(id: Id, statement: impl Into<String>, by: &str) -> Self {
        ReqRev {
            id,
            statement: statement.into(),
            pattern: None,
            status: Status::Extracted,
            rating: None,
            owner: None,
            reason: None,
            anchors: vec![],
            questions: vec![],
            history: vec![History::new(by, "create")],
        }
    }
}

// ---------- qst ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QstStatus {
    Open,
    Answered,
    Withdrawn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reading {
    pub key: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Answer {
    pub reading: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A `questions/<slug>.yaml` file is a list of these (revs), newest last.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub id: Id,
    pub status: QstStatus,
    pub quote: String,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    pub materiality: Level,
    #[serde(default)]
    pub readings: Vec<Reading>,
    #[serde(default)]
    pub affects: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    /// Held answer when `obj~qst-conflict~1` fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Answer>,
    /// `affects` revs at raise time, so conflicts can be detected.
    #[serde(default)]
    pub affects_revs: Vec<Id>,
    #[serde(default)]
    pub history: Vec<History>,
}

// ---------- grp ----------

/// A `groups/<slug>.yaml` file is a list of these (revs), newest last.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Group {
    pub id: Id,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The only place membership lives (`obj~grp-membership~1`, `obj~req-groups-derived~1`).
    #[serde(default)]
    pub members: Vec<Id>,
    #[serde(default)]
    pub history: Vec<History>,
}

// ---------- rounds ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Context {
    Branch { branch: String },
    Pr { pr: u64, head: String },
}

impl Context {
    /// Directory-safe key for this context: `main` or `pr-42`.
    pub fn key(&self) -> String {
        match self {
            Context::Branch { branch } => branch.replace('/', "__"),
            Context::Pr { pr, .. } => format!("pr-{pr}"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RoundSummary {
    #[serde(default)]
    pub created: Vec<Id>,
    #[serde(default)]
    pub changed: Vec<Id>,
    #[serde(default)]
    pub retired: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdicts: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Round {
    pub doc: String,
    pub n: u32,
    pub snapshot: String,
    pub context: Context,
    #[serde(with = "time::serde::rfc3339")]
    pub opened: OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub closed: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RoundSummary>,
}

// ---------- marks (span classifications that are not anchors) ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkKind {
    NonNormative,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `marks/<doc-key>.yaml`: span-id → mark. Requirement-mapped and question-flagged states are
/// derived from `req.anchors` / `qst.anchors`; only non-normative needs its own record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Marks {
    #[serde(default)]
    pub spans: std::collections::BTreeMap<String, Mark>,
}

// ---------- repo.yaml ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackedDoc {
    /// Forward-slash path relative to repo root, as stored in git (`ui~windows-paths~1`).
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoConfig {
    /// `owner/name`
    pub github: String,
    pub remote: String,
    pub default_branch: String,
    /// Local clone location (absolute).
    pub local_path: String,
    #[serde(default)]
    pub tracked: Vec<TrackedDoc>,
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_fetch: Option<OffsetDateTime>,
}

/// `exports/last.yaml`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportRecord {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub out_dir: String,
    /// Content hash of the exported inventory, to detect unexported changes.
    pub inventory_hash: String,
}

/// Turn a doc path into a filesystem-safe key: `docs/hld.md` → `docs__hld.md`.
pub fn doc_key(path: &str) -> String {
    path.replace('/', "__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_serializes_as_list() {
        let r = Rating {
            value: Level::H,
            risk: Level::M,
        };
        assert_eq!(serde_yaml::to_string(&r).unwrap().trim(), "- H\n- M");
        let back: Rating = serde_yaml::from_str("[H, M]").unwrap();
        assert_eq!(back, r);
        assert!(serde_yaml::from_str::<Rating>("[H]").is_err());
    }

    #[test]
    fn req_roundtrip_matches_reqtrace_shape() {
        let id: Id = "req~login-throttling~2".parse().unwrap();
        let mut r = ReqRev::new(id, "When x, the system shall y.", "cj");
        r.status = Status::Confirmed;
        r.rating = Some(Rating {
            value: Level::H,
            risk: Level::L,
        });
        r.owner = Some("pm-jane".into());
        let y = serde_yaml::to_string(&vec![r.clone()]).unwrap();
        assert!(y.contains("id: req~login-throttling~2"));
        assert!(y.contains("status: confirmed"));
        assert!(y.contains("rating:\n  - H\n  - L"));
        let back: Vec<ReqRev> = serde_yaml::from_str(&y).unwrap();
        assert_eq!(back[0], r);
    }

    #[test]
    fn context_untagged() {
        let b = Context::Branch {
            branch: "main".into(),
        };
        let p = Context::Pr {
            pr: 42,
            head: "9b2e".into(),
        };
        assert_eq!(serde_yaml::to_string(&b).unwrap().trim(), "branch: main");
        let back: Context = serde_yaml::from_str("pr: 42\nhead: 9b2e").unwrap();
        assert_eq!(back, p);
        assert_eq!(p.key(), "pr-42");
        assert_eq!(
            Context::Branch {
                branch: "feat/x".into()
            }
            .key(),
            "feat__x"
        );
    }
}
