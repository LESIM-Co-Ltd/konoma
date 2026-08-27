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
    (
        // `SPACELIST` is counted in **characters** and `getParent` takes the last node with a strictly
        // smaller count, so indentation does not have to be consistent. This is the documentation's own
        // uneven example: C at 6 hangs off A at 4, not off B at 8.
        "mindmap-uneven-indent",
        "mindmap\nRoot\n    A\n        B\n      C\n        D\n",
    ),
    (
        "mindmap-deep",
        "mindmap\n  root((core))\n   l1\n    l2\n     l3\n      l4\n       l5\n        l6\n",
    ),
    (
        "mindmap-wide",
        "mindmap\n  root((hub))\n    one\n    two\n    three\n    four\n    five\n    six\n    seven\n    eight\n",
    ),
    (
        // `::icon(...)` and `:::class` are statements of their own that decorate the node before them —
        // read so the line does not become a phantom node, drawn as neither.
        "mindmap-decorations",
        "mindmap\n  Root\n    Reading\n    ::icon(fa fa-book)\n    Writing\n    :::urgent large\n    Plain\n",
    ),
    (
        // Inside `NODE` a `["]` opens `NSTR`, which runs to the next quotation mark — brackets and all.
        // Without that state a bracket in the words closes the node early.
        "mindmap-quoted-brackets",
        "mindmap\n  Root\n    a[\"a (b) and [c]\"]\n    b(\"one ) two\")\n",
    ),
    (
        // `["][`]` … `[`]["]` spans lines, because `NSTR2` has no rule for a newline.
        "mindmap-markdown-string",
        "mindmap\n  id1[\"`**Root** with\na second line`\"]\n    id2[\"`a *child*\nover two lines`\"]\n",
    ),
    (
        // `nodeWithoutId` — the description becomes the id too.
        "mindmap-no-id",
        "mindmap\n  (a rounded label)\n    [a square label]\n    ((a circle))\n",
    ),
    (
        "mindmap-one-node",
        "mindmap\n  Only\n",
    ),
    (
        "mindmap-comments",
        "mindmap\n%% a comment is a blank line here\n  Root\n\n    A\n  %% and so is an indented one\n    B\n",
    ),
    (
        // Three siblings with the same words are three nodes with the **same id** upstream too, because
        // `nodeWithId` uses what was written. A drawing that collapsed them would lose two branches.
        "mindmap-duplicate-labels",
        "mindmap\n  Root\n    Same\n    Same\n    Same\n",
    ),
    (
        "mindmap-long-label",
        "mindmap\n  root((centre))\n    A label long enough that a terminal cannot show it on one line without wrapping it somewhere sensible\n    short\n",
    ),
    (
        // The first node's own count is the baseline and is subtracted from every later one, so a whole
        // diagram indented by eight is the same diagram.
        "mindmap-indented-whole",
        "mindmap\n        Root\n            A\n                B\n            C\n",
    ),
    (
        "mindmap-cjk-shapes",
        "mindmap\n  ルート((全体))\n    四角[設定を読む]\n    丸(描画する)\n    雲)ここは雲(\n    六角{{判定}}\n",
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
    (
        // A column with no cards is still a column: `getSection` makes one for every node at the first
        // node's level, whatever follows it.
        "kanban-empty-column",
        "kanban\n  Todo\n    a[First]\n  Blocked\n  Done\n    b[Second]\n",
    ),
    (
        "kanban-shapes",
        "kanban\n  Todo\n    a(rounded)\n    b((circle))\n    c{{hexagon}}\n    d[square]\n    plain\n",
    ),
    (
        "kanban-decorations",
        "kanban\n  Todo\n    a[Card]\n    ::icon(fa fa-bug)\n    b[Another]\n    :::urgent\n",
    ),
    (
        "kanban-cjk",
        "kanban\n  やること\n    [設計を書く]\n    id2[レンダラを実装する]@{ assigned: '担当', priority: 'Very High' }\n  完了\n    [調査]\n",
    ),
    (
        "kanban-many-columns",
        "kanban\n  Backlog\n    a1[One]\n  Todo\n    a2[Two]\n  Doing\n    a3[Three]\n  Review\n    a4[Four]\n  Done\n    a5[Five]\n",
    ),
    (
        "kanban-tall-column",
        "kanban\n  Todo\n    c1[One]\n    c2[Two]\n    c3[Three]\n    c4[Four]\n    c5[Five]\n    c6[Six]\n  Done\n    d1[Shipped]\n",
    ),
    (
        "kanban-long-card",
        "kanban\n  Todo\n    a[A card whose text is far longer than any column header and has to be wrapped to be read]\n  Done\n    b[ok]\n",
    ),
    (
        // Two columns written with the same words. Upstream makes two of them, so konoma does too, and
        // the frames have to stay apart — a lookup that collapsed them would hide a card in the wrong one.
        "kanban-duplicate-columns",
        "kanban\n  Todo\n    a[One]\n  Doing\n    b[Two]\n  Todo\n    c[Three]\n",
    ),
    (
        "kanban-markdown-card",
        "kanban\n  Todo\n    a[\"`**bold** card\nsecond line`\"]\n  Done\n    b[plain]\n",
    ),
    (
        "kanban-comments",
        "kanban\n%% the board\n  Todo\n\n    a[One]\n  %% between columns\n  Done\n    b[Two]\n",
    ),
    (
        "kanban-meta-all",
        "kanban\n  Backlog\n    a[Card]@{ assigned: 'knsv', ticket: MC-1, priority: 'Very Low', icon: 'bug' }\n    b[Other]@{ priority: 'High' }\n    c[Third]@{ priority: 'Low' }\n",
    ),
    (
        // Everything below the section level is a card in the last column, however deep — `getSection`
        // compares against `nodes[0].level` and not against the level of the node before it.
        "kanban-deeper-card",
        "kanban\n  Todo\n    a[One]\n        b[Deeper still]\n  Done\n    c[Two]\n",
    ),
    (
        "kanban-one-card",
        "kanban\n  Todo\n    a[Only]\n",
    ),
    (
        "kanban-label-override",
        "kanban\n  Todo\n    a[written in the brackets]@{ label: 'written in the metadata' }\n  Done\n    b[plain]\n",
    ),
    (
        // Cards written with nothing in their brackets. `nodeWithoutId` names a card by its own
        // words, so a card with no words has no name — and `kanbanDb` falls back to `kbn` plus a
        // counter. Nothing in the corpus reached that branch, so a mutation that made every
        // generated id the same string survived: there was never a second card that needed one.
        "kanban-empty-cards",
        "kanban\n  Todo\n    []\n    []\n  Done\n    a[Real]\n",
    ),
    (
        // The same words on two cards in two columns. `nodeWithId` names a card by what was
        // written, so these two are asking to be told apart by something other than their text.
        "kanban-repeated-card-words",
        "kanban\n  Todo\n    [Review]\n  Doing\n    [Review]\n  Done\n    [Ship]\n",
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
    (
        // The mouth **is** the number, so every one of the five has to be drawn somewhere in the corpus.
        "journey-all-scores",
        "journey\n  title Every face\n  section Scores\n    One: 1: Me\n    Two: 2: Me\n    Three: 3: Me\n    Four: 4: Me\n    Five: 5: Me\n",
    ),
    (
        "journey-cjk",
        "journey\n  title 一日の流れ\n  section 出社\n    お茶をいれる: 5: 私\n    階段をのぼる: 3: 私, 猫\n  section 帰宅\n    座る: 4: 私\n",
    ),
    (
        "journey-many-people",
        "journey\n  title Handover\n  section Release\n    Cut the tag: 3: Alice, Bob, Carol, Dave, Erin\n    Announce: 5: Alice\n",
    ),
    (
        // `Number('n/a')` is `NaN` upstream. A face drawn anyway would be a claim the source never made,
        // so the task keeps its box and gets none.
        "journey-unreadable-score",
        "journey\n  title Missing scores\n  Ready: n/a: Me\n  Set: 4: Me\n  Go: : Me\n",
    ),
    (
        // The grammar takes any number. A face has five mouths, so what happens at 0 and at 9 is a
        // question the corpus has to ask rather than assume.
        "journey-out-of-range-score",
        "journey\n  title Out of range\n  Low: 0: Me\n  High: 9: Me\n  Fine: 3: Me\n",
    ),
    (
        "journey-many-sections",
        "journey\n  title A long day\n  section Morning\n    Wake: 3: Me\n  section Noon\n    Eat: 5: Me\n  section Afternoon\n    Work: 2: Me\n  section Evening\n    Rest: 5: Me\n",
    ),
    (
        // A section name used twice, not next to each other, is **two bands**. They have to stay apart,
        // and each has to hold its own tasks.
        "journey-repeated-section",
        "journey\n  title Back and forth\n  section Office\n    Arrive: 4: Me\n  section Home\n    Lunch: 5: Me\n  section Office\n    Leave: 2: Me\n",
    ),
    (
        "journey-accessibility",
        "journey\n  title Checkout\n  accTitle: The checkout journey\n  accDescr: How a customer pays\n  section Pay\n    Enter card: 4: Customer\n",
    ),
    (
        // `journey` is one of the two languages where `#` starts a comment anywhere on a line.
        "journey-comments",
        "journey\n  %% a mermaid comment\n  title Comments\n  section Work # this is dropped\n    Do it: 3: Me\n",
    ),
    (
        "journey-long-name",
        "journey\n  title Long\n  section Work\n    A task whose name is long enough that the box under the face has to wrap it: 4: Me\n    Short: 2: Me\n",
    ),
    (
        "journey-one-task",
        "journey\n  Only: 3: Me\n",
    ),
    (
        // `getActors` is first-appearance order over the whole journey, not per section.
        "journey-actors-across-sections",
        "journey\n  title Who is here\n  section First\n    A: 3: Ann\n    B: 4: Bob, Ann\n  section Second\n    C: 5: Cara, Bob\n",
    ),
    (
        "journey-no-people-mixed",
        "journey\n  title Some have people\n  section Solo\n    Alone: 4\n    Together: 5: Me, You\n",
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
    (
        "timeline-lr",
        "timeline LR\n  title Explicit direction\n  2020 : Started\n  2021 : Grew\n  2022 : Shipped\n",
    ),
    (
        // An event line begins with `: ` and belongs to the period above it, which is how the
        // documentation writes a period with several events.
        "timeline-events-on-their-own-lines",
        "timeline\n  title Continuations\n  2002 : LinkedIn\n  2004 : Facebook\n       : Google\n       : Orkut\n",
    ),
    (
        // `":"\s(?:[^:\n]|":"(?!\s))+` — a colon **not** followed by whitespace stays inside the event,
        // and one that is followed by whitespace starts another.
        "timeline-colon-inside-an-event",
        "timeline\n  title Times\n  Monday : Stand-up at 09:30\n  Tuesday : Review at 14:00 : Retro at 16:00\n",
    ),
    (
        "timeline-cjk",
        "timeline\n  title 日本語の年表\n  section 前半\n    2002年 : 創業 : 最初の製品\n    2004年 : 上場\n  section 後半\n    2010年 : 海外展開\n",
    ),
    (
        // A period with no events is still a period — `addTask` runs on the `period` rule alone.
        "timeline-no-events",
        "timeline\n  title Periods only\n  2019\n  2020\n  2021\n",
    ),
    (
        "timeline-many-events",
        "timeline\n  title Busy\n  2023 : One : Two : Three : Four : Five\n  2024 : Only one\n",
    ),
    (
        "timeline-one-period",
        "timeline\n  2020 : Alone\n",
    ),
    (
        "timeline-long-event",
        "timeline\n  title Long\n  2024 : An event whose text is long enough that it has to wrap inside the column its period owns\n  2025 : short\n",
    ),
    (
        "timeline-repeated-section",
        "timeline\n  title Back and forth\n  section Growth\n    2019 : Hired\n  section Pause\n    2020 : Waited\n  section Growth\n    2021 : Hired again\n",
    ),
    (
        // Two different rules, one line apart. `"title"\s[^\n]+` takes the **rest of the line**,
        // `#` included — unlike `journey`, whose title rule is `[^#\n;]+` — while a period line
        // is reached only after `\#[^\n]*` has been skipped. So the title here keeps its `#` and
        // the period loses everything after one.
        "timeline-comments",
        "timeline\n  %% a mermaid comment\n  title Comments # kept, because the title rule takes \
         the whole line\n  2020 : Fine # and this half is dropped\n",
    ),
    (
        // Direction crossed with *no* sections: the vertical renderer and the banding are separate
        // decisions, and the corpus only had them together.
        "timeline-td-no-sections",
        "timeline TD\n  title Down the page\n  Q1 : Plan\n  Q2 : Build\n  Q3 : Ship\n",
    ),
    (
        "timeline-accessibility",
        "timeline\n  title Accessible\n  accTitle: A short timeline\n  accDescr: Three years\n  2020 : One\n  2021 : Two\n",
    ),
    (
        "timeline-uneven-sections",
        "timeline\n  title Uneven\n  section Long\n    2001 : a\n    2002 : b\n    2003 : c\n  section Short\n    2004 : d\n",
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
    (
        // Every corpus case had a `section`, so the un-banded chart — which is what a short gantt in a
        // README looks like — was never drawn.
        "gantt-no-section",
        "gantt\n  title No sections at all\n  dateFormat YYYY-MM-DD\n  first :a1, 2024-01-01, 5d\n  second :after a1, 3d\n",
    ),
    (
        // `until x` is the other half of `after x`, and neither is in the documentation's own example.
        "gantt-until",
        "gantt\n  dateFormat YYYY-MM-DD\n  section Work\n    build :b1, 2024-01-01, 10d\n    watch :w1, 2024-01-02, until b1\n",
    ),
    (
        "gantt-tick-interval",
        "gantt\n  title Weekly ticks\n  dateFormat YYYY-MM-DD\n  axisFormat %d/%m\n  tickInterval 1week\n  weekday monday\n  section S\n    long :2024-01-01, 28d\n",
    ),
    (
        "gantt-unix-dates",
        "gantt\n  title Unix seconds\n  dateFormat X\n  axisFormat %H:%M\n  section S\n    a :0, 3600\n    b :3600, 7200\n",
    ),
    (
        "gantt-inclusive-and-top-axis",
        "gantt\n  title Inclusive end dates\n  dateFormat YYYY-MM-DD\n  inclusiveEndDates\n  topAxis\n  section S\n    a :2024-01-01, 2024-01-05\n    b :2024-01-06, 2024-01-08\n",
    ),
    (
        // Both are read and not drawn — a terminal has nothing to click, and an unread line becomes a
        // phantom task called `click a1 href` (§2-3).
        "gantt-today-marker-and-click",
        "gantt\n  dateFormat YYYY-MM-DD\n  todayMarker off\n  section S\n    a :a1, 2024-01-01, 5d\n    click a1 href \"https://example.invalid\"\n",
    ),
    (
        "gantt-cjk",
        "gantt\n  title 開発計画\n  dateFormat YYYY-MM-DD\n  section 設計\n    要件をまとめる :a1, 2024-01-01, 7d\n    設計書を書く :after a1, 5d\n  section 実装\n    実装する :2024-01-15, 20d\n",
    ),
    (
        "gantt-includes",
        "gantt\n  dateFormat YYYY-MM-DD\n  excludes weekends\n  includes 2024-01-06\n  section S\n    a :2024-01-01, 8d\n",
    ),
    (
        // A section written twice, apart, is **two bands**. Every band is a frame, and two frames with
        // the same name still have to be told apart — which is what a repeated title tests.
        "gantt-repeated-section",
        "gantt\n  title Back to the first section\n  dateFormat YYYY-MM-DD\n  section Design\n    a :2024-01-01, 3d\n  section Build\n    b :2024-01-04, 3d\n  section Design\n    c :2024-01-07, 3d\n",
    ),
    (
        // Three fields means **id**, start, end; two means start, end; one means end alone. Getting the
        // count rule wrong turns `i1` into a date.
        "gantt-explicit-ids",
        "gantt\n  dateFormat YYYY-MM-DD\n  section Chain\n    one :i1, 2024-01-01, 2d\n    two :i2, after i1, 2d\n    three :i3, after i2, 2d\n    four :after i3, 2d\n",
    ),
    (
        "gantt-long-names",
        "gantt\n  dateFormat YYYY-MM-DD\n  section S\n    A task whose name is much longer than the bar it labels :2024-01-01, 2d\n    b :2024-01-03, 2d\n",
    ),
    (
        "gantt-one-task",
        "gantt\n  dateFormat YYYY-MM-DD\n  section S\n    only :2024-01-01, 4d\n",
    ),
    (
        // A milestone is a moment, so it contributes a centre and not a bar — and the chart's own extent
        // starts and ends on one here, which is where a bound that only allowed for the plot fails.
        "gantt-milestone-only",
        "gantt\n  title Milestones and a bar\n  dateFormat YYYY-MM-DD\n  section S\n    kickoff :milestone, m1, 2024-01-01, 0d\n    work :2024-01-01, 5d\n    launch :milestone, m2, 2024-01-06, 0d\n",
    ),
    (
        "gantt-vert",
        "gantt\n  dateFormat YYYY-MM-DD\n  section S\n    a :2024-01-01, 5d\n    marker :vert, v1, 2024-01-03, 0d\n",
    ),
    (
        // A milestone written **with a duration**. `ganttDb` sets a milestone's end to its start
        // whatever the source asked for, because a milestone is a moment — and every milestone in
        // the corpus was written `0d`, which makes that rule and *no rule at all* the same
        // drawing. A mutation that deleted it therefore survived.
        "gantt-milestone-with-a-duration",
        "gantt\n  dateFormat YYYY-MM-DD\n  section S\n    review :milestone, m1, 2024-01-01, \
         5d\n    work :2024-01-02, 6d\n",
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
    (
        "requirement-every-type",
        "requirementDiagram\n  requirement r1 {\n  id: 1\n  text: plain\n  }\n  functionalRequirement r2 {\n  id: 2\n  }\n  interfaceRequirement r3 {\n  id: 3\n  }\n  performanceRequirement r4 {\n  id: 4\n  }\n  physicalRequirement r5 {\n  id: 5\n  }\n  designConstraint r6 {\n  id: 6\n  }\n  r1 - contains -> r2\n  r1 - contains -> r3\n",
    ),
    (
        // All seven `relationship` terminals. The corpus had two.
        "requirement-every-relation",
        "requirementDiagram\n  requirement a {\n  id: 1\n  }\n  requirement b {\n  id: 2\n  }\n  element e {\n  type: test\n  }\n  a - contains -> b\n  a - copies -> b\n  a - derives -> b\n  e - satisfies -> a\n  e - verifies -> b\n  a - refines -> b\n  a - traces -> b\n",
    ),
    (
        "requirement-risks-and-methods",
        "requirementDiagram\n  requirement low {\n  id: 1\n  risk: low\n  verifymethod: analysis\n  }\n  requirement medium {\n  id: 2\n  risk: medium\n  verifymethod: inspection\n  }\n  requirement high {\n  id: 3\n  risk: high\n  verifymethod: demonstration\n  }\n  requirement tested {\n  id: 4\n  verifymethod: test\n  }\n  low - traces -> high\n",
    ),
    (
        "requirement-element-docref",
        "requirementDiagram\n  requirement r {\n  id: 1\n  text: needs a source\n  }\n  element doc {\n  type: document\n  docRef: docs/PRD.md\n  }\n  doc - traces -> r\n",
    ),
    (
        "requirement-quoted",
        "requirementDiagram\n  requirement \"a quoted name\" {\n  id: \"1.2.3\"\n  text: \"a quoted, comma-bearing text\"\n  risk: high\n  }\n  element \"a quoted element\" {\n  type: \"a quoted type\"\n  }\n  \"a quoted element\" - satisfies -> \"a quoted name\"\n",
    ),
    (
        // Colour rules are read and dropped. A line the grammar has a rule for and the parser has none
        // for becomes a phantom box (§2-3), which is the defect this shape guards.
        "requirement-styles",
        "requirementDiagram\n  requirement a {\n  id: 1\n  }\n  requirement b {\n  id: 2\n  }\n  classDef hot fill:#f96\n  class a hot\n  style b fill:#333\n  a - derives -> b\n",
    ),
    (
        "requirement-cjk",
        "requirementDiagram\n  requirement 要件A {\n  id: 1\n  text: 全画面で読めること\n  risk: high\n  verifymethod: test\n  }\n  element 検証環境 {\n  type: simulation\n  }\n  検証環境 - verifies -> 要件A\n",
    ),
    (
        "requirement-direction-tb",
        "requirementDiagram\n  direction TB\n  requirement a {\n  id: 1\n  }\n  requirement b {\n  id: 2\n  }\n  a - contains -> b\n",
    ),
    (
        "requirement-direction-rl",
        "requirementDiagram\n  direction RL\n  requirement a {\n  id: 1\n  }\n  requirement b {\n  id: 2\n  }\n  b <- derives - a\n",
    ),
    (
        // Every field is optional, so a body with nothing in it is legal and draws a box with only a
        // name in it.
        "requirement-empty-body",
        "requirementDiagram\n  requirement bare {\n  }\n  element thing {\n  }\n  thing - satisfies -> bare\n",
    ),
    (
        "requirement-chain",
        "requirementDiagram\n  requirement top {\n  id: 1\n  }\n  functionalRequirement mid {\n  id: 1.1\n  }\n  functionalRequirement leaf {\n  id: 1.1.1\n  }\n  element impl {\n  type: code\n  }\n  top - contains -> mid\n  mid - contains -> leaf\n  impl - satisfies -> leaf\n",
    ),
    (
        // `A - r -> B` and `A <- r - B` put the source at opposite ends. Both, on the same pair, is what
        // makes a swapped reading visible.
        "requirement-both-arrows",
        "requirementDiagram\n  requirement a {\n  id: 1\n  }\n  requirement b {\n  id: 2\n  }\n  a - derives -> b\n  a <- traces - b\n",
    ),
    (
        "requirement-long-text",
        "requirementDiagram\n  requirement r {\n  id: 1\n  text: a requirement text long enough that the box has to wrap it over more than one line to stay readable\n  }\n  element e {\n  type: manual\n  }\n  e - verifies -> r\n",
    ),
    (
        "requirement-one-box",
        "requirementDiagram\n  requirement alone {\n  id: 1\n  text: nothing points at it\n  }\n",
    ),
    (
        // **Two** elements. Every case had one, so "an element box holds its own element's rows"
        // and "every element box holds the first element's rows" were the same drawing.
        "requirement-two-elements",
        "requirementDiagram\n  requirement r {\n  id: 1\n  }\n  element rig {\n  type: \
         simulation\n  docRef: docs/rig.md\n  }\n  element bench {\n  type: hardware\n  \
         docRef: docs/bench.md\n  }\n  rig - verifies -> r\n  bench - satisfies -> r\n",
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
    (
        "gitgraph-lr-explicit",
        "gitGraph LR:\n  commit\n  branch feature\n  commit\n  checkout main\n  merge feature\n",
    ),
    (
        "gitgraph-bt",
        "gitGraph BT:\n  commit\n  branch develop\n  commit\n  checkout main\n  commit\n  merge develop\n",
    ),
    (
        // `switch` is `checkout` — the langium grammar lists both spellings.
        "gitgraph-switch",
        "gitGraph\n  commit\n  branch topic\n  switch topic\n  commit\n  switch main\n  merge topic\n",
    ),
    (
        // `order:` overrides which lane a branch is drawn on, which is a statement about the picture and
        // not about the history.
        "gitgraph-branch-order",
        "gitGraph\n  commit\n  branch last order: 9\n  branch first order: 1\n  checkout first\n  commit\n  checkout last\n  commit\n",
    ),
    (
        // Every field may come in any order and `tag:` **accumulates** while the others overwrite.
        "gitgraph-fields-in-any-order",
        "gitGraph\n  commit tag: \"v1\" id: \"one\" type: REVERSE\n  commit type: HIGHLIGHT id: \"two\"\n  commit id: \"three\" tag: \"v2\" tag: \"v3\"\n",
    ),
    (
        // A bare string is the message; `msg:` is optional.
        "gitgraph-messages",
        "gitGraph\n  commit msg: \"with the keyword\"\n  commit \"without it\"\n  commit id: \"third\" msg: \"both\"\n",
    ),
    (
        "gitgraph-many-branches",
        "gitGraph\n  commit\n  branch a\n  commit\n  branch b\n  commit\n  branch c\n  commit\n  checkout main\n  merge a\n  merge b\n  merge c\n",
    ),
    (
        "gitgraph-merge-fields",
        "gitGraph\n  commit\n  branch topic\n  commit\n  checkout main\n  merge topic id: \"the merge\" tag: \"v2.0\" type: HIGHLIGHT\n",
    ),
    (
        "gitgraph-cjk",
        "gitGraph\n  commit id: \"最初\" tag: \"リリース\"\n  branch 開発\n  commit id: \"作業\"\n  checkout main\n  merge 開発\n",
    ),
    (
        "gitgraph-one-commit",
        "gitGraph\n  commit\n",
    ),
    (
        "gitgraph-long-ids",
        "gitGraph\n  commit id: \"a commit message long enough to be wider than the lane it sits on\"\n  commit id: \"short\"\n",
    ),
    (
        // Direction crossed with a cherry-pick: the two were only ever exercised apart.
        "gitgraph-cherry-pick-tb",
        "gitGraph TB:\n  commit id: \"ZERO\"\n  branch develop\n  commit id: \"A\"\n  checkout main\n  cherry-pick id: \"A\"\n",
    ),
    (
        // A tag on **lane 0**, which is the top of the drawing: a tag hangs above its commit, so
        // this is the case whose viewBox has to grow *upward* to hold it. Nothing else in the
        // corpus put anything above the topmost lane.
        "gitgraph-tag-on-the-top-lane",
        "gitGraph\n  commit tag: \"v1.0\"\n  branch develop\n  commit\n  checkout main\n  commit tag: \"v1.1\"\n",
    ),
    (
        // Several tags on one commit **that also has an id**: the tags stack upward and the id
        // stays below, so one commit carries three separate things a reader has to be able to
        // tell apart.
        "gitgraph-tags-over-an-id",
        "gitGraph\n  commit id: \"cut\" tag: \"v2.0.0\" tag: \"stable\" tag: \"lts\"\n  commit id: \"after\"\n",
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
    (
        "c4-component",
        "C4Component\n  title Components of the API\n  Container_Boundary(api, \"API Application\") {\n    Component(sign, \"Sign In Controller\", \"MVC Rest Controller\", \"Allows users to sign in.\")\n    ComponentDb(store, \"Session Store\", \"Redis\", \"Holds sessions.\")\n    ComponentQueue(events, \"Events\", \"Kafka\")\n  }\n  Rel(sign, store, \"Reads and writes\", \"Jedis\")\n",
    ),
    (
        "c4-dynamic",
        "C4Dynamic\n  title A dynamic diagram\n  Person(customer, \"Customer\")\n  Container(spa, \"Single Page App\", \"JavaScript\")\n  Container(api, \"API\", \"Java\")\n  RelIndex(1, customer, spa, \"Submits\")\n  RelIndex(2, spa, api, \"Calls\")\n",
    ),
    (
        "c4-external-and-shapes",
        "C4Context\n  Person(a, \"Inside\", \"A person here.\")\n  Person_Ext(b, \"Outside\", \"A person elsewhere.\")\n  System(c, \"Ours\")\n  System_Ext(d, \"Theirs\")\n  SystemDb(e, \"Our database\")\n  SystemQueue(f, \"Our queue\")\n  Rel(a, c, \"Uses\")\n  Rel(b, d, \"Uses\")\n",
    ),
    (
        "c4-relationship-directions",
        "C4Context\n  System(a, \"A\")\n  System(b, \"B\")\n  System(c, \"C\")\n  Rel_U(a, b, \"up\")\n  Rel_D(a, c, \"down\")\n  Rel_L(b, c, \"left\")\n  Rel_R(c, b, \"right\")\n  BiRel(a, c, \"both ways\")\n  Rel_Back(b, a, \"back\")\n",
    ),
    (
        // `$key="value"` does not consume a position, which is the part a positional reader gets wrong.
        "c4-named-arguments",
        "C4Context\n  Person(a, \"A person\", $tags=\"v1\", \"a description that is still the third positional\")\n  System(b, \"A system\", \"its description\", $tags=\"v2\")\n  Rel(a, b, \"Uses\", $tags=\"v1\")\n",
    ),
    (
        // Appearance, read and dropped — §0-1 settles that konoma decides its own.
        "c4-update-styles",
        "C4Context\n  Person(a, \"A\")\n  System(b, \"B\")\n  Rel(a, b, \"Uses\")\n  UpdateElementStyle(a, $fontColor=\"red\", $bgColor=\"grey\")\n  UpdateRelStyle(a, b, $textColor=\"blue\", $offsetY=\"-10\")\n  UpdateLayoutConfig($c4ShapeInRow=\"3\", $c4BoundaryInRow=\"1\")\n",
    ),
    (
        "c4-cjk",
        "C4Context\n  title システム構成\n  Enterprise_Boundary(b0, \"社内\") {\n    Person(user, \"利用者\", \"社内の担当者。\")\n    System(app, \"業務システム\", \"申請を受け付ける。\")\n  }\n  System_Ext(mail, \"メール基盤\", \"社外の配信基盤。\")\n  Rel(user, app, \"利用する\")\n  Rel(app, mail, \"送信する\", \"SMTP\")\n",
    ),
    (
        // Three frames inside one another, which is where a frame that forgot its parent stops being
        // checked at all (§6-A item 14).
        "c4-boundaries-three-deep",
        "C4Deployment\n  Deployment_Node(dc, \"Data centre\", \"Ubuntu\") {\n    Deployment_Node(host, \"Host\", \"Docker\") {\n      Deployment_Node(pod, \"Pod\", \"Kubernetes\") {\n        Container(api, \"API\", \"Java\")\n      }\n    }\n  }\n  Person(u, \"User\")\n  Rel(u, api, \"Uses\", \"HTTPS\")\n",
    ),
    (
        "c4-boundary-brace-on-the-next-line",
        "C4Context\n  System_Boundary(b1, \"Boundary\")\n  {\n    System(a, \"A\")\n    System(b, \"B\")\n  }\n  Rel(a, b, \"Uses\")\n",
    ),
    (
        "c4-flat",
        "C4Context\n  title No boundaries at all\n  Person(a, \"A\")\n  System(b, \"B\")\n  System(c, \"C\")\n  Rel(a, b, \"Uses\")\n  Rel(b, c, \"Calls\", \"gRPC\")\n",
    ),
    (
        "c4-long-descriptions",
        "C4Context\n  Person(a, \"A person with a long name indeed\", \"A description long enough that the box has to wrap it over several lines before it can be read.\")\n  System(b, \"B\")\n  Rel(a, b, \"Uses in a way that needs explaining at length\", \"HTTPS/1.1\")\n",
    ),
    (
        // Two frames side by side with a line between them: the case that separates "inside its own
        // frame" from "inside some frame".
        "c4-two-boundaries",
        "C4Context\n  Enterprise_Boundary(left, \"Left\") {\n    System(a, \"A\")\n  }\n  Enterprise_Boundary(right, \"Right\") {\n    System(b, \"B\")\n  }\n  Rel(a, b, \"Crosses\")\n",
    ),
    (
        "c4-direction",
        "C4Context\n  direction LR\n  Person(a, \"A\")\n  System(b, \"B\")\n  Rel(a, b, \"Uses\")\n",
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
    (
        // A `block:` inside a `block:` is documented syntax and was not in the corpus, which is how a
        // nested frame that never recorded its parent survived (§6-A item 14).
        "block-nested",
        "block-beta\n  columns 1\n  block:outer\n    columns 2\n    x y\n    block:inner\n      columns 1\n      z w\n    end\n    q\n  end\n  tail\n",
    ),
    (
        // An edge between two blocks **inside** different frames, and one between the frames themselves.
        "block-nested-edge",
        "block-beta\n  columns 2\n  block:left\n    columns 1\n    a\n    b\n  end\n  block:right\n    columns 1\n    c\n  end\n  a --> c\n  left --> right\n",
    ),
    (
        "block-shapes",
        "block-beta\n  columns 4\n  a[\"square\"] b(\"round\") c([\"stadium\"]) d[[\"subroutine\"]]\n  e[(\"cylinder\")] f((\"circle\")) g{\"diamond\"} h{{\"hexagon\"}}\n  i[/\"lean right\"/] j[\\\"lean left\"\\] k[/\"trapezoid\"\\] l[\\\"inv trapezoid\"/]\n",
    ),
    (
        // `space:n` leaves n cells empty, which is the only way to say "nothing here" on this grid.
        "block-space-sized",
        "block-beta\n  columns 4\n  a space:2 b\n  c d e f\n",
    ),
    (
        "block-arrows-vertical",
        "block-beta\n  columns 3\n  up<[\"up\"]>(up) down<[\"down\"]>(down) both<[\"y\"]>(y)\n",
    ),
    (
        // A composite may span columns too — `block:id:n`.
        "block-composite-width",
        "block-beta\n  columns 3\n  block:wide:2\n    columns 1\n    a\n    b\n  end\n  c\n  d e f\n",
    ),
    (
        "block-cjk",
        "block-beta\n  columns 3\n  設定[\"設定を読む\"] 解決[\"種別を解決\"] 描画[\"全画面で描く\"]\n  設定 --> 解決\n  解決 --> 描画\n",
    ),
    (
        // `columns auto` is `-1` and means one row as wide as it needs to be. It is also the default,
        // so writing it is the only way to tell the two apart.
        "block-columns-auto",
        "block-beta\n  columns auto\n  a b c d\n",
    ),
    (
        "block-styles",
        "block-beta\n  columns 2\n  a b\n  classDef hot fill:#f96\n  class a hot\n  style b fill:#333\n  a --> b\n",
    ),
    (
        "block-one",
        "block-beta\n  only\n",
    ),
    (
        "block-long-label",
        "block-beta\n  columns 2\n  a[\"A label long enough that the cell it sits in has to grow or wrap it\"] b[\"ok\"]\n",
    ),
    (
        "block-twelve",
        "block-beta\n  columns 4\n  a b c d\n  e f g h\n  i j k l\n  a --> l\n",
    ),
    (
        "block-edges-every-stroke",
        "block-beta\n  columns 5\n  a b c d e\n  a --> b\n  b --- c\n  c -.-> d\n  d ==> e\n  a -- \"labelled\" --> e\n",
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
    (
        // `group … in …` is documented syntax and was not in the corpus, which is how a group frame that
        // escaped its parent by twelve pixels survived a whole stage.
        "architecture-nested",
        "architecture-beta\n  group outer(cloud)[Outer]\n  group inner(cloud)[Inner] in outer\n  service s1(server)[S1] in inner\n  service s2(server)[S2] in outer\n  s1:R --> L:s2\n",
    ),
    (
        "architecture-nested-thrice",
        "architecture-beta\n  group a(cloud)[A]\n  group b(cloud)[B] in a\n  group c(cloud)[C] in b\n  service s(server)[S] in c\n  service t(server)[T] in a\n",
    ),
    (
        "architecture-titled-edges",
        "architecture-beta\n  service web(server)[Web]\n  service db(database)[Database]\n  service cache(disk)[Cache]\n  web:R -[reads]- L:db\n  web:B -[writes]- T:cache\n",
    ),
    (
        // All four end decorations: none, one at the far end, one at the near end, and both.
        //
        // Written as one chain on purpose. Two edges reaching the same service is a *different*
        // case — the grid walk honours the first and contradicts the second — and it is stated
        // on its own below rather than being smuggled in here, where it would only look like an
        // arrowhead test that fails.
        "architecture-arrow-forms",
        "architecture-beta\n  service a(server)[A]\n  service b(server)[B]\n  service c(server)[C]\n  service d(server)[D]\n  service e(server)[E]\n  a:R -- L:b\n  b:R --> L:c\n  c:R <-- L:d\n  d:R <--> L:e\n",
    ),
    (
        // `{group}` attaches the line to the group's border upstream. konoma reads it and draws to the
        // service, which is the same two things joined — but it has to be *read*, or it becomes part of
        // the id.
        "architecture-group-edge-end",
        "architecture-beta\n  group g(cloud)[G]\n  service a(server)[A] in g\n  service b(server)[B]\n  a{group}:R --> L:b\n",
    ),
    (
        "architecture-cjk",
        "architecture-beta\n  group 社内(cloud)[社内ネットワーク]\n  service 蓄積(database)[データベース] in 社内\n  service 配信(server)[配信サーバ] in 社内\n  蓄積:R -- L:配信\n",
    ),
    (
        "architecture-quoted-icon",
        "architecture-beta\n  service a(\"🚀\")[Launcher]\n  service b(\"📦\")[Store]\n  a:R --> L:b\n",
    ),
    (
        "architecture-align-column",
        "architecture-beta\n  service a(server)[A]\n  service b(server)[B]\n  service c(server)[C]\n  a:R --> L:c\n  b:R --> L:c\n  align column a b\n",
    ),
    (
        "architecture-one-service",
        "architecture-beta\n  service alone(server)[Alone]\n",
    ),
    (
        "architecture-long-labels",
        "architecture-beta\n  service a(server)[A service whose name is long enough to widen its own cell]\n  service b(disk)[Short]\n  a:R -- L:b\n",
    ),
    (
        "architecture-many-services",
        "architecture-beta\n  service s1(server)[S1]\n  service s2(server)[S2]\n  service s3(server)[S3]\n  service s4(server)[S4]\n  service s5(server)[S5]\n  service s6(server)[S6]\n  s1:R -- L:s2\n  s2:R -- L:s3\n  s4:R -- L:s5\n  s5:R -- L:s6\n  s1:B -- T:s4\n",
    ),
    (
        "architecture-junction-cross",
        "architecture-beta\n  service n(server)[North]\n  service s(server)[South]\n  service e(server)[East]\n  service w(server)[West]\n  junction j\n  n:B -- T:j\n  s:T -- B:j\n  e:L -- R:j\n  w:R -- L:j\n",
    ),
    (
        // A service in no group at all, beside two that are: "inside the groups it is written in, and
        // inside no other" has to hold for a service written in none.
        "architecture-groups-and-a-loose-service",
        "architecture-beta\n  group g1(cloud)[One]\n  service a(server)[A] in g1\n  group g2(cloud)[Two]\n  service b(server)[B] in g2\n  service loose(server)[Loose]\n  a:R --> L:b\n  b:B --> T:loose\n",
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
    (
        "zenuml-par-and-opt",
        "zenuml\n  A->B: start\n  par {\n    A->B: one\n    A->C: two\n  }\n  opt {\n    B->A: maybe\n  }\n",
    ),
    (
        "zenuml-try-catch-finally",
        "zenuml\n  Client->Service: call\n  try {\n    Service->Db: read\n  } catch {\n    Service->Client: error\n  } finally {\n    Service->Client: done\n  }\n",
    ),
    (
        "zenuml-new",
        "zenuml\n  @Starter(Client)\n  new Order(id)\n  Order.total() {\n    return 42\n  }\n",
    ),
    (
        // `x = A.m()` is the call **and** a dotted reply labelled `x`; a typed declaration in front of it
        // is the same statement.
        "zenuml-assignment",
        "zenuml\n  total = Order.total()\n  int count = Cart.size()\n  Cart.clear()\n",
    ),
    (
        "zenuml-else-if",
        "zenuml\n  A->B: ask\n  if(first) {\n    B->A: one\n  } else if(second) {\n    B->A: two\n  } else {\n    B->A: three\n  }\n",
    ),
    (
        "zenuml-annotators",
        "zenuml\n  @Actor Alice\n  @Database Store\n  @Boundary Gate\n  Alice->Gate: request\n  Gate->Store: query\n  Store->Alice: rows\n",
    ),
    (
        "zenuml-cjk",
        "zenuml\n  title 注文の流れ\n  @Actor 利用者\n  利用者->受付: 注文する\n  受付->倉庫: 在庫を確認\n  倉庫->受付: 在庫あり\n  受付->利用者: 受け付けました\n",
    ),
    (
        "zenuml-comments",
        "zenuml\n  // a comment, not a participant\n  A->B: one\n  // another\n  B->A: two\n",
    ),
    (
        "zenuml-one-message",
        "zenuml\n  A->B: only\n",
    ),
    (
        "zenuml-deep-nesting",
        "zenuml\n  Client->A.one() {\n    B.two() {\n      C.three() {\n        D.four() {\n          return d\n        }\n        return c\n      }\n      return b\n    }\n    return a\n  }\n",
    ),
    (
        "zenuml-long-message",
        "zenuml\n  A->B: a message long enough that the arrow it labels has to be widened or the words wrapped\n  B->A: ok\n",
    ),
    (
        // A frame inside a frame, in the one language here that is a sequence diagram: the nesting case
        // the corpus had in no other form.
        "zenuml-loop-inside-alt",
        "zenuml\n  A->B: begin\n  if(needed) {\n    while(more) {\n      A->B: again\n    }\n  } else {\n    A->B: skip\n  }\n",
    ),
    (
        "zenuml-reply-forms",
        "zenuml\n  Client->Service: call\n  @return\n  Service->Client: reply\n  Service.work() {\n    return done\n  }\n",
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

/// Which band each row of a banded diagram belongs to, counted in **runs**.
///
/// The four banded languages all draw one frame per run of consecutive rows sharing a section
/// name, so a name written twice with another between the two is two frames. Counting the runs
/// here — from the model, by the language's own rule — is what lets a test name the *second*
/// `Design` band rather than asking for `Design` and being handed the first.
fn section_runs<'a>(names: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut open: Option<&str> = None;
    let mut run = 0usize;
    for name in names {
        match open {
            Some(previous) if previous == name => {}
            Some(_) => run += 1,
            None => {}
        }
        open = Some(name);
        out.push(run);
    }
    out
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
        // A git graph's caption is the commit's **id**. It used to be spelled with a `tag:` here,
        // which stopped being a bare label the moment a tag grew a ticket round it — and a tag is
        // instrumented in `a_boxed_label_still_fits_the_box_konoma_sized_for_it` instead, because
        // its box is its ink plus what `shapes::size` adds.
        ("gitgraph caption", "gitGraph\n  commit id: \"{}\"\n"),
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
        ("gitgraph tag", "gitGraph\n  commit tag: \"{}\"\n"),
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

        // **A merge's two lines come down two different lanes.** That is the whole of what a
        // merge dot says — this history joined two branches — and it was guarded by the corpus
        // golden alone: a mutation that routed both lines down the child's own lane left every
        // commit on its own lane, every commit in order, and a picture that no longer showed
        // where the merge came from.
        for commit in model.commits.iter().filter(|c| c.parents.len() > 1) {
            let tails: HashSet<String> = commit
                .parents
                .iter()
                .filter_map(|p| model.commit(p))
                .map(|p| p.branch.clone())
                .collect();
            if tails.len() < 2 {
                continue;
            }
            let mut arrived: Vec<f64> = d
                .edges
                .iter()
                .filter(|e| e.to == commit.id)
                .filter_map(|e| e.points.first())
                .map(across)
                .collect();
            arrived.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            arrived.dedup_by(|a, b| (*a - *b).abs() < 0.5);
            assert!(
                arrived.len() >= 2,
                "{name}: the merge {:?} joins {:?} and its lines all come down one lane",
                commit.id,
                tails
            );
        }
    }
}

/// **A tag and an id sit on opposite sides of their commit, across the lanes, and a commit that
/// has both gets both.**
///
/// The three things this states are the three the renderer used to get wrong: a tag was drawn in
/// the same band as the id; a tag **replaced** the id, so `merge renderer tag: "v0.27.0"` lost
/// whatever the author had called that merge; and there was no shape to tell one from the other.
/// mermaid draws both, on opposite sides of the dot (`gitGraphRenderer.ts`: `drawCommitLabel` at
/// `y + 25`, `drawCommitTags` at `y - 16 - yOffset`).
///
/// **Stated across the lanes rather than up and down the page**, which is the fourth thing it used
/// to get wrong: in `TB:`/`BT:` up and down is the *time* axis, so a tag hung above its commit sat
/// on that lane's own line and the line was drawn through the middle of the ticket.
///
/// Everything here is looked up **by id** — `{commit}#tag{i}` and `{commit}#caption` — rather than
/// searched for by the words it holds, which is §6-A item 15: a test that finds a label by its
/// text is blind to the two labels swapping places.
#[test]
fn a_git_graph_draws_a_tag_and_an_id_on_opposite_sides_of_their_commit() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph::{self, Direction};
    let mut with_both = 0usize;
    let mut tags_seen = 0usize;
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let model = gitgraph::parse(src).expect("parses");
        let d = laid_out(src);
        // How far out across the lanes a point is. `LR:` spaces the lanes down the page and
        // `TB:`/`BT:` space them across it; the caption's side is the increasing one either way.
        let lr = matches!(model.direction, Direction::LeftToRight);
        let across = |p: &Point| if lr { p.y } else { p.x };
        let across_span = |n: &super::PlacedNode| {
            let (l, t, r, b) = n.bounds();
            if lr {
                (t, b)
            } else {
                (l, r)
            }
        };
        for commit in &model.commits {
            let dot = d
                .node(&commit.id)
                .unwrap_or_else(|| panic!("{name}: commit {:?} was not drawn", commit.id));
            let (dot_near, dot_far) = across_span(dot);

            // The id, on the far side — **only** when the author wrote one, which is the rule that
            // keeps a history of generated ids from being captioned with noise.
            let caption = d.node(&format!("{}#caption", commit.id));
            match (commit.explicit_id, caption) {
                (true, Some(c)) => {
                    assert_eq!(
                        c.label.lines.join(" "),
                        commit.id,
                        "{name}: the caption beside {:?} does not read as its id — a tag is drawn \
                         on the other side of the commit and never in place of the id",
                        commit.id
                    );
                    assert!(
                        across_span(c).0 >= dot_far - 0.01,
                        "{name}: the id beside {:?} reaches back over its own commit",
                        commit.id
                    );
                }
                (true, None) => panic!(
                    "{name}: {:?} was given an id by the source and nothing is drawn beside it",
                    commit.id
                ),
                (false, Some(c)) => panic!(
                    "{name}: {:?} has a generated id and is captioned {:?} anyway",
                    commit.id,
                    c.label.lines.join(" ")
                ),
                (false, None) => {}
            }

            // The tags, on the near side — one node each, in source order, each holding its own
            // words.
            for (i, tag) in commit.tags.iter().enumerate() {
                let node = d.node(&format!("{}#tag{i}", commit.id)).unwrap_or_else(|| {
                    panic!("{name}: tag {tag:?} on {:?} was not drawn", commit.id)
                });
                assert_eq!(
                    node.shape,
                    Glyph::Tag,
                    "{name}: the tag {tag:?} is drawn as a {:?} — a tag a reader cannot tell from \
                     a caption is the defect this glyph exists to fix",
                    node.shape
                );
                assert_eq!(
                    node.label.lines.join(" "),
                    text_metrics::collapse_spaces(tag).into_owned(),
                    "{name}: the {i}th tag of {:?} holds another tag's words",
                    commit.id
                );
                assert!(
                    across_span(node).1 <= dot_near + 0.01,
                    "{name}: the tag {tag:?} is drawn on its commit {:?} or past it — a tag \
                     belongs on the other side of the dot from the id",
                    commit.id
                );
                // Beside the dot **and** clear of the line its commit sits on, which runs straight
                // through the commit's own centre: a tag drawn on the lane is a tag with a rule
                // through the middle of it.
                assert!(
                    across_span(node).1 < across(&dot.center) - super::shapes::COMMIT_RADIUS,
                    "{name}: the tag {tag:?} reaches back onto its own lane line",
                );
                tags_seen += 1;
            }
            if !commit.tags.is_empty() && commit.explicit_id {
                with_both += 1;
            }
        }
    }
    assert!(tags_seen >= 8, "only {tags_seen} tags were checked");
    assert!(
        with_both > 0,
        "no corpus case has a commit carrying a tag **and** an id, which is the case where \
         drawing one in place of the other loses something"
    );
}

/// **Several tags on one commit stack outward, in the order they were written, without touching.**
///
/// Nearest the commit is the tag written first, so the column reads the way the source does.
/// (mermaid stacks the *last* one nearest, because `drawCommitTags` walks `commit.tags.reverse()`;
/// §0-1 lets konoma read the column in source order rather than backwards.)
///
/// Stated as a gap rather than as a coordinate, so it catches the collapse — every tag drawn in
/// one place — as well as a stack that runs the wrong way. "Outward" is away from the lane across
/// the lanes, which is up the page in `LR:` and to the left of it in `TB:`/`BT:`.
#[test]
fn a_git_graphs_tags_stack_outward_in_the_order_they_were_written() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph::{self, Direction};
    let mut stacks = 0usize;
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let model = gitgraph::parse(src).expect("parses");
        let d = laid_out(src);
        let lr = matches!(model.direction, Direction::LeftToRight);
        // (near, far) across the lanes: the edge of the box toward the lane, and away from it.
        let span = |n: &super::PlacedNode| {
            let (l, t, r, b) = n.bounds();
            if lr {
                (b, t)
            } else {
                (r, l)
            }
        };
        for commit in model.commits.iter().filter(|c| c.tags.len() > 1) {
            stacks += 1;
            let tags: Vec<&super::PlacedNode> = (0..commit.tags.len())
                .map(|i| {
                    d.node(&format!("{}#tag{i}", commit.id))
                        .unwrap_or_else(|| panic!("{name}: {:?} lost a tag", commit.id))
                })
                .collect();
            for (i, pair) in tags.windows(2).enumerate() {
                let (inner, outer) = (pair[0], pair[1]);
                // Outward is the *decreasing* coordinate in both projections — up the page in
                // `LR:`, left across it in `TB:`/`BT:` — so one comparison covers all three.
                let gap = span(inner).1 - span(outer).0;
                assert!(
                    gap >= -0.01,
                    "{name}: the tags {:?} and {:?} on {:?} are drawn in the same place — a \
                     stack that collapsed is one tag on top of another",
                    commit.tags[i],
                    commit.tags[i + 1],
                    commit.id
                );
                assert!(
                    gap >= super::gitgraph::TAG_STACK - 0.01,
                    "{name}: the tags {:?} and {:?} on {:?} touch",
                    commit.tags[i],
                    commit.tags[i + 1],
                    commit.id
                );
            }
        }
    }
    assert!(
        stacks > 0,
        "no corpus case puts more than one tag on a commit"
    );
}

/// **A tag on the topmost lane makes the drawing taller instead of being clipped.**
///
/// A tag hangs above its commit, so a tag on lane 0 hangs above everything — and `normalise` is
/// what has to notice. The invariant suite already runs
/// [`check_view_box_contains_everything`] over the corpus; this says the *reason* it passes,
/// because "nothing is outside the box" is also true of a drawing that never grew the thing that
/// would have stuck out.
#[test]
fn a_tag_on_the_top_lane_makes_the_drawing_taller_instead_of_being_clipped() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph;
    let (name, src) = CASES
        .iter()
        .find(|(n, _)| *n == "gitgraph-tag-on-the-top-lane")
        .copied()
        .expect("the corpus has a tag on the top lane");
    let model = gitgraph::parse(src).expect("parses");
    let d = laid_out(src);
    let top = model
        .commits
        .iter()
        .find(|c| !c.tags.is_empty())
        .expect("a tagged commit");
    let tag = d
        .node(&format!("{}#tag0", top.id))
        .expect("the tag is drawn");
    let dot = d.node(&top.id).expect("the commit is drawn");

    // It is the highest thing on the page: above its own commit, and above every lane name.
    assert!(
        tag.bounds().1 < dot.bounds().1,
        "{name}: the tag is not above its commit"
    );
    for n in &d.nodes {
        assert!(
            n.bounds().1 >= tag.bounds().1 - 0.01,
            "{name}: {} is drawn above the tag on the topmost lane",
            n.id
        );
    }
    // …and the page starts at the margin above it, rather than cutting it off.
    assert!(
        (tag.bounds().1 - super::MARGIN).abs() < 0.01,
        "{name}: the drawing starts {} above the tag instead of at the margin",
        svg::num(tag.bounds().1)
    );
    assert!(
        tag.bounds().3 <= d.height + 0.01 && tag.bounds().1 >= -0.01,
        "{name}: the tag is outside the viewBox"
    );
}

/// **A `TB:`/`BT:` git graph does not open with a band of empty page.**
///
/// Time starts at `widest branch name + NAME_GAP` in every direction, which is right in `LR:` —
/// the names really do sit before the first commit there, and their width is what has to be got
/// past. In `TB:`/`BT:` they sit *above* the lanes instead, so the same offset is added to the top
/// of the drawing as blank page: `gitGraph TB:` opens with about sixty pixels of nothing between
/// its branch names and its first commit. What should be true is that the gap is [`NAME_GAP`], the
/// same as the one the names were placed with.
///
/// Left ignored deliberately. It is a defect with a cause of its own — an `LR:` sentence applied
/// to three directions — and not part of the label-aware steps, which is what the work around it
/// was for; fixing it moves every `TB:`/`BT:` golden a second time for an unrelated reason.
/// `docs/STATUS.md` is where it belongs.
///
/// [`NAME_GAP`]: super::gitgraph::NAME_GAP
#[test]
#[ignore = "known defect, out of scope of the label-aware steps: a `TB:`/`BT:` git graph uses the \
            `LR:` origin, so the widest branch name's *width* is added to the top of the drawing \
            as blank page"]
fn a_top_to_bottom_git_graph_does_not_open_with_a_band_of_empty_page() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph::{self, Direction};
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let model = gitgraph::parse(src).expect("parses");
        if matches!(model.direction, Direction::LeftToRight) {
            continue;
        }
        let d = laid_out(src);
        // How far down the page the drawing starts, and how far down the names stop. In `BT:` time
        // runs the other way but the names are still at the top, so both are read in page order.
        let names_end = d
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("branch#"))
            .map(|n| n.bounds().3)
            .fold(f64::NEG_INFINITY, f64::max);
        let first = d
            .nodes
            .iter()
            .filter(|n| !n.id.starts_with("branch#"))
            .map(|n| n.bounds().1)
            .fold(f64::INFINITY, f64::min);
        assert!(
            first - names_end <= super::gitgraph::NAME_GAP + 0.01,
            "{name}: {} of empty page between the branch names and the drawing, where {} was \
             asked for",
            svg::num(first - names_end),
            svg::num(super::gitgraph::NAME_GAP)
        );
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
        // Everything that legitimately reaches past the plot, named: the names column and the
        // gap to the plot, the tick label centred on **each** end of the axis (half of it hangs
        // over), half a milestone's diamond, the section frames' padding on each side, and the
        // two margins `normalise` adds.
        //
        // The left-hand overhang is not decoration. A tick label is centred on the plot's left
        // edge, so half of it reaches back into the names column — and where the row names are
        // short it reaches past them, which pushes the whole drawing right when `normalise`
        // moves the origin. A bound that only allowed for the right-hand half therefore called a
        // correctly drawn chart too wide the first time one came along with narrow names.
        let names_column = names + super::chart::TICK_GAP * 2.0;
        let overhang_left = (widest_tick / 2.0 - names_column).max(0.0);
        let overhang_right = (widest_tick / 2.0).max(super::gantt::MILESTONE / 2.0);
        let bound = overhang_left
            + names_column
            + plot
            + overhang_right
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
        //
        // A band is **a run of consecutive tasks sharing a section**, which is the language's own
        // rule and not the renderer's — a section name written twice with another between the two
        // is two bands. So the ordinal is counted here rather than the name being looked up, and
        // a chart that put the third task in the first `Design` band would be caught instead of
        // being read as correct.
        let ordinals = section_runs(model.tasks.iter().map(|t| t.section.as_str()));
        for (i, task) in model.tasks.iter().enumerate() {
            if task.section.trim().is_empty() {
                continue;
            }
            let (Some(bar), Some(frame)) = (
                d.node(&format!("task#{i}")),
                d.cluster(&format!("section#{}#{}", ordinals[i], task.section)),
            ) else {
                panic!("{name}: task {i} has no bar or its own band was not drawn");
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

/// **The same statement, on a source where two ported edges reach one service.**
///
/// `a:R -- L:b`, `b:R --> L:c`, `c:R <-- L:d` and `a:B <--> T:d` are all satisfiable together —
/// `a(0,0) b(1,0) c(2,0) d(3,1)` honours every one of them — and konoma does not find it. The
/// placement is a single walk of the edges (`place_on_grid`): a service is put one cell along
/// from whichever edge reached it **first**, and any later edge that also names it is drawn and
/// not obeyed. Repairing it afterwards does not converge — moving `c` to satisfy `b:R` breaks
/// `c:R`, and moving it back breaks `b:R` again — so the fix is a constraint solve over the
/// grid, not a patch to the walk, and that is a piece of work rather than a correction.
///
/// Left here, ignored, because the alternative is that nothing states it at all: the live test
/// above runs on chains, where each service is reached once, and would go on passing forever
/// while this shape stayed broken.
#[test]
#[ignore = "known limitation, recorded in docs/STATUS.md: the architecture grid is placed by one \
            greedy walk, so a service reached by two ported edges honours the first and \
            contradicts the second; satisfying both needs a constraint solve"]
fn an_architecture_edges_ports_are_honoured_even_when_two_edges_reach_one_service() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture::{self, Side};
    let name = "architecture-two-edges-reach-one-service";
    let src = "architecture-beta\n  service a(server)[A]\n  service b(server)[B]\n  service \
               c(server)[C]\n  service d(server)[D]\n  a:R -- L:b\n  b:R --> L:c\n  c:R <-- \
               L:d\n  a:B <--> T:d\n";
    let model = architecture::parse(src).expect("parses");
    let d = laid_out(src);
    for e in &model.edges {
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

/// **A service is inside every group it is written in, and inside no other.**
///
/// *Every* group, not one: `group inner in outer` is documented syntax, and a service in `inner`
/// is in `outer` too — a frame drawn inside another frame holds what that one holds. Saying
/// "inside exactly one" would be a statement about the language that is not true, and it would
/// come due the first time a nested group reached the corpus.
#[test]
fn every_architecture_service_is_inside_the_groups_it_is_written_in() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture;
    for (name, src) in cases_of(architecture::is_architecture) {
        let model = architecture::parse(src).expect("parses");
        let d = laid_out(src);
        // Whether a service written into `group` is, directly or through the groups between,
        // written into `want`. Walked here rather than borrowed from the renderer, which has the
        // same walk and would be checking itself.
        let written_in = |group: Option<&str>, want: &str| {
            let mut at = group;
            for _ in 0..model.groups.len() + 1 {
                let Some(id) = at else { return false };
                if id == want {
                    return true;
                }
                at = model
                    .groups
                    .iter()
                    .find(|g| g.id == id)
                    .and_then(|g| g.parent.as_deref());
            }
            false
        };
        for s in &model.services {
            let Some(node) = d.node(&s.id) else { continue };
            let (l, t, r, b) = node.bounds();
            for group in &model.groups {
                let Some(frame) = d.cluster(&group.id) else {
                    continue;
                };
                let (fl, ft, fr, fb) = frame.bounds();
                let inside = l >= fl - 0.5 && t >= ft - 0.5 && r <= fr + 0.5 && b <= fb + 0.5;
                let mine = written_in(s.group.as_deref(), &group.id);
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

/// Whether a block diagram puts a `block:` inside a `block:`.
fn a_composite_holds_a_composite(items: &[crate::preview::mermaid::block::Item]) -> bool {
    use crate::preview::mermaid::block::Item;
    items.iter().any(|item| match item {
        Item::Composite(c) => {
            c.children.iter().any(|k| matches!(k, Item::Composite(_)))
                || a_composite_holds_a_composite(&c.children)
        }
        _ => false,
    })
}

/// Whether **this source** nests one frame inside another, read from its own model.
///
/// Read from the model and not from the drawing, because the drawing is what is on trial: a
/// renderer that lost the nesting would answer "no frames nest here" and skip its own defect.
/// Read from the model and not from a list of case names, because a name list stops covering the
/// corpus the moment somebody adds a case — the rule §6-A item 10 settled.
fn the_source_nests_a_frame(src: &str) -> bool {
    use crate::preview::mermaid as m;
    if m::architecture::is_architecture(src) {
        return m::architecture::parse(src)
            .map(|a| a.groups.iter().any(|g| g.parent.is_some()))
            .unwrap_or(false);
    }
    if m::block::is_block_diagram(src) {
        return m::block::parse(src)
            .map(|b| a_composite_holds_a_composite(&b.items))
            .unwrap_or(false);
    }
    if m::c4::is_c4(src) {
        return m::c4::parse(src)
            .map(|c| c.boundaries.iter().any(|b| b.parent.is_some()))
            .unwrap_or(false);
    }
    false
}

/// **A frame drawn inside another stays inside it — and says whose it is.**
///
/// The second half is not decoration. A [`PlacedCluster`] with no `parent` is not a frame missing
/// an attribute, it is a **switched-off check**: `check_nested_clusters_sit_inside_their_parent`
/// skips a parentless frame, so a test that only called it would have gone green on exactly the
/// defect it was written for, and `check_unrelated_clusters_do_not_overlap` would read a frame
/// and the frame around it as strangers and call their nesting a collision. So the drawing is
/// required to say whose each nested frame is *before* the invariants are called — and which
/// sources are required to say it is decided by [`the_source_nests_a_frame`], from the source.
///
/// [`PlacedCluster`]: super::PlacedCluster
#[test]
fn a_frame_drawn_inside_another_stays_inside_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut checked = 0usize;
    for (name, src) in CASES {
        if !the_source_nests_a_frame(src) {
            continue;
        }
        checked += 1;
        let d = laid_out(src);
        let nested = d.clusters.iter().filter(|c| c.parent.is_some()).count();
        assert!(
            nested > 0,
            "{name}: the source puts one frame inside another and not one drawn frame says which \
             frame it is inside, so the two invariants below would assert nothing about it"
        );
        check_nested_clusters_sit_inside_their_parent(name, &d);
        check_unrelated_clusters_do_not_overlap(name, &d);
        check_view_box_contains_everything(name, &d);

        // …and a child's frame is *strictly* smaller than its parent's, which is what makes the
        // nesting readable rather than two borders drawn on top of each other.
        for c in d.clusters.iter().filter(|c| c.parent.is_some()) {
            let parent = d
                .cluster(c.parent.as_deref().expect("has one"))
                .unwrap_or_else(|| {
                    panic!("{name}: frame {} names a parent that is not drawn", c.id)
                });
            assert!(
                c.size.w < parent.size.w && c.size.h < parent.size.h,
                "{name}: frame {} is {}x{} inside a parent that is {}x{}",
                c.id,
                svg::num(c.size.w),
                svg::num(c.size.h),
                svg::num(parent.size.w),
                svg::num(parent.size.h)
            );
        }
    }
    assert!(
        checked >= 3,
        "only {checked} corpus sources put a frame inside a frame, and all three languages that \
         can (architecture, block, C4) are supposed to be represented"
    );
}

/// **Two things drawn in one diagram do not share an id.**
///
/// `Diagram::node` and `Diagram::cluster` answer with the *first* match, so a duplicate id is not
/// an untidy label — it is a lookup that silently reads the wrong thing, and every test that goes
/// through one of them stops being able to tell the two apart. That is how a gantt section
/// written twice, and a kanban column written twice, made a correctly drawn diagram look wrong
/// while hiding the case where it really would have been.
///
/// Stated over the whole corpus rather than for the kinds it has already bitten, because the ids
/// several of these renderers use come from the author's own text and an author may repeat it.
#[test]
fn no_two_things_in_one_diagram_share_an_id() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        for (what, ids) in [
            ("nodes", d.nodes.iter().map(|n| &n.id).collect::<Vec<_>>()),
            (
                "frames",
                d.clusters.iter().map(|c| &c.id).collect::<Vec<_>>(),
            ),
        ] {
            let mut seen: HashSet<&String> = HashSet::new();
            for id in ids {
                assert!(
                    seen.insert(id),
                    "{name}: two {what} are both called {id:?}, so a lookup for it answers with \
                     one of them and never the other"
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
        for (ci, column) in model.columns.iter().enumerate() {
            let frame = d
                .cluster(&format!("column#{ci}"))
                .unwrap_or_else(|| panic!("{name}: column {:?} has no frame", column.id));
            let (fl, ft, fr, fb) = frame.bounds();
            for (k, card) in column.cards.iter().enumerate() {
                let node = d
                    .node(&format!("card#{ci}#{k}"))
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
            // …and the **tie** between the period and its events runs down that period's own
            // column. Found by a mutation that moved the line one column over and was caught by
            // nothing at all: the events were still checked to be in their own column, and the
            // line that claims them was checked by nobody — so the drawing said the events below
            // belonged to the period beside them and every assertion passed.
            if !model.periods[i].events.is_empty() {
                let mine = d.edges.iter().any(|e| {
                    e.points.iter().all(|p| match model.direction {
                        Direction::LeftToRight => (p.x - node.center.x).abs() < 0.5,
                        Direction::TopToBottom => (p.y - node.center.y).abs() < 0.5,
                    })
                });
                assert!(
                    mine,
                    "{name}: period {i} has events and no line joining them runs along its own \
                     column"
                );
            }
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

/// Everything in a git graph that is made of words: the captions, the tickets, the branch names
/// and the title.
///
/// Read off the drawing by **glyph**, not by id, so a label added later is covered by every test
/// below without one of them being edited — §6-A item 10 (a test that enumerates what it checks
/// stops being a net the moment something new is drawn).
fn git_graph_labels(d: &Diagram) -> Vec<&super::PlacedNode> {
    d.nodes
        .iter()
        .filter(|n| matches!(n.shape, Glyph::Tag | Glyph::ChartLabel))
        .collect()
}

/// Whether the segment `a`–`b` passes through the axis-aligned box `(l, t, r, bottom)`.
///
/// Liang–Barsky, exactly, rather than sampling the segment: a 2px line crossing a box is a thin
/// thing to hit with samples, and "the test missed it" is the failure this whole file exists to
/// avoid. The box is shrunk by a hair first so that a line ending *on* a boundary — which is what
/// every line into a dot does — is not counted as passing through.
fn segment_meets_box(a: &Point, b: &Point, (l, t, r, bottom): (f64, f64, f64, f64)) -> bool {
    let (l, t, r, bottom) = (l + 0.01, t + 0.01, r - 0.01, bottom - 0.01);
    if l >= r || t >= bottom {
        return false;
    }
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.x - l),
        (dx, r - a.x),
        (-dy, a.y - t),
        (dy, bottom - a.y),
    ] {
        if p.abs() < f64::EPSILON {
            if q < 0.0 {
                return false;
            }
        } else {
            let at = q / p;
            if p < 0.0 {
                if at > hi {
                    return false;
                }
                lo = lo.max(at);
            } else {
                if at < lo {
                    return false;
                }
                hi = hi.min(at);
            }
        }
    }
    lo <= hi
}

/// **Two of a git graph's labels do not overlap.**
///
/// Seen in the gallery, not by a test: `Reverse` and `Highlight`, two commits apart, were drawn
/// over one another and read as the single word `ReverseHighlight`. The step between commits used
/// to be a constant, so nothing in the drawing could give way — and a constant cannot know how
/// wide a label is. Both steps are now derived from the measured labels
/// ([`COMMIT_STEP`] and [`LANE_STEP`] are the *floors* they start from), which is how every other
/// renderer here sizes a column.
///
/// **A tag is in the same family** and is checked here rather than in a test of its own: a ticket
/// is a label too, and one piece of work fixes both. So is a **branch name**, which sits on its
/// own lane in `LR:` and takes up room in that lane's rows.
///
/// Nothing is clipped, truncated or wrapped to make it fit; konoma does not do that anywhere, so
/// the only thing that can give way is the step.
///
/// [`COMMIT_STEP`]: super::gitgraph::COMMIT_STEP
/// [`LANE_STEP`]: super::gitgraph::LANE_STEP
#[test]
fn no_two_git_graph_captions_are_drawn_on_top_of_each_other() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph;
    let mut pairs = 0usize;
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let d = laid_out(src);
        let labels = git_graph_labels(&d);
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                pairs += 1;
                let (al, at, ar, ab) = a.bounds();
                let (bl, bt, br, bb) = b.bounds();
                let overlap = al < br && bl < ar && at < bb && bt < ab;
                assert!(
                    !overlap,
                    "{name}: the labels {:?} and {:?} are drawn on top of one another",
                    a.label.lines.join(" "),
                    b.label.lines.join(" ")
                );
            }
        }
    }
    assert!(pairs > 100, "only {pairs} pairs of labels were compared");
}

/// **Every git graph label stays inside its own lane's column.**
///
/// The other half of the collision: a caption hangs toward the next lane and a stack of tags
/// toward the previous one, and both used to be drawn at a fixed pitch. `cherry-pick:MERGE` on the
/// `release` lane came within 6px of `develop`'s line and 38 from its own, so it read as
/// `develop`'s ticket. [`LANE_STEP`] is now a floor and the pitch is derived from the labels.
///
/// A lane's **column** is its own side of the midline between it and the next lane, and the
/// midline is what this checks against. "Does not overlap anything on the next lane" is the
/// tempting weaker sentence and it is not enough: it was true of `cherry-pick:MERGE` the whole
/// time — the ticket lay across `develop`'s row at a point in time where `develop` happened to
/// have no label — and a mutation putting the pitch back to a constant passed a test written that
/// way.
///
/// Every lane's position is **read off a commit drawn on it**, so nothing here recomputes the
/// spacing it is checking.
///
/// [`LANE_STEP`]: super::gitgraph::LANE_STEP
#[test]
fn no_git_graph_label_reaches_into_a_neighbouring_lane() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph::{self, Direction};
    let mut checked = 0usize;
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let model = gitgraph::parse(src).expect("parses");
        let d = laid_out(src);
        let lr = matches!(model.direction, Direction::LeftToRight);
        let across = |p: &Point| if lr { p.y } else { p.x };
        let span = |n: &super::PlacedNode| {
            let (l, t, r, b) = n.bounds();
            if lr {
                (t, b)
            } else {
                (l, r)
            }
        };
        let lanes: Vec<&str> = model.lanes().iter().map(|b| b.name.as_str()).collect();
        // Where each lane runs, and what it is called — from the drawing, not from the constants.
        let at_lane: Vec<Option<f64>> = lanes
            .iter()
            .map(|b| {
                model
                    .commits
                    .iter()
                    .find(|c| c.branch == **b)
                    .and_then(|c| d.node(&c.id))
                    .map(|n| across(&n.center))
            })
            .collect();
        // Which lane a label belongs to: a caption or a ticket belongs to its commit's branch, a
        // branch name to the branch it names. Looked up rather than guessed from where it is
        // drawn, so a label drawn on the wrong lane is caught instead of being explained away.
        let owner = |id: &str| -> Option<usize> {
            if let Some(branch) = id.strip_prefix("branch#") {
                return lanes.iter().position(|b| *b == branch);
            }
            let commit = id.split('#').next()?;
            let branch = &model.commits.iter().find(|c| c.id == commit)?.branch;
            lanes.iter().position(|b| *b == branch)
        };
        for label in git_graph_labels(&d) {
            let Some(mine) = owner(&label.id) else {
                continue;
            };
            let Some(home) = at_lane[mine] else { continue };
            let (lo, hi) = span(label);
            for (k, line) in at_lane.iter().enumerate() {
                let Some(line) = line else { continue };
                if k == mine {
                    continue;
                }
                checked += 1;
                // The boundary between two columns is halfway between the two lines.
                let midline = (home + *line) / 2.0;
                assert!(
                    midline <= lo + 0.01 || midline >= hi - 0.01,
                    "{name}: the label {:?} belongs to lane {:?} and reaches into lane {:?}'s \
                     column",
                    label.label.lines.join(" "),
                    lanes[mine],
                    lanes[k]
                );
            }
        }
    }
    assert!(checked > 40, "only {checked} label/lane pairs were checked");
}

/// **No line in a git graph crosses a label.**
///
/// Two separate things put a line through a word, and this states the geometry of both.
///
/// * A **lane's own line** ran through its commits' captions and tickets in `TB:`/`BT:`, because
///   the labels were offset up and down the page and up and down the page is the *time* axis
///   there. They are offset across the lanes now, which is the one direction the lane line is not
///   in.
/// * A **line that changes lanes** turned at its child's own position, which is exactly where the
///   child's caption is: the merge into `MERGE` was drawn straight down the middle of the word.
///   It crosses in the strip between two commits instead — a strip the step leaves empty, because
///   every consecutive pair is pushed apart until their labels clear each other.
///
/// Every segment against every label, exactly (see [`segment_meets_box`]) — not the pairs that
/// look likely.
#[test]
fn a_git_graphs_lines_never_cross_a_label() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph;
    let mut segments = 0usize;
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let d = laid_out(src);
        let labels = git_graph_labels(&d);
        for e in &d.edges {
            let points = e.drawn_points();
            for w in points.windows(2) {
                segments += 1;
                for label in &labels {
                    assert!(
                        !segment_meets_box(&w[0], &w[1], label.bounds()),
                        "{name}: the line {:?} → {:?} is drawn through the label {:?}",
                        e.from,
                        e.to,
                        label.label.lines.join(" ")
                    );
                }
            }
        }
    }
    assert!(segments > 30, "only {segments} segments were checked");
}

/// **A git graph's lines are drawn under its labels; a chart's data path is drawn over its
/// marks.**
///
/// The two halves of one rule, stated together because the rule is the difference between them.
/// [`PlacedEdge::overlay`] says which of the two a line is, and it is asked of the line rather
/// than read off its `series`: a git graph's lane line carries a series — a lane *is* a colour —
/// so inferring depth from that put every lane line in the group emitted after the nodes, and the
/// lane was drawn over the captions and the tickets with the words struck through.
///
/// Read off the **emitted document**, because draw order is a property of the document and of
/// nothing else. The corpus golden sees this too, and that is exactly why it is written out here:
/// §6-A records twice over that a behaviour only a golden guards is lost the next time the golden
/// is regenerated.
///
/// [`PlacedEdge::overlay`]: super::PlacedEdge::overlay
#[test]
fn a_git_graphs_lines_go_under_its_labels_and_a_charts_data_path_over_its_marks() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph;
    let group = |doc: &str, class: &str| doc.find(&format!("<g class=\"{class}\">"));

    // (a) a git graph. Every one of its lines is in the group that comes before the nodes, and
    // every one of its labels is a node — so no line is drawn over any word in it.
    let mut graphs = 0usize;
    for (name, src) in cases_of(gitgraph::is_git_graph) {
        let doc = rendered(src);
        graphs += 1;
        assert!(
            group(&doc, "series").is_none(),
            "{name}: a git graph line is in the group drawn after the nodes"
        );
        let edges = group(&doc, "edges").unwrap_or_else(|| panic!("{name}: no edge group"));
        let nodes = group(&doc, "nodes").unwrap_or_else(|| panic!("{name}: no node group"));
        assert!(
            edges < nodes,
            "{name}: the lines are emitted after the nodes"
        );
        if let Some(text) = doc.find("<text") {
            assert!(
                text > nodes,
                "{name}: a word is emitted before the nodes, where a line could still cover it"
            );
        }
    }
    assert!(graphs >= 8, "only {graphs} git graphs were checked");

    // (b) a chart. Its line plot is in the group after the nodes, over the bars — which is the
    // half of the rule that must survive the fix to the other half.
    for (name, src) in [
        (
            "xychart",
            "xychart-beta\n  x-axis [a, b, c]\n  bar [3, 7, 2]\n  line [3, 7, 2]\n",
        ),
        (
            "radar",
            "radar-beta\n  axis a, b, c\n  curve x{1, 2, 3}\n  max 3\n",
        ),
    ] {
        let doc = rendered(src);
        let nodes = group(&doc, "nodes").unwrap_or_else(|| panic!("{name}: no node group"));
        let series =
            group(&doc, "series").unwrap_or_else(|| panic!("{name}: the data path is not drawn"));
        assert!(
            series > nodes,
            "{name}: the data path is drawn under the marks, where a tall bar hides it"
        );
    }
}

/// **A box is drawn as the shape its own source named.**
///
/// Seven of the mutations in this file's campaign were killed by [`kinds_corpus_golden`] and by
/// nothing else, and every one of them was of this shape: a datum drawn with the *other* datum's
/// mark. That is the failure §6-A keeps recording — a behaviour only a golden guards is lost
/// silently the next time the golden is regenerated — so the mapping is written out here, where
/// changing it means changing a test that says what it is for.
///
/// [`kinds_corpus_golden`]: kinds_corpus_golden
#[test]
fn every_box_is_drawn_as_the_shape_its_own_source_named() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::flowchart::Shape;
    use crate::preview::mermaid::mindmap::NodeShape;
    use crate::preview::mermaid::{c4, gitgraph, mindmap};
    let mut checked = 0usize;
    for (name, src) in CASES {
        if mindmap::is_mindmap(src) {
            let model = mindmap::parse(src).expect("parses");
            let d = laid_out(src);
            for (i, node) in model.nodes.iter().enumerate() {
                let want = match node.shape {
                    NodeShape::NoBorder => Glyph::Underline,
                    NodeShape::RoundedRect => Glyph::Flow(Shape::RoundedRect),
                    NodeShape::Rect => Glyph::Flow(Shape::Rect),
                    NodeShape::Circle => Glyph::Flow(Shape::Circle),
                    NodeShape::Cloud => Glyph::Cloud,
                    NodeShape::Bang => Glyph::Bang,
                    NodeShape::Hexagon => Glyph::Flow(Shape::Hexagon),
                };
                let placed = d.node(&format!("n{i}")).expect("drawn");
                checked += 1;
                assert_eq!(
                    placed.shape, want,
                    "{name}: {:?} was written as a {:?} and is drawn as a {:?}",
                    node.label, node.shape, placed.shape
                );
            }
        }
        if c4::is_c4(src) {
            let model = c4::parse(src).expect("parses");
            let d = laid_out(src);
            for e in &model.elements {
                let want = match e.shape {
                    c4::ShapeKind::Person => Glyph::Actor,
                    c4::ShapeKind::Box => Glyph::Flow(Shape::Rect),
                    c4::ShapeKind::Database => Glyph::Flow(Shape::Cylinder),
                    c4::ShapeKind::Queue => Glyph::Flow(Shape::Stadium),
                };
                let Some(placed) = d.node(&e.alias) else {
                    continue;
                };
                checked += 1;
                assert_eq!(
                    placed.shape, want,
                    "{name}: {:?} was declared a {:?} and is drawn as a {:?}",
                    e.alias, e.shape, placed.shape
                );
            }
        }
        if gitgraph::is_git_graph(src) {
            use crate::preview::mermaid::gitgraph::CommitKind;
            let model = gitgraph::parse(src).expect("parses");
            let d = laid_out(src);
            for commit in &model.commits {
                let want = match commit.custom_type.unwrap_or(commit.kind) {
                    // A highlight has to be told from an ordinary dot at a terminal's scale, and
                    // a square is what does that; a revert is the crossed dot.
                    CommitKind::Reverse => Glyph::Reverted,
                    CommitKind::Highlight => Glyph::Flow(Shape::Rect),
                    CommitKind::Merge => Glyph::Flow(Shape::DoubleCircle),
                    CommitKind::CherryPick | CommitKind::Normal => Glyph::Flow(Shape::Circle),
                };
                let placed = d.node(&commit.id).expect("drawn");
                checked += 1;
                assert_eq!(
                    placed.shape, want,
                    "{name}: commit {:?} is a {:?} and is drawn as a {:?}",
                    commit.id, commit.kind, placed.shape
                );
            }
        }
    }
    assert!(checked > 0, "no box was checked");
}

/// **A gantt bar is painted by its own tags, and the four states take four colours.**
///
/// Which colour goes with which state is konoma's decision (§0-1) and it was recorded *only* in
/// the corpus golden: a mutation that rotated the four survived every named test. The palette is
/// categorical, so the numbers below are the decision, not an accident of it.
#[test]
fn a_gantt_bars_colour_is_its_own_state() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gantt;
    let mut seen: HashSet<usize> = HashSet::new();
    for (name, src) in cases_of(gantt::is_gantt) {
        let model = gantt::parse(src).expect("parses");
        let d = laid_out(src);
        for (i, task) in model.tasks.iter().enumerate() {
            let want = match (task.tags.crit, task.tags.done, task.tags.active) {
                (true, _, _) => 3,
                (_, true, _) => 1,
                (_, _, true) => 2,
                _ => 0,
            };
            let Some(bar) = d.node(&format!("task#{i}")) else {
                continue;
            };
            seen.insert(want);
            assert_eq!(
                bar.series,
                Some(want),
                "{name}: task {:?} is {}{}{}and is painted in another state's colour",
                task.name,
                if task.tags.crit { "critical " } else { "" },
                if task.tags.done { "done " } else { "" },
                if task.tags.active { "active " } else { "" },
            );
        }
    }
    assert!(
        seen.len() >= 4,
        "only {} of the four task states are in the corpus, so this states less than it looks",
        seen.len()
    );
}

