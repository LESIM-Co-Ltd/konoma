# testdata/office — complex spreadsheets and Word documents for the office-preview tests

Own content only (no third-party files). Test-only: `Cargo.toml` excludes `/testdata` like
`/snapshots`, and the tests skip when a file is absent. Used by
`src/preview/office/tests_complex.rs` and the `e2e_complex_*` tests in `src/e2e_tests.rs`.

| files | what is in them | made by |
|---|---|---|
| `pivot.{xlsx,xls,ods}` | source rows (4 regions x 4 products x 6 months, JA + `&<>`), 4 pivot tables: region>product x month with subtotals/grand total, a second pivot on the same sheet (AVERAGE), a page-filter pivot, a COUNT pivot | LibreOffice (DataPilot) |
| `objects.{xlsx,xls,ods}` | embedded chart, picture, rectangle, text box, comments, conditional format, list validation, HYPERLINK() and a URL field | LibreOffice |
| `layout.{xlsx,xls,ods}` | hidden rows/columns, collapsed outline rows, auto-filter that hides rows, merged cells, multi-line/long text, tab/NBSP/space runs, protected sheet, frozen panes | LibreOffice |
| `formulas.{xlsx,xls,ods}` | cross-sheet refs, named ranges, array formulas (single, multi-cell, MMULT), error values, NOW()/TODAY()/RAND() | LibreOffice |
| `formats.{xlsx,xls,ods}`, `date1904.*` | 43 number formats (JA era, 24h+, fractions, E notation, currencies, red negatives, conditions, `@`, ...) and the 1904 date system | LibreOffice |
| `sheets.{xlsx,xls,ods}`, `empty.*` | 20 sheets with JA/symbol/emoji names, 2 hidden, used range starting at C5, an empty sheet | LibreOffice |
| `encrypted.{xlsx,xls,ods}` | password `konoma` | LibreOffice |
| `widesheet.xlsx` | 301 columns | LibreOffice |
| `special.xlsx`, `newerrors.xlsx` | veryHidden/hidden sheets, Excel Table, chart sheet, rich text, `inlineStr`, `t="str"`, `t="b"`, `t="e"` for every classic error, `_x000B_` escapes, a formula with no cached value; Excel 365 error codes (`#SPILL!`, `#CALC!`, `#FIELD!`, `#GETTING_DATA`) | openpyxl + zip patch |
| `word.docx`, `word-ja.docx`, `word.odt`, `word-ja.odt` | a Word document (English / Japanese) written by LibreOffice, and the same document saved as OpenDocument text (`.odt`; `tests_odt.rs` checks that both convert to the same Markdown): title and headings, bold / italic / strike / underline, literal markup characters, line break, bullet and numbered lists (nested; letters, roman, Japanese `ア` / `①` formats), a table with merged cells, a picture (alt text), footnote + endnote, hyperlinks (external and to a heading), tracked changes (one deletion, one insertion), a comment, header and footer, two formulas (OMML), a text box, a table of contents, a code paragraph, a page break. Read by `tests_docx_robust.rs` and `tests_odt.rs` | LibreOffice (`gen_word.py`) |
| `expected/*.json` | what LibreOffice itself displays for each saved file (`<file>.json`: sheet -> grid of strings; diagnostic ground truth, *not* compared blindly) and `pivot.expected.json`: the pivot sums computed from the source rows (this one the tests do compare against) | generators |

## Regenerate

```
gen/make_all.sh            # needs /Applications/LibreOffice.app and python3 with openpyxl
```

`run_job.sh gen_x.py` runs one generator **inside** a one-shot headless `soffice` (own throw-away
profile under `WORK`, default `../../../../NoCode/.cache/office-complex`, locale forced to en-US;
the window never appears: `--headless --invisible`). It does not use LibreOffice's bundled Python
binary or a UNO socket, because on the machine this was built on that binary is SIGKILLed at
launch. `lo_runner/` is the macro glue; `lo_lib.py` has the UNO helpers.
Regenerating changes the file bytes (timestamps) but not what the tests check. LibreOffice's pivot
output order depends on the locale collation, so the tests do not assume it.

`gen_word.py` is run the same way (`WORK=<scratch> OUT_DIR=<dir> ./run_job.sh gen_word.py`); it writes `word.docx`, `word-ja.docx`, `word.odt` and
`word-ja.odt` into `OUT_DIR` (default: this directory's parent). **No file here was made by Microsoft Word** (there is no Office
on the machine this was built on): Word-specific structures (`numbering.xml` formats and `lvlOverride`, `mc:AlternateContent`,
`w:sdt`, `w:fldChar`, `w:moveFrom` / `w:moveTo`, `vMerge`, VML pictures, broken / cyclic `basedOn`, huge input) are covered by
hand-written XML packages generated inside the tests (`tests_docx*.rs`).

Large files (the 120,000-row `big` workbook) are generated inside the tests, not kept here.
