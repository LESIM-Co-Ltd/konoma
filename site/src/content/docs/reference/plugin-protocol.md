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
Its `key` must be exactly one non-control, non-whitespace character; invalid or
duplicate copy keys in the same context reject the manifest and name the action.
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
| `SwitchRoot` | `{ root: Path }` | Change the tree root |
| `OpenTab` | `{ path: Path }` | Open a new host tab |

Navigation paths must be absolute, have no NUL bytes and fit the existing path
limit. The host's **`q` pops one screen**; it is host behavior, not another plugin
outcome or an action that bypasses the guard. The host applies navigation after
the worker returns the outcome.

The checked examples live in
`crates/konoma-plugin-test/tests/vectors/project-v2.json`, `project-calls-v2.msgpack.hex` and
`tests/project_vectors.rs`; the SDK tests cover each new type and outcome.
