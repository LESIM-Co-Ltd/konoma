---
title: Plugin protocol
description: Version 1 of the language-independent child-process protocol, implemented by the Rust SDK.
sidebar:
  order: 3
---

This specifies the current Unix child-process transport. The Rust `konoma-plugin` crate is the
reference implementation. For a Rust walkthrough, see [Write a plugin](../../guides/write-a-plugin/).
The wire protocol version is **1**; the separate plugin API version is **[0, 1]**.

## Starting and stopping

konoma starts a configured program as `<command> <args> --serve`, with `--serve` appended last.
It does not invoke a shell. For a program carrying several plugins, put `--plugin <id>` in
`args`; each process serves one plugin. `konoma --plugin <path>` loads a program for that run,
and makes a relative program path absolute. Use absolute script/file paths in configured `args`:
the product uses a private working directory, not the user's current directory. The lower-level
SDK `CommandLaunch` defaults to `/` unless a directory is supplied; `konoma-plugin-test` instead
inherits its caller's directory so interpreter scripts can be relative.

The process inherits the host environment: `CommandLaunch` does not clear or add variables.
Executable discovery uses the host's `PATH`. No special plugin environment variable is required:
language, settings and terminal capabilities arrive in messages. Do not assume a terminal on the
pipes. Stdin receives protocol frames, stdout sends protocol frames, stderr is for diagnostics.
The SDK's `serve` duplicates the protocol descriptors with close-on-exec, redirects ordinary
stdout to stderr and ordinary stdin to `/dev/null`; another-language implementation must itself
keep logging and inherited helper descriptors from corrupting or retaining the protocol pipes.
The host starts a new process group and disables core dumps. Plugins run with the user's rights;
the read scope in messages is a cooperation/host-reading rule, not an OS sandbox.

The SDK also accepts `--manifest` (pretty JSON of the wire manifest), `--version` (one
`<id> <version>` line per selected plugin), and `--help`. These are separate invocations, not
framed messages. With several plugins, `--serve` and `--manifest` need `--plugin <id>`;
`--version` alone lists them all. A single-plugin program may omit the selector. A file argument
launches the SDK's standalone terminal only when built with the `app` feature. The product
discovers a child manifest through the handshake, rather than invoking `--manifest`.
Neither the product's child loader nor `konoma-plugin-test` requires `--manifest` or `--version`
from a non-Rust plugin. They are optional convenience modes; implement `--serve` (and selection
of `--plugin <id>` when you offer multiple plugins) for those consumers.

## Protect the protocol pipes

The requirement is to keep the original stdin/stdout exclusively for protocol bytes. The host
does not require or inspect particular descriptor syscalls. The SDK uses this Unix recipe:

1. Duplicate original fd 1, then fd 0, with `fcntl(fd, F_DUPFD_CLOEXEC, 3)`. Keep these new
   descriptors for protocol output/input; the minimum fd 3 avoids overwriting 0, 1 or 2.
2. Use `dup2(2, 1)` so ordinary stdout writes become stderr diagnostics.
3. Open `/dev/null` with `O_RDONLY | O_CLOEXEC`, use `dup2(null_fd, 0)`, then close the extra
   null descriptor if it is above 2. Ordinary reads now see EOF rather than consuming messages.
4. Read/write frames only through the saved descriptors. Their close-on-exec flags prevent an
   executed helper from keeping the protocol pipes open. Keep diagnostics on stderr.

Equivalent descriptor handling in another language is sufficient. The SDK reports duplication
or stdout-redirection errors; its `/dev/null` redirection is best-effort (an open failure leaves
fd 0 as it was, and the `dup2` result there is unchecked). Do not mistake that fallback for a
guarantee that a library cannot consume stdin.

## Frame and value notation

Every frame is exactly:

```text
offset  bytes   meaning
0       4       unsigned body length, big-endian (excludes these four bytes)
4       length  one MessagePack value
```

Flush after each complete frame. Body size is at most **67,108,864 bytes (64 MiB)**, checked
before reading the body. Decoding has a nesting limit of **128** and rejects trailing bytes.
EOF between frames is clean transport EOF; EOF inside a header/body is truncation. There is no
magic prefix, delimiter, JSON, compression or request envelope before the handshake.

The following schema uses `S` = UTF-8 MessagePack string, `B` = MessagePack **bin**, `bool`,
`u8/u16/u32/u64` = nonnegative integers fitting that range, `i64`, and `f64`.
`T?` means `nil` or `T`; `[T]` means an array; `(A,B)` means a two-element array, not a map.
Structs are maps with exactly the field spellings shown. Send every field listed, as the
reference encoder does (nullable fields may be `nil`). The current serde decoder also reads
missing nullable fields as `nil`; `Text.note` has an explicit default. Missing nonnullable fields
fail, and unknown struct fields are ignored; this is not permission to change a field's type.
Map keys (field names and enum tags) are strings, and tuples such as `[action_id, enabled]` are
arrays. For example, `Act.screen` in the manifest accepts either `screen:nil` or an omitted
`screen`, both meaning the main view's context (the declared `shows` types); `screen:"text"`
selects that context. A required
nonnullable field such as `Act.id` accepts neither omission nor `nil`. `Text.note` is explicitly
defaulted to `nil`; there is no general rule that missing nonnullable fields get defaults.

Enums are externally tagged: unit variant `Shutdown` is the string `"Shutdown"`, a variant
with one value is `{"Text": value}`, a variant with named fields is `{"Open": {fields...}}`,
and a tuple variant is `{"Rgb": [r,g,b]}`. Variant names are case-sensitive. `Option` has no
`Some` wrapper. A character is one Unicode scalar encoded as a string. Integer encodings need
not use a fixed byte width. `WPath` is transparently **bin** containing Unix path bytes, not a
UTF-8 string. Paths reject NUL and lengths over 16,384 bytes; paths that will be read must be
absolute. Host input paths are made absolute; relative child view paths fail conversion.
The SDK byte visitor also accepts arrays of u8 on decode; emit bin for the reference encoding.

## Handshake

The host sends `Hello` first; the child sends nothing before receiving it. `Hello` is this bare
map, not `{"Hello": ...}`. The reply is `{"Ack": {version:..., manifest:...}}` (or `Refused`),
as defined by `HelloReply`: it is tagged, but neither handshake value is a normal `ToChild`
call or a `{rid,answer}` response.

```text
Hello = {versions:[u32], host_version:S, side_dir:WPath, lang:S}
HelloReply = Ack {version:u32, manifest:Manifest}
           | Refused {supported:[u32], reason:S}
```

`versions` currently contains `[1, 2]`: the host offers the current transport version and its
predecessor. A plugin that declares a project, a prompt, read-only or refresh targets supports only 2. The new request/context fields and nonempty row values also require version 2; this extends the unpublished version 2 and does not introduce version 3. `host_version` is an informational
program version, not the protocol version. `side_dir` is the absolute handoff directory.
`lang` is the initial language; each later call carries its own language too.