/// **`contains` is the one relationship drawn solid.**
///
/// It is the only *structural* verb — it says the destination is part of the source — and every
/// other one is drawn dashed, upstream and here. Also golden-only until now.
#[test]
fn only_a_contains_relationship_is_drawn_with_an_unbroken_line() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::flowchart::Stroke;
    use crate::preview::mermaid::requirement::{self, Relation};
    let mut solid = 0usize;
    let mut dashed = 0usize;
    for (name, src) in cases_of(requirement::is_requirement_diagram) {
        let model = requirement::parse(src).expect("parses");
        let d = laid_out(src);
        if model.links.len() != d.edges.len() {
            continue;
        }
        for (edge, link) in d.edges.iter().zip(model.links.iter()) {
            let want = if link.relation == Relation::Contains {
                Stroke::Normal
            } else {
                Stroke::Dotted
            };
            if want == Stroke::Normal {
                solid += 1;
            } else {
                dashed += 1;
            }
            assert_eq!(
                edge.stroke,
                want,
                "{name}: `{}` is drawn {}",
                link.relation.word(),
                if edge.stroke == Stroke::Normal {
                    "solid"
                } else {
                    "dashed"
                }
            );
        }
    }
    assert!(
        solid > 0 && dashed > 0,
        "the corpus has {solid} `contains` links and {dashed} others, so one side of this is \
         asserting nothing"
    );
}

