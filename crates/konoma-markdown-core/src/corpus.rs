pub mod task_corpus {
    /// Documents exercising every construct that the renderer treats specially, each paired with the
    /// number of checkboxes a user actually sees. The whole point is that the *write-back scanner* must
    /// agree with the *renderer* on this number for every one of them: when they disagree the safety
    /// check cancels every toggle in the document (a safe fallback, principle #3), so one stray construct
    /// breaks an entire file. Instances are cheap to add here; the class is what is being pinned.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            ("plain dash", "- [ ] a\n- [x] b\n"),
            ("bullet star", "* [ ] a\n* [x] b\n"),
            ("bullet plus", "+ [ ] a\n+ [x] b\n"),
            ("upper X", "- [X] a\n"),
            ("mixed bullets", "- [ ] a\n* [ ] b\n+ [x] c\n"),
            ("nested two levels", "- [ ] a\n  - [ ] b\n    - [x] c\n"),
            ("ordered list sibling", "1. plain\n2. also\n\n- [ ] task\n"),
            ("heading between", "- [ ] a\n\n## H\n\n- [ ] b\n"),
            ("alert NOTE", "> [!NOTE]\n> - [ ] a\n"),
            ("alert TIP", "> [!TIP]\n> - [ ] a\n> - [x] b\n"),
            ("alert IMPORTANT", "> [!IMPORTANT]\n> - [ ] a\n"),
            ("alert WARNING", "> [!WARNING]\n> - [x] a\n"),
            ("alert CAUTION", "> [!CAUTION]\n> - [ ] a\n"),
            ("alert titled", "> [!NOTE] My title\n> - [ ] a\n"),
            ("alert nested list", "> [!NOTE]\n> - [ ] a\n>   - [ ] b\n"),
            ("alert then plain", "> [!NOTE]\n> - [ ] in\n\n- [ ] out\n"),
            ("plain then alert", "- [ ] out\n\n> [!NOTE]\n> - [ ] in\n"),
            ("two alerts", "> [!NOTE]\n> - [ ] a\n\n> [!TIP]\n> - [ ] b\n"),
            ("plain blockquote", "> - [ ] quoted\n"),
            // These three pin the *reach* of `render_quote`'s model-only decoration (2026-08
            // investigation into this test's own oracle — see
            // `app::tests::md_task_toggle_is_byte_exact_across_the_corpus`'s doc comment): a nested
            // plain quote, a plain quote nested inside an alert's own body, and an alert nested
            // inside a plain quote. None of these is expected to be found by `task_source_locs`
            // either (that scanner deliberately still does not recognize a plain, non-alert quote at
            // all, at any depth — see `heading_gap_is_fixed_and_plain_blockquote_gap_has_no_mismatch`
            // in `task_scan_parity_tests`) — what these entries actually exercise is
            // `md_task_toggle_is_byte_exact_across_the_corpus`'s real, end-to-end write path
            // (`App::md_toggle_focused_task`) for a shape the legacy-calibrated scanner cannot
            // describe, not the scanner itself.
            ("nested plain blockquote (two levels)", "> > - [ ] quoted twice\n"),
            (
                "plain blockquote nested inside an alert",
                "> [!NOTE]\n> > - [ ] quoted in alert\n",
            ),
            (
                "alert nested inside a plain blockquote",
                "> > [!NOTE]\n> > - [ ] alert in quote\n",
            ),
            (
                "details closed",
                "<details>\n<summary>S</summary>\n\n- [ ] hidden\n\n</details>\n",
            ),
            (
                "details open",
                "<details open>\n<summary>S</summary>\n\n- [ ] shown\n\n</details>\n",
            ),
            (
                "details closed then plain",
                "<details>\n<summary>S</summary>\n\n- [ ] hidden\n\n</details>\n\n- [ ] after\n",
            ),
            (
                "details open then plain",
                "<details open>\n<summary>S</summary>\n\n- [ ] shown\n\n</details>\n\n- [ ] after\n",
            ),
            ("fence backtick", "```\n- [ ] not a task\n```\n\n- [ ] real\n"),
            ("fence tilde", "~~~\n- [ ] not a task\n~~~\n\n- [ ] real\n"),
            (
                "fence with language",
                "```rust\n// - [ ] no\n```\n\n- [ ] real\n",
            ),
            (
                "table then task",
                "| a | b |\n|---|---|\n| 1 | 2 |\n\n- [ ] after table\n",
            ),
            // A table cell holding an image (`![alt](url)`, e.g. the badge-table idiom common in
            // README files such as `ndk`'s) used to fall through `parse_cell_segments` one character
            // at a time as literal text — the raw `![AAA](a.svg)` markup showed up verbatim instead
            // of even the alt text. Fixed by giving `CellSeg` an `Image` variant; pinned here through
            // the golden-snapshot net (no checkboxes in this one — same as "no tasks" above).
            (
                "table cell containing an image",
                "| badge | meaning |\n|---|---|\n| ![AAA](a.svg) | first |\n",
            ),
            // The more common real-world shape (shields.io-style CI/version badges in a crate's
            // README table): the image is itself wrapped in a click-through link,
            // `[![alt](img)](href)`. See `parse_link_wrapped_image_body`'s own doc comment for why
            // this renders with the image's alt text but the *link's* own href as the open target.
            (
                "table cell containing a link-wrapped image",
                "| crate | badges |\n|---|---|\n| ndk | [![CI](a.svg)](https://ci.example) |\n",
            ),
            ("html block then task", "<div>hi</div>\n\n- [ ] after html\n"),
            ("inline code lookalike", "- [ ] real `- [ ] fake`\n"),
            ("link in task", "- [ ] see [docs](./d.md)\n"),
            ("cjk", "- [ ] 日本語のタスク\n- [x] 全角　スペース\n"),
            ("emphasis in task", "- [ ] **bold** and *em*\n"),
            ("no trailing newline", "- [ ] a\n- [x] b"),
            ("crlf", "- [ ] a\r\n- [x] b\r\n"),
            ("front matter", "---\ntitle: t\n---\n\n- [ ] a\n"),
            // Root cause (b)'s "a pseudo-task inside front matter" is **deliberately not included
            // here**: the premise differs. `md_task_toggle_is_byte_exact_across_the_corpus`
            // (app/tests.rs), which uses this corpus, derives its "expected write position" by
            // working backward from the plain (front-matter-unaware) `task_source_locs(src, ..)`,
            // which is a different implementation from the App side (root cause (b)'s fix) that
            // actually strips front matter out of the body before scanning. Covered directly by the
            // dedicated test `md_task_toggle_skips_pseudo_tasks_inside_front_matter` (app/tests.rs).
            ("footnote", "- [ ] a[^1]\n\n[^1]: note\n"),
            (
                "fence containing a table lookalike",
                "```text\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n\n- [ ] real\n",
            ),
            (
                "fence containing an html block lookalike",
                "```html\n<div class=\"x\">\nhello\n</div>\n```\n\n- [ ] real\n",
            ),
            (
                "fence containing an alert lookalike",
                "```text\n> [!NOTE]\nlooks like an alert\n```\n\n- [ ] real\n",
            ),
            ("empty doc", ""),
            ("no tasks", "# just text\n\nparagraph\n"),
            (
                "everything",
                "# Doc\n\n- [ ] top\n\n> [!NOTE] Heads up\n> - [ ] in alert\n> - [x] done\n\n\
                 | a | b |\n|---|---|\n| 1 | 2 |\n\n```\n- [ ] fenced\n```\n\n\
                 <details>\n<summary>More</summary>\n\n- [ ] collapsed\n\n</details>\n\n- [x] bottom\n",
            ),
            // Root cause (indented code blocks, 2026-08): a top-level indented (4+ column) code
            // block was not recognized as code by the write-back scanner at all — a `- [ ] fake`
            // example inside one was mistaken for a real, toggleable checkbox.
            (
                "task-lookalike inside a top-level indented code block stays literal",
                "para\n\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike inside an indented block nested in an alert stays literal",
                "> [!NOTE]\n> para\n>\n>     - [ ] fake\n>\n> - [ ] real\n",
            ),
            (
                "task-lookalike inside an indented block nested in an open details stays literal",
                "<details open>\n<summary>S</summary>\n\npara\n\n    - [ ] fake\n\n- [ ] real\n\n</details>\n",
            ),
            (
                "a real task at a list item's own indentation is still a real task",
                "- [ ] outer\n\n  - [ ] nested at matching indent\n",
            ),
            (
                "an indented paragraph inside a list item is not a task even if it starts with a dash",
                "- item\n\n  - [ ] looks like a task but is just this item's own indentation\n",
            ),
            // Root cause (fence unification, 2026-08) — see the matching comment in `code_corpus`:
            // a task-lookalike must stay hidden (literal code) for exactly as long as the fence it
            // sits inside is actually still open, per CommonMark's char/length/indentation rules —
            // not per a naive "any 3-of-the-same-char line toggles it" approximation.
            (
                "task-lookalike inside a fence indented 3 columns (still a real fence)",
                "para\n\n   ```rust\n   - [ ] fake\n   ```\n\n- [ ] real\n",
            ),
            (
                "task-lookalike inside a fence-lookalike top-level indented code block",
                "para\n\n    ```rust\n    - [ ] fake\n    ```\n\n- [ ] real\n",
            ),
            (
                "task-lookalike inside a tilde fence with a shorter nested tilde lookalike",
                "~~~~md\n~~~\n- [ ] fake\n~~~\n~~~~\n\n- [ ] real\n",
            ),
            // Fence unification stage 2: same shape as the tilde entry above, but with the
            // renderer's own now-fixed same-character-backtick-nesting bug (see the matching
            // comment in `code_corpus`) — before stage 2 the renderer drew **zero** checkboxes for
            // this document (the fake one hidden as intended, but the real one also swallowed into
            // the bogus second code block), disagreeing with the scanner's correct count of 1.
            (
                "task-lookalike inside a backtick fence with a shorter nested backtick lookalike (same char)",
                "````md\n```\n- [ ] fake\n```\n````\n\n- [ ] real\n",
            ),
            // Root cause (alert/details mutual nesting, 2026-08): an alert nested inside a
            // `<details>` block used to render unconditionally, leaking regardless of the fold —
            // and the reverse (a `<details>` nested inside an alert) had no fold at all (always
            // shown). See `render_md_body_nested`'s doc comment. These entries pin the write-back
            // scanner's (`task_source_locs`/`scan_task_lines`) side of that fix: the checkbox count
            // must still match what's actually drawn, in both nesting directions and both
            // open/closed states, or every toggle in the document gets refused.
            (
                "alert nested inside a closed details hides its task",
                "<details>\n<summary>S</summary>\n\n> [!NOTE]\n> - [ ] hidden\n\n</details>\n",
            ),
            (
                "alert nested inside an open details shows its task",
                "<details open>\n<summary>S</summary>\n\n> [!NOTE]\n> - [ ] shown\n\n</details>\n",
            ),
            (
                "details nested inside an alert, closed, hides its task",
                "> [!NOTE]\n> <details>\n> <summary>S</summary>\n>\n\
                 > - [ ] hidden\n>\n> </details>\n",
            ),
            (
                "details nested inside an alert, open, shows its task",
                "> [!NOTE]\n> <details open>\n> <summary>S</summary>\n>\n\
                 > - [ ] shown\n>\n> </details>\n",
            ),
            (
                "a details nested inside an alert nested inside a closed details hides everything",
                "<details>\n<summary>Outer</summary>\n\n> [!NOTE]\n> <details open>\n\
                 > <summary>Inner</summary>\n>\n> - [ ] deeply hidden\n>\n> </details>\n\n\
                 </details>\n\n- [ ] real\n",
            ),
            (
                "a details nested inside an alert nested inside an open details shows everything",
                "<details open>\n<summary>Outer</summary>\n\n> [!NOTE]\n> <details open>\n\
                 > <summary>Inner</summary>\n>\n> - [ ] deeply shown\n>\n> </details>\n\n\
                 </details>\n\n- [ ] real\n",
            ),
            (
                "an alert nested inside a details nested inside an alert, all open, shows the task",
                "> [!NOTE]\n> <details open>\n> <summary>Inner</summary>\n>\n\
                 > > [!TIP]\n> > - [ ] triple-nested\n>\n> </details>\n",
            ),
            // Root cause (heading/thematic-break/setext gate, 2026-08): an indented block right
            // after a heading, thematic break, or setext heading underline (no blank line needed —
            // only a paragraph gates a following indented block, and none of these three is one) was
            // not recognized by the write-back scanner at all, so a `- [ ] fake` example inside one
            // was mistaken for a real, toggleable checkbox. Mirrors the matching `code_corpus`
            // entries below.
            (
                "task-lookalike in an indented block right after an ATX heading (no blank) stays literal",
                "## Usage\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike in an indented block right after a thematic break (no blank) stays literal",
                "---\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike in an indented block right after a setext heading underline (no blank) stays literal",
                "Title\n=====\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike in an indented block right after a short setext dash underline (no blank) stays literal",
                "Title\n--\n    - [ ] fake\n\n- [ ] real\n",
            ),
        ]
    }
}

