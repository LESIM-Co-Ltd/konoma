// Syntax highlighting lives in the `konoma-code-core` crate (shared by the `code` plugin, the
// Markdown code blocks and the diff view); this module is the host's name for it.

pub use konoma_code_core::*;

#[cfg(test)]
mod tests {
    use super::*;

    // GUARDS: after `warm_dir` scans a directory and pre-compiles its languages' grammars, a
    // subsequent `highlight()` for one of those languages is fast (the warm-up actually did its job —
    // this exercises the directory-scanning entry point specifically, distinct from
    // `warm_then_highlight_is_fast`'s direct `warm_file` call). Converted from a wall-clock bound
    // (which — being tied to a one-time, process-global regex-compilation cost — is exactly the kind
    // of thing that becomes free noise-margin guesswork once the previous run's grammar is already
    // cached, and flakes if it isn't) to a **deterministic allocation ceiling**: a cold first-time
    // highlight compiles many regex patterns across the language's grammar contexts (an
    // allocation-heavy one-time cost — see `assets()`'s doc comment on why this is expensive), so an
    // unwarmed highlight allocates orders of magnitude more than a warm one.
    //
    // Uses "lua" rather than "rust"/"ts"/"py" (used throughout this test suite's own fixtures)
    // specifically because it is **not** used as a file extension anywhere else in this codebase's
    // tests: if the test used an already-commonly-warmed language, it would pass even if `warm_dir`
    // itself did nothing (some *other* test would have warmed it first) — no longer testing what it
    // claims to.
    #[test]
    fn warm_dir_makes_subsequent_highlight_fast() {
        let dir = crate::test_support::unique_tmp("konoma_code_warmdir");
        std::fs::create_dir_all(&dir).unwrap();
        let src = "-- a lua comment\nlocal function greet(name)\n  print('hello, ' .. name)\nend\n";
        let f = dir.join("a.lua");
        std::fs::write(&f, src).unwrap();
        assert!(
            has_named_syntax(&f),
            "lua の文法が解決できない(テスト前提が崩れている)"
        );

        warm_dir(dir.to_path_buf());
        let alloc = crate::mem_tests::allocated_by(|| {
            let lines = highlight(src, &f, "TwoDark");
            assert!(!lines.is_empty());
        });
        assert!(
            alloc < 2_000_000, // measured ~82KB warm vs ~26.5MB cold (an unwarmed "nim" highlight) — ~24x/~13x headroom
            "warm_dir 後の初回ハイライトの確保バイト数が多すぎる(warm_dir が効いていない?): {alloc}"
        );
    }
}
