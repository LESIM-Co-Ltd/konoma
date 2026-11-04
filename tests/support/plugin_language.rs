pub fn assert_japanese_plugin_prose(label: &str, output: &str) {
    // Cargo/fixture diagnostics and OS errors are external text, not konoma context lines.
    let mut text = output.split("Usage:").next().unwrap().to_owned();
    for allowed in [
        "konoma plugin:",
        "command =",
        "Fixture author",
        "fixture metadata failure",
        "the source path is neither a regular file nor a symlink to a regular file",
        "Resource temporarily unavailable (os error 35)",
        "Resource temporarily unavailable (os error 11)",
        "Operation not permitted (os error 1)",
        "File exists (os error 17)",
        "error: fake build failure",
        "No such file or directory (os error 2)",
        "Is a directory (os error 21)",
        "Directory not empty (os error 66)",
        "Is a directory (os error 21)",
        "Not a directory (os error 20)",
        "Permission denied (os error 13)",
        "crates.io",
        "Cargo.lock",
        "plugins.toml",
        "config.toml",
        "[y/N]",
        "konoma-foo",
        "konoma-bar",
        "konoma-office",
        "0.3.2",
        "1.0.0",
    ] {
        text = text.replace(allowed, "");
    }
    for original in text.lines().filter(|line| !line.trim().is_empty()) {
        // Only balanced spans are identifiers. An unmatched delimiter exempts none of its line,
        // and can never carry quoting into the next line.
        let mut quoted = false;
        let line: String = if original.matches('`').count() % 2 == 0 {
            original
                .chars()
                .filter(|&c| {
                    if c == '`' {
                        quoted = !quoted;
                        return false;
                    }
                    !quoted
                })
                .collect()
        } else {
            original.to_owned()
        };
        let mut run = 0;
        let mut words = 0;
        for token in line.split_whitespace() {
            let word = token.trim_matches(|c: char| c.is_ascii_punctuation());
            // Unquoted schema/command identifiers and numeric/path values are data too.
            if token.starts_with('{') && token.ends_with('}')
                || token.contains('/')
                || token.starts_with("--")
                || matches!(
                    word,
                    "id" | "git" | "konoma" | "HOME" | "PATH" | "CARGO" | "JSON" | "TOML" | "UTC"
                )
                || ((word.contains('-') || word.contains('_'))
                    && word
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
                || word.chars().all(|c| c.is_ascii_digit() || c == '.')
            {
                continue;
            }
            let mut found = false;
            for _word in token
                .split(|c: char| !c.is_ascii_alphabetic())
                .filter(|s| !s.is_empty())
            {
                found = true;
                run += 1;
                words += 1;
                assert!(run < 2, "English prose in {label}: {line}");
            }
            if !found {
                run = 0;
            }
        }
        assert!(
            line.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c)) || words == 0,
            "untranslated line in {label}: {line}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::assert_japanese_plugin_prose;

    fn rejects(text: &str) {
        assert!(
            std::panic::catch_unwind(|| assert_japanese_plugin_prose("helper regression", text))
                .is_err(),
            "accepted {text:?}"
        );
    }

    #[test]
    fn rejects_english_mixed_into_japanese() {
        rejects("`/fixture/path` を読めません: bad data");
    }

    #[test]
    fn rejects_english_touching_japanese_punctuation() {
        rejects("読めません: bad data。");
        rejects("読めません: bad dataです。");
    }

    #[test]
    fn rejects_pure_english() {
        rejects("the build failed");
    }

    #[test]
    fn rejects_english_after_an_unmatched_backtick() {
        rejects("`\nこの入力は bad data です");
    }

    #[test]
    fn keeps_an_unmatched_backtick_line_in_the_check() {
        rejects("これは `bad data です");
    }

    #[test]
    fn accepts_japanese_with_quoted_identifiers_paths_and_flags() {
        assert_japanese_plugin_prose("real message forms", "プラグイン `/fixture/a path` を読めません\n`konoma-foo` は入っていません（`konoma plugin list` で確認できます）\n`--branch` と `--rev` はどちらか一方を指定してください\nkonoma-code  0.1.0");
    }
}
