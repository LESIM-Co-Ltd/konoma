---
title: Plugin protocol
description: The child-process plugin protocol, including project declarations in version 2.
---

The reference implementation is `crates/konoma-plugin/src/proc/{proto,wire}.rs`.
Programs in any language can implement this protocol and run
`konoma-plugin-test PROGRAM --file FILE` to check it. The SDK's `export!`,
`--manifest` and `--serve` use these same conversions.

## Transport and negotiation

The host starts `PROGRAM ARGS --serve`. Standard input and standard output are
protocol pipes; diagnostics belong on standard error. Each frame is a four-byte
unsigned big-endian body length followed by MessagePack. The body limit is 64 MiB
and the nesting limit is 128. Structs are maps with named fields. Enums are either
a variant string or a single-entry map, for example `"Reload"` or
`{"OpenDiff":{"path":…}}`. JSON below is explanatory; paths are MessagePack
**binary** values containing Unix path bytes, not UTF-8 strings. JSON manifest
exports represent those bytes as integer arrays.

The host first sends `Hello { versions, host_version, side_dir, lang }`. The child
answers `Ack { version, manifest }` or `Refused { supported, reason }`, using the
same enum encoding. `side_dir` is an absolute path. Large binary payloads may use
the existing `WBin::Side` handoff instead of inline bytes; project data does not
introduce another handoff mechanism.

The current protocol is **2**. Hosts offer `[1, 2]`. The SDK selects the highest
offered supported version. A plugin with project, prompt, read-only or refresh declarations supports
only 2; a plugin without them supports both 1 and 2. An old host offering only 1
therefore still runs an ordinary file plugin and receives `Refused` from a project
plugin. A version 1 child remains accepted by a version 2 host.

Version 2 is necessary because an old peer cannot decode the new `Project` input
or navigation outcome variants. Optional fields alone cannot negotiate these
variants, and ignoring confirmation declarations would change an action's meaning.
New manifest/action fields default to absent and are omitted when absent; the
row mark defaults to false and is omitted when false. Existing messages retain
their version 1 encoding. Both SDK peers and the conformance harness reject
project data on a connection that selected 1.
The new request/context fields and nonempty row values also require version 2.
This extends the unpublished version 2 in the same change; it does not introduce version 3.

## Calls and replies

Host calls are `Open { sid, rid, input, cx }`, `View { sid, rid, req, cx }`,
`Act { sid, rid, action, cx }` and `Key { sid, rid, key, cx }`.
`Close { sid }`, `Cancel { rid }` and `Shutdown` are notices with no reply.
The child answers `FromChild { rid, answer }`, where `answer` is `Ok(reply)` or
`Err(failure)`. Replies are `Opened { state }`, `Viewed { view, state }` and
`Acted { outcome, state }`. `state` contains `layout` and the enabled state of
each declared action. Existing file/content/embed inputs, display types, keys,
settings, failure kinds, cancellation and side-file rules remain unchanged.

All translated strings use `Tr = { en: string, others: [[language, string], …] }`.
English is required; missing translations fall back to English.

### Project view and action calls (version 2)

`View.req.project` is absent for file previews. For a project it is
`{ screen: string, selection: string | null }`. `selection` is the opaque value
from `PushScreen`; it is kept in the view request because screens can change
within one open project session. The SDK builds it with
`ViewReq::new(cols, rows).project(ProjectView::new(screen).selection(value))`.

`Act.cx.project` and `Key.cx.project` are absent outside project actions. Their shape is:

