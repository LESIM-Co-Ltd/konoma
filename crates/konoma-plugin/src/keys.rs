//! Key notation, the layered reserved keys, and the validation of a manifest's keys.
//!
//! Layers (design 10-3): 0 global, 1 basics that are the same on every display, 2 per display type,
//! 3 verbs. A plugin may not use keys of layers 0 to 3. A plugin whose key collides keeps the action
//! (it stays in the `Space` menu) but loses the key.

use std::fmt;

use crate::manifest::{Act, Manifest};

/// A key without its modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A character; an upper-case letter means Shift is included.
    Char(char),
    Enter,
    Tab,
    /// Shift-Tab.
    BackTab,
    Esc,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// A function key, `F(1)` to `F(12)`.
    F(u8),
}

/// A parsed key: a code and whether Ctrl is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub code: KeyCode,
    pub ctrl: bool,
}

impl KeySpec {
    pub fn ch(c: char) -> KeySpec {
        KeySpec {
            code: KeyCode::Char(c),
            ctrl: false,
        }
    }

    pub fn ctrl(c: char) -> KeySpec {
        KeySpec {
            code: KeyCode::Char(c),
            ctrl: true,
        }
    }

    pub fn key(code: KeyCode) -> KeySpec {
        KeySpec { code, ctrl: false }
    }
}

impl fmt::Display for KeySpec {
    /// The notation used in help: `w`, `W`, `Ctrl-t`, `Enter`, `Shift-Tab`, `Space`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl-")?;
        }
        match self.code {
            KeyCode::Char(' ') => write!(f, "Space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => write!(f, "Enter"),
            KeyCode::Tab => write!(f, "Tab"),
            KeyCode::BackTab => write!(f, "Shift-Tab"),
            KeyCode::Esc => write!(f, "Esc"),
            KeyCode::Backspace => write!(f, "Backspace"),
            KeyCode::Delete => write!(f, "Delete"),
            KeyCode::Up => write!(f, "Up"),
            KeyCode::Down => write!(f, "Down"),
            KeyCode::Left => write!(f, "Left"),
            KeyCode::Right => write!(f, "Right"),
            KeyCode::Home => write!(f, "Home"),
            KeyCode::End => write!(f, "End"),
            KeyCode::PageUp => write!(f, "PageUp"),
            KeyCode::PageDown => write!(f, "PageDown"),
            KeyCode::F(n) => write!(f, "F{n}"),
        }
    }
}

/// Why a key string did not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    Empty,
    /// A modifier with no key after it.
    MissingKey(String),
    /// More than one key (chords are not allowed in plugin defaults) or an unknown name.
    Unknown(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Empty => write!(f, "empty key"),
            KeyError::MissingKey(s) => write!(f, "missing key after the modifier: {s}"),
            KeyError::Unknown(s) => write!(f, "unknown key: {s}"),
        }
    }
}

impl std::error::Error for KeyError {}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// Parse a key. Same rules as konoma's `[keys]`: names are case-insensitive (`enter`, `Space`),
/// `ctrl-`/`c-` (any case) is Ctrl, a single character keeps its case (upper case includes Shift).
/// `Shift-Tab` (or `backtab`) is Shift-Tab; `Shift-<letter>` is the upper-case letter.
pub fn parse_key(s: &str) -> Result<KeySpec, KeyError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(KeyError::Empty);
    }
    let (ctrl, rest) = if let Some(r) = strip_prefix_ci(s, "ctrl-") {
        (true, r)
    } else if let Some(r) = strip_prefix_ci(s, "c-") {
        (true, r)
    } else {
        (false, s)
    };
    if rest.is_empty() {
        return Err(KeyError::MissingKey(s.to_string()));
    }
    if let Some(r) = strip_prefix_ci(rest, "shift-") {
        if r.eq_ignore_ascii_case("tab") {
            return Ok(KeySpec {
                code: KeyCode::BackTab,
                ctrl,
            });
        }
        let mut it = r.chars();
        return match (it.next(), it.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Ok(KeySpec {
                code: KeyCode::Char(c.to_ascii_uppercase()),
                ctrl,
            }),
            (None, _) => Err(KeyError::MissingKey(s.to_string())),
            _ => Err(KeyError::Unknown(s.to_string())),
        };
    }
    let code = match rest.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" | "pagedn" => KeyCode::PageDown,
        name => {
            // `F1` to `F12`.
            if let Some(n) = name
                .strip_prefix('f')
                .filter(|d| d.len() <= 2 && d.chars().all(|c| c.is_ascii_digit()))
                .and_then(|d| d.parse::<u8>().ok())
                .filter(|n| (1..=12).contains(n))
            {
                return Ok(KeySpec {
                    code: KeyCode::F(n),
                    ctrl,
                });
            }
            let mut it = rest.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return Err(KeyError::Unknown(s.to_string())),
            }
        }
    };
    Ok(KeySpec { code, ctrl })
}

