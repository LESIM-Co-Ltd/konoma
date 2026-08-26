//! mermaid's `sankey-beta` language.
//!
//! Read from `sankey.jison` and `sankeyDB.ts` at tag `mermaid@11.17.2`. The grammar is RFC 4180
//! CSV, cut down to three columns:
//!
//! ```text
//! record: field[source] COMMA field[target] COMMA field[value];
//! field:  DQUOTE ESCAPED_TEXT DQUOTE | NON_ESCAPED_TEXT;
//! ```
//!
//! * **A field is quoted with `"` and a literal quote is written `""`** — CSV's doubling rule, not
//!   a backslash. `findOrCreateNode` does the un-doubling (`replaceAll('""', '"')`), which is why
//!   it happens here rather than in [`super::read_quoted`].
//! * **A bare field may contain spaces and apostrophes but never a comma or a quote**
//!   (`TEXTDATA` is `[ -!#-+--~]` — printable ASCII minus `"` and
//!   `,`). Note what that excludes: **non-ASCII**. A node named 顧客 is outside `TEXTDATA`, so
//!   upstream cannot read it unquoted. konoma accepts it, deliberately — refusing a CJK label
//!   because a 2009 CSV grammar predates it fails §0-1's "the same source read correctly", and
//!   there is nothing ambiguous about it.
//! * **Nodes are created in first-appearance order** and shared by id.
//! * **A repeated pair is a second link**, not a merge.
//!
//! # A cycle is refused
//!
//! Upstream hands the graph to `d3-sankey`, whose `computeNodeDepths` throws "circular link" on a
//! cycle. konoma refuses it too rather than drawing something: a Sankey's whole claim is that
//! everything entering a node leaves it, and a diagram with a loop in it has no left-to-right
//! order to draw that claim along.

use super::{find_header, first_word, parse_number, Envelope, ParseError, Preamble, TitleSyntax};

/// The keywords, longest first.
pub const KEYWORDS: &[&str] = &["sankey-beta", "sankey"];

/// The name used in messages.
pub const KEYWORD: &str = "sankey";

/// One flow.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// Index into [`Sankey::nodes`].
    pub source: usize,
    /// Index into [`Sankey::nodes`].
    pub target: usize,
    /// How much flows.
    pub value: f64,
}

/// A parsed Sankey diagram.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sankey {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// Node names, in first-appearance order.
    pub nodes: Vec<String>,
    /// The links, in source order.
    pub links: Vec<Link>,
}

/// Whether this source is a Sankey diagram.
pub fn is_sankey(src: &str) -> bool {
    find_header(src, KEYWORDS).is_some()
}

/// Parses a mermaid Sankey diagram.
pub fn parse(src: &str) -> Result<Sankey, ParseError> {
    let Some(source) = find_header(src, KEYWORDS) else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };
    let mut sankey = Sankey::default();
    let mut env = Envelope::new(source.front_matter_title);

    // `start: SANKEY NEWLINE csv` — the keyword has its own line and nothing follows it.
    if !source.header_rest.is_empty() {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: source.header_index + 1,
            text: source.header_rest.clone(),
        });
    }

    // A quoted field may hold a newline (`ESCAPED_TEXT` matches CR and LF), so records are
    // gathered rather than read one line at a time.
    let mut record = String::new();
    let mut record_line = 0usize;
    for (i, line) in source
        .lines
        .iter()
        .enumerate()
        .skip(source.header_index + 1)
    {
        let number = i + 1;
        if record.is_empty() {
            if line.trim().is_empty() {
                continue;
            }
            if env.read(line, TitleSyntax::Terminal) {
                continue;
            }
            record_line = number;
        } else {
            record.push('\n');
        }
        record.push_str(line);
        if quotes_balanced(&record) {
            let fields = split_record(&record);
            read_record(&mut sankey, &fields, record_line)?;
            record.clear();
        }
    }
    if !record.is_empty() {
        return Err(ParseError::Invalid {
            line: record_line,
            message: "a quoted field is never closed".to_string(),
        });
    }

    sankey.preamble = env.preamble;
    if sankey.links.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "link",
        });
    }
    if let Some(cycle) = find_cycle(&sankey) {
        return Err(ParseError::Invalid {
            line: 0,
            message: format!("the flow loops back on itself at `{cycle}`"),
        });
    }
    Ok(sankey)
}

