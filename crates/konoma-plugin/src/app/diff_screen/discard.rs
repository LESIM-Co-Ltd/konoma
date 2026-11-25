//! The file diff's confirmation and asynchronous discard service.
use super::*;
use konoma_host_core::keymap::CoreResolution;

pub(super) struct Discard {
    pub(super) enabled: bool,
    keys: Vec<KeyPress>,
    global_keys: Vec<KeyPress>,
    confirming: bool,
    reply: Option<mpsc::Receiver<Result<(), Failure>>>,
    finished: bool,
}

impl Discard {
    pub(super) fn new(
        request: &FileDiffRequest,
        services: &dyn ProjectServices,
        config: &SharedConfig,
    ) -> Self {
        let mut keys = vec![KeyPress::ch('x')];
        let mut global_keys = Vec::new();
        for surface in ["preview_git_diff", "global", "diff"] {
            if let Some(table) = config.keys.surfaces.get(surface) {
                for (chord, action) in table {
                    let Ok(key) = KeyPress::parse(chord) else {
                        continue;
                    };
                    keys.retain(|candidate| *candidate != key);
                    if action == "git_diff_discard" {
                        keys.push(key);
                        if surface == "global" {
                            global_keys.push(key);
                        }
                    }
                }
            }
        }
        keys.sort_by_key(|key| konoma_host_core::help::display_order(*key));
        Self {
            keys,
            global_keys,
            enabled: services.discard_enabled()
                && request.info.backend == super::super::project::ProjectBackend::Git
                && config
                    .plugins
                    .get("git")
                    .and_then(|t| t.get("read_only"))
                    .and_then(toml::Value::as_bool)
                    != Some(true),
            confirming: false,
            reply: None,
            finished: false,
        }
    }
    pub(super) fn key(&self, keys: &CoreKeys, key: KeyPress) -> bool {
        self.enabled
            && self.keys.contains(&key)
            && (self.global_keys.contains(&key)
                || (keys.resolve(CoreContext::Diff, key) == CoreResolution::Unbound
                    && keys.host_op(CoreContext::Diff, key).is_none()
                    && !keys
                        .contexts
                        .get(&CoreContext::Diff)
                        .is_some_and(|t| t.contains_key(&key))))
    }
    pub(super) fn keys(&self, keys: &CoreKeys) -> Vec<KeyPress> {
        self.keys
            .iter()
            .copied()
            .filter(|key| self.key(keys, *key))
            .collect()
    }
    pub(super) fn confirm(&mut self) {
        self.confirming = true;
    }
    pub(super) fn confirming(&self) -> bool {
        self.confirming
    }
    pub(super) fn pending(&self) -> bool {
        self.reply.is_some()
    }
    pub(super) fn poll(&mut self) -> Result<bool, Failure> {
        let Some(reply) = &self.reply else {
            return Ok(false);
        };
        match reply.try_recv() {
            Ok(result) => {
                self.reply = None;
                result?;
                self.finished = true;
                Ok(true)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.reply = None;
                Err(Failure::external("discard worker disconnected"))
            }
        }
    }
}

impl DiffScreenState {
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn settle_discard_for_test(&mut self) {
        if let Some(reply) = self.discard.reply.take() {
            let result = reply
                .recv_timeout(Duration::from_secs(30))
                .expect("project idle fence: discard reply");
            match result {
                Ok(()) => self.discard.finished = true,
                Err(error) => self.discard_failure(error),
            }
        }
    }
    pub(super) fn discard_failure(&mut self, error: Failure) {
        self.notice = Some(format!(
            "{}: {}",
            self.word("failed", "失敗"),
            error
                .message
                .get(if self.lang == Lang::Jp { "ja" } else { "en" })
        ));
    }
    pub(in crate::app) fn take_discarded(&mut self) -> bool {
        std::mem::take(&mut self.discard.finished)
    }
    pub(super) fn handle_discard_key(
        &mut self,
        key: KeyPress,
    ) -> Result<(DiffScreenAction, Option<String>), Failure> {
        match key.code {
            crossterm::event::KeyCode::Char('n' | 'N') | crossterm::event::KeyCode::Esc => {
                self.discard.confirming = false;
                Ok((
                    DiffScreenAction::Stay,
                    Some(self.word("canceled", "取消").into()),
                ))
            }
            crossterm::event::KeyCode::Char('y' | 'Y') => {
                self.discard.confirming = false;
                let services = self.services.clone();
                let request = self.request.clone();
                let (tx, rx) = mpsc::channel();
                std::thread::Builder::new()
                    .name("file-discard".into())
                    .spawn(move || {
                        let _ = tx.send(services.discard_file(&request.info, &request.path));
                    })
                    .map_err(|e| Failure::external(e.to_string()))?;
                self.discard.reply = Some(rx);
                Ok((DiffScreenAction::Stay, None))
            }
            _ => Ok((DiffScreenAction::Stay, None)),
        }
    }
    pub(in crate::app) fn draw_discard(&self, buf: &mut Buffer, area: Rect) {
        use ratatui::{
            style::{Color, Stylize},
            text::Line,
        };
        if !self.discard.confirming {
            return;
        }
        let name = self
            .request
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        konoma_widgets::dialog::draw_dialog_box(
            buf,
            area,
            vec![
                Line::raw(format!(
                    "{} {name} ?",
                    self.word("Discard changes to", "変更を破棄しますか:")
                ))
                .bold(),
                Line::raw(""),
                Line::raw(self.word("y = yes    n / Esc = no", "y = 実行    n / Esc = 取消")).dim(),
            ],
            self.word(" Confirm ", " 確認 "),
            Color::Red,
            0,
        );
    }
}
