//! Protocol conformance checks for plugin programs, regardless of implementation language.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use konoma_plugin::keys::{parse_key, KeyName};
pub use konoma_plugin::proc::client::Limits;
use konoma_plugin::proc::{frame, proto::*, wire};
use konoma_plugin::{
    testing, Failure, Input, Key, KeyCode, KeyMods, Manifest, OpenCx, Plugin, Session, View,
    ViewCx, ViewReq,
};

pub mod cli;
mod connection;
use connection::{cx, Connection};

#[derive(Clone, Debug)]
pub struct Options {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub plugin: Option<String>,
    pub files: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail(String),
    Skip(String),
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub status: Status,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == Status::Pass)
            .count()
    }
    pub fn failed(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| matches!(c.status, Status::Fail(_)))
            .count()
    }
    pub fn skipped(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| matches!(c.status, Status::Skip(_)))
            .count()
    }
    pub fn exit_code(&self) -> u8 {
        u8::from(self.failed() != 0)
    }

    fn skip(&mut self, name: impl Into<String>, reason: &str) {
        self.checks.push(Check {
            name: one_line(&name.into()),
            status: Status::Skip(one_line(reason)),
        });
    }

    fn record(
        &mut self,
        name: impl Into<String>,
        expected: &str,
        result: Result<(), String>,
        tail: String,
    ) {
        let status = match result {
            Ok(()) => Status::Pass,
            Err(actual) => Status::Fail(one_line(&format!(
                "expected {expected}; got {actual}; stderr tail: {}",
                if tail.is_empty() { "<empty>" } else { &tail }
            ))),
        };
        self.checks.push(Check {
            name: one_line(&name.into()),
            status,
        });
    }

    fn probe(
        &mut self,
        name: &str,
        expected: &str,
        options: &Options,
        limits: &Limits,
        check: impl FnOnce(&Connection) -> Result<(), String>,
    ) {
        let (result, tail) = inspect(options, limits, check);
        self.record(name, expected, result, tail);
    }
}

fn one_line(s: &str) -> String {
    s.chars().flat_map(char::escape_debug).collect()
}

fn inspect<T>(
    options: &Options,
    limits: &Limits,
    check: impl FnOnce(&Connection) -> Result<T, String>,
) -> (Result<T, String>, String) {
    match Connection::spawn(options, limits) {
        Ok(mut c) => {
            let result = check(&c);
            (result, c.finish())
        }
        Err(e) => (Err(format!("could not start program: {e}")), String::new()),
    }
}