/// The layer a reserved key belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    /// Layer 0: the same everywhere.
    Global,
    /// Layer 1: the basics of every display.
    Base,
    /// Layer 2: the display type (and the host-only keys of the tree).
    Type,
    /// Layer 3: verbs.
    Verb,
    /// Layer 4: taken by a plugin in this context.
    Plugin,
}

fn chars(layer: Layer, cs: &str) -> Vec<(KeySpec, Layer)> {
    cs.chars().map(|c| (KeySpec::ch(c), layer)).collect()
}

fn ctrls(layer: Layer, cs: &str) -> Vec<(KeySpec, Layer)> {
    cs.chars().map(|c| (KeySpec::ctrl(c), layer)).collect()
}

fn named(layer: Layer, codes: &[KeyCode]) -> Vec<(KeySpec, Layer)> {
    codes.iter().map(|c| (KeySpec::key(*c), layer)).collect()
}

/// The host contexts that show a pane the plugin could add keys to.
const VIEW_CONTEXTS: [&str; 5] = ["text", "document", "table", "image", "diff"];

/// The keys a plugin cannot use in `context` (layers 0 to 3 and the context's layer 2).
///
/// Contexts: `global`, `tree`, `text`, `document`, `table`, `image`, `diff`, `list`. Any other
/// name (`git.changes`, a plugin's own screen) is treated as a list.
pub fn reserved(context: &str) -> Vec<(KeySpec, Layer)> {
    let mut v = Vec::new();
    // Layer 0.
    v.extend(chars(Layer::Global, "?QFPtT[]123456789"));
    // Layer 1.
    v.extend(chars(Layer::Base, "jkhlgG0$q/nNvVyYrm', "));
    v.extend(ctrls(Layer::Base, "dufbnpt"));
    v.extend(named(
        Layer::Base,
        &[
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Esc,
            KeyCode::Enter,
            // Aliases of the page keys and of `g`/`G`.
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
        ],
    ));
    // Layer 3.
    v.extend(chars(Layer::Verb, "eop!"));
    // Layer 2: page keys are reserved on every display type.
    v.extend(chars(Layer::Type, "JKO"));
    match context {
        "global" => {}
        "tree" => v.extend(chars(Layer::Type, "dCaA.i")),
        "text" | "document" => {
            v.extend(named(Layer::Type, &[KeyCode::Tab, KeyCode::BackTab]));
            v.extend(chars(Layer::Type, "R+-=f"));
        }
        "image" => v.extend(chars(Layer::Type, "R+-=")),
        "table" => v.extend(chars(Layer::Type, "R")),
        "diff" => v.extend(chars(Layer::Type, "sR")),
        // "list" and every plugin screen.
        _ => {
            // `Tab`/`Shift-Tab` walk the version-control screens (the host's).
            v.extend(named(Layer::Type, &[KeyCode::Tab, KeyCode::BackTab]));
            v.extend(chars(Layer::Type, "R"));
        }
    }
    let mut seen = Vec::new();
    v.retain(|(k, _)| {
        if seen.contains(k) {
            false
        } else {
            seen.push(*k);
            true
        }
    });
    v
}

