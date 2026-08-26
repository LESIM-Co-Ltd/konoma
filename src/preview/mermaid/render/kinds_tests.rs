//! What stage 5b's eleven languages are checked with.
//!
//! # One module for eleven kinds, and why that is not a shortcut
//!
//! The eleven do not share a *layout* — three go through [`lay_out_spec`], two are placed on a
//! grid, four are banded, one is a lane diagram and one is a sequence diagram in disguise. What
//! they share is everything a test can state: the same [`Diagram`], the same twelve geometric
//! invariants, the same main instrument, the same golden format. Splitting that into eleven files
//! would mean eleven copies of the same six functions, and the failure mode konoma keeps hitting
//! is a copy that stopped matching its original.
//!
//! [`lay_out_spec`]: super::lay_out_spec
//!
//! # Which of the twelve shared invariants apply
//!
//! | # | invariant | applies to |
//! |---|---|---|
//! | 1 | nodes do not overlap | **the graph and grid kinds** — mindmap, requirement, C4, architecture, block, git graph. Not the banded ones: a journey's face sits above its own task box on purpose, and a gantt bar and its row label share a row. |
//! | 2 | edges stay out of shapes | **the graph kinds**. Not the grid or banded ones: an architecture line crosses the cell between two boxes by design, and a gantt grid line runs the whole height of the chart, which is what a grid line is. |
//! | 3 | labels ride their edge | **every kind that puts a label on an edge** — requirement, C4, block, architecture. |
//! | 4 | endpoints land on an outline | **the graph, grid and block kinds**. Not the git graph, whose lines join dot centres so a merge's two lines meet at a point. |
//! | 5 | the viewBox contains everything | **all eleven**, and it is the one that catches a clipped box — which is the crate's own requirement-diagram defect. |
//! | 6–11 | the six cluster invariants | **every kind that draws a frame** — C4, architecture, journey, timeline, gantt, kanban, block. |
//! | 12 | panels stay inside their box | **requirement, C4, kanban**. |
//!
//! Each is *called*, never copied — the rule stages 3, 4 and 5a settled.

use std::collections::HashSet;

use crate::preview::markdown::mermaid_to_svg_reason;
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::shapes::{Glyph, Mark};
use super::tests::{
    assert_snapshot, check_endpoints_land_on_the_outline, check_labels_ride_their_edge,
    check_nested_clusters_sit_inside_their_parent, check_nodes_do_not_overlap,
    check_panels_stay_inside_their_box, check_unrelated_clusters_do_not_overlap,
    check_view_box_contains_everything, mask_numbers, measured_texts, tree_of,
};
use super::{svg, Diagram, Label, RenderError};

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