Choose an offered version and return `Ack`. If none is supported, return `Refused` with your
supported versions and a reason. The host rejects an unoffered selection, an unreadable manifest,
or a manifest id different from the configured id. Refusal ends startup: the host closes/kills
that connection; it sends no normal calls. The SDK flushes `Refused` and returns successfully.
The checker accepts successful exit or a process still alive after refusal, but rejects a crash;
it does not require a second handshake on that connection. `Hello.host_version` and `Hello.lang`
are not validated by the SDK server. The API compatibility check is separate: major versions
must match; at major 0, minors must match exactly; otherwise the plugin minor must be no newer
than the host minor. Konoma checks this when registering the plugin; an incompatible API
declaration disables the plugin and is reported at startup.

The SDK decodes the first value directly as `Hello`, without a separate message discriminator.
If decoding fails (for example, the first value is `"Shutdown"`, or a required field is absent
or has the wrong type), it sends no `Refused` or call error: serving fails, the CLI writes a
diagnostic to stderr and exits 1. A schema-compatible map with extra fields can still decode
as `Hello`; it is not rejected simply because its sender intended a different message.
Clean EOF before that first frame returns successfully without sending anything.

## Calls and replies

After `Ack`, the host sends these enum values. `sid` is the host's session id; `rid` is a host
request id unique within this connection. The SDK host starts both counters at 1. Work is serial
across all sessions, in arrival order, while a reader can receive cancellation during work.

| Host message | Fields | Successful reply / effect |
|---|---|---|
| `Open` | `sid:u64, rid:u64, input:Input, cx:Cx` | `Opened {state:State}`; create a session |
| `View` | `sid:u64, rid:u64, req:ViewReq, cx:Cx` | `Viewed {view:View, state:State}`; read current display |
| `Act` | `sid:u64, rid:u64, action:S, cx:Cx` | `Acted {outcome:Outcome, state:State}`; declared action id |
| `Key` | `sid:u64, rid:u64, key:Key, cx:Cx` | `Acted {outcome:Outcome, state:State}`; raw key, normally canvas text input |
| `Cancel` | `rid:u64` | No separate reply; request the original call to finish |
| `Close` | `sid:u64` | No reply; drop that session |
| `Shutdown` | none (unit string) | No reply; drop sessions and exit successfully |

Each call receives exactly one `FromChild = {rid:u64, answer:Answer}`, where
`Answer = Ok(Reply) | Err(Failure)` and `Reply` is one of the three replies above. Any call can
return `Err`; an error does not include a replacement state. A wrong successful reply kind or
unknown/duplicate response `rid` ends the connection. The current host routes by `rid`, without
explicitly checking response order; serial processing is the reference server behavior and the
assumption behind deadline accounting. The server does not reject duplicate request ids or
session ids as a separate validation step: repeated `Open.sid` replaces and drops the old session.
Unknown session calls return `Err(Corrupt)`; unknown `Close` and `Cancel` are ignored.
The reference server allows 1,024 sessions; a further new open returns `TooHeavy`.

Cancellation raises a flag, not an immediate response. A queued cancelled call is answered
`Err(Cancelled)` when it reaches the worker, without invoking plugin code. An already finished
view may still succeed. Running work should check cancellation and return promptly.
On `Shutdown` the SDK also cancels running/queued calls, processes the queue, then drops sessions
in ascending sid order (the host allocates them in open order). On stdin EOF it raises all flags
and exits; `serve` forces process exit after 2 seconds if work remains stuck. Malformed input
ends serving with an error. Plugin panics inside SDK calls become `Err(External)`; a panic in
`manifest()` fails startup. These panic guards are SDK behavior, not a cross-language facility.

```text
State = {layout:Layout, enabled:[(S,bool)]}
Layout = {reprs:[(S,Tr)], default_repr:S?, default_repr_without_pixels:S?, pages:Pages?}
Pages = Separate {count:u64?, titles:[S]} | Anchored {heading_level:u8, count:u64}
ViewReq = {repr:S?, page:u64?, cols:u16, rows:u16, cell_px:(u16,u16)?}
Outcome = Unhandled | Redraw | Flash(Tr) | Reload
Failure = {kind:FailureKind, message:Tr, detail:S?}
FailureKind = Unsupported | Encrypted | Corrupt | TooLarge | TooHeavy | Timeout
            | Cancelled | Io | External
Key = {code:KeyCode, mods:u8}
KeyCode = Char(character) | Enter | Backspace | Delete | Tab | BackTab | Left | Right
        | Up | Down | Home | End | PageUp | PageDown | Esc | F(u8) | Paste(S)
```

`repr` selects a representation; `nil` selects the default. Pages are zero-based; `nil` means
first/whole. The wire conversion checks that a page fits the receiving machine's index type;
it does not validate it against the declared page count. `cols/rows` may be zero. `cell_px` is
cell width/height in pixels when known. `layout` and `enabled` are snapshots, refreshed with each
successful reply. An omitted action in `enabled` defaults to true; duplicate ids use the last
value. `Manifest.actions` supplies each action's key, label and help, while `State.enabled`
reports its current availability. Konoma uses these to show available actions in the footer,
help and menu. Supply nonempty English labels and help for enabled actions. The check named
`hints` opens the input, requests a view, and checks the declared keys and the labels/help that
the host would display from the returned state. It uses the `Viewed` state, or keeps the
`Opened` state after a valid `View` error. `Unhandled` leaves an action/key unused; `Redraw` requests a
new view; `Flash` shows a translated message; `Reload` asks the host to reopen the input.
Raw key modifiers are bit flags Ctrl=1, Alt=2, Shift=4; unknown bits are truncated. The converter
does not restrict the function-key number. Layouts are clipped to 64 representations and 100,000
pages/titles; overlong representation ids are dropped. User-facing translated strings are
clipped to 512 characters, with control characters other than newline/tab and U+10EEEE removed;
up to 16 non-English translations are kept. Failure details are clipped too.

<a id="minimal-loop"></a>

## Minimal loop for implementers

For a text-only implementation, the wire responsibilities are:

1. Read `Hello` before writing anything, select an offered version and send `Ack` with a manifest,
   or send `Refused` if no version matches. Supply the nonnullable manifest fields shown below;
   optional fields can be `nil`, and unused declaration arrays can be empty. Declare a file bind
   and `shows:["Text"]` for a file viewer. No settings, actions, custom screens or launch verbs
   are necessary. The manifest in the worked exchange is one complete minimal example.
2. Process calls serially in received order, maintaining sessions by `sid`. For `Open`, read or
   prepare the input and answer `Opened` with a layout/enabled snapshot, or `Err`. For `View`,
   return `Viewed` with a `Text` view and the current snapshot, or `Err`. A single, size-independent
   text representation with no pages need not use `repr`, `page`, `cols`, `rows` or `cell_px` to
   change its output. These fields still have the declared wire types.
3. Answer `Act` and `Key` even when you implement no actions or typing: `Acted` with
   `outcome:"Unhandled"` and the current state is sufficient. Do not silently drop a call.
   Calls on an unopened `sid` must fail; the SDK uses `Err(Corrupt)`.
4. `Cancel` has no reply of its own; let the named call finish within the cancellation grace,
   possibly with `Err(Cancelled)`. A view already finished may succeed. Ignore an unknown cancel
   id. Drop a session on `Close`, without replying; an unknown close id may be ignored.
   If a call can block, arrange to receive cancellation while it runs; serial work does not mean
   you can stop reading the pipe indefinitely.
