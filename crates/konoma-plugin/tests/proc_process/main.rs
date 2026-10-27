//! Tests with a real child process. This binary is two programs in one: when the environment
//! variable `KP_PROC_CHILD` is set it is a plugin and runs `cli_main` (so `--serve`, `--manifest`,
//! `--version` and `--help` are the real ones); otherwise it runs the tests, which start
//! `current_exe()` as the child.
//!
//! `harness = false`: the child needs its own `main`. The test side answers the two requests of
//! libtest's command line that nextest makes: `--list --format terse` and `--exact NAME`.
#![cfg(feature = "proc")]

mod child;
mod tests;

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(mode) = std::env::var_os(child::CHILD_ENV) {
        // "multi": a program that carries two plugins (`export!(A, B)`).
        if mode == "multi" {
            return konoma_plugin::proc::cli_main_many(vec![
                Box::new(child::ChildPlugin),
                Box::new(child::SecondPlugin),
            ]);
        }
        return konoma_plugin::proc::cli_main(child::ChildPlugin);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = tests::all();
    if args.iter().any(|a| a == "--list") {
        // `--ignored` lists the ignored tests: there are none.
        if !args.iter().any(|a| a == "--ignored") {
            for (name, _) in &all {
                println!("{name}: test");
            }
        }
        return ExitCode::SUCCESS;
    }
    // `--exact NAME` runs one; a bare name or nothing runs by substring / everything.
    let mut exact = None;
    let mut filters = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--exact" => {}
            "--test-threads" | "--format" | "--color" | "--logfile" | "--skip" => {
                it.next();
            }
            s if s.starts_with("--") => {}
            s => {
                exact = Some(s.to_string());
                filters.push(s.to_string());
            }
        }
    }
    let mut failed = 0;
    let mut ran = 0;
    for (name, f) in &all {
        if !filters.is_empty() && !filters.iter().any(|x| x == name) {
            continue;
        }
        ran += 1;
        // One test per process under nextest; in a bare run they are sequential here (each starts
        // and waits for its own children, so none leaves a process behind for the next).
        match std::panic::catch_unwind(f) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    if exact.is_some() && ran == 0 {
        eprintln!("no test named {exact:?}");
        return ExitCode::FAILURE;
    }
    println!(
        "\ntest result: {}. {ran} run, {failed} failed",
        if failed == 0 { "ok" } else { "FAILED" }
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}