/// The diagrams every structural test here runs over.
///
/// Built from **each language's own documentation page** — `mindmap.md`, `timeline.md`,
/// `userJourney.md`, `kanban.md`, `architecture.md`, `requirementDiagram.md`, `gitgraph.md`,
/// `c4.md`, `block.md`, `gantt.md`, `zenuml.md` at tag `mermaid@11.17.2` — plus the case
/// distinctions each grammar makes that no example happens to show. Not from a list of bugs
/// already found (memory `corpus-from-spec-not-from-bugs`).
pub const CASES: &[(&str, &str)] = &[
    // --- mindmap -----------------------------------------------------------------------------
    (
        "mindmap",
        "mindmap\n  root((mindmap))\n    Origins\n      Long history\n      Popularisation\n        \
         British popular psychology author Tony Buzan\n    Research\n      On effectiveness<br/>and \
         features\n      On Automatic creation\n    Tools\n      Pen and paper\n      Mermaid",
    ),
    (
        "mindmap-shapes",
        "mindmap\n  Root\n    a[square]\n    b(rounded)\n    c((circle))\n    d))bang((\n    \
         e)cloud(\n    f{{hexagon}}\n    plain",
    ),
    (
        // `getType` reads **both** brackets: `(` closed by `)` is a rounded rectangle and `(`
        // closed by anything else is a cloud. No documentation example shows the second, so the
        // corpus has to.
        "mindmap-open-cloud",
        "mindmap\n  Root\n    a(cloud(\n    b(rounded)",
    ),
    (
        "mindmap-cjk",
        "mindmap\n  root((全画面プレビュー))\n    ツリー\n      種別を解決\n    窓読み",
    ),
    // --- kanban ------------------------------------------------------------------------------
    (
        "kanban",
        "kanban\n  Todo\n    [Create Documentation]\n    docs[Create Blog about the new diagram]\n  \
         id7[In progress]\n    id6[Create renderer so that it works in all cases]\n  id9[Ready for \
         deploy]\n    id8[Design grammar]@{ assigned: 'knsv' }\n",
    ),
    (
        "kanban-meta",
        "kanban\n  Backlog\n    id1[Ticket]@{ assigned: 'knsv', ticket: MC-2038, priority: 'Very \
         High' }\n    id2[Another]@{ priority: 'Low' }\n  Done\n",
    ),
    // --- journey -----------------------------------------------------------------------------
    (
        "journey",
        "journey\n    title My working day\n    section Go to work\n      Make tea: 5: Me\n      Go \
         upstairs: 3: Me\n      Do work: 1: Me, Cat\n    section Go home\n      Go downstairs: 5: \
         Me\n      Sit down: 5: Me",
    ),
    (
        "journey-no-section",
        "journey\n  Wake: 4\n  Sleep: 2: Me, You",
    ),
    // --- timeline ----------------------------------------------------------------------------
    (
        "timeline",
        "timeline\n    title History of Social Media Platform\n    2002 : LinkedIn\n    2004 : \
         Facebook : Google\n    2005 : YouTube\n    2006 : Twitter",
    ),
    (
        "timeline-sections",
        "timeline\n    title Timeline of Industrial Revolution\n    section 17th-20th century\n      \
         Industry 1.0 : Machinery, Water power, Steam power\n      Industry 2.0 : Electricity, \
         Internal combustion engine\n    section 21st century\n      Industry 4.0 : Internet, \
         Robotics",
    ),
    (
        "timeline-td",
        "timeline TD\n  title MermaidChart 2023 Timeline\n  section 2023 Q1\n    Bullet 1 : \
         sub-point 1a : sub-point 1b\n    Bullet 2 : sub-point 2a\n  section 2023 Q2\n    Bullet 3 \
         : sub-point 3a",
    ),
    // --- gantt -------------------------------------------------------------------------------
    (
        "gantt",
        "gantt\n    title A Gantt Diagram\n    dateFormat YYYY-MM-DD\n    section Section\n        A \
         task          :a1, 2014-01-01, 30d\n        Another task    :after a1, 20d\n    section \
         Another\n        Task in Another :2014-01-12, 12d\n        another task    :24d",
    ),
    (
        "gantt-tags",
        "gantt\n  dateFormat YYYY-MM-DD\n  axisFormat %d/%m\n  section Design\n    done task :done, \
         d1, 2024-01-01, 5d\n    active task :active, d2, after d1, 5d\n    critical :crit, d3, \
         after d2, 3d\n    a milestone :milestone, m1, after d3, 0d",
    ),
    (
        "gantt-weekends",
        "gantt\n  dateFormat YYYY-MM-DD\n  excludes weekends\n  section S\n    long :2024-01-01, 10d",
    ),
    // --- requirement -------------------------------------------------------------------------
    (
        "requirement",
        "requirementDiagram\n\n  requirement test_req {\n  id: 1\n  text: the test text.\n  risk: \
         high\n  verifymethod: test\n  }\n\n  functionalRequirement test_req2 {\n  id: 1.1\n  text: \
         the second test text.\n  risk: low\n  verifymethod: inspection\n  }\n\n  element \
         test_entity {\n  type: simulation\n  }\n\n  test_entity - satisfies -> test_req2\n  \
         test_req - contains -> test_req2",
    ),
    (
        "requirement-directions",
        "requirementDiagram\n  direction LR\n  requirement a {\n  id: 1\n  }\n  requirement b {\n  \
         id: 2\n  }\n  a <- derives - b",
    ),
    // --- gitgraph ----------------------------------------------------------------------------
    (
        "gitgraph",
        "gitGraph\n   commit\n   commit\n   branch develop\n   checkout develop\n   commit\n   \
         commit\n   checkout main\n   merge develop\n   commit\n   commit",
    ),
    (
        "gitgraph-types",
        "gitGraph\n   commit id: \"Normal\" tag: \"v1.0.0\"\n   commit\n   commit id: \"Reverse\" \
         type: REVERSE tag: \"RC_1\"\n   commit id: \"Highlight\" type: HIGHLIGHT",
    ),
    (
        "gitgraph-cherry-pick",
        "gitGraph\n    commit id: \"ZERO\"\n    branch develop\n    branch release\n    commit \
         id:\"A\"\n    checkout main\n    commit id:\"ONE\"\n    checkout develop\n    commit \
         id:\"B\"\n    checkout main\n    merge develop id:\"MERGE\"\n    commit id:\"TWO\"\n    \
         checkout release\n    cherry-pick id:\"MERGE\" parent:\"B\"",
    ),
    (
        "gitgraph-tb",
        "gitGraph TB:\n  commit\n  branch develop\n  commit\n  checkout main\n  merge develop",
    ),
    // --- c4 ----------------------------------------------------------------------------------
    (
        "c4-context",
        "C4Context\n  title System Context diagram for Internet Banking System\n  \
         Enterprise_Boundary(b0, \"BankBoundary0\") {\n    Person(customerA, \"Banking Customer \
         A\", \"A customer of the bank.\")\n    System(SystemAA, \"Internet Banking System\", \
         \"Allows customers to view information.\")\n  }\n  System_Ext(SystemC, \"E-mail system\", \
         \"The internal Microsoft Exchange system.\")\n  Rel(customerA, SystemAA, \"Uses\")\n  \
         Rel(SystemAA, SystemC, \"Sends e-mails\", \"SMTP\")",
    ),
    (
        "c4-container",
        "C4Container\n  title Containers\n  Container_Boundary(c1, \"Internet Banking\") {\n    \
         Container(web_app, \"Web Application\", \"Java, Spring MVC\", \"Delivers the content.\")\n \
         ContainerDb(database, \"Database\", \"SQL Database\", \"Stores information.\")\n    \
         ContainerQueue(queue, \"Events\", \"Kafka\")\n  }\n  Rel(web_app, database, \"Reads from \
         and writes to\", \"JDBC\")",
    ),
    (
        "c4-deployment",
        "C4Deployment\n  Deployment_Node(dn, \"Big Bank plc\", \"Ubuntu 16.04 LTS\") {\n    \
         Deployment_Node(inner, \"bigbank-api\", \"Docker\") {\n      Container(api, \"API \
         Application\", \"Java\")\n    }\n  }\n  Person(user, \"Customer\")\n  Rel(user, api, \
         \"Uses\", \"HTTPS\")",
    ),
    // --- block -------------------------------------------------------------------------------
    (
        "block",
        "block-beta\ncolumns 1\n  db((\"DB\"))\n  blockArrowId6<[\"  \"]>(down)\n  block:ID\n    A\n \
         B[\"A wide one in the middle\"]\n    C\n  end\n  space\n  D\n  ID --> D\n  C --> D",
    ),
    (
        "block-columns",
        "block-beta\n  columns 3\n  a[\"A label\"] b:2 c:2 d\n  e f g",
    ),
    (
        // A `columns` count that has to wrap, with nothing else in the way: the row a block lands
        // on is what `columns` *means*.
        "block-wrap",
        "block-beta\n  columns 2\n  a b c d e",
    ),
    (
        "block-arrows",
        "block-beta\n  blockArrowId<[\"right\"]>(right)\n  blockArrowId2<[\"left\"]>(left)\n  \
         blockArrowId5<[\"both\"]>(x)",
    ),
    (
        "block-link",
        "block-beta\n  columns 5\n  a b c d e\n  a --> e\n  a-- \"labelled\" -->c",
    ),
    // --- architecture ------------------------------------------------------------------------
    (
        "architecture",
        "architecture-beta\n    group api(cloud)[API]\n\n    service db(database)[Database] in api\n \
         service disk1(disk)[Storage] in api\n    service disk2(disk)[Storage] in api\n    service \
         server(server)[Server] in api\n\n    db:L -- R:server\n    disk1:T -- B:server\n    \
         disk2:T -- B:db",
    ),
    (
        "architecture-groups",
        "architecture-beta\n    group sources(cloud)[Sources]\n        service src_a(server)[Source A] \
         in sources\n        service src_b(server)[Source B] in sources\n\n    group \
         storage(database)[Storage]\n        service db_one(database)[DB One] in storage\n        \
         service db_two(database)[DB Two] in storage\n\n    src_a:B --> T:db_one\n    src_b:B --> \
         T:db_two",
    ),
    (
        "architecture-junction",
        "architecture-beta\n    service left_disk(disk)[Disk]\n    service top_disk(disk)[Disk]\n    \
         service bottom_disk(disk)[Disk]\n    junction junctionCenter\n\n    left_disk:R -- \
         L:junctionCenter\n    top_disk:B -- T:junctionCenter\n    bottom_disk:T -- B:junctionCenter",
    ),
    (
        "architecture-align",
        "architecture-beta\n    service src1(server)[Source 1]\n    service src2(server)[Source 2]\n \
         service proc(server)[Processor]\n    src1:B --> T:proc\n    src2:B --> T:proc\n    align \
         row src1 src2",
    ),
    // --- zenuml ------------------------------------------------------------------------------
    (
        "zenuml",
        "zenuml\n    title Demo\n    Alice->John: Hello John, how are you?\n    John->Alice: \
         Great!\n    Alice->John: See you later!",
    ),
    (
        "zenuml-sync",
        "zenuml\n    Client->A.method() {\n      B.method() {\n        return x1\n      }\n      \
         return x2\n    }",
    ),
    (
        "zenuml-blocks",
        "zenuml\n  @Actor Alice\n  Alice->Bob: Hello Bob, how are you?\n  if(is_sick) {\n    \
         Bob->Alice: Not so good\n  } else {\n    Bob->Alice: Fresh as a daisy\n  }\n  while(true) \
         {\n    Alice->Bob: again\n  }",
    ),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Every case of one kind, **found by that kind's own routing predicate**.
///
/// The `cases_of` pattern §6-A item 10 settled: a hard-coded list of case names stops covering the
/// corpus the moment somebody adds a case, and that is how a radar mutation survived twice.
pub fn cases_of(is_ours: fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    let found: Vec<(&str, &str)> = CASES
        .iter()
        .filter(|(_, src)| is_ours(src))
        .copied()
        .collect();
    assert!(!found.is_empty(), "the corpus has no case of this kind");
    found
}

/// Lays a source out, whichever of the eleven it is.
pub fn laid_out(src: &str) -> Diagram {
    dispatch(src).unwrap_or_else(|e| panic!("corpus source must lay out: {e}\n{src}"))
}

fn dispatch(src: &str) -> Result<Diagram, RenderError> {
    use super as draw;
    use crate::preview::mermaid as m;
    if m::mindmap::is_mindmap(src) {
        return draw::mindmap::lay_out(&m::mindmap::parse(src)?);
    }
    if m::kanban::is_kanban(src) {
        return draw::kanban::lay_out(&m::kanban::parse(src)?);
    }
    if m::journey::is_journey(src) {
        return draw::journey::lay_out(&m::journey::parse(src)?);
    }
    if m::timeline::is_timeline(src) {
        return draw::timeline::lay_out(&m::timeline::parse(src)?);
    }
    if m::gantt::is_gantt(src) {
        return draw::gantt::lay_out(&m::gantt::parse(src)?);
    }
    if m::requirement::is_requirement_diagram(src) {
        return draw::requirement::lay_out(&m::requirement::parse(src)?);
    }
    if m::gitgraph::is_git_graph(src) {
        return draw::gitgraph::lay_out(&m::gitgraph::parse(src)?);
    }
    if m::c4::is_c4(src) {
        return draw::c4::lay_out(&m::c4::parse(src)?);
    }
    if m::block::is_block_diagram(src) {
        return draw::block::lay_out(&m::block::parse(src)?);
    }
    if m::architecture::is_architecture(src) {
        return draw::architecture::lay_out(&m::architecture::parse(src)?);
    }
    if m::zenuml::is_zenuml(src) {
        return draw::zenuml::lay_out(src);
    }
    panic!("no stage 5b parser claims this source")
}

/// The rendered SVG, through the production entry point.
fn rendered(src: &str) -> String {
    mermaid_to_svg_reason(src, "dark").unwrap_or_else(|e| panic!("{e}\n{src}"))
}

/// Which languages a case belongs to, for the invariant tables below.
fn is_graph_kind(src: &str) -> bool {
    use crate::preview::mermaid as m;
    m::mindmap::is_mindmap(src) || m::requirement::is_requirement_diagram(src) || m::c4::is_c4(src)
}

fn is_grid_kind(src: &str) -> bool {
    use crate::preview::mermaid as m;
    m::architecture::is_architecture(src) || m::block::is_block_diagram(src)
}

fn is_banded_kind(src: &str) -> bool {
    use crate::preview::mermaid as m;
    m::journey::is_journey(src)
        || m::timeline::is_timeline(src)
        || m::gantt::is_gantt(src)
        || m::kanban::is_kanban(src)
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// Label strings the instrument runs over — every category §6 names, plus the double space.
const LABELS: &[(&str, &str)] = &[
    ("ascii", "Revenue"),
    ("kerning", "AVATAR To Wa"),
    ("thin", "illicit lilli"),
    ("cjk", "全画面プレビュー"),
    ("cjk-punct", "開始、処理。「確認」"),
    ("emoji", "build 🚀 ship"),
    ("digits", "0123456789"),
    ("mixed", "konoma の preview を全画面 fullscreen で"),
    ("double-space", "loop  Every minute"),
];

/// **The instrument §6 asks for, for every new kind of label stage 5b draws.**
///
/// A [`Glyph::ChartLabel`]'s box **is** its ink, so the relation is the sharp one — the number
/// konoma sized the box to and the number resvg laid the glyphs out at are the *same number*, with
/// nothing in between that could hide an error. The reference is usvg's own
/// `Text::bounding_box().width()`, read out of the document konoma emitted, so this is konoma
/// against **resvg's real shaper** and can never decay into konoma against konoma.
#[test]
fn every_new_label_kind_is_drawn_at_the_width_konoma_measured() {
    if !text_metrics::fonts_available() {
        eprintln!("no sans-serif face — skipping (the renderer refuses to draw here too)");
        return;
    }
    // (what kind of label this is, a source that puts `{}` into one).
    let kinds: &[(&str, &str)] = &[
        ("journey people", "journey\n  Task: 3: {}\n"),
        ("journey title", "journey\n  title {}\n  Task: 3: Me\n"),
        ("timeline title", "timeline\n  title {}\n  a : b\n"),
        // An `axisFormat` copies everything that is not a `%` directive through verbatim, so
        // this is how arbitrary text gets into a tick label.
        (
            "gantt tick",
            "gantt\n  dateFormat YYYY-MM-DD\n  axisFormat {}\n  a :2024-01-01, 3d\n",
        ),
        (
            "gantt row name",
            "gantt\n  dateFormat YYYY-MM-DD\n  {} :2024-01-01, 3d\n",
        ),
        (
            "gantt title",
            "gantt\n  title {}\n  dateFormat YYYY-MM-DD\n  a :2024-01-01, 3d\n",
        ),
        (
            "gitgraph branch",
            "gitGraph\n  commit\n  branch {}\n  commit\n",
        ),
        ("gitgraph caption", "gitGraph\n  commit tag: \"{}\"\n"),
    ];

    let mut checked = 0usize;
    for (kind, template) in kinds {
        for (case, text) in LABELS {
            // A gantt row name is `[^:\n]+`, so a colon in it would end the name; a git branch is
            // a `REFERENCE`, which has no spaces. Both are the grammar's rule, not a limitation
            // of the instrument, so those rows skip the texts they cannot carry.
            if kind.starts_with("gitgraph branch") && text.contains(' ') {
                continue;
            }
            if kind.starts_with("architecture edge") && text.contains(']') {
                continue;
            }
            let src = template.replace("{}", text);
            let d = match dispatch(&src) {
                Ok(d) => d,
                Err(e) => panic!("{kind}/{case}: {e}\n{src}"),
            };
            let drawn_text = text_metrics::collapse_spaces(text).into_owned();
            let node = d
                .nodes
                .iter()
                .filter(|n| n.shape == Glyph::ChartLabel)
                .find(|n| n.label.lines.join(" ") == drawn_text)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: no label reading {drawn_text:?}; the labels are {:?}",
                        d.nodes
                            .iter()
                            .filter(|n| n.shape == Glyph::ChartLabel)
                            .map(|n| n.label.lines.join(" "))
                            .collect::<Vec<_>>()
                    )
                });

            let svg = rendered(&src);
            let mut texts = Vec::new();
            measured_texts(tree_of(&svg).root(), &mut texts);
            let drawn = texts
                .iter()
                .find(|(s, _)| *s == drawn_text)
                .map(|(_, w)| *w)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: resvg drew no <text> reading {drawn_text:?} — a dropped \
                         label means the font resolution konoma measured with is not the one \
                         resvg drew with. It drew: {:?}",
                        texts.iter().map(|(s, _)| s).collect::<Vec<_>>()
                    )
                });
            assert!(
                (node.size.w - drawn).abs() <= 1.0,
                "{kind}/{case} {text:?}: konoma sized the box {} wide and resvg drew the text {} \
                 wide — a label's box *is* its ink, so these are one number",
                svg::num(node.size.w),
                svg::num(drawn)
            );
            checked += 1;
        }
    }
    assert!(checked >= 60, "only {checked} label measurements ran");
    eprintln!("{checked} stage-5b label measurements against resvg");
}

