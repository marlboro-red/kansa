//! Markdown-aware sentence segmentation (`ui~spans~1`, `obj~span-blocks~1`).
//!
//! Prose paragraphs are sentence-split; headings, list items, table rows, code blocks and
//! HTML blocks are single spans. Every span carries its byte range in the source so the UI can
//! render source slices and reconciliation can diff on text.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Block {
    Para,
    Heading,
    Li,
    Row,
    Code,
    Html,
}

/// A segmented unit before identity is assigned (see `snapshot::Snapshot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub block: Block,
    pub text: String,
    pub range: Range<usize>,
    /// Nearest preceding heading text, if any.
    pub section: Option<String>,
    /// Heading level for `Block::Heading`, list depth for `Block::Li`, else 0.
    pub depth: u8,
}

/// Segment a markdown document into pieces in document order.
pub fn segment(src: &str) -> Vec<Piece> {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let parser = Parser::new_ext(src, opts).into_offset_iter();
    let mut seg = Segmenter {
        src,
        out: vec![],
        section: None,
        list_depth: 0,
        in_table_head: false,
        stack: vec![],
    };
    for (ev, range) in parser {
        seg.event(ev, range);
    }
    seg.out
}

/// A run of inline text with its source range; a block collects several before flushing.
struct Inline {
    text: String,
    range: Range<usize>,
}

enum Frame {
    /// Prose container: paragraph (sentence-split on flush).
    Para(Vec<Inline>),
    Heading(u8, Vec<Inline>, Range<usize>),
    /// List item: direct inline content only; nested blocks push their own frames.
    Item(Vec<Inline>, Range<usize>, u8),
    Row(Vec<String>, Range<usize>),
    /// Marker: the item's first paragraph — its inlines go to the enclosing `Item`.
    ItemPara,
    /// An item whose span has already been emitted (nested list started); later inlines are dropped.
    ItemDone,
    /// Something we swallow entirely into one span (code, html).
    Opaque,
}

struct Segmenter<'a> {
    src: &'a str,
    out: Vec<Piece>,
    section: Option<String>,
    list_depth: u8,
    in_table_head: bool,
    stack: Vec<Frame>,
}

impl<'a> Segmenter<'a> {
    fn push_inline(&mut self, text: &str, range: Range<usize>) {
        // Attach text to the innermost frame that collects inlines.
        for f in self.stack.iter_mut().rev() {
            match f {
                Frame::Para(v) | Frame::Heading(_, v, _) | Frame::Item(v, _, _) => {
                    v.push(Inline {
                        text: text.to_string(),
                        range,
                    });
                    return;
                }
                Frame::Row(cells, _) => {
                    if let Some(last) = cells.last_mut() {
                        last.push_str(text);
                    }
                    return;
                }
                Frame::ItemPara => continue,
                Frame::ItemDone | Frame::Opaque => return,
            }
        }
    }

    /// Emit the innermost open `Item` (if it still holds text) and mark it done.
    fn flush_item(&mut self) {
        let idx = self
            .stack
            .iter()
            .rposition(|f| matches!(f, Frame::Item(..) | Frame::ItemDone));
        if let Some(i) = idx {
            if let Frame::Item(inl, r, depth) =
                std::mem::replace(&mut self.stack[i], Frame::ItemDone)
            {
                // Range: item start → end of last direct inline (so nested lists aren't swallowed).
                let end = inl.last().map(|x| x.range.end).unwrap_or(r.start);
                let text = join(&inl);
                if !text.trim().is_empty() {
                    self.emit(Block::Li, text, r.start..end.max(r.start), depth);
                }
            }
        }
    }