/// **A panel row holds one drawn line.**
///
/// `svg::emit_line_of_text` writes a panel cell as a *single* `<text>`, and `Panel::cell_bounds`
/// measures it as one line — so a cell whose label carries two is drawn as its lines joined with
/// a space, which is wider than the box the panel sized for it and wider than the rectangle
/// `check_panels_stay_inside_their_box` checks. The words run out of the box and every assertion
/// passes. A kanban card written with a two-line markdown string did exactly that.
///
/// Stated as the contract rather than as the symptom: if a cell never holds two lines, the
/// overflow cannot happen at all.
#[test]
fn no_panel_cell_holds_more_than_one_line() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut checked = 0usize;
    for (name, src) in CASES {
        let d = laid_out(src);
        for node in &d.nodes {
            let Some(panel) = &node.panel else { continue };
            for row in &panel.rows {
                for cell in &row.cells {
                    checked += 1;
                    assert!(
                        cell.label.lines.len() <= 1,
                        "{name}: a cell of {:?} holds {:?}, and a panel row is drawn as one line",
                        node.id,
                        cell.label.lines
                    );
                }
            }
        }
    }
    assert!(checked > 0, "no panel cell was checked");
}

/// **The arrowhead is at the end the source pointed to.**
///
/// Found by a mutation that moved it to the other end and was caught by *nothing* — the corpus
/// golden included. A tip is drawn as a `<path d="…">`, and `mask_numbers` replaces a `d`
/// wholesale, so which end of a line an arrowhead sits on is invisible to the golden by
/// construction. That is §6-A item 15 again: the mark is there, on the wrong datum.
///
/// The links are paired with the drawn lines **by position**, not by looking one up by its two
/// endpoints: `Rel_D(a, c, …)` and `BiRel(a, c, …)` in one source are two lines between the same
/// pair, and a lookup by endpoints answers with the first of them for both — which is the
/// mis-pairing this whole file is written to avoid.
#[test]
fn an_arrowhead_is_drawn_at_the_end_the_source_points_to() {
    if !text_metrics::fonts_available() {
        return;
    }
    use super::edges::Tip;
    use crate::preview::mermaid::{c4, requirement};
    let mut checked = 0usize;
    for (name, src) in CASES {
        // (what the two ends should carry, how to say which line it was).
        let wanted: Vec<((Tip, Tip), String)> = if requirement::is_requirement_diagram(src) {
            let model = requirement::parse(src).expect("parses");
            model
                .links
                .iter()
                .map(|l| {
                    (
                        (Tip::None, Tip::Arrow),
                        format!("{:?} - {} -> {:?}", l.src, l.relation.word(), l.dst),
                    )
                })
                .collect()
        } else if c4::is_c4(src) {
            let model = c4::parse(src).expect("parses");
            model
                .rels
                .iter()
                .map(|r| {
                    // `BiRel` is the only one that carries two heads, and it carries them
                    // because the source said the two talk both ways.
                    let start = if r.bidirectional {
                        Tip::Arrow
                    } else {
                        Tip::None
                    };
                    ((start, Tip::Arrow), format!("{:?} -> {:?}", r.from, r.to))
                })
                .collect()
        } else {
            continue;
        };
        let d = laid_out(src);
        if wanted.len() != d.edges.len() {
            // A line whose end never became a box is not drawn; nothing here is about that.
            continue;
        }
        for (edge, (tips, what)) in d.edges.iter().zip(wanted.iter()) {
            checked += 1;
            assert_eq!(
                (edge.tip_start, edge.tip_end),
                *tips,
                "{name}: {what} is drawn with its heads at the wrong ends"
            );
        }
    }
    assert!(checked > 0, "no line was checked");
}