/// Run all independent checks. Every probe owns a fresh process group, so one broken connection
/// cannot hide later results. File-dependent checks are explicitly skipped without an input.
/// `limits` are the product defaults in the CLI; callers may shorten them for their own tests.
pub fn run(options: &Options, limits: Limits) -> Report {
    let mut report = Report::default();
    let (handshake, handshake_tail) = inspect(options, &limits, |c| {
        c.handshake(&limits, options.plugin.as_deref())
    });
    let manifest = handshake.as_ref().ok().cloned();
    report.record(
        "handshake",
        "Ack selecting an offered version and a readable manifest",
        handshake.map(|_| ()),
        handshake_tail.clone(),
    );
    if let Some(m) = &manifest {
        let issues = testing::check_manifest(&Declarations(m.clone()));
        let result = if issues.is_empty() {
            Ok(())
        } else {
            Err(format!("manifest issues: {issues:?}"))
        };
        report.record(
            "manifest",
            "no manifest issues (keys and shows)",
            result,
            handshake_tail,
        );
    } else {
        report.skip("manifest", "handshake did not yield a manifest");
    }
    report.probe(
        "version refusal",
        "Refused with non-empty supported versions",
        options,
        &limits,
        |c| match c.hello(vec![u32::MAX], limits.start_timeout)? {
            HelloReply::Refused { supported, .. } if !supported.is_empty() => {
                // The SDK exits successfully after refusing. Keeping the connection open is
                // allowed too, but a crash after emitting Refused is not a valid refusal.
                match c.exit_if_any(limits.shutdown_grace)? {
                    Some(status) if !status.success() => {
                        Err(format!("Refused followed by exit status {status}"))
                    }
                    _ => Ok(()),
                }
            }
            HelloReply::Refused { .. } => Err("Refused with empty supported versions".into()),
            other => Err(format!("{other:?}")),
        },
    );
    if options.files.is_empty() {
        for name in ["open", "view", "hints", "act", "key", "cancel"] {
            report.skip(name, "no --file supplied");
        }
    }
    for (i, file) in options.files.iter().enumerate() {
        let suffix = format!("[{}: {}]", i + 1, file.to_string_lossy());
        if manifest.is_none() {
            for name in ["open", "view", "hints", "act", "key", "cancel"] {
                report.skip(
                    format!("{name} {suffix}"),
                    "handshake did not yield a manifest",
                );
            }
            continue;
        }
        file_checks(&mut report, options, &limits, file, &suffix);
    }
    report.probe(
        "unknown session",
        "Err for an unopened sid and a live child",
        options,
        &limits,
        |c| {
            c.handshake(&limits, options.plugin.as_deref())?;
            match c.call(&Connection::view(u64::MAX, 2), 2, limits.open_view)? {
                Answer::Err(_) => c.alive(&limits),
                other => Err(format!("{other:?}")),
            }
        },
    );
    report.probe(
        "shutdown",
        "natural successful exit within shutdown_grace",
        options,
        &limits,
        |c| {
            c.handshake(&limits, options.plugin.as_deref())?;
            let until = Instant::now() + limits.shutdown_grace;
            c.send(&ToChild::Shutdown, until)?;
            let status = c.exited(until.saturating_duration_since(Instant::now()))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("exit status {status}"))
            }
        },
    );
    for (name, packet) in [
        ("malformed body", Connection::packet(vec![0xc1])),
        (
            "oversized frame",
            ((frame::MAX_FRAME + 1) as u32).to_be_bytes().to_vec(),
        ),
    ] {
        report.probe(
            name,
            "child exit within shutdown_grace after invalid frame",
            options,
            &limits,
            |c| {
                c.handshake(&limits, options.plugin.as_deref())?;
                let until = Instant::now() + limits.shutdown_grace;
                c.write(packet, until)?;
                c.exited(until.saturating_duration_since(Instant::now()))
                    .map(|_| ())
            },
        );
    }
    report
}

