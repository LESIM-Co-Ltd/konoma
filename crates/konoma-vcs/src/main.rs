//! `konoma-vcs <directory>`: the Git and Jujutsu project plugins as a program of their own. Opens
//! the directory (cwd when omitted) in a terminal, serves konoma on stdin and stdout (`--serve`)
//! and answers `--manifest`, `--version` and `--help`. File plugins are Preview destinations only.

fn main() -> std::process::ExitCode {
    konoma_vcs::standalone::main()
}
