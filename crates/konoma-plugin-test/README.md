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