    fn event(&mut self, ev: Event<'a>, range: Range<usize>) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    // First paragraph of a loose list item is the item's own text.
                    if matches!(self.stack.last(), Some(Frame::Item(v, _, _)) if v.is_empty()) {
                        self.stack.push(Frame::ItemPara);
                    } else {
                        self.stack.push(Frame::Para(vec![]));
                    }
                }
                Tag::Heading { level, .. } => {
                    self.stack.push(Frame::Heading(level as u8, vec![], range))
                }
                Tag::List(_) => {
                    self.flush_item();
                    self.list_depth += 1;
                }
                Tag::Item => self.stack.push(Frame::Item(vec![], range, self.list_depth)),
                Tag::TableHead => {
                    self.in_table_head = true;
                    self.stack.push(Frame::Row(vec![], range));
                }
                Tag::TableRow => self.stack.push(Frame::Row(vec![], range)),
                Tag::TableCell => {
                    if let Some(Frame::Row(cells, _)) = self.stack.last_mut() {
                        cells.push(String::new());
                    }
                }
                Tag::CodeBlock(kind) => {
                    let text = match kind {
                        CodeBlockKind::Fenced(_) | CodeBlockKind::Indented => {
                            self.src[range.clone()].to_string()
                        }
                    };
                    self.emit_raw(Block::Code, text.trim_end().to_string(), range);
                    self.stack.push(Frame::Opaque);
                }
                Tag::HtmlBlock => {
                    self.emit_raw(
                        Block::Html,
                        self.src[range.clone()].trim_end().to_string(),
                        range,
                    );
                    self.stack.push(Frame::Opaque);
                }
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. } => {}
                Tag::BlockQuote(_)
                | Tag::Table(_)
                | Tag::FootnoteDefinition(_)
                | Tag::MetadataBlock(_) => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => match self.stack.pop() {
                    Some(Frame::Para(inl)) => self.flush_prose(inl),
                    Some(Frame::ItemPara) => self.flush_item(),
                    _ => {}
                },
                TagEnd::Heading(_) => {
                    if let Some(Frame::Heading(level, inl, r)) = self.stack.pop() {
                        let text = join(&inl);
                        self.section = Some(text.clone());
                        self.emit(Block::Heading, text, r, level);
                    }
                }
                TagEnd::List(_) => self.list_depth = self.list_depth.saturating_sub(1),
                TagEnd::Item => {
                    self.flush_item();
                    self.stack.pop();
                }
                TagEnd::TableHead => {
                    self.in_table_head = false;
                    self.end_row();
                }
                TagEnd::TableRow => self.end_row(),
                TagEnd::CodeBlock | TagEnd::HtmlBlock => {
                    self.stack.pop();
                }
                _ => {}
            },
            Event::Text(t) => self.push_inline(&t, range),
            Event::Code(t) => self.push_inline(&t, range),
            Event::InlineMath(t) | Event::DisplayMath(t) => self.push_inline(&t, range),
            Event::Html(t) | Event::InlineHtml(t) => self.push_inline(&t, range),
            Event::SoftBreak | Event::HardBreak => self.push_inline(" ", range),
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(_) => {}
            Event::Rule => {}
        }
    }

    fn end_row(&mut self) {
        if let Some(Frame::Row(cells, r)) = self.stack.pop() {
            let text = cells
                .iter()
                .map(|c| c.trim())
                .collect::<Vec<_>>()
                .join(" | ");
            if !text.trim().is_empty() {
                self.emit(Block::Row, text, r, 0);
            }
        }
    }

    fn emit_raw(&mut self, block: Block, text: String, range: Range<usize>) {
        if text.trim().is_empty() {
            return;
        }
        self.out.push(Piece {
            block,
            text,
            range,
            section: self.section.clone(),
            depth: 0,
        });
    }

    fn emit(&mut self, block: Block, text: String, range: Range<usize>, depth: u8) {
        let text = normalize_ws(&text);
        if text.is_empty() {
            return;
        }
        self.out.push(Piece {
            block,
            text,
            range,
            section: self.section.clone(),
            depth,
        });
    }

    /// Sentence-split a paragraph's inlines, mapping each sentence back to a source range.
    fn flush_prose(&mut self, inl: Vec<Inline>) {
        // Build combined text + per-byte map back to source offsets.
        let mut combined = String::new();
        let mut per_byte: Vec<usize> = vec![]; // per_byte[i] = source offset of combined byte i
        for i in &inl {
            let base = i.range.start;
            // Only map byte-for-byte when the inline text is a literal slice of the source
            // (Text events are; Code events include backticks, breaks are synthesized).
            let literal = self.src.get(i.range.clone()) == Some(i.text.as_str());
            for bi in 0..i.text.len() {
                per_byte.push(if literal {
                    base + bi
                } else {
                    base + bi.min(i.range.len())
                });
            }
            combined.push_str(&i.text);
        }
        let end_src = inl.last().map(|i| i.range.end).unwrap_or(0);
        per_byte.push(end_src);

        for (s, e) in split_sentences(&combined) {
            let text = combined[s..e].trim();
            if text.is_empty() {
                continue;
            }
            // trim adjust
            let lead = combined[s..e].len() - combined[s..e].trim_start().len();
            let trail = combined[s..e].len() - combined[s..e].trim_end().len();
            let (s2, e2) = (s + lead, e - trail);
            let src_start = per_byte[s2];
            let src_end = if e2 < per_byte.len() {
                per_byte[e2]
            } else {
                end_src
            };
            let src_end = src_end.max(src_start);
            self.emit(Block::Para, text.to_string(), src_start..src_end, 0);
        }
    }
}

