//! Shape names for the `A@{ shape: cyl }` syntax, and the tiny YAML reader that gets at them.
//!
//! Added in mermaid v11.3.0, this is the one piece of flowchart syntax the crate konoma ships
//! today does not know: it turns `A@{ shape: cyl }` into a *second* node, so the diagram grows a
//! box that the author never wrote (`docs/FEATURE-MERMAID-RENDERER.md` §2-3). Recognising the
//! form is therefore not optional even though konoma draws only a handful of the 53 shapes.
//!
//! Two rules from mermaid's `FlowDB::addVertex` are copied exactly, because they are the
//! difference between reading a diagram and inventing one:
//!
//! * a shape name must be lowercase and must not contain `_`, and
//! * an unrecognised name is an **error**, not a fallback.
//!
//! What konoma does differently is only what it *draws*: all 53 names are accepted, and each is
//! folded onto the nearest member of [`Shape`]. §2-4 settles that ("53 形状すべての描き分けは
//! 不要") — the acceptance test for this renderer is that the diagram reads correctly in konoma's
//! own idiom, not that it is pixel-wise GitHub.

use super::model::Shape;

/// Every shape name mermaid accepts, paired with the konoma shape it is drawn as.
///
/// Taken from `packages/mermaid/src/rendering-util/rendering-elements/shapes.ts` (v11.17):
/// each entry's `shortName` plus its `aliases`. Names carrying capitals or an underscore
/// (`squareRect`, `lean_right`, `stateStart`, …) exist in mermaid's map but are rejected by its
/// own validity check before the map is consulted, so they are not listed here.
///
/// The last group is mermaid's *undocumented* internal shapes that happen to pass the lowercase
/// rule. They are accepted for the same reason the other 53 are: refusing a name mermaid takes
/// would send a whole readable diagram to the text fallback.
pub(super) const SHAPE_NAMES: &[(&str, Shape)] = &[
    // Process / rectangle family.
    ("rect", Shape::Rect),
    ("proc", Shape::Rect),
    ("process", Shape::Rect),
    ("rectangle", Shape::Rect),
    ("rounded", Shape::RoundedRect),
    ("event", Shape::RoundedRect),
    ("stadium", Shape::Stadium),
    ("terminal", Shape::Stadium),
    ("pill", Shape::Stadium),
    ("fr-rect", Shape::Subroutine),
    ("subprocess", Shape::Subroutine),
    ("subproc", Shape::Subroutine),
    ("framed-rectangle", Shape::Subroutine),
    ("subroutine", Shape::Subroutine),
    // Storage.
    ("cyl", Shape::Cylinder),
    ("db", Shape::Cylinder),
    ("database", Shape::Cylinder),
    ("cylinder", Shape::Cylinder),
    ("datastore", Shape::Cylinder),
    ("data-store", Shape::Cylinder),
    ("bucket", Shape::Cylinder),
    ("h-cyl", Shape::Cylinder),
    ("das", Shape::Cylinder),
    ("horizontal-cylinder", Shape::Cylinder),
    ("lin-cyl", Shape::Cylinder),
    ("disk", Shape::Cylinder),
    ("lined-cylinder", Shape::Cylinder),
    // Boxes that konoma draws as a plain rectangle.
    ("folder", Shape::Rect),
    ("directory", Shape::Rect),
    ("console", Shape::Rect),
    ("browser", Shape::Rect),
    ("person", Shape::Rect),
    ("bang", Shape::Rect),
    ("notch-rect", Shape::Rect),
    ("card", Shape::Rect),
    ("notched-rectangle", Shape::Rect),
    ("lin-rect", Shape::Rect),
    ("lined-rectangle", Shape::Rect),
    ("lined-process", Shape::Rect),
    ("lin-proc", Shape::Rect),
    ("shaded-process", Shape::Rect),
    ("fork", Shape::Rect),
    ("join", Shape::Rect),
    ("hourglass", Shape::Rect),
    ("collate", Shape::Rect),
    ("brace", Shape::Rect),
    ("comment", Shape::Rect),
    ("brace-l", Shape::Rect),
    ("brace-r", Shape::Rect),
    ("braces", Shape::Rect),
    ("bolt", Shape::Rect),
    ("com-link", Shape::Rect),
    ("lightning-bolt", Shape::Rect),
    ("doc", Shape::Rect),
    ("document", Shape::Rect),
    ("div-rect", Shape::Rect),
    ("div-proc", Shape::Rect),
    ("divided-rectangle", Shape::Rect),
    ("divided-process", Shape::Rect),
    ("tri", Shape::Rect),
    ("extract", Shape::Rect),
    ("triangle", Shape::Rect),
    ("win-pane", Shape::Rect),
    ("internal-storage", Shape::Rect),
    ("window-pane", Shape::Rect),
    ("flip-tri", Shape::Rect),
    ("manual-file", Shape::Rect),
    ("flipped-triangle", Shape::Rect),
    ("sl-rect", Shape::Rect),
    ("manual-input", Shape::Rect),
    ("sloped-rectangle", Shape::Rect),
    ("docs", Shape::Rect),
    ("documents", Shape::Rect),
    ("st-doc", Shape::Rect),
    ("stacked-document", Shape::Rect),
    ("st-rect", Shape::Rect),
    ("procs", Shape::Rect),
    ("processes", Shape::Rect),
    ("stacked-rectangle", Shape::Rect),
    ("bow-rect", Shape::Rect),
    ("stored-data", Shape::Rect),
    ("bow-tie-rectangle", Shape::Rect),
    ("tag-doc", Shape::Rect),
    ("tagged-document", Shape::Rect),
    ("tag-rect", Shape::Rect),
    ("tagged-rectangle", Shape::Rect),
    ("tag-proc", Shape::Rect),
    ("tagged-process", Shape::Rect),
    ("flag", Shape::Rect),
    ("paper-tape", Shape::Rect),
    ("lin-doc", Shape::Rect),
    ("lined-document", Shape::Rect),
    // Rounded.
    ("cloud", Shape::RoundedRect),
    ("delay", Shape::RoundedRect),
    ("half-rounded-rectangle", Shape::RoundedRect),
    // Circles.
    ("circle", Shape::Circle),
    ("circ", Shape::Circle),
    ("sm-circ", Shape::Circle),
    ("start", Shape::Circle),
    ("small-circle", Shape::Circle),
    ("f-circ", Shape::Circle),
    ("junction", Shape::Circle),
    ("filled-circle", Shape::Circle),
    ("cross-circ", Shape::Circle),
    ("summary", Shape::Circle),
    ("crossed-circle", Shape::Circle),
    ("dbl-circ", Shape::DoubleCircle),
    ("double-circle", Shape::DoubleCircle),
    ("doublecircle", Shape::DoubleCircle),
    ("fr-circ", Shape::DoubleCircle),
    ("stop", Shape::DoubleCircle),
    ("framed-circle", Shape::DoubleCircle),
    // Decisions and hexagons.
    ("diam", Shape::Diamond),
    ("decision", Shape::Diamond),
    ("diamond", Shape::Diamond),
    ("question", Shape::Diamond),
    ("hex", Shape::Hexagon),
    ("hexagon", Shape::Hexagon),
    ("prepare", Shape::Hexagon),
    ("notch-pent", Shape::Hexagon),
    ("loop-limit", Shape::Hexagon),
    ("notched-pentagon", Shape::Hexagon),
    // Slanted.
    ("lean-r", Shape::LeanRight),
    ("lean-right", Shape::LeanRight),
    ("in-out", Shape::LeanRight),
    ("lean-l", Shape::LeanLeft),
    ("lean-left", Shape::LeanLeft),
    ("out-in", Shape::LeanLeft),
    ("trap-b", Shape::Trapezoid),
    ("priority", Shape::Trapezoid),
    ("trapezoid-bottom", Shape::Trapezoid),
    ("trapezoid", Shape::Trapezoid),
    ("curv-trap", Shape::Trapezoid),
    ("curved-trapezoid", Shape::Trapezoid),
    ("display", Shape::Trapezoid),
    ("trap-t", Shape::InvTrapezoid),
    ("manual", Shape::InvTrapezoid),
    ("trapezoid-top", Shape::InvTrapezoid),
    ("inv-trapezoid", Shape::InvTrapezoid),
    // Borderless and odd.
    ("text", Shape::Text),
    ("odd", Shape::Odd),
    // Undocumented internals that still pass mermaid's lowercase rule.
    ("state", Shape::Rect),
    ("note", Shape::Rect),
    ("composite", Shape::Rect),
    ("anchor", Shape::Rect),
    ("icon", Shape::Rect),
    ("choice", Shape::Diamond),
];

