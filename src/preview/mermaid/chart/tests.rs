//! What stage 5's seven chart grammars are checked with.
//!
//! # The corpus is built from the spec's case distinctions, not from bugs
//!
//! Memory `corpus-from-spec-not-from-bugs`: a corpus grown from past bugs only ever prevents the
//! bugs that already happened. Every case below comes from a branch in the grammar or the DB that
//! upstream actually has — `NUMBER_PIE` carrying a sign where `NUMBER` does not, `hasSetXAxis`
//! making a plot's meaning depend on where the `x-axis` line sits, `getNextFittingBlock` splitting
//! a field at a row boundary — plus the examples mermaid's own documentation shows.
//!
//! # "描画不要 ≠ パース不要"
//!
//! §2-3. A statement the grammar has a rule for and the parser has none for does not disappear: it
//! falls through to whatever the parser's last resort is and becomes a phantom element. In these
//! seven that would be a `classDef` drawn as a data point and an `accDescr` drawn as a treemap
//! node. [`deliberate_drops_are_parsed_and_produce_nothing`] is the test that pins every one of
//! them, and it asserts **both** halves: the statement parses, and it draws nothing.

use super::*;

/// The sources every structural test runs over, and the ones the render gallery writes out.
///
/// mermaid's own documented examples where there is one, because a chart that cannot draw the
/// example on its own documentation page is not finished.
pub const CASES: &[(&str, &str)] = &[
    (
        "pie",
        "pie title Pets adopted by volunteers\n    \"Dogs\" : 386\n    \"Cats\" : 85\n    \
         \"Rats\" : 15\n",
    ),
    (
        "pie-showdata",
        "pie showData\n    title Key elements in Product X\n    \"Calcium\" : 42.96\n    \
         \"Potassium\" : 50.05\n    \"Magnesium\" : 10.01\n    \"Iron\" :  5\n",
    ),
    (
        "xychart",
        "xychart-beta\n    title \"Sales Revenue\"\n    x-axis [jan, feb, mar, apr, may, jun]\n    \
         y-axis \"Revenue (in $)\" 4000 --> 11000\n    bar [5000, 6000, 7500, 8200, 9500, 10500]\n    \
         line [5000, 6000, 7500, 8200, 9500, 10500]\n",
    ),
    (
        "xychart-two-bars",
        "xychart-beta\n  title \"Quarterly\"\n  x-axis [Q1, Q2, Q3, Q4]\n  y-axis \"Units\"\n  \
         bar \"North\" [30, 45, 20, 60]\n  bar \"South\" [22, 12, 48, 35]\n  \
         line \"Target\" [28, 30, 34, 50]\n",
    ),
    (
        "xychart-negative",
        "xychart-beta\n  x-axis [a, b, c, d]\n  y-axis -40 --> 60\n  bar [30, -20, 55, -35]\n",
    ),
    (
        "quadrant",
        "quadrantChart\n    title Reach and engagement of campaigns\n    \
         x-axis Low Reach --> High Reach\n    y-axis Low Engagement --> High Engagement\n    \
         quadrant-1 We should expand\n    quadrant-2 Need to promote\n    quadrant-3 Re-evaluate\n    \
         quadrant-4 May be improved\n    Campaign A: [0.3, 0.6]\n    Campaign B: [0.45, 0.23]\n    \
         Campaign C: [0.57, 0.69]\n    Campaign D: [0.78, 0.34]\n    Campaign E: [0.4, 0.34]\n    \
         Campaign F: [0.35, 0.78]\n",
    ),
    (
        "radar",
        "radar-beta\n  title Grades\n  axis m[\"Math\"], s[\"Science\"], e[\"English\"]\n  \
         axis h[\"History\"], g[\"Geography\"], a[\"Art\"]\n  \
         curve a[\"Alice\"]{85, 90, 80, 70, 75, 90}\n  curve b[\"Bob\"]{70, 75, 85, 80, 90, 85}\n\n  \
         max 100\n  min 0\n",
    ),
    (
        "radar-polygon",
        "radar-beta\n  axis a, b, c, d, e\n  curve x{10, 20, 30, 40, 50}\n  graticule polygon\n  \
         ticks 4\n",
    ),
    (
        // **A scale whose minimum is not zero.** Without this case, subtracting `min` in
        // `radius_of` is a no-op on the whole corpus, and a mutation that dropped it survived —
        // which is what put this row here (memory `corpus-from-spec-not-from-bugs`: the case
        // distinction is in the spec, `options.min`, so the corpus owes it a row).
        "radar-offset-scale",
        "radar-beta\n  axis a, b, c, d\n  curve x[\"only\"]{20, 45, 70, 100}\n  min 20\n  \
         max 100\n  ticks 4\n",
    ),
    (
        "treemap",
        "treemap-beta\n\"Section 1\"\n    \"Leaf 1.1\": 12\n    \"Section 1.2\"\n      \
         \"Leaf 1.2.1\": 12\n\"Section 2\"\n    \"Leaf 2.1\": 20\n    \"Leaf 2.2\": 25\n",
    ),
    (
        "packet",
        "packet-beta\n0-15: \"Source Port\"\n16-31: \"Destination Port\"\n32-63: \"Sequence Number\"\n\
         64-95: \"Acknowledgment Number\"\n96-99: \"Data Offset\"\n100-105: \"Reserved\"\n\
         106: \"URG\"\n107: \"ACK\"\n108: \"PSH\"\n109: \"RST\"\n110: \"SYN\"\n111: \"FIN\"\n\
         112-127: \"Window\"\n128-143: \"Checksum\"\n144-159: \"Urgent Pointer\"\n",
    ),
    (
        "sankey",
        "sankey-beta\n\nAgricultural 'waste',Bio-conversion,124.729\nBio-conversion,Liquid,0.597\n\
         Bio-conversion,Losses,26.862\nBio-conversion,Solid,280.322\nBio-conversion,Gas,81.144\n\
         Biofuel imports,Liquid,35\nBiomass imports,Solid,35\n",
    ),
    (
        "cjk",
        "pie title 円グラフ\n    \"りんご\" : 30\n    \"みかん\" : 20\n    \"ぶどう\" : 50\n",
    ),
    // ------------------------------------------------------------------------------------------
    // The case distinctions each grammar makes, one row each — read at `mermaid@11.17.2` and
    // enumerated from the grammar and the DB, **not** from a list of bugs (memory
    // `corpus-from-spec-not-from-bugs`).
    //
    // Appended rather than interleaved so that the blocks already in `mermaid_chart.snap` keep
    // their byte offsets: a corpus that grows must not move what is already pinned.
    //
    // The axes every kind gets a row for, because a chart that is wrong along one of them still
    // reads as data rather than as a broken picture:
    //
    // * **numbers** — a single datum, every value equal, a zero among non-zeroes, one value
    //   dwarfing the rest, magnitudes at both ends, values that do not divide evenly, a repeated
    //   label;
    // * **scale and axis** — a `min` that is not zero, a range narrower than one tick, data
    //   outside a declared range, and an axis declared *after* the data (`hasSetXAxis` makes that
    //   a different chart);
    // * **structure** — deep nesting, a field spanning the 32-bit row boundary, a fan-in and a
    //   fan-out, a curve with fewer entries than there are axes;
    // * **text** — CJK, emoji, very long, empty.
    (
        "pie-single",
        "pie\n    \"Only one\" : 1\n",
    ),
    (
        "pie-equal",
        "pie title Three equal ways\n    \"A\" : 10\n    \"B\" : 10\n    \"C\" : 10\n",
    ),
    (
        "pie-dominant",
        "pie\n    \"Bulk\" : 9990\n    \"Trace\" : 8\n    \"Residue\" : 2\n",
    ),
    (
        "pie-zero-slice",
        "pie showData\n    \"Present\" : 5\n    \"Absent\" : 0\n    \"Also here\" : 5\n",
    ),
    (
        "pie-many",
        "pie showData\n    \"Series 01\" : 1\n    \"Series 02\" : 2\n    \"Series 03\" : 3\n    \"Ser\
         ies 04\" : 4\n    \"Series 05\" : 5\n    \"Series 06\" : 6\n    \"Series 07\" : 7\n    \"Ser\
         ies 08\" : 8\n    \"Series 09\" : 9\n    \"Series 10\" : 10\n",
    ),
    (
        "pie-thirds",
        "pie\n    \"one\" : 1\n    \"two\" : 1\n    \"three\" : 1\n",
    ),
    (
        "pie-huge",
        "pie\n    \"Ocean\" : 999999999999\n    \"Puddle\" : 1\n",
    ),
    (
        "pie-small",
        "pie showData\n    \"a\" : 0.0001\n    \"b\" : 0.0002\n    \"c\" : 0.0003\n",
    ),
    (
        "pie-long-label",
        "pie\n    \"A slice whose name runs on for very much longer than the wedge it belongs to\" : \
         3\n    \"Short\" : 1\n",
    ),
    (
        "pie-repeated-label",
        "pie\n    \"Dup\" : 1\n    \"Dup\" : 99\n    \"Other\" : 1\n",
    ),
    (
        "pie-single-quotes",
        "pie title Apostrophes\n    'Agricultural waste' : 4\n    'Bio-conversion' : 6\n",
    ),
    (
        "pie-escaped-quote",
        "pie\n    \"a \\\"quoted\\\" word\" : 1\n    \"plain\" : 2\n",
    ),
    (
        "pie-emoji",
        "pie\n    \"🚀 launch\" : 5\n    \"日本語のラベル\" : 3\n    \"plain\" : 2\n",
    ),
    (
        "pie-blank-label",
        "pie showData\n    \"\" : 5\n    \"named\" : 5\n",
    ),
    (
        "xychart-linear-axis",
        "xychart-beta\n  title \"Linear x\"\n  x-axis \"Depth\" 0 --> 10\n  y-axis \"Reading\" 0 --> \
         100\n  line [10, 35, 60, 80, 95]\n",
    ),
    (
        "xychart-single-datum",
        "xychart-beta\n  bar [42]\n",
    ),
    (
        "xychart-point-labels",
        "xychart-beta\n  x-axis [mon, tue, wed]\n  line \"Load\" [5 \"five\", 6 \"six\", 7]\n",
    ),
    (
        "xychart-flat",
        "xychart-beta\n  x-axis [a, b, c, d]\n  bar [7, 7, 7, 7]\n",
    ),
    (
        "xychart-zeroes",
        "xychart-beta\n  x-axis [a, b, c]\n  bar [0, 0, 0]\n",
    ),
    (
        "xychart-truncated",
        "xychart-beta\n  x-axis [a, b, c]\n  bar [1, 2, 3, 4, 5]\n",
    ),
    (
        "xychart-fewer-than-categories",
        "xychart-beta\n  x-axis [a, b, c, d]\n  bar [3, 4]\n",
    ),
    (
        "xychart-plot-before-axis",
        "xychart-beta\n  bar [1, 2, 3, 4, 5]\n  x-axis [a, b, c]\n",
    ),
    (
        "xychart-two-plots-no-axis",
        "xychart-beta\n  bar \"Short\" [1, 2, 3]\n  bar \"Long\" [1, 2, 3, 4, 5]\n",
    ),
    (
        "xychart-all-negative",
        "xychart-beta\n  y-axis -100 --> -10\n  x-axis [a, b, c]\n  bar [-20, -50, -90]\n",
    ),
    (
        "xychart-narrow-range",
        "xychart-beta\n  x-axis [a, b, c]\n  y-axis 0 --> 0.001\n  bar [0.0002, 0.0007, 0.001]\n",
    ),
    (
        "xychart-huge",
        "xychart-beta\n  x-axis [a, b, c]\n  bar [1000000000, 2000000000, 1500000000]\n",
    ),
    (
        "xychart-out-of-range",
        "xychart-beta\n  x-axis [a, b, c]\n  y-axis 0 --> 10\n  bar [5, 25, 8]\n",
    ),
    (
        "xychart-cjk",
        "xychart-beta\n  title \"売上の推移\"\n  x-axis [一月, 二月, 三月, 四月]\n  y-axis \"億円\" 0 --> 10\n  bar \
         [3, 5, 8, 6]\n",
    ),
    (
        "xychart-long-categories",
        "xychart-beta\n  x-axis [\"a really quite long category name\", \"another long one\", \"third\
         \"]\n  bar [3, 5, 8]\n",
    ),
    (
        "xychart-many-categories",
        "xychart-beta\n  x-axis [c01, c02, c03, c04, c05, c06, c07, c08, c09, c10, c11, c12]\n  line \
         [8, 2, 9, 3, 10, 4, 11, 5, 12, 6, 13, 7]\n",
    ),
    (
        "xychart-horizontal",
        "xychart-beta horizontal\n  x-axis [a, b, c]\n  bar [4, 8, 2]\n",
    ),
    (
        "xychart-line-only",
        "xychart-beta\n  x-axis [a, b, c, d]\n  line \"Trend\" [2, 9, 4, 7]\n",
    ),
    (
        "xychart-semicolons",
        "xychart-beta\n  x-axis [a, b, c]; y-axis 0 --> 10; bar [2, 5, 9]\n",
    ),
    (
        "quadrant-points-only",
        "quadrantChart\n  Alpha: [0.2, 0.8]\n  Beta: [0.7, 0.3]\n",
    ),
    (
        "quadrant-names-only",
        "quadrantChart\n  quadrant-1 Expand\n  quadrant-2 Promote\n  quadrant-3 Re-evaluate\n  quadra\
         nt-4 Improve\n",
    ),
    (
        "quadrant-x-axis-only",
        "quadrantChart\n  x-axis Cheap --> Dear\n  P: [0.5, 0.25]\n",
    ),
    (
        "quadrant-dangling-arrow",
        "quadrantChart\n  x-axis Low Reach -->\n  y-axis Low -->\n  P: [0.25, 0.75]\n",
    ),
    (
        "quadrant-corners",
        "quadrantChart\n  x-axis Left --> Right\n  y-axis Bottom --> Top\n  SW: [0, 0]\n  NW: [0, 1]\
         \n  NE: [1, 1]\n  SE: [1, 0]\n",
    ),
    (
        // Exactly on both dividers, which is the boundary the placement arithmetic turns on. The
        // second point is far away on purpose: two points a millionth apart also collide their
        // labels, and label collision is a defect already on the record rather than this case's
        // subject (`docs/STATUS.md`).
        // Exactly on both dividers, which is the boundary the placement arithmetic turns
        // on. The second point is far away on purpose: two points a millionth apart also
        // collide their labels, and label collision is a defect already on the record
        // rather than this case's subject (`docs/STATUS.md`).
        "quadrant-centre",
        "quadrantChart\n  Dead centre: [0.5, 0.5]\n  Far corner: [1, 1]\n",
    ),
    (
        "quadrant-classes",
        "quadrantChart\n  classDef hot radius: 8, color: #ff0000\n  Hot one:::hot: [0.2, 0.3]\n  Cool\
         : [0.8, 0.9]\n",
    ),
    (
        "quadrant-styles",
        "quadrantChart\n  Styled: [0.2, 0.3] radius: 10, color: #ff0000\n  Plain: [0.7, 0.6]\n",
    ),
    (
        // Three points on one spot. Legal, and the tiles/dots are right; the *labels* land on top
        // of one another, which `docs/STATUS.md` already records. Kept out of the rendered corpus
        // for that reason and pinned by an ignored test instead
        // (`render::chart::tests::coincident_quadrant_points_should_not_stack_their_labels`).
        "quadrant-near-neighbours",
        "quadrantChart\n  First: [0.4, 0.4]\n  Second: [0.44, 0.28]\n  Third: [0.36, 0.52]\n",
    ),
    (
        "quadrant-cjk",
        "quadrantChart\n  title 四象限図\n  x-axis 低い --> 高い\n  y-axis 小さい --> 大きい\n  顧客A: [0.3, 0.7]\n  \
         顧客B: [0.8, 0.2]\n",
    ),
    (
        "quadrant-long-labels",
        "quadrantChart\n  x-axis A rather long axis label on the left --> And a long one on the right\
         \n  A campaign with a very long descriptive name indeed: [0.3, 0.6]\n",
    ),
    (
        "quadrant-colon-label",
        "quadrantChart\n  Phase 1: build: [0.3, 0.4]\n  Phase 2: ship: [0.6, 0.7]\n",
    ),
    (
        "quadrant-quoted",
        "quadrantChart\n  \"Quoted, with a comma\": [0.2, 0.3]\n  Plain: [0.6, 0.7]\n",
    ),
    (
        // Twelve points, all on distinct coordinates. The first draft generated them
        // arithmetically and put two of them on the same spot, which is a *label*
        // collision — a defect already on the record — rather than this case's subject.
        "quadrant-many",
        "quadrantChart\n  P01: [0.05, 0.12]\n  P02: [0.17, 0.88]\n  P03: [0.29, 0.35]\n  P04: [0.41, 0.66]\n  P05: [0.53, 0.09]\n  P06: [0.65, 0.94]\n  P07: [0.77, 0.41]\n  P08: [0.89, 0.73]\n  P09: [0.11, 0.55]\n  P10: [0.23, 0.21]\n  P11: [0.35, 0.82]\n  P12: [0.47, 0.47]\n",
    ),
    (
        "quadrant-semicolons",
        "quadrantChart\n  x-axis Low --> High; A: [0.2, 0.3]; B: [0.8, 0.9]\n",
    ),
    (
        "quadrant-emoji",
        "quadrantChart\n  🚀 launch: [0.8, 0.8]\n  🐌 crawl: [0.2, 0.2]\n",
    ),
    (
        "radar-named-entries",
        "radar-beta\n  axis a[\"Alpha\"], b[\"Beta\"], c[\"Gamma\"]\n  curve one[\"One\"]{c: 3, a: 1, \
         b: 2}\n  max 5\n",
    ),
    (
        "radar-multiline-curve",
        "radar-beta\n  axis a, b, c, d\n  curve x{\n    10,\n    20,\n    30,\n    40\n  }\n  max 50\
         \n",
    ),
    (
        "radar-no-legend",
        "radar-beta\n  axis a, b, c, d\n  curve x[\"Hidden\"]{1, 2, 3, 4}\n  showLegend false\n",
    ),
    (
        "radar-one-ring",
        "radar-beta\n  axis a, b, c\n  curve x{1, 2, 3}\n  ticks 1\n",
    ),
    (
        "radar-many-rings",
        "radar-beta\n  axis a, b, c, d\n  curve x{1, 2, 3, 4}\n  ticks 99\n",
    ),
    (
        "radar-clamped-high",
        "radar-beta\n  axis a, b, c, d\n  curve x{5, 50, 200, 20}\n  max 100\n  min 0\n",
    ),
    (
        "radar-clamped-low",
        "radar-beta\n  axis a, b, c, d\n  curve x{5, 30, 60, 90}\n  max 100\n  min 25\n",
    ),
    (
        "radar-flat",
        "radar-beta\n  axis a, b, c, d, e\n  curve x{7, 7, 7, 7, 7}\n  max 10\n",
    ),
    (
        "radar-spike",
        "radar-beta\n  axis a, b, c, d, e, f\n  curve x{1, 1, 1, 100, 1, 1}\n",
    ),
    (
        // A curve that is short and one that is full length, together: the short one is skipped
        // (upstream's rule — see `render::chart::radar::is_drawable`) and the chart still draws.
        // The two on their own are *refused*, which is why they are in
        // [`a_radar_curve_without_an_entry_for_every_axis_is_not_drawn`] and not here.
        "radar-mixed-lengths",
        "radar-beta\n  axis a, b, c, d\n  curve full[\"Full\"]{1, 2, 3, 4}\n  \
         curve short[\"Short\"]{1, 2}\n  max 5\n",
    ),
    (
        "radar-cjk",
        "radar-beta\n  title 能力値\n  axis 攻[\"攻撃\"], 守[\"防御\"], 速[\"素早さ\"], 技[\"技術\"]\n  curve 甲[\"甲選手\
         \"]{80, 60, 90, 70}\n  max 100\n",
    ),
    (
        "radar-many-axes",
        "radar-beta\n  axis a01, a02, a03, a04, a05, a06, a07, a08, a09, a10, a11, a12\n  curve x{6, \
         11, 5, 10, 4, 9, 3, 8, 2, 7, 1, 6}\n  max 12\n",
    ),
    (
        "radar-colon-header",
        "radar-beta:\n  axis a, b, c\n  curve x{1, 2, 3}\n",
    ),
    (
        "radar-three-curves",
        "radar-beta\n  axis a, b, c, d\n  curve p[\"P\"]{1, 4, 2, 3}\n  curve q[\"Q\"]{4, 1, 3, 2}\n  \
         curve r[\"R\"]{2, 2, 4, 1}\n  max 5\n",
    ),
    (
        "treemap-flat",
        "treemap-beta\n\"Alpha\": 30\n\"Beta\": 20\n\"Gamma\": 50\n",
    ),
    (
        "treemap-deep",
        "treemap-beta\n\"L1\"\n  \"L2\"\n    \"L3\"\n      \"leaf a\": 10\n      \"leaf b\": 20\n    \
         \"L3 sibling\": 30\n  \"L2 sibling\": 40\n\"Other\": 60\n",
    ),
    (
        "treemap-comma-separator",
        "treemap-beta\n\"Alpha\", 30\n\"Beta\", 20\n",
    ),
    (
        "treemap-classes",
        "treemap-beta\nclassDef leafy fill:#f9f\n\"a\": 1:::leafy\n\"b\": 2\n",
    ),
    (
        "treemap-tabs",
        "treemap-beta\n\"Section\"\n	\"leaf a\": 10\n	\"leaf b\": 20\n",
    ),
    (
        "treemap-dominant",
        "treemap-beta\n\"Bulk\": 9990\n\"Trace\": 8\n\"Residue\": 2\n",
    ),
    (
        "treemap-equal",
        "treemap-beta\n\"a\": 25\n\"b\": 25\n\"c\": 25\n\"d\": 25\n",
    ),
    (
        // **A zero-valued node first.** `squarify` lays only the nodes with a value, so the k-th
        // rectangle it produces belongs to the k-th *live* node — and the two indices agree for
        // every source whose leading values are positive. This one makes them disagree, which is
        // what a mis-indexed write has to be measured against.
        "treemap-leading-zero",
        "treemap-beta\n\"nothing\": 0\n\"half\": 50\n\"the rest\": 50\n",
    ),
    (
        "treemap-zero-leaf",
        "treemap-beta\n\"present\": 50\n\"absent\": 0\n\"also here\": 50\n",
    ),
    (
        "treemap-single",
        "treemap-beta\n\"only\": 1\n",
    ),
    (
        "treemap-cjk",
        "treemap-beta\ntitle 面積図\n\"果物\"\n  \"りんご\": 30\n  \"みかん\": 20\n\"野菜\"\n  \"にんじん\": 25\n",
    ),
    (
        "treemap-long-names",
        "treemap-beta\n\"A section whose name is far too long to fit inside the band above its childr\
         en\"\n  \"and a leaf whose name is also much too long\": 60\n  \"s\": 40\n",
    ),
    (
        "treemap-many",
        "treemap-beta\n\"n01\": 8\n\"n02\": 15\n\"n03\": 22\n\"n04\": 6\n\"n05\": 13\n\"n06\": 20\n\"\
         n07\": 4\n\"n08\": 11\n\"n09\": 18\n\"n10\": 2\n\"n11\": 9\n\"n12\": 16\n\"n13\": 23\n\"n14\
         \": 7\n\"n15\": 14\n\"n16\": 21\n\"n17\": 5\n\"n18\": 12\n\"n19\": 19\n\"n20\": 3\n",
    ),
    (
        "treemap-title",
        "treemap-beta\ntitle Where the budget went\n\"build\": 60\n\"run\": 40\n",
    ),
    (
        "treemap-fractional",
        "treemap-beta\n\"a\": 0.125\n\"b\": 0.25\n\"c\": 0.625\n",
    ),
    (
        "treemap-thousands",
        "treemap-beta\n\"a\": 1,234.5\n\"b\": 2,469\n",
    ),
    (
        "treemap-sibling-leaf",
        "treemap-beta\n\"a\": 1\n  \"b\": 2\n  \"c\": 3\n",
    ),
    (
        "packet-relative",
        "packet-beta\n+8: \"type\"\n+8: \"code\"\n+16: \"checksum\"\n",
    ),
    (
        "packet-crossing",
        "packet-beta\n0-15: \"head\"\n16-63: \"spans the row boundary\"\n",
    ),
    (
        "packet-one-row",
        "packet-beta\n0-7: \"a\"\n8-15: \"b\"\n16-31: \"c\"\n",
    ),
    (
        "packet-full-row",
        "packet-beta\n0-31: \"the whole word\"\n",
    ),
    (
        "packet-many-rows",
        "packet-beta\n0-31: \"row 0\"\n32-63: \"row 1\"\n64-95: \"row 2\"\n96-127: \"row 3\"\n128-159\
         : \"row 4\"\n160-191: \"row 5\"\n",
    ),
    (
        "packet-title",
        "packet-beta\ntitle A framed header\n0-15: \"length\"\n16-31: \"flags\"\n",
    ),
    (
        "packet-cjk",
        "packet-beta\n0-15: \"送信元ポート\"\n16-31: \"宛先ポート\"\n",
    ),
    (
        "packet-long-label",
        "packet-beta\n0-3: \"a label far too long for four bits\"\n4-31: \"roomy\"\n",
    ),
    (
        "packet-mixed",
        "packet-beta\n0-15: \"absolute\"\n+8: \"relative\"\n24-31: \"absolute again\"\n",
    ),
    (
        "packet-partial-row",
        "packet-beta\n0-31: \"full\"\n32-39: \"and a bit\"\n",
    ),
    (
        "packet-64",
        "packet-beta\n0-63: \"a sixty-four bit field\"\n",
    ),
    (
        "packet-empty-label",
        "packet-beta\n0-7: \"\"\n8-31: \"named\"\n",
    ),
    (
        "packet-single-bit-only",
        "packet-beta\n0: \"f\"\n",
    ),
    (
        "packet-all-single-bits",
        "packet-beta\n0: \"b0\"\n1: \"b1\"\n2: \"b2\"\n3: \"b3\"\n4: \"b4\"\n5: \"b5\"\n6: \"b6\"\n7: \
         \"b7\"\n",
    ),
    (
        "packet-duplicate-labels",
        "packet-beta\n0-7: \"pad\"\n8-15: \"data\"\n16-23: \"pad\"\n24-31: \"more\"\n",
    ),
    (
        "sankey-chain",
        "sankey-beta\na,b,10\nb,c,10\nc,d,10\n",
    ),
    (
        "sankey-diamond",
        "sankey-beta\nsource,left,6\nsource,right,4\nleft,sink,6\nright,sink,4\n",
    ),
    (
        "sankey-fan-out",
        "sankey-beta\nhub,one,5\nhub,two,3\nhub,three,2\nhub,four,1\n",
    ),
    (
        "sankey-fan-in",
        "sankey-beta\none,hub,5\ntwo,hub,3\nthree,hub,2\nfour,hub,1\n",
    ),
    (
        "sankey-repeated-pair",
        "sankey-beta\na,b,3\na,b,7\nb,c,10\n",
    ),
    (
        "sankey-zero-link",
        "sankey-beta\na,b,10\na,c,0\n",
    ),
    (
        "sankey-tiny-vs-huge",
        "sankey-beta\nbig,out,10000\nsmall,out,1\n",
    ),
    (
        "sankey-quoted-comma",
        "sankey-beta\n\"a, with comma\",b,5\nb,\"c, also\",5\n",
    ),
    (
        "sankey-escaped-quote",
        "sankey-beta\n\"say \"\"hi\"\"\",b,4\nb,c,4\n",
    ),
    (
        "sankey-cjk",
        "sankey-beta\n顧客,注文,12\n注文,発送,8\n注文,キャンセル,4\n",
    ),
    (
        "sankey-long-names",
        "sankey-beta\nA node whose name runs on quite a long way,B,5\nB,Another node with a very long \
         name indeed,5\n",
    ),
    (
        "sankey-many",
        "sankey-beta\nn01,sink,1\nn02,sink,2\nn03,sink,3\nn04,sink,4\nn05,sink,5\nn06,sink,6\nn07,sin\
         k,7\nn08,sink,8\nn09,sink,9\nn10,sink,10\nn11,sink,11\nn12,sink,12\n",
    ),
    (
        "sankey-disconnected",
        "sankey-beta\na,b,5\nc,d,3\n",
    ),
    (
        "sankey-title",
        "sankey-beta\ntitle Where the energy went\na,b,5\nb,c,5\n",
    ),
    (
        "sankey-equal",
        "sankey-beta\na,x,5\nb,x,5\nc,x,5\n",
    ),
    (
        // **Branches of different lengths.** Every other Sankey here has all its sinks at the same
        // depth, so `justify` — which pulls a node with nothing leaving it out to the last column —
        // could be removed without moving anything. `e` is a sink one step from the source while
        // the other branch is three steps long.
        "sankey-uneven-depths",
        "sankey-beta\na,b,5\nb,c,5\nc,d,5\na,e,3\n",
    ),
    (
        "sankey-long-chain",
        "sankey-beta\nn0,n1,4\nn1,n2,4\nn2,n3,4\nn3,n4,4\nn4,n5,4\nn5,n6,4\n",
    ),

];