/// The same instrument for the labels that live **inside a box** rather than in a bare
/// [`Glyph::ChartLabel`] — a mindmap node, a timeline event, a block, an architecture service.
///
/// The relation is the padded one: the box is the ink plus what `shapes::size` adds, so what is
/// checked is that the ink still *fits*, with the slack it was given. A measurement that drifts
/// makes the words overflow their box, which is what this catches.
#[test]
fn a_boxed_label_still_fits_the_box_konoma_sized_for_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let kinds: &[(&str, &str)] = &[
        ("mindmap node", "mindmap\n  root\n    {}\n"),
        ("timeline period", "timeline\n  {} : an event\n"),
        ("timeline event", "timeline\n  a period : {}\n"),
        ("block", "block-beta\n  a[\"{}\"]\n"),
        (
            "architecture service",
            "architecture-beta\n  service a(x)[{}]\n",
        ),
        ("kanban card", "kanban\n  Todo\n    c[{}]\n"),
        ("journey task", "journey\n  {}: 3: Me\n"),
    ];
    let mut checked = 0usize;
    for (kind, template) in kinds {
        for (case, text) in LABELS {
            if kind.starts_with("journey task") && text.contains(':') {
                continue;
            }
            if (kind.starts_with("block") || kind.starts_with("kanban")) && text.contains(']') {
                continue;
            }
            let src = template.replace("{}", text);
            let d = match dispatch(&src) {
                Ok(d) => d,
                Err(e) => panic!("{kind}/{case}: {e}\n{src}"),
            };
            let drawn_text = text_metrics::collapse_spaces(text).into_owned();
            let svg = rendered(&src);
            let mut texts = Vec::new();
            measured_texts(tree_of(&svg).root(), &mut texts);
            let drawn = texts
                .iter()
                .find(|(s, _)| *s == drawn_text)
                .map(|(_, w)| *w)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: resvg drew no <text> reading {drawn_text:?}. It drew: {:?}",
                        texts.iter().map(|(s, _)| s).collect::<Vec<_>>()
                    )
                });
            // Whatever holds those words — a node's own label, or a row of its panel.
            let holder = d
                .nodes
                .iter()
                .find(|n| {
                    n.label.lines.join(" ") == drawn_text
                        || n.panel.as_ref().is_some_and(|p| {
                            p.rows
                                .iter()
                                .flat_map(|r| &r.cells)
                                .any(|c| c.label.lines.join(" ") == drawn_text)
                        })
                })
                .unwrap_or_else(|| {
                    panic!("{kind}/{case}: nothing on the page holds {drawn_text:?}")
                });
            assert!(
                holder.size.w + 0.5 >= drawn,
                "{kind}/{case} {text:?}: resvg drew {} of ink in a box konoma made {} wide",
                svg::num(drawn),
                svg::num(holder.size.w)
            );
            checked += 1;
        }
    }
    assert!(checked >= 50, "only {checked} box measurements ran");
}