5. On `Shutdown`, send no reply, release sessions and exit 0 within the shutdown grace. On clean
   EOF between frames, stop accepting work, release resources and finish; the SDK normally exits
   0 here, including EOF before `Hello`. A truncated/malformed/oversized frame is an error, not
   clean EOF: stop serving (the SDK CLI exits 1). The checker explicitly probes invalid frames.

The reference SDK supplies the reader/worker split, cancel flags, panic guards and descriptor
recipe; another language must supply equivalent wire behavior itself. In particular, on host
EOF it cancels work immediately and forces exit 1 after 2 seconds if work remains stuck. That
SDK escape path is distinct from its normal successful EOF exit. The host treats unexpected
child EOF as connection failure regardless of exit status; the checker requires exit 0 for
`Shutdown`. For a plugin with actions, send their declarations in the manifest and current
availability in each state snapshot; the host builds the action labels and help from them.

## Input, context and manifest

```text
Input = File {path:WPath, hint:S?}
      | Content {content_type:S, text:S, resources:[(S,Bin)], origin:Origin}
      | Embed {kind:S, source:S, display:bool}
Origin = File(WPath) | Converted {from:WPath}
Cx = {settings:[(S,Value)], lang:S, pixels:bool, colors:u8, external:bool, read_dirs:[WPath]?}
Value = Bool(bool) | Int(i64) | Float(f64) | Str(S) | List([Value]) | Table([(S,Value)])
Tr = {en:S, others:[(S,S)]}
```

`File` names a file to read; `hint` is an optional content-type hint. `Content` is already
converted text with named byte resources and source origin; `Embed` is an embedded source
(`display` distinguishes block/display form). Lazy SDK resources are omitted, not serialized.
`settings` carries plugin settings; nested `Value` conversion refuses depth greater than 32
(root depth 0). `pixels` reports pixel support. `colors`: 0=ANSI16, 1=ANSI256, 2=truecolor;
unknown values become ANSI16. `external` grants cooperative permission for an external fallback.
`read_dirs=nil` is unrestricted, `[]` allows no directories, otherwise absolute roots. It
describes what the host may read for a plugin; a plugin reading itself should honor it.
`Tr.en` is English; `others` contains language/text pairs, e.g. `[ ["ja", "幅"] ]`, with English
fallback when a translation is absent.

The same manifest map is in `Ack` and in the SDK's JSON `--manifest` output (JSON cannot carry
arbitrary path/bin values, but this manifest schema has none):

```text
Manifest = {
  id:S, version:S, api:(u16,u16), name:Tr,
  binds:[Bind], actions:[Act], verbs:[Verb], settings:[Setting], screens:[S], shows:[ViewKind],
  work_group:S?, converts:bool, file_text:bool, sized:bool, unavailable:Unavailable
}
Bind = {targets:[Target], option:bool}
Target = Glob(S) | FileName(S) | Mime(S) | Content(S) | Embed(S)
Act = {id:S, key:S?, label:Tr, help:Tr, footer:u8?, screen:S?, writes:bool, destructive:bool}
Verb = {open:bool, chain:[Launcher], gui:bool, line_token:bool, label:Tr?}
Launcher = Editor | MacApp {bundle_id:S} | Command([S]) | OsDefault
Setting = {name:S, default:Value, help:Tr}
ViewKind = Text | Document | Table | Image | Diff | List | Canvas
Unavailable = Message | RawText {with_reason:bool} | Notice(Tr) | Picture
```

`id` identifies the plugin, `version` its release, `api` the SDK API contract, `name` its displayed
name. Bind targets select filename globs, exact filenames, MIME, converted content or embed
kinds; `option=true` makes a binding an explicit choice rather than the default. `shows` declares
main display kinds (a file binding without it has no attached screen). `screens` declares custom
key contexts. An action's `key` is key notation, not `KeyCode` (e.g. `"w"`, `"ctrl-g"`, `"F2"`);
`footer` is optional priority, lower first; `screen` restricts its context. `writes` identifies
writing actions for read-only gating; `destructive` marks data-destroying actions, but this API
does not yet have their confirmation declaration (do not infer a confirmation flow from the flag).
`label` is short footer/menu wording and `help` the explanation. Verbs declare launch fallback
chains: `open=true` means Open, false Edit; `gui` chooses GUI vs terminal; `line_token` permits
the editor line token. Settings declare defaults and help. `work_group` names an exclusive job
group; `converts` marks converted content, `file_text` identifies text of the original file,
`sized` marks size-dependent views; `unavailable` selects host failure presentation.
When konoma registers a plugin, it validates ids, globs and action keys as well as reading these
fields. Use the checker below to exercise those rules.

At registration, ids, action ids, setting names, screens (including action contexts) and work
groups must be 1..64 ASCII characters from `[a-z0-9._-]`. Limits are 256 actions, 256 settings,
1,024 binds, 64 screens, 512 targets per bind; targets are at most 1,024 bytes with no control
characters or U+10EEEE, and globs must compile. Setting defaults have at most 1,024 value nodes,
8 nested list/table levels (outermost container is level 1), and table keys at most 256 bytes
with no control characters or U+10EEEE. This registration check is stricter than wire value
depth 32. Version text is clipped to 64 characters, not semver-validated here. Later duplicate
action ids and destructive actions are dropped with a key-validation issue. Invalid, colliding
or reserved action keys can be dropped while keeping the action in the menu. `check_manifest`
and the protocol checker report these issues rather than treating them as a clean manifest.

## Views

```text
View = Text(Text) | Document(Document) | Table(Table) | Image(ImageView)
     | List([ListRow]) | Diff([[DiffLine]]) | Canvas(Canvas)
     | Reopen(Reopen) | Message([Line])
Style = {fg:Color?, bg:Color?, add:u16, sub:u16}
Color = Reset | Black | Red | Green | Yellow | Blue | Magenta | Cyan | Gray | DarkGray
      | LightRed | LightGreen | LightYellow | LightBlue | LightMagenta | LightCyan | White
      | Indexed(u8) | Rgb(u8,u8,u8)
Line = {spans:[(S,Style)], style:Style}
Text = {source:TextSource, file_lines:bool, syntax_name:S?, note:S?}
TextSource = File(WPath) | Owned(S) | Lines([Line])
Document = {markdown:S, origin:Origin, resources:[(S,Bin)], options:DocOptions, title:S?, truncated:bool}
DocOptions = {front_matter:bool, footnotes:bool, inline_html:bool, autolink:bool, emoji:bool,
  alerts:bool, details:S, task_states:[character], tasks_writable:bool, table_align:S, image_align:S}
Table = {title:S?, rainbow:bool, addressed:bool, ncols:u64, header:[S?], truncated:bool, rows:[[S]]}
ImageView = {data:Image, sharp_zoom:bool, raster_max_px:u32?}
Image = One(Leaf) | Frames([(Leaf,u64)])
Leaf = File(WPath) | Encoded(Bin) | Svg {data:Bin, base_dir:WPath?, trusted:bool}
     | Rgba {width:u32, height:u32, pixels:Bin}
ListRow = {cells:[Line], kind:S, target:WPath?, checked:bool?}
DiffLine = {kind:DiffKind, old_no:u64?, new_no:u64?, text:S}
DiffKind = Context | Added | Removed
Reopen = {content_type:S, text:S, resources:[(S,Bin)], origin:Origin, truncated:bool}
Canvas = {cols:u16, rows:u16, styles:Bin, runs:[[(u32,S)]], images:[CanvasImage],
  redraw_after_ms:u32?, text_input:bool}
CanvasImage = {id:u64, x:u16, y:u16, cols:u16, rows:u16, image:Image?}
```