/// Keys that must not be a default: they cannot be told apart from other keys on many terminals
/// (`Ctrl-j` is Enter, `Ctrl-m` is Enter, `Ctrl-i` is Tab, `Ctrl-h` is Backspace, `Ctrl-[` is Esc)
/// or are taken by the terminal (`Ctrl-s`, `Ctrl-q`).
pub fn forbidden_defaults() -> Vec<KeySpec> {
    "jmih[sq".chars().map(KeySpec::ctrl).collect()
}

/// What happened to an action because of an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The action is registered without its key (it stays in the `Space` menu).
    KeyDropped,
    /// The action cannot be registered.
    Rejected,
}

/// What is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyIssueKind {
    /// The key string does not parse (or is a chord).
    Unparseable(KeyError),
    /// The key is reserved by a layer in a context.
    Reserved { layer: Layer, context: String },
    /// Another action of the same plugin has the same key in the same context.
    Duplicate { other: String, context: String },
    /// The key is on the forbidden list.
    Forbidden,
    /// A destructive action cannot be registered until a confirmation can be declared.
    DestructiveWithoutConfirm,
    /// Another action has the same id.
    DuplicateActId,
    /// A plugin cannot add single keys to the tree.
    TreeKey,
}

/// One problem found in a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyIssue {
    /// The action that is affected (the one that loses its key or is rejected).
    pub act: String,
    pub key: Option<String>,
    pub kind: KeyIssueKind,
    pub effect: Effect,
}

impl fmt::Display for KeyIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = self.key.as_deref().unwrap_or("-");
        match &self.kind {
            KeyIssueKind::Unparseable(e) => write!(f, "action `{}`: key `{key}`: {e}", self.act)?,
            KeyIssueKind::Reserved { layer, context } => write!(
                f,
                "action `{}`: key `{key}` is reserved ({layer:?}) in `{context}`",
                self.act
            )?,
            KeyIssueKind::Duplicate { other, context } => write!(
                f,
                "action `{}`: key `{key}` is already used by `{other}` in `{context}`",
                self.act
            )?,
            KeyIssueKind::Forbidden => {
                write!(f, "action `{}`: key `{key}` cannot be a default", self.act)?
            }
            KeyIssueKind::DestructiveWithoutConfirm => write!(
                f,
                "action `{}` is destructive and there is no way to declare a confirmation yet",
                self.act
            )?,
            KeyIssueKind::DuplicateActId => {
                write!(f, "action id `{}` is declared twice", self.act)?
            }
            KeyIssueKind::TreeKey => write!(
                f,
                "action `{}`: key `{key}`: a plugin cannot bind single keys in the tree",
                self.act
            )?,
        }
        match self.effect {
            Effect::KeyDropped => write!(f, " (registered without the key)"),
            Effect::Rejected => write!(f, " (rejected)"),
        }
    }
}

/// The reserved keys of the contexts an action without a context of its own lives in: the display
/// types the manifest says it shows ([`Manifest::shows`]), or every display type when it says
/// nothing.
fn reserved_for_main_view(manifest: &Manifest) -> Vec<(KeySpec, Layer)> {
    let contexts: Vec<&str> = if manifest.shows.is_empty() {
        VIEW_CONTEXTS.iter().copied().chain(["list"]).collect()
    } else {
        manifest.shows.iter().map(|k| k.context()).collect()
    };
    let mut all = Vec::new();
    for c in contexts {
        for (k, l) in reserved(c) {
            if !all.iter().any(|(x, _)| *x == k) {
                all.push((k, l));
            }
        }
    }
    all
}