/// The same instrument for the labels that ride an **edge**.
///
/// An edge label's box is its ink plus [`super::LABEL_PAD_X`] on each side — the patch drawn
/// behind it — so the relation here is exact in the other direction: box minus padding *is* the
/// ink. Stated separately from the two above because it is a different number, and a test that
/// allowed either would catch neither.
#[test]
fn every_new_edge_label_is_drawn_at_the_width_konoma_measured() {
    if !text_metrics::fonts_available() {
        return;
    }
    let kinds: &[(&str, &str)] = &[
        (
            "architecture edge",
            "architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  a:R -[{}]- L:b\n",
        ),
        ("block link", "block-beta\n  a space b\n  a-- \"{}\" -->b\n"),
        (
            "c4 rel",
            "C4Context\n  System(a, \"A\")\n  System(b, \"B\")\n  Rel(a, b, \"{}\")\n",
        ),
    ];
    let mut checked = 0usize;
    for (kind, template) in kinds {
        for (case, text) in LABELS {
            if text.contains(']') || text.contains('"') {
                continue;
            }
            let src = template.replace("{}", text);
            let d = match dispatch(&src) {
                Ok(d) => d,
                Err(e) => panic!("{kind}/{case}: {e}\n{src}"),
            };
            let drawn_text = text_metrics::collapse_spaces(text).into_owned();
            let label = d
                .edges
                .iter()
                .filter_map(|e| e.label.as_ref())
                .find(|l| l.label.lines.join(" ") == drawn_text)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: no edge label reading {drawn_text:?}; the labels are {:?}",
                        d.edges
                            .iter()
                            .filter_map(|e| e.label.as_ref())
                            .map(|l| l.label.lines.join(" "))
                            .collect::<Vec<_>>()
                    )
                });
            let svg = rendered(&src);
            let mut texts = Vec::new();
            measured_texts(tree_of(&svg).root(), &mut texts);
            let drawn = texts
                .iter()
                .find(|(s, _)| *s == drawn_text)
                .map(|(_, w)| *w)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: resvg drew no <text> reading {drawn_text:?}. It drew: {:?}",
                        texts.iter().map(|(s, _)| s).collect::<Vec<_>>()
                    )
                });
            let slack = label.size.w - drawn;
            assert!(
                (slack - super::LABEL_PAD_X * 2.0).abs() <= 1.0,
                "{kind}/{case} {text:?}: the patch behind the words is {} wider than the ink, and \
                 the padding is {}",
                svg::num(slack),
                svg::num(super::LABEL_PAD_X * 2.0)
            );
            checked += 1;
        }
    }
    assert!(checked >= 18, "only {checked} edge-label measurements ran");
}

// ---------------------------------------------------------------------------------------------
// 2. The shared invariants, per kind
// ---------------------------------------------------------------------------------------------

/// (5) The viewBox holds everything — **all eleven kinds**.
///
/// This is the one that catches the crate's requirement-diagram defect (a box whose top is
/// clipped), so it runs over the whole corpus with no exceptions at all.
#[test]
fn nothing_is_drawn_outside_the_view_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        check_view_box_contains_everything(name, &laid_out(src));
    }
}

/// (12) A panel and the box drawn round it are the same box — requirement, C4, kanban.
#[test]
fn every_panel_stays_inside_its_own_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        check_panels_stay_inside_their_box(name, &laid_out(src));
    }
}

/// (1) Two boxes do not overlap — the graph and grid kinds, plus the git graph.
///
/// Not the banded ones, and the reason is in the module docs: a journey's face is *meant* to sit
/// over its task, and a gantt bar shares its row with the row's name.
#[test]
fn boxes_do_not_overlap_where_they_are_not_meant_to() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        if !(is_graph_kind(src) || is_grid_kind(src)) {
            continue;
        }
        check_nodes_do_not_overlap(name, &laid_out(src));
    }
}

/// (3) and (4): a label sits on its own line, and a line ends on an outline.
#[test]
fn edge_labels_ride_their_line_and_ends_land_on_an_outline() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        if !(is_graph_kind(src) || is_grid_kind(src)) {
            continue;
        }
        let d = laid_out(src);
        check_labels_ride_their_edge(name, &d);
        check_endpoints_land_on_the_outline(name, &d);
    }
}

/// (7) and (8): a nested frame sits inside its parent, and two unrelated frames do not overlap.
#[test]
fn frames_nest_and_do_not_collide() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        if d.clusters.is_empty() {
            continue;
        }
        check_nested_clusters_sit_inside_their_parent(name, &d);
        check_unrelated_clusters_do_not_overlap(name, &d);
    }
}

/// The two of the twelve that cannot apply anywhere here, asserted rather than assumed.
#[test]
fn no_stage_5b_kind_grows_a_lifeline_except_the_sequence_dialect() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let zen = crate::preview::mermaid::zenuml::is_zenuml(src);
        assert_eq!(
            !d.lifelines.is_empty(),
            zen,
            "{name}: only a ZenUML diagram has lifelines, because only it is a sequence diagram"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The invariants that are about these kinds
// ---------------------------------------------------------------------------------------------

/// **A gantt bar's width is its duration, through one scale.**
///
/// The statement that makes the crate's "axis to 2058" impossible, and it is written **scale
/// free**: the whole drawing spans the chart's whole extent, so a bar's share of the drawing has
/// to equal its task's share of the extent. Nothing here recomputes the plot's width, so the test
/// cannot agree with the renderer by copying it.
#[test]
fn a_gantt_bars_width_is_its_own_duration() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gantt;
    for (name, src) in cases_of(gantt::is_gantt) {
        let model = gantt::parse(src).expect("parses");
        let d = laid_out(src);
        let (from, to) = model.extent().expect("tasks");
        let span = (to.millis() - from.millis()) as f64;
        assert!(span > 0.0, "{name}: no extent");

        // How wide the whole time axis came out, read off the drawing: the earliest task starts
        // at one end of it and the latest ends at the other, by definition of the extent.
        //
        // A **milestone** contributes its centre, not its box: it is a moment drawn as a diamond
        // straddling its own date, so its outline reaches half a diamond past the instant it
        // marks — which is a fact about the mark and not about the time axis.
        let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
        for (i, task) in model.tasks.iter().enumerate() {
            let Some(node) = d.node(&format!("task#{i}")) else {
                continue;
            };
            let (l, r) = if task.tags.milestone {
                (node.center.x, node.center.x)
            } else {
                let (l, _, r, _) = node.bounds();
                (l, r)
            };
            left = left.min(l);
            right = right.max(r);
        }
        let plot = right - left;
        assert!(plot > 0.0, "{name}: the chart has no width");

        for (i, task) in model.tasks.iter().enumerate() {
            let node = d
                .node(&format!("task#{i}"))
                .unwrap_or_else(|| panic!("{name}: task {i} was not drawn"));
            if task.tags.milestone {
                assert_eq!(
                    node.shape,
                    Glyph::Flow(crate::preview::mermaid::flowchart::Shape::Diamond),
                    "{name}: a milestone is a moment, so it is not a bar"
                );
                continue;
            }
            let share = (task.end.millis() - task.start.millis()) as f64 / span;
            let drawn = node.size.w / plot;
            assert!(
                (drawn - share).abs() < 0.01,
                "{name}: task {i} ({:?}) is {:.1}% of the chart's time and {:.1}% of its width",
                task.name,
                share * 100.0,
                drawn * 100.0
            );
            // …and it *starts* where its date does.
            let offset = (task.start.millis() - from.millis()) as f64 / span;
            let drawn_offset = (node.bounds().0 - left) / plot;
            assert!(
                (drawn_offset - offset).abs() < 0.01,
                "{name}: task {i} ({:?}) begins {:.1}% into the chart's time and {:.1}% into its \
                 width",
                task.name,
                offset * 100.0,
                drawn_offset * 100.0
            );
        }
    }
}

/// **A gantt tick is inside the chart's own extent, and the ticks increase.**
///
/// An axis that runs past its data is exactly the crate's defect, so it is stated directly.
#[test]
fn every_gantt_tick_is_a_date_the_chart_actually_covers() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gantt;
    for (name, src) in cases_of(gantt::is_gantt) {
        let model = gantt::parse(src).expect("parses");
        let (from, to) = model.extent().expect("tasks");
        let ticks = super::gantt::tick_dates(from, to, &model.tick_interval);
        assert!(!ticks.is_empty(), "{name}: no ticks");
        for t in &ticks {
            assert!(
                *t >= from && *t <= to,
                "{name}: a tick at {} is outside the chart's own {}..{}",
                crate::preview::mermaid::gantt::time::format(*t, "%Y-%m-%d"),
                crate::preview::mermaid::gantt::time::format(from, "%Y-%m-%d"),
                crate::preview::mermaid::gantt::time::format(to, "%Y-%m-%d"),
            );
        }
        for w in ticks.windows(2) {
            assert!(w[1] > w[0], "{name}: the ticks do not increase");
        }
    }
}

/// **A journey's face is the score it was written with.**
#[test]
fn a_journeys_face_carries_its_own_task_score() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::journey;
    for (name, src) in cases_of(journey::is_journey) {
        let model = journey::parse(src).expect("parses");
        let d = laid_out(src);
        for (i, task) in model.tasks.iter().enumerate() {
            let face = d.node(&format!("task#{i}#face"));
            match task.score {
                Some(score) => {
                    let face = face.unwrap_or_else(|| panic!("{name}: task {i} has no face"));
                    assert_eq!(
                        face.mark,
                        Some(Mark::Face { score }),
                        "{name}: the face on task {i} is not its own score"
                    );
                }
                // A score that could not be read draws no face, because a neutral one is a claim.
                None => assert!(
                    face.is_none(),
                    "{name}: task {i} has no readable score and drew a face anyway"
                ),
            }
        }
    }
}

