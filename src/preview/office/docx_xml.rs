//! A small, budgeted element tree for the Word reader.
//!
//! The Word reader needs context a pure event stream makes awkward (a run's style, a paragraph's
//! numbering, a table cell's span), so it builds a tree -- but **one top-level block at a time**
//! (a paragraph, a table, ...), converts it and drops it. Memory is therefore bounded by the size of
//! one block (`max_nodes`), not by the document. Every reader is an `XmlReader` (depth 256), DTD
//! entities are never expanded, and a block over the node budget is refused as a whole
//! ([`Tree::TooBig`]) rather than half-read.
//!
//! Only the text of `w:t`, `m:t`, `w:instrText` and `w:delText` is kept (other text nodes are layout
//! whitespace). Names are kept as `prefix` + `local`; the converter matches on the local name, which
//! is unambiguous for everything it reads (`m:t` and `w:t` both being `t` is handled where it matters:
//! math is serialised back to XML without being interpreted).

use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::XmlVersion;

use super::fmt_xlsx::{xml_err, XmlReader};
use super::OfficeError;

/// Longest attribute value copied (a description or a hyperlink; anything longer is cut).
const MAX_ATTR_BYTES: usize = 4096;
/// Most attributes kept per element.
const MAX_ATTRS: usize = 48;

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub prefix: String,
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<Kid>,
}

#[derive(Debug, Clone)]
pub(crate) enum Kid {
    N(Node),
    T(String),
}

impl Node {
    /// The value of the attribute whose local name is `local`, whatever its prefix.
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| local_of(k) == local)
            .map(|(_, v)| v.as_str())
    }

    /// A relationship id attribute (`r:id`, `r:embed`, `r:link`, `o:relid`): prefers a
    /// non-`w` prefix, because `w:id` means something else.
    pub fn rel_attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| local_of(k) == local && prefix_of(k) != "w")
            .map(|(_, v)| v.as_str())
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> + Clone {
        self.kids.iter().filter_map(|k| match k {
            Kid::N(n) => Some(n),
            Kid::T(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.nodes().find(|n| n.name == name)
    }

    /// The `w:val` of a property child: `Some(true)` for a bare element or a true value,
    /// `Some(false)` for `0` / `false` / `off` / `none`, `None` when the child is absent.
    pub fn toggle(&self, name: &str) -> Option<bool> {
        let c = self.child(name)?;
        Some(match c.attr("val") {
            None => true,
            Some(v) => !matches!(v.trim(), "0" | "false" | "off" | "none"),
        })
    }

    /// The concatenated text directly inside this element.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for k in &self.kids {
            if let Kid::T(t) = k {
                s.push_str(t);
            }
        }
        s
    }
}

pub(crate) fn local_of(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

fn prefix_of(qname: &str) -> &str {
    qname.split_once(':').map_or("", |(p, _)| p)
}

/// What reading one element produced.
pub(crate) enum Tree {
    Ok(Node),
    /// Over the node or text budget: the element was skipped, the reader is positioned after it.
    TooBig,
}

/// Counts what one tree may hold.
pub(crate) struct Budget {
    pub nodes: usize,
    pub text_bytes: usize,
    /// Text was dropped for lack of budget.
    text_over: bool,
    /// Keep the text of every element (OpenDocument: prose lives directly in `text:p`, `text:span` ..).
    /// Word keeps only the text of the few elements that hold data (`w:t` ..).
    all_text: bool,
}

impl Budget {
    pub fn new(nodes: usize, text_bytes: usize) -> Budget {
        Budget {
            nodes,
            text_bytes,
            text_over: false,
            all_text: false,
        }
    }

    /// A budget for an OpenDocument tree: the text of every element is kept.
    pub fn odf(nodes: usize, text_bytes: usize) -> Budget {
        Budget {
            all_text: true,
            ..Budget::new(nodes, text_bytes)
        }
    }
}

/// Takes the bytes an element's names and attributes hold out of the block's byte budget (the
/// same budget as its text: a document of few elements with 48 attributes of 4 KiB each must not
/// cost more memory than one of text). Returns false once the budget is spent.
fn charge(n: &Node, budget: &mut Budget) -> bool {
    let cost = n.name.len()
        + n.prefix.len()
        + n.attrs
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>();
    if budget.text_bytes < cost {
        budget.text_bytes = 0;
        budget.text_over = true;
        return false;
    }
    budget.text_bytes -= cost;
    true
}

fn start_to_node(e: &BytesStart<'_>) -> Node {
    let qn = e.name();
    let full = String::from_utf8_lossy(qn.as_ref()).into_owned();
    let (prefix, name) = match full.split_once(':') {
        Some((p, n)) => (p.to_string(), n.to_string()),
        None => (String::new(), full),
    };
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false).flatten() {
        if attrs.len() >= MAX_ATTRS {
            break;
        }
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        // `xmlns` declarations are not data.
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let raw = &a.value;
        let val = if raw.len() > MAX_ATTR_BYTES * 4 {
            String::new()
        } else {
            match a.normalized_value(XmlVersion::Implicit1_0) {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(raw).into_owned(),
            }
        };
        attrs.push((key, cut(val, MAX_ATTR_BYTES)));
    }
    Node {
        prefix,
        name,
        attrs,
        kids: Vec::new(),
    }
}

fn cut(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut n = max;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
    s
}

/// Whether the text inside an element of this local name is data (`a:tableStyleId` names the style
/// of a PowerPoint table).
fn keeps_text(name: &str) -> bool {
    matches!(name, "t" | "instrText" | "delText" | "tableStyleId")
}