/// **`direction` decides which way the ranks run.**
///
/// Also found by a mutation that was caught by nothing: forcing every diagram to `TB` left the
/// masked golden byte-identical, because a rotation moves *every* coordinate and the golden pins
/// none of them. So a documented keyword was being read and thrown away, and the corpus's four
/// `direction` cases were asserting that it parsed and nothing more.
///
/// Stated only over the sources whose own edges form a **DAG**: `lay_out_spec` reverses an edge
/// to break a cycle, so "every line runs forward along the axis" is not true of a diagram that
/// contains one — and that is a fact about the source, worked out here from the source.
#[test]
fn the_direction_a_source_names_is_the_axis_its_lines_run_along() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::flowchart::Direction;
    use crate::preview::mermaid::{c4, requirement};
    let mut seen: HashSet<Direction> = HashSet::new();
    for (name, src) in CASES {
        let (direction, pairs): (Direction, Vec<(String, String)>) =
            if requirement::is_requirement_diagram(src) {
                let m = requirement::parse(src).expect("parses");
                (
                    m.direction,
                    m.links
                        .iter()
                        .map(|l| (l.src.clone(), l.dst.clone()))
                        .collect(),
                )
            } else if c4::is_c4(src) {
                let m = c4::parse(src).expect("parses");
                (
                    m.direction,
                    m.rels
                        .iter()
                        .map(|r| (r.from.clone(), r.to.clone()))
                        .collect(),
                )
            } else {
                continue;
            };
        if pairs.is_empty() || !is_a_dag(&pairs) {
            continue;
        }
        let d = laid_out(src);
        let mut any = false;
        for (from, to) in &pairs {
            let (Some(a), Some(b)) = (d.node(from), d.node(to)) else {
                continue;
            };
            let ok = match direction {
                Direction::TopToBottom => b.center.y > a.center.y,
                Direction::BottomToTop => b.center.y < a.center.y,
                Direction::LeftToRight => b.center.x > a.center.x,
                Direction::RightToLeft => b.center.x < a.center.x,
            };
            assert!(
                ok,
                "{name}: the source says `direction {}` and {from:?} -> {to:?} does not run that \
                 way",
                direction.as_str()
            );
            any = true;
        }
        if any {
            seen.insert(direction);
        }
    }
    assert!(
        seen.len() >= 3,
        "only {} of the four directions were exercised, so this states less than it looks",
        seen.len()
    );
}

