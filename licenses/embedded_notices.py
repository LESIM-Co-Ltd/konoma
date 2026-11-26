"""Prints the notices of third-party material that dependency crates put into the program.

cargo-about reports the license a crate declares, not the licenses of the C libraries, fonts,
character maps, color profiles, tables, syntax definitions and ported code inside it. The texts
printed here are read from the sources of the crate versions that the released targets link, so
they match the crate list. A missing crate, file or expected statement is an error: it means this
list has to be looked at again.

Used by licenses/third-party-licenses.sh:  python3 -I embedded_notices.py <Cargo.toml>
"""

import json
import os
import re
import struct
import subprocess
import sys
import zlib
from typing import NoReturn

# Keep in step with about.toml and the matrix in .github/workflows/release.yml.
TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu"]
ROOT_PACKAGE = "konoma"


def fail(message) -> NoReturn:
    sys.exit(f"embedded_notices: {message}")


def linked_packages(manifest):
    """{(name, version): (directory, targets)} that konoma links into its released binaries.

    The set comes from `cargo tree`, which resolves features as a build does. The resolve graph of
    `cargo metadata` also names optional dependencies that no build compiles. Proc macros and what
    only they depend on run at build time and are left out.
    """
    found = {}
    for target in TARGETS:
        meta = json.loads(
            subprocess.check_output(
                ["cargo", "metadata", "--locked", "--format-version", "1"]
                + ["--filter-platform", target, "--manifest-path", manifest]
            )
        )
        directories = {
            (p["name"], p["version"]): os.path.dirname(p["manifest_path"]) for p in meta["packages"]
        }
        tree = subprocess.check_output(
            ["cargo", "tree", "--locked", "--manifest-path", manifest, "--package", ROOT_PACKAGE]
            + ["--edges", "normal,no-proc-macro", "--target", target]
            + ["--prefix", "none", "--format", "{p}"],
            text=True,
        )
        keys = set()
        for line in tree.splitlines():
            words = line.split()
            if len(words) >= 2:
                keys.add((words[0], words[1].removeprefix("v")))
        if not any(name == ROOT_PACKAGE for name, _ in keys):
            fail(f"cargo tree does not list {ROOT_PACKAGE} for {target}")
        for key in sorted(keys):
            if key not in directories:
                fail(f"cargo metadata does not know {key[0]} {key[1]}")
            found.setdefault(key, (directories[key], []))[1].append(target)
    return found


PACKAGES = linked_packages(sys.argv[1])


def crates(name):
    found = sorted((v, d, t) for (n, v), (d, t) in PACKAGES.items() if n == name)
    if not found:
        fail(f"crate {name} is no longer linked into any released target")
    return found


def read(path):
    try:
        with open(path, encoding="utf-8-sig") as f:
            text = f.read()
    except OSError as e:
        fail(f"cannot read {path}: {e}")
    if not text.strip():
        fail(f"{path} is empty")
    return text.replace("\r\n", "\n")


def require(name, *files, marker=None):
    """Fails unless every linked version of `name` has `files` (and `marker` in the first one)."""
    for _, root, _ in crates(name):
        for rel in files:
            if not os.path.isfile(os.path.join(root, rel)):
                fail(f"{name}: {rel} is gone")
        if marker and marker not in read(os.path.join(root, files[0])):
            fail(f"{name}: {files[0]} no longer says {marker!r}")


def block(text):
    longest = max((len(m) for m in re.findall(r"`+", text)), default=0)
    fence = "`" * max(3, longest + 1)
    return f"{fence}text\n{text.strip(chr(10)).rstrip()}\n{fence}\n"


def heading(title, name, version, targets, intro):
    only = "" if len(targets) == len(TARGETS) else f" Linked on: {', '.join(targets)}."
    print(f"## {title} (`{name}` {version})\n\n{intro}{only}\n")


def section(title, name, intro, files):
    for version, root, targets in crates(name):
        heading(title, name, version, targets, intro)
        for rel in files:
            print(f"`{rel}`:\n\n{block(read(os.path.join(root, rel)))}")


def comments(path):
    return re.findall(r"/\*.*?\*/", read(path), re.S)


def comment_with(path, marker):
    for comment in comments(path):
        if marker in comment:
            return comment
    fail(f"{path}: no comment mentions {marker!r}")


# ---- libgit2 (C code built into the program by libgit2-sys) ----------------------------------