Text uses a path, owned text, or styled lines. `file_lines` says line numbers correspond to the
original file; `syntax_name` is an optional host highlighter name. `note` is an SDK diagnostic
for a styling fallback, logged to child stderr and not shown by the host. No styler callback
crosses: the child styles text locally, only up to 8 MiB; styled encoding over 40 MiB falls back
to plain text/file. Table cells cross as strings with headers, optional title, rainbow/address
display flags and a truncation marker; per-cell detail callbacks do not cross. Rows may be short,
but not wider than `ncols`; headers may not be wider either. Maximum columns: 65,536; the sum
of `max(row length,1)` is at most 4,000,000.

Document carries Markdown, resources and rendering switches. `details` and alignment strings
are passed through by the converter, not enum-validated there. Reopen delegates converted
content to another plugin; Message is host-rendered explanatory lines. List rows have styled
cells, a kind, an optional absolute jump target and optional checkbox state. Diff is an array of
hunks, each an array of context/added/removed lines with optional original/new line numbers.

Image `Encoded` contains encoded image bytes; `Rgba` is tightly packed RGBA8 with nonzero width
and height and exactly `width*height*4` bytes. Animation delays are **microseconds**; at most
5,000 leaves, without nested animations. Lazy images cannot cross. SVG `trusted` is always
forced to false on reception, regardless of its sent value. File/base/origin/target paths must
be absolute. Outside a canvas the wire RGBA converter checks byte count and nonzero dimensions,
not the canvas's 4,096-pixel side bound; later host rendering has its own limits.

Canvas has at most 4,096 cells per side and 4,194,304 cells total. `runs` has exactly `rows`
arrays; each run is `[zero-based style index, concatenated cell symbols]`. Split text by Unicode
grapheme clusters to reconstruct cells, and use U+0000 for a wide character's continuation cell.
Every row must reconstruct exactly `cols` cells. A normal symbol is one grapheme, at most 32
UTF-8 bytes, width 1 or 2, without control characters or U+10EEEE; width 2 requires the following
continuation in that row. Width uses Unicode display width plus one column for each U+FF9E or
U+FF9F. Continuations must immediately follow a wide symbol. Split runs when adjacent symbols
would fuse into a grapheme. The SDK validates the reconstructed canvas.

`styles` contains 12-byte records: foreground 4 bytes, background 4 bytes, added modifier bits
2 bytes little-endian, removed bits 2 bytes little-endian. Each color record is `[tag,a,b,c]`:
0=absent; 1..17=the named Color variants above in order (Reset through White); 18=indexed
(`a` is index); 19=RGB (`a,b,c`). Unused color bytes are emitted as zero but are not checked on
receive. Unknown tags fail. Records must be whole; no more than `max(cols*rows,1)` styles.
Modifier bits 0..8 are bold, dim, italic, underlined, slow blink, rapid blink, reversed, hidden,
crossed out; unknown modifier bits are truncated (also for ordinary `Style`).

Canvas pictures use at most 4,096 placements and 256 distinct ids; placements must be nonempty
and within the canvas, and each id must use one cell size. `image=nil` reuses that id's previous
picture. Canvas RGBA sides are at most 4,096 pixels, including animation leaves. `redraw_after_ms`
is nullable milliseconds (the SDK rounds up); the wire converter accepts its u32 range without
clamping, while the canvas display contract schedules 33 ms to 24 hours only while fully visible.
`text_input` asks for raw keys; the host retains <kbd>Esc</kbd> to leave typing. Prefer a normal
view type when possible, to get host scrolling/search/selection/copy.

## Large byte data

```text
Bin = Inline(B) | File {name:S, len:u64}
```

The host makes a fresh handoff directory (0700), named by `Hello.side_dir`. SDK byte strings
up to 1 MiB stay inline; larger ones use side files. This is a sender threshold, not a receiver
ban on larger inline data (the frame cap still applies). Create a new regular file exclusively
with mode 0600 and finish writing before sending the message. SDK names are `h-0`, `h-1`, …
from the host and `c-0`, `c-1`, … from the child, unique for the connection. The receiver does not
require those prefixes: it requires a single name, 1..64 bytes, neither `.` nor `..`, with no `/`
or NUL. `len` must match the file length exactly. Links and nonregular files are refused; the
receiver reads without following links and deletes the file after reading, even on a short read.
Maximum individual file: 256 MiB; total side-file bytes per host message: 512 MiB, per child
answer: 256 MiB. Inline bytes are not counted in those totals. Resources are capped at 100,000
entries when decoded. Files from a failed conversion are discarded; connection teardown removes
the handoff directory. These files carry byte resources, pictures and canvas styles; arbitrary
large text/table strings do not automatically become side files. An answer still over the frame
limit becomes `Err(TooLarge)` in the reference server.

### Project view and action calls (version 2)

`View.req.project` is absent for file previews. For a project it is
`{ screen: string, selection: string | null, tree_root?: Path, open_dir?: Path, home_dir?: Path }`.
`selection` is the opaque value
from `PushScreen`; it is kept in the view request because screens can change
within one open project session. The SDK builds it with
`ViewReq::new(cols, rows).project(ProjectView::new(screen).selection(value))`.

`Act.cx.project` and `Key.cx.project` are absent outside project actions. Their shape is:

```text
ProjectAct {
  screen: string | null,
  selected: { kind: string, value: string, name?: string } | null,
  marked: [{ kind: string, value: string, name?: string }],
  confirmation: string | null,
  text: string | null,
  choice: string | null
}
```

A tree/diff action can omit the project screen. Selected and marked rows carry
the exact `ListRow.kind` and `ListRow.value`, not rendered cells or row indices.
The confirmation and list answers are the declared `Choice.value`; text is the
entered string. Empty strings are answers, distinct from absent answers.
Use `ActCx::with_project(ProjectAct::new()...)` to attach this data.

## Project declaration (version 2)

### Screen presentation and path context

`ProjectScreen::filter_label(Tr)` optionally supplies the footer's list-filter
label. Its wire field `filter_label?: Tr` defaults to `None` and is omitted when
absent, so existing screens keep the shared label and their previous encoding.
It changes wording only; shared project keys keep their order before row actions.

`ProjectScreen.marks(bool)` and `.checks(bool)` reserve fixed multi-selection
and check columns. Both default to false (omitted on the wire). Row data never
changes the column width; a missing check state reserves four blank cells.
Row `kind` is action metadata and is never rendered. `ListRow::selectable(false)`
represents a non-interactive message or heading, without selection reversal,
marks or actions. For an empty list, return a styled non-selectable row whose
text comes from `Tr`. The field defaults to true, preserving legacy list bytes.