/// Whether a list of (from, to) pairs has no cycle in it.
fn is_a_dag(pairs: &[(String, String)]) -> bool {
    let mut left: Vec<(String, String)> = pairs.to_vec();
    // Kahn's algorithm, written on the pairs themselves: repeatedly drop every edge whose source
    // nothing points at. What cannot be dropped is a cycle.
    for _ in 0..=pairs.len() {
        let before = left.len();
        let targets: HashSet<String> = left.iter().map(|(_, t)| t.clone()).collect();
        left.retain(|(f, _)| targets.contains(f));
        if left.len() == before {
            break;
        }
    }
    left.is_empty()
}

/// **`columns auto` is one row.**
///
/// `-1` means "one row, as wide as it needs to be" and it is also the default, so most block
/// diagrams in the wild are this shape. A mutation that made it one *column* survived everything:
/// reading order — top to bottom, then left to right — is the same for a row and a column, so the
/// text tests could not see it, and the golden masks every coordinate.
#[test]
fn a_block_grid_written_columns_auto_is_one_row() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block::{self, Item, AUTO};
    let mut checked = 0usize;
    for (name, src) in cases_of(block::is_block_diagram) {
        let model = block::parse(src).expect("parses");
        if model.columns != AUTO {
            continue;
        }
        let d = laid_out(src);
        let drawn: Vec<&super::PlacedNode> = model
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Node(n) => d.node(&n.id),
                _ => None,
            })
            .collect();
        if drawn.len() < 2 {
            continue;
        }
        checked += 1;
        let first = drawn[0].center.y;
        for n in &drawn {
            assert!(
                (n.center.y - first).abs() < 0.5,
                "{name}: {:?} is not on the same row as the block written first, and `columns \
                 auto` is one row",
                n.id
            );
        }
    }
    assert!(checked > 0, "no `columns auto` case had two blocks in it");
}