for version, root, targets in crates("libgit2-sys"):
    heading(
        "libgit2 and the code bundled with it",
        "libgit2-sys",
        version,
        targets,
        "`libgit2-sys` links a system libgit2 of a compatible version when pkg-config finds "
        "one; otherwise it compiles the sources in the crate and links them statically. The "
        "texts below come from those sources and apply to a program built that way. The crate "
        "list above shows only the license of the Rust binding. `libgit2/COPYING` is "
        "reproduced whole, so it also covers parts that are not compiled.",
    )
    for rel in [
        "libgit2/COPYING",
        "libgit2/deps/pcre2/LICENCE.md",
        "libgit2/deps/llhttp/LICENSE-MIT",
    ]:
        print(f"`{rel}`:\n\n{block(read(os.path.join(root, rel)))}")

    print("### xdiff\n\nThe distinct header comments of the files in `libgit2/deps/xdiff/`:\n")
    xdiff = os.path.join(root, "libgit2/deps/xdiff")
    headers = {}
    for name in sorted(os.listdir(xdiff)):
        if not name.endswith((".c", ".h")):
            continue
        found = comments(os.path.join(xdiff, name))
        if not found:
            fail(f"libgit2 xdiff/{name} has no header comment")
        if "the libgit2 contributors" not in found[0]:
            headers.setdefault(found[0], []).append(name)
    for needed in ["GNU Lesser General Public", "Eclipse Distribution License"]:
        if not any(needed in text for text in headers):
            fail(f"libgit2 xdiff: no header mentions {needed!r}; check its licensing again")
    for text, names in headers.items():
        print(f"{', '.join(f'`{n}`' for n in names)}:\n\n{block(text)}")

    print("### Notices inside libgit2 source files that `COPYING` does not carry\n")
    for rel, marker in [
        ("libgit2/src/util/utf8.c", "Public Software Group"),
        ("libgit2/src/util/hashmap.h", "Attractive Chaos"),
        ("libgit2/src/util/tsort.c", "Christopher Swenson"),
        ("libgit2/src/util/util.c", "Regents of the University of California"),
    ]:
        print(f"`{rel}`:\n\n{block(comment_with(os.path.join(root, rel), marker))}")

# ---- fonts, character maps and color profiles of the PDF renderer -----------------------------

require("hayro-interpret", "assets/FoxitSans.pfb", "assets/CGATS001Compat-v2-micro.icc")
section(
    "Foxit fonts and color profiles",
    "hayro-interpret",
    "Embedded stand-ins for the standard PDF fonts (from PDFium) and ICC profiles.",
    ["assets/README.md", "assets/LICENSE_FOXIT", "assets/CGATS_LICENSE.txt"],
)
require("hayro-cmap", "assets/cmaps.brotli")
section("Adobe CMap resources", "hayro-cmap", "Embedded character maps.", ["assets/LICENSE.txt"])
require("hayro-jpeg2000", "assets/ProPhoto-v2-micro.icc", "assets/LAB.icc")
section(
    "ICC color profiles",
    "hayro-jpeg2000",
    "Embedded color profiles.",
    ["assets/README.md", "assets/LICENSE.txt"],
)

# ---- Unicode data ----------------------------------------------------------------------------

section(
    "Unicode data",
    "regex-syntax",
    "Tables generated from the Unicode Character Database.",
    ["src/unicode_tables/LICENSE-UNICODE"],
)
section("Unicode emoji data", "emojis", "Emoji data from Unicode.", ["LICENSE-UNICODE"])

# ---- ring ------------------------------------------------------------------------------------

section(
    "Code from BoringSSL, fiat-crypto and others",
    "ring",
    "Compiled into the crate.",
    ["LICENSE-other-bits", "LICENSE-BoringSSL", "third_party/fiat/LICENSE"],
)

# ---- Wayland ---------------------------------------------------------------------------------

section(
    "Wayland protocol definitions",
    "wayland-protocols",
    "The program contains code generated from these protocol files.",
    ["protocols/COPYING"],
)
for title, name, rel in [
    ("Wayland core protocol", "wayland-client", "wayland.xml"),
    (
        "ext data control protocol",
        "wayland-protocols",
        "protocols/staging/ext-data-control/ext-data-control-v1.xml",
    ),
    (
        "wlr data control protocol",
        "wayland-protocols-wlr",
        "wlr-protocols/unstable/wlr-data-control-unstable-v1.xml",
    ),
]:
    for version, root, targets in crates(name):
        notice = re.search(
            r"<copyright>\n?(.*?)\s*</copyright>", read(os.path.join(root, rel)), re.S
        )
        if not notice or not notice[1].strip():
            fail(f"no <copyright> in {name}/{rel}")
        heading(title, name, version, targets, f"The `<copyright>` element of `{rel}`.")
        print(block(notice[1]))