pub mod code_corpus {
    /// Documents exercising indented (4+ column) code blocks and their interaction with the other
    /// constructs the renderer treats specially — mirrors `task_corpus`. The write-back scanner
    /// (`code_block_source_locs`) must agree with the renderer on the number of code blocks for
    /// every one of these, or `y c` gets refused for the whole document (one stray construct breaks
    /// the whole file — reported by the user for indented code blocks specifically, 2026-08).
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "standalone indented block",
                "Normal paragraph.\n\n    indented code line\n    second line\n\nTail.\n",
            ),
            ("indented block at doc start", "    line one\n    line two\n"),
            ("tab-indented block", "para\n\n\ttabbed line\n"),
            (
                "not code: lazy continuation right after a paragraph (no blank)",
                "para\n    not code, just continues the paragraph\n",
            ),
            (
                "not code: inside a list item at the item's own content column",
                "- item\n\n  still item content, not code\n",
            ),
            (
                "not code: inside an ordered list item at the item's own content column",
                "1. item\n\n   still item content, not code\n",
            ),
            (
                "not code: 3-space indent is not enough",
                "para\n\n   three spaces is not enough\n",
            ),
            (
                "two chunks glued by a blank separator into one code block",
                "para\n\n    chunk one\n\n    chunk two\n",
            ),
            (
                "mixed: indented block then (after a blank) a real fence",
                "para\n\n    indented\n\n```rust\nfenced\n```\n",
            ),
            (
                "mixed: fence then an indented block right after (no blank needed)",
                "```\nfenced\n```\n    indented after fence\n",
            ),
            (
                "indented block nested inside an open alert",
                "> [!NOTE]\n> para\n>\n>     indented in alert\n",
            ),
            (
                "indented block nested inside an open details",
                "<details open>\n<summary>S</summary>\n\npara\n\n    indented in details\n\n</details>\n",
            ),
            (
                "indented block nested inside a closed details is not counted",
                "<details>\n<summary>S</summary>\n\npara\n\n    hidden\n\n</details>\n\n```\nreal\n```\n",
            ),
            (
                "indented block containing a task-lookalike stays literal code",
                "para\n\n    - [ ] not a real task\n",
            ),
            (
                "front matter then an indented block",
                "---\ntitle: t\n---\n\npara\n\n    indented after front matter\n",
            ),
            (
                "crlf indented block",
                "para\r\n\r\n    indented\r\n    second\r\n",
            ),
            ("no trailing newline", "para\n\n    indented"),
            (
                "indented block followed by a real list",
                "para\n\n    indented\n\n- item\n",
            ),
            (
                "a plain (non-alert) block quote wrapping a fence is not detected by either side",
                "> para\n>\n>     code\n",
            ),
            (
                "everything",
                "# Doc\n\nintro\n\n    top level indented\n\n> [!NOTE]\n> body\n>\n>     indented in note\n\n\
                 <details open>\n<summary>More</summary>\n\ndetail para\n\n    indented in details\n\n</details>\n\n\
                 ```rust\nfn real_fence() {}\n```\n",
            ),
            // Root cause (fence unification, 2026-08): several scanners tracked "am I inside a fence"
            // with their own naive per-function toggle (`starts_with("```") || starts_with("~~~")`,
            // closing on the first line that merely starts with 3 of the same char) instead of the
            // file's one CommonMark-correct `parse_fence` (matching char, length ≥ the opener's, and
            // requiring 0-3 columns of indentation to even count as a fence at all). The naive version
            // let the *count* the count-guard checks happen to still agree with what the renderer draws
            // for these shapes (both sides were wrong in step), while the *content* — verified against
            // `code_block_content_matches_the_render_for_fence_edge_cases` below — was corrupted. These
            // entries pin count parity; content is pinned separately.
            (
                "fence indented 3 columns is still a fence, not an indented code block",
                "para\n\n   ```rust\n   fn a(){}\n   ```\n",
            ),
            (
                "a fence-lookalike indented 4 columns is the literal content of an indented code block",
                "para\n\n    ```rust\n    fn a(){}\n    ```\n",
            ),
            (
                "closing fence line with a language suffix does not close (has to be exactly the fence chars)",
                "```rust\nbody\n```js\nmore\n```\n",
            ),
            (
                "a shorter nested tilde fence lookalike inside a longer tilde fence doesn't close it",
                "~~~~md\n~~~\ninner\n~~~\n~~~~\n",
            ),
            (
                "a tilde-fence lookalike nested inside a backtick fence doesn't close it (different fence char)",
                "```rust\n~~~\nnot closing\n~~~\n```\n",
            ),
            ("plain 4-backtick fence, no nesting", "````rust\nbody\n````\n"),
            // Root cause (fence unification stage 2, 2026-08): `decorate_code_blocks` (the
            // **renderer**) had its own, independent version of the exact same class of bug as the
            // scanner entries above — it scanned rendered *text* for `` ``` `` instead of keying off
            // tui-markdown's own per-line *style* (`is_code_block_line`) — so a shorter nested
            // fence-lookalike of the *same character* as its enclosing fence used to be mistaken for
            // the real close, silencing the rest of the document into a bogus second code block.
            // Deliberately outside the strict `drawn == scanned` loop in stage 1 (the renderer's own
            // count was 2 there, disagreeing with the scanner's correct 1); now that the renderer is
            // fixed too, this belongs in the corpus like every other fence shape.
            (
                "a shorter nested backtick fence lookalike inside a longer backtick fence doesn't close it (same char)",
                "````md\n```\ninner\n```\n````\n",
            ),
            // Root cause (heading/thematic-break/setext gate, 2026-08): an indented code block right
            // after a heading, thematic break, or setext heading underline — with no blank line
            // needed, since none of those three is a paragraph — was not recognized by the
            // write-back scanner at all, so any document containing one refused `y c` for **every**
            // code block in it, fenced ones included (the count guard is document-wide). Reported by
            // the user via a heading directly followed by an indented example, with an unrelated real
            // fence right after it also collaterally refused.
            (
                "indented block immediately after an ATX heading (no blank line)",
                "## Usage\n    npm install foo\n",
            ),
            (
                "indented block immediately after a thematic break (no blank line)",
                "---\n    code here\n",
            ),
            (
                "indented block immediately after a setext level-1 heading underline (no blank line)",
                "Title\n=====\n    code here\n",
            ),
            (
                "indented block immediately after a short setext dash underline, too short to also be a thematic break (no blank line)",
                "Title\n--\n    code here\n",
            ),
            (
                "heading then an indented block then an unrelated real fence — the fence must not be collaterally refused",
                "## Usage\n    npm install foo\n\n```bash\nnpm test\n```\n",
            ),
            // ---- Container context (2026-08) ----
            //
            // Root cause: CommonMark measures a block's indentation **relative to the container it
            // sits in**, while the hand-written scanner measured absolute columns. Inside a list item
            // every column shifts right by the marker's width, so the two sides disagreed in *both*
            // directions — a fence indented 4 columns under `1. ` is a real fence that got called an
            // indented code block, and a continuation paragraph indented 6 columns under `   4. ` is
            // not code but got counted as one. Either way the count guard broke and `y c` was refused
            // for the whole document. Both shapes are ordinary (a numbered install guide; the Apache
            // license text) — 26 of 2,182 real `.md` files in the crate registry tripped it.
            //
            // This axis is **orthogonal** to the fenced/indented axis above and was missing from the
            // corpus entirely — which is how it shipped. The entries above were written one per past
            // bug; these are written from the spec's own case split (container × content), which is
            // the only way a corpus covers something nobody has hit yet.
            //
            // Fixed by handing the leaf detection to pulldown-cmark — the very parser the renderer
            // renders through — instead of re-deriving its rules (see `parser_code_blocks`).
            (
                "① fence indented 4 columns inside an ordered item is a real fence (registry: pastey CONTRIBUTING.md)",
                "1. Fork it:\n\n    ```sh\n    git clone x\n    ```\n\n2. Done.\n",
            ),
            (
                "② 6-column continuation paragraph under `   4. ` is not code (registry: LICENSE-APACHE.md)",
                "   4. Redistribution. You may reproduce and distribute copies of the\n\n      (b) You must cause any modified files to carry prominent notices\n",
            ),
            (
                "fence at a bullet item's own content column",
                "- Step:\n\n  ```sh\n  echo hi\n  ```\n",
            ),
            (
                "fence indented 4 columns inside a bullet item is still a fence",
                "- Step:\n\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "fence-lookalike 4 columns past a bullet item's content column is indented code",
                "- Step:\n\n      ```sh\n      echo hi\n      ```\n",
            ),
            (
                "indented code inside a bullet item (content column + 4)",
                "- Step:\n\n      code line\n      second line\n",
            ),
            (
                "tilde fence inside a bullet item",
                "- Step:\n\n  ~~~sh\n  echo hi\n  ~~~\n",
            ),
            (
                "tight list: fence on the line right after the marker, no blank line",
                "- Step:\n  ```sh\n  echo hi\n  ```\n",
            ),
            (
                "tight ordered list: fence indented 4 columns right after the marker line",
                "1. Do:\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "fence at an ordered item's own content column",
                "1. Step:\n\n   ```sh\n   echo hi\n   ```\n",
            ),
            (
                "indented code inside an ordered item (content column + 4)",
                "1. Step:\n\n       code line\n",
            ),
            (
                "a two-digit ordered marker shifts the content column right",
                "10. Step:\n\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "ordered list with a `)` delimiter",
                "1) Step:\n\n   ```sh\n   echo hi\n   ```\n",
            ),
            (
                "lazy continuation inside a list item is not code",
                "- item text\n    still the same paragraph\n",
            ),
            (
                "fence inside a nested bullet item",
                "- outer\n  - inner:\n\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "continuation paragraph at a nested item's content column is not code",
                "- outer\n  - inner\n\n    still inner content\n",
            ),
            (
                "indented code inside a nested item (nested content column + 4)",
                "- outer\n  - inner\n\n        code line\n",
            ),
            (
                "fence inside a plain block quote draws no header (tui-markdown prefixes every line)",
                "> quoted:\n>\n> ```sh\n> echo hi\n> ```\n",
            ),
            // These two mirror `task_corpus`'s "nested plain blockquote"/"alert nested inside a
            // plain blockquote" pair (2026-08 investigation into `code_block_source_locs`'s own
            // reach — see `app::tests::md_task_toggle_is_byte_exact_across_the_corpus`'s doc
            // comment): a fence inside a *nested* plain quote, and a fence inside an alert nested
            // inside a plain quote. `code_block_source_locs` is not expected to find either (it
            // still has no plain-quote branch at any depth) — these exercise
            // `md_code_block_copy_is_byte_exact_across_the_corpus`'s real `y c` path
            // (`focused_code_source`, reading straight off the model's own `RenderOut::code_blocks`)
            // for a shape that scanner cannot describe.
            (
                "fence inside a nested plain block quote (two levels)",
                "> > ```sh\n> > echo hi\n> > ```\n",
            ),
            (
                "fence inside an alert nested inside a plain block quote",
                "> > [!NOTE]\n> > ```sh\n> > echo hi\n> > ```\n",
            ),
            (
                "block quote nested inside a list item, wrapping an indented block",
                "- item\n\n  > quoted:\n  >\n  >     code\n",
            ),
            (
                "block quote nested inside a list item, wrapping a fence",
                "- item\n\n  > ```sh\n  > echo hi\n  > ```\n",
            ),
            (
                "an alert inside a list item",
                "- item\n\n  > [!NOTE]\n  > ```sh\n  > echo hi\n  > ```\n",
            ),
            (
                "a fence in a list item, then a real top-level fence after the list ends",
                "1. Step:\n\n    ```sh\n    echo hi\n    ```\n\nAfter the list.\n\n```rust\nfn a(){}\n```\n",
            ),
            // The mermaid diversion is measured in **absolute** columns (`parse_fence`, used by
            // `split_segments`/`split_block_parts`), while CommonMark measures the fence itself
            // relative to the list item — so these two neighbours land on opposite sides on purpose.
            (
                "a mermaid fence at an ordered item's content column is diverted to a diagram",
                "1. Diagram:\n\n   ```mermaid\n   flowchart TD\n   A-->B\n   ```\n",
            ),
            (
                "a mermaid fence indented 4 columns is left in the text and drawn as ordinary code",
                "1. Diagram:\n\n    ```mermaid\n    flowchart TD\n    A-->B\n    ```\n",
            ),
            // A clean, top-level fence — not nested in a list/quote/details, nothing else in the
            // document to trigger any *other*, unrelated mismatch — so this case's own `lines`
            // always match between the two renderers, and its `images` comparison in
            // `md_render_diff_tests` is never masked by a pre-existing, tracked mismatch the way the
            // two neighbouring mermaid cases above are (both already `BEHAVIOR_DECISIONS`/
            // `INTENDED_IMPROVEMENTS`-listed for their own, unrelated `lines`-level reasons, which
            // would silently absorb an `ImagePlacement` regression too — see
            // `md_render_diff_tests`'s own module doc comment on why `images` needs its own,
            // independent comparison in the first place). Exists specifically so this corpus can pin
            // a real, unmasked regression in `render_mermaid_slot`'s own `ImagePlacement` fields
            // (`url`/`line`/`cols`/`rows`/`fence_ord`/`alt`) — see `mermaid_fence_url`'s own doc
            // comment on why `url` in particular being wrong is invisible to `lines` entirely.
            (
                "a top-level mermaid fence with no surrounding complication (image placement parity)",
                "```mermaid\nA-->B\n```\n",
            ),
            // Root cause (block-image extraction, 2026-08): pulling an `<img>` line out of a centered
            // banner splits the surrounding HTML block in two, and the `    |` separator left between
            // the halves becomes a real indented code block on screen. Only visible through the
            // production entry point — see `rendered_code_blocks`.
            (
                "4-column-indented HTML inside a centered banner is an HTML block, not indented code",
                "<div align=\"center\">\n    <a href=\"https://example.com\">Docs</a>\n</div>\n\npara\n",
            ),
            // Not `KNOWN_MISMATCHES`/`INTENDED_IMPROVEMENTS`/`BEHAVIOR_DECISIONS` in `md_render_diff_tests`
            // at all, despite `render_doc` visibly *fixing* a real content-loss bug here (an `Html`
            // block nested inside a `Quote` used to vanish from the screen entirely — see
            // `model::BlockKind::Html.body_spans`'s own doc comment for the confirmed per-line
            // `Event::Html` dump the fix relies on): `render_case` — this harness's own "production"
            // side — calls the real `render_markdown_with_images` *dispatcher*, not the legacy
            // renderer directly, and that dispatcher now routes this exact shape to `render::render_doc`
            // too (`RenderOut::unsupported` is empty for it, post-fix), the identical function
            // `render_case_new` (the "new renderer" side) also calls — so both sides of *this*
            // harness's comparison are the same function call on the same input, an exact match by
            // construction, not a case this stage's own architecture merely happens to draw the same
            // way production's separate legacy path does. See
            // `render::tests::html_block_nested_inside_a_quote_renders_its_content_instead_of_vanishing`
            // for the dedicated regression test that pins the actual fixed *content*, and the legacy
            // fallback's own (`markdown.rs`'s `banner_quoted_multiline_no_longer_leaks_its_leftover_fragments`)
            // for what changed on that separate, no-longer-reached path. Kept in this corpus anyway —
            // exercising the shape here still catches a regression that made this stage stop matching
            // (or, worse, start reporting `Unsupported`/mismatching again), it is simply not the *kind*
            // of gap those three lists exist to catalogue.
            (
                "an HTML block nested inside a blockquote is drawn instead of dropped entirely",
                "> <div align=\"center\">\n> HTML-INSIDE-QUOTE\n> </div>\n",
            ),
            // konoma draws GFM tables itself (tui-markdown collapses them), so the parser never sees
            // those lines — and with `ENABLE_TABLES` off it would read them as an ordinary paragraph,
            // which changes whether the line after them can open an indented code block.
            (
                "an indented line right after a table block, with no blank line between",
                "| a | b |\n|---|---|\n    code here\n",
            ),
            // Both of these were named, in 2026-08, for what konoma's *own* HTML-block splitter did
            // back when the renderer had one: it ran a start line to the **next blank line**, which
            // is wider than CommonMark's end conditions, so the indented line following the tag was
            // swallowed into the HTML block and drawn as rescued HTML text rather than as code. The
            // block model reads pulldown-cmark's structure instead, and CommonMark makes both of
            // these lines indented code — measured against the parser, not assumed:
            //
            //   * `<!-- note -->` opens an HTML block of **type 2** (a comment), which ends on the
            //     line holding `-->`. So the block is that one line, and the 4-column-indented line
            //     after it opens an indented code block. For `"<!-- note -->\n    code here\n"`
            //     pulldown-cmark reports `Start(HtmlBlock) 0..14` / `End(HtmlBlock) 0..14`, then
            //     `Start(CodeBlock(Indented)) 18..28`.
            //   * `<span>` is HTML block **type 7**, the one type that cannot interrupt a paragraph
            //     — so `para\n<span>x</span>` is a single paragraph carrying inline HTML, and after
            //     the blank line the indented line is again indented code (`Start(Paragraph) 0..20`
            //     … `Start(CodeBlock(Indented)) 25..35`).
            //
            // Both names claimed the opposite while both goldens had been dumping the code block all
            // along (header sentinel, gutter, one `CodeBlock` item) — a case name contradicting its
            // own recorded output, which is worse than no name at all: it is the shape that makes a
            // reviewer accept a regenerated snapshot showing the "expected" loss. Renamed, not
            // "fixed": the rendering is what CommonMark specifies.
            (
                "an indented line after a one-line HTML comment block is indented code",
                "<!-- note -->\n    code here\n",
            ),
            (
                "an indented line after a paragraph ending in an inline tag is indented code",
                "para\n<span>x</span>\n\n    code here\n",
            ),
            (
                "a centered banner whose links wrap images (registry: criterion README.md)",
                "<div align=\"center\">\n    <a href=\"https://example.com/ci\"><img src=\"https://img.example/badge.svg\" alt=\"CI\"></a>\n    |\n    <a href=\"https://example.com/crate\"><img src=\"https://img.example/v.svg\" alt=\"Crates.io\"></a>\n</div>\n\npara\n",
            ),
            // Task markers in the same container contexts — this corpus is shared with the checkbox
            // write-back scanner (`task_scanner_matches_renderer_across_indented_code_corpus`), which
            // has the same "second implementation of the renderer" shape and so the same failure mode
            // (one stray construct cancels every toggle in the document).
            (
                "checkbox in a nested list item written with three spaces after the bullet (registry: zerocopy agent_docs)",
                "*   **Checklist:**\n    *   [ ] Can this be done with an existing utility?\n    *   [x] Done already\n",
            ),
            (
                "checkbox with nothing after the closing bracket (registry: line-clipping README.md)",
                "- [x]\n- [ ] with text\n",
            ),
            (
                "checkbox with four spaces after the bullet",
                "-    [ ] four spaces is still a task\n",
            ),
            (
                "checkbox-lookalike five spaces after the bullet (the item's content starts with code)",
                "-     [ ] five spaces is not a task marker\n",
            ),
            (
                "checkbox in an ordered list item",
                "1. [ ] ordered task\n2. [x] done\n",
            ),
            (
                "checkbox inside a nested bullet",
                "- outer\n  - [ ] inner task\n",
            ),
            (
                "checkbox inside a plain block quote",
                "> - [ ] quoted task\n",
            ),
            (
                "checkboxes around a fence indented inside an ordered item",
                "- [ ] before\n\n1. Fork it:\n\n    ```sh\n    git clone x\n    ```\n\n- [x] after\n",
            ),
            // ---- Closing line × container (2026-08) ----
            //
            // Root cause: a fence is closed by a line of the same char, at least as long, **with
            // nothing after it** — and, failing that, by the end of the container it opened in. The
            // hand-written mask honored the first rule and had no concept of the second, so a fence
            // whose closing line carries trailing text — `` ``` ([#2642](…)) ``, a shape that reads
            // as perfectly natural when writing a changelog entry — ran to the end of the *document*
            // instead of the end of its list item, and every construct below it was treated as being
            // inside code. nix's CHANGELOG.md does this at line 110; the 617-line GFM table below
            // rendered as one line of raw pipes (all three vendored nix versions in the registry).
            //
            // The axis being pinned is (does this line close? × what container is the fence in? ×
            // what follows it?), written from the spec's case split rather than from the one shape
            // that was reported — the closing-line half was missing from the corpus entirely, which
            // is how it shipped.
            (
                "closing line with trailing text does not close, but the list item ends the block",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` ([#1](https://example.com/1))\n\n- next\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text inside a nested list item",
                "- outer\n  - inner\n\n    ```rust\n    let x = 1;\n    ``` (note)\n\n- after\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text inside a block quote",
                "> ```rust\n> let x = 1;\n> ``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text inside a list item inside a block quote",
                "> - item\n>\n>   ```rust\n>   let x = 1;\n>   ``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text at top level really does run to EOF",
                "```rust\nlet x = 1;\n``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "unclosed fence in a list item ends with the item, not the document",
                "- item\n\n  ```rust\n  let x = 1;\n\n- next\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "a longer closing line closes",
                "- item\n\n  ```rust\n  let x = 1;\n  ````\n\n- next\n",
            ),
            (
                "a tilde closing line does not close a backtick fence",
                "- item\n\n  ```rust\n  let x = 1;\n  ~~~\n\n- next\n",
            ),
            (
                "a shorter closing line does not close a longer fence",
                "- item\n\n  ````rust\n  let x = 1;\n  ```\n\n- next\n",
            ),
            (
                "an alert after a fence whose closing line has trailing text",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\n- next\n\n> [!NOTE]\n> body\n",
            ),
            (
                "a details block after a fence whose closing line has trailing text",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\n- next\n\n<details open>\n<summary>S</summary>\n\nbody\n\n</details>\n",
            ),
            (
                "a heading and a real fence after a fence whose closing line has trailing text",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\n- next\n\n## H\n\n```sh\necho hi\n```\n",
            ),
            (
                "a checkbox after a fence whose closing line has trailing text (list ended by a paragraph)",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\nText.\n\n- [ ] a\n- [x] b\n",
            ),
            (
                "a checkbox after a fence whose closing line has trailing text (tight list)",
                "- item\n  ```rust\n  let x = 1;\n  ``` (note)\n- [ ] a\n- [x] b\n",
            ),
            // Deliberately **not** in this corpus: the same shape with the checkboxes as further
            // items of the *same loose list* (`… ``` (note)\n\n- next\n\n- [ ] a\n`). tui-markdown
            // panics outright on that document — the loose-list-with-a-task-item panic
            // `render_text_block_safe` exists to survive (verified by calling
            // `tui_markdown::from_str_with_options` on it directly: it unwinds) — so what reaches the
            // screen is the bisected degradation, not a parse, and the renderer draws zero
            // checkboxes while the scanner correctly finds two. That mismatch is upstream's, not
            // this gate's: it only refuses the toggle (principle #3), and the two shapes above cover
            // the same axis without conscripting an unrelated upstream bug into the assertion.
            // The mirror direction: an indented code block whose *content* looks like a construct.
            // `fence_mask` cannot see an indented block at all, so `split_tables`/`split_alerts` used
            // to carve the construct out of it — the table's rows left the block as a rendered table
            // and the `> [!NOTE]` opened a real callout, in both cases tearing the code block apart.
            (
                "indented block whose content is a GFM table stays literal code",
                "para\n\n    | a | b |\n    |---|---|\n    | 1 | 2 |\n",
            ),
            (
                "indented block whose content is an alert header stays literal code",
                "para\n\n    > [!NOTE]\n    > body\n",
            ),
            // ---- Indented code whose content is tag-shaped × the context that decides it (2026-08)
            //
            // Root cause: the same "content decides the block" mistake as the two rows just above,
            // for the third splitter — `split_html_blocks` asked `fence_mask`, which has no notion
            // of indentation, so an indented block opening with anything `is_html_block_start`
            // accepts was rescued as an HTML block: tags stripped, gutter gone, nothing left on
            // screen for `    <code>`, and neither Tab-focusable nor copyable with `y c`.
            //
            // The axis is (**what shape is the block's first line**) × (**is this run a whole
            // document, or a fragment `split_block_parts` cut an `<img>` line out of**), and the
            // second half is the reason it is not enough to write the first. The two contexts
            // produce byte-identical fragments — `    </a>\n    <a href="…">` reads as an indented
            // code block on its own and as `<div>`-interior in place — so a gate can always satisfy
            // one column of this table by getting the other wrong, and both single-column fixes
            // were in fact tried. Only a mask taken over the intact document answers both.
            //
            // The first column is enumerated from what `is_html_block_start` accepts (any tag name,
            // so: bare / attributes / closing / void / comment), plus the tags this module splits
            // its own blocks on, plus the two angle-bracket forms that are not tags at all — not
            // from the one tag a user reported.
            //
            // **What these rows can and cannot catch**, stated plainly because it is easy to assume
            // more: this corpus measures the renderer and the scanner *agreeing*, and both of them
            // reach their answer through the same `split_html_blocks`. Every wrong gate tried for
            // this axis was wrong on both sides at once, so the counts stayed equal and these rows
            // stayed green (measured: reverting the fix, in either of the two directions, fails none
            // of them). They are non-regression pins for the failure mode this module keeps having —
            // one side moving without the other, which refuses `y c` for the whole document — while
            // the correctness of the answer itself is carried by
            // `an_indented_code_block_is_code_whatever_its_content_looks_like` and
            // `a_centered_banner_stays_html_even_though_its_cut_fragments_read_as_indented_code`,
            // which assert what is drawn rather than that two counts match.
            (
                "indented block whose content is a bare tag",
                "para\n\n    <code>\n",
            ),
            (
                "indented block whose content is a tag with attributes",
                "para\n\n    <a href=\"x\">\n",
            ),
            (
                "indented block whose content is a closing tag",
                "para\n\n    </a>\n",
            ),
            (
                "indented block whose content is a void tag",
                "para\n\n    <br>\n",
            ),
            (
                "indented block whose content is an HTML comment",
                "para\n\n    <!-- a comment -->\n",
            ),
            (
                "indented block whose content is a <details> tag",
                "para\n\n    <details>\n",
            ),
            (
                "indented block whose content is a <summary> tag",
                "para\n\n    <summary>S</summary>\n",
            ),
            (
                "indented block whose content is a <kbd> pair",
                "para\n\n    <kbd>Ctrl</kbd>\n",
            ),
            (
                "indented block whose content is an autolink",
                "para\n\n    <https://example.com>\n",
            ),
            // Not a tag name, so `is_html_block_start` always rejected it — this is the one row of
            // the first column that was *correct* while every other row was broken. Pinned so a
            // future change cannot quietly make it the only correct one again.
            (
                "indented block whose content is cjk in angle brackets",
                "para\n\n    <仕様書>\n",
            ),
            // Only the block's *first* line is ever asked, so the two orderings are different cases.
            (
                "indented block whose first line is plain and a later line is tag-shaped",
                "para\n\n    plain first\n    <div>\n",
            ),
            (
                "indented block whose content is tag-shaped, followed by a real fence",
                "para\n\n    <kbd>K</kbd>\n\n```rust\nfn a(){}\n```\n",
            ),
            // Both readings in one document: the `<div>` really is an HTML block, the indented
            // `<div>` below it really is code. A gate that answers with the line alone cannot get
            // both, whichever way it leans.
            (
                "a real HTML block and a tag-shaped indented block in one document",
                "<div align=\"center\">\n  <b>hi</b>\n</div>\n\npara\n\n    <div>\n",
            ),
            // The second column: the `<img>` lines are what make this a *cut*, so they are not
            // decoration — without them nothing is removed, the ambiguous fragment never forms, and
            // the row degenerates into an ordinary HTML block.
            (
                "centered banner whose <img> lines are cut out of it stays HTML",
                "<p align=\"center\">\n    <a href=\"https://example.com/y\">\n      <img src=\"https://img.example/a.svg\" alt=\"a\">\n    </a>\n    <a href=\"https://example.com/x\">\n      <img src=\"https://img.example/b.svg\" alt=\"b\">\n    </a>\n</p>\n\ntail\n",
            ),
            (
                "centered banner with a bare <br> line in it (registry: raw-window-metal README.md)",
                "<p align=\"center\">\n    <a href=\"https://crates.io/crates/rwm\">\n      <img src=\"https://img.example/v.svg\" alt=\"crates.io\">\n    </a>\n    <br>\n    <a href=\"LICENSE-MIT\">\n      <img src=\"https://img.example/mit.svg\" alt=\"License - MIT\">\n    </a>\n</p>\n\ntail\n",
            ),
            (
                "centered banner preceded by a markdown block image (registry: static_assertions README.md)",
                "[![Banner](https://img.example/banner.png)](https://example.com/repo)\n\n<div align=\"center\">\n    <a href=\"https://crates.io/crates/sa\">\n        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\n    </a>\n    <img src=\"https://img.example/rustc.svg\" alt=\"rustc\">\n    <br>\n    <a href=\"https://example.com/patron\">\n        <img src=\"https://img.example/patron.png\" alt=\"Patron\">\n    </a>\n</div>\n\ntail.\n",
            ),
            (
                "crlf centered banner with no <br> (registry: tinytemplate README.md)",
                "<h1 align=\"center\">TT</h1>\r\n\r\n<div align=\"center\">\r\n    <a href=\"https://docs.rs/tt/\">API Documentation</a>\r\n    |\r\n    <a href=\"https://example.com/changelog\">Changelog</a>\r\n</div>\r\n\r\n<div align=\"center\">\r\n    <a href=\"https://example.com/actions\">\r\n        <img src=\"https://img.example/ci.svg\" alt=\"CI\">\r\n    </a>\r\n</div>\r\n\r\ntail\r\n",
            ),
            // ---- KNOWN FAILURE, NOT YET FIXED (2026-08): a lazily continued paragraph glues onto a
            // list-item-nested fence's closing line upstream, dropping the code block's header ----
            //
            // Root cause (paragraph side): `flush_code_run` requires a code run's *last* rendered row
            // to be nothing but fence characters (`is_closing_fence`). When a fenced block sits inside
            // a list item and the very next line — no blank line in between, i.e. a CommonMark lazy
            // continuation — is a paragraph whose first inline is a code span, tui-markdown renders
            // that paragraph's first line glued onto the *same row* as the closing fence (e.g. the
            // rendered row reads `` "```inline" `` instead of a bare `` "```" ``). `is_closing_fence`
            // then rejects that row, so the whole run — opener, body, and the glued closer — is handed
            // back completely undecorated (no header is ever drawn for it), while the write-back
            // scanner (`code_block_source_locs`, which asks pulldown-cmark directly and never sees
            // tui-markdown's rendered rows) still counts the block normally. The mismatch cancels `y c`
            // for every code block in the document, not just this one (the count guard is
            // document-wide). A paragraph whose first line is plain text does not glue — its own line
            // stays on its own row — which is the control case right below the failing ones.
            (
                "KNOWN FAILURE: list item fence glued to a following inline-code paragraph (no blank line) drops the header",
                "1. item:\n   ```\n   aaa\n   ```\n   `inline` after\n",
            ),
            (
                "KNOWN FAILURE: same glue, with an unrelated real fence following later in the document",
                "1. item:\n   ```\n   aaa\n   ```\n   `inline` after\n\n```\nbbb\n```\n",
            ),
            (
                "control: list item fence + a plain-text lazy continuation (no blank line) does not glue",
                "1. item:\n   ```\n   aaa\n   ```\n   after\n",
            ),
            (
                "control: list item fence + a blank line before the inline-code paragraph does not glue",
                "1. item:\n   ```\n   aaa\n   ```\n\n   `inline` after\n",
            ),
            (
                "KNOWN FAILURE: same glue in a bullet list item",
                "- item:\n  ```\n  aaa\n  ```\n  `inline` after\n",
            ),
            (
                "control: bullet list item fence + a plain-text lazy continuation does not glue",
                "- item:\n  ```\n  aaa\n  ```\n  after\n",
            ),
            // Not a case of the glue defect after all (checked by hand while writing this corpus,
            // 2026-08): a *plain* block quote wraps every line of its contents in a `"> "` prefix —
            // including the fence's own opening/closing marker lines — so the run's first line never
            // reads `"```"` in the first place; `flush_code_run` already hands that whole shape back
            // undecorated regardless of what follows it (see that function's own doc comment, and the
            // pre-existing corpus entry "a plain (non-alert) block quote wrapping a fence is not
            // detected by either side" a few entries above this one). The write-back scanner agrees
            // (also 0), so this stays green — the two sides matching here says nothing about the glue
            // defect either way, only that this particular shape was already a known non-detection on
            // both sides before this corpus addition.
            (
                "list item fence nested inside a plain block quote, with an inline-code continuation — masked by the pre-existing block-quote non-detection, not the glue defect",
                "> - item:\n>   ```\n>   aaa\n>   ```\n>   `inline` after\n",
            ),
            // Root cause (body side, a related but distinct upstream defect): konoma's own renderer
            // also runs a list-item-nested fence's own **body** lines together onto a single rendered
            // row, faithfully reproducing the same upstream tui-markdown joining that used to be
            // pinned for *indented* blocks by its own dedicated test (removed along with the legacy
            // tui-markdown-based renderer in `479df2c`; see `list_item_fence_body_lines_stay_split_on_screen`
            // below, whose doc comment covers this same joining) — this is the fenced-in-a-list-item
            // case of the same upstream behavior; a top-level fence's body is not affected. This
            // does not change how many blocks are drawn, so the count stays in parity here and this
            // entry alone is expected to stay green; the on-screen row split (or lack of it) is
            // checked directly by `list_item_fence_body_lines_stay_split_on_screen` below.
            (
                "list item fence with a two-line body (count parity only — content is checked elsewhere)",
                "1. item:\n\n   ```\n   aaa\n   bbb\n   ```\n",
            ),
        ]
    }
}

