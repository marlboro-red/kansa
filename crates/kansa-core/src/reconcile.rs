//! Reconciliation (spec §4.4, `obj~anchor~1`, `obj~round-supersede~1`): when a tracked doc
//! changes, map every classified span of the old snapshot onto the new one and propose a
//! verdict per span. Humans confirm; anchors are never rewritten silently.

use crate::snapshot::{Snapshot, Span};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum VerdictKind {
    /// Same content hash exists in the new snapshot.
    Unchanged,
    /// Best match is similar but not identical (proposed; human may downgrade to meaning-changed).
    Reworded,
    /// Human decided the rewording changes meaning.
    MeaningChanged,
    /// No acceptable match in the new snapshot.
    Missing,
}

/// What the human decided for one verdict.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Decision {
    /// Accept the proposed mapping (unchanged/reworded → keep anchor on `to`).
    Accept,
    /// Rewording changed meaning: keep the mapping, mark affected requirements.
    MeaningChanged,
    /// Re-anchor to a specific new span.
    Reanchor { span: String },
    /// Drop this anchor (requirement keeps its other anchors, or must be retired if none).
    Drop,
    /// Retire the affected requirement(s) with a reason.
    Retire { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    /// Span id in the old snapshot.
    pub from: String,
    pub from_text: String,
    /// Proposed span id in the new snapshot (None when missing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_text: Option<String>,
    pub kind: VerdictKind,
    /// 0..1 similarity between from/to texts.
    pub similarity: f32,
    /// Requirement ids anchored to `from`.
    #[serde(default)]
    pub reqs: Vec<String>,
    /// Question ids anchored to `from`.
    #[serde(default)]
    pub questions: Vec<String>,
    /// True if `from` had a non-normative mark.
    #[serde(default)]
    pub non_normative: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
}

impl Verdict {
    /// Verdicts that need a human before the round can close: anything not `unchanged`.
    pub fn needs_confirmation(&self) -> bool {
        self.kind != VerdictKind::Unchanged && self.decision.is_none()
    }
}

/// A pending reconciliation between two snapshots of one doc in one context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reconciliation {
    pub doc: String,
    pub from: String,
    pub to: String,
    pub verdicts: Vec<Verdict>,
    /// New-snapshot spans with no counterpart in the old one (they become residue).
    #[serde(default)]
    pub added: Vec<String>,
}

impl Reconciliation {
    pub fn unconfirmed(&self) -> usize {
        self.verdicts
            .iter()
            .filter(|v| v.needs_confirmation())
            .count()
    }
    pub fn counts(&self) -> HashMap<VerdictKind, usize> {
        let mut m = HashMap::new();
        for v in &self.verdicts {
            *m.entry(v.kind).or_default() += 1;
        }
        m
    }
}

/// Text similarity in 0..1: max of normalized Levenshtein and word-set Jaccard.
pub fn similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let lev = strsim::normalized_levenshtein(a, b) as f32;
    let wa: HashSet<&str> = a.split_whitespace().collect();
    let wb: HashSet<&str> = b.split_whitespace().collect();
    let inter = wa.intersection(&wb).count() as f32;
    let uni = wa.union(&wb).count().max(1) as f32;
    lev.max(inter / uni)
}

pub const REWORD_THRESHOLD: f32 = 0.55;

