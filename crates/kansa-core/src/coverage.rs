//! Per-span classification state and the coverage meter (spec §4.1 status bar).

use crate::model::{Context, MarkKind, QstStatus, Status};
use crate::segment::Block;
use crate::snapshot::Snapshot;
use crate::store::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpanState {
    Unclassified,
    Mapped,
    NonNormative,
    Question,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpanStatus {
    pub state: SpanState,
    /// Requirement ids anchored here (current revs).
    pub reqs: Vec<String>,
    /// Open question ids anchored here.
    pub questions: Vec<String>,
    /// True for headings/code/html: not counted in the denominator unless the user maps them.
    pub structural: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Meter {
    /// Spans that count (prose, list items, table rows + any structural span the user classified).
    pub total: usize,
    pub classified: usize,
    pub residue: usize,
    pub mapped: usize,
    pub non_normative: usize,
    pub questioned: usize,
    pub open_questions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocCoverage {
    pub doc: String,
    pub snapshot: String,
    pub meter: Meter,
    /// span-id → status, in snapshot order.
    pub spans: Vec<(String, SpanStatus)>,
}

pub fn is_structural(b: Block) -> bool {
    matches!(b, Block::Heading | Block::Code | Block::Html)
}

/// Compute classification state for every span of a snapshot.
pub fn doc_coverage(store: &Store, ctx: &Context, snap: &Snapshot) -> Result<DocCoverage> {
    let mut reqs_by_span: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let current_reqs = store.current_reqs()?;
    for r in &current_reqs {
        if r.status == Status::Retired {
            continue;
        }
        for a in &r.anchors {
            if a.doc == snap.doc {
                reqs_by_span
                    .entry(a.span.as_str())
                    .or_default()
                    .push(r.id.to_string());
            }
        }
    }
    let mut qsts_by_span: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let qsts = store.current_qsts()?;
    let mut open_questions = 0;
    for q in &qsts {
        if q.status != QstStatus::Open {
            continue;
        }
        let mut touches = false;
        for a in &q.anchors {
            if a.doc == snap.doc {
                touches = true;
                qsts_by_span
                    .entry(a.span.as_str())
                    .or_default()
                    .push(q.id.to_string());
            }
        }
        if touches {
            open_questions += 1;
        }
    }
    // Marks are per context; a PR context inherits the default branch's marks for spans that
    // still exist (anchors are content-addressed, so this keeps PR views consistent).
    let mut marks = store.marks(ctx, &snap.doc)?;
    if let Context::Pr { .. } = ctx {
        let base = Context::Branch {
            branch: store.repo()?.default_branch,
        };
        let base_marks = store.marks(&base, &snap.doc)?;
        for (k, v) in base_marks.spans {
            marks.spans.entry(k).or_insert(v);
        }
    }

    let mut meter = Meter {
        open_questions,
        ..Default::default()
    };
    let mut spans = Vec::with_capacity(snap.spans.len());
    for s in &snap.spans {
        let reqs = reqs_by_span.remove(s.id.as_str()).unwrap_or_default();
        let questions = qsts_by_span.remove(s.id.as_str()).unwrap_or_default();
        let structural = is_structural(s.block);
        let state = if !reqs.is_empty() {
            SpanState::Mapped
        } else if !questions.is_empty() {
            SpanState::Question
        } else if marks
            .spans
            .get(&s.id)
            .map(|m| m.kind == MarkKind::NonNormative)
            .unwrap_or(false)
        {
            SpanState::NonNormative
        } else {
            SpanState::Unclassified
        };
        let counts = !structural || state != SpanState::Unclassified;
        if counts {
            meter.total += 1;
            match state {
                SpanState::Unclassified => meter.residue += 1,
                SpanState::Mapped => meter.mapped += 1,
                SpanState::NonNormative => meter.non_normative += 1,
                SpanState::Question => meter.questioned += 1,
            }
        }
        spans.push((
            s.id.clone(),
            SpanStatus {
                state,
                reqs,
                questions,
                structural,
            },
        ));
    }
    meter.classified = meter.total - meter.residue;
    Ok(DocCoverage {
        doc: snap.doc.clone(),
        snapshot: snap.sha.clone(),
        meter,
        spans,
    })
}

/// Repo-level rollups for oversight (`ui~oversight~1`, partial in UM0).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RepoRollup {
    pub reqs_by_status: BTreeMap<String, usize>,
    pub open_questions: usize,
    pub groups: usize,
    pub unexported_changes: bool,
}

pub fn repo_rollup(store: &Store) -> Result<RepoRollup> {
    let mut r = RepoRollup::default();
    for s in Status::all() {
        r.reqs_by_status.insert(s.as_str().into(), 0);
    }
    for req in store.current_reqs()? {
        *r.reqs_by_status
            .entry(req.status.as_str().into())
            .or_default() += 1;
    }
    r.open_questions = store
        .current_qsts()?
        .iter()
        .filter(|q| q.status == QstStatus::Open)
        .count();
    r.groups = store.current_grps()?.len();
    r.unexported_changes = crate::export::has_unexported_changes(store)?;
    Ok(r)
}