/// **A block arrow points the way it was written.**
///
/// `<["left"]>(left)` is an arrow that points left, and that is the whole content of the mark.
/// A mutation that swapped left for right survived: the outline is a `<path d="…">`, which the
/// golden masks, and `new_glyph_emit_golden` pins the outline for a mark *assembled by hand* —
/// so the step from the source's `(left)` to the mark was checked by nothing.
#[test]
fn a_block_arrow_points_the_way_the_source_wrote_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block::{self, Dir, Item};
    let mut checked = 0usize;
    for (name, src) in cases_of(block::is_block_diagram) {
        let model = block::parse(src).expect("parses");
        let d = laid_out(src);
        for item in &model.items {
            let Item::Node(n) = item else { continue };
            let Some(dirs) = n.arrow.as_ref() else {
                continue;
            };
            let Some(placed) = d.node(&n.id) else {
                continue;
            };
            checked += 1;
            assert_eq!(
                placed.mark,
                Some(Mark::BlockArrow {
                    left: dirs.iter().any(|x| matches!(x, Dir::Left | Dir::X)),
                    right: dirs.iter().any(|x| matches!(x, Dir::Right | Dir::X)),
                    up: dirs.iter().any(|x| matches!(x, Dir::Up | Dir::Y)),
                    down: dirs.iter().any(|x| matches!(x, Dir::Down | Dir::Y)),
                }),
                "{name}: the arrow {:?} was written {dirs:?} and points somewhere else",
                n.id
            );
        }
    }
    assert!(checked > 0, "no block arrow was checked");
}