# ---- syntax definitions and themes -----------------------------------------------------------


# two-face carries the notices of its syntax definitions and themes as a zlib-compressed bincode
# dump of `Acknowledgements { for_syntaxes: Vec<License>, for_themes: Vec<License> }`, with
# `License { ty: enum, text: String, rel_path: PathBuf }`.
def acknowledgements(path):
    try:
        with open(path, "rb") as f:
            data = zlib.decompress(f.read())
    except (OSError, zlib.error) as e:
        fail(f"cannot read {path}: {e}")
    pos = 0

    def take(n):
        nonlocal pos
        if pos + n > len(data):
            fail("unexpected format of the two-face acknowledgements")
        out = data[pos : pos + n]
        pos += n
        return out

    def u64():
        return struct.unpack("<Q", take(8))[0]

    def string():
        return take(u64()).decode("utf-8")

    groups = []
    for _ in range(2):
        entries = []
        for _ in range(u64()):
            take(4)  # license type
            text = string()
            entries.append((string(), text))
        groups.append(entries)
    if pos != len(data) or not all(groups):
        fail("unexpected format of the two-face acknowledgements")
    return groups


require("two-face", "generated/syntaxes-fancy-newlines.bin", "generated/themes.bin")
for version, root, targets in crates("two-face"):
    heading(
        "Syntax definitions and themes",
        "two-face",
        version,
        targets,
        "Embedded syntax definitions and themes, curated by the `bat` project "
        "(<https://github.com/sharkdp/bat>). These are the notices `two-face` ships in "
        "`generated/acknowledgements_full.bin`, which may also cover definitions that only "
        "the Oniguruma build of `two-face` embeds.",
    )
    groups = acknowledgements(os.path.join(root, "generated/acknowledgements_full.bin"))
    for kind, entries in zip(("Syntax", "Theme"), groups):
        for rel_path, text in entries:
            print(f"### {kind}: `{rel_path}`\n\n{block(text)}")

# ---- notices that the crate list above does not carry ----------------------------------------

for version, root, targets in crates("color_quant"):
    heading(
        "NeuQuant",
        "color_quant",
        version,
        targets,
        "The notice of the original algorithm, from a comment in `src/lib.rs`.",
    )
    print(block(comment_with(os.path.join(root, "src/lib.rs"), "Anthony Dekker")))

# Crates whose block in the crate list is a license template without their copyright holders,
# but which ship a file that names them.
for name, rel in [
    ("brotli-decompressor", "LICENSE"),
    ("chrono", "LICENSE.txt"),
    ("exr", "LICENSE.md"),
    ("libm", "LICENSE.txt"),
    ("palette_math", "LICENSE_MIT"),
    ("siphasher", "COPYING"),
]:
    section(
        "Copyright holders",
        name,
        "The crate list above shows a license template; this is the file the crate ships.",
        [rel],
    )

# ---- ported, translated and copied code, and tables generated from other projects ---------------

# Texts that no crate ships are kept unmodified in licenses/upstream/, taken from the original
# project at the version named here.
UPSTREAM = os.path.join(os.path.dirname(os.path.abspath(__file__)), "upstream")
printed_upstream = set()


def evidence(name, paths, marker):
    """[(version, root, file)]: every linked version of `name` must still say `marker`."""
    out = []
    for version, root, _ in crates(name):
        for rel in paths.split("|"):
            path = os.path.join(root, rel)
            if os.path.isfile(path) and marker in read(path):
                out.append((version, root, rel))
                break
        else:
            fail(f"{name} {version}: {paths} no longer says {marker!r}")
    return out


def line_comment(path, marker):
    """The run of `//` lines that contains `marker`."""
    lines = read(path).split("\n")
    for i, line in enumerate(lines):
        if marker in line and line.lstrip().startswith("//"):
            start = end = i
            while start and lines[start - 1].lstrip().startswith("//"):
                start -= 1
            while end + 1 < len(lines) and lines[end + 1].lstrip().startswith("//"):
                end += 1
            return "\n".join(lines[start : end + 1])
    fail(f"{path}: no line comment mentions {marker!r}")


def head_comment(path, marker):
    """The comment lines (with the blank lines between them) that a file starts with."""
    head = []
    for line in read(path).split("\n"):
        if line.strip() and not line.startswith("//"):
            break
        head.append(line)
    text = "\n".join(head).strip()
    if marker not in text:
        fail(f"{path}: its leading comment no longer says {marker!r}")
    return text


