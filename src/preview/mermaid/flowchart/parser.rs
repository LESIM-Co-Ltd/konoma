//! mermaid `flowchart` / `graph` source in, [`Flowchart`] out.
//!
//! # What this stage is for
//!
//! Three quarters of the mermaid in the wild is a flowchart (`docs/FEATURE-MERMAID-RENDERER.md`
//! §2-1: `graph` + `flowchart` = 75.6% of a 7,081-file scan), so this is the grammar that
//! decides whether konoma's own renderer is useful at all. It produces structure only — no
//! geometry, no SVG. Sizes and coordinates arrive in later stages and read this model.
//!
//! # How faithful it is, and where it is not
//!
//! The reference is mermaid's `flow.jison` plus `flowDb.ts`, not the rendered picture: konoma's
//! acceptance test is "the same source reads correctly in konoma's own idiom" (§0-1), and only
//! the *syntax* is tracked upstream (§0-2 — the grammar grew 10% in two years while the renderer
//! grew 111%). Rules that are not guessable were read out of the grammar and are cited where
//! they are implemented: link length is `line.length - 1` except for dotted links, where it is
//! the number of dots; a shape is decided by its **closing** bracket, so `[/x/]` and `[/x\]` are
//! different shapes; the text inside brackets is its own lexer state, which is why
//! `A[開始、処理。「確認」]` needs no quotes.
//!
//! Four places diverge from mermaid **on purpose**, each because mermaid's behaviour there is a
//! quiet wrong answer rather than a feature:
//!
//! * `direction` is only a direction statement when the statement *starts* with it. mermaid's
//!   lexer matches `.*direction\s+TB[^\n]*`, so `A[go direction TB]` becomes a direction
//!   statement and the node vanishes.
//! * `;` does not split a `style` / `classDef` / `class` / `linkStyle` / `click` / `accTitle` /
//!   `accDescr` line. In mermaid `style A fill:red;stroke:blue` splits, and `stroke:blue`
//!   becomes a node — the exact "phantom node" failure §2-3 is about.
//! * `style X …` and `class X …` do not bring `X` into existence. mermaid's `addVertex` creates
//!   a node for a `style` naming an unknown id (it logs a warning nobody sees), so a typo
//!   silently adds a box.
//! * A subgraph member list is kept in declaration order. mermaid's grammar action builds it
//!   backwards (`$node.concat($vertexStatement.nodes)`), which is an artefact of the LALR
//!   reduction order rather than a decision.
//!
//! Two mermaid behaviours that *look* like bugs are kept, because a diagram author saw them
//! when they wrote the file: `---` followed by an id starting with `o` or `x` reads as `--o` /
//! `--x`, and a top-level `direction` statement is ignored (mermaid pushes it onto a document
//! array that nothing reads; only a subgraph's own `direction` has an effect).

use super::model::{
    Arrow, ClassDef, Direction, Edge, Flowchart, LinkStyle, LinkStyleTarget, Node, ParseError,
    Shape, Stroke, Subgraph,
};
use super::preprocess::preprocess;
use super::shapes::{parse_shape_data, resolve_shape, ShapeData};
use super::text::decode_label;

/// mermaid's own ceiling on how many edges one diagram may declare (`maxEdges`, default 500).
///
/// It is a guard, not a style rule: `A & B & C & … --> X & Y & …` multiplies out, so a few
/// kilobytes of adversarial source can ask for millions of edges. mermaid throws at the limit;
/// konoma stops adding, because a partly drawn 500-edge diagram is still more use in a terminal
/// than no diagram at all.
const MAX_EDGES: usize = 500;

/// Parses a mermaid flowchart.
///
/// # Errors
///
/// Only where drawing would be worse than not drawing — see [`ParseError`]. In particular a
/// diagram whose header is not `flowchart` / `graph`, and a flowchart with no nodes, are both
/// failures: the crate konoma ships today answers the first with three boxes reading "venn",
/// "beta", "sets" and the second with an empty box stretched across the pane.
pub fn parse(src: &str) -> Result<Flowchart, ParseError> {
    let pre = preprocess(src);
    let lines: Vec<&str> = pre.text.split('\n').collect();
    let header = find_header(&lines)?;

    // Rebuild the body with the header's own line blanked out but *kept*, so that a line number
    // in an error message is a line number in the file the user has open.
    let mut body = String::new();
    for _ in 0..header.line_index {
        body.push('\n');
    }
    body.push_str(&header.trailing);
    for line in &lines[header.line_index + 1..] {
        body.push('\n');
        body.push_str(line);
    }
    let body: Vec<char> = body.chars().collect();

    let statements = split_statements(&body)?;
    let mut builder = Builder::new(header.direction, pre.title);
    for stmt in &statements {
        builder.statement(&stmt.chars, stmt.line)?;
    }
    builder.finish()
}

/// Whether this source is a diagram konoma's own renderer owns — i.e. whether its header keyword
/// is `flowchart`, `graph` or `flowchart-elk`.
///
/// This is the routing predicate `preview::markdown::mermaid_to_svg` asks before it decides which
/// renderer to run (stage 1e). It is deliberately *not* "does [`parse`] succeed": a flowchart this
/// parser refuses is still konoma's to answer for, and handing it to a second renderer would
/// let the old crate paint over a bug in the new one — the shape
/// `docs/FEATURE-MERMAID-RENDERER.md` §7 warns about, and the same failure mode as the markdown
/// diff harness that ended up comparing a renderer with itself.
///
/// It runs the same [`preprocess`] and the same `find_header` [`parse`] does, so the two cannot
/// drift apart; [`super::tests`] pins the equivalence over a corpus. Preprocessing runs twice for
/// a flowchart, which is a handful of string passes over a few hundred bytes.
pub fn is_flowchart(src: &str) -> bool {
    let pre = preprocess(src);
    let lines: Vec<&str> = pre.text.split('\n').collect();
    find_header(&lines).is_ok()
}

// ---------------------------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------------------------