/// **A git graph's commits run in `seq` order along the time axis, and a commit sits on its own
/// branch's lane.**
#[test]
fn a_git_graphs_commits_run_in_order_along_their_own_lanes() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph::{self, Direction};
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let model = gitgraph::parse(src).expect("parses");
        let d = laid_out(src);
        let lanes: Vec<&str> = model.lanes().iter().map(|b| b.name.as_str()).collect();

        // Time.
        let along = |p: &Point| match model.direction {
            Direction::LeftToRight => p.x,
            Direction::TopToBottom => p.y,
            Direction::BottomToTop => -p.y,
        };
        let across = |p: &Point| match model.direction {
            Direction::LeftToRight => p.y,
            _ => p.x,
        };
        let mut previous: Option<f64> = None;
        for commit in &model.commits {
            let node = d
                .node(&commit.id)
                .unwrap_or_else(|| panic!("{name}: commit {:?} was not drawn", commit.id));
            let t = along(&node.center);
            if let Some(p) = previous {
                assert!(
                    t > p,
                    "{name}: commit {:?} is drawn at or before the one made before it",
                    commit.id
                );
            }
            previous = Some(t);

            // Lane.
            let want = lanes
                .iter()
                .position(|b| *b == commit.branch)
                .unwrap_or_else(|| panic!("{name}: {:?} is on no lane", commit.branch));
            let others: Vec<(usize, f64)> = lanes
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    // The lane a commit *would* be on, read off another commit of that branch.
                    let sample = model
                        .commits
                        .iter()
                        .find(|c| c.branch == *lanes[i])
                        .and_then(|c| d.node(&c.id))
                        .map(|n| across(&n.center));
                    (i, sample.unwrap_or(f64::INFINITY))
                })
                .collect();
            let nearest = others
                .iter()
                .filter(|(_, v)| v.is_finite())
                .min_by(|a, b| {
                    (a.1 - across(&node.center))
                        .abs()
                        .partial_cmp(&(b.1 - across(&node.center)).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| *i)
                .unwrap_or(want);
            assert_eq!(
                nearest, want,
                "{name}: commit {:?} is on branch {:?} but is drawn on another branch's lane",
                commit.id, commit.branch
            );
        }
    }
}

/// **An architecture edge leaves and arrives by the sides the source named.**
///
/// The whole reason this language is placed on a grid rather than ranked, so it is the property
/// that has to be stated: a line out of `:R` starts on its box's right edge.
#[test]
fn an_architecture_edge_leaves_by_the_side_it_names() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture::{self, Side};
    for (name, src) in cases_of(architecture::is_architecture) {
        let model = architecture::parse(src).expect("parses");
        let d = laid_out(src);
        for e in &model.edges {
            let Some(edge) = d.edges.iter().find(|x| x.from == e.from && x.to == e.to) else {
                continue;
            };
            for (id, side, at) in [
                (&e.from, e.from_side, edge.points.first()),
                (&e.to, e.to_side, edge.points.last()),
            ] {
                let Some(node) = d.node(id) else { continue };
                let (l, t, r, b) = node.bounds();
                let p = at.expect("a routed edge has ends");
                let ok = match side {
                    Side::Left => (p.x - l).abs() < 0.5,
                    Side::Right => (p.x - r).abs() < 0.5,
                    Side::Top => (p.y - t).abs() < 0.5,
                    Side::Bottom => (p.y - b).abs() < 0.5,
                };
                assert!(
                    ok,
                    "{name}: the line at {id:?} was written to meet its {side:?} side and does not"
                );
            }
        }
    }
}

/// **A block spans the columns it asked for.**
///
/// `a:2` is twice as wide as `a`, and that is the whole of what `columns` and `:n` mean.
#[test]
fn a_block_that_asked_for_two_columns_is_twice_as_wide() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block::{self, Item};
    for (name, src) in cases_of(block::is_block_diagram) {
        let model = block::parse(src).expect("parses");
        let d = laid_out(src);
        // The one-cell blocks are all the same width, and an n-cell one is n of those plus the
        // gaps between them.
        let unit = model
            .items
            .iter()
            .filter(|i| i.width() == 1)
            .filter_map(|i| i.id())
            .filter_map(|id| d.node(id))
            .map(|n| n.size.w)
            .fold(f64::NAN, f64::min);
        if !unit.is_finite() {
            continue;
        }
        for item in &model.items {
            let Item::Node(node) = item else { continue };
            if node.width < 2 {
                continue;
            }
            let Some(placed) = d.node(&node.id) else {
                continue;
            };
            let want = unit * node.width as f64 + super::block::CELL_GAP * (node.width - 1) as f64;
            assert!(
                (placed.size.w - want).abs() < 0.5,
                "{name}: {:?} asked for {} columns, which is {} wide, and was drawn {} wide",
                node.id,
                node.width,
                svg::num(want),
                svg::num(placed.size.w)
            );
        }
    }
}

/// **Every mindmap node is on the page, and a child is drawn beyond its parent.**
#[test]
fn a_mindmap_child_is_drawn_further_out_than_its_parent() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::mindmap;
    for (name, src) in cases_of(mindmap::is_mindmap) {
        let model = mindmap::parse(src).expect("parses");
        let d = laid_out(src);
        assert_eq!(d.nodes.len(), model.nodes.len(), "{name}: node count");
        for (i, node) in model.nodes.iter().enumerate() {
            let Some(parent) = node.parent else { continue };
            let (child, up) = (
                d.node(&format!("n{i}")).expect("drawn"),
                d.node(&format!("n{parent}")).expect("drawn"),
            );
            assert!(
                child.center.x > up.center.x,
                "{name}: {:?} is a child of {:?} and is not drawn beyond it",
                node.label,
                model.nodes[parent].label
            );
        }
    }
}

/// **A gantt scale maps the chart's own ends onto the plot's own ends.**
///
/// Stated on [`super::gantt::Scale`] directly, with numbers chosen here, because everything else
/// on a gantt chart shares that one scale: shift it and every bar, every tick and every frame
/// shifts with it, so the picture stays self-consistent while being about the wrong dates. This is
/// the definition of a scale, not konoma's opinion of one, and it is where a lost origin shows.
#[test]
fn a_gantt_scale_puts_the_charts_ends_on_the_plots_ends() {
    use crate::preview::mermaid::gantt::time::Instant;
    let from = Instant::from_civil(2014, 1, 1, 0, 0, 0, 0);
    let to = Instant::from_civil(2014, 3, 1, 0, 0, 0, 0);
    let scale = super::gantt::Scale {
        left: 120.0,
        right: 640.0,
        from,
        to,
    };
    assert!(
        (scale.x(from) - 120.0).abs() < 1e-9,
        "the chart's first instant is not at the plot's left edge: {}",
        svg::num(scale.x(from))
    );
    assert!(
        (scale.x(to) - 640.0).abs() < 1e-9,
        "the chart's last instant is not at the plot's right edge: {}",
        svg::num(scale.x(to))
    );
    // …and the half-way date lands half-way along.
    let mid = Instant::from_millis((from.millis() + to.millis()) / 2);
    assert!(
        (scale.x(mid) - 380.0).abs() < 1.0,
        "{}",
        svg::num(scale.x(mid))
    );
    // A zero-width range has nowhere to put anything, and answers with the left edge rather than
    // with a division by zero.
    let flat = super::gantt::Scale {
        left: 10.0,
        right: 20.0,
        from,
        to: from,
    };
    assert_eq!(flat.x(from), 10.0);
}

/// **A gantt chart is as wide as its names plus its plot**, and not wider.
///
/// The other half of the statement above, at the level of the finished drawing: a scale that has
/// lost its origin sends every mark thousands of pixels to the right, the frames follow them, and
/// every relative check still passes — while the diagram rasterises to a smear. A bound on the
/// *absolute* width is the thing that cannot be satisfied by shifting everything equally.
#[test]
fn a_gantt_chart_is_no_wider_than_its_names_and_its_plot() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gantt;
    for (name, src) in cases_of(gantt::is_gantt) {
        let model = gantt::parse(src).expect("parses");
        let d = laid_out(src);
        let names = model
            .tasks
            .iter()
            .map(|t| Label::measure(&t.name).width)
            .fold(0.0_f64, f64::max);
        // The plot is at least `PLOT_WIDTH` and grows to fit the tick labels, which are dates.
        let (from, to) = model.extent().expect("tasks");
        let ticks = super::gantt::tick_dates(from, to, &model.tick_interval);
        let widest_tick = ticks
            .iter()
            .map(|t| Label::measure(&gantt::time::format(*t, &model.axis_format)).width)
            .fold(0.0_f64, f64::max);
        let plot = super::gantt::PLOT_WIDTH
            .max((widest_tick + super::chart::TICK_GAP * 2.0) * ticks.len().max(1) as f64);
        // Everything that legitimately reaches past the plot, named: the gap between the names
        // and the plot, the tick label centred on the right-hand end (half of it hangs over), half
        // a milestone's diamond, the section frames' padding on each side, and the two margins
        // `normalise` adds.
        let bound = names
            + super::chart::TICK_GAP * 2.0
            + plot
            + (widest_tick / 2.0).max(super::gantt::MILESTONE / 2.0)
            + super::gantt::SECTION_PAD * 2.0
            + super::MARGIN * 2.0;
        assert!(
            d.width <= bound,
            "{name}: the chart is {} wide, and its names ({}) plus its plot ({}) come to {}",
            svg::num(d.width),
            svg::num(names),
            svg::num(plot),
            svg::num(bound)
        );
    }
}

