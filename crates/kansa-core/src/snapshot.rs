//! Snapshots: an immutable segmented view of one doc at one blob sha (spec §0.1).

use crate::segment::{segment, Block};
use crate::store::SEGMENTER_VERSION;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Span {
    /// `s-<8 hex>` or `s-<8 hex>-<n>` for repeats (`obj~span-id~1`).
    pub id: String,
    /// Display order.
    pub ord: u32,
    pub block: Block,
    pub text: String,
    /// Full content hash (16 hex) of normalized text.
    pub h: String,
    /// Byte range in the source doc.
    pub start: usize,
    pub end: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: u8,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub doc: String,
    /// git blob sha of the doc content.
    pub sha: String,
    pub segmenter: u32,
    pub spans: Vec<Span>,
}

/// git blob sha1 of `content` — same as `git hash-object`.
pub fn blob_sha(content: &[u8]) -> String {
    git2::Oid::hash_object(git2::ObjectType::Blob, content)
        .map(|o| o.to_string())
        .unwrap_or_default()
}

pub fn content_hash(normalized: &str) -> String {
    let d = Sha256::digest(normalized.as_bytes());
    hex::encode(&d[..8])
}

impl Snapshot {
    /// Segment `src` and assign span identity. Deterministic for identical input (`ui~spans~1`).
    pub fn build(doc: &str, src: &str) -> Snapshot {
        let sha = blob_sha(src.as_bytes());
        let mut seen: HashMap<String, u32> = HashMap::new();
        let spans = segment(src)
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let h = content_hash(&p.text);
                let n = seen.entry(h.clone()).or_insert(0);
                *n += 1;
                let id = if *n == 1 {
                    format!("s-{}", &h[..8])
                } else {
                    format!("s-{}-{}", &h[..8], n)
                };
                Span {
                    id,
                    ord: i as u32,
                    block: p.block,
                    text: p.text,
                    h,
                    start: p.range.start,
                    end: p.range.end,
                    section: p.section,
                    depth: p.depth,
                }
            })
            .collect();
        Snapshot {
            doc: doc.into(),
            sha,
            segmenter: SEGMENTER_VERSION,
            spans,
        }
    }

    pub fn span(&self, id: &str) -> Option<&Span> {
        self.spans.iter().find(|s| s.id == id)
    }

    pub fn index(&self) -> HashMap<&str, &Span> {
        self.spans.iter().map(|s| (s.id.as_str(), s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_content_derived_and_disambiguated() {
        let a = Snapshot::build("d.md", "Same thing.\n\nSame thing.\n\nOther.\n");
        assert_eq!(a.spans.len(), 3);
        assert_eq!(a.spans[0].id, format!("s-{}", &a.spans[0].h[..8]));
        assert_eq!(a.spans[1].id, format!("{}-2", a.spans[0].id));
        assert_ne!(a.spans[2].id, a.spans[0].id);
        // Moving text keeps its id.
        let b = Snapshot::build("d.md", "Other.\n\nSame thing.\n");
        assert_eq!(b.spans[0].id, a.spans[2].id);
        assert_eq!(b.spans[1].id, a.spans[0].id);
        assert_ne!(a.sha, b.sha);
    }

    #[test]
    fn blob_sha_matches_git() {
        // `printf 'hello\n' | git hash-object --stdin` = ce013625030ba8dba906f756967f9e9ca394464a
        assert_eq!(
            blob_sha(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn golden_sample_hld() {
        let src = include_str!("../tests/fixtures/sample-hld.md");
        let snap = Snapshot::build("docs/hld.md", src);
        let got = serde_yaml::to_string(&snap).unwrap();
        let golden_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample-hld.snapshot.yaml"
        );
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(golden_path, &got).unwrap();
        }
        let want = std::fs::read_to_string(golden_path)
            .expect("golden file missing; run with UPDATE_GOLDEN=1");
        assert_eq!(
            got, want,
            "snapshot golden drift — run with UPDATE_GOLDEN=1 if intended"
        );
    }
}