/// Check the keys of a manifest. An action without a context is checked against the keys of the
/// display types the manifest [`shows`](Manifest::shows) (every display type when it does not say).
pub fn validate(manifest: &Manifest) -> Vec<KeyIssue> {
    let mut issues = Vec::new();
    let forbidden = forbidden_defaults();
    let mut seen_ids: Vec<&str> = Vec::new();
    // (context, key, act id) of keys already taken.
    let mut taken: Vec<(String, KeySpec, &str)> = Vec::new();
    for act in &manifest.actions {
        if seen_ids.contains(&act.id.as_str()) {
            issues.push(KeyIssue {
                act: act.id.clone(),
                key: act.key.clone(),
                kind: KeyIssueKind::DuplicateActId,
                effect: Effect::Rejected,
            });
            continue;
        }
        seen_ids.push(&act.id);
        if act.destructive {
            issues.push(KeyIssue {
                act: act.id.clone(),
                key: act.key.clone(),
                kind: KeyIssueKind::DestructiveWithoutConfirm,
                effect: Effect::Rejected,
            });
            continue;
        }
        if let Some(issue) = check_key(manifest, act, &forbidden, &taken) {
            issues.push(issue);
        } else if let Some(key) = &act.key {
            if let Ok(spec) = parse_key(key) {
                taken.push((context_of(act).to_string(), spec, &act.id));
            }
        }
    }
    issues
}

/// The manifest as the host registers it: an action that lost its key keeps the action and loses
/// the key (it stays in the `Space` menu), an action that is rejected is dropped. Returns the
/// issues [`validate`] found.
pub fn sanitize(manifest: &Manifest) -> (Manifest, Vec<KeyIssue>) {
    let issues = validate(manifest);
    let mut out = manifest.clone();
    // `validate` reports the later of two declarations with one id, and a destructive action,
    // as rejected: replay its order.
    let mut seen: Vec<&str> = Vec::new();
    let mut kept = Vec::new();
    for act in &manifest.actions {
        if seen.contains(&act.id.as_str()) {
            continue;
        }
        seen.push(&act.id);
        let mut act = act.clone();
        if act.destructive {
            continue;
        }
        if issues
            .iter()
            .any(|i| i.act == act.id && i.effect == Effect::KeyDropped)
        {
            act.key = None;
        }
        kept.push(act);
    }
    out.actions = kept;
    (out, issues)
}

fn context_of(act: &Act) -> &str {
    act.screen.as_deref().unwrap_or("")
}

fn check_key(
    manifest: &Manifest,
    act: &Act,
    forbidden: &[KeySpec],
    taken: &[(String, KeySpec, &str)],
) -> Option<KeyIssue> {
    let key = act.key.as_ref()?;
    let issue = |kind| {
        Some(KeyIssue {
            act: act.id.clone(),
            key: Some(key.clone()),
            kind,
            effect: Effect::KeyDropped,
        })
    };
    let spec = match parse_key(key) {
        Ok(s) => s,
        Err(e) => return issue(KeyIssueKind::Unparseable(e)),
    };
    if forbidden.contains(&spec) {
        return issue(KeyIssueKind::Forbidden);
    }
    let ctx = context_of(act);
    if ctx == "tree" {
        return issue(KeyIssueKind::TreeKey);
    }
    let table = if ctx.is_empty() {
        reserved_for_main_view(manifest)
    } else {
        reserved(ctx)
    };
    if let Some((_, layer)) = table.iter().find(|(k, _)| *k == spec) {
        let context = if ctx.is_empty() { "any" } else { ctx };
        return issue(KeyIssueKind::Reserved {
            layer: *layer,
            context: context.to_string(),
        });
    }
    if let Some((_, _, other)) = taken.iter().find(|(c, k, _)| c == ctx && *k == spec) {
        let context = if ctx.is_empty() { "main" } else { ctx };
        return issue(KeyIssueKind::Duplicate {
            other: other.to_string(),
            context: context.to_string(),
        });
    }
    None
}
