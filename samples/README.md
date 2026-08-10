# Sample files

All assets in this directory are **original content created for konoma** — used as
preview/thumbnail demo material and as test fixtures. They contain no third-party work
and are covered by the project's MIT license.

- `sample.png` — the project's key visual: artwork generated for konoma with Google Gemini,
  with the wordmark and tagline composited by the project. No third-party imagery is used.
- `sample.jpg` / `sample.gif` — generated with ImageMagick (gradients, shapes, and an
  animated dot). `sample.gif` is a short original animation.
- `sample.pdf` — a generated three-page text PDF (exercises PDF page navigation).
- `sample.mp4` / `sample-hevc.mp4` / `sample.mkv` — an ffmpeg-generated test pattern (`testsrc2`,
  320x240, 3s); no recorded footage. The same pattern encoded as **H.264** and as **HEVC (H.265,
  Main profile)** in mp4, and as **H.264 in Matroska**, so the two built-in video decoders and the
  two built-in container readers can be compared side by side — and so each has a fixture in the
  test suite that proves a thumbnail comes out with no external tool installed.
- `images.md` — demonstrates **inline block-level images** (Markdown `![]()` and HTML
  `<img>`): the local `sample.png` / `sample.jpg`, plus remote `http(s)://` images
  fetched at runtime (a placeholder raster and a shields.io badge — nothing is
  committed here; they are downloaded into the user's cache on preview).
- `sample.csv` / `sample.tsv` / `data.csv` — hand-written tables that exercise the CSV/TSV
  table preview (rainbow columns, cell navigation, quoted commas, full-width CJK cells, and
  long-value truncation).
- `sample.zip` / `sample.tar.gz` — exercise the archive-listing preview (entry name / size /
  modified date in the same table renderer). Built by simply packing a few of this directory's
  *other* sample files with the system `zip`/`tar` tools (`sample.zip` = `code/hello.rs`,
  `code/app.ts`, `code/calc.c`, `code/main.go`; `sample.tar.gz` = `sample.csv`, `sample.tsv`,
  `japanese.txt`) — no third-party content, same as everything else here.
- `sample.svg`, the text / Markdown files, and everything under `code/` — written by hand
  for konoma.
- The walkthrough demos are **English by default**; the Japanese versions use a `.ja` suffix
  (`markdown.ja.md`, `links.ja.md`, `long-lines.ja.txt`, `tutorial.ja.md`). `japanese.txt` and the
  full-width cells in `sample.csv` stay as CJK demos (they exercise konoma's CJK-width handling).

If you add new sample files, only commit material you have the right to redistribute
under MIT (your own work, CC0, or public domain).