fn read_record(sankey: &mut Sankey, fields: &[String], line: usize) -> Result<(), ParseError> {
    if fields.len() != 3 {
        return Err(ParseError::Invalid {
            line,
            message: format!(
                "a link is written `source,target,value`; this row has {} field{}",
                fields.len(),
                if fields.len() == 1 { "" } else { "s" }
            ),
        });
    }
    let source = node_id(sankey, fields[0].trim());
    let target = node_id(sankey, fields[1].trim());
    let text = fields[2].trim();
    let Some(value) = parse_number(text, true) else {
        return Err(ParseError::BadNumber {
            line,
            text: text.to_string(),
            why: "a link's value must be a number",
        });
    };
    if value < 0.0 {
        return Err(ParseError::BadNumber {
            line,
            text: text.to_string(),
            why: "a link's value must not be negative",
        });
    }
    if source == target {
        return Err(ParseError::Invalid {
            line,
            message: format!("`{}` flows into itself", sankey.nodes[source]),
        });
    }
    sankey.links.push(Link {
        source,
        target,
        value,
    });
    Ok(())
}

/// `findOrCreateNode` — by name, in first-appearance order.
fn node_id(sankey: &mut Sankey, name: &str) -> usize {
    match sankey.nodes.iter().position(|n| n == name) {
        Some(i) => i,
        None => {
            sankey.nodes.push(name.to_string());
            sankey.nodes.len() - 1
        }
    }
}

/// Whether every `"` in `text` has been closed, treating `""` as an escaped quote.
fn quotes_balanced(text: &str) -> bool {
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut open = false;
    while i < b.len() {
        if b[i] == '"' {
            if open && b.get(i + 1) == Some(&'"') {
                i += 2;
                continue;
            }
            open = !open;
        }
        i += 1;
    }
    !open
}

/// One CSV record into its fields, un-doubling `""` inside quoted ones.
fn split_record(record: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = record.chars().collect();
    let mut i = 0;
    let mut quoted = false;
    while i < chars.len() {
        let c = chars[i];
        if quoted {
            if c == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    cur.push('"');
                    i += 2;
                    continue;
                }
                quoted = false;
                i += 1;
                continue;
            }
            cur.push(c);
            i += 1;
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                i += 1;
            }
            ',' => {
                out.push(std::mem::take(&mut cur));
                i += 1;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    out.push(cur);
    out
}

/// The name of a node on a cycle, if the graph has one.
fn find_cycle(sankey: &Sankey) -> Option<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let n = sankey.nodes.len();
    let mut mark = vec![Mark::White; n];
    // (node, how many of its out-edges have been taken) — an explicit stack, because a source
    // with ten thousand rows would blow a recursive walk's stack.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let out: Vec<Vec<usize>> = (0..n)
        .map(|i| {
            sankey
                .links
                .iter()
                .filter(|l| l.source == i)
                .map(|l| l.target)
                .collect()
        })
        .collect();
    for start in 0..n {
        if mark[start] != Mark::White {
            continue;
        }
        mark[start] = Mark::Grey;
        stack.push((start, 0));
        while let Some((node, next)) = stack.pop() {
            if next < out[node].len() {
                stack.push((node, next + 1));
                let to = out[node][next];
                match mark[to] {
                    Mark::Grey => return Some(sankey.nodes[to].clone()),
                    Mark::White => {
                        mark[to] = Mark::Grey;
                        stack.push((to, 0));
                    }
                    Mark::Black => {}
                }
            } else {
                mark[node] = Mark::Black;
            }
        }
    }
    None
}