struct Header {
    direction: Direction,
    /// 0-based index of the line the header keyword was on.
    line_index: usize,
    /// Whatever followed the direction on that line; mermaid allows `graph TD; A --> B`.
    trailing: String,
}

/// The diagram keywords that mean "this is a flowchart".
///
/// `flowchart-elk` is one of them: it asks for the ELK layout engine, which konoma does not have
/// — and neither does GitHub, which registers only dagre, swimlane and cose-bilkent, so an
/// `flowchart-elk` diagram is already drawn with dagre everywhere it matters (§0-4). Longest
/// first, so that `flowchart` does not claim the `flowchart-elk` prefix.
const FLOWCHART_KEYWORDS: &[&str] = &["flowchart-elk", "flowchart", "graph"];

fn find_header(lines: &[&str]) -> Result<Header, ParseError> {
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let t = line.trim_start();
        for kw in FLOWCHART_KEYWORDS {
            let Some(rest) = t.strip_prefix(kw) else {
                continue;
            };
            // `graphs` is a node called "graphs", not a header: mermaid's lexer would prefer the
            // longer NODE_STRING match.
            if rest.chars().next().is_some_and(is_plain_id_char) {
                continue;
            }
            return Ok(read_direction(rest, i));
        }
        let header: String = t
            .chars()
            .take_while(|c| !c.is_whitespace())
            .take(40)
            .collect();
        return Err(ParseError::NotAFlowchart { header });
    }
    Err(ParseError::Empty)
}