fn join(inl: &[Inline]) -> String {
    inl.iter().map(|i| i.text.as_str()).collect::<String>()
}

pub fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

const ABBREVIATIONS: &[&str] = &[
    "e.g", "i.e", "etc", "vs", "cf", "fig", "no", "approx", "incl", "excl", "resp", "dept", "est",
    "min", "max", "sec", "mr", "mrs", "ms", "dr", "prof", "st", "jr", "sr", "inc", "ltd", "co",
    "corp", "ver", "rev", "ch", "sect", "para", "vol", "pp", "ed", "eds", "al", "u.s", "u.k",
];

/// Byte ranges of sentences in `text`. Conservative: only splits on `.!?` followed by whitespace
/// and something that looks like a sentence start; skips common abbreviations and initials.
pub fn split_sentences(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = vec![];
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '.' || c == '!' || c == '?' {
            // absorb runs like "?!" or "..."
            let mut j = i;
            while j + 1 < bytes.len() && matches!(bytes[j + 1] as char, '.' | '!' | '?') {
                j += 1;
            }
            // absorb closing quotes/brackets
            let mut k = j;
            while k + 1 < bytes.len()
                && matches!(
                    bytes[k + 1] as char,
                    '"' | '\'' | ')' | ']' | '”' | '’' | '`' | '*' | '_'
                )
            {
                k += 1;
            }
            let at_end = k + 1 >= bytes.len();
            let next_ws = !at_end && (bytes[k + 1] as char).is_whitespace();
            if at_end || next_ws {
                let boundary = at_end || {
                    // find next non-space char
                    let mut m = k + 1;
                    while m < bytes.len() && (bytes[m] as char).is_whitespace() {
                        m += 1;
                    }
                    if m >= bytes.len() {
                        true
                    } else {
                        let nc = text[m..].chars().next().unwrap();
                        let starts_like_sentence = nc.is_uppercase()
                            || nc.is_ascii_digit()
                            || matches!(
                                nc,
                                '"' | '\''
                                    | '('
                                    | '['
                                    | '“'
                                    | '‘'
                                    | '`'
                                    | '*'
                                    | '_'
                                    | '-'
                                    | '—'
                                    | '–'
                            );
                        starts_like_sentence && c != '.'
                            || (c == '.'
                                && starts_like_sentence
                                && !is_abbreviation(&text[start..=i]))
                    }
                };
                if boundary {
                    out.push((start, k + 1));
                    start = k + 1;
                    i = k + 1;
                    continue;
                }
            }
            i = k + 1;
            continue;
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push((start, bytes.len()));
    }
    out
}