/// **A gantt bar is on its own row, beside its own name.**
///
/// The chart is scale-free otherwise — every bar and every tick share one scale, so a uniform
/// shift of all of them is invisible. The *row names* do not share it, and this is what ties the
/// two halves of the drawing together: a bar sits on the centre line of the row whose name it is,
/// and to the right of the names.
#[test]
fn a_gantt_bar_shares_a_row_with_its_own_name() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gantt;
    for (name, src) in cases_of(gantt::is_gantt) {
        let model = gantt::parse(src).expect("parses");
        let d = laid_out(src);
        let names_right = d
            .nodes
            .iter()
            .filter(|n| n.id.ends_with("#name"))
            .map(|n| n.bounds().2)
            .fold(f64::NEG_INFINITY, f64::max);
        let axis_bottom = d
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("tick#"))
            .map(|n| n.bounds().3)
            .fold(f64::NEG_INFINITY, f64::max);
        for i in 0..model.tasks.len() {
            let (Some(bar), Some(label)) = (
                d.node(&format!("task#{i}")),
                d.node(&format!("task#{i}#name")),
            ) else {
                continue;
            };
            assert!(
                (bar.center.y - label.center.y).abs() < 0.5,
                "{name}: the bar for task {i} is not on the row its name is on"
            );
            assert!(
                bar.bounds().0 >= names_right - 0.5,
                "{name}: the bar for task {i} runs back into the names column"
            );
            // …and below the axis. The bars and the ticks share one scale, so a uniform shift of
            // that scale is invisible between them; the axis is drawn at a *fixed* height, so this
            // is where a scale that lost its origin shows.
            assert!(
                bar.bounds().1 >= axis_bottom - 0.5,
                "{name}: the bar for task {i} is drawn up into the axis"
            );
        }

        // …and inside its own section's frame, which is drawn across the plot at a fixed width.
        // A time scale that has lost its origin sends every bar off the end of its own band, and
        // nothing else in this file would notice: everything else on the time axis moves with it.
        for (i, task) in model.tasks.iter().enumerate() {
            if task.section.trim().is_empty() {
                continue;
            }
            let (Some(bar), Some(frame)) = (
                d.node(&format!("task#{i}")),
                d.cluster(&format!("section#{}", task.section)),
            ) else {
                continue;
            };
            let (fl, ft, fr, fb) = frame.bounds();
            let (l, t, r, b) = bar.bounds();
            assert!(
                l >= fl - 0.5 && t >= ft - 0.5 && r <= fr + 0.5 && b <= fb + 0.5,
                "{name}: the bar for task {i} is drawn outside its own section {:?}",
                task.section
            );
        }
    }
}

/// **`columns n` wraps.** A grid with more blocks than columns puts the rest on the next row, and
/// the next row is below this one.
#[test]
fn a_block_grid_wraps_at_the_column_count_it_declares() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block::{self, AUTO};
    let mut checked = 0usize;
    for (name, src) in cases_of(block::is_block_diagram) {
        let model = block::parse(src).expect("parses");
        if model.columns == AUTO {
            continue;
        }
        let d = laid_out(src);
        let per_row = model.columns.max(1) as usize;
        // Which row each item was declared onto, by the same fill rule the source describes.
        let (mut col, mut row) = (0usize, 0usize);
        let mut rows: Vec<(String, usize)> = Vec::new();
        for item in &model.items {
            let span = item.width().max(1).min(per_row);
            if col + span > per_row && col > 0 {
                col = 0;
                row += 1;
            }
            if let Some(id) = item.id() {
                rows.push((id.to_string(), row));
            }
            col += span;
        }
        // Not every `columns` case has more blocks than columns; the ones that do are what this
        // test is about, and `checked` below makes sure at least one of them does.
        if !rows.iter().any(|(_, r)| *r > 0) {
            continue;
        }
        checked += 1;
        // Every block on a later row is drawn below every block on an earlier one.
        for (a, ra) in &rows {
            for (b, rb) in &rows {
                if ra >= rb {
                    continue;
                }
                let (Some(na), Some(nb)) = (d.node(a), d.node(b)) else {
                    continue;
                };
                assert!(
                    na.bounds().3 <= nb.bounds().1 + 0.5,
                    "{name}: {a:?} is on row {ra} and {b:?} on row {rb}, and {a:?} is not above it"
                );
            }
        }
    }
    assert!(
        checked > 0,
        "no corpus case has more blocks than its `columns`, so nothing was checked"
    );
}

/// **A circle stays a circle.** A cell is stretched to the grid's width; a shape whose
/// proportions carry meaning is not, or `columns 1` would draw an ellipse the width of the page.
#[test]
fn a_block_shape_whose_proportions_mean_something_is_not_stretched() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block::{self, Item};
    use crate::preview::mermaid::flowchart::Shape;
    for (name, src) in cases_of(block::is_block_diagram) {
        let model = block::parse(src).expect("parses");
        let d = laid_out(src);
        for item in &model.items {
            let Item::Node(n) = item else { continue };
            let Some(placed) = d.node(&n.id) else {
                continue;
            };
            if matches!(n.shape, Shape::Circle | Shape::DoubleCircle) {
                assert!(
                    (placed.size.w - placed.size.h).abs() < 0.5,
                    "{name}: {:?} is a circle drawn {} by {}",
                    n.id,
                    svg::num(placed.size.w),
                    svg::num(placed.size.h)
                );
            }
        }
    }
}

/// **An architecture edge's ports place the boxes.**
///
/// `db:L -- R:server` says `server` is to the *left* of `db`. That is the whole reason this
/// language is placed on a grid rather than ranked, and it is a statement about the boxes rather
/// than about the line — which is why it is stated separately from
/// [`an_architecture_edge_leaves_by_the_side_it_names`].
#[test]
fn an_architecture_edges_ports_decide_where_the_boxes_go() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture::{self, Side};
    for (name, src) in cases_of(architecture::is_architecture) {
        let model = architecture::parse(src).expect("parses");
        let d = laid_out(src);
        for e in &model.edges {
            // Only where the two are in the same group: an edge across two groups is drawn and
            // not obeyed, because a group's members are kept together (`place_on_grid`).
            let group_of = |id: &str| {
                model
                    .services
                    .iter()
                    .find(|s| s.id == id)
                    .and_then(|s| s.group.clone())
            };
            if group_of(&e.from) != group_of(&e.to) {
                continue;
            }
            let (Some(from), Some(to)) = (d.node(&e.from), d.node(&e.to)) else {
                continue;
            };
            let ok = match e.from_side {
                Side::Left => to.center.x < from.center.x,
                Side::Right => to.center.x > from.center.x,
                Side::Top => to.center.y < from.center.y,
                Side::Bottom => to.center.y > from.center.y,
            };
            assert!(
                ok,
                "{name}: `{}:{:?}` says {:?} is on that side of {:?}, and it is not",
                e.from, e.from_side, e.to, e.from
            );
        }
    }
}

/// **A service is inside its own group's frame, and inside no other.**
#[test]
fn every_architecture_service_is_inside_its_own_group_only() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture;
    for (name, src) in cases_of(architecture::is_architecture) {
        let model = architecture::parse(src).expect("parses");
        let d = laid_out(src);
        for s in &model.services {
            let Some(node) = d.node(&s.id) else { continue };
            let (l, t, r, b) = node.bounds();
            for group in &model.groups {
                let Some(frame) = d.cluster(&group.id) else {
                    continue;
                };
                let (fl, ft, fr, fb) = frame.bounds();
                let inside = l >= fl - 0.5 && t >= ft - 0.5 && r <= fr + 0.5 && b <= fb + 0.5;
                let mine = s.group.as_deref() == Some(group.id.as_str());
                assert_eq!(
                    inside,
                    mine,
                    "{name}: {:?} is {}in {:?}",
                    s.id,
                    if inside { "" } else { "not " },
                    group.id
                );
            }
        }
    }
}

