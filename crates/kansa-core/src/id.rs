//! Object identifiers: `<type>~<slug>~<rev>` (reqtrace `inv~id-grammar~2`).
//!
//! `type`  = `[a-z][a-z0-9]*`
//! `slug`  = `[a-z0-9]+(-[a-z0-9]+)*`   (no leading/trailing/consecutive hyphens)
//! `rev`   = `[1-9][0-9]*`

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("id `{0}` must have exactly three `~`-separated parts")]
    Shape(String),
    #[error("id `{0}`: bad type (want [a-z][a-z0-9]*)")]
    Type(String),
    #[error("id `{0}`: bad slug (want [a-z0-9]+(-[a-z0-9]+)*)")]
    Slug(String),
    #[error("id `{0}`: bad rev (want positive integer, no leading zero)")]
    Rev(String),
}

/// A `type~slug` pair without rev — what coverage and grouping key on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub ty: String,
    pub slug: String,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}~{}", self.ty, self.slug)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    pub ty: String,
    pub slug: String,
    pub rev: u32,
}

impl Id {
    pub fn new(ty: &str, slug: &str, rev: u32) -> Result<Self, IdError> {
        let s = format!("{ty}~{slug}~{rev}");
        s.parse()
    }

    pub fn key(&self) -> Key {
        Key {
            ty: self.ty.clone(),
            slug: self.slug.clone(),
        }
    }

    pub fn with_rev(&self, rev: u32) -> Id {
        Id {
            ty: self.ty.clone(),
            slug: self.slug.clone(),
            rev,
        }
    }
}

pub fn valid_type(s: &str) -> bool {
    let mut it = s.chars();
    matches!(it.next(), Some(c) if c.is_ascii_lowercase())
        && it.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_rev(s: &str) -> Option<u32> {
    if s.is_empty() || s.starts_with('0') || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl FromStr for Id {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let parts: Vec<&str> = s.split('~').collect();
        if parts.len() != 3 {
            return Err(IdError::Shape(s.into()));
        }
        if !valid_type(parts[0]) {
            return Err(IdError::Type(s.into()));
        }
        if !valid_slug(parts[1]) {
            return Err(IdError::Slug(s.into()));
        }
        let rev = valid_rev(parts[2]).ok_or_else(|| IdError::Rev(s.into()))?;
        Ok(Id {
            ty: parts[0].into(),
            slug: parts[1].into(),
            rev,
        })
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}~{}~{}", self.ty, self.slug, self.rev)
    }
}

impl Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "by", "at", "as", "is",
    "are", "be", "been", "shall", "should", "must", "will", "may", "can", "system", "tool", "it",
    "its", "this", "that", "these", "those", "when", "while", "if", "then", "where", "which",
    "who", "from", "into", "than", "not", "no", "any", "all", "each", "every", "per", "via", "so",
    "such", "only", "also", "both", "either", "neither", "we", "user", "users",
];

/// Turn free text into a slug candidate: lowercase alnum words, stopwords dropped, whole words
/// only, up to `max_len` characters. Falls back to the first words if everything was a stopword.
pub fn slugify(text: &str, max_len: usize) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let pick = |ws: &[String]| -> String {
        let mut out = String::new();
        for w in ws {
            let need = if out.is_empty() {
                w.len()
            } else {
                out.len() + 1 + w.len()
            };
            if need > max_len {
                break;
            }
            if !out.is_empty() {
                out.push('-');
            }
            out.push_str(w);
        }
        out
    };
    let meaningful: Vec<String> = words
        .iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .cloned()
        .collect();
    let mut out = pick(&meaningful);
    if out.is_empty() {
        out = pick(&words);
    }
    if out.is_empty() {
        // a single overlong word: hard-cut it
        out = words
            .first()
            .map(|w| w.chars().take(max_len).collect())
            .unwrap_or_default();
    }
    if out.is_empty() {
        "item".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_good_ids() {
        let id: Id = "req~login-throttling~2".parse().unwrap();
        assert_eq!(id.ty, "req");
        assert_eq!(id.slug, "login-throttling");
        assert_eq!(id.rev, 2);
        assert_eq!(id.to_string(), "req~login-throttling~2");
        assert!("ui2~a1~10".parse::<Id>().is_ok());
    }

    #[test]
    fn rejects_bad_ids() {
        for bad in [
            "req~x",
            "req~x~1~2",
            "Req~x~1",
            "req~-x~1",
            "req~x-~1",
            "req~a--b~1",
            "req~x~0",
            "req~x~01",
            "req~x~",
            "~x~1",
            "req~X~1",
            "req~x_y~1",
        ] {
            assert!(bad.parse::<Id>().is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn serde_roundtrip() {
        let id: Id = "grp~validation~1".parse().unwrap();
        let y = serde_yaml::to_string(&id).unwrap();
        assert_eq!(y.trim(), "grp~validation~1");
        let back: Id = serde_yaml::from_str(&y).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(
            slugify("When a user fails login 5 times!", 40),
            "fails-login-5-times"
        );
        assert_eq!(slugify("  --Foo__Bar--  ", 40), "foo-bar");
        assert_eq!(slugify("!!!", 40), "item");
        assert_eq!(
            slugify("The system shall be the tool.", 40),
            "the-system-shall-be-the-tool"
        );
        assert_eq!(
            slugify(
                "The tool shall read a requirements inventory and design docs, compute coverage",
                32
            ),
            "read-requirements-inventory"
        );
        assert_eq!(
            slugify("Supercalifragilisticexpialidocious", 10),
            "supercalif"
        );
        assert!(valid_slug(&slugify(
            "Some really long sentence about validation rules across forms",
            24
        )));
    }
}