/// Reads the element whose start tag was just read (`start`; `empty` when it was `<x/>`) through its
/// end tag, within `budget`. After an over-budget element the rest of it is still consumed.
pub(crate) fn read_element<R: BufRead>(
    rd: &mut XmlReader<R>,
    start: &BytesStart<'_>,
    empty: bool,
    budget: &mut Budget,
) -> Result<Tree, OfficeError> {
    let root = start_to_node(start);
    if !charge(&root, budget) {
        if !empty {
            skip_rest(rd)?;
        }
        return Ok(Tree::TooBig);
    }
    if budget.nodes == 0 {
        if !empty {
            skip_rest(rd)?;
        }
        return Ok(Tree::TooBig);
    }
    budget.nodes -= 1;
    if empty {
        return Ok(Tree::Ok(root));
    }
    let mut stack: Vec<Node> = vec![root];
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => {
                // Over budget the tree is of no use: stop building it (what is built so far is
                // dropped) and just read to the end of the element.
                if budget.nodes == 0 {
                    return skip_depth(rd, stack.len() + 1).map(|()| Tree::TooBig);
                }
                budget.nodes -= 1;
                let n = start_to_node(&e);
                if !charge(&n, budget) {
                    return skip_depth(rd, stack.len() + 1).map(|()| Tree::TooBig);
                }
                stack.push(n);
            }
            Event::Empty(e) => {
                if budget.nodes == 0 {
                    return skip_depth(rd, stack.len()).map(|()| Tree::TooBig);
                }
                budget.nodes -= 1;
                let n = start_to_node(&e);
                if !charge(&n, budget) {
                    return skip_depth(rd, stack.len()).map(|()| Tree::TooBig);
                }
                if let Some(top) = stack.last_mut() {
                    top.kids.push(Kid::N(n));
                }
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else {
                    break;
                };
                match stack.last_mut() {
                    Some(parent) => parent.kids.push(Kid::N(done)),
                    None => {
                        return Ok(if budget.text_over {
                            Tree::TooBig
                        } else {
                            Tree::Ok(done)
                        });
                    }
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    if budget.all_text || keeps_text(&top.name) {
                        let s = match t.xml10_content() {
                            Ok(s) => s.into_owned(),
                            Err(_) => String::from_utf8_lossy(&t).into_owned(),
                        };
                        push_text(top, &s, budget);
                    }
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    if budget.all_text || keeps_text(&top.name) {
                        let s = String::from_utf8_lossy(&t).into_owned();
                        push_text(top, &s, budget);
                    }
                }
            }
            Event::GeneralRef(e) => {
                if let Some(top) = stack.last_mut() {
                    if budget.all_text || keeps_text(&top.name) {
                        let mut s = String::new();
                        push_ref(&e, &mut s);
                        push_text(top, &s, budget);
                    }
                }
            }
            Event::Eof => {
                return Err(OfficeError::Corrupt("xml: unexpected end of part".into()));
            }
            _ => {}
        }
    }
    Ok(Tree::TooBig)
}

/// Appends text to the element's last text kid, within the text budget.
fn push_text(top: &mut Node, s: &str, budget: &mut Budget) {
    if s.is_empty() {
        return;
    }
    if budget.text_bytes < s.len() {
        budget.text_bytes = 0;
        budget.text_over = true;
        return;
    }
    budget.text_bytes -= s.len();
    match top.kids.last_mut() {
        Some(Kid::T(t)) => t.push_str(s),
        _ => top.kids.push(Kid::T(s.to_string())),
    }
}

fn push_ref(e: &quick_xml::events::BytesRef<'_>, out: &mut String) {
    let Ok(name) = e.decode() else {
        return;
    };
    if let Some(s) = quick_xml::escape::resolve_xml_entity(&name) {
        out.push_str(s);
    } else if let Ok(Some(c)) = e.resolve_char_ref() {
        out.push(c);
    } else {
        out.push('&');
        out.push_str(&name);
        out.push(';');
    }
}

/// Consumes events to the end tag that closes the element already opened.
pub(crate) fn skip_rest<R: BufRead>(rd: &mut XmlReader<R>) -> Result<(), OfficeError> {
    skip_depth(rd, 1)
}

/// Reads on until `depth` more end tags have been seen.
fn skip_depth<R: BufRead>(rd: &mut XmlReader<R>, depth: usize) -> Result<(), OfficeError> {
    let mut depth = depth;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => return Err(OfficeError::Corrupt("xml: unexpected end of part".into())),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// serialisation (for the math converter)
// ---------------------------------------------------------------------------------------------

const NS_M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The element as XML text, with the `m` and `w` namespaces declared on the root. `None` when it
/// would be longer than `max_bytes`.
pub(crate) fn to_xml(n: &Node, max_bytes: usize) -> Option<String> {
    let mut out = String::new();
    write_node(n, &mut out, true, max_bytes)?;
    Some(out)
}

fn write_node(n: &Node, out: &mut String, root: bool, max: usize) -> Option<()> {
    if out.len() > max {
        return None;
    }
    let qn = if n.prefix.is_empty() {
        n.name.clone()
    } else {
        format!("{}:{}", n.prefix, n.name)
    };
    out.push('<');
    out.push_str(&qn);
    if root {
        out.push_str(&format!(" xmlns:m=\"{NS_M}\" xmlns:w=\"{NS_W}\""));
    }
    for (k, v) in &n.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        escape_into(v, out, true);
        out.push('"');
    }
    if n.kids.is_empty() {
        out.push_str("/>");
        return Some(());
    }
    out.push('>');
    for k in &n.kids {
        match k {
            Kid::N(c) => write_node(c, out, false, max)?,
            Kid::T(t) => escape_into(t, out, false),
        }
        if out.len() > max {
            return None;
        }
    }
    out.push_str("</");
    out.push_str(&qn);
    out.push('>');
    Some(())
}

fn escape_into(s: &str, out: &mut String, attr: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}