/// **Every kanban card is inside its own column's frame.**
#[test]
fn every_kanban_card_is_inside_its_own_column() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::kanban;
    for (name, src) in cases_of(kanban::is_kanban) {
        let model = kanban::parse(src).expect("parses");
        let d = laid_out(src);
        for column in &model.columns {
            let frame = d
                .cluster(&column.id)
                .unwrap_or_else(|| panic!("{name}: column {:?} has no frame", column.id));
            let (fl, ft, fr, fb) = frame.bounds();
            for card in &column.cards {
                let node = d
                    .node(&card.id)
                    .unwrap_or_else(|| panic!("{name}: card {:?} was not drawn", card.id));
                let (l, t, r, b) = node.bounds();
                assert!(
                    l >= fl - 0.5 && t >= ft - 0.5 && r <= fr + 0.5 && b <= fb + 0.5,
                    "{name}: card {:?} is drawn outside {:?}",
                    card.id,
                    column.label
                );
            }
        }
    }
}

/// **A timeline's periods run in source order along its own axis.**
#[test]
fn a_timelines_periods_run_in_source_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::timeline::{self, Direction};
    for (name, src) in cases_of(timeline::is_timeline) {
        let model = timeline::parse(src).expect("parses");
        let d = laid_out(src);
        let along = |p: &Point| match model.direction {
            Direction::LeftToRight => p.x,
            Direction::TopToBottom => p.y,
        };
        let mut previous: Option<f64> = None;
        for i in 0..model.periods.len() {
            let node = d
                .node(&format!("period#{i}"))
                .unwrap_or_else(|| panic!("{name}: period {i} was not drawn"));
            let at = along(&node.center);
            if let Some(p) = previous {
                assert!(
                    at > p,
                    "{name}: period {i} is drawn before the one before it"
                );
            }
            previous = Some(at);
            // …and every event is on its own period's line.
            for k in 0..model.periods[i].events.len() {
                let event = d
                    .node(&format!("period#{i}#event#{k}"))
                    .unwrap_or_else(|| panic!("{name}: event {i}.{k} was not drawn"));
                let axis = match model.direction {
                    Direction::LeftToRight => (event.center.x - node.center.x).abs(),
                    Direction::TopToBottom => 0.0,
                };
                assert!(
                    axis < 0.5,
                    "{name}: event {i}.{k} is not in its own period's column"
                );
            }
        }
    }
}

/// Every kind draws **something**: a diagram that lays out to no marks at all is a blank page.
#[test]
fn every_case_puts_something_on_the_page() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        assert!(
            !d.nodes.is_empty() || !d.clusters.is_empty(),
            "{name}: nothing was placed"
        );
        assert!(d.width > 0.0 && d.height > 0.0, "{name}: no extent");
        let svg = rendered(src);
        assert!(svg.contains("<svg"), "{name}: no SVG");
    }
}

// ---------------------------------------------------------------------------------------------
// 3b. Adversarial: a diagram or an error, never a panic and never a hang
// ---------------------------------------------------------------------------------------------

/// Sources chosen to break a parser or a layout rather than to be drawn.
///
/// Nothing here asserts *which* answer comes back — several are legitimately readable — only that
/// one comes back. §1 says the renderer runs on a worker thread inside `catch_unwind`, so a panic
/// would not take the UI down; it would take the diagram down **silently**, which is worse than an
/// error message. Both halves are swept: the parser, and the whole path through to SVG.
#[test]
fn adversarial_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    let mut sources: Vec<String> = vec![
        // Headers with nothing under them.
        "mindmap".into(),
        "kanban".into(),
        "journey".into(),
        "timeline".into(),
        "gantt".into(),
        "requirementDiagram".into(),
        "gitGraph".into(),
        "C4Context".into(),
        "block-beta".into(),
        "architecture-beta".into(),
        "zenuml".into(),
        // Indentation that is not a tree.
        "mindmap\n\ta\n  b\n\t\tc\n".into(),
        "mindmap\nRoot\nSecond root\n".into(),
        "mindmap\n  a[unclosed\n".into(),
        "mindmap\n  a[\"`never closed\n".into(),
        "kanban\n      deep\n  shallow\n".into(),
        "kanban\n  Todo\n    a@{ not yaml at all }\n".into(),
        // Dates that are not dates, and arithmetic that cannot close.
        "gantt\n  dateFormat YYYY-MM-DD\n  a :2024-02-30, 1d\n".into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  a :after a, 1d\n".into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  a :a, after b, 1d\n  b :b, after a, 1d\n".into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  a :2024-01-01, 99999999d\n".into(),
        "gantt\n  dateFormat X\n  a :99999999999999999999, 1d\n".into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  excludes weekends monday tuesday wednesday thursday \
         friday saturday sunday\n  a :2024-01-01, 3d\n"
            .into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  tickInterval 0day\n  a :2024-01-01, 3d\n".into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  a :milestone, 2024-01-01, 0d\n".into(),
        // Histories that cannot have happened.
        "gitGraph\n  merge main\n".into(),
        "gitGraph\n  commit\n  branch a\n  branch a\n".into(),
        "gitGraph\n  commit\n  cherry-pick id: \"nope\"\n".into(),
        "gitGraph\n  commit id: \"\"\n".into(),
        "gitGraph\n  commit tag: \"unclosed\n".into(),
        // Grids that do not fit.
        "block-beta\n  columns 0\n  a b c\n".into(),
        "block-beta\n  columns 999999\n  a\n".into(),
        "block-beta\n  a:999999\n".into(),
        "block-beta\n  space:999999\n  a\n".into(),
        "block-beta\n  block:a\n    block:a\n      x\n    end\n  end\n".into(),
        "block-beta\n  a[unclosed\n".into(),
        "block-beta\n  a --> \n".into(),
        "block-beta\n  --> b\n".into(),
        // Ports and groups that contradict each other.
        "architecture-beta\n  service a(x)[A]\n  a:R -- L:a\n".into(),
        "architecture-beta\n  group a(x)[A] in b\n  group b(x)[B] in a\n  service s(x)[S] in a\n"
            .into(),
        "architecture-beta\n  service a(x)[A]\n  a:R -- L:nope\n".into(),
        "architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  align row a a\n".into(),
        // C4 argument lists that do not close.
        "C4Context\n  Person(a, \"unclosed\n".into(),
        "C4Context\n  System(a\n".into(),
        "C4Context\n  Boundary(b, \"B\") {\n".into(),
        "C4Context\n  }\n".into(),
        // Requirement bodies that do not close.
        "requirementDiagram\n  requirement a {\n".into(),
        "requirementDiagram\n  requirement a {\n  id: 1\n  }\n  a - contains -> nope\n".into(),
        // ZenUML statements outside the layer konoma reads.
        "zenuml\n  }\n".into(),
        "zenuml\n  if(a) {\n".into(),
        "zenuml\n  A.b().c()\n".into(),
        "zenuml\n  return\n".into(),
        // Unicode, emoji, control characters, and the double space.
        "mindmap\n  root((🚀))\n    日本語のラベル\n    \u{200b}\n".into(),
        "journey\n  タスク: 3: 私, 猫\n".into(),
        "timeline\n  2002 : ロケット🚀\n".into(),
        "kanban\n  やること\n    タブ\tと改行\n".into(),
        "journey\n  loop  Every minute: 3: Me\n".into(),
        "block-beta\n  a[\"loop  Every minute\"]\n".into(),
        // Keywords used as data.
        "timeline\n  section : an event\n".into(),
        "journey\n  title: 3: Me\n".into(),
        "gantt\n  dateFormat YYYY-MM-DD\n  section :a, 2024-01-01, 1d\n".into(),
        "gitGraph\n  commit id: \"commit\"\n".into(),
    ];
    // Sizes at which an accidental O(n^2) stops returning.
    let deep: String = (0..300)
        .map(|i| format!("{}n{i}\n", " ".repeat(i)))
        .collect();
    sources.push(format!("mindmap\n{deep}"));
    let wide: String = (0..400).map(|i| format!("  n{i}\n")).collect();
    sources.push(format!("kanban\n  Todo\n{wide}"));
    let tasks: String = (0..400)
        .map(|i: usize| format!("  t{i} :t{i}, after t{}, 1d\n", i.saturating_sub(1)))
        .collect();
    sources.push(format!(
        "gantt\n  dateFormat YYYY-MM-DD\n  first :t0, 2024-01-01, 1d\n{tasks}"
    ));
    let commits: String = (0..400).map(|_| "  commit\n".to_string()).collect();
    sources.push(format!("gitGraph\n{commits}"));
    let blocks: String = (0..400).map(|i| format!("  b{i}\n")).collect();
    sources.push(format!("block-beta\n  columns 20\n{blocks}"));
    let services: String = (0..200)
        .map(|i| format!("  service s{i}(x)[S{i}]\n"))
        .collect();
    let links: String = (0..199)
        .map(|i| format!("  s{i}:R --> L:s{}\n", i + 1))
        .collect();
    sources.push(format!("architecture-beta\n{services}{links}"));
    let msgs: String = (0..400).map(|i| format!("  A->B: m{i}\n")).collect();
    sources.push(format!("zenuml\n{msgs}"));
    let nested: String = (0..80)
        .map(|i| format!("{}A.m{i}() {{\n", " ".repeat(i)))
        .collect();
    let closes: String = (0..80)
        .map(|i| format!("{}}}\n", " ".repeat(79 - i)))
        .collect();
    sources.push(format!("zenuml\n{nested}{closes}"));

    for src in &sources {
        // The parser.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = dispatch(src);
        }));
        assert!(
            caught.is_ok(),
            "the parser panicked on {:?}",
            &src[..src.len().min(120)]
        );
        // …and the whole path, through the production entry point.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = mermaid_to_svg_reason(src, "dark");
        }));
        assert!(
            caught.is_ok(),
            "the renderer panicked on {:?}",
            &src[..src.len().min(120)]
        );
    }
    eprintln!("{} adversarial sources swept", sources.len());
}