EXTRACT = {"c": comment_with, "line": line_comment, "head": head_comment}


def ported(title, intro, based_on, excerpts=(), texts=()):
    """One section.

    based_on: (crate, file, statement in that file)
    excerpts: (crate, file, a key of EXTRACT, text in the comment)
    texts: (file in licenses/upstream/, where it came from)
    """
    print(f"## {title}\n\n{intro}\n\nFound in:\n")
    for name, paths, marker in based_on:
        for version, _, rel in evidence(name, paths, marker):
            print(f"- `{name}` {version}: `{rel}`")
    print()
    shown = set()
    for name, paths, kind, marker in excerpts:
        for _, root, rel in evidence(name, paths, marker):
            text = EXTRACT[kind](os.path.join(root, rel), marker)
            if text not in shown:
                shown.add(text)
                print(f"From `{name}`, `{rel}`:\n\n{block(text)}")
    for filename, source in texts:
        printed_upstream.add(filename)
        print(f"{source}:\n\n{block(read(os.path.join(UPSTREAM, filename)))}")


PDFJS = "Mozilla pdf.js"
ported(
    "Adobe Glyph List",
    "A table that maps glyph names to characters.",
    [
        ("hayro-interpret", "src/font/mod.rs", "via Adobe Glyph List"),
        ("hayro-interpret", "src/font/generated/glyph_names.rs", "phf_map!"),
    ],
    texts=[
        ("agl-glyphlist-header.txt", "Header of `glyphlist.txt`, adobe-type-tools/agl-aglfn 4036a9ca80a6"),
        ("agl-aglfn-LICENSE.md", "`LICENSE.md`, adobe-type-tools/agl-aglfn 4036a9ca80a6"),
    ],
)
ported(
    "Adobe Core 14 font metrics",
    "The glyph widths and the Standard, Symbol and ZapfDingbats encodings of the 14 standard "
    "PDF fonts, converted from Adobe's AFM files into Rust tables (a modified form of the "
    "files). The paragraph is from `MustRead.html`, which accompanies the AFM files in "
    "`assets/font_metrics/` of LaurenzV/hayro; the lines after it are the notices of the files.",
    [
        ("hayro-interpret", "src/font/generated/metrics.rs", "static HELVETICA:"),
        ("hayro-interpret", "src/font/generated/standard.rs", "static STANDARD:"),
        ("hayro-interpret", "src/font/generated/symbol.rs", "static SYMBOL:"),
        ("hayro-interpret", "src/font/generated/zapf_dings.rs", "static ZAPF_DING_BATS:"),
    ],
    texts=[("adobe-core14-afm-notices.txt", "LaurenzV/hayro ea9c81dc70cd")],
)
ported(
    f"Code from {PDFJS}",
    "Ported decryption, decoding and color conversion code.",
    [
        ("hayro-interpret", "src/color/cal_rgb.rs", "mozilla/pdf.js"),
        ("hayro-syntax", "src/crypto/aes.rs", "pdf.js"),
        ("hayro-syntax", "src/crypto/md5.rs", "pdf.js"),
        ("hayro-syntax", "src/crypto/mod.rs", "pdf.js"),
        ("hayro-syntax", "src/crypto/sha256.rs", "pdf.js"),
        ("hayro-syntax", "src/crypto/sha384.rs", "pdf.js"),
        ("hayro-syntax", "src/filter/lzw_flate.rs", "pdf.js"),
        ("hayro-syntax", "src/crypto/sha512.rs", "pdf.js"),
        ("hayro-interpret", "src/color/cal_gray.rs", "mozilla/pdf.js"),
    ],
    texts=[
        ("pdfjs-colorspace.js-header.txt", "Header of `src/core/colorspace.js`, mozilla/pdf.js 06f44916c893"),
        ("pdfjs-crypto.js-header.txt", "Header of `src/core/crypto.js`, mozilla/pdf.js ac7d3717e478"),
        ("pdfjs-flate_stream.js-header.txt", "Header of `src/core/flate_stream.js`, mozilla/pdf.js ac7d3717e478"),
        ("pdfjs-LICENSE", "`LICENSE`, mozilla/pdf.js 06f44916c893"),
    ],
)
ported(
    "Code and tables from Apache PDFBox",
    "Character encoding tables, dictionary key names, page sizes, and font and function "
    "handling code.",
    [
        ("hayro-interpret", "src/font/generated/win_ansi.rs", "apache/pdfbox"),
        ("hayro-interpret", "src/font/generated/mac_roman.rs", "apache/pdfbox"),
        ("hayro-interpret", "src/font/generated/mac_os_roman.rs", "apache/pdfbox"),
        ("hayro-interpret", "src/font/generated/mac_expert.rs", "apache/pdfbox"),
        ("hayro-syntax", "src/object/dict.rs", "PDFBox"),
        ("hayro-syntax", "src/page.rs", "apache/pdfbox"),
        ("hayro-interpret", "src/function/type0.rs", "apache/pdfbox"),
        ("hayro-interpret", "src/font/standard_font.rs", "apache/pdfbox"),
    ],
    texts=[
        ("pdfbox-NOTICE.txt", "`NOTICE.txt`, apache/pdfbox 4438b8fdc67a"),
        ("pdfbox-LICENSE.txt", "`LICENSE.txt`, apache/pdfbox 4438b8fdc67a"),
    ],
)

