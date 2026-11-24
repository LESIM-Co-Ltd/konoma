//! Confirmation keys share the legacy host's cancellation contract at every prompt stage.

use super::*;
use crate::app::project_screens::{ProjectDialog, ProjectPrompt};
use crate::i18n::Lang;

fn confirmation_stage(f: &mut Fixture, preceding: usize) {
    open(f);
    press(f, KeyCode::F(if preceding == 0 { 2 } else { 5 }));
    if preceding != 0 {
        // Production declarations currently put confirmation first. Exercise the dialog state
        // machine with the same declared prompts after a real text or choice submission, too.
        let Some(ProjectDialog::Act {
            prompts,
            buffer,
            cursor,
            ..
        }) = &mut f.sim.app.project_dialog
        else {
            panic!("missing project prompt");
        };
        let prompt = prompts.remove(preceding);
        *buffer = match &prompt {
            ProjectPrompt::Text(input) => input.initial.clone(),
            _ => String::new(),
        };
        *cursor = buffer.chars().count();
        prompts.insert(0, prompt);
        if preceding == 1 {
            f.sim.keys(" submitted");
        } else {
            f.sim.key('j');
        }
        f.sim.enter();
        let Some(ProjectDialog::Act { prompts, stage, .. }) = &f.sim.app.project_dialog else {
            panic!("submission must advance to confirmation");
        };
        assert_eq!(*stage, 1);
        assert!(matches!(prompts[*stage], ProjectPrompt::Confirmation(_)));
    }
    assert!(matches!(
        &f.sim.app.project_dialog,
        Some(ProjectDialog::Act { prompts, stage, .. })
            if matches!(prompts[*stage], ProjectPrompt::Confirmation(_))
    ));
    assert!(f.log.lock().unwrap().acts.is_empty());
}

fn cancel_confirmation(name: &str, key: KeyCode, lang: Lang, preceding: usize) {
    let mut f = fixture(name, false, 1, None, None, None);
    f.sim.app.lang = lang;
    confirmation_stage(&mut f, preceding);
    let path = f.sim.app.tab.root.join("file.txt");
    let before = std::fs::read(&path).unwrap();
    f.sim.app.flash = None;
    press(&mut f, key);
    assert!(f.sim.app.project_dialog.is_none());
    assert!(f.log.lock().unwrap().acts.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        f.sim.app.flash.as_deref(),
        Some(match lang {
            Lang::En => "canceled",
            Lang::Jp => "取消",
        })
    );
}

macro_rules! cancellation {
    ($name:ident, $key:expr, $lang:expr, $preceding:expr) => {
        #[test]
        fn $name() {
            cancel_confirmation(stringify!($name), $key, $lang, $preceding);
        }
    };
}

cancellation!(
    e2e_project_confirmation_cancel_n_first_en,
    KeyCode::Char('n'),
    Lang::En,
    0
);
cancellation!(
    e2e_project_confirmation_cancel_n_first_ja,
    KeyCode::Char('n'),
    Lang::Jp,
    0
);
cancellation!(
    e2e_project_confirmation_cancel_n_text_en,
    KeyCode::Char('n'),
    Lang::En,
    1
);
cancellation!(
    e2e_project_confirmation_cancel_n_text_ja,
    KeyCode::Char('n'),
    Lang::Jp,
    1
);
cancellation!(
    e2e_project_confirmation_cancel_n_choice_en,
    KeyCode::Char('n'),
    Lang::En,
    2
);
cancellation!(
    e2e_project_confirmation_cancel_n_choice_ja,
    KeyCode::Char('n'),
    Lang::Jp,
    2
);
cancellation!(
    e2e_project_confirmation_cancel_upper_n_first_en,
    KeyCode::Char('N'),
    Lang::En,
    0
);
cancellation!(
    e2e_project_confirmation_cancel_upper_n_first_ja,
    KeyCode::Char('N'),
    Lang::Jp,
    0
);
cancellation!(
    e2e_project_confirmation_cancel_upper_n_text_en,
    KeyCode::Char('N'),
    Lang::En,
    1
);
cancellation!(
    e2e_project_confirmation_cancel_upper_n_text_ja,
    KeyCode::Char('N'),
    Lang::Jp,
    1
);
cancellation!(
    e2e_project_confirmation_cancel_upper_n_choice_en,
    KeyCode::Char('N'),
    Lang::En,
    2
);
cancellation!(
    e2e_project_confirmation_cancel_upper_n_choice_ja,
    KeyCode::Char('N'),
    Lang::Jp,
    2
);
cancellation!(
    e2e_project_confirmation_cancel_escape_first_en,
    KeyCode::Esc,
    Lang::En,
    0
);
cancellation!(
    e2e_project_confirmation_cancel_escape_first_ja,
    KeyCode::Esc,
    Lang::Jp,
    0
);
cancellation!(
    e2e_project_confirmation_cancel_escape_text_en,
    KeyCode::Esc,
    Lang::En,
    1
);
cancellation!(
    e2e_project_confirmation_cancel_escape_text_ja,
    KeyCode::Esc,
    Lang::Jp,
    1
);
cancellation!(
    e2e_project_confirmation_cancel_escape_choice_en,
    KeyCode::Esc,
    Lang::En,
    2
);
cancellation!(
    e2e_project_confirmation_cancel_escape_choice_ja,
    KeyCode::Esc,
    Lang::Jp,
    2
);