pub mod code_span_corpus {
    /// One document plus what each source-rewriting pass must do to it.
    pub struct Case {
        pub name: String,
        pub src: String,
        /// Expected `process_footnotes` output, byte for byte.
        pub footnotes: String,
        /// Expected `process_inline_html` output, byte for byte.
        pub inline_html: String,
        /// Expected `collect_math_exprs` output, as (latex, is_display).
        pub math: Vec<(String, bool)>,
    }

    /// The notations every pass rewrites, gathered into one payload so a single case exercises all
    /// three passes at once. `<br>` is deliberately absent: it rewrites to a hard line break, which
    /// would restructure the surrounding line and obscure what the case is actually pinning (it gets
    /// its own cases below).
    const PAYLOAD: &str = "[^1] <kbd>K</kbd> $x$";
    /// `PAYLOAD` after `process_footnotes` has numbered the reference.
    const PAYLOAD_FN: &str = "¹ <kbd>K</kbd> $x$";
    /// `PAYLOAD` after `process_inline_html` has converted the tag.
    const PAYLOAD_IH: &str = "[^1] `K` $x$";

    /// A case built around a `block` that must survive **every** pass byte for byte — the block holds
    /// `PAYLOAD` inside an inline code span (or inside a fence). A control line carrying the same
    /// payload *outside* any span follows, so the expectations pin both halves at once: a fix that
    /// merely stopped converting everything fails the control, and a fix that keeps converting inside
    /// code fails the block.
    ///
    /// Every expectation here is plain string concatenation, derived from the case's own inputs and
    /// not from the passes being checked.
    fn verbatim(name: &str, block: &str) -> Case {
        Case {
            name: name.to_string(),
            src: format!("{block}\n\nout {PAYLOAD}\n\n[^1]: note\n"),
            footnotes: format!("{block}\n\nout {PAYLOAD_FN}\n\n\n---\n\n1. note\n"),
            inline_html: format!("{block}\n\nout {PAYLOAD_IH}\n\n[^1]: note\n"),
            math: vec![("x".to_string(), false)],
        }
    }