/// Resolves a `shape:` value, or `None` when mermaid would reject it.
///
/// The two rejection rules are mermaid's, not konoma's: a name that is not already lowercase, or
/// that contains `_`, is refused *before* the table is consulted, which is why `lean_right` is
/// an error while `lean-right` is fine.
///
/// The pre-check is redundant while [`SHAPE_NAMES`] holds only names that pass it — a test pins
/// that — but it is the rule, and it keeps the answer right if a future upstream name
/// (mermaid's own map really does contain `lean_right` and `squareRect`) is ever copied in
/// verbatim.
pub fn resolve_shape(name: &str) -> Option<Shape> {
    if name.contains('_') || name.chars().any(|c| c.is_uppercase()) {
        return None;
    }
    SHAPE_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
}

/// The fields konoma reads out of an `@{ ... }` block.
///
/// mermaid understands a dozen more (`icon`, `img`, `w`, `h`, `constraint`, `animate`, `curve`,
/// …); every one of them is either a rendering option konoma does not implement (§2-5) or edge
/// metadata. They are parsed and dropped rather than rejected — an unknown *key* is not a
/// mermaid error, only an unknown *shape* is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShapeData {
    /// The `shape:` value exactly as written, before validation.
    pub shape: Option<String>,
    /// The `label:` value, quotes already removed.
    pub label: Option<String>,
}