for version, root, targets in crates("image"):
    require("image", "src/codecs/jpeg/transform.rs", marker="Independent JPEG Group")
    heading(
        "Independent JPEG Group",
        "image",
        version,
        targets,
        "In this program, this software is based in part on the work of the Independent JPEG "
        "Group. The JPEG encoder's forward DCT (`src/codecs/jpeg/transform.rs`) is a Rust "
        "translation of `jfdctint.c` from libjpeg 9a.",
    )
    print(block(comment_with(os.path.join(root, "src/codecs/jpeg/transform.rs"), "Independent JPEG Group")))

ported(
    "Cube root from FreeBSD (Sun Microsystems)",
    "Vectorized cube root adapted from FreeBSD's `s_cbrtf.c`.",
    [("quantette", "src/color_space.rs", "Sun Microsystems")],
    excerpts=[
        ("quantette", "src/color_space.rs", "c", "FreeBSD"),
        ("quantette", "src/color_space.rs", "c", "Ian Lance Taylor"),
        ("quantette", "src/color_space.rs", "c", "Sun Microsystems"),
    ],
)
ported(
    "Cube root from CORE-MATH",
    "The `cbrt` of `libm` is a port. The `LICENSE.txt` of the crate names CORE-MATH but does "
    "not carry the copyright line of this file.",
    [("libm", "src/math/cbrt.rs", "Alexei Sibidanov")],
    texts=[("core-math-cbrt.c-header.txt", "Header of `src/binary64/cbrt/cbrt.c`, core-math 70076f6a1ece")],
)
ported(
    "FreeType",
    "Font hinting code and tables ported from FreeType. FreeType is offered under "
    "the FreeType License or the GPLv2; the FreeType License is reproduced here. The software "
    "is based in part of the work of the FreeType Team (<https://freetype.org>).",
    [
        ("skrifa", "src/outline/autohint/metrics/blues.rs", "freetype"),
        ("skrifa", "src/outline/autohint/topo/segments.rs", "freetype"),
        ("skrifa", "src/outline/cff/hint.rs", "freetype"),
        ("skrifa", "src/outline/glyf/hint/math.rs", "Direct port of <https://gitlab.freedesktop.org/freetype"),
        ("skrifa", "src/outline/glyf/hint/zone.rs", "freetype"),
        ("read-fonts", "src/tables/glyf/bytecode/opcode.rs", "pulled from FreeType"),
    ],
    texts=[
        ("freetype-LICENSE.TXT", "`LICENSE.TXT`, freetype/freetype 68db2bb4a471"),
        ("freetype-FTL.TXT", "`docs/FTL.TXT`, freetype/freetype 68db2bb4a471"),
    ],
)
ported(
    "HarfBuzz and rustybuzz",
    "Ported code.",
    [
        ("skrifa", "src/decycler.rs", "hb_decycler_t"),
        ("ttf-parser", "src/tables/gsub.rs", "port of https://github.com/harfbuzz/rustybuzz"),
        ("ttf-parser", "src/tables/gpos.rs", "port of https://github.com/harfbuzz/rustybuzz"),
        ("ttf-parser", "src/ggg/mod.rs", "port of https://github.com/harfbuzz/rustybuzz"),
    ],
    texts=[
        ("harfbuzz-COPYING", "`COPYING`, harfbuzz/harfbuzz a2ea5d28cb53"),
        ("rustybuzz-LICENSE", "`LICENSE`, harfbuzz/rustybuzz 9faca9674086"),
    ],
)
ported(
    "Code from Skia and tiny-skia",
    "Ported gradient, image sampling and name table code.",
    [
        ("vello_common", "src/encode.rs", "Skia"),
        ("vello_common", "src/encode.rs", "tiny-skia"),
        ("vello_cpu", "src/fine/common/image.rs", "skia"),
        ("skrifa", "src/string.rs", "Borrowed from Skia"),
    ],
    texts=[("skia-LICENSE", "`LICENSE`, google/skia 220fef664978")],
)
for version, root, targets in crates("tiny-skia"):
    heading("tiny-skia", "tiny-skia", version, targets, "The license file of the crate.")
    print(block(read(os.path.join(root, "LICENSE"))))