`ProjectView` carries optional absolute `tree_root`, `open_dir` and `home_dir`.
The current tree root may differ from the detected root in `Input::Project`;
`open_dir` is the tab's launch directory. The host passes these on every request
and invalidates cached views when their path context changes. Older peers may
omit them. Display preferences remain plugin settings: Git and jj declare
`[plugins.git]` / `[plugins.jj] path_style = "relative" | "home" | "full"`.
Without an override, the host uses its existing path display setting.

### Session watch paths

`Session::watch_paths(&ViewCx) -> Result<Option<Vec<PathBuf>>, Failure>` returns
absolute paths. `None` uses the declaration's root-relative `watch_paths`;
`Some(vec![])` watches no data paths. The host calls on the session owner worker
after opening and after invalidation, and updates watches on its notification
worker. Notifications and chip refreshes use the effective paths. Linked Git
worktrees include the resolved gitdir and shared metadata directory, even outside
the project root; replacing a gitdir file causes paths to be resolved again.

Protocol-2 children advertise `session_watch: true` in the handshake manifest.
The call is `WatchPaths { sid, rid, cx }`; the reply is
`WatchPaths { paths: Path[] | null, state }`. Paths must be absolute; the host
accepts at most 512. Without the capability, the client returns `None` locally
and sends no unknown request to an old child. Ordinary protocol-1 data and
default-valued fields retain their bytes.

### Static declaration

`Manifest.project` is optional and independent of file bindings and the existing
`screens: string[]` key contexts. Its shape is:

```text
ProjectDecl {
  markers: [{ name: string, kind: "Any" | "File" | "Directory" }],
  screens: [{ id: string, title: Tr, key: character | null, row_kinds: string[], marks?: boolean, checks?: boolean, pushed?: boolean }],
  chip: Tr | null,
  watch_paths: Path[],
  version_control: boolean
}
```

`ProjectScreen::pushed()` sets `pushed: true` (absent means `false`). Such a screen
is reached only through `PushScreen`, is omitted from the tabs and their cycle,
and ignores `Tab` / `Shift-Tab` while active. It cannot declare a jump `key`;
validation names the offending screen. `q` / `Esc` pop it and preserve the
underlying screen's cursor. On lists, the host's Right motion (`l` / Right arrow
by default, following the text keymap and its remaps) runs the cursor row's
`Enter` action through the same guard. Plugins declare only `Enter`; no opening
hint is shown when that action cannot run on the row.

Markers are case-sensitive file-name globs, matched at the tree root and then
each ancestor. Several markers and project plugins may match. A marker describes
an entry name, rather than a path to traverse. `Any` supports `.git` as either a
file or a directory. Screen order declares tab order; the optional key is one
character. Row-kind names are local to a screen. The chip is a short tree-title
summary. Watch paths are relative to the detected project root; a change
invalidates the project's data. `version_control` advertises an optional
capability; **the capability calls are static-link only** and have no wire RPC.

For project sessions, the host asks `Session::chip` on the worker after opening,
after an action declaring `RefreshTarget::Chip`, and when declared watch paths
invalidate the project. It caches the result, rather than calling on every key or
draw. `Ok(None)` uses `ProjectDecl.chip` in the host's language (or no summary if
absent); an empty `Some(Chip::default())` explicitly hides it. A failure or panic
is reported through the host's flash message, keeps the session alive, and uses
the declared fallback. File-only sessions are never asked.

A computed `Chip` contains a styled `Line`. The host takes it through
`Chip::clipped` once before keeping it: at most 64 spans and 512 characters, with
controls and the terminal image placeholder removed and a trailing `…` when text
is cut. The tree title preserves span colors and uses terminal cell widths when
clipping, including CJK text. Multiple project summaries retain detection order
and the ` · ` separator; each tab displays only its detected projects.

Protocol 2 children advertise chip calls with `session_chip: true`. An older
child without that advertisement receives no chip call: the SDK client returns
`Ok(None)`, so the host displays the declared fallback. Project discovery in the
host currently accepts statically registered providers only.

The input is `{"Project":{"root":Path,"marker":string}}`: an absolute detected
root and the actual matched entry name (for example `MyApp.xcworkspace`, rather
than `*.xcworkspace`). It is not a file byte input.

`Manifest.read_only` is a static boolean declaration, default false and omitted
when false. The host combines it with `[plugins.<id>] read_only` using OR before
running a `writes` action. No runtime read-only override is defined by this API.

## Hint groups

`Manifest::hint_group(HintGroup::new(id, label).help(description))` declares one
footer/help hint for several actions. `Act::hint_group(id)` joins it. `label` and
`description` are `Tr`; an empty help text uses the label in the selected language.
The optional wire fields are:

```text
Manifest.hint_groups: [{ id: string, label: Tr, help: Tr }]
Act.hint_group: string | null
```

Both fields default to absent (an empty list / no membership) for older children
and are omitted when empty. JSON and MessagePack carry the same declarations.
Group ids are local to the plugin. Unknown membership, duplicate group ids,
members with different `screen`/`context` values, and members on different project
screens are separate registration errors naming the group and affected actions
(an empty duplicate group has no action to name). Different row kinds on the same
project screen are allowed, as are groups without members.

The footer replaces the members with one token at the position of the member
with the smallest footer priority, keeping the existing declaration order for
ties. Members without a footer priority do not contribute. Only enabled members'
keys appear, in action declaration order, with user rebinding applied, using the
host's usual notation (`s/S:stage(all)`). No enabled keyed member means no token.
Help similarly places one row at the first member in the existing help order,
including enabled members even without a footer priority, and writes its keys
as the host writes a multi-action row (`s / S`). The row uses the group's help
text and disappears when no enabled member has a key. This matches the legacy
hub's enabled-member rule. The `Space` menu keeps each action's entry and label.

## Actions and dialogs (version 2)

`Act::copy()` declares an item of the context's host `y` menu. Its optional wire
field is `copy: boolean` (absent means `false`, and `false` is omitted on output).
Copy items belong to a project screen: `project_context` must be `Screen`.
View-only, tree-selection and generic diff declarations are rejected with the action's name.
Its `key` must be exactly one non-control, non-whitespace character; invalid or
duplicate copy keys in the same context reject the manifest and name the action.
Menu closing keys (`q` and `Esc`) are reserved and cannot be copy item keys.
Copy keys do not participate in direct-key reservations or collisions. Copy items
have no direct key, individual footer hint or `Space` entry, even if a footer
priority is declared. Non-copy actions retain their existing behavior and wire data.

On project screens the host offers enabled items in declaration order, using their
declared menu keys and localized labels. It uses the generic localized copy title.
The help lists the `y` entries and the footer hints `y` only while the menu can open.
With no enabled items, `y` does nothing. On a pushed non-list screen, a copy action's
context's selected row carries the pushed selection as its value, with kind `selection`.

Existing action fields (`id`, `key`, `label`, `help`, `footer`, `screen`, `writes`,
`destructive`) keep their meaning. Four optional fields are added:

```text
project_context:
  { "Screen": { screen: string, row_kinds: string[] } }
  | "TreeSelection" | "Diff" | null
confirmation: { text: Tr, choices: [{ key: character, choice: Choice }], setting?: string } | null
text_input: { label: Tr, initial: string, keep_on_failure?: boolean } | null
choice_input: { label: Tr, choices: Choice[] } | null
refresh: RefreshTarget[]
Choice = { value: string, label: Tr }
```