// ---------------------------------------------------------------------------------------------
// The corpus parses, and its shape is what the source says
// ---------------------------------------------------------------------------------------------

/// Every case parses, and a second parse of the same text gives the same model.
///
/// The second half is not padding: five of these seven parsers carry state across a statement
/// (`hasSetXAxis`, the last bit of a packet, an open `{`), and a parser that leaked state between
/// runs would be caught here and nowhere else.
#[test]
fn every_case_parses_and_parsing_is_a_function_of_its_input() {
    for (name, src) in CASES {
        let first = describe(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        let second = describe(src).expect("parses twice");
        assert_eq!(first, second, "{name}: two parses of one source disagree");
    }
}

/// A short description of whichever chart this is, for the tests that only need "did it parse".
fn describe(src: &str) -> Result<String, ParseError> {
    if pie::is_pie(src) {
        return pie::parse(src).map(|p| format!("{p:?}"));
    }
    if xychart::is_xychart(src) {
        return xychart::parse(src).map(|p| format!("{p:?}"));
    }
    if quadrant::is_quadrant_chart(src) {
        return quadrant::parse(src).map(|p| format!("{p:?}"));
    }
    if radar::is_radar(src) {
        return radar::parse(src).map(|p| format!("{p:?}"));
    }
    if treemap::is_treemap(src) {
        return treemap::parse(src).map(|p| format!("{p:?}"));
    }
    if packet::is_packet(src) {
        return packet::parse(src).map(|p| format!("{p:?}"));
    }
    if sankey::is_sankey(src) {
        return sankey::parse(src).map(|p| format!("{p:?}"));
    }
    Err(ParseError::NotThisChart {
        expected: "chart",
        header: first_word(src),
    })
}

/// **No two of the seven predicates claim the same source.**
///
/// The routing arm in `preview::markdown` asks them in a fixed order, which would quietly hide an
/// overlap; this says there is none to hide. It matters because the keywords do share prefixes —
/// `packet` / `packet-beta`, `treemap` / `treemap-beta` — and because `is_*` reads only the header,
/// so an overlap would be a whole diagram kind routed to the wrong parser.
#[test]
fn exactly_one_chart_predicate_claims_each_source() {
    type Claims = (&'static str, fn(&str) -> bool);
    let predicates: &[Claims] = &[
        ("pie", pie::is_pie),
        ("xychart", xychart::is_xychart),
        ("quadrant", quadrant::is_quadrant_chart),
        ("radar", radar::is_radar),
        ("treemap", treemap::is_treemap),
        ("packet", packet::is_packet),
        ("sankey", sankey::is_sankey),
    ];
    for (name, src) in CASES {
        let claims: Vec<&str> = predicates
            .iter()
            .filter(|(_, is)| is(src))
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(claims.len(), 1, "{name}: claimed by {claims:?}");
    }
    // …and every spelling of every keyword lands on its own parser, including the ones that share
    // a prefix and the `-beta` suffixes.
    for (src, want) in [
        ("pie\n\"a\":1", "pie"),
        ("xychart\n  bar [1]", "xychart"),
        ("xychart-beta\n  bar [1]", "xychart"),
        ("quadrantChart\n  a: [0, 0]", "quadrant"),
        ("radar-beta\n  axis a,b,c\n  curve x{1,2,3}", "radar"),
        ("treemap\n\"a\": 1", "treemap"),
        ("treemap-beta\n\"a\": 1", "treemap"),
        ("packet\n0: \"a\"", "packet"),
        ("packet-beta\n0: \"a\"", "packet"),
        ("sankey\na,b,1", "sankey"),
        ("sankey-beta\na,b,1", "sankey"),
    ] {
        let claims: Vec<&str> = predicates
            .iter()
            .filter(|(_, is)| is(src))
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(claims, vec![want], "{src:?}");
    }
    // A keyword that only *starts* like one of them is nobody's.
    for src in [
        "pieces\n  a --> b",
        "packets\n  a",
        "treemapping\n  a",
        "sankeyish\n  a",
        "radar\n  axis a",
        "flowchart LR\n  A --> B",
        "sequenceDiagram\n  A->>B: hi",
    ] {
        assert!(
            predicates.iter().all(|(_, is)| !is(src)),
            "{src:?} was claimed by a chart parser"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The rules that are in the grammar and would not be guessed
// ---------------------------------------------------------------------------------------------

/// `NUMBER_PIE` takes a sign and `pieDb.addSection` then throws on a negative — two different
/// rules, and konoma reproduces both, with two different messages.
#[test]
fn a_pie_slice_may_be_written_negative_and_is_then_refused() {
    let e = pie::parse("pie\n  \"a\" : -5\n").unwrap_err();
    assert_eq!(
        e.to_string(),
        "`-5` at line 2: a slice's value must not be negative"
    );
    let e = pie::parse("pie\n  \"a\" : five\n").unwrap_err();
    assert_eq!(
        e.to_string(),
        "`five` at line 2: a slice's value must be a number"
    );
    // Zero is legal and stays in the model — the legend still names it (`render::chart::pie`).
    let p = pie::parse("pie\n  \"a\" : 0\n  \"b\" : 3\n").unwrap();
    assert_eq!(p.slices[0].value, 0.0);
}

/// A repeated pie label keeps the **first** value and is not an error: `pieDb.addSection` is
/// guarded by `if (!sections.has(label))`.
#[test]
fn a_repeated_pie_label_keeps_the_first_value() {
    let p = pie::parse("pie\n  \"a\" : 1\n  \"a\" : 99\n  \"b\" : 2\n").unwrap();
    assert_eq!(p.slices.len(), 2);
    assert_eq!(p.slices[0].value, 1.0);
}

/// A pie label **must** be quoted. `PieSection` is `label=STRING ":" value`, with no bare-word
/// alternative, so accepting `Dogs : 386` would draw a chart mermaid refuses.
#[test]
fn a_pie_label_must_be_quoted() {
    assert!(pie::parse("pie\n  Dogs : 386\n").is_err());
    assert!(pie::parse("pie\n  'Dogs' : 386\n").is_ok());
}

/// **Where the `x-axis` line sits changes what the plot means.** `hasSetXAxis` is false until an
/// axis statement runs, so a `bar` above it sets the range from its own length and a `bar` below a
/// band axis is truncated to the categories.
#[test]
fn an_xychart_resolves_its_data_against_the_axis_in_source_order() {
    // Band first: five values, three categories -> truncated to three.
    let after =
        xychart::parse("xychart-beta\n  x-axis [a, b, c]\n  bar [1, 2, 3, 4, 5]\n").unwrap();
    assert_eq!(after.plots[0].data.len(), 3);
    assert_eq!(
        after.plots[0].data,
        vec![
            ("a".to_string(), 1.0),
            ("b".to_string(), 2.0),
            ("c".to_string(), 3.0)
        ]
    );

    // Plot first: the x axis becomes linear 1..5 and every value survives.
    let before =
        xychart::parse("xychart-beta\n  bar [1, 2, 3, 4, 5]\n  x-axis [a, b, c]\n").unwrap();
    assert_eq!(before.plots[0].data.len(), 5);
    assert_eq!(before.plots[0].data[0].0, "1");
    assert_eq!(before.plots[0].data[4].0, "5");
    // …and the *later* band statement still wins for the axis itself.
    assert_eq!(
        before.x,
        xychart::XAxis::Band(vec!["a".into(), "b".into(), "c".into()])
    );

    // **The range a plot infers is then fixed**, because upstream reaches the inference through
    // `setXAxisRangeData`, whose last line is `hasSetXAxis = true` (`xychartDb.ts:91-93`). So the
    // first plot decides the span and a longer second plot is spread across that same span — it
    // does not widen it. Without this, the two plots resolve their keys against two different
    // ranges and the chart has two x axes.
    let two = xychart::parse("xychart-beta\n  bar [1, 2, 3]\n  bar [1, 2, 3, 4, 5]\n").unwrap();
    assert_eq!(
        two.x,
        xychart::XAxis::Linear { min: 1.0, max: 3.0 },
        "the second plot widened the range the first one fixed"
    );
    assert_eq!(
        two.plots[1]
            .data
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "1.5", "2", "2.5", "3"],
        "the longer plot is spread across the first plot's span"
    );
}

/// In an axis clause a digit is a **number**, never part of the title — the lexer's `axis_data`
/// state matches `NUMBER_WITH_DECIMAL` before the `[0-9]+` that feeds `alphaNum`.
#[test]
fn a_number_in_an_xychart_axis_clause_is_never_part_of_the_title() {
    let c = xychart::parse("xychart-beta\n  y-axis Units 0 --> 100\n  bar [1]\n").unwrap();
    assert_eq!(c.y_title, "Units");
    assert_eq!(c.y, (0.0, 100.0));
    // A range with no title at all.
    let c = xychart::parse("xychart-beta\n  y-axis 0 --> 10\n  bar [1]\n").unwrap();
    assert_eq!(c.y_title, "");
    assert_eq!(c.y, (0.0, 10.0));
}

/// A chart with no `bar` and no `line` is an error, not an empty frame: `getDrawableElem` throws
/// "No Plot to render". Same rule as stage 1's `graph TD`.
#[test]
fn an_xychart_with_no_plot_is_refused() {
    let e = xychart::parse("xychart-beta\n  x-axis [a, b]\n  y-axis 0 --> 1\n").unwrap_err();
    assert_eq!(e.to_string(), "xychart declares no plot");
}

/// **`horizontal` is read.** Not reading it would make the word an "unexpected statement" and the
/// whole chart refuse to draw, which is worse than drawing it the other way round (§2-3).
/// `render::chart::tests` holds the other half — that it is not applied.
#[test]
fn a_horizontal_xychart_is_parsed() {
    let c = xychart::parse("xychart-beta horizontal\n  bar [1, 2]\n").unwrap();
    assert_eq!(c.orientation, xychart::Orientation::Horizontal);
    let c = xychart::parse("xychart-beta vertical\n  bar [1, 2]\n").unwrap();
    assert_eq!(c.orientation, xychart::Orientation::Vertical);
    let c = xychart::parse("xychart-beta\n  bar [1, 2]\n").unwrap();
    assert_eq!(c.orientation, xychart::Orientation::Vertical);
}

/// **konoma keeps the spaces in unquoted text; upstream drops them.** Stated here as a test rather
/// than as a comment, because it is a deliberate departure (§0-1) and the next person to compare
/// konoma with mermaid needs to find it.
#[test]
fn unquoted_xychart_text_keeps_its_spaces_where_mermaid_removes_them() {
    let c = xychart::parse("xychart-beta\n  title Sales Revenue\n  bar [1]\n").unwrap();
    // mermaid's `alphaNum` concatenates with `$1 + '' + $2` and skips `\s+`, so upstream's answer
    // here is "SalesRevenue".
    assert_eq!(c.preamble.title.as_deref(), Some("Sales Revenue"));
}

/// A quadrant coordinate is `(1)|(0(.\d+)?)` and nothing else — `1.0` is a lexer error upstream.
#[test]
fn a_quadrant_coordinate_is_zero_one_or_a_fraction_of_one() {
    for good in ["0", "1", "0.5", "0.05", "0.999999"] {
        let src = format!("quadrantChart\n  P: [{good}, 0.5]\n");
        assert!(quadrant::parse(&src).is_ok(), "{good} should parse");
    }
    for bad in ["1.0", "2", "-0.1", "1.5", ".5", "0.", "one"] {
        let src = format!("quadrantChart\n  P: [{bad}, 0.5]\n");
        let e = quadrant::parse(&src).unwrap_err();
        assert!(
            e.to_string().contains("must be `0`, `1`, or `0.`"),
            "{bad}: {e}"
        );
    }
}

/// `x-axis Low Reach --> High Reach` reads **two** labels with their spaces, and an arrow with
/// nothing after it appends `" ⟶ "` to the one label there is.
#[test]
fn a_quadrant_axis_reads_both_ends_and_keeps_a_dangling_arrow() {
    let c =
        quadrant::parse("quadrantChart\n  x-axis Low Reach --> High Reach\n  P: [0, 0]\n").unwrap();
    assert_eq!(c.x_left, "Low Reach");
    assert_eq!(c.x_right, "High Reach");

    let c = quadrant::parse("quadrantChart\n  x-axis Low Reach -->\n  P: [0, 0]\n").unwrap();
    assert_eq!(c.x_left, "Low Reach ⟶ ");
    assert_eq!(c.x_right, "");

    let c = quadrant::parse("quadrantChart\n  y-axis Just one\n  P: [0, 0]\n").unwrap();
    assert_eq!(c.y_bottom, "Just one");
    assert_eq!(c.y_top, "");
}

/// **Two `axis` lines are six axes, not three.** The grammar's body is a `*` loop, so the lists
/// accumulate. The crate konoma is replacing keeps only the last line.
#[test]
fn radar_axis_statements_accumulate() {
    let r =
        radar::parse("radar-beta\n  axis a, b, c\n  axis d, e, f\n  curve x{1, 2, 3, 4, 5, 6}\n")
            .unwrap();
    assert_eq!(
        r.axes.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c", "d", "e", "f"]
    );
    assert_eq!(r.curves[0].entries.len(), 6);
}

/// A curve written with axis names is **looked up per axis**, so its source order does not matter
/// and a missing axis is an error ("Missing entry for axis …").
#[test]
fn a_named_radar_curve_is_ordered_by_the_axes_not_by_the_source() {
    let r = radar::parse("radar-beta\n  axis a, b, c\n  curve x{c: 3, a: 1, b: 2}\n").unwrap();
    assert_eq!(r.curves[0].entries, vec![1.0, 2.0, 3.0]);

    let e = radar::parse("radar-beta\n  axis a, b, c\n  curve x{a: 1, b: 2}\n").unwrap_err();
    assert!(e.to_string().contains("no entry for axis `c`"), "{e}");
}

/// A curve's braces may span lines (`Entries` has `NEWLINE*` between every element).
#[test]
fn a_radar_curve_may_span_lines() {
    let r = radar::parse("radar-beta\n  axis a, b, c\n  curve x{\n    1,\n    2,\n    3\n  }\n")
        .unwrap();
    assert_eq!(r.curves[0].entries, vec![1.0, 2.0, 3.0]);
}

/// The five options, their defaults, and the cap upstream puts on `ticks`.
#[test]
fn radar_options_have_upstreams_defaults_and_upstreams_ceiling() {
    let plain = radar::parse("radar-beta\n  axis a,b,c\n  curve x{1,2,3}\n").unwrap();
    assert!(plain.options.show_legend);
    assert_eq!(plain.options.ticks, 5);
    assert_eq!(plain.options.min, 0.0);
    assert_eq!(plain.options.max, None);
    assert_eq!(plain.options.graticule, radar::Graticule::Circle);
    // `max` unset means "the largest entry anywhere".
    assert_eq!(plain.max(), 3.0);

    let set = radar::parse(
        "radar-beta\n  axis a,b,c\n  curve x{1,2,3}\n  showLegend false, ticks 99\n  \
         max 10, min 1, graticule polygon\n",
    )
    .unwrap();
    assert!(!set.options.show_legend);
    assert_eq!(
        set.options.ticks,
        radar::MAX_TICKS,
        "upstream caps ticks at 32"
    );
    assert_eq!(set.options.max, Some(10.0));
    assert_eq!(set.options.min, 1.0);
    assert_eq!(set.options.graticule, radar::Graticule::Polygon);
}

/// A treemap value may carry thousands separators; `parseFloat(input.replace(/,/g, ''))`.
#[test]
fn a_treemap_value_may_contain_commas() {
    let t = treemap::parse("treemap-beta\n\"a\": 1,234.5\n").unwrap();
    assert_eq!(t.roots[0].value, Some(1234.5));
    // Either separator opens a leaf.
    let t = treemap::parse("treemap-beta\n\"a\", 7\n").unwrap();
    assert_eq!(t.roots[0].value, Some(7.0));
}

/// Nesting is by strictly-greater indent, **and only a section can hold children**: a leaf
/// indented under a leaf becomes that leaf's sibling, which is `buildHierarchy`'s behaviour and
/// not what the indentation looks like.
#[test]
fn a_leaf_cannot_hold_children() {
    let t = treemap::parse("treemap-beta\n\"a\": 1\n  \"b\": 2\n").unwrap();
    assert_eq!(t.roots.len(), 2, "`b` is a sibling of `a`, not its child");
    assert!(t.roots[0].children.is_empty());
}

/// A packet's fields must be contiguous, `+n` continues from the last bit, and a field that
/// crosses a row boundary is split in two — both halves keeping the label.
#[test]
fn packet_fields_are_contiguous_and_split_at_the_row_boundary() {
    let p = packet::parse("packet-beta\n0-15: \"a\"\n+16: \"b\"\n").unwrap();
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.rows[0][1].start, 16);
    assert_eq!(p.rows[0][1].end, 31);

    let p = packet::parse("packet-beta\n0-47: \"wide\"\n").unwrap();
    assert_eq!(p.rows.len(), 2, "a 48-bit field spans two 32-bit rows");
    assert_eq!((p.rows[0][0].start, p.rows[0][0].end), (0, 31));
    assert_eq!((p.rows[1][0].start, p.rows[1][0].end), (32, 47));
    assert_eq!(p.rows[1][0].label, "wide", "both halves keep the label");

    for (src, wanted) in [
        ("packet-beta\n0-7: \"a\"\n16-23: \"b\"\n", "not contiguous"),
        ("packet-beta\n7-0: \"a\"\n", "end must not be before start"),
        ("packet-beta\n+0: \"a\"\n", "zero bit field"),
    ] {
        let e = packet::parse(src).unwrap_err();
        assert!(e.to_string().contains(wanted), "{src:?}: {e}");
    }
}

/// A Sankey field is CSV: `""` is an escaped quote, a bare field may hold spaces and apostrophes,
/// and nodes are created in first-appearance order.
#[test]
fn sankey_reads_csv_the_way_rfc_4180_does() {
    let s = sankey::parse(
        "sankey-beta\n\"a, with comma\",b,1\n\"say \"\"hi\"\"\",b,2\nAgricultural 'waste',b,3\n",
    )
    .unwrap();
    assert_eq!(
        s.nodes,
        vec![
            "a, with comma".to_string(),
            "b".to_string(),
            "say \"hi\"".to_string(),
            "Agricultural 'waste'".to_string()
        ]
    );
    assert_eq!(s.links.len(), 3);
    // A repeated pair is a second link, not a merge.
    let s = sankey::parse("sankey-beta\na,b,1\na,b,2\n").unwrap();
    assert_eq!(s.links.len(), 2);
}

/// A Sankey with a loop in it has no left-to-right order to draw along, so it is refused rather
/// than drawn — the same conclusion d3-sankey's "circular link" reaches.
#[test]
fn a_sankey_cycle_is_refused() {
    let e = sankey::parse("sankey-beta\na,b,1\nb,c,1\nc,a,1\n").unwrap_err();
    assert!(e.to_string().contains("loops back on itself"), "{e}");
    let e = sankey::parse("sankey-beta\na,a,1\n").unwrap_err();
    assert!(e.to_string().contains("flows into itself"), "{e}");
    // A diamond is not a cycle.
    assert!(sankey::parse("sankey-beta\na,b,1\na,c,1\nb,d,1\nc,d,1\n").is_ok());
}

// ---------------------------------------------------------------------------------------------
// Deliberate drops
// ---------------------------------------------------------------------------------------------

/// **Everything konoma reads and does not draw, listed once, with both halves asserted.**
///
/// §2-3: 描画不要 ≠ パース不要. Each row is a statement the grammar has a rule for, and the test
/// says (a) the source still parses and (b) the statement did not become a data point. Without (b)
/// a `classDef` line would be read as a quadrant point's label and appear in the picture as a
/// phantom.
#[test]
fn deliberate_drops_are_parsed_and_produce_nothing() {
    // accTitle / accDescr, on every one of the seven. They are for screen readers and konoma's
    // output is a raster image, so reading them is the only thing they are for here.
    let acc = "  accTitle: a title for readers\n  accDescr: a description\n";
    let p = pie::parse(&format!("pie\n{acc}  \"a\" : 1\n")).unwrap();
    assert_eq!(p.slices.len(), 1, "an acc statement became a slice");
    assert_eq!(p.preamble.acc_title.as_deref(), Some("a title for readers"));
    assert_eq!(p.preamble.acc_descr.as_deref(), Some("a description"));

    let x = xychart::parse(&format!("xychart-beta\n{acc}  bar [1]\n")).unwrap();
    assert_eq!(x.plots.len(), 1);
    assert_eq!(x.preamble.acc_title.as_deref(), Some("a title for readers"));

    let q = quadrant::parse(&format!("quadrantChart\n{acc}  P: [0, 0]\n")).unwrap();
    assert_eq!(q.points.len(), 1, "an acc statement became a point");

    let r = radar::parse(&format!(
        "radar-beta\n{acc}  axis a,b,c\n  curve x{{1,2,3}}\n"
    ))
    .unwrap();
    assert_eq!(r.axes.len(), 3, "an acc statement became an axis");

    let t = treemap::parse(&format!("treemap-beta\n{acc}\"a\": 1\n")).unwrap();
    assert_eq!(t.roots.len(), 1, "an acc statement became a node");

    let k = packet::parse(&format!("packet-beta\n{acc}0: \"a\"\n")).unwrap();
    assert_eq!(k.rows[0].len(), 1, "an acc statement became a field");

    let s = sankey::parse(&format!("sankey-beta\n{acc}a,b,1\n")).unwrap();
    assert_eq!(s.links.len(), 1, "an acc statement became a link");

    // `accDescr { … }` over several lines, whose body must not be read as data either.
    let p = pie::parse("pie\n  accDescr {\n    \"not\" : 999\n  }\n  \"a\" : 1\n").unwrap();
    assert_eq!(p.slices.len(), 1, "the body of accDescr became a slice");
    assert!(p.preamble.acc_descr.is_some());

    // `classDef` and `:::`, in the two charts whose grammars have them.
    let q = quadrant::parse(
        "quadrantChart\n  classDef hot radius: 8, color: #ff0000\n  P:::hot: [0.2, 0.3]\n",
    )
    .unwrap();
    assert_eq!(q.points.len(), 1, "classDef became a point");
    assert_eq!(q.class_defs.len(), 1);
    assert_eq!(q.points[0].class.as_deref(), Some("hot"));
    assert_eq!(
        q.points[0].label, "P",
        "the class selector stayed in the label"
    );

    let t = treemap::parse("treemap-beta\nclassDef leafy fill:#f9f\n\"a\": 1:::leafy\n").unwrap();
    assert_eq!(t.roots.len(), 1, "classDef became a node");
    assert_eq!(t.class_defs.len(), 1);
    assert_eq!(t.roots[0].class.as_deref(), Some("leafy"));

    // A quadrant point's per-point styles.
    let q = quadrant::parse("quadrantChart\n  P: [0.2, 0.3] radius: 10, color: #ff0000\n").unwrap();
    assert_eq!(q.points[0].styles, vec!["radius: 10", "color: #ff0000"]);
    assert_eq!(q.points[0].x, 0.2);
}

/// The front matter, the `%%{init}%%` directives and the `%%` comments every mermaid diagram may
/// carry — removed by the shared preprocessor, and therefore not data here either.
#[test]
fn front_matter_directives_and_comments_are_not_data() {
    let src = "---\ntitle: From the front matter\n---\n%%{init: {'theme':'dark'}}%%\npie\n\
               %% a comment\n  \"a\" : 1\n";
    let p = pie::parse(src).unwrap();
    assert_eq!(p.slices.len(), 1);
    assert_eq!(p.preamble.title.as_deref(), Some("From the front matter"));
    // A body `title` overrides the front matter's.
    let p = pie::parse(&format!("{src}  title Body wins\n")).unwrap();
    assert_eq!(p.preamble.title.as_deref(), Some("Body wins"));
}

/// **`title "x"` keeps its quotes in six of the seven charts and loses them in `xychart`.**
///
/// Two different grammars: `common.langium`'s `TITLE` terminal takes the characters after the
/// keyword, while `xychart.jison`'s `title text` takes a `STR` and hands over what was inside it.
/// Pinned because it is exactly the sort of difference a shared preamble reader would flatten by
/// accident.
#[test]
fn the_three_spellings_of_title_are_kept_apart() {
    let p = pie::parse("pie title \"Quoted\"\n  \"a\" : 1\n").unwrap();
    assert_eq!(p.preamble.title.as_deref(), Some("\"Quoted\""));

    let q = quadrant::parse("quadrantChart\n  title \"Quoted\"\n  P: [0, 0]\n").unwrap();
    assert_eq!(q.preamble.title.as_deref(), Some("\"Quoted\""));

    let x = xychart::parse("xychart-beta\n  title \"Quoted\"\n  bar [1]\n").unwrap();
    assert_eq!(x.preamble.title.as_deref(), Some("Quoted"));

    // `title:` is not a title for the langium charts — the terminal wants whitespace after the
    // keyword — and is one for the jison ones.
    let q = quadrant::parse("quadrantChart\n  title: colon form\n  P: [0, 0]\n").unwrap();
    assert_eq!(q.preamble.title.as_deref(), Some("colon form"));
}

// ---------------------------------------------------------------------------------------------
// Adversarial: it must be an error or a model, and never a panic or a hang
// ---------------------------------------------------------------------------------------------

/// Sources chosen to break a parser rather than to be drawn.
///
/// Nothing here asserts *which* answer comes back — several of these are legitimately readable —
/// only that one comes back. §1 says the renderer runs on a worker thread inside `catch_unwind`,
/// so a panic would not take the UI down; it would take the diagram down silently, which is worse
/// than an error message.
#[test]
fn adversarial_sources_produce_a_model_or_an_error_and_never_a_panic() {
    let mut sources: Vec<String> = vec![
        // Headers with nothing under them.
        "pie".into(),
        "pie\n".into(),
        "xychart-beta".into(),
        "quadrantChart".into(),
        "radar-beta".into(),
        "treemap".into(),
        "packet-beta".into(),
        "sankey-beta".into(),
        // Zero, negative, enormous and unspellable numbers.
        "pie\n  \"a\" : 0\n  \"b\" : 0\n".into(),
        "pie\n  \"a\" : 0.0000000001\n  \"b\" : 99999999999999\n".into(),
        "xychart-beta\n  bar [0, 0, 0]\n".into(),
        "xychart-beta\n  bar [-1e9]\n".into(),
        "xychart-beta\n  y-axis 5 --> 5\n  bar [5]\n".into(),
        "xychart-beta\n  y-axis 100 --> 0\n  bar [50]\n".into(),
        "xychart-beta\n  bar [NaN]\n".into(),
        "xychart-beta\n  bar [inf]\n".into(),
        "xychart-beta\n  bar []\n".into(),
        "xychart-beta\n  bar [1\n".into(),
        "xychart-beta\n  x-axis [\n  bar [1]\n".into(),
        "radar-beta\n  axis a\n  curve x{1}\n".into(),
        "radar-beta\n  axis a,b,c\n  curve x{}\n".into(),
        "radar-beta\n  axis a,b,c\n  curve x{1,2,3}\n  max 0\n  min 0\n".into(),
        "radar-beta\n  axis a,b,c\n  curve x{\n".into(),
        "treemap\n\"a\": 0\n".into(),
        "treemap\n\"unclosed\n".into(),
        "packet-beta\n0-4294967295: \"huge\"\n".into(),
        "packet-beta\n99999999999: \"too big\"\n".into(),
        "sankey-beta\na,b\n".into(),
        "sankey-beta\na,b,c,d\n".into(),
        "sankey-beta\na,b,notanumber\n".into(),
        "sankey-beta\n\"never closed,b,1\n".into(),
        "sankey-beta\n,,\n".into(),
        // Unicode, emoji, control characters, and a lone surrogate's worth of oddity.
        "pie\n  \"🚀\" : 1\n  \"日本語のラベル\" : 2\n  \"\u{200b}\" : 3\n".into(),
        "treemap\n\"タブ\tと改行\": 1\n".into(),
        "quadrantChart\n  日本語: [0.5, 0.5]\n".into(),
        "sankey-beta\n顧客,注文,5\n".into(),
        // A label with two spaces in it, the case `docs/STATUS.md` records for the measuring path.
        "pie\n  \"loop  Every minute\" : 1\n".into(),
        // Statements in the wrong order, and keywords used as data.
        "pie\n  \"title\" : 1\n".into(),
        "quadrantChart\n  quadrant-1: [0.5, 0.5]\n".into(),
        "radar-beta\n  curve x{1,2,3}\n  axis a,b,c\n".into(),
        "packet-beta\n0: \"axis\"\n".into(),
    ];
    // 500 data points, which is the size at which an O(n^2) placement stops returning.
    let many: String = (0..500)
        .map(|i| format!("  \"s{i}\" : {}\n", i + 1))
        .collect();
    sources.push(format!("pie\n{many}"));
    let bars: String = (0..500).map(|i| format!("{i},")).collect();
    sources.push(format!(
        "xychart-beta\n  bar [{}]\n",
        bars.trim_end_matches(',')
    ));
    let links: String = (0..500).map(|i| format!("n{i},n{},1\n", i + 1)).collect();
    sources.push(format!("sankey-beta\n{links}"));
    let deep: String = (0..200)
        .map(|i| format!("{}\"s{i}\"\n", " ".repeat(i)))
        .collect();
    sources.push(format!("treemap\n{deep}  \"leaf\": 1\n"));

    for src in &sources {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| describe(src)));
        assert!(
            caught.is_ok(),
            "panicked on {:?}",
            &src[..src.len().min(120)]
        );
    }
}