/// **A composite's frame is drawn round its contents, not beside them.**
///
/// A cell is levelled to the grid's column width, so a frame may be *wider* than the blocks in
/// it — but it is drawn round them, which means the room left over is the same on both sides. A
/// mutation that shifted every cell one column right kept the reading order, kept the
/// containment, and left a blank column inside every nested frame; nothing noticed, because a
/// uniform shift is exactly what a relative check cannot see (§6-A item 12).
#[test]
fn a_composite_frame_is_centred_on_what_is_inside_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block;
    let mut checked = 0usize;
    for (name, src) in cases_of(block::is_block_diagram) {
        let d = laid_out(src);
        for frame in &d.clusters {
            let (fl, _, fr, _) = frame.bounds();
            let mut left = f64::INFINITY;
            let mut right = f64::NEG_INFINITY;
            for n in &d.nodes {
                let (l, t, r, b) = n.bounds();
                let (_, ft, _, fb) = frame.bounds();
                if l >= fl - 0.5 && r <= fr + 0.5 && t >= ft - 0.5 && b <= fb + 0.5 {
                    left = left.min(l);
                    right = right.max(r);
                }
            }
            if !left.is_finite() {
                continue;
            }
            checked += 1;
            assert!(
                ((left - fl) - (fr - right)).abs() < 1.0,
                "{name}: frame {} leaves {} on its left and {} on its right",
                frame.id,
                svg::num(left - fl),
                svg::num(fr - right)
            );
        }
    }
    assert!(checked > 0, "no block frame was checked");
}

