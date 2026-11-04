pub fn assert_japanese_plugin_prose(label: &str, output: &str) {
    // Cargo/fixture diagnostics and OS errors are external text, not konoma context lines.
    let text = output.split("Usage:").next().unwrap();
    let allowed = [
        "konoma plugin:",
        "command =",
        "[plugins.<id>]",
        "Fixture author",
        "fixture metadata failure",
        "the source path is neither a regular file nor a symlink to a regular file",
        "error: fake build failure",
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
    ];
    let mut previous_line_is_context = false;
    for original in text.lines().filter(|line| !line.trim().is_empty()) {
        let follows_context = previous_line_is_context;
        previous_line_is_context = original.trim_end().ends_with(':');
        // io::Error's OS Display is external data, regardless of platform/errno/message.
        // Its start must be a context separator on this line, or the start of a diagnostic
        // below a context ending in ':', as PluginVerifyRefused formats the child's stderr.
        let mut context = String::new();
        let mut copied = 0;
        for (at, _) in original.match_indices(" (os error ") {
            let code_start = at + " (os error ".len();
            let Some(close) = original[code_start..].find(')') else {
                continue;
            };
            let code = &original[code_start..code_start + close];
            if code.is_empty() || !code.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let Some(start) = original[..at]
                .rfind(": ")
                .map(|at| at + 2)
                .or_else(|| follows_context.then_some(0))
            else {
                continue;
            };
            if start >= copied {
                context.push_str(&original[copied..start]);
                copied = code_start + close + 1;
            }
        }
        context.push_str(&original[copied..]);
        for allowed in allowed {
            context = context.replace(allowed, "");
        }
        let original = context.as_str();
        // Only balanced spans are identifiers. An unmatched delimiter exempts none of its line,
        // and can never carry quoting into the next line.
        let mut quoted = false;
        let line: String = if original.matches('`').count().is_multiple_of(2) {
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
    fn accepts_linux_os_error_after_japanese_context() {
        assert_japanese_plugin_prose(
            "Linux OS error",
            "プラグイン処理の監視結果を読み取れません: Connection reset by peer (os error 104)",
        );
    }

    #[test]
    fn accepts_macos_os_error_after_japanese_context() {
        assert_japanese_plugin_prose(
            "macOS OS error",
            "プラグイン処理の監視結果を読み取れません: Broken pipe (os error 32)",
        );
    }

    #[test]
    fn accepts_os_cause_on_its_own_line_after_context() {
        for gap in ["\n", "\n\n"] {
            // PluginVerifyRefused places the child diagnostic below its context line.
            assert_japanese_plugin_prose(
                "multiline build failure",
                &format!(
                    "新しいビルドを起動できませんでした。何も置き換えていません:{gap}Permission denied (os error 13)"
                ),
            );
        }
    }

    #[test]
    fn rejects_english_line_ending_in_an_os_error() {
        rejects("could not read the file (os error 2)");
    }

    #[test]
    fn rejects_english_sentence_before_an_os_error() {
        rejects("The build failed. Permission denied (os error 13)");
    }

    #[test]
    fn rejects_os_cause_without_a_preceding_context() {
        for text in [
            "Permission denied (os error 13)",
            "ビルドに失敗しました\nPermission denied (os error 13)",
            "ビルドに失敗しました:\n詳細を表示します\nPermission denied (os error 13)",
        ] {
            rejects(text);
        }
    }

    #[test]
    fn rejects_positive_signed_errno() {
        rejects("読み取れません: Permission denied (os error +5)");
    }

    #[test]
    fn rejects_negative_signed_errno() {
        rejects("読み取れません: Permission denied (os error -1)");
    }

    #[test]
    fn rejects_english_context_before_an_os_error() {
        rejects("could not read the watcher result: Connection reset by peer (os error 104)");
    }

    #[test]
    fn rejects_english_with_a_malformed_os_error_suffix() {
        for suffix in ["(os error", "(os error N)", "(os error 104", "(os error )"] {
            rejects(&format!("読み取れません: this is konoma prose {suffix}"));
        }
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
    fn accepts_the_literal_toml_table_in_the_configuration_example() {
        assert_japanese_plugin_prose(
            "configuration example",
            "[plugins.<id>]\n    command = \"/fixture/plugin\"",
        );
    }

    #[test]
    fn accepts_japanese_with_quoted_identifiers_paths_and_flags() {
        assert_japanese_plugin_prose("real message forms", "プラグイン `/fixture/a path` を読めません\n`konoma-foo` は入っていません（`konoma plugin list` で確認できます）\n`--branch` と `--rev` はどちらか一方を指定してください\nkonoma-code  0.1.0");
    }
}