An empty `row_kinds` applies to the whole named screen. `TreeSelection` actions
are menu-only. `Enter` is declared with `key: "Enter"` and a screen context with
one or more row kinds, through the same declaration and validation path as any
other action. Distinct row kinds may have distinct Enter actions. Confirmation
choices can distinguish `y` → `-d` and `!` → `-D`.

At any confirmation stage, `n`, `N` and `Esc` cancel the whole dialog without running
the action and show the host's localized cancellation flash (`canceled` / `取消`).
These keys cannot be declared as choices. An exact declared key wins; only if no
exact match exists does an uppercase ASCII letter pick its lowercase choice
(for example `Y` picks `y`). `Enter` and all other undeclared keys do nothing.
On text prompts, list prompts and the project chooser, `Esc` closes silently.

After selecting the language, the host replaces every literal `{row}` in confirmation
text with the invoked row's `name`, falling back to its `value`. Without a selected
row it uses an empty string. The invocation captures the row, so moving the cursor
or refreshing the list while the dialog is open does not change the prompt's name.
There are no other placeholders or escaping; text without `{row}` is unchanged.

`Confirmation::setting(key)` makes the confirmation conditional on a boolean setting
in `[plugins.<id>]`, using the same settings as `cx.settings` and the declared default
when absent. Only `false` skips the confirmation; a missing effective value or a
non-boolean value asks. Later text and choice prompts still appear, and the action
runs normally. `setting` defaults to `None` and is omitted on the wire; older children
therefore always ask. The registry rejects a setting on a destructive confirmation,
and a key without a `SettingDecl` with a boolean default, with separate errors naming
the action and key. Destructive actions always require confirmation.

`TextInput::keep_on_failure()` opts an action into reopening its text prompt when
`act` returns a `Failure`, including a panic or a child process failure. The host
flashes the failure and restores the submitted text with the cursor at the end.
The original row, screen and answers are retained, so Enter retries the same action;
only the text prompt returns, without repeating a preceding confirmation.
Restoration requires the invoking tab to be active, still displaying the same
project screen, with no dialog or text input active. Otherwise the failure is only
flashed. Success and every `Ok` outcome, including `Flash`, never reopen the prompt.
Failures while reading watch paths or refreshing after a successful action also
never reopen it.
The flag defaults to false and is omitted on the wire when false; older children
therefore keep the existing behavior.

`RefreshTarget` is one of `"CurrentScreen"`, `{"Screen":{"screen":string}}`,
`"AllScreens"`, `"TreeStatus"`, or `"Chip"`. An action's `refresh` list defaults
to empty and is then omitted. After execution, the host displays the result and
refreshes the declared targets. Named screens must exist in this plugin's project
declaration; at most 64 targets may be declared per action.

The registry refuses a destructive action without nonempty confirmation choices,
duplicate screen ids (including collisions with legacy screen names), duplicate
screen keys, unknown screens or row kinds in a typed project context, and a
project with no detection markers. Legacy `screen` key contexts retain their
existing behavior. Refusal reasons are typed in the SDK and translated by the
host into English or Japanese. Names and declaration counts keep bounded
validation, including project screen/row names and marker/row-kind counts.

Watch paths must be nonempty relative paths without parent, root or prefix components
(at most 512 paths). Markers must be a single nonempty file-name component other than
`.` or `..`, at most 1024 bytes. Screen keys use the existing list reserved-key table;
confirmation keys reserve its global layer plus `q`, `n`, `N` and `Esc`, while allowing `y` and `!`.
Control characters and duplicate keys in either set are refused. A confirmation has
1–64 choices; list prompts also have at most 64 choices. Choice values are at most
1024 bytes and every translated choice label is at most 512 characters.
Project screen key contexts have their own namespace, even for screen ids such as `tree`.
Until project navigation is implemented in standalone apps, its five outcomes show an
unsupported-action notice. A version refusal reports the manifest's actual supported
versions; a panic while reading the manifest produces a refusal instead of an abrupt EOF.

The host renders project screens and dialogs, passes prompt answers, and applies
the shared write guard before dispatching actions on the project worker.

## Lists and navigation (version 2)

A list remains `{"List":[row, …]}`. Each row has `cells: Line[]`, `kind: string`,
`target: Path | null`, `checked: boolean | null`, and the new `marked: boolean`.
It also has an opaque `value: string`, built with `ListRow::value`. Its default is
the empty string, omitted on the wire, so legacy rows keep their bytes.
`ListRow::name(impl Into<String>)` adds an optional short prompt name (`Option<String>`,
default `None`). The optional wire field is omitted when absent: older children
produce `None` and older hosts ignore it. It also reaches selected and marked
`ProjectRow`s; `ProjectRow::new(kind, value).name(name)` sets it directly.
`ListRow::right(Line)` adds `right: Line | null` (SDK `Option<Line>`, default
`None`), a styled, single-line suffix that the host aligns to the body's right
edge. If its display width plus two exceeds the body width, the host hides it
and ellipsizes the left cells. Otherwise it ellipsizes the left cells to leave
one space, pads the gap, and preserves the suffix's styles. CJK display widths
count; selection reverses the whole row.
`ListRow::filter_text(text)` adds `filter_text: string | null` (SDK
`Option<String>`, default `None`). Hosts use it for case-insensitive substring
filtering, including selection remapping after refresh. When absent, matching
uses the displayed cells and optional right suffix without alignment padding;
an explicit empty string only matches an empty query. Both fields are omitted
on the wire when absent: older children default to `None`, older hosts ignore
them, and existing rows keep their encoding.
`ListRow::initial_selection(true)` requests the initial cursor on that selectable
row when a screen first opens. The first visible request wins; later refreshes
preserve the selected identity. `ListRow::filter_fallback(true)` makes a message
unselectable and shows it only when no ordinary row matches, including an empty
query. `ListView::visible_rows(query)` implements that rule. Both wire fields
are booleans, default to false and are omitted when false, preserving old JSON
and MessagePack rows. A fallback is never a selection or an action target.
`kind`, target and plugin-owned `checked` keep their existing meaning. `marked`
is independent host multi-selection. Cells already contain styled spans:
`Line { spans: [[text, Style], …], style: Style }`; graph strokes use these same
spans. Each span is a two-element array on the wire. A `Style` has `fg`, `bg`,
`add` and `sub` fields; the last two are modifier bit masks.
`DiffView::new(hunks).header(lines)` carries styled `header: Line[]` above the
diff. A nonempty header is followed by a dim, full-width `─` divider.
`DiffHunk::new(lines).path(path)` identifies the file: a changed path produces a
file header and selects syntax colours by its extension; consecutive hunks with
the same path share that header. Both fields default to empty / `None`.

The diff wire payload is `{ header?: Line[], hunks: [{ path?: string | null,
lines: DiffLine[] }] }`. Readers also accept the old `DiffLine[][]` payload.
Line encoding is unchanged. The host uses its shared unified/split/auto diff
widget and highlights only the visible window. Its cached rows depend on the
data, width, layout and vertical window; split layout also includes the horizontal
offset. Unified horizontal scrolling reuses those rows and moves the header with
the diff. Row totals and horizontal limits are computed without highlighting.
With an empty diff, a header is followed by the divider and a dim
no-changes line; without a header the message is centred.