/// Interest = spans of `old` that carry classification. `classified(span_id) -> (reqs, questions, nn)`.
pub fn reconcile(
    doc: &str,
    old: &Snapshot,
    new: &Snapshot,
    classified: impl Fn(&str) -> Option<(Vec<String>, Vec<String>, bool)>,
) -> Reconciliation {
    let new_by_id: HashMap<&str, &Span> = new.spans.iter().map(|s| (s.id.as_str(), s)).collect();
    let old_ids: HashSet<&str> = old.spans.iter().map(|s| s.id.as_str()).collect();
    let mut taken: HashSet<String> = HashSet::new(); // new span ids already matched
    let mut verdicts = vec![];

    // Pass 1: exact matches (same content hash → same id).
    let mut pending: Vec<(&Span, Vec<String>, Vec<String>, bool)> = vec![];
    for s in &old.spans {
        let Some((reqs, questions, nn)) = classified(&s.id) else {
            continue;
        };
        if let Some(n) = new_by_id.get(s.id.as_str()) {
            taken.insert(n.id.clone());
            verdicts.push(Verdict {
                from: s.id.clone(),
                from_text: s.text.clone(),
                to: Some(n.id.clone()),
                to_text: Some(n.text.clone()),
                kind: VerdictKind::Unchanged,
                similarity: 1.0,
                reqs,
                questions,
                non_normative: nn,
                decision: Some(Decision::Accept),
            });
        } else {
            pending.push((s, reqs, questions, nn));
        }
    }

    // Pass 2: fuzzy — greedy by best similarity, restricted to unmatched new spans that are
    // themselves not present in the old snapshot (moved-but-identical text was handled above).
    let candidates: Vec<&Span> = new
        .spans
        .iter()
        .filter(|s| !old_ids.contains(s.id.as_str()))
        .collect();
    let mut scored: Vec<(f32, usize, usize)> = vec![]; // (sim, pending idx, candidate idx)
    for (pi, (s, ..)) in pending.iter().enumerate() {
        for (ci, c) in candidates.iter().enumerate() {
            if c.block != s.block {
                continue;
            }
            let sim = similarity(&s.text, &c.text);
            if sim >= REWORD_THRESHOLD {
                // slight preference for nearby positions
                let dist = (s.ord as f32 - c.ord as f32).abs();
                scored.push((sim - dist * 0.0005, pi, ci));
            }
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut matched_pending: HashMap<usize, usize> = HashMap::new();
    let mut used_candidates: HashSet<usize> = HashSet::new();
    for (_, pi, ci) in scored {
        if matched_pending.contains_key(&pi) || used_candidates.contains(&ci) {
            continue;
        }
        matched_pending.insert(pi, ci);
        used_candidates.insert(ci);
    }
    for (pi, (s, reqs, questions, nn)) in pending.into_iter().enumerate() {
        match matched_pending.get(&pi) {
            Some(&ci) => {
                let c = candidates[ci];
                taken.insert(c.id.clone());
                verdicts.push(Verdict {
                    from: s.id.clone(),
                    from_text: s.text.clone(),
                    to: Some(c.id.clone()),
                    to_text: Some(c.text.clone()),
                    kind: VerdictKind::Reworded,
                    similarity: similarity(&s.text, &c.text),
                    reqs,
                    questions,
                    non_normative: nn,
                    decision: None,
                });
            }
            None => verdicts.push(Verdict {
                from: s.id.clone(),
                from_text: s.text.clone(),
                to: None,
                to_text: None,
                kind: VerdictKind::Missing,
                similarity: 0.0,
                reqs,
                questions,
                non_normative: nn,
                decision: None,
            }),
        }
    }
    // keep document order
    let ord: HashMap<&str, u32> = old.spans.iter().map(|s| (s.id.as_str(), s.ord)).collect();
    verdicts.sort_by_key(|v| ord.get(v.from.as_str()).copied().unwrap_or(u32::MAX));

    let added = new
        .spans
        .iter()
        .filter(|s| !old_ids.contains(s.id.as_str()) && !taken.contains(&s.id))
        .map(|s| s.id.clone())
        .collect();

    Reconciliation {
        doc: doc.into(),
        from: old.sha.clone(),
        to: new.sha.clone(),
        verdicts,
        added,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(src: &str) -> Snapshot {
        Snapshot::build("d.md", src)
    }

    #[test]
    fn verdict_kinds() {
        let old = snap("# T\n\nThe system shall lock after 3 failures. Users get an email. This one vanishes entirely.\n\nUnchanged sentence here.\n");
        let new = snap("# T\n\nThe system shall lock after 5 failures. Users get an email.\n\nUnchanged sentence here.\n\nBrand new sentence.\n");
        let ids: Vec<String> = old.spans.iter().map(|s| s.id.clone()).collect();
        // classify: lock (req), email (nn), vanishes (req), unchanged (nn)
        let r = reconcile("d.md", &old, &new, |id| {
            let i = ids.iter().position(|x| x == id)?;
            match i {
                1 => Some((vec!["req~lock~1".into()], vec![], false)),
                2 => Some((vec![], vec![], true)),
                3 => Some((vec!["req~gone~1".into()], vec![], false)),
                4 => Some((vec![], vec![], true)),
                _ => None,
            }
        });
        let kinds: Vec<VerdictKind> = r.verdicts.iter().map(|v| v.kind).collect();
        assert_eq!(
            kinds,
            vec![
                VerdictKind::Reworded,
                VerdictKind::Unchanged,
                VerdictKind::Missing,
                VerdictKind::Unchanged
            ]
        );
        assert!(r.verdicts[0].similarity > 0.8);
        assert_eq!(
            r.verdicts[0].to_text.as_deref(),
            Some("The system shall lock after 5 failures.")
        );
        assert_eq!(r.unconfirmed(), 2);
        assert_eq!(r.added.len(), 1); // "Brand new sentence."
        assert!(new.span(&r.added[0]).unwrap().text.starts_with("Brand new"));
    }

    #[test]
    fn similarity_basics() {
        assert_eq!(similarity("a b c", "a b c"), 1.0);
        assert!(similarity("The tool shall exit 0.", "The tool shall exit 1.") > 0.8);
        assert!(similarity("completely different words here", "nothing alike at all") < 0.4);
    }
}