/// **`align row` puts its members on one row, and `align column` in one column.**
///
/// Two documented statements whose whole content is a coordinate. A mutation that swapped the two
/// survived: both keep every box on the grid, both keep every frame nesting, and the golden masks
/// the coordinates that are the difference between them.
#[test]
fn an_architecture_alignment_lines_its_members_up_the_way_it_says() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture;
    let mut checked = 0usize;
    for (name, src) in cases_of(architecture::is_architecture) {
        let model = architecture::parse(src).expect("parses");
        let d = laid_out(src);
        for a in &model.alignments {
            let placed: Vec<&super::PlacedNode> =
                a.members.iter().filter_map(|m| d.node(m)).collect();
            if placed.len() < 2 {
                continue;
            }
            checked += 1;
            let first = placed[0];
            for n in &placed[1..] {
                if a.row {
                    assert!(
                        (n.center.y - first.center.y).abs() < 0.5,
                        "{name}: `align row` names {:?} and {:?} and they are on two rows",
                        first.id,
                        n.id
                    );
                    assert!(
                        (n.center.x - first.center.x).abs() > 0.5,
                        "{name}: `align row` names {:?} and {:?} and drew them on one another",
                        first.id,
                        n.id
                    );
                } else {
                    assert!(
                        (n.center.x - first.center.x).abs() < 0.5,
                        "{name}: `align column` names {:?} and {:?} and they are in two columns",
                        first.id,
                        n.id
                    );
                    assert!(
                        (n.center.y - first.center.y).abs() > 0.5,
                        "{name}: `align column` names {:?} and {:?} and drew them on one another",
                        first.id,
                        n.id
                    );
                }
            }
        }
    }
    assert!(checked > 0, "no alignment was checked");
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