fn file_checks(report: &mut Report, options: &Options, limits: &Limits, file: &Path, suffix: &str) {
    let mut opened = false;
    report.probe(
        &format!("open {suffix}"),
        "Opened within open_view",
        options,
        limits,
        |c| {
            c.handshake(limits, options.plugin.as_deref())?;
            let result = c.open(file, limits);
            opened = result.is_ok();
            result.map(|_| ())
        },
    );
    if !opened {
        for name in ["view", "hints", "act", "key", "cancel"] {
            report.skip(format!("{name} {suffix}"), "file could not be opened");
        }
        return;
    }
    report.probe(
        &format!("view {suffix}"),
        "Viewed within open_view with a type in manifest.shows",
        options,
        limits,
        |c| {
            let manifest = c.handshake(limits, options.plugin.as_deref())?;
            c.open(file, limits)?;
            match c.call(&Connection::view(1, 2), 2, limits.open_view)? {
                Answer::Ok(Reply::Viewed { view, .. }) => {
                    let view = wire::view_from_wire(
                        view,
                        &mut wire::Dec::with_total_max(&c.side, wire::SIDE_TOTAL_MAX_FROM_CHILD),
                    )
                    .map_err(|e| e.to_string())?;
                    if view.kind().is_some_and(|k| manifest.shows.contains(&k)) {
                        Ok(())
                    } else {
                        Err(format!(
                            "view type {:?}; manifest.shows = {:?}",
                            view.kind(),
                            manifest.shows
                        ))
                    }
                }
                other => Err(format!("{other:?}")),
            }
        },
    );
    report.probe(
        &format!("hints {suffix}"),
        "clean hints for enabled actions, with label and help",
        options,
        limits,
        |c| {
            let manifest = c.handshake(limits, options.plugin.as_deref())?;
            let mut state = c.open(file, limits)?;
            // A successful view may change enabled state. A failed view leaves the open state usable.
            if let Ok(Answer::Ok(Reply::Viewed { state: next, .. })) =
                c.call(&Connection::view(1, 2), 2, limits.open_view)
            {
                state = next;
            }
            let hints = testing::check_hints(&Declarations(manifest), &Snapshot(state));
            if hints.is_clean() {
                Ok(())
            } else {
                Err(format!("{hints:?}"))
            }
        },
    );
    // Discover the declared/enabled operations independently of the display check.
    let (discovery, discovery_tail) = inspect(options, limits, |c| {
        let manifest = c.handshake(limits, options.plugin.as_deref())?;
        let state = c.open(file, limits)?;
        Ok((manifest.actions, state))
    });
    if let Ok((actions, state)) = &discovery {
        let mut ran_act = false;
        let mut ran_key = false;
        for action in actions {
            if !Snapshot(state.clone()).enabled(&action.id) {
                continue;
            }
            ran_act = true;
            report.probe(
                &format!("act {} {suffix}", action.id),
                "Acted or Err within act_key and a live child",
                options,
                limits,
                |c| {
                    c.handshake(limits, options.plugin.as_deref())?;
                    let state = c.open(file, limits)?;
                    if !Snapshot(state).enabled(&action.id) {
                        return Err(format!("action {} became disabled", action.id));
                    }
                    check_action(
                        c,
                        &ToChild::Act {
                            sid: 1,
                            rid: 4,
                            action: action.id.clone(),
                            cx: cx(),
                        },
                        4,
                        limits,
                    )
                },
            );
            if let Some(key) = &action.key {
                ran_key = true;
                report.probe(
                    &format!("key {key} {suffix}"),
                    "Acted or Err within act_key and a live child",
                    options,
                    limits,
                    |c| {
                        c.handshake(limits, options.plugin.as_deref())?;
                        c.open(file, limits)?;
                        let key = declared_key(key)?;
                        check_action(
                            c,
                            &ToChild::Key {
                                sid: 1,
                                rid: 5,
                                key: wire::key_to_wire(&key),
                                cx: cx(),
                            },
                            5,
                            limits,
                        )
                    },
                );
            }
        }
        if !ran_act {
            report.skip(format!("act {suffix}"), "no enabled declared actions");
        }
        if !ran_key {
            report.skip(
                format!("key {suffix}"),
                "no keys of enabled declared actions",
            );
        }
    } else {
        for name in ["act", "key"] {
            report.record(
                format!("{name} {suffix}"),
                "an open session for operation discovery",
                discovery.as_ref().map(|_| ()).map_err(Clone::clone),
                discovery_tail.clone(),
            );
        }
    }
    report.probe(
        &format!("cancel {suffix}"),
        "answer within cancel_grace after Cancel",
        options,
        limits,
        |c| {
            c.handshake(limits, options.plugin.as_deref())?;
            c.open(file, limits)?;
            let until = Instant::now() + limits.cancel_grace;
            // Queue both frames in one write. This tests cancellation without assuming the child
            // takes a particular amount of time; an already finished call is allowed to succeed.
            let mut packet = Connection::packet(
                frame::encode(&Connection::view(1, 3)).map_err(|e| e.to_string())?,
            );
            packet.extend(Connection::packet(
                frame::encode(&ToChild::Cancel { rid: 3 }).map_err(|e| e.to_string())?,
            ));
            c.write(packet, until)?;
            match c.answer(3, until)? {
                Answer::Ok(Reply::Viewed { .. }) | Answer::Err(_) => Ok(()),
                other => Err(format!("{other:?}")),
            }
        },
    );
}

