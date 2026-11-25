// The tests of the child-process transport (`--features proc`). One test binary: this file and the
// modules below. Real-process tests (a spawned program) are not here; the end-to-end tests run
// the child as a thread over a pair of sockets.
#![cfg(feature = "proc")]

mod client;
mod common;
mod frames;
mod server;
mod wire;