/// Does the text ending at a `.` end with an abbreviation, initial, or number-ish token?
fn is_abbreviation(upto_dot: &str) -> bool {
    let body = &upto_dot[..upto_dot.len() - 1];
    let word: String = body
        .chars()
        .rev()
        .take_while(|c| !c.is_whitespace() && *c != '(')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let w = word
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '.')
        .to_ascii_lowercase();
    if w.is_empty() {
        return false;
    }
    // Numeric like "v1.2." → not obviously; but "3." list-like numbers, treat "1." at start as abbreviation-ish only if word is all digits AND short.
    ABBREVIATIONS
        .iter()
        .any(|a| w == *a || w.ends_with(&format!(".{a}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str) -> Vec<(Block, String)> {
        segment(src)
            .into_iter()
            .map(|p| (p.block, p.text))
            .collect()
    }

    #[test]
    fn splits_plain_sentences() {
        let s = split_sentences("First one. Second one! Third? Yes.");
        assert_eq!(s.len(), 4);
        let s = split_sentences("Approx. 5 users, e.g. admins, log in at 3.5 pm. Then it stops.");
        assert_eq!(s.len(), 2, "{s:?}");
        let s = split_sentences("See Fig. 3 for details. Done.");
        assert_eq!(s.len(), 2);
        let s = split_sentences("He said \"stop.\" Then left.");
        assert_eq!(s.len(), 2);
        let s = split_sentences("Sentence without period");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn paragraph_sentences_have_source_ranges() {
        let src = "# Title\n\nThe system shall do X. It **must** also do Y.\n";
        let ps = segment(src);
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0].block, Block::Heading);
        assert_eq!(ps[0].text, "Title");
        assert_eq!(ps[1].text, "The system shall do X.");
        assert_eq!(&src[ps[1].range.clone()], "The system shall do X.");
        assert_eq!(ps[2].text, "It must also do Y.");
        assert_eq!(&src[ps[2].range.clone()], "It **must** also do Y.");
        assert_eq!(ps[2].section.as_deref(), Some("Title"));
    }

    #[test]
    fn lists_tables_code_are_units() {
        let src = "\
- First item. With two sentences.
- Second item
  - Nested item. Also two.

| a | b |
|---|---|
| 1. x | 2. y |

```rust
fn main() {} // 1. 2. 3.
```

Para one. Para two.
";
        let t = texts(src);
        assert_eq!(
            t,
            vec![
                (Block::Li, "First item. With two sentences.".into()),
                (Block::Li, "Second item".into()),
                (Block::Li, "Nested item. Also two.".into()),
                (Block::Row, "a | b".into()),
                (Block::Row, "1. x | 2. y".into()),
                (Block::Code, "```rust\nfn main() {} // 1. 2. 3.\n```".into()),
                (Block::Para, "Para one.".into()),
                (Block::Para, "Para two.".into()),
            ]
        );
        let ps = segment(src);
        assert_eq!(ps[2].depth, 2);
        assert_eq!(&src[ps[1].range.clone()], "- Second item");
    }

    #[test]
    fn loose_list_items_and_blockquotes() {
        let src = "1. Do this.\n\n2. Then that. And more.\n\n> Quoted. Twice.\n";
        let t = texts(src);
        assert_eq!(t[0], (Block::Li, "Do this.".into()));
        assert_eq!(t[1], (Block::Li, "Then that. And more.".into()));
        assert_eq!(t[2], (Block::Para, "Quoted.".into()));
        assert_eq!(t[3], (Block::Para, "Twice.".into()));
    }

    #[test]
    fn deterministic() {
        let src = include_str!("../tests/fixtures/sample-hld.md");
        let a = segment(src);
        let b = segment(src);
        assert_eq!(a, b);
        assert!(a.len() > 20);
        for p in &a {
            assert!(!p.text.is_empty());
            assert!(p.range.end <= src.len());
        }
    }
}