ported(
    "Code from OpenJPEG",
    "The calculation of the value ranges of CIELab color data, which `hayro-jpeg2000` says it "
    "copied from OpenJPEG.",
    [("hayro-jpeg2000", "src/lib.rs", "Copied from OpenJPEG")],
    texts=[("openjpeg-LICENSE", "`LICENSE`, uclouvain/openjpeg 8314119b067c")],
)
ported(
    "Code from font-rs and stb",
    "Glyph rasterization and JPEG decoding code.",
    [
        ("ab_glyph_rasterizer", "src/raster.rs", "stb_truetype"),
        ("zune-jpeg", "src/bitstream.rs", "stb"),
        ("zune-jpeg", "src/mcu.rs", "derived from stb"),
        ("zune-jpeg", "src/worker.rs", "Borrowed from stb"),
    ],
    excerpts=[("ab_glyph_rasterizer", "src/raster.rs", "line", "Copyright 2015 Google")],
    texts=[("stb-LICENSE", "`LICENSE`, nothings/stb 5c205738c191")],
)
ported(
    "Code from libwebp",
    "Ported alpha blending and color conversion code.",
    [
        ("image-webp", "src/alpha_blending.rs", "libwebp"),
        ("image-webp", "src/yuv.rs", "from the libwebp source"),
    ],
    texts=[("libwebp-COPYING", "`COPYING`, webmproject/libwebp 8534f53960be")],
)
ported(
    "Code from fontTools and lyon",
    "Curve conversion, area moments and SVG arc parsing code.",
    [
        ("kurbo", "src/cubicbez.rs", "fontTools"),
        ("kurbo", "src/moments.rs", "fontTools"),
        ("kurbo", "src/svg.rs", "lyon"),
    ],
    texts=[
        ("fonttools-LICENSE", "`LICENSE`, fonttools/fonttools 3b9a73ff8379"),
        ("lyon-LICENSE-MIT", "`LICENSE-MIT`, nical/lyon c3e869fa3858"),
    ],
)
ported(
    "Blur filters of resvg",
    "`box_blur.rs` is based on fastblur. `iir_blur.rs` is based on Pascal Getreuer's "
    "`gaussianiir`, and the comment below is what `resvg` records about its license.",
    [
        ("resvg", "src/filter/box_blur.rs", "fschutt/fastblur"),
        ("resvg", "src/filter/iir_blur.rs", "gaussianiir"),
    ],
    excerpts=[("resvg", "src/filter/iir_blur.rs", "line", "gaussianiir")],
    texts=[("fastblur-LICENSE-MIT", "`LICENSE-MIT`, fschutt/fastblur 3c3b9db112ad")],
)
ported(
    "Code from glam, DirectXMath and the vector class library",
    "Ported SIMD math.",
    [
        ("wide", "src/f32x4_.rs", "glam"),
        ("wide", "src/f32x4_.rs", "DirectXMath"),
        ("wide", "src/f32x16_.rs", "vectorclass"),
        ("zlib-rs", "src/adler32/avx2.rs", "Agner Fog"),
    ],
    texts=[
        ("glam-LICENSE-MIT", "`LICENSE-MIT`, bitshifter/glam-rs 26f184cbb7e6"),
        ("directxmath-LICENSE", "`LICENSE`, microsoft/DirectXMath 6ac6a3c51d4d"),
        ("vectorclass-LICENSE", "`LICENSE`, vectorclass/version2 667577495efa"),
    ],
)
ported(
    "CRC table from the FLAC reference decoder",
    "A CRC-16 table.",
    [("symphonia-core", "src/checksum/crc16.rs", "reference FLAC decoder")],
    texts=[("flac-COPYING.Xiph", "`COPYING.Xiph`, xiph/flac 59fa3402cb19")],
)
ported(
    "SHA-2 code adapted from Mbed TLS",
    "The AArch64 implementations.",
    [
        ("sha2", "src/sha256/aarch64.rs|src/sha256/aarch64_sha2.rs", "mbedtls"),
        ("sha2", "src/sha512/aarch64.rs|src/sha512/aarch64_sha3.rs", "mbedtls"),
    ],
    texts=[("mbedtls-LICENSE", "`LICENSE`, Mbed-TLS/mbedtls 0e2cc5fdac71")],
)
ported(
    "Adler-32 code from Chromium zlib",
    "The NEON implementation, used on AArch64.",
    [("simd-adler32", "src/imp/neon.rs", "Chromium")],
    texts=[
        ("chromium-adler32_simd.c-header.txt", "Header of `third_party/zlib/adler32_simd.c`, chromium/chromium 6d765ed98aed"),
        ("chromium-LICENSE", "`LICENSE`, chromium/chromium 6d765ed98aed"),
    ],
)
ported(
    "UTF-8 decoder by Bjoern Hoehrmann",
    "The automaton of <https://bjoern.hoehrmann.de/utf-8/decoder/dfa/>.",
    [
        ("bstr", "src/utf8.rs", "hoehrmann"),
        ("regex-lite", "src/utf8.rs", "hoehrmann"),
    ],
    texts=[("hoehrmann-utf8-dfa-LICENSE.txt", "The license section of that page")],
)
ported(
    "Code from font-kit",
    "Font loading code.",
    [("fontdb", "src/lib.rs", "servo/font-kit")],
    texts=[("font-kit-LICENSE-MIT", "`LICENSE-MIT`, servo/font-kit 7da40df62c83")],
)
ported(
    "Code from sublimate",
    "Syntax highlighting code.",
    [("syntect", "src/highlighting/highlighter.rs", "sublimate")],
    texts=[("sublimate-LICENSE-MIT", "`LICENSE-MIT`, defuz/sublimate 6f7541a7cb93")],
)
ported(
    "Code from hyperium/mime",
    "Media type parsing code.",
    [("data-url", "src/mime.rs", "hyperium/mime")],
    texts=[("mime-LICENSE-MIT", "`LICENSE-MIT`, hyperium/mime v0.3.5")],
)
ported(
    "John Nagle's parsing code in chrono",
    "A notice that the `LICENSE.txt` of the crate does not carry.",
    [("chrono", "src/format/parse.rs", "John Nagle")],
    excerpts=[("chrono", "src/format/parse.rs", "line", "John Nagle")],
)
ported(
    "Code from cmark",
    "The handling of blank lines in lists, which `pulldown-cmark` says it lifted almost "
    "verbatim from a change to cmark.",
    [("pulldown-cmark", "src/parse.rs", "commonmark/cmark")],
    texts=[("cmark-COPYING", "`COPYING`, commonmark/cmark 18edfe7e9d9b")],
)
ported(
    "Code from odds",
    "Range handling code.",
    [("fixedbitset", "src/range.rs", "bluss/odds")],
    texts=[("odds-LICENSE-MIT", "`LICENSE-MIT`, bluss/odds f331c3282d5c")],
)
ported(
    "Multiplication from LLVM compiler-rt",
    "A translation of `__mulddi3`.",
    [("rand", "src/distributions/utils.rs|src/distr/utils.rs", "__mulddi3")],
    texts=[("llvm-compiler-rt-LICENSE.TXT", "`compiler-rt/LICENSE.TXT`, llvm/llvm-project d35bb1657ae0")],
)
ported(
    "Float formatting from zmij",
    "A port of Victor Zverovich's implementation.",
    [("zmij", "src/lib.rs", "Zverovich")],
    texts=[("zmij-LICENSE", "`LICENSE`, vitaut/zmij f97cc4f65fc2")],
)
ported(
    "Diff code from diffy",
    "`similar` says that its Myers diff and compaction are based on the work of Brandon "
    "Williams.",
    [
        ("similar", "src/algorithms/myers.rs", "Brandon Williams"),
        ("similar", "src/algorithms/compact.rs", "Brandon Williams"),
    ],
    texts=[("diffy-LICENSE-MIT", "`LICENSE-MIT`, bmwill/diffy eba5e533f3c3")],
)
ported(
    "Layout constant from termaid",
    "A constant that `mermaid-text` says it ported.",
    [("mermaid-text", "src/layout/subgraph.rs", "termaid")],
    texts=[("termaid-LICENSE", "`LICENSE`, fasouto/termaid 5279b503bcc4")],
)
ported(
    "Code from OfficeParser",
    "The VBA project reader of `calamine`.",
    [("calamine", "src/vba.rs", "OfficeParser")],
    texts=[("officeparser-LICENSE", "`LICENSE`, unixfreak0037/officeparser fce88741d3f7")],
)
ported(
    "Code from mp4box.js",
    "`re_mp4` says that some of its code was ported from mp4box.js.",
    [("re_mp4", "README.md", "mp4box.js")],
    texts=[("mp4box.js-LICENSE", "`LICENSE`, gpac/mp4box.js ba75abc9138d")],
)
ported(
    "Code from libaom",
    "Part of the loop restoration filter of `rav1e`.",
    [("rav1e", "src/lrf.rs", "from libaom")],
    texts=[("libaom-LICENSE", "`LICENSE`, aomedia.googlesource.com/aom v3.12.0")],
)