    /// The counterpart to `verbatim`: `line` looks like it holds a code span but does not (the
    /// backticks are literal), so its payload **is** rewritten, exactly like the control line. Pins
    /// the boundary from the other side — an over-eager "treat any backtick as code" fix fails here.
    fn no_span(name: &str, line: &str, line_fn: &str, line_ih: &str) -> Case {
        Case {
            name: name.to_string(),
            src: format!("{line}\n\nout {PAYLOAD}\n\n[^1]: note\n"),
            footnotes: format!("{line_fn}\n\nout {PAYLOAD_FN}\n\n\n---\n\n1. note\n"),
            inline_html: format!("{line_ih}\n\nout {PAYLOAD_IH}\n\n[^1]: note\n"),
            math: vec![("x".to_string(), false), ("x".to_string(), false)],
        }
    }

    /// A case with hand-written expectations, for the notations and footnote semantics that do not
    /// fit the shared payload.
    fn case(
        name: &str,
        src: &str,
        footnotes: &str,
        inline_html: &str,
        math: &[(&str, bool)],
    ) -> Case {
        Case {
            name: name.to_string(),
            src: src.to_string(),
            footnotes: footnotes.to_string(),
            inline_html: inline_html.to_string(),
            math: math.iter().map(|(l, d)| (l.to_string(), *d)).collect(),
        }
    }