// ---------------------------------------------------------------------------------------------
// 4. The golden
// ---------------------------------------------------------------------------------------------

/// The whole corpus, through the production entry point, masked as §6 requires.
#[test]
fn kinds_corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CASES {
        out.push_str(&format!("=== {name} ===\n"));
        match mermaid_to_svg_reason(src, "dark") {
            Ok(svg) => out.push_str(&mask_numbers(&svg)),
            Err(e) => out.push_str(&format!("(refused: {e})\n")),
        }
        out.push('\n');
    }
    assert_snapshot("mermaid_kinds", &out);
}

/// The six new glyphs, assembled by hand so **no font is involved and every number is pinned
/// exactly** — the other half of the golden.
///
/// The corpus golden above pins *structure* over real sources with its numbers masked, because a
/// coordinate descends from a label width and a label width descends from the machine's fonts.
/// This one pins the arithmetic: the cloud's four curves, the bang's thirty-two vertices, the
/// underline's rule, the face's mouth at each of the five scores, the block arrow's outline for
/// each combination of heads, and the crossed dot. None of them is measured, so it is identical on
/// every machine — and it is where a mutation to any of those shapes has to fail.
#[test]
fn new_glyph_emit_golden() {
    let d = synthetic_glyph_diagram();
    let mut out = String::new();
    for t in super::theme::ALL {
        out.push_str(&format!("=== {} ===\n", t.name));
        out.push_str(&svg::emit(&d, t));
        out.push('\n');
    }
    assert_snapshot("mermaid_kinds_emit", &out);
}

/// One node of every glyph stage 5b added, at coordinates chosen by hand.
fn synthetic_glyph_diagram() -> Diagram {
    use super::{PlacedNode, Size};
    let label = |text: &str, w: f64| Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * super::labels::line_height(),
    };
    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut push = |id: &str, shape: Glyph, x: f64, y: f64, size: Size, mark: Option<Mark>| {
        nodes.push(PlacedNode {
            id: id.to_string(),
            shape,
            center: Point::new(x, y),
            size,
            label: label("label", 40.0),
            panel: None,
            series: None,
            mark,
        });
    };
    push(
        "cloud",
        Glyph::Cloud,
        80.0,
        60.0,
        Size::new(120.0, 60.0),
        None,
    );
    push(
        "bang",
        Glyph::Bang,
        240.0,
        60.0,
        Size::new(120.0, 60.0),
        None,
    );
    push(
        "underline",
        Glyph::Underline,
        400.0,
        60.0,
        Size::new(100.0, 24.0),
        None,
    );
    push(
        "reverted",
        Glyph::Reverted,
        540.0,
        60.0,
        Size::new(24.0, 24.0),
        None,
    );
    // Every score, because the mouth **is** the number.
    for (i, score) in [1.0_f64, 2.0, 3.0, 4.0, 5.0].into_iter().enumerate() {
        push(
            &format!("face{i}"),
            Glyph::Face,
            80.0 + i as f64 * 50.0,
            160.0,
            Size::new(24.0, 24.0),
            Some(Mark::Face { score }),
        );
    }
    // Every combination of heads, because each one is a different outline.
    for (i, (left, right, up, down)) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
        (true, true, false, false),
        (false, false, true, true),
        (true, true, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        push(
            &format!("arrow{i}"),
            Glyph::BlockArrow,
            80.0 + i as f64 * 110.0,
            240.0,
            Size::new(100.0, 60.0),
            Some(Mark::BlockArrow {
                left,
                right,
                up,
                down,
            }),
        );
    }
    Diagram {
        width: 900.0,
        height: 320.0,
        nodes,
        ..Diagram::default()
    }
}

/// Every palette draws every case, and the five are all different.
#[test]
fn every_palette_draws_every_case() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let mut seen: HashSet<String> = HashSet::new();
        for theme in ["dark", "light", "classic", "forest", "neutral"] {
            let svg =
                mermaid_to_svg_reason(src, theme).unwrap_or_else(|e| panic!("{name}/{theme}: {e}"));
            assert!(
                !svg.contains("var(--"),
                "{name}/{theme}: a CSS variable, which usvg does not resolve (§1)"
            );
            assert!(
                svg.contains("fill=\"none\"") || !svg.contains("<rect x=\"0\" y=\"0\""),
                "{name}/{theme}: an opaque page background, which §1 forbids"
            );
            seen.insert(svg);
        }
        assert_eq!(
            seen.len(),
            5,
            "{name}: two palettes drew byte-identical output"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 5. Looking at it
// ---------------------------------------------------------------------------------------------

/// `KONOMA_GALLERY=/some/dir cargo test -- --ignored kinds_tests::gallery`
#[test]
#[ignore = "writes SVG files for a person to look at: KONOMA_GALLERY=<dir> cargo test -- --ignored gallery"]
fn gallery() {
    let dir = std::env::var("KONOMA_GALLERY").expect("set KONOMA_GALLERY to a directory");
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    let theme = std::env::var("KONOMA_GALLERY_THEME").unwrap_or_else(|_| "dark".to_string());
    for (name, src) in CASES {
        match mermaid_to_svg_reason(src, &theme) {
            Ok(svg) => {
                std::fs::write(format!("{dir}/{name}.svg"), svg).expect("write");
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
    eprintln!("wrote the stage-5b gallery to {dir}");
}

/// A label as it will be drawn, for the messages above.
fn _words(label: &Label) -> String {
    label.lines.join(" ")
}

/// `cargo test --release -- --ignored kinds_tests::render_time` — what a diagram costs.
///
/// §6-A records stage 1's measurement (p50 0.72 ms against the crate's 13.2 ms) and §8 records the
/// crate's p90 at 86.8 ms and its worst case at 662 ms. This is the same measurement over
/// **every** corpus konoma now has — all twenty-three kinds — through the production entry point.
/// Reported rather than asserted: a timing threshold in a test suite is a flake, and the number is
/// for `docs/STATUS.md`.
#[test]
#[ignore = "a measurement, not an assertion: cargo test --release -- --ignored render_time"]
fn render_time() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut corpus: Vec<(&str, &str)> = Vec::new();
    corpus.extend(super::tests::CORPUS.iter().copied());
    corpus.extend(super::state_tests::CASES.iter().copied());
    corpus.extend(super::class_tests::CASES.iter().copied());
    corpus.extend(super::er_tests::CASES.iter().copied());
    corpus.extend(super::sequence_tests::CASES.iter().copied());
    corpus.extend(crate::preview::mermaid::chart::tests::CASES.iter().copied());
    corpus.extend(CASES.iter().copied());

    // One warm pass so the font database and the syntax caches are not in the numbers.
    for (_, src) in &corpus {
        let _ = mermaid_to_svg_reason(src, "dark");
    }

    let mut times: Vec<(f64, &str)> = Vec::new();
    for (name, src) in &corpus {
        let mut best = f64::INFINITY;
        for _ in 0..5 {
            let at = std::time::Instant::now();
            let _ = mermaid_to_svg_reason(src, "dark");
            best = best.min(at.elapsed().as_secs_f64() * 1000.0);
        }
        times.push((best, name));
    }
    times.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| times[((times.len() as f64 - 1.0) * q).round() as usize];
    eprintln!(
        "{} diagrams: p50 {:.2}ms  p90 {:.2}ms  max {:.2}ms ({})",
        times.len(),
        at(0.5).0,
        at(0.9).0,
        times[times.len() - 1].0,
        times[times.len() - 1].1
    );
    for (t, name) in times.iter().rev().take(5) {
        eprintln!("  slowest: {name} {t:.2}ms");
    }
}