# ring: the license files say that the details are at the top of each C and assembly file.
for version, root, targets in crates("ring"):
    holders = set()
    for top in ("crypto", "include", "third_party"):
        for directory, _, names in os.walk(os.path.join(root, top)):
            for filename in names:
                if filename.endswith((".c", ".h", ".S", ".pl", ".inl")):
                    for line in read(os.path.join(directory, filename)).split("\n"):
                        found = re.match(r"\s*(?:/\*|\*|//|#|;)*\s*(Copyright\b.*?)\s*(?:\*/)?\s*$", line)
                        if found:
                            holders.add(found[1])
    if not any("OpenSSL" in h for h in holders):
        fail("ring: the copyright lines of the C sources were not found")
    heading(
        "Copyright lines of ring's C and assembly sources",
        "ring",
        version,
        targets,
        "The distinct copyright lines in `crypto/`, `include/` and `third_party/`.",
    )
    print(block("\n".join(sorted(holders))))

# Tables generated from Unicode data (or from tables that were).
UNICODE_TABLES = [
    ("compact_str", "src/unicode_data.rs"),
    ("harfrust", "src/hb/ucd_table.rs"),
    ("harfrust", "src/hb/unicode_emoji_table.rs"),
    ("harfrust", "src/hb/ot_shaper_arabic_table.rs"),
    ("harfrust", "src/hb/ot_shaper_use_table.rs"),
    ("plist", "src/stream/ascii_reader.rs"),
    ("pulldown-cmark", "src/puncttable.rs"),
    ("unicase", "src/unicode/map.rs"),
    ("unicode-bidi", "src/char_data/tables.rs"),
    ("unicode-normalization", "src/tables.rs"),
    ("unicode-script", "src/tables.rs"),
    ("unicode-segmentation", "src/tables.rs"),
    ("unicode-vo", "src/lib.rs"),
    ("unicode-width", "src/tables.rs"),
]
print(
    "## Unicode tables in other crates\n\nThese files hold data derived from the Unicode "
    "Character Database or other Unicode Consortium files.\n"
)
for name, rel in UNICODE_TABLES:
    require(name, rel)
    for version, _, _ in crates(name):
        print(f"- `{name}` {version}: `{rel}`")