```text
ProjectAct {
  screen: string | null,
  selected: { kind: string, value: string } | null,
  marked: [{ kind: string, value: string }],
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

`Manifest.project` is optional and independent of file bindings and the existing
`screens: string[]` key contexts. Its shape is:

```text
ProjectDecl {
  markers: [{ name: string, kind: "Any" | "File" | "Directory" }],
  screens: [{ id: string, title: Tr, key: character | null, row_kinds: string[] }],
  chip: Tr | null,
  watch_paths: Path[],
  version_control: boolean
}
```

Markers are case-sensitive file-name globs, matched at the tree root and then
each ancestor. Several markers and project plugins may match. A marker describes
an entry name, rather than a path to traverse. `Any` supports `.git` as either a
file or a directory. Screen order declares tab order; the optional key is one
character. Row-kind names are local to a screen. The chip is a short tree-title
summary. Watch paths are relative to the detected project root; a change
invalidates the project's data. `version_control` advertises an optional
capability; **the capability calls are static-link only** and have no wire RPC.

The input is `{"Project":{"root":Path,"marker":string}}`: an absolute detected
root and the actual matched entry name (for example `MyApp.xcworkspace`, rather
than `*.xcworkspace`). It is not a file byte input.

`Manifest.read_only` is a static boolean declaration, default false and omitted
when false. The host combines it with `[plugins.<id>] read_only` using OR before
running a `writes` action. No runtime read-only override is defined by this API.

## Actions and dialogs (version 2)

Existing action fields (`id`, `key`, `label`, `help`, `footer`, `screen`, `writes`,
`destructive`) keep their meaning. Four optional fields are added:

```text
project_context:
  { "Screen": { screen: string, row_kinds: string[] } }
  | "TreeSelection" | "Diff" | null
confirmation: { text: Tr, choices: [{ key: character, choice: Choice }] } | null
text_input: { label: Tr, initial: string } | null
choice_input: { label: Tr, choices: Choice[] } | null
refresh: RefreshTarget[]
Choice = { value: string, label: Tr }
```

An empty `row_kinds` applies to the whole named screen. `TreeSelection` actions
are menu-only. `Enter` is declared with `key: "Enter"` and a screen context with
one or more row kinds, through the same declaration and validation path as any
other action. Distinct row kinds may have distinct Enter actions. Confirmation
choices can distinguish `y` → `-d` and `!` → `-D`.

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
confirmation keys reserve its global layer plus `q` and `Esc`, while allowing `y` and `!`.
Control characters and duplicate keys in either set are refused. A confirmation has
1–64 choices; list prompts also have at most 64 choices. Choice values are at most
1024 bytes and every translated choice label is at most 512 characters.
Project screen key contexts have their own namespace, even for screen ids such as `tree`.
Until project navigation is implemented in standalone apps, its five outcomes show an
unsupported-action notice. A version refusal reports the manifest's actual supported
versions; a panic while reading the manifest produces a refusal instead of an abrupt EOF.

This stage declares dialogs only. Host dialogs, prompt answers, project screen
rendering and the single write guard are attached in a later change.

## Lists and navigation (version 2)

A list remains `{"List":[row, …]}`. Each row has `cells: Line[]`, `kind: string`,
`target: Path | null`, `checked: boolean | null`, and the new `marked: boolean`.
It also has an opaque `value: string`, built with `ListRow::value`. Its default is
the empty string, omitted on the wire, so legacy rows keep their bytes.
`kind`, target and plugin-owned `checked` keep their existing meaning. `marked`
is independent host multi-selection. Cells already contain styled spans:
`Line { spans: [[text, Style], …], style: Style }`; graph strokes use these same
spans. Each span is a two-element array on the wire. A `Style` has `fg`, `bg`,
`add` and `sub` fields; the last two are modifier bit masks.
Diff views retain their existing hunks and lines without a new encoding.

`Acted.outcome` accepts the existing `Unhandled`, `Redraw`, `Flash(Tr)` and
`Reload`, plus:

| Variant | Payload | Host behavior |
|---|---|---|
| `PushScreen` | `{ screen, selection: string \| null }` | Push this plugin's screen with an opaque row identifier |
| `OpenFile` | `{ path: Path, line: integer \| null }` | Open a preview; line is 1-based |
| `OpenDiff` | `{ path: Path }` | Open the host's full-screen diff |
| `SwitchRoot` | `{ root: Path }` | Change the tree root |
| `OpenTab` | `{ path: Path }` | Open a new host tab |

Navigation paths must be absolute, have no NUL bytes and fit the existing path
limit. The host's **`q` pops one screen**; it is host behavior, not another plugin
outcome or an action that bypasses the guard. Navigation is declared and
transported here; the host attaches its behavior in the subsequent stage.

The checked examples live in
`crates/konoma-plugin-test/tests/vectors/project-v2.json`, `project-calls-v2.msgpack.hex` and
`tests/project_vectors.rs`; the SDK tests cover each new type and outcome.