#[test]
fn e2e_project_confirmation_uppercase_y_picks_lowercase_choice() {
    let mut f = fixture("project_confirmation_upper_y", false, 1, None, None, None);
    confirmation_stage(&mut f, 0);
    press(&mut f, KeyCode::Char('Y'));
    assert!(f.sim.app.project_dialog.is_none());
    assert_eq!(f.log.lock().unwrap().acts.len(), 1);
    assert_eq!(last_act(&f).1.confirmation.as_deref(), Some("yes-value"));
}

#[test]
fn e2e_project_confirmation_exact_uppercase_precedes_folding() {
    for (key, expected) in [('a', "lower-value"), ('A', "upper-value")] {
        let mut f = fixture(
            &format!("project_confirmation_exact_{key}"),
            false,
            1,
            None,
            None,
            None,
        );
        confirmation_stage(&mut f, 0);
        press(&mut f, KeyCode::Char(key));
        assert!(f.sim.app.project_dialog.is_none());
        assert_eq!(f.log.lock().unwrap().acts.len(), 1);
        assert_eq!(last_act(&f).1.confirmation.as_deref(), Some(expected));
    }
}

fn ignored_key(name: &str, key: KeyCode) {
    for preceding in 0..3 {
        let mut f = fixture(&format!("{name}_{preceding}"), false, 1, None, None, None);
        confirmation_stage(&mut f, preceding);
        let before = chip_screen(&f.sim);
        press(&mut f, key);
        assert!(f.sim.app.project_dialog.is_some());
        assert!(f.log.lock().unwrap().acts.is_empty());
        assert_eq!(f.sim.app.flash, None);
        assert_eq!(chip_screen(&f.sim), before);
    }
}

#[test]
fn e2e_project_confirmation_enter_does_nothing() {
    ignored_key("project_confirmation_enter", KeyCode::Enter);
}

#[test]
fn e2e_project_confirmation_undeclared_keys_do_nothing() {
    for (index, key) in [
        KeyCode::Down,
        KeyCode::Char('z'),
        KeyCode::Char('q'),
        KeyCode::Char('Q'),
        KeyCode::Char('É'),
        KeyCode::Backspace,
        KeyCode::F(12),
    ]
    .into_iter()
    .enumerate()
    {
        ignored_key(&format!("project_confirmation_unknown_{index}"), key);
    }
}