print()
printed_upstream.add("unicode-license.txt")
print(
    "The Unicode License, <https://www.unicode.org/license.txt>:\n\n"
    + block(read(os.path.join(UPSTREAM, "unicode-license.txt")))
)

unused = set(os.listdir(UPSTREAM)) - printed_upstream
if unused:
    fail(f"not printed: licenses/upstream/{', '.join(sorted(unused))}")

# ---- evidence for the hand-written notices ----------------------------------------------------

# licenses/katex.md, licenses/gemoji.md and the end of about.hbs are written by hand. Fail when
# what they describe is no longer there.
require("ratex-parser", "src/mhchem/mod.rs", marker="KaTeX mhchem 3.3.0")
require("ratex-parser", "src/mhchem/data/machines.json", "src/mhchem/data/patterns_regex.json")
require("ratex-font", "src/data/metrics_data.rs", marker="KaTeX fontMetricsData.js")
require("ratex-types", "src/unicode_scripts.rs", marker="unicodeScripts.ts")
require("emojis", "README.md", marker="[gemoji] v4.1.0")
require("ratex-katex-fonts", "fonts/OFL.txt", "fonts/FONT_NOTICE.txt")
require(
    "konoma-office-core",
    "src/slide_draw/presets/presetShapeDefinitions.xml",
    "src/slide_draw/presets/NOTICE.md",
)
require("konoma-mermaid-core", "src/layout/LICENSE-APACHE")
