// All integration tests of this crate are built as ONE test binary (this file plus the modules
// below) instead of one binary per file: every extra test binary costs a separate link step and
// process start-up, which dominates the build and run time of small tests.
// To add tests, create a new file next to this one and list it here with `mod <name>;`.

mod convert;
mod survivors_batch_e;
mod width_parity;