    /// Documents pinning **the** invariant every source-rewriting pass shares: the contents of a
    /// literal code region — an inline `` `code span` ``, **or a code block, fenced or indented** —
    /// are a literal *example* of some syntax, never syntax to act on. Enumerated from the
    /// specification's case splits — backtick run lengths, matched/unmatched delimiters, where the
    /// span sits in the line, backslash escapes, multibyte boundaries, and (for blocks) the kind of
    /// indentation, its column count, and what construct can precede it without a blank line — crossed
    /// with the notations the passes rewrite. Deliberately *not* enumerated from the bugs already
    /// found: a corpus grown from past bugs only ever prevents those exact bugs from recurring, which
    /// is how this rule came to be re-derived (and to drift) once per pass in the first place.
    ///
    /// The block half of this was missing until 2026-08: all three passes originally gated only on
    /// `fence_mask`, which recognizes a fenced (```` ``` ````/`~~~`) block but has no notion of
    /// indentation at all, so an *indented* code block — CommonMark's other kind — was invisible to
    /// them. `[^1]`/`<kbd>x</kbd>`/`$x$` written inside one purely to *document* the syntax (e.g. "indent
    /// your snippet 4 columns like this: ...") was rewritten as if it were real, indistinguishable on
    /// screen from an actual reference/tag/expression elsewhere in the document. The fix folds
    /// `code_block_mask` (which asks pulldown-cmark directly, so it agrees with the parser on indented
    /// blocks in any container) into `fence_mask` via `literal_code_mask` — see that function's own
    /// doc comment for why the union, not either mask alone, is what matches the renderer.
    pub fn cases() -> Vec<Case> {
        let mut v = vec![
            // --- Backtick run length: 1 / 2 / 3, and runs that do not pair up ---
            verbatim("one backtick", &format!("a `{PAYLOAD}` b")),
            verbatim("two backticks", &format!("a ``{PAYLOAD}`` b")),
            verbatim("three backticks", &format!("a ```{PAYLOAD}``` b")),
            verbatim(
                "a span may contain a shorter backtick run",
                &format!("a `` `{PAYLOAD}` `` b"),
            ),
            verbatim(
                "a span may contain a lone backtick",
                &format!("a ``{PAYLOAD} ` tail`` b"),
            ),
            // --- Where the span sits in the line ---
            verbatim("span at line start", &format!("`{PAYLOAD}` tail")),
            verbatim("span at line end", &format!("head `{PAYLOAD}`")),
            verbatim("span is the whole line", &format!("`{PAYLOAD}`")),
            verbatim("two spans on one line", "`[^1]` mid `<kbd>K</kbd>` end"),
            verbatim("three spans on one line", "`[^1]`, `<kbd>K</kbd>`, `$x$`"),
            verbatim(
                "span spanning the line with text either side",
                &format!("x`{PAYLOAD}`y"),
            ),
            // --- Multibyte safety right at the delimiters ---
            verbatim("cjk around a span", &format!("日本 `{PAYLOAD}` 語")),
            verbatim(
                "cjk immediately against the delimiters",
                &format!("日本`{PAYLOAD}`語"),
            ),
            verbatim(
                "emoji immediately against the delimiters",
                &format!("🎉`{PAYLOAD}`🎉"),
            ),
            verbatim("cjk inside the span", &format!("a `日本{PAYLOAD}語` b")),
            verbatim(
                "escaped multibyte char before a span",
                &format!("a \\あ `{PAYLOAD}` b"),
            ),
            verbatim(
                "escaped emoji before a span",
                &format!("a \\🎉 `{PAYLOAD}` b"),
            ),
            // --- Fences: already protected before this fix; must stay protected ---
            verbatim("backtick fence", &format!("```\n{PAYLOAD}\n```")),
            verbatim("tilde fence", &format!("~~~\n{PAYLOAD}\n~~~")),
            verbatim("fence with a language", &format!("```rust\n{PAYLOAD}\n```")),
            verbatim(
                "fence holding an unclosed backtick",
                &format!("```\n{PAYLOAD} ` alone\n```"),
            ),
            verbatim(
                "span on the line after a fence",
                &format!("```\ncode\n```\n\nafter `{PAYLOAD}` end"),
            ),
            // --- Literal backticks that do NOT open a span (the boundary from the other side) ---
            no_span(
                "opener longer than closer is not a span",
                &format!("a ``{PAYLOAD}` b"),
                &format!("a ``{PAYLOAD_FN}` b"),
                &format!("a ``{PAYLOAD_IH}` b"),
            ),
            no_span(
                "closer longer than opener is not a span",
                &format!("a `{PAYLOAD}`` b"),
                &format!("a `{PAYLOAD_FN}`` b"),
                &format!("a `{PAYLOAD_IH}`` b"),
            ),
            no_span(
                "unclosed backtick",
                &format!("a `{PAYLOAD} b"),
                &format!("a `{PAYLOAD_FN} b"),
                &format!("a `{PAYLOAD_IH} b"),
            ),
            no_span(
                "escaped backtick does not open a span",
                &format!("a \\`{PAYLOAD}` b"),
                &format!("a \\`{PAYLOAD_FN}` b"),
                &format!("a \\`{PAYLOAD_IH}` b"),
            ),
            no_span(
                "two adjacent backticks with no closer",
                &format!("a `` {PAYLOAD} b"),
                &format!("a `` {PAYLOAD_FN} b"),
                &format!("a `` {PAYLOAD_IH} b"),
            ),
            no_span(
                "no backticks at all (the control for every case above)",
                &format!("a {PAYLOAD} b"),
                &format!("a {PAYLOAD_FN} b"),
                &format!("a {PAYLOAD_IH} b"),
            ),
        ];
        // An escaped *backslash* leaves the following backtick free to open a real span — the exact
        // counterpart of "escaped backtick does not open a span" above.
        v.push(verbatim(
            "escaped backslash then a real span",
            &format!("a \\\\`{PAYLOAD}` b"),
        ));
        v.extend([
            // --- One case per notation the passes rewrite, inside and outside a span ---
            case(
                "del inside and outside",
                "`<del>d</del>` and <del>d</del>\n",
                "`<del>d</del>` and <del>d</del>\n",
                "`<del>d</del>` and ~~d~~\n",
                &[],
            ),
            case(
                "s inside and outside",
                "`<s>d</s>` and <s>d</s>\n",
                "`<s>d</s>` and <s>d</s>\n",
                "`<s>d</s>` and ~~d~~\n",
                &[],
            ),
            case(
                "strike inside and outside",
                "`<strike>d</strike>` and <strike>d</strike>\n",
                "`<strike>d</strike>` and <strike>d</strike>\n",
                "`<strike>d</strike>` and ~~d~~\n",
                &[],
            ),
            case(
                "sup inside and outside",
                "`<sup>2</sup>` and <sup>2</sup>\n",
                "`<sup>2</sup>` and <sup>2</sup>\n",
                "`<sup>2</sup>` and ²\n",
                &[],
            ),
            case(
                "sub inside and outside",
                "`<sub>2</sub>` and <sub>2</sub>\n",
                "`<sub>2</sub>` and <sub>2</sub>\n",
                "`<sub>2</sub>` and ₂\n",
                &[],
            ),
            // `<br>` inside a span used to inject a newline *into* the span, splitting it in half.
            case(
                "br inside and outside",
                "`<br>` and <br> tail\n",
                "`<br>` and <br> tail\n",
                "`<br>` and   \n tail\n",
                &[],
            ),
            case(
                "br self-closing inside and outside",
                "`<br />` and <br /> tail\n",
                "`<br />` and <br /> tail\n",
                "`<br />` and   \n tail\n",
                &[],
            ),
            case(
                "uppercase br inside and outside",
                "`<BR>` and <BR> tail\n",
                "`<BR>` and <BR> tail\n",
                "`<BR>` and   \n tail\n",
                &[],
            ),
            case(
                "display math inside and outside",
                "`$$x$$` and $$y$$\n",
                "`$$x$$` and $$y$$\n",
                "`$$x$$` and $$y$$\n",
                &[("y", true)],
            ),
            case(
                "paren math inside and outside",
                "`\\(x\\)` and \\(y\\)\n",
                "`\\(x\\)` and \\(y\\)\n",
                "`\\(x\\)` and \\(y\\)\n",
                &[("y", false)],
            ),
            case(
                "bracket math inside and outside",
                "`\\[x\\]` and \\[y\\]\n",
                "`\\[x\\]` and \\[y\\]\n",
                "`\\[x\\]` and \\[y\\]\n",
                &[("y", true)],
            ),
            // A tag pair may legitimately *wrap* a code span. Taken verbatim from palette 0.7.7's
            // README, which is how this case was found at all: the first attempt at this fix split
            // the line at each span, which hid `</strike>` from `<strike>` and left both tags raw.
            // The span's contents stay literal; the strikethrough around it still happens.
            case(
                "a tag pair straddling a code span (palette README)",
                "x <strike>Enables `named::from_str`, which maps names.</strike>\n",
                "x <strike>Enables `named::from_str`, which maps names.</strike>\n",
                "x ~~Enables `named::from_str`, which maps names.~~\n",
                &[],
            ),
            case(
                "a tag pair straddling two code spans",
                "<del>a `b` c `d` e</del>\n",
                "<del>a `b` c `d` e</del>\n",
                "~~a `b` c `d` e~~\n",
                &[],
            ),
            case(
                "a kbd pair straddling a code span",
                "<kbd>press `X` now</kbd>\n",
                "<kbd>press `X` now</kbd>\n",
                "`press `X` now`\n",
                &[],
            ),
            // --- Footnote-specific semantics ---
            // A reference that exists only inside a span is not a reference at all, so the document
            // has no *referenced* definitions and the whole pass stays a no-op — no numbering, no
            // appended section, and the definition line stays where the author put it.
            case(
                "the only reference is inside a span",
                "just `[^1]` here\n\n[^1]: note\n",
                "just `[^1]` here\n\n[^1]: note\n",
                "just `[^1]` here\n\n[^1]: note\n",
                &[],
            ),
            case(
                "reference inside a span on one line, outside on the next",
                "`[^1]`\n\nreal [^1]\n\n[^1]: note\n",
                "`[^1]`\n\nreal ¹\n\n\n---\n\n1. note\n",
                "`[^1]`\n\nreal [^1]\n\n[^1]: note\n",
                &[],
            ),
            // Numbering follows first appearance *outside* spans: `[^b]` inside the span must not
            // claim number 1, which would also reorder the appended section.
            case(
                "numbering ignores references inside spans",
                "`[^b]` then [^a] then [^b]\n\n[^a]: A\n[^b]: B\n",
                "`[^b]` then ¹ then ²\n\n\n---\n\n1. A\n2. B\n",
                "`[^b]` then [^a] then [^b]\n\n[^a]: A\n[^b]: B\n",
                &[],
            ),
            case(
                "undefined reference stays literal inside and outside",
                "`[^9]` and [^9] and [^1]\n\n[^1]: note\n",
                "`[^9]` and [^9] and ¹\n\n\n---\n\n1. note\n",
                "`[^9]` and [^9] and [^1]\n\n[^1]: note\n",
                &[],
            ),
            // --- Degenerate inputs ---
            case("empty document", "", "", "", &[]),
            case(
                "span holding only spaces",
                "a `   ` b [^1]\n\n[^1]: note\n",
                "a `   ` b ¹\n\n\n---\n\n1. note\n",
                "a `   ` b [^1]\n\n[^1]: note\n",
                &[],
            ),
            case(
                "line that is nothing but backticks",
                "````\n",
                "````\n",
                "````\n",
                &[],
            ),
            case(
                "trailing backslash at end of line",
                "a [^1] \\\n\n[^1]: note\n",
                "a ¹ \\\n\n\n---\n\n1. note\n",
                "a [^1] \\\n\n[^1]: note\n",
                &[],
            ),
        ]);
        // --- Indented code blocks: CommonMark's *other* kind of code block (fenced ``` / ~~~ is
        // covered above). `literal_code_mask` only started covering these with the `code_block_mask`
        // union (2026-08) — before that, all three passes gated on `fence_mask` alone, which is blind
        // to indentation entirely, so `[^1]`/`<kbd>`/`$x$` written inside one purely to *document* the
        // syntax (e.g. "indent your snippet 4 columns like this: ...") were rewritten as if they were
        // real. Columns are built with `.repeat` rather than counted by eye in a string literal, since
        // an off-by-one here would silently test the wrong boundary.
        let ind3 = " ".repeat(3);
        let ind4 = " ".repeat(4);
        let ind5 = " ".repeat(5);
        let ind6 = " ".repeat(6);
        let ind8 = " ".repeat(8);
        v.extend([
            // The case explicitly requested: a 4-column indent at document start.
            verbatim("indented code block", &format!("    {PAYLOAD}")),
            // A tab is 4 columns (CommonMark's tab-stop rule, `leading_ws_width`'s own doc comment) —
            // a different *kind* of indented block, not just a different width of the same one.
            verbatim(
                "tab-indented code block",
                &format!("\t{PAYLOAD}"),
            ),
            // The boundary from the other side: one column short of 4 is still an ordinary paragraph,
            // so the payload on it *is* rewritten, exactly like the control line — `no_span`, not
            // `verbatim`.
            no_span(
                "indented three columns is not a code block (ordinary paragraph)",
                &format!("{ind3}{PAYLOAD}"),
                &format!("{ind3}{PAYLOAD_FN}"),
                &format!("{ind3}{PAYLOAD_IH}"),
            ),
            // Past 4 columns, the extra indentation is literal *content* of the same one block, not a
            // second, more-indented block.
            verbatim(
                "indented eight columns is still one literal code block",
                &format!("{ind8}{PAYLOAD}"),
            ),
            // --- Position: what can precede an indented code block without a blank line ---
            verbatim(
                "indented code block after a paragraph",
                &format!("para\n\n{ind4}{PAYLOAD}"),
            ),
            // A heading is not a paragraph, so — unlike the paragraph case just above, which *needs*
            // the blank line — it cannot absorb a following indented line as a continuation. No blank
            // line is needed for the indented line right after it to start a fresh code block.
            verbatim(
                "indented code block right after a heading, no blank line",
                &format!("# H\n{ind4}{PAYLOAD}"),
            ),
            // Inside a list item the indentation is relative to the item's own content column: a
            // "- " marker's content starts at column 2, so a code block inside it needs marker + 4 =
            // six columns, and (like the top-level paragraph case) a blank line first — an indented
            // line right after the item's opening text is just a continuation of that paragraph.
            verbatim(
                "indented code block inside a list item, six columns, blank line before it",
                &format!("- item\n\n{ind6}{PAYLOAD}"),
            ),
            // The boundary from the other side: four columns under the same marker is short of the
            // six the item needs, so it stays a (lazily-indented) continuation of "- item"'s paragraph
            // — not a code block — and the payload on it is rewritten normally.
            no_span(
                "indented four columns inside a list item is not a code block (paragraph continuation)",
                &format!("- item\n{ind4}{PAYLOAD}"),
                &format!("- item\n{ind4}{PAYLOAD_FN}"),
                &format!("- item\n{ind4}{PAYLOAD_IH}"),
            ),
            // Inside a blockquote the same rule applies relative to the quote's own content column
            // (`>` plus one optional space = column 1): here the indented block is the very first
            // line of the quote, so — like the document-start case — no blank line is needed first.
            verbatim(
                "indented code block inside a blockquote",
                &format!(">{ind5}{PAYLOAD}"),
            ),
            // Two indented chunks separated by one blank line are a single block in CommonMark (the
            // blank line is folded into the block's own range, not treated as ending it) — both
            // chunks, and the blank line between them, must stay untouched.
            verbatim(
                "two indented chunks separated by a blank line stay one literal block",
                &format!("{ind4}{PAYLOAD}\n\n{ind4}{PAYLOAD}"),
            ),
            // --- Notations besides PAYLOAD's own (footnote ref / <kbd> / inline math), each inside an
            // indented block vs. outside one. None of these documents define a footnote, so
            // `process_footnotes` short-circuits to a byte-identical no-op for all nine — that itself
            // is part of what's being pinned (a stray indented block must never manufacture a
            // reference/definition that was not there).
            case(
                "del inside an indented code block and outside",
                "    <del>d</del>\n\n<del>d</del>\n",
                "    <del>d</del>\n\n<del>d</del>\n",
                "    <del>d</del>\n\n~~d~~\n",
                &[],
            ),
            case(
                "s inside an indented code block and outside",
                "    <s>d</s>\n\n<s>d</s>\n",
                "    <s>d</s>\n\n<s>d</s>\n",
                "    <s>d</s>\n\n~~d~~\n",
                &[],
            ),
            case(
                "strike inside an indented code block and outside",
                "    <strike>d</strike>\n\n<strike>d</strike>\n",
                "    <strike>d</strike>\n\n<strike>d</strike>\n",
                "    <strike>d</strike>\n\n~~d~~\n",
                &[],
            ),
            case(
                "sup inside an indented code block and outside",
                "    <sup>2</sup>\n\n<sup>2</sup>\n",
                "    <sup>2</sup>\n\n<sup>2</sup>\n",
                "    <sup>2</sup>\n\n²\n",
                &[],
            ),
            case(
                "sub inside an indented code block and outside",
                "    <sub>2</sub>\n\n<sub>2</sub>\n",
                "    <sub>2</sub>\n\n<sub>2</sub>\n",
                "    <sub>2</sub>\n\n₂\n",
                &[],
            ),
            // `<br>` inside an indented block used to inject a newline *into* the block, splitting it
            // in two (the same failure mode as the inline-code-span "br inside and outside" case
            // above, for the other kind of code block).
            case(
                "br inside an indented code block and outside",
                "    <br>\n\n<br> tail\n",
                "    <br>\n\n<br> tail\n",
                "    <br>\n\n  \n tail\n",
                &[],
            ),
            case(
                "display math ($$...$$) inside an indented code block and outside",
                "    $$x$$\n\n$$y$$\n",
                "    $$x$$\n\n$$y$$\n",
                "    $$x$$\n\n$$y$$\n",
                &[("y", true)],
            ),
            case(
                "paren math (\\(...\\)) inside an indented code block and outside",
                "    \\(x\\)\n\n\\(y\\)\n",
                "    \\(x\\)\n\n\\(y\\)\n",
                "    \\(x\\)\n\n\\(y\\)\n",
                &[("y", false)],
            ),
            case(
                "bracket math (\\[...\\]) inside an indented code block and outside",
                "    \\[x\\]\n\n\\[y\\]\n",
                "    \\[x\\]\n\n\\[y\\]\n",
                "    \\[x\\]\n\n\\[y\\]\n",
                &[("y", true)],
            ),
            // ---- Math containing a backslash-escaped ASCII punctuation character (`` \, ``/`` \_ ``/
            // `` \% ``, CommonMark's own generic escape syntax for *any* ASCII punctuation, not a
            // notation this file's own math extension owns — so this is ordinary, unremarkable LaTeX,
            // `\,` a thin space and `\_`/`\%` literal underscore/percent). `collect_math_exprs` (this
            // corpus's own `math` field, checked byte for byte by `code_spans_are_literal_in_every_
            // source_pass`) works from raw `src` directly and has always handled these correctly —
            // `scan_inline_math`'s own escape handling (`b'\\' if i + 1 < n`) keeps the backslash
            // literally in its accumulation buffer regardless of what character follows it. The
            // *rendering* side (`render.rs`'s `render_doc`, an independent pulldown-cmark-event-driven
            // pipeline `md_render_diff_tests`'s own `markdown_render_diff_report` compares against this
            // exact corpus) does not share that same robustness for free: pulldown-cmark's own escape
            // handling splits an `Event::Text` at *every* backslash-escape point (confirmed directly —
            // see `render.rs`'s own `render_dollar_math_tail`/`backslash_math_opener` doc comments for
            // the exact byte ranges), so a math span with an escape inside it never appears whole within
            // any *single* event's own payload — a real, confirmed bug this corpus exists to pin against
            // regressing back to (`render_dollar_math_tail`/`render_backslash_math`'s own raw-`src`-slice
            // fix; before it, every one of these cases rendered as unlifted literal text for `$…$`/
            // `$$…$$`, or lost the escape's own backslash entirely for `\(…\)`/`\[…\]`).
            case(
                "escaped comma inside display math ($$a\\,b$$)",
                "$$a\\,b$$\n",
                "$$a\\,b$$\n",
                "$$a\\,b$$\n",
                &[("a\\,b", true)],
            ),
            case(
                "escaped underscore inside display math ($$a\\_b$$)",
                "$$a\\_b$$\n",
                "$$a\\_b$$\n",
                "$$a\\_b$$\n",
                &[("a\\_b", true)],
            ),
            case(
                "escaped percent inside inline math ($a\\%b$)",
                "$a\\%b$\n",
                "$a\\%b$\n",
                "$a\\%b$\n",
                &[("a\\%b", false)],
            ),
            case(
                "escaped comma inside backslash-paren inline math (\\(a\\,b\\))",
                "\\(a\\,b\\)\n",
                "\\(a\\,b\\)\n",
                "\\(a\\,b\\)\n",
                &[("a\\,b", false)],
            ),
            case(
                "escaped comma inside backslash-bracket display math (\\[a\\,b\\])",
                "\\[a\\,b\\]\n",
                "\\[a\\,b\\]\n",
                "\\[a\\,b\\]\n",
                &[("a\\,b", true)],
            ),
            // The escape is the *very first* thing inside the delimiters — the opening `$$` and the
            // escape's own backslash are adjacent, with nothing else between them (so the pulldown-cmark
            // event boundary the escape causes sits right at the content's own start, not somewhere
            // partway through it).
            case(
                "escaped comma at the very start of display math content ($$\\,ab$$)",
                "$$\\,ab$$\n",
                "$$\\,ab$$\n",
                "$$\\,ab$$\n",
                &[("\\,ab", true)],
            ),
            // The mirror image: the escape is the *very last* thing before the closing `$$`.
            case(
                "escaped comma right before the closing delimiter of display math ($$ab\\,$$)",
                "$$ab\\,$$\n",
                "$$ab\\,$$\n",
                "$$ab\\,$$\n",
                &[("ab\\,", true)],
            ),
            // CJK either side of the escape — same multibyte-safety concern
            // `code_span_corpus`'s own "cjk"/"escaped multibyte" cases already pin for the *opener*/
            // *closer* detection itself (`utf8_len`-based advancing, never a fixed byte count).
            case(
                "escaped comma between cjk characters inside display math ($$あ\\,い$$)",
                "$$あ\\,い$$\n",
                "$$あ\\,い$$\n",
                "$$あ\\,い$$\n",
                &[("あ\\,い", true)],
            ),
            // Negative: the identical escaped-math-looking text sitting inside an inline code span must
            // stay literal, never lifted — mirrors `verbatim`'s own "protected inside a span, still
            // converts on a control line right outside it" shape, but for an escape-bearing expression
            // specifically (a plain, escape-free `$x$` already has this property pinned by every
            // `verbatim`/`no_span` case above; this one confirms the *escaped* shape does not somehow
            // leak through the code-span boundary via the very mechanism that lets it survive an
            // ordinary `Event::Text` boundary split).
            case(
                "escaped math inside an inline code span stays literal; the same shape outside is lifted",
                "`$$a\\,b$$` and $$c\\,d$$\n",
                "`$$a\\,b$$` and $$c\\,d$$\n",
                "`$$a\\,b$$` and $$c\\,d$$\n",
                &[("c\\,d", true)],
            ),
            // Robustness, not required by any known bug: a *fully* escaped `\$` (CommonMark's own
            // generic escape of the dollar sign itself, `code_span_corpus`'s sibling `inline_corpus`
            // module already pins as "not math" for `collect_math_exprs`) still leaves a literal `$`
            // character inside `scan_inline_math`'s own accumulation buffer (the backslash is kept, not
            // stripped — see `scan_inline_math`'s own doc comment) — which is exactly the signal
            // `render.rs`'s own `dollar_math_still_open` probe reacts to, even though nothing here is
            // *actually* an unresolved opener at all. Combined with unrelated escaped asterisks
            // (`\*not italic\*`) elsewhere in the very same paragraph — pulled into the same
            // escape-joined chain `render_dollar_math_tail`'s own growth loop follows — this is the
            // shape most likely to regress into showing a stray literal backslash if that growth loop
            // ever renders its own "give up" fallback from the *raw* (backslash-preserved) slice instead
            // of replaying the original, escape-processed events (see that function's own doc comment,
            // "give up", for exactly why it must not).
            case(
                "escaped currency dollars mixed with an unrelated escape elsewhere render literally, not as math",
                "cost \\$5 and \\$10 for \\*not italic\\*.\n",
                "cost \\$5 and \\$10 for \\*not italic\\*.\n",
                "cost \\$5 and \\$10 for \\*not italic\\*.\n",
                &[],
            ),
            // A real, confirmed panic found while fixing the escape-splitting bug above (not itself an
            // escape case at all — `dollar_math_still_open`'s own trigger condition is deliberately
            // broad enough to fire for *any* never-closing `$`, escape or not): `render_dollar_math_tail`
            // used to give up the instant a non-continuing event turned up, which crashed the moment
            // that event was a `Start(tag)` (bold/italic/link, any of them) whose own `range` spans
            // *past* the individual nested events still left to consume — see that function's own "Never
            // stop mid-construct" doc comment for the exact mechanism and byte ranges. Three shapes:
            // ordinary nesting, nesting with an interior line break, and a link.
            case(
                "a never-closing dollar sign followed by unrelated bold markup does not crash",
                "It costs $50 **bold** text.\n",
                "It costs $50 **bold** text.\n",
                "It costs $50 **bold** text.\n",
                &[],
            ),
            case(
                "a never-closing dollar sign followed by markup containing a line break does not crash",
                "It costs $50 *italic\nbreak* more.\n",
                "It costs $50 *italic\nbreak* more.\n",
                "It costs $50 *italic\nbreak* more.\n",
                &[],
            ),
            case(
                "a never-closing dollar sign followed by an unrelated link does not crash",
                "It costs $50 [a link](url) more.\n",
                "It costs $50 [a link](url) more.\n",
                "It costs $50 [a link](url) more.\n",
                &[],
            ),
            // The flip side of the panic fix: a dollar-math span whose real closer sits past an
            // intervening inline code span, or past nested markup, now resolves correctly too — matching
            // production's own raw-source `collect_math_exprs`, which has always treated such markup as
            // ordinary literal characters (it runs before tui-markdown ever interprets any of it).
            case(
                "escaped math resolves across an intervening inline code span",
                "$$a\\,b `x` c\\_d$$\n",
                "$$a\\,b `x` c\\_d$$\n",
                "$$a\\,b `x` c\\_d$$\n",
                &[("a\\,b `x` c\\_d", true)],
            ),
            case(
                "escaped math resolves across intervening nested markup",
                "$$a\\,b **c** d\\_e$$\n",
                "$$a\\,b **c** d\\_e$$\n",
                "$$a\\,b **c** d\\_e$$\n",
                &[("a\\,b **c** d\\_e", true)],
            ),
            // The other reason `literal_code_mask` exists at all (see its own doc comment): a fence
            // with no blank line between it and a preceding `<summary>…</summary>` line is one thing
            // `code_block_mask` alone gets *wrong* (it reads the whole thing as one HTML block) and
            // `fence_mask` gets right by accident — so this is the one corpus case where the union's
            // *other* half is doing the protecting, not `code_block_mask`. Already covered by two
            // individual tests (`process_inline_html_leaves_a_details_fence_without_a_blank_line_untouched`,
            // `process_footnotes_leaves_a_details_fence_without_a_blank_line_untouched`) and one
            // hand-written math case below in `fence_and_math_extraction_tests` — added here too so it
            // also runs through the corpus's two mechanically-derived tests (span survival, math
            // opacity), which those individual tests don't exercise.
            verbatim(
                "details/summary immediately followed by a fenced code block, no blank line",
                &format!("<details>\n<summary>s</summary>\n```rust\n{PAYLOAD}\n```\n</details>"),
            ),
        ]);
        // --- What the block's *first line looks like* (2026-08). The enumeration above varies the
        // indentation and what precedes the block; it never varies the block's own content, and the
        // shape of the first line turned out to decide whether the block was treated as a code block
        // at all: `split_html_blocks` asked `fence_mask`, which is blind to indentation, so an
        // indented block opening with anything `is_html_block_start` accepts (any tag name) was
        // rescued as an HTML block instead — tags stripped, gutter gone, and its contents no longer
        // literal. That gate is the renderer's rather than these three passes', so what these rows
        // pin is the half this corpus owns: whatever the opening line looks like, the *contents* of
        // the block stay bytes. A future fix that leans on the first line's shape in a pass as well
        // as in the splitter fails here.
        v.extend([
            verbatim(
                "indented code block opening with a rewritable tag",
                &format!("{ind4}<kbd>K</kbd>\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with a closing tag",
                &format!("{ind4}</a>\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with an HTML comment",
                &format!("{ind4}<!-- c -->\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with a tag with attributes",
                &format!("{ind4}<a href=\"x\">\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with an autolink",
                &format!("{ind4}<https://example.com>\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with cjk in angle brackets",
                &format!("{ind4}<仕様書>\n{ind4}{PAYLOAD}"),
            ),
            // The other 2026-08 fix, at the byte level nothing else measures it: inside an HTML
            // block a `<br>` line that would come out blank is **removed**, because a blank line
            // ends the block and no substitution — not even a single space — avoids that. The
            // banner's other lines have to come back byte-identical, so this pins the removal is
            // exactly one line wide, and `process_footnotes` (which runs first in production, and
            // is what relocates a surviving tag somewhere it can do damage) is a no-op here.
            case(
                "bare br line inside a centered banner is removed, the rest byte-identical",
                "<p align=\"center\">\n    <a href=\"https://a\">\n    <br>\n    <a href=\"https://b\">\n</p>\n",
                "<p align=\"center\">\n    <a href=\"https://a\">\n    <br>\n    <a href=\"https://b\">\n</p>\n",
                "<p align=\"center\">\n    <a href=\"https://a\">\n    <a href=\"https://b\">\n</p>\n",
                &[],
            ),
            // The boundary from the other side, so "remove the line" cannot become "remove the tag
            // everywhere": with text on both sides the rewrite leaves two non-blank lines, which an
            // HTML block tolerates, so it is an ordinary hard break — which is also what keeps an
            // `<img>` sharing a line with a `<br>` on a line of its own for the block-image pass.
            case(
                "mid-line br inside a centered banner is still a hard break",
                "<div align=\"center\">\n  before<br>after\n</div>\n",
                "<div align=\"center\">\n  before<br>after\n</div>\n",
                "<div align=\"center\">\n  before  \nafter\n</div>\n",
                &[],
            ),
        ]);
        v
    }
}

pub mod preprocess_corpus {
    /// Documents exercising the **source pre-passes** — `process_footnotes` and
    /// `process_inline_html` — and their interaction with every construct the renderer treats
    /// specially. Sibling of `task_corpus` / `code_corpus`, and built the same way: from the case
    /// split the *specification* implies, not from the list of bugs already met.
    ///
    /// Why this axis needed its own corpus (2026-08, reported by a user on the released build): the
    /// existing corpora hand one text to the renderer and the same text to the scanner, so they can
    /// only ever catch the two disagreeing *about identical input*. Production does not do that. The
    /// renderer draws the text these pre-passes produce while the scanners read the file, and every
    /// disagreement about **which text** is being described is invisible to a same-text parity test
    /// by construction. `rand_chacha`'s README — a footnote definition wrapped over two lines — was
    /// refused by `y c` in the wild for exactly that reason, and no test could have seen it.
    ///
    /// The axes are: definition shape (single line / continued at 2, 4 and 6 columns / lazy / several
    /// paragraphs / carrying a fence, a list or a checkbox), reference shape (defined, undefined,
    /// duplicated, inside a code span), the tags `process_inline_html` rewrites (`<br>` and its
    /// spellings — the only one that adds a line — plus `<kbd>`, `<del>`, `<s>`, `<strike>`, `<sup>`,
    /// `<sub>`), where each sits relative to a code block or a checkbox, the containers (list, nested
    /// list, quote, alert, `<details>` open and closed), and the text-level axes that have bitten
    /// this module before: front matter, CRLF, a missing trailing newline, and CJK — including CJK
    /// inside angle brackets, which is what the reporting user's own payload contained.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            // ---- footnote definition shapes ----
            ("def single line", "Ref[^a].\n\n[^a]: one line.\n"),
            ("def continued 2 cols", "Ref[^a].\n\n[^a]: first\n  second\n"),
            ("def continued 4 cols", "Ref[^a].\n\n[^a]: first\n    second\n"),
            // The exact shape rand_chacha/rand_xorshift use: a link split across two lines, the
            // continuation indented past the indented-code threshold.
            (
                "def continued 6 cols with a wrapped link",
                "Ref[^a].\n\n[^a]: [label](\n      https://example.com/x)\n",
            ),
            ("def lazy continuation", "Ref[^a].\n\n[^a]: first\nsecond\n"),
            (
                "def with two paragraphs",
                "Ref[^a].\n\n[^a]: first\n\n    second para\n",
            ),
            (
                "def carrying a fence",
                "Ref[^a].\n\n[^a]: see\n\n    ```rust\n    fn a() {}\n    ```\n",
            ),
            (
                "def carrying a list",
                "Ref[^a].\n\n[^a]: see\n\n    - one\n    - two\n",
            ),
            (
                "def carrying a checkbox",
                "Ref[^a].\n\n[^a]: see\n\n    - [ ] inside the note\n",
            ),
            (
                "two defs both continued",
                "A[^a] and B[^b].\n\n[^a]: first\n      more\n\n[^b]: second\n      more\n",
            ),
            ("def never referenced", "No reference here.\n\n[^a]: orphan\n"),
            ("ref with no def", "Dangling[^zz] reference.\n"),
            ("same ref twice", "One[^a] two[^a].\n\n[^a]: shared\n"),
            (
                "ref inside a code span stays literal",
                "Text `[^a]` literal.\n\n[^a]: def\n",
            ),
            (
                "def inside a fence stays literal",
                "```\n[^a]: not a def\n```\n\nRef[^a].\n",
            ),
            (
                "def inside indented code stays literal",
                "para\n\n    [^a]: not a def\n\nRef[^a].\n",
            ),
            // ---- footnotes next to a code block ----
            (
                "continued def before a fence",
                "Ref[^a].\n\n[^a]: first\n      more\n\n```\ncode\n```\n",
            ),
            (
                "continued def after a fence",
                "```\ncode\n```\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "continued def before indented code",
                "Ref[^a].\n\n[^a]: first\n      more\n\npara\n\n    indented\n",
            ),
            (
                "continued def between two fences",
                "```\none\n```\n\nRef[^a].\n\n[^a]: first\n      more\n\n~~~\ntwo\n~~~\n",
            ),
            // ---- inline HTML: <br> (the only tag that adds a line) ----
            ("br plain", "before<br>after\n"),
            ("br slash", "before<br/>after\n"),
            ("br spaced slash", "before<br />after\n"),
            ("br uppercase", "before<BR>after\n"),
            ("br making a list interrupt a paragraph", "prose<br>- [ ] injected\n"),
            ("br before a fence", "prose<br>more\n\n```\ncode\n```\n"),
            ("br after a fence", "```\ncode\n```\n\nprose<br>more\n"),
            ("br inside a fence is literal", "```\na<br>b\n```\n"),
            ("br inside indented code is literal", "para\n\n    a<br>b\n"),
            ("br inside a code span is literal", "text `a<br>b` more\n"),
            ("br on a checkbox line", "- [ ] task<br>tail\n"),
            // ---- inline HTML: the paired tags ----
            ("kbd", "Press <kbd>Ctrl</kbd> now.\n"),
            ("kbd inside a code span is literal", "Write `<kbd>Ctrl</kbd>` so.\n"),
            ("del", "This <del>was</del> that.\n"),
            ("s tag", "This <s>was</s> that.\n"),
            ("strike tag", "This <strike>was</strike> that.\n"),
            ("sup", "x<sup>2</sup>\n"),
            ("sub", "H<sub>2</sub>O\n"),
            ("empty del makes tildes", "a<del></del>b\n"),
            // `<del></del>` alone on a line rewrites to `~~~~`, which *is* a tilde fence opener: the
            // preprocessed text has a code block the file does not. The mirror image of the footnote
            // phantom, and the reason the copy scanner has to read the renderer's text rather than
            // the file even once multi-line definitions are handled correctly.
            ("empty del alone becomes a fence opener", "before\n\n<del></del>\n\nafter\n"),
            // A `<br>` inside an alert injects a line with no `>` prefix, which ends the callout
            // early: the fence below it is drawn as literal text, so the screen has *fewer* code
            // blocks than the file does.
            (
                "br ending an alert early above a fence",
                "> [!NOTE]\n> Some prose with <br> inline text.\n>\n> ```\n> code line one\n> ```\n",
            ),
            ("kbd on a checkbox line", "- [ ] press <kbd>x</kbd>\n"),
            ("kbd before a fence", "<kbd>x</kbd>\n\n```\ncode\n```\n"),
            ("paired tag inside a fence is literal", "```\n<kbd>x</kbd>\n```\n"),
            // ---- footnote reference on a checkbox line (the tail gets rewritten) ----
            ("checkbox carrying a ref", "- [ ] task[^a]\n\n[^a]: note\n"),
            (
                "checkbox carrying a ref and a continued def",
                "- [ ] task[^a]\n\n[^a]: note\n      more\n",
            ),
            // ---- containers ----
            (
                "continued def inside a list item",
                "- item\n\n  Ref[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "continued def inside a nested list item",
                "- outer\n  - inner Ref[^a].\n\n[^a]: first\n      more\n",
            ),
            ("br inside a list item", "- item<br>tail\n"),
            ("br inside a nested list item", "- outer\n  - inner<br>tail\n"),
            ("br inside a quote", "> quoted<br>tail\n"),
            ("br inside an alert", "> [!NOTE]\n> quoted<br>tail\n"),
            // A table row has no continuation form, so the injected line became a second, shifted
            // row. `| x<br>y | z |` is GitHub's own idiom for a line break in a cell.
            (
                "br inside a table cell",
                "| a | b |\n| --- | --- |\n| x<br>y | z |\n",
            ),
            // The centered-banner idiom: a bare `<br>` line between two badge blocks. Rewritten, it
            // left a whitespace-only line — a blank line to CommonMark — which ended the HTML block
            // and turned every following indented line into a real indented code block.
            (
                "bare br line inside an html block",
                "<p align=\"center\">\n  <a href=\"https://a\">\n    <img src=\"https://a.svg\" alt=\"a\">\n  </a>\n  <br>\n  <a href=\"https://b\">\n    <img src=\"https://b.svg\" alt=\"b\">\n  </a>\n</p>\n",
            ),
            // The other half of the same axis: a `<br>` with text on both sides inside an HTML
            // block leaves two non-blank lines, which the block tolerates, so it is still rewritten.
            (
                "mid-line br inside an html block",
                "<div align=\"center\">\n  before<br>after\n</div>\n",
            ),
            (
                "continued def inside a quote",
                "> Ref[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "alert with a fence and a continued def after it",
                "> [!NOTE]\n> ```\n> code\n> ```\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "open details with a fence and a continued def",
                "<details open>\n<summary>S</summary>\n\n```\ncode\n```\n\n</details>\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "closed details with a fence and a continued def",
                "<details>\n<summary>S</summary>\n\n```\ncode\n```\n\n</details>\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            ("br inside an open details", "<details open>\n<summary>S</summary>\n\nprose<br>tail\n\n</details>\n"),
            // An indented code block as the *first* content of an open `<details>` body: the body is
            // peeled out of the tags before being re-parsed, and trimming that fragment used to eat
            // the four columns that make it code at all.
            (
                "open details whose body starts with indented code",
                "<details open>\n<summary>S</summary>\n\n    indented line\n\n</details>\n",
            ),
            (
                "open details with prose before indented code",
                "<details open>\n<summary>S</summary>\n\nprose first.\n\n    indented line\n\n</details>\n",
            ),
            (
                "checkbox in an alert plus a continued def",
                "> [!NOTE]\n> - [ ] task\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            // ---- text-level axes ----
            (
                "front matter then a continued def",
                "---\ntitle: t\n---\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "front matter then br",
                "---\ntitle: t\n---\n\nprose<br>tail\n",
            ),
            ("crlf with a continued def", "Ref[^a].\r\n\r\n[^a]: first\r\n      more\r\n"),
            ("crlf with br", "prose<br>tail\r\n"),
            ("no trailing newline continued def", "Ref[^a].\n\n[^a]: first\n      more"),
            ("no trailing newline br", "prose<br>tail"),
            // ---- CJK, including inside angle brackets (the reporting user's own payload) ----
            ("cjk continued def", "参照[^a]。\n\n[^a]: 最初の行\n      続きの行\n"),
            ("cjk br", "前<br>後\n"),
            ("cjk checkbox with a ref", "- [ ] やること[^a]\n\n[^a]: 注記\n"),
            (
                "angle brackets holding cjk next to a fence",
                "Use <仕様書> and <資料dir>.\n\n```\n/groundwork <仕様書> <NotionURL> <資料dir>\n```\n",
            ),
            (
                "angle brackets holding cjk with a continued def",
                "Use <仕様書>[^a].\n\n[^a]: <NotionURL> の説明\n      続き\n",
            ),
            (
                "cjk fence plus br plus continued def",
                "前<br>後\n\n```\n/groundwork <仕様書> <NotionURL> <資料dir>\n```\n\n参照[^a]。\n\n[^a]: 注記\n      続き\n",
            ),
            // ---- the four real registry files, reduced to their essence ----
            (
                "rand_chacha readme shape",
                "ChaCha[^1], used as an RNG.\n\nselected by eSTREAM[^2].\n\nLinks:\n\n-   [API](https://docs.rs/rand_chacha)\n\n[rand]: https://crates.io/crates/rand\n[^1]: D. J. Bernstein, [*ChaCha*](\n      https://cr.yp.to/chacha.html)\n\n[^2]: [eSTREAM](\n      http://www.ecrypt.eu.org/stream/)\n\n\n## Crate Features\n",
            ),
            (
                "shlex quoting_warning shape",
                "Text[^1] with a fence.\n\n```sh\necho hi\n```\n\n[^1]: A note that wraps\n      onto a second line.\n",
            ),
            // A second corruption shape, and the one that survives the multi-line-footnote fix: the
            // `~~~~` that `<del></del>` becomes swallows the checkbox below it into a code block, so
            // the file has a checkbox the screen does not draw, while the `<br>` above gives the
            // screen one the file does not have. The two cancel in any count, and both are unchecked,
            // so a count-and-state guard writes to the swallowed line.
            (
                "br injected checkbox above a del-fence swallowed one",
                "prose<br>- [ ] INJECTED\n\n<del></del>\n\n- [ ] SWALLOWED\n",
            ),
            // ---- the confirmed corruption document (see md_tasks.rs) ----
            (
                "footnote leftover plus br injected checkbox",
                "[^1]: def text\n    - [ ] LEFTOVER\n\n- [ ] REAL\n\nprose<br>- [ ] INJECTED\n\nSee[^1].\n",
            ),
            // ---- A pre-pass rewrite and the block splitters, over the same lines (2026-08) ----
            //
            // The axis the rows above stop just short of: what a pre-pass does to a line *and* what
            // the block splitters then make of the line it leaves behind. Both 2026-08 fixes live
            // here and they pull in opposite directions on the same document — the `<br>` rule has
            // to keep an HTML block from ending, and the block splitter then has to keep calling
            // the 4-column-indented markup inside it HTML rather than indented code. A row that
            // exercises only one of them can be satisfied by a change that breaks the other, which
            // is how each of the two single-sided fixes came to look correct.
            //
            // Cheap to state, and the reason these belong in *this* corpus rather than only in
            // `code_corpus`: `code_corpus` hands the raw text to both sides, so it never sees a
            // line the pre-pass removed. Only the app-faithful runners over this corpus do.
            (
                "centered banner with a bare br line (registry: raw-window-metal README.md)",
                "<p align=\"center\">\n    <a href=\"https://crates.io/crates/rwm\">\n      <img src=\"https://img.example/v.svg\" alt=\"crates.io\">\n    </a>\n    <br>\n    <a href=\"LICENSE-MIT\">\n      <img src=\"https://img.example/mit.svg\" alt=\"License - MIT\">\n    </a>\n</p>\n\ntail\n",
            ),
            (
                "markdown block image above a centered banner (registry: static_assertions README.md)",
                "[![Banner](https://img.example/banner.png)](https://example.com/repo)\n\n<div align=\"center\">\n    <a href=\"https://crates.io/crates/sa\">\n        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\n    </a>\n    <img src=\"https://img.example/rustc.svg\" alt=\"rustc\">\n    <br>\n    <a href=\"https://example.com/patron\">\n        <img src=\"https://img.example/patron.png\" alt=\"Patron\">\n    </a>\n</div>\n\ntail.\n",
            ),
            // No `<br>` anywhere, and CRLF throughout: isolates the block-splitter half of the pair
            // on the axis (`\r` left on a line's end) that changes what "only tags" and "blank" both
            // answer.
            (
                "crlf centered banner with no br (registry: tinytemplate README.md)",
                "<h1 align=\"center\">TT</h1>\r\n\r\n<div align=\"center\">\r\n    <a href=\"https://example.com/actions\">\r\n        <img src=\"https://img.example/ci.svg\" alt=\"CI\">\r\n    </a>\r\n    <a href=\"https://crates.io/crates/tt\">\r\n        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\r\n    </a>\r\n</div>\r\n\r\ntail\r\n",
            ),
            // The mirror direction, and the shape a user reported: the pre-pass must leave a
            // tag-shaped indented code line alone *and* the renderer must still draw it as code —
            // with the identical tag outside the block, which the pre-pass must still rewrite. One
            // row per angle-bracket form the two sides disagreed about.
            (
                "tag-shaped indented code beside the same tag outside it",
                "para\n\n    <kbd>Ctrl</kbd>\n\nPress <kbd>Ctrl</kbd> now.\n",
            ),
            (
                "closing-tag indented code beside a real html block",
                "<div align=\"center\">\n  <b>hi</b>\n</div>\n\npara\n\n    </a>\n",
            ),
            (
                "comment-shaped indented code beside a real comment block",
                "<!-- real block -->\n\npara\n\n    <!-- a comment -->\n",
            ),
            (
                "autolink-shaped indented code beside a real autolink",
                "para\n\n    <https://example.com>\n\nSee <https://example.com> too.\n",
            ),
            (
                "cjk angle brackets inside indented code and outside it",
                "para\n\n    <仕様書>\n\n本文で <仕様書> に触れる。\n",
            ),
            (
                "void-tag indented code beside a real br",
                "para\n\n    <br>\n\nprose<br>tail\n",
            ),
        ]
    }
}

pub mod html_table_corpus {
    /// Documents exercising the **HTML `<table>`** surface — the one shape of raw HTML the block
    /// model gives real structure to (`model::BlockKind::HtmlTable`), added with that feature.
    /// Sibling of `task_corpus`/`code_corpus`/`inline_corpus`/`list_corpus`, and built the same way:
    /// from the case split HTML's own table grammar implies, not from a list of bugs already met —
    /// see `preprocess_corpus`'s own doc comment for why that distinction matters.
    ///
    /// Why this axis needed its own corpus: before it, **no** corpus and no `samples/*.md` file
    /// contained a single `<table>`, `<tr>` or `<td>` (confirmed by grep over both at the time this
    /// was written), so every golden snapshot could have passed with the whole feature broken.
    ///
    /// Several cases below deliberately pin a **degraded** rendering rather than a correct one — a
    /// `colspan`/`rowspan` drawn as one plain cell, a nested `<table>` flattened into its enclosing
    /// cell's text, a `<caption>` dropped, a table interrupted by a blank line not recognized as a
    /// table at all. Those are the documented non-goals of the first pass (see
    /// `model::BlockKind::HtmlTable`'s own doc comment and `docs/STATUS.md`); pinning them here is
    /// what makes a later pass that fixes one show up as a reviewable snapshot diff instead of
    /// passing silently.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            // ---- the shape this feature was written for ----
            (
                "readme screenshot grid two by two",
                "## Screenshots\n\n<table>\n  <tr>\n    <td width=\"50%\"><img src=\"tree.png\" alt=\"Tree view with git status colors\"></td>\n    <td width=\"50%\"><img src=\"graph.png\" alt=\"Custom git commit-graph renderer\"></td>\n  </tr>\n  <tr>\n    <td align=\"center\"><b>Tree view</b> — git status colors</td>\n    <td align=\"center\"><b>Git graph</b> — custom commit-graph renderer</td>\n  </tr>\n</table>\n\nafter\n",
            ),
            // ---- header cells: present, absent, and in every position ----
            (
                "th header row",
                "<table>\n<tr><th>Name</th><th>Qty</th></tr>\n<tr><td>apple</td><td>3</td></tr>\n</table>\n",
            ),
            (
                "no th at all",
                "<table>\n<tr><td>apple</td><td>3</td></tr>\n<tr><td>pear</td><td>4</td></tr>\n</table>\n",
            ),
            (
                // Every row is a header row, so the header rule lands under the last one, right above
                // the bottom border. Shared with the GFM path (a `| a |` table with a delimiter row
                // and no body rows draws the identical double rule), pinned here rather than changed.
                "a table of nothing but th rows",
                "<table>\n<tr><th>H</th></tr>\n</table>\n",
            ),
            (
                "two leading th rows are both header rows",
                "<table>\n<tr><th>Group</th><th>Group</th></tr>\n<tr><th>Name</th><th>Qty</th></tr>\n<tr><td>apple</td><td>3</td></tr>\n</table>\n",
            ),
            (
                "th as the leading column of every row",
                "<table>\n<tr><th>Name</th><td>apple</td></tr>\n<tr><th>Qty</th><td>3</td></tr>\n</table>\n",
            ),
            (
                "a stray th in the middle of a body row",
                "<table>\n<tr><td>a</td><th>b</th><td>c</td></tr>\n</table>\n",
            ),
            (
                "first row mixing th and td is not a header row",
                "<table>\n<tr><th>Name</th><td>Qty</td></tr>\n<tr><td>apple</td><td>3</td></tr>\n</table>\n",
            ),
            // ---- align: the three values, an unknown one, and none ----
            (
                "align left center right on one row",
                "<table>\n<tr><td align=\"left\">L</td><td align=\"center\">C</td><td align=\"right\">R</td></tr>\n<tr><td>wide cell one</td><td>wide cell two</td><td>wide cell three</td></tr>\n</table>\n",
            ),
            (
                "align with an unrecognized value falls back to the column",
                "<table>\n<tr><td align=\"justify\">J</td></tr>\n<tr><td>wide cell here</td></tr>\n</table>\n",
            ),
            (
                "align spelled in uppercase",
                "<table>\n<tr><td ALIGN=\"CENTER\">C</td></tr>\n<tr><td>wide cell here</td></tr>\n</table>\n",
            ),
            // ---- cell content: images, markdown, inline html, br ----
            (
                "cell with an img",
                "<table>\n<tr><td><img src=\"a.png\" alt=\"picture a\"></td><td>text</td></tr>\n</table>\n",
            ),
            (
                "cell with a self closing img",
                "<table>\n<tr><td><img src='a.png' alt='picture a'/></td></tr>\n</table>\n",
            ),
            (
                "cell with an img wrapped in a link",
                "<table>\n<tr><td><a href=\"https://example.com\"><img src=\"a.png\" alt=\"badge\"></a></td></tr>\n</table>\n",
            ),
            (
                "cell with markdown emphasis",
                "<table>\n<tr><td>**bold** and *italic* and `code`</td></tr>\n</table>\n",
            ),
            (
                "cell with a markdown link",
                "<table>\n<tr><td>[label](https://example.com)</td></tr>\n</table>\n",
            ),
            (
                "cell with html b i code",
                "<table>\n<tr><td><b>bold</b> <i>italic</i> <code>code</code></td></tr>\n</table>\n",
            ),
            (
                "cell with html strong em",
                "<table>\n<tr><td><strong>bold</strong> <em>italic</em></td></tr>\n</table>\n",
            ),
            (
                "cell with an anchor",
                "<table>\n<tr><td><a href=\"https://example.com\">site</a></td></tr>\n</table>\n",
            ),
            (
                "cell with an anchor that has no href",
                "<table>\n<tr><td><a name=\"x\">anchor</a></td></tr>\n</table>\n",
            ),
            (
                "cell with an unclosed anchor",
                "<table>\n<tr><td><a href=\"https://example.com\">site</td></tr>\n</table>\n",
            ),
            (
                "cell with a br",
                "<table>\n<tr><td>first<br>second</td></tr>\n</table>\n",
            ),
            (
                // These four are rewritten *before* any parse, by `process_inline_html_traced` —
                // the document-wide pre-pass — so this case is what proves that pass and the table
                // fold compose (the snapshot harness runs the real `pre_src_for`, pre-pass included).
                "cell with the tags the inline-html pre-pass owns",
                "<table>\n<tr><td><del>gone</del> <kbd>Ctrl</kbd> H<sub>2</sub>O x<sup>2</sup></td></tr>\n</table>\n",
            ),
            (
                "cell with an html entity",
                "<table>\n<tr><td>a &amp; b &lt;c&gt;</td></tr>\n</table>\n",
            ),
            (
                "cell with an html comment",
                "<table>\n<tr><td>before<!-- hidden -->after</td></tr>\n</table>\n",
            ),
            (
                "cell with an unknown tag",
                "<table>\n<tr><td><span class=\"x\">text</span></td></tr>\n</table>\n",
            ),
            (
                "cell with a literal pipe",
                "<table>\n<tr><td>a | b</td></tr>\n</table>\n",
            ),
            ("cjk cells", "<table>\n<tr><th>名前</th><th>数</th></tr>\n<tr><td>りんご</td><td>三</td></tr>\n</table>\n"),
            (
                "cell content spread over several physical lines",
                "<table>\n<tr>\n<td>\nfirst line\nsecond line\n</td>\n</tr>\n</table>\n",
            ),
            // ---- shape edges: ragged, empty, wrappers, case, quoting ----
            (
                "ragged rows",
                "<table>\n<tr><td>a</td><td>b</td><td>c</td></tr>\n<tr><td>d</td></tr>\n</table>\n",
            ),
            ("empty cells", "<table>\n<tr><td></td><td>b</td></tr>\n</table>\n"),
            // ---- rows with no cell in them: markup left behind when the cells were cut ----
            //
            // Nothing here existed before: no corpus case and no `samples/*.md` file held a `<tr>`
            // with nothing in it, so `parse_html_table`'s own "drop empty rows" step was pinned by
            // no golden at all. `model::tests::an_empty_tr_never_becomes_a_row` states the same rule
            // on the parse side; these keep the *drawn* result honest.
            (
                "an empty row before a real one",
                "<table>\n<tr></tr>\n<tr><td>a</td><td>b</td></tr>\n</table>\n",
            ),
            (
                "an empty row between the header row and the body",
                "<table>\n<tr><th>H</th></tr>\n<tr></tr>\n<tr><td>a</td></tr>\n</table>\n",
            ),
            (
                "a table of nothing but empty rows stays plain html",
                "<table>\n<tr></tr><tr></tr>\n</table>\n",
            ),
            (
                "empty table with no cells stays plain html",
                "<table>\n</table>\n",
            ),
            (
                "table with only a caption stays plain html",
                "<table>\n<caption>Just a caption</caption>\n</table>\n",
            ),
            (
                "caption alongside real rows keeps the table unfolded",
                "<table>\n<caption>LOOSE-caption</caption>\n<tr><td>apple</td></tr>\n</table>\n",
            ),
            // ---- text outside every cell: nothing in a folded table can carry it ----
            //
            // `HtmlTable` holds cell ranges and nothing else, so folding a table with characters
            // outside its cells would delete them from the screen — the same silent loss the
            // trailing-content rule already refuses. Every case here marks that text `LOOSE-…`,
            // which `app::md_snapshot_tests::no_corpus_case_ever_drops_text_outside_a_table_cell`
            // fails on if it does *not* reach a drawn span, so a shape added later is checked for
            // free. The `model` side of the same axis is pinned by
            // `model::tests::text_outside_every_cell_keeps_the_table_unfolded`.
            (
                "a stray line straight inside the table and the row",
                "<table>\n  LOOSE-before-row\n  <tr>\n    LOOSE-in-row\n    <td>cell</td>\n  </tr>\n</table>\n",
            ),
            (
                "prose between one cell and the next",
                "<table>\n<tr><td>a</td>LOOSE-between<td>b</td></tr>\n</table>\n",
            ),
            (
                "a stray line inside a thead wrapper",
                "<table>\n<thead>\nLOOSE-in-thead\n<tr><th>H</th></tr></thead>\n<tbody><tr><td>a</td></tr></tbody>\n</table>\n",
            ),
            (
                "loose text inside a quoted table",
                "> <table>\n> LOOSE-quoted\n> <tr><td>a</td></tr>\n> </table>\n",
            ),
            (
                // The control for the four above: whitespace is the layout of every real table's
                // markup, never content, so this one still folds into a drawn grid.
                "whitespace and newlines outside the cells still fold",
                "<table>\n  <tr>\n    <td>a</td>\n    <td>b</td>\n  </tr>\n</table>\n",
            ),
            (
                // The second control: a comment is text the author removed, so it cannot be lost by
                // folding — this table folds, and the marker is never drawn.
                "a comment outside the cells still folds",
                "<table>\n<!-- SECRET-outside -->\n<tr><td>keep</td></tr>\n</table>\n",
            ),
            (
                "thead tbody tfoot wrappers",
                "<table>\n<thead><tr><th>H</th></tr></thead>\n<tbody><tr><td>B</td></tr></tbody>\n<tfoot><tr><td>F</td></tr></tfoot>\n</table>\n",
            ),
            (
                "colgroup and col",
                "<table>\n<colgroup><col><col></colgroup>\n<tr><td>a</td><td>b</td></tr>\n</table>\n",
            ),
            (
                "uppercase tags",
                "<TABLE>\n<TR><TH>H</TH></TR>\n<TR><TD>b</TD></TR>\n</TABLE>\n",
            ),
            (
                "single quoted attributes",
                "<table>\n<tr><td align='center'>C</td></tr>\n<tr><td>wide cell here</td></tr>\n</table>\n",
            ),
            (
                "unquoted attribute value",
                "<table>\n<tr><td align=center>C</td></tr>\n<tr><td>wide cell here</td></tr>\n</table>\n",
            ),
            (
                "omitted closing td and tr tags",
                "<table>\n<tr><td>a<td>b\n<tr><td>c<td>d\n</table>\n",
            ),
            (
                "td outside any tr",
                "<table>\n<td>a</td><td>b</td>\n</table>\n",
            ),
            // ---- documented non-goals, pinned as they degrade ----
            (
                "colspan is drawn as one plain cell",
                "<table>\n<tr><td colspan=\"2\">spanning</td></tr>\n<tr><td>a</td><td>b</td></tr>\n</table>\n",
            ),
            (
                "rowspan is drawn as one plain cell",
                "<table>\n<tr><td rowspan=\"2\">spanning</td><td>a</td></tr>\n<tr><td>b</td></tr>\n</table>\n",
            ),
            (
                "nested table is flattened into its enclosing cell",
                "<table>\n<tr><td><table><tr><td>inner</td></tr></table></td><td>outer</td></tr>\n</table>\n",
            ),
            (
                "blank line inside the table is not a table",
                "<table>\n<tr>\n\n<td>a</td>\n</tr>\n</table>\n",
            ),
            (
                "unclosed table is not a table",
                "<table>\n<tr><td>a</td></tr>\n",
            ),
            (
                "table not the first tag in the block is not a table",
                "<div>\n<table>\n<tr><td>a</td></tr>\n</table>\n</div>\n",
            ),
            // ---- surroundings: quote, details, list, neighbouring blocks ----
            (
                "table inside a block quote",
                "> <table>\n> <tr><td>a</td><td>b</td></tr>\n> </table>\n",
            ),
            (
                "table inside a block quote with a multi line cell",
                "> <table>\n> <tr>\n> <td>\n> first line\n> second line\n> </td>\n> </tr>\n> </table>\n",
            ),
            (
                "table inside details",
                "<details>\n<summary>More</summary>\n\n<table>\n<tr><th>H</th></tr>\n<tr><td>b</td></tr>\n</table>\n\n</details>\n",
            ),
            (
                "table inside an open details",
                "<details open>\n<summary>More</summary>\n\n<table>\n<tr><th>H</th></tr>\n<tr><td>b</td></tr>\n</table>\n\n</details>\n",
            ),
            (
                "table inside a list item",
                "- item\n\n  <table>\n  <tr><td>a</td><td>b</td></tr>\n  </table>\n",
            ),
            (
                // CommonMark ends an HTML block at a blank line, so this paragraph is glued onto the
                // *same* block as the table. Nothing here can carry it, so the block declines to fold
                // and renders exactly as it always has — pinning that it is never silently dropped.
                "paragraph glued directly after the table keeps the block unfolded",
                "<table>\n<tr><td>a</td></tr>\n</table>\nafter\n",
            ),
            (
                "paragraph after the table across a blank line",
                "<table>\n<tr><td>a</td></tr>\n</table>\n\nafter\n",
            ),
            (
                "fence directly after the table",
                "<table>\n<tr><td>a</td></tr>\n</table>\n\n```rust\nlet x = 1;\n```\n",
            ),
            (
                "two tables in a row",
                "<table>\n<tr><td>a</td></tr>\n</table>\n\n<table>\n<tr><td>b</td></tr>\n</table>\n",
            ),
            (
                "gfm table with only a header row",
                "| a |\n|---|\n",
            ),
            (
                "gfm table and html table in the same document",
                "| a | b |\n|---|--:|\n| 1 | 2 |\n\n<table>\n<tr><td>c</td><td align=\"right\">d</td></tr>\n</table>\n",
            ),
            // ---- HTML comments: text the author removed, never structure ----
            //
            // Every case here that hides something inside `<!-- … -->` marks it `SECRET-…`, which
            // `app::md_snapshot_tests::no_corpus_case_ever_draws_commented_out_text` fails on if it
            // reaches a drawn span — so a shape added later is checked for free, not only by
            // whoever remembers to read the golden. The disclosure this axis was written for:
            // `parse_html_table` had no comment rule at all, so a commented-out `<tr>` folded into
            // a real row whose byte range no longer held the `<!--` that hid it, and the row was
            // drawn. The `model` side of the same axis is pinned by
            // `model::tests::a_comment_is_content_never_table_structure`.
            (
                "a commented-out row",
                "<table>\n<tr><td>keep</td></tr>\n<!-- <tr><td>SECRET-row</td></tr> -->\n</table>\n",
            ),
            (
                "a commented-out row across several lines",
                "<table>\n<tr><td>keep</td></tr>\n<!--\n<tr><td>SECRET-multiline</td></tr>\n-->\n</table>\n",
            ),
            (
                "a commented-out thead",
                "<table>\n<!--\n<thead><tr><th>SECRET-head</th></tr></thead>\n-->\n<tbody><tr><td>keep</td></tr></tbody>\n</table>\n",
            ),
            (
                "a commented-out td inside a live row",
                "<table>\n<tr><td>keep</td><!-- <td>SECRET-cell</td> --></tr>\n</table>\n",
            ),
            (
                "a comment inline before a cell on the same line",
                "<table>\n<tr><!-- <td>SECRET-inline</td> --><td>keep</td></tr>\n</table>\n",
            ),
            (
                // The comment opens inside the cell and closes past its own `</td>`: one cell, whose
                // renderer — not the table parser — is what drops the commented middle.
                "a comment that opens in a cell and closes past its end tag",
                "<table>\n<tr><td>keep <!-- </td></tr><tr><td>SECRET-crossing --> tail</td></tr>\n</table>\n",
            ),
            (
                "a comment containing the table's own closing tag",
                "<table>\n<tr><td>keep</td></tr>\n<!-- </table> -->\n<tr><td>second</td></tr>\n</table>\n",
            ),
            (
                // Comments do not nest: the first `-->` closes the comment the first `<!--` opened.
                "a comment containing a second comment opener",
                "<table>\n<tr><td>keep</td></tr>\n<!-- <tr><td>SECRET-outer</td></tr> <!-- <tr><td>SECRET-inner</td></tr> -->\n<tr><td>after the comment</td></tr>\n</table>\n",
            ),
            (
                // Never closed: it swallows the `</table>` too, so this is not one complete table
                // and the ordinary `Html` path draws it — dropping everything from `<!--` on.
                "a comment that is never closed",
                "<table>\n<tr><td>keep</td></tr>\n<!-- <tr><td>SECRET-unclosed</td></tr>\n</table>\n",
            ),
            (
                "a table whose only row is commented out",
                "<table>\n<!-- <tr><td>SECRET-only</td></tr> -->\n</table>\n",
            ),
            (
                "an attribute value that looks like a comment opener",
                "<table>\n<tr><td title=\"<!--\">keep</td></tr>\n</table>\n",
            ),
            (
                // A degradation older than the comment rule and untouched by it: the tag scan ends
                // a tag at its first `>`, which here is the one inside the attribute's own `-->`.
                "an attribute value that looks like a whole comment",
                "<table>\n<tr><td title=\"<!-- x -->\">keep</td></tr>\n</table>\n",
            ),
            (
                "a comment indented, with blank space around it",
                "<table>\n  <!--\n    <tr><td>SECRET-spaced</td></tr>\n  -->  \n  <tr><td>keep</td></tr>\n</table>\n",
            ),
            (
                "a commented-out row inside a quoted table",
                "> <table>\n> <tr><td>keep</td></tr>\n> <!-- <tr><td>SECRET-quoted</td></tr> -->\n> </table>\n",
            ),
            (
                // Content glued on after `</table>` keeps the block unfolded, a comment included.
                "a comment glued onto the block after the close",
                "<table>\n<tr><td>keep</td></tr>\n</table>\n<!-- trailing -->\n",
            ),
            (
                "a comment containing a nested table",
                "<table>\n<tr><td>keep</td></tr>\n<!-- <table><tr><td>SECRET-nested</td></tr></table> -->\n<tr><td>second</td></tr>\n</table>\n",
            ),
        ]
    }
}