Text and diff screens scroll the top line, keeping the final page full. Their
text keymap (including the less scheme and `[keys.text]` remaps) supplies line,
half-page (10), page (20), top/bottom, horizontal (4 columns) and line-start/end
motions. Lists keep their cursor navigation. Diff screens additionally offer
`s` to cycle the shared layout and reset the horizontal offset; text screens do
not. The `cycle_diff_layout` binding uses the diff view's keymap context,
so `[keys.diff]` remaps apply to file diff previews and every project diff screen.
The footer and help show the resolved keys. This also works for project diffs
without the Git feature. `r` rereads the active project screen.

`Acted.outcome` accepts the existing `Unhandled`, `Redraw`, `Flash(Tr)` and
`Reload`, plus:

| Variant | Payload | Host behavior |
|---|---|---|
| `Copy` | `{ text: string }` | Copy the exact text, including an empty string, and show the standard clipboard success preview or failure notice |
| `PushScreen` | `{ screen, selection: string \| null }` | Push this plugin's screen with an opaque row identifier |
| `OpenFile` | `{ path: Path, line: integer \| null }` | Open a preview; line is 1-based |
| `OpenDiff` | `{ path: Path }` | Open the host's full-screen diff |
| `SwitchRoot` | `{ root: Path, message?: Tr }` | Change the tree root and launch directory; optionally override the success flash |
| `OpenTab` | `{ path: Path }` | Open a new host tab |

Navigation paths must be absolute, have no NUL bytes and fit the existing path
limit. The host's **`q` pops one screen**; it is host behavior, not another plugin
outcome or an action that bypasses the guard. The host applies navigation after
the worker returns the outcome.

`SwitchRoot.message` defaults to `None` and is omitted when absent, preserving
the old encoding and default switch flash. The message is displayed only after
a successful switch. For project lists, `Ctrl-t` uses the selected row's
`target`: a directory becomes the new tab's root and launch directory, a file
opens in preview, and the source tab and its screen remain intact. The footer
and help offer it only when the selected row has a target. Project footer plans
reconcile host open hints and declared actions to show each key once.

### Bundled VCS worktree screen

The git plugin declares `worktrees` (`w`), after branches when present, otherwise
after log. It lists git's worktrees in order, including current, bare, detached,
locked and prunable entries. Paths use the home-relative form; `/` matches only
the branch or full path, ignoring case. `Enter` (also `l` / Right) switches the
current tab, `Ctrl-t` opens an available non-current checkout in another tab,
`c` creates from a new or existing branch, and `d` pushes a host-rendered diff.
Creation trims the name, replaces `/` with `-` in the directory name, resolves
placement from the main worktree, switches to the canonical path and flashes
“Created worktree”. Blank names and git failures close the input without retry.
Only creation is a write and is blocked by read-only mode.

| `[plugins.git]` setting | Default | Meaning |
|---|---|---|
| `worktree_dir` | `"../"` | Placement relative to the main worktree |
| `graph_base_branches` | `[]` | Ordered base candidates; the main worktree's branch is appended |

Diffs exclude the worktree's own branch and choose the newest merge-base;
ties keep the first candidate. They show committed and uncommitted changes
since that base, falling back to uncommitted changes alone. With neither, the
screen flashes “no changes”. `s` changes layout, text motions scroll, and
`q` / Esc restores the list cursor. Reading the old `[git] worktree_dir` and
`[ui] graph_base_branches` names is deferred to the renamed-settings unit.

The legacy hub remains unchanged for comparison. Accepted differences are the
project title/tabs/frame and navigation hints, an empty message instead of
refusing to open an empty list, staying on the list after Enter on the current
worktree, and silent `Ctrl-t` with no hint on current/bare/unavailable rows.
jj declares neither this screen nor `w`, instead of the legacy refusal flash.
`/` comes first in the footer, followed by the row operations in their declared
order; its worktree label and help retain the legacy wording. A worktree diff's
title uses the diff header and divider instead of the legacy outer frame.

The checked examples live in
`crates/konoma-plugin-test/tests/vectors/project-v2.json`, `project-calls-v2.msgpack.hex` and
`tests/project_vectors.rs`; the SDK tests cover each new type and outcome.

## Deadlines and failures

| Limit | Product default |
|---|---|
| First discovery handshake, when no cached manifest exists | 5 seconds |
| Ordinary start/restart handshake | 30 seconds |
| Open / View | 30 seconds |
| Act / Key | 10 seconds |
| Answer after cancellation | 2 seconds |
| Exit after Shutdown | 2 seconds |
| Stop when idle, with no sessions/calls | 5 minutes (when idle reap runs) |
| Failed-start retry suppression | 10 seconds |
| Child resident memory | 2 GiB, sampled every 100 ms |
| Retained stderr tail | 4,096 bytes (failure detail is then sanitized/clipped) |

RSS enforcement depends on the OS helper returning a measurement; if it cannot, that check is
inactive and deadlines still apply. The RSS limit is monitored, not a hard OS memory quota.

Failure-kind conventions come from the plugin and SDK, not an enforced request-to-kind table.
The sample propagates `std::fs::read` errors through the SDK's `From<std::io::Error>`, producing
`Io` with an English/Japanese I/O-error message and the error text in `detail` (including an
unreadable file). It explicitly uses `Unsupported` for a non-file input. The SDK similarly uses
`Corrupt` for an unknown session and `Cancelled` for work cancelled before execution. These are
reference conventions, not a mandatory kind for every equivalent failure in another language:
`Answer::Err` carries any listed `FailureKind`; there is no wire rule assigning unreadable files
or unsupported inputs a particular kind. Choose a useful message and the appropriate kind for
your plugin rather than inventing another wire error format.

A call's deadline starts when it is the oldest outstanding call, not when queued. Cancellation
grace starts when both cancellation was requested and the call reached the head. These are host
defaults, not negotiated fields. The checker uses the normal `Limits` defaults, including a
30-second handshake; its CLI has no timeout override.

Timeout, crash/EOF, malformed/oversized frame, unmatched response id, wrong reply kind and memory
overuse end the whole child process group. Pending calls fail, other sessions become unusable,
and a subsequent open starts a new child (failed startups are suppressed for the retry interval).
The failure includes bounded stderr diagnostics. Stray stdout bytes are interpreted as a header
or body, so they can cause a framing failure or a deadline; there is no recovery scan. A valid
`Err` or a view that fails conversion (e.g. bad canvas/relative image path) fails only that call.
An intentional stop sends Shutdown first and kills the remaining group after the exit grace;
group cleanup also kills descendants after the leader exits. Bounded reaping allows 500 ms after
SIGKILL; failed cleanup is retained for a nonblocking reaper.

## Worked exchange (decoded MessagePack)

This is a minimal hexdump-like plugin serving `/data/a.bin`, containing the byte `0x41`.
Each arrow below is one framed value; the capital symbols `M`, `C`, `L` and `S` are expanded first
so the later blocks can be read without repeating them. They are notation, not wire references.
`bin("...")` denotes the literal bytes of the path. All optional fields are present here.