fn read_direction(rest: &str, line_index: usize) -> Header {
    let r = rest.trim_start();
    for name in ["TB", "TD", "BT", "RL", "LR"] {
        let Some(after) = r.strip_prefix(name) else {
            continue;
        };
        if after.chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }
        let direction = Direction::parse(name).unwrap_or_default();
        return Header {
            direction,
            line_index,
            trailing: after.strip_prefix(';').unwrap_or(after).to_string(),
        };
    }
    Header {
        direction: Direction::default(),
        line_index,
        trailing: r.strip_prefix(';').unwrap_or(r).to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Statement splitting
// ---------------------------------------------------------------------------------------------

struct Stmt {
    chars: Vec<char>,
    /// 1-based line of the statement's first non-blank character, in the original source.
    line: usize,
}

/// Statements that must be taken whole even if they contain a `;`.
///
/// See the module docs: splitting `style A fill:red;stroke:blue` on the `;` is how a stylesheet
/// fragment turns into a node.
const UNSPLIT_KEYWORDS: &[&str] = &[
    "style",
    "classDef",
    "class",
    "linkStyle",
    "click",
    "accTitle",
    "accDescr",
];

/// Cuts the body into statements at newlines and `;`.
///
/// Neither is a separator inside a quoted string, inside a shape's brackets, or between the two
/// `|` of an edge label — all three are lexer states in mermaid where the character is ordinary
/// text. A label really may span source lines (`A[one⏎two]` is one node), which is why this
/// cannot simply be a line iterator.
fn split_statements(body: &[char]) -> Result<Vec<Stmt>, ParseError> {
    let mut out: Vec<Stmt> = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut cur_line: Option<usize> = None;
    let mut line = 1usize;
    let mut depth = 0usize;
    let mut in_pipe = false;
    let mut i = 0usize;

    while i < body.len() {
        let c = body[i];
        if c == '"' {
            let end = scan_string(body, i).ok_or(ParseError::UnclosedString { line })?;
            if cur_line.is_none() {
                cur_line = Some(line);
            }
            for ch in &body[i..end] {
                if *ch == '\n' {
                    line += 1;
                }
                cur.push(*ch);
            }
            i = end;
            continue;
        }
        let boundary = depth == 0 && !in_pipe && (c == '\n' || c == ';');
        if boundary && c == ';' && starts_with_unsplit_keyword(&cur) {
            cur.push(c);
            i += 1;
            continue;
        }
        if boundary {
            if let Some(l) = cur_line.take() {
                out.push(Stmt {
                    chars: std::mem::take(&mut cur),
                    line: l,
                });
            }
            cur.clear();
            if c == '\n' {
                line += 1;
            }
            i += 1;
            continue;
        }
        match c {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => in_pipe = !in_pipe,
            '\n' => line += 1,
            _ => {}
        }
        if !c.is_whitespace() && cur_line.is_none() {
            cur_line = Some(line);
        }
        cur.push(c);
        i += 1;
    }

    if depth > 0 {
        return Err(ParseError::UnclosedShape { line });
    }
    if let Some(l) = cur_line {
        out.push(Stmt {
            chars: cur,
            line: l,
        });
    }
    Ok(out)
}

fn starts_with_unsplit_keyword(cur: &[char]) -> bool {
    let s: String = cur.iter().collect();
    let s = s.trim_start();
    UNSPLIT_KEYWORDS.iter().any(|kw| {
        s.strip_prefix(kw).is_some_and(|rest| {
            rest.starts_with(|c: char| c.is_whitespace() || c == ':' || c == '{')
        })
    })
}

/// Consumes a `"…"` (or `"`…`"` markdown) string starting at `i`, returning the index past it.
fn scan_string(body: &[char], i: usize) -> Option<usize> {
    let md = body.get(i + 1) == Some(&'`');
    let mut p = i + 1;
    if md {
        p += 1;
        while p < body.len() {
            if body[p] == '`' && body.get(p + 1) == Some(&'"') {
                return Some(p + 2);
            }
            p += 1;
        }
        return None;
    }
    while p < body.len() {
        if body[p] == '"' {
            return Some(p + 1);
        }
        p += 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------------------------

/// A `subgraph` block whose `end` has not been seen yet.
struct Pending {
    id: Option<String>,
    title: Option<String>,
    /// True for the `subgraph One Two` form, where mermaid uses the same text for both and then
    /// throws the id away if it holds whitespace.
    id_is_title: bool,
    members: Vec<String>,
    direction: Option<Direction>,
}

struct Builder {
    chart: Flowchart,
    stack: Vec<Pending>,
    /// mermaid's `subCount`: bumped for *every* subgraph, named or not, and used to name the
    /// unnamed ones. So the generated ids depend on how many named blocks closed first.
    sub_count: usize,
}

impl Builder {
    fn new(direction: Direction, title: Option<String>) -> Self {
        Builder {
            chart: Flowchart::new(direction, title),
            stack: Vec::new(),
            sub_count: 0,
        }
    }

    fn finish(mut self) -> Result<Flowchart, ParseError> {
        // A `subgraph` that never met its `end` is closed here rather than rejected: the block
        // is readable, only its extent was left implicit.
        while !self.stack.is_empty() {
            self.close_subgraph();
        }
        self.prune_subgraph_nodes();
        if self.chart.nodes.is_empty() {
            return Err(ParseError::NoNodes);
        }
        Ok(self.chart)
    }

    /// Drops nodes whose id names a subgraph.
    ///
    /// `subgraph one … end` followed by `one --> b` uses the block as an edge endpoint, which is
    /// legal (§2-2). The edge is recorded against the id either way; what must not survive is a
    /// *box* called `one` sitting next to the block of the same name — which is what a plain
    /// `addVertex` leaves behind when the block is declared after the edge.
    fn prune_subgraph_nodes(&mut self) {
        if self.chart.subgraphs.is_empty() {
            return;
        }
        let ids: Vec<String> = self.chart.subgraphs.iter().map(|s| s.id.clone()).collect();
        if !self.chart.nodes.iter().any(|n| ids.contains(&n.id)) {
            return;
        }
        let kept: Vec<Node> = std::mem::take(&mut self.chart.nodes)
            .into_iter()
            .filter(|n| !ids.contains(&n.id))
            .collect();
        self.chart.nodes = kept;
        self.chart.reindex();
    }

    /// Strips any leading `end` keywords (each closing a block) and dispatches what is left.
    ///
    /// Written as a loop rather than as recursion on purpose: `end` is the one keyword that does
    /// not consume the rest of its line, so a statement of ten thousand `end`s would otherwise
    /// be ten thousand stack frames.
    fn statement(&mut self, text: &[char], line: usize) -> Result<(), ParseError> {
        let mut cur: Vec<char> = trim_slice(text).to_vec();
        loop {
            if cur.is_empty() {
                return Ok(());
            }
            let s: String = cur.iter().collect();
            let Some(rest) = keyword(&s, "end") else {
                return self.dispatch(&cur, &s, line);
            };
            self.close_subgraph();
            cur = trim_slice(&rest.chars().collect::<Vec<char>>()).to_vec();
        }
    }

    fn dispatch(&mut self, text: &[char], s: &str, line: usize) -> Result<(), ParseError> {
        if let Some(rest) = keyword(s, "subgraph") {
            self.begin_subgraph(rest);
            return Ok(());
        }
        if let Some(dir) = direction_statement(s) {
            // Only a subgraph's own `direction` has an effect; see the module docs.
            if let Some(p) = self.stack.last_mut() {
                p.direction = Some(dir);
            }
            return Ok(());
        }
        if let Some(rest) = keyword(s, "classDef") {
            self.class_def(rest);
            return Ok(());
        }
        if let Some(rest) = keyword(s, "class") {
            self.class_statement(rest);
            return Ok(());
        }
        if let Some(rest) = keyword(s, "style") {
            self.style_statement(rest);
            return Ok(());
        }
        if let Some(rest) = keyword(s, "linkStyle") {
            self.link_style(rest);
            return Ok(());
        }
        if keyword(s, "click").is_some() {
            // Interactivity has no meaning in a terminal (§2-5). Recognised so the line cannot
            // be mistaken for a node declaration, then dropped.
            return Ok(());
        }
        if let Some(v) = acc_value(s, "accTitle") {
            self.chart.acc_title = Some(v);
            return Ok(());
        }
        if let Some(v) = acc_value(s, "accDescr") {
            self.chart.acc_descr = Some(v);
            return Ok(());
        }

        let ids = self.vertex_statement(text, line)?;
        if let Some(p) = self.stack.last_mut() {
            p.members.extend(ids);
        }
        Ok(())
    }

    // -- subgraphs --------------------------------------------------------------------------

    fn begin_subgraph(&mut self, rest: &str) {
        let rest = rest.trim();
        if rest.is_empty() {
            self.stack.push(Pending {
                id: None,
                title: None,
                id_is_title: false,
                members: Vec::new(),
                direction: None,
            });
            return;
        }
        let chars: Vec<char> = rest.chars().collect();
        if let Some(open) = find_title_bracket(&chars) {
            let id: String = decode_label(&chars[..open].iter().collect::<String>());
            let title = match scan_delimited(&chars, open + 1, "]", '[') {
                Some((raw, _)) => decode_label(&raw),
                None => decode_label(&chars[open + 1..].iter().collect::<String>()),
            };
            self.stack.push(Pending {
                id: Some(id),
                title: Some(title),
                id_is_title: false,
                members: Vec::new(),
                direction: None,
            });
            return;
        }
        let text = decode_label(rest);
        self.stack.push(Pending {
            id: Some(text.clone()),
            title: Some(text),
            id_is_title: true,
            members: Vec::new(),
            direction: None,
        });
    }

    /// Registers the innermost pending subgraph. A stray `end` is ignored rather than fatal.
    fn close_subgraph(&mut self) {
        let Some(p) = self.stack.pop() else {
            return;
        };
        let mut id = p.id.as_ref().map(|s| s.trim().to_string());
        // `subgraph One Two` names the block and titles it with the same text; mermaid throws
        // the id away when that text holds whitespace, so the block gets a generated id and the
        // words become the title.
        if p.id_is_title {
            if let Some(t) = &p.title {
                if t.chars().any(char::is_whitespace) {
                    id = None;
                }
            }
        }
        if id.as_deref() == Some("") {
            id = None;
        }
        let auto_id = id.is_none();
        let id = id.unwrap_or_else(|| format!("subGraph{}", self.sub_count));
        self.sub_count += 1;

        let mut members: Vec<String> = Vec::new();
        for m in p.members {
            if m.trim().is_empty() || members.contains(&m) {
                continue;
            }
            // mermaid's `makeUniq`: a node already claimed by a block that closed earlier — that
            // is, by a *nested* block — does not appear again in this one.
            if self
                .chart
                .subgraphs
                .iter()
                .any(|sg| sg.members.iter().any(|x| x == &m))
            {
                continue;
            }
            members.push(m);
        }

        let title = p.title.unwrap_or_default().trim().to_string();
        self.chart.subgraphs.push(Subgraph {
            id: id.clone(),
            title,
            members,
            direction: p.direction,
            classes: Vec::new(),
            auto_id,
        });
        if let Some(parent) = self.stack.last_mut() {
            parent.members.push(id);
        }
    }

    // -- statements that are parsed and dropped ---------------------------------------------

    fn class_def(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let Some((names, styles)) = rest.split_once(char::is_whitespace) else {
            return;
        };
        let styles = split_styles(styles);
        for name in names.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            self.chart.class_defs.push(ClassDef {
                name: name.to_string(),
                styles: styles.clone(),
            });
        }
    }

    fn class_statement(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let Some((ids, class_name)) = rest.split_once(char::is_whitespace) else {
            return;
        };
        let class_name = class_name.trim();
        if class_name.is_empty() {
            return;
        }
        for id in ids.split(',') {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            // Applied only to things that exist: a `class` naming a typo must not conjure a box.
            if let Some(n) = self.chart.node_mut(id) {
                n.classes.push(class_name.to_string());
            }
            if let Some(e) = self.chart.edge_mut(id) {
                e.classes.push(class_name.to_string());
            }
            if let Some(sg) = self.chart.subgraph_mut(id) {
                sg.classes.push(class_name.to_string());
            }
        }
    }

    fn style_statement(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return;
        };
        let styles = split_styles(styles);
        if let Some(n) = self.chart.node_mut(id.trim()) {
            n.styles.extend(styles);
        }
    }

    fn link_style(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let mut parts = rest.splitn(2, char::is_whitespace);
        let Some(head) = parts.next() else {
            return;
        };
        let tail = parts.next().unwrap_or("").trim();
        let target = if head.eq_ignore_ascii_case("default") {
            LinkStyleTarget::Default
        } else {
            let idx: Vec<usize> = head
                .split(',')
                .filter_map(|n| n.trim().parse::<usize>().ok())
                .collect();
            if idx.is_empty() {
                return;
            }
            LinkStyleTarget::Indices(idx)
        };
        let (interpolate, styles) = match tail.strip_prefix("interpolate") {
            Some(after) => {
                let after = after.trim_start();
                let mut it = after.splitn(2, char::is_whitespace);
                let curve = it.next().unwrap_or("").to_string();
                let styles = it.next().unwrap_or("").trim();
                (Some(curve), split_styles(styles))
            }
            None => (None, split_styles(tail)),
        };
        self.chart.link_styles.push(LinkStyle {
            target,
            styles,
            interpolate,
        });
    }

    // -- vertex statements ------------------------------------------------------------------

    /// Parses `A --> B & C -.-> D`, registering nodes and edges, and returns the ids it saw so
    /// that an enclosing subgraph can claim them.
    fn vertex_statement(&mut self, text: &[char], line: usize) -> Result<Vec<String>, ParseError> {
        let mut seen: Vec<String> = Vec::new();
        let mut prev: Option<Vec<String>> = None;
        let mut pending: Option<LinkInfo> = None;
        let mut i = 0usize;

        loop {
            i = skip_ws(text, i);
            if i >= text.len() {
                break;
            }
            let group = self.node_group(text, &mut i, line)?;
            if group.is_empty() {
                // Nothing at this position reads as a node; the rest of the statement is junk
                // (a dangling `-->`, a stray `&`). Keeping what came before beats failing.
                break;
            }
            for id in &group {
                if !seen.contains(id) {
                    seen.push(id.clone());
                }
            }
            if let (Some(from), Some(link)) = (prev.take(), pending.take()) {
                self.add_links(&from, &group, &link);
            }
            prev = Some(group);

            i = skip_ws(text, i);
            if i >= text.len() {
                break;
            }
            let Some((info, next)) = read_link(text, i) else {
                break;
            };
            pending = Some(info);
            i = next;
        }
        Ok(seen)
    }

    /// `A`, or `A & B & C`.
    fn node_group(
        &mut self,
        text: &[char],
        i: &mut usize,
        line: usize,
    ) -> Result<Vec<String>, ParseError> {
        let mut ids = Vec::new();
        while let Some(id) = self.vertex(text, i, line)? {
            ids.push(id);
            let after = skip_ws(text, *i);
            if text.get(after) != Some(&'&') {
                break;
            }
            *i = skip_ws(text, after + 1);
        }
        Ok(ids)
    }

    /// One vertex: an id, an optional shape, an optional `:::class`, an optional `@{ … }`.
    fn vertex(
        &mut self,
        text: &[char],
        i: &mut usize,
        line: usize,
    ) -> Result<Option<String>, ParseError> {
        let (id, mut p) = scan_id(text, *i);
        if id.is_empty() {
            return Ok(None);
        }

        let mut label = None;
        let mut shape = None;
        if let Some((opener, after)) = shape_open(text, p) {
            let Some((raw, end, sh)) = scan_shape(text, after, opener) else {
                return Err(ParseError::UnclosedShape { line });
            };
            label = Some(decode_label(&raw));
            shape = Some(sh);
            p = end;
        }

        let mut class_name = None;
        if starts_with(text, p, ":::") {
            let (name, end) = scan_id(text, p + 3);
            if !name.is_empty() {
                class_name = Some(name);
                p = end;
            }
        }

        let mut data = None;
        if text.get(p) == Some(&'@') && text.get(p + 1) == Some(&'{') {
            let Some((raw, end)) = scan_shape_data(text, p + 2) else {
                return Err(ParseError::UnclosedShapeData { line });
            };
            data = Some(parse_shape_data(&raw));
            p = end;
        }

        *i = p;
        if !self.apply_vertex(&id, label, shape, class_name, data, line)? {
            return Ok(None);
        }
        Ok(Some(id))
    }

    fn apply_vertex(
        &mut self,
        id: &str,
        label: Option<String>,
        shape: Option<Shape>,
        class_name: Option<String>,
        data: Option<ShapeData>,
        line: usize,
    ) -> Result<bool, ParseError> {
        // `e1@{ animate: true }` and `sub1@{ view: collapsed }` reuse the node syntax to attach
        // metadata to an edge or a block. Routing them here is what stops `e1` from becoming a
        // box; mermaid checks the subgraph first, then the edge.
        if data.is_some() && (self.chart.subgraph(id).is_some() || self.chart.edge(id).is_some()) {
            return Ok(false);
        }

        // Validate before interning: an unknown shape must not leave a half-built node behind.
        let resolved = match data.as_ref().and_then(|d| d.shape.as_deref()) {
            Some(name) => match resolve_shape(name) {
                Some(s) => Some(s),
                None => {
                    return Err(ParseError::UnknownShape {
                        name: name.to_string(),
                        line,
                    })
                }
            },
            None => None,
        };

        self.chart.intern_node(id);
        let data_label = data.and_then(|d| d.label);
        let Some(node) = self.chart.node_mut(id) else {
            return Ok(false);
        };
        if let Some(l) = label {
            node.label = l;
        }
        if let Some(s) = shape {
            node.shape = s;
        }
        if let Some(c) = class_name {
            node.classes.push(c);
        }
        if let Some(s) = resolved {
            node.shape = s;
        }
        if let Some(l) = data_label {
            node.label = decode_label(&l);
        }
        Ok(true)
    }

    /// The cartesian product `A & B --> C & D` produces four edges.
    fn add_links(&mut self, from: &[String], to: &[String], link: &LinkInfo) {
        for (si, s) in from.iter().enumerate() {
            for (ti, t) in to.iter().enumerate() {
                // A group form gives the author-written id to one edge only: the one between the
                // last source and the first target.
                let id = if si + 1 == from.len() && ti == 0 {
                    link.id.clone()
                } else {
                    None
                };
                self.add_single_link(s, t, link, id);
            }
        }
    }

    fn add_single_link(&mut self, from: &str, to: &str, link: &LinkInfo, id: Option<String>) {
        if self.chart.edges.len() >= MAX_EDGES {
            return;
        }
        let (id, user_defined_id) = match id {
            Some(id) if self.chart.edge(&id).is_none() => (id, true),
            _ => {
                let existing = self
                    .chart
                    .edges
                    .iter()
                    .filter(|e| e.from == from && e.to == to)
                    .count();
                // mermaid's counter: 0 for the first edge of a pair, then `n + 1`, so the second
                // edge is `_2`. Reproduced rather than tidied because the id is user-visible via
                // `linkStyle` and `class`.
                let counter = if existing == 0 { 0 } else { existing + 1 };
                (format!("L_{from}_{to}_{counter}"), false)
            }
        };
        self.chart.edges.push(Edge {
            id,
            user_defined_id,
            from: from.to_string(),
            to: to.to_string(),
            arrow: link.arrow,
            stroke: link.stroke,
            length: link.length.min(10),
            label: link.label.clone(),
            classes: Vec::new(),
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------------------------

/// Reads a link, with the edge id that may sit in front of it (`A e1@--> B`).
fn read_link(text: &[char], i: usize) -> Option<(LinkInfo, usize)> {
    if let Some(r) = parse_link(text, i) {
        return Some(r);
    }
    // Only treat `e1@` as an edge id when a link really follows: an `@` inside an id (or a stray
    // one) must not silently eat the token.
    let (id, after) = scan_link_id(text, i)?;
    let (mut info, next) = parse_link(text, after)?;
    info.id = Some(id);
    Some((info, next))
}

/// One parsed link, before it is multiplied over the node groups on either side.
#[derive(Debug, Clone)]
struct LinkInfo {
    arrow: Arrow,
    stroke: Stroke,
    length: usize,
    label: Option<String>,
    id: Option<String>,
}

/// Which of the four link alphabets a token belongs to. mermaid keeps a separate lexer state per
/// alphabet, which is why `-- text ==>` cannot work: the `==>` never terminates the `--` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Normal,
    Thick,
    Dotted,
    Invisible,
}

fn parse_link(text: &[char], i: usize) -> Option<(LinkInfo, usize)> {
    let full = match_full_link(text, i);
    let start = match_start_link(text, i);
    // Longest match wins, exactly as the lexer does: `-->` is a whole link, `-- ` is the opening
    // half of a labelled one.
    let use_full = match (full, start) {
        (Some((fe, _)), Some((se, _))) => fe >= se,
        (Some(_), None) => true,
        _ => false,
    };

    if use_full {
        let (end, _) = full?;
        let core: String = text[i..end].iter().collect();
        let (arrow, stroke, length) = destruct_end_link(&core);
        let mut p = skip_ws(text, end);
        let mut label = None;
        if text.get(p) == Some(&'|') {
            let (raw, end) = scan_pipe_label(text, p + 1);
            let decoded = decode_label(&raw);
            label = (!decoded.is_empty()).then_some(decoded);
            p = end;
        }
        return Some((
            LinkInfo {
                arrow,
                stroke,
                length,
                label,
                id: None,
            },
            p,
        ));
    }

    let (start_end, family) = start?;
    let start_core: String = text[i..start_end].iter().collect();
    let (raw, link_start, link_end) = scan_edge_text(text, start_end, family)?;
    let end_core: String = text[link_start..link_end].iter().collect();
    let (arrow, stroke, length) = destruct_link(&end_core, &start_core);
    let decoded = decode_label(&raw);
    Some((
        LinkInfo {
            arrow,
            stroke,
            length,
            label: (!decoded.is_empty()).then_some(decoded),
            id: None,
        },
        link_end,
    ))
}

/// `\s*[xo<]?\-\-+[-xo>]\s*` and its thick, dotted and invisible siblings.
fn match_full_link(text: &[char], i: usize) -> Option<(usize, Family)> {
    let mut best: Option<(usize, Family)> = None;
    for m in [
        match_dashed(text, i, '-').map(|e| (e, Family::Normal)),
        match_dashed(text, i, '=').map(|e| (e, Family::Thick)),
        match_dotted(text, i).map(|e| (e, Family::Dotted)),
        match_invisible(text, i).map(|e| (e, Family::Invisible)),
    ]
    .into_iter()
    .flatten()
    {
        if best.is_none_or(|(b, _)| m.0 > b) {
            best = Some(m);
        }
    }
    best
}

/// `[xo<]?` then two or more `c`, then one of `-xo>` (where the run's own last character may
/// serve as the `-`).
fn match_dashed(text: &[char], i: usize, c: char) -> Option<usize> {
    let mut p = i;
    if matches!(text.get(p), Some('x') | Some('o') | Some('<')) {
        p += 1;
    }
    let run_start = p;
    while text.get(p) == Some(&c) {
        p += 1;
    }
    let run = p - run_start;
    if run < 2 {
        return None;
    }
    if matches!(text.get(p), Some('x') | Some('o') | Some('>')) {
        return Some(p + 1);
    }
    // `---`: the third dash is the `[-]` of the pattern, so two dashes alone are not a link.
    (run >= 3).then_some(p)
}

/// `[xo<]?\-?\.+\-[xo>]?`
fn match_dotted(text: &[char], i: usize) -> Option<usize> {
    let mut p = i;
    if matches!(text.get(p), Some('x') | Some('o') | Some('<')) {
        p += 1;
    }
    if text.get(p) == Some(&'-') {
        p += 1;
    }
    let dots_start = p;
    while text.get(p) == Some(&'.') {
        p += 1;
    }
    if p == dots_start {
        return None;
    }
    if text.get(p) != Some(&'-') {
        return None;
    }
    p += 1;
    if matches!(text.get(p), Some('x') | Some('o') | Some('>')) {
        p += 1;
    }
    Some(p)
}

/// `\~\~[\~]+` — three or more tildes.
fn match_invisible(text: &[char], i: usize) -> Option<usize> {
    let mut p = i;
    while text.get(p) == Some(&'~') {
        p += 1;
    }
    (p - i >= 3).then_some(p)
}

/// The opening half of a labelled link: `[xo<]?--`, `[xo<]?==`, `[xo<]?-.`
fn match_start_link(text: &[char], i: usize) -> Option<(usize, Family)> {
    let mut p = i;
    if matches!(text.get(p), Some('x') | Some('o') | Some('<')) {
        p += 1;
    }
    match (text.get(p), text.get(p + 1)) {
        (Some('-'), Some('-')) => Some((p + 2, Family::Normal)),
        (Some('='), Some('=')) => Some((p + 2, Family::Thick)),
        (Some('-'), Some('.')) => Some((p + 2, Family::Dotted)),
        _ => None,
    }
}

/// Collects the text of a `-- like this -->` link and locates the closing token.
///
/// Only the same family closes the state, which is what makes `x-- a ==>` unparseable rather
/// than a mixed link.
fn scan_edge_text(text: &[char], from: usize, family: Family) -> Option<(String, usize, usize)> {
    let mut p = from;
    let mut raw = String::new();
    while p < text.len() {
        if text[p] == '"' {
            let end = scan_string(text, p)?;
            raw.extend(&text[p..end]);
            p = end;
            continue;
        }
        if let Some((end, f)) = match_full_link(text, p) {
            if f == family {
                return Some((raw, p, end));
            }
        }
        raw.push(text[p]);
        p += 1;
    }
    None
}

/// mermaid's `destructEndLink`: the arrow comes from the last character, the stroke and the
/// length from what is left.
fn destruct_end_link(core: &str) -> (Arrow, Stroke, usize) {
    let s = core.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return (Arrow::None, Stroke::Normal, 0);
    }
    let last = chars[chars.len() - 1];
    let mut line: Vec<char> = chars[..chars.len() - 1].to_vec();
    let first = chars[0];
    let mut arrow = match last {
        'x' => Arrow::Cross,
        '>' => Arrow::Point,
        'o' => Arrow::Circle,
        _ => Arrow::None,
    };
    let doubled = matches!((last, first), ('x', 'x') | ('>', '<') | ('o', 'o'));
    if doubled && !line.is_empty() {
        arrow = match arrow {
            Arrow::Cross => Arrow::DoubleCross,
            Arrow::Point => Arrow::DoublePoint,
            Arrow::Circle => Arrow::DoubleCircle,
            other => other,
        };
        line.remove(0);
    }
    let mut stroke = Stroke::Normal;
    if line.first() == Some(&'=') {
        stroke = Stroke::Thick;
    }
    if line.first() == Some(&'~') {
        stroke = Stroke::Invisible;
    }
    let mut length = line.len().saturating_sub(1);
    let dots = line.iter().filter(|c| **c == '.').count();
    if dots > 0 {
        stroke = Stroke::Dotted;
        length = dots;
    }
    (arrow, stroke, length)
}

/// mermaid's `destructStartLink`, folded into `destructLink`: the two halves of a labelled link
/// must agree on the stroke, and on the arrow unless the opening half has none.
fn destruct_link(end_core: &str, start_core: &str) -> (Arrow, Stroke, usize) {
    let (end_arrow, end_stroke, length) = destruct_end_link(end_core);
    let s = start_core.trim();
    let mut rest: Vec<char> = s.chars().collect();
    let start_arrow = match rest.first() {
        Some('<') => {
            rest.remove(0);
            Arrow::Point
        }
        Some('x') => {
            rest.remove(0);
            Arrow::Cross
        }
        Some('o') => {
            rest.remove(0);
            Arrow::Circle
        }
        _ => Arrow::None,
    };
    let start_stroke = if rest.contains(&'.') {
        Stroke::Dotted
    } else if rest.contains(&'=') {
        Stroke::Thick
    } else {
        Stroke::Normal
    };
    if start_stroke != end_stroke {
        return (Arrow::Invalid, Stroke::Invalid, length);
    }
    let arrow = if start_arrow == Arrow::None {
        end_arrow
    } else if start_arrow != end_arrow {
        return (Arrow::Invalid, Stroke::Invalid, length);
    } else {
        match start_arrow {
            Arrow::Point => Arrow::DoublePoint,
            Arrow::Cross => Arrow::DoubleCross,
            Arrow::Circle => Arrow::DoubleCircle,
            other => other,
        }
    };
    (arrow, end_stroke, length)
}

/// `|text|` after a link. An unclosed `|` takes the rest of the statement rather than failing.
fn scan_pipe_label(text: &[char], from: usize) -> (String, usize) {
    let mut p = from;
    let mut raw = String::new();
    while p < text.len() {
        if text[p] == '"' {
            if let Some(end) = scan_string(text, p) {
                raw.extend(&text[p..end]);
                p = end;
                continue;
            }
        }
        if text[p] == '|' {
            return (raw, p + 1);
        }
        raw.push(text[p]);
        p += 1;
    }
    (raw, p)
}

/// `[^\s"]+@` where the `@` is not followed by `{` or `"` — mermaid's LINK_ID token. The regex
/// is greedy, so the *last* `@` of the run is the one that counts.
fn scan_link_id(text: &[char], i: usize) -> Option<(String, usize)> {
    let mut end = i;
    while end < text.len() && !text[end].is_whitespace() && text[end] != '"' {
        end += 1;
    }
    let at = (i..end)
        .rev()
        .find(|&p| text[p] == '@' && !matches!(text.get(p + 1), Some('{') | Some('"') | None))?;
    if at == i {
        return None;
    }
    Some((text[i..at].iter().collect(), at + 1))
}

// ---------------------------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------------------------

/// The bracket a vertex opened with. The *shape* is not decided yet — for the trapezoid family
/// it depends on the closing bracket (§2-6 rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    DoubleCircle,
    Circle,
    Stadium,
    Round,
    Subroutine,
    Cylinder,
    Trap,
    InvTrap,
    Square,
    Hexagon,
    Diamond,
    Odd,
}

/// Longest first, so `([` is a stadium and not a round shape containing `[`.
const OPENERS: &[(&str, Opener)] = &[
    ("(((", Opener::DoubleCircle),
    ("([", Opener::Stadium),
    ("((", Opener::Circle),
    ("(", Opener::Round),
    ("[[", Opener::Subroutine),
    ("[(", Opener::Cylinder),
    ("[/", Opener::Trap),
    ("[\\", Opener::InvTrap),
    ("[", Opener::Square),
    ("{{", Opener::Hexagon),
    ("{", Opener::Diamond),
    (">", Opener::Odd),
];

fn shape_open(text: &[char], i: usize) -> Option<(Opener, usize)> {
    for (tok, opener) in OPENERS {
        if starts_with(text, i, tok) {
            return Some((*opener, i + tok.chars().count()));
        }
    }
    None
}

/// Reads a shape's body and decides its shape from the closing bracket.
fn scan_shape(text: &[char], from: usize, opener: Opener) -> Option<(String, usize, Shape)> {
    if matches!(opener, Opener::Trap | Opener::InvTrap) {
        let (raw, end, closer) = scan_trapezoid(text, from)?;
        // §2-6 rule 3: the *closing* bracket picks the shape. `[/x/]` is a parallelogram and
        // `[/x\\]` a trapezoid, from the same opening bracket.
        let leaning = closer == if opener == Opener::Trap { '/' } else { '\\' };
        let shape = match (opener, leaning) {
            (Opener::Trap, true) => Shape::LeanRight,
            (Opener::Trap, false) => Shape::Trapezoid,
            (_, true) => Shape::LeanLeft,
            (_, false) => Shape::InvTrapezoid,
        };
        return Some((raw, end, shape));
    }
    let (close, nest, shape) = match opener {
        Opener::DoubleCircle => (")))", '(', Shape::DoubleCircle),
        Opener::Circle => ("))", '(', Shape::Circle),
        Opener::Stadium => ("])", '[', Shape::Stadium),
        Opener::Round => (")", '(', Shape::RoundedRect),
        Opener::Subroutine => ("]]", '[', Shape::Subroutine),
        Opener::Cylinder => (")]", '(', Shape::Cylinder),
        Opener::Square => ("]", '[', Shape::Rect),
        Opener::Hexagon => ("}}", '{', Shape::Hexagon),
        Opener::Diamond => ("}", '{', Shape::Diamond),
        Opener::Odd => ("]", '[', Shape::Odd),
        Opener::Trap | Opener::InvTrap => unreachable!("handled above"),
    };
    let (raw, end) = scan_delimited(text, from, close, nest)?;
    Some((raw, end, shape))
}

/// Scans to `close`, skipping quoted strings and balanced `nest` pairs.
fn scan_delimited(text: &[char], from: usize, close: &str, nest: char) -> Option<(String, usize)> {
    let mirror = match nest {
        '(' => ')',
        '[' => ']',
        _ => '}',
    };
    let mut depth = 0usize;
    let mut raw = String::new();
    let mut p = from;
    while p < text.len() {
        if text[p] == '"' {
            let end = scan_string(text, p)?;
            raw.extend(&text[p..end]);
            p = end;
            continue;
        }
        if text[p] == nest {
            depth += 1;
        } else if text[p] == mirror {
            if depth > 0 {
                depth -= 1;
            } else if starts_with(text, p, close) {
                return Some((raw, p + close.chars().count()));
            }
        } else if depth == 0 && starts_with(text, p, close) {
            return Some((raw, p + close.chars().count()));
        }
        raw.push(text[p]);
        p += 1;
    }
    None
}

/// `[/ … /]`, `[/ … \]`, `[\ … /]`, `[\ … \]` — returns the character that closed it.
fn scan_trapezoid(text: &[char], from: usize) -> Option<(String, usize, char)> {
    let mut raw = String::new();
    let mut p = from;
    while p < text.len() {
        if text[p] == '"' {
            let end = scan_string(text, p)?;
            raw.extend(&text[p..end]);
            p = end;
            continue;
        }
        if (text[p] == '/' || text[p] == '\\') && text.get(p + 1) == Some(&']') {
            return Some((raw, p + 2, text[p]));
        }
        raw.push(text[p]);
        p += 1;
    }
    None
}

/// The body of `A@{ … }`. `}` inside a quoted value does not close it.
fn scan_shape_data(text: &[char], from: usize) -> Option<(String, usize)> {
    let mut raw = String::new();
    let mut p = from;
    while p < text.len() {
        match text[p] {
            '"' | '\'' => {
                let quote = text[p];
                raw.push(quote);
                p += 1;
                while p < text.len() && text[p] != quote {
                    raw.push(text[p]);
                    p += 1;
                }
                if p >= text.len() {
                    return None;
                }
                raw.push(quote);
                p += 1;
            }
            '}' => return Some((raw, p + 1)),
            c => {
                raw.push(c);
                p += 1;
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

/// The `[` that opens a `subgraph id [Title]` — the first one outside a quoted string.
fn find_title_bracket(text: &[char]) -> Option<usize> {
    let mut p = 0;
    while p < text.len() {
        match text[p] {
            '"' => p = scan_string(text, p)?,
            '[' => return Some(p),
            _ => p += 1,
        }
    }
    None
}

fn skip_ws(text: &[char], mut i: usize) -> usize {
    while i < text.len() && text[i].is_whitespace() {
        i += 1;
    }
    i
}

fn trim_slice(text: &[char]) -> &[char] {
    let mut start = 0;
    let mut end = text.len();
    while start < end && text[start].is_whitespace() {
        start += 1;
    }
    while end > start && text[end - 1].is_whitespace() {
        end -= 1;
    }
    &text[start..end]
}

fn starts_with(text: &[char], i: usize, needle: &str) -> bool {
    (i..)
        .zip(needle.chars())
        .all(|(p, c)| text.get(p) == Some(&c))
}

/// True for a character that can only ever be part of an identifier, used to reject `graphs` as
/// a header keyword.
fn is_plain_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Reads an identifier.
///
/// mermaid assembles one out of NODE_STRING, NUM, COLON, COMMA, AMP, BRKT, MULT and UNICODE_TEXT
/// tokens, so an id may hold `:` `,` `#` `*` `.` and a great deal else. Three characters are
/// context-dependent and are what stop an id from swallowing the link that follows it:
/// `-` unless a `-`, `>` or `.` comes next, `=` unless another `=` does, and `&` unless a space
/// does (which is what makes `A & B` two nodes and `A&B` one).
///
/// konoma accepts any non-ASCII character here, where mermaid's UNICODE_TEXT rule lists specific
/// blocks. Being more permissive can only turn a diagram mermaid rejects into one konoma draws;
/// it can never invent a node in a diagram that parses.
fn scan_id(text: &[char], from: usize) -> (String, usize) {
    let mut out = String::new();
    let mut p = from;
    while p < text.len() {
        let c = text[p];
        if c.is_whitespace() {
            break;
        }
        match c {
            '[' | ']' | '(' | ')' | '{' | '}' | '|' | '"' | ';' | '>' | '<' | '@' | '~' => break,
            ':' => {
                if starts_with(text, p, ":::") {
                    break;
                }
                out.push(c);
                p += 1;
            }
            '-' => {
                if matches!(text.get(p + 1), Some('-') | Some('>') | Some('.') | None) {
                    break;
                }
                out.push(c);
                p += 1;
            }
            '=' => {
                if text.get(p + 1) == Some(&'=') {
                    break;
                }
                out.push(c);
                p += 1;
            }
            '&' => {
                if text.get(p + 1).is_none_or(|n| n.is_whitespace()) {
                    break;
                }
                out.push(c);
                p += 1;
            }
            _ => {
                out.push(c);
                p += 1;
            }
        }
    }
    (out, p)
}

/// `kw` followed by whitespace or end of statement, returning what follows.
fn keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(kw)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        _ => None,
    }
}

/// `direction LR` and friends — but only when the statement *starts* with the word.
fn direction_statement(s: &str) -> Option<Direction> {
    let rest = keyword(s.trim(), "direction")?;
    let word: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect();
    Direction::parse(&word)
}

/// `accTitle: text`, `accDescr: text` and the `accDescr { … }` block form.
fn acc_value(s: &str, kw: &str) -> Option<String> {
    let rest = s.strip_prefix(kw)?;
    let rest = rest.trim_start();
    if let Some(v) = rest.strip_prefix(':') {
        return Some(v.trim().to_string());
    }
    let v = rest.strip_prefix('{')?;
    Some(v.trim_end().trim_end_matches('}').trim().to_string())
}

/// Splits a style list on `,`, honouring mermaid's `\,` escape for a literal comma.
///
/// `pub(crate)`: the ER, treemap and quadrant parsers reuse this rather than re-implementing the
/// escape rule, now that they all keep a `classDef`'s declarations for `render::style` to read.
pub(crate) fn split_styles(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&',') {
            chars.next();
            cur.push(',');
            continue;
        }
        if c == ',' {
            let t = cur.trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            cur.clear();
            continue;
        }
        cur.push(c);
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        out.push(t);
    }
    out
}