fn check_action(c: &Connection, msg: &ToChild, rid: u64, limits: &Limits) -> Result<(), String> {
    match c.call(msg, rid, limits.act_key)? {
        Answer::Ok(Reply::Acted { .. }) | Answer::Err(_) => c.alive(limits),
        other => Err(format!("{other:?}")),
    }
}

fn declared_key(key: &str) -> Result<Key, String> {
    let spec = parse_key(key).map_err(|e| e.to_string())?;
    let code = match spec.code {
        KeyName::Char(c) => KeyCode::Char(c),
        KeyName::Enter => KeyCode::Enter,
        KeyName::Tab => KeyCode::Tab,
        KeyName::BackTab => KeyCode::BackTab,
        KeyName::Esc => KeyCode::Esc,
        KeyName::Backspace => KeyCode::Backspace,
        KeyName::Delete => KeyCode::Delete,
        KeyName::Up => KeyCode::Up,
        KeyName::Down => KeyCode::Down,
        KeyName::Left => KeyCode::Left,
        KeyName::Right => KeyCode::Right,
        KeyName::Home => KeyCode::Home,
        KeyName::End => KeyCode::End,
        KeyName::PageUp => KeyCode::PageUp,
        KeyName::PageDown => KeyCode::PageDown,
        KeyName::F(n) => KeyCode::F(n),
    };
    Ok(Key::new(code).with_mods(if spec.ctrl {
        KeyMods::CTRL
    } else {
        KeyMods::NONE
    }))
}

// Feed the decoded protocol state to the SDK's checks without inventing another set of rules.
struct Declarations(Manifest);
impl Plugin for Declarations {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported("declarations only"))
    }
}
struct Snapshot(WState);
impl Session for Snapshot {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Err(Failure::unsupported("state only"))
    }
    fn enabled(&self, action: &str) -> bool {
        self.0
            .enabled
            .iter()
            .rev()
            .find(|(id, _)| id == action)
            .is_none_or(|(_, enabled)| *enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::hints::hints;
    use konoma_plugin::view::Layout;
    use konoma_plugin::{Act, ViewKind};

    #[test]
    fn disabled_snapshot_is_clean_and_host_omits_its_hint() {
        let plugin = Declarations(
            Manifest::new("hints", "0.1.0")
                .shows(ViewKind::Canvas)
                .action(
                    Act::new("disabled")
                        .key("c")
                        .label("Disabled")
                        .help("Unavailable now"),
                )
                .action(
                    Act::new("enabled")
                        .key("d")
                        .label("Enabled")
                        .help("Available now"),
                ),
        );
        let session = Snapshot(WState {
            layout: wire::layout_to_wire(&Layout::single()),
            enabled: vec![("disabled".into(), false), ("enabled".into(), true)],
        });
        assert!(!session.enabled("disabled"));
        let report = testing::check_hints(&plugin, &session);
        assert!(report.is_clean(), "{report:?}");
        assert!(report.shown_while_disabled.is_empty());
        let shown = hints(&plugin.manifest(), &|id| session.enabled(id));
        assert_eq!(
            shown
                .iter()
                .map(|hint| hint.act.as_str())
                .collect::<Vec<_>>(),
            vec!["enabled"]
        );

        // Positive control: the same declared action acquires a hint when it becomes enabled.
        let enabled = Snapshot(WState {
            layout: wire::layout_to_wire(&Layout::single()),
            enabled: vec![("disabled".into(), true), ("enabled".into(), true)],
        });
        let shown = hints(&plugin.manifest(), &|id| enabled.enabled(id));
        assert_eq!(shown.len(), 2);
        assert!(shown.iter().any(|hint| hint.act == "disabled"));
    }
}
