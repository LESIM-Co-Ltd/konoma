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

`versions` currently contains `[1]`; the host version policy accepts the current transport
version and its predecessor (there is no version before 1). `host_version` is an informational
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