```text
M = {id:"hex", version:"0.1.0", api:[0,1], name:{en:"Hex",others:[]},
     binds:[{targets:[{Glob:"*.bin"}],option:false}], actions:[], verbs:[], settings:[],
     screens:[], shows:["Text"], work_group:nil, converts:false, file_text:false,
     sized:false, unavailable:"Message"}
C = {settings:[],lang:"en",pixels:false,colors:0,external:false,read_dirs:nil}
L = {reprs:[],default_repr:nil,default_repr_without_pixels:nil,pages:nil}
S = {fg:nil,bg:nil,add:0,sub:0}

host → {versions:[1],host_version:"0.31.2",side_dir:bin("/private/handoff"),lang:"en"}
child → {Ack:{version:1,manifest:M}}
host → {Open:{sid:1,rid:1,input:{File:{path:bin("/data/a.bin"),hint:nil}},cx:C}}
child → {rid:1,answer:{Ok:{Opened:{state:{layout:L,enabled:[]}}}}}
host → {View:{sid:1,rid:2,req:{repr:nil,page:nil,cols:80,rows:20,cell_px:nil},cx:C}}
child → {rid:2,answer:{Ok:{Viewed:{view:{Text:{source:{Lines:[{spans:[["00000000  41                                               |A|",S]],style:S}]},
          file_lines:false,syntax_name:nil,note:nil}},state:{layout:L,enabled:[]}}}}}
host → {Act:{sid:1,rid:3,action:"unknown",cx:C}}
child → {rid:3,answer:{Ok:{Acted:{outcome:"Unhandled",state:{layout:L,enabled:[]}}}}}
host → {Key:{sid:1,rid:4,key:{code:{Char:"x"},mods:0},cx:C}}
child → {rid:4,answer:{Ok:{Acted:{outcome:"Unhandled",state:{layout:L,enabled:[]}}}}}
host → {Cancel:{rid:2}}
host → {Close:{sid:1}}
host → "Shutdown"
```

The `Lines` source, one unstyled span and line style match the real sample's `View::text(lines)`;
`S` expands to a separate style map in both positions. Its 16-byte hex column is padded to 47
characters. This minimal manifest declares no actions, so the `Act` and raw `Key` examples are
unhandled. Here `Cancel` refers to an already answered view and is ignored. `Cancel`, `Close`
and the bare-string `Shutdown` have no child replies; Shutdown ends the process.

For example, the complete Shutdown frame is `00 00 00 09 a8 53 68 75 74 64 6f 77 6e`:
body length 9, MessagePack fixstr length 8 (`a8`), UTF-8 `Shutdown`. No newline follows it.

## Reference test vectors

These are complete frames, including their 4-byte length prefixes, produced by the reference
encoder. They encode the `Hello`
and `Ack` above, using the minimal `M` (not the sample's richer action manifest). Offsets are
hexadecimal byte offsets from the start of each frame. The Hello body is 67 bytes (`0x43`),
and the Ack body is 218 bytes (`0xda`). Named-map field order below is the reference encoder's
order; a receiver must use the map keys rather than depend on this order.

Hello:
<!-- vector:hello -->
```text
0000  00 00 00 43 84 a8 76 65 72 73 69 6f 6e 73 91 01
0010  ac 68 6f 73 74 5f 76 65 72 73 69 6f 6e a6 30 2e
0020  33 31 2e 32 a8 73 69 64 65 5f 64 69 72 c4 10 2f
0030  70 72 69 76 61 74 65 2f 68 61 6e 64 6f 66 66 a4
0040  6c 61 6e 67 a2 65 6e
```

Ack:
<!-- vector:ack -->
```text
0000  00 00 00 da 81 a3 41 63 6b 82 a7 76 65 72 73 69
0010  6f 6e 01 a8 6d 61 6e 69 66 65 73 74 8f a2 69 64
0020  a3 68 65 78 a7 76 65 72 73 69 6f 6e a5 30 2e 31
0030  2e 30 a3 61 70 69 92 00 01 a4 6e 61 6d 65 82 a2
0040  65 6e a3 48 65 78 a6 6f 74 68 65 72 73 90 a5 62
0050  69 6e 64 73 91 82 a7 74 61 72 67 65 74 73 91 81
0060  a4 47 6c 6f 62 a5 2a 2e 62 69 6e a6 6f 70 74 69
0070  6f 6e c2 a7 61 63 74 69 6f 6e 73 90 a5 76 65 72
0080  62 73 90 a8 73 65 74 74 69 6e 67 73 90 a7 73 63
0090  72 65 65 6e 73 90 a5 73 68 6f 77 73 91 a4 54 65
00a0  78 74 aa 77 6f 72 6b 5f 67 72 6f 75 70 c0 a8 63
00b0  6f 6e 76 65 72 74 73 c2 a9 66 69 6c 65 5f 74 65
00c0  78 74 c2 a5 73 69 7a 65 64 c2 ab 75 6e 61 76 61
00d0  69 6c 61 62 6c 65 a7 4d 65 73 73 61 67 65
```

## Check an implementation

From a konoma checkout, with an existing readable `.bin` file and your built program:

```sh
cargo run -p konoma-plugin-test -- --file a.bin /absolute/path/to/plugin
cargo run -p konoma-plugin-test -- --plugin hex --file a.bin python3 -- /absolute/path/plugin.py
```

To use the checker without Cargo at runtime, first build it, then run the binary. Its default
development path is `target/debug/konoma-plugin-test`; with `CARGO_TARGET_DIR` set, that path is
`$CARGO_TARGET_DIR/debug/konoma-plugin-test` (release builds use `release` instead of `debug`).

```sh
cargo build -p konoma-plugin-test
"${CARGO_TARGET_DIR:-target}/debug/konoma-plugin-test" --file a.bin /absolute/path/to/plugin
"${CARGO_TARGET_DIR:-target}/debug/konoma-plugin-test" --plugin hex --file a.bin python3 -- ./plugin.py
```

The checker inherits its caller's directory, so the relative script `./plugin.py` works there.
In product configuration, `command` need not be absolute: `python3` uses PATH, and `./viewer`
is resolved from konoma's startup directory. The child then runs in a private directory; use an
absolute script path in `args`, e.g. `args = ["/absolute/path/plugin.py"]`. Argument strings are
passed unchanged, not rejected just for being relative. See [configuration](../configuration/).

The checker appends `--plugin <id>` when selected, then `--serve`; it probes the framed
handshake, not `--manifest`. It validates the manifest and declared action keys, and checks
version refusal, opening and views. Its `hints` check builds the action labels/help from the
manifest actions and returned enabled state, requiring nonempty English label/help text for
enabled actions. It also probes initially enabled actions and their raw keys, unknown sessions,
cancellation, shutdown, malformed MessagePack and oversized headers. It uses fresh process groups and private handoff
directories for independent probes. File-dependent checks without `--file` are **SKIP**, not
passes; action/key `Err` responses are valid if the connection survives. Cancellation accepts
a view already completed. Exit codes: 0=no failures, 1=conformance failures, 2=invalid usage.
Passing these probes is evidence for the exercised paths, not proof of every rendering or
security property. Rust tests can call `konoma_plugin_test::run(&Options, Limits::default())`.
