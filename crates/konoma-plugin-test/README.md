# konoma-plugin-test

Check a child-process plugin using the same protocol and deadlines as konoma, regardless of the
language used to implement it:

```sh
cargo run -p konoma-plugin-test -- --file samples/demo.konoma-demo path/to/plugin
cargo run -p konoma-plugin-test -- --plugin example --file example.bin python3 -- plugin.py
```

Arguments after `--` go before `--plugin <id> --serve` in the child command. `--file` is repeatable.
Without files, open/view/hints/actions/keys/cancellation are reported as `SKIP`, never as passes.
Each check produces one English line followed by a summary. Exit codes are 0 (no failures),
1 (conformance failures), and 2 (invalid usage, including a missing/non-executable program or file).
The CLI has no timeout override; the library's `run(&Options, Limits)` and `cli::execute` accept
explicit limits for tests and embedding.

Checks cover the handshake and manifest, version refusal, file opening, declared view types,
the SDK's hint rules, every initially enabled declared action and its key, unknown sessions,
cancellation, graceful shutdown, malformed MessagePack, and oversized frame headers. An action
or key answering `Err` is valid; a dead connection, wrong reply, broken frame, or late answer fails.
Cancellation sends a view and its Cancel together and accepts a view that already finished.
Manifest checks reuse the host registry's validation of ids, declarations and globs, in addition
to the SDK's key and `shows` checks. Hints use the state from `Viewed`, or keep the open state after
a valid `Answer::Err`; EOF, a deadline, a broken frame or an unexpected reply fails the hints check.

The harness offers protocol versions 1 and 2. Project, dialog, read-only and refresh declarations,
project inputs and view requests, action selections/answers, row values, marked rows and navigation outcomes require version 2; sending them after
selecting version 1 fails conformance. Navigation paths and preview line numbers are checked
before an action reply is accepted. The project manifest JSON vector is
`tests/vectors/project-v2.json`; `tests/project_vectors.rs` covers project input, all five
navigation outcomes and independent list marks. `tests/vectors/project-calls-v2.msgpack.hex` fixes the
view selection, row values, marked set and dialog answers. SDK tests also cover each project wire type.

Project chip values can be fetched through the library's
`project_chip(&options, root, marker, &view_context, limits)`. It opens a project in a fresh child,
returns the current styled `Option<Chip>`, bounds the text/spans, and checks that the child remains
alive. `None` uses the declared static chip. A child without a project declaration or the
protocol-2 `session_chip` capability receives no chip call. Errors include the bounded stderr
tail; wrong reply kinds/ids, malformed frames, EOF and deadlines fail just as for view/act.
File probes do not call chip. Each project-chip fetch stops and reaps its child before returning.

Each independent probe uses a fresh process group and a private handoff directory. The child's
working directory is inherited so interpreter arguments such as `plugin.py` still resolve.
Every path, including timeout and unwinding, kills that specific group, waits for the child, joins
the pipe workers and removes the directory. Failing checks include the expectation, actual
response/error, and bounded stderr tail; control characters are escaped to keep one line per check.
File-dependent checks whose prerequisite cannot open the file are skipped. Other probes continue.

The current wire protocol carries action declarations and enabled state, rather than a separate
hint list: the host derives hints from them. `testing::check_hints` is the authority here; the
disabled-action rule is enforced by that derivation. For the stable enabled state in `Snapshot`,
the `check_hints` branch that adds `shown_while_disabled` cannot be reached over the wire: there is
no child-supplied hint list, and the host removes disabled actions before checking the shown hints.
A unit test using `Snapshot` and `Declarations` asserts that enabled=false produces a clean report,
that the host omits that action's hint, and that enabling it adds the hint. A label/help omission is the negative hint
fixture. The SDK sends `Refused` and exits successfully for an unsupported version; that is accepted,
as is a peer that stays up after refusal. A crash following refusal fails.

Tests use a program under `tests/` with explicit fixture modes and the real SDK multi-plugin entry,
plus the real `konoma-demo-clock` executable. The clock test builds that executable offline with
the profile and target of the test executable and the caller's unchanged `CARGO_TARGET_DIR`.
No fixture modes enter the product binary. Passing tests
use `Limits::default()`; only negative tests specifically exercising a deadline use shorter limits.
Message ordering uses protocol messages and pipe barriers, never sleeps.