fn drawn_dialog_hint(sim: &Sim) -> String {
    let buffer = sim.term.backend().buffer();
    let (left, bottom) = (1..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .find(|&pos| buffer[pos].symbol() == "└" && buffer[pos].fg == ratatui::style::Color::Cyan)
        .expect("dialog bottom border");
    let right = (left + 1..buffer.area.width)
        .find(|&x| buffer[(x, bottom)].symbol() == "┘")
        .expect("dialog right border");
    let mut hint = String::new();
    let mut x = left + 1;
    while x < right {
        let symbol = buffer[(x, bottom - 1)].symbol();
        hint.push_str(symbol);
        x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
    }
    hint.trim().to_string()
}

fn confirmation_cancel_line(name: &str, lang: Lang, expected: &str) {
    for preceding in 0..3 {
        let mut f = fixture(&format!("{name}_{preceding}"), false, 1, None, None, None);
        f.sim.app.lang = lang;
        confirmation_stage(&mut f, preceding);
        let screen = chip_screen(&f.sim);
        assert_eq!(drawn_dialog_hint(&f.sim), expected, "{screen}");
        assert!(screen.contains(if preceding == 0 {
            "y: Accept"
        } else {
            "y: Yes"
        }));
    }
}

#[test]
fn e2e_project_confirmation_cancel_line_en() {
    confirmation_cancel_line("project_confirmation_line_en", Lang::En, "n / Esc = cancel");
}

#[test]
fn e2e_project_confirmation_cancel_line_ja() {
    confirmation_cancel_line("project_confirmation_line_ja", Lang::Jp, "n / Esc = 取消");
}

fn text_choose_line(name: &str, lang: Lang) {
    let mut f = fixture(name, false, 1, None, None, None);
    f.sim.app.lang = lang;
    open(&mut f);
    press(&mut f, KeyCode::F(3));
    let screen = chip_screen(&f.sim);
    assert_eq!(
        drawn_dialog_hint(&f.sim),
        crate::i18n::tr(lang, crate::i18n::Msg::ProjectDialogKeys),
        "{screen}"
    );
}

#[test]
fn e2e_project_text_keeps_choose_line_en() {
    text_choose_line("project_text_line_en", Lang::En);
}

#[test]
fn e2e_project_text_keeps_choose_line_ja() {
    text_choose_line("project_text_line_ja", Lang::Jp);
}

#[cfg(feature = "git")]
fn cancel_matches_legacy(name: &str, key: KeyCode, lang: Lang) {
    let mut f = fixture(name, false, 1, None, None, None);
    f.sim.app.lang = lang;
    let root = f.sim.app.tab.root.clone();
    let git = |args: &[&str]| {
        // Fixture Git commands use no user configuration, hooks, identity or clock.
        let output = std::process::Command::new("git")
            .current_dir(&root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["init", "-q", "--template=", "-b", "main"]);
    git(&["add", "file.txt"]);
    git(&["commit", "-qm", "seed"]);
    std::fs::write(root.join("file.txt"), "modified\n").unwrap();
    let statuses = || git(&["status", "--porcelain=v1"]);
    let before = statuses();
    let mut cfg = Config::default();
    cfg.ui.lang = match lang {
        Lang::En => "en",
        Lang::Jp => "ja",
    }
    .into();
    let mut old = Sim::with_config_sized(&root, cfg, 160, 24);
    old.key('p');
    assert!(old.app.is_git_view());
    let selected = old
        .app
        .git_view_entries()
        .iter()
        .position(|entry| entry.path == root.join("file.txt"))
        .expect("modified file in legacy Git view");
    for _ in 0..selected {
        old.key('j');
    }
    old.key('x');
    assert!(old.app.dialog_is_confirm());
    confirmation_stage(&mut f, 0);
    old.press(key, KeyModifiers::NONE);
    press(&mut f, key);
    assert!(!old.app.is_dialog());
    assert!(f.sim.app.project_dialog.is_none());
    assert_eq!(f.sim.app.flash, old.app.flash);
    assert_eq!(
        f.sim.app.flash.as_deref(),
        Some(match lang {
            Lang::En => "canceled",
            Lang::Jp => "取消",
        })
    );
    assert!(f.log.lock().unwrap().acts.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt")).unwrap(),
        "modified\n"
    );
    assert_eq!(statuses(), before);
}

macro_rules! legacy_cancellation {
    ($name:ident, $key:expr, $lang:expr) => {
        #[cfg(feature = "git")]
        #[test]
        fn $name() {
            cancel_matches_legacy(stringify!($name), $key, $lang);
        }
    };
}

legacy_cancellation!(
    e2e_project_confirmation_matches_legacy_n_en,
    KeyCode::Char('n'),
    Lang::En
);
legacy_cancellation!(
    e2e_project_confirmation_matches_legacy_n_ja,
    KeyCode::Char('n'),
    Lang::Jp
);
legacy_cancellation!(
    e2e_project_confirmation_matches_legacy_escape_en,
    KeyCode::Esc,
    Lang::En
);
legacy_cancellation!(
    e2e_project_confirmation_matches_legacy_escape_ja,
    KeyCode::Esc,
    Lang::Jp
);