/// Reads the body of an `A@{ ... }` block.
///
/// mermaid feeds this to js-yaml with the JSON schema, after wrapping a single-line body in
/// braces so that `shape: cyl, label: "x"` parses as a flow mapping. That is reproduced here at
/// the size the job actually needs: a flow mapping when the body is one line, a block mapping
/// when it is several, values optionally quoted. Nothing else in the grammar reaches YAML, so a
/// YAML dependency would buy nothing but its own surface.
pub fn parse_shape_data(body: &str) -> ShapeData {
    let body = fold_string_newlines(body);
    let mut data = ShapeData::default();
    let entries: Vec<String> = if body.contains('\n') {
        body.lines().map(|l| l.to_string()).collect()
    } else {
        split_flow(&body)
    };
    for entry in entries {
        let entry = entry.trim();
        let Some((key, value)) = entry.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches('"').trim_matches('\'');
        let value = unquote(value.trim());
        match key {
            "shape" => data.shape = Some(value),
            "label" => data.label = Some(value),
            _ => {}
        }
    }
    data
}

/// mermaid's lexer turns a newline (plus the indent that follows it) inside an `@{ … }` quoted
/// string into `<br/>`, so a multi-line label written across source lines stays one YAML scalar.
/// Doing it here keeps the "is this a flow mapping or a block mapping?" test — which asks
/// whether the body still holds a newline — reading the same text mermaid tests.
fn fold_string_newlines(body: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                    out.push(c);
                    i += 1;
                } else if c == '\n' {
                    out.push_str("<br/>");
                    i += 1;
                    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                        i += 1;
                    }
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Splits a one-line body on the commas that are not inside a quoted value.
fn split_flow(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in body.chars() {
        match quote {
            Some(q) if c == q => {
                quote = None;
                cur.push(c);
            }
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c == ',' => {
                out.push(std::mem::take(&mut cur));
            }
            None => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Removes one matching pair of surrounding quotes, leaving unbalanced ones alone.
fn unquote(v: &str) -> String {
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}
