use super::*;

impl App {
    /// Whether any inline Markdown image is still loading — a remote download in flight, a decode not yet
    /// finished, or an encode in flight (used by the run loop to keep ticking so results are applied
    /// promptly and the image appears without waiting for the next key press).
    pub fn md_images_loading(&self) -> bool {
        !self.md_remote_inflight.is_empty()
            || self
                .md_image_cache
                .values()
                .any(|e| (e.decoded.is_none() && !e.failed) || e.enc_inflight)
    }

    /// Drop tx and the image state on exit (to terminate the worker thread).
    pub fn detach_image_backend(&mut self) {
        self.clear_image();
        self.img_tx = None;
    }

    /// Decode a still image and place it in image_src (the ThreadProtocol is built asynchronously at render time).
    /// Still images (PNG/JPG) decode fast, so this stays synchronous. On decode failure / no backend, image_src=None.
    pub(super) fn load_image(&mut self, path: &Path) {
        if self.picker.is_none() || self.img_tx.is_none() {
            return; // バックエンド無し: 描画側がテキストにフォールバック
        }
        let Some(dyn_img) = crate::preview::image::decode_static(path) else {
            return;
        };
        self.set_static_image(dyn_img);
    }

    /// Common processing to set a still image (including SVG raster results / single-frame GIFs) as the display source.
    /// zoom/center are left untouched (enter_preview has the preceding clear_image set defaults, and tab restore has already
    /// overwritten them with the restored values; so that a late asynchronous result does not break the restored zoom/center).
    pub(super) fn set_static_image(&mut self, img: image::DynamicImage) {
        self.image_src = Some(std::sync::Arc::new(img));
        self.image_crop = None; // 次の描画(prepare_image)でプロトコルを構築させる
                                // kitty 経路の「再ビルドせよ」合図。ズームでなく**ピクセルだけ差し替わる**(PDF ページ送り /
                                // 動画の再サムネ)経路は crop も表示セルも不変=`kitty_want` が変わらず、前ラスタが居残る。
                                // ここで want/shown を無効化し在り得る in-flight を陳腐化(gen bump)＝差し替えは必ず再ビルドされる。
                                // (SVG reraster は crop_rect 変化で別途再ビルドされ、fresh/reload は clear_image 経由で二重でも無害。)
        self.kitty_gen = self.kitty_gen.wrapping_add(1);
        self.kitty_want = None;
        self.kitty_shown = None;
    }

    /// Common processing to set all GIF frames into the display state (used from both the synchronous and asynchronous paths).
    /// zoom/center are left untouched for the same reason as set_static_image.
    pub(super) fn set_gif_frames(
        &mut self,
        frames: Vec<(image::DynamicImage, std::time::Duration)>,
    ) {
        self.gif_frames = frames;
        self.gif_idx = 0;
        self.gif_shown_at = None; // 最初の tick で計時開始
        self.gif_protocol = None;
        self.gif_proto_key = None;
        self.image_crop = None;
    }

    /// Start loading image-type media according to the preview kind (from enter_preview / tab restore).
    /// Still images are synchronous, while **heavy SVG rasterization / GIF full-frame decode are offloaded to a separate thread**
    /// (to start display fast without blocking the UI thread). With no media_tx (tests, etc.), a synchronous fallback is used.
    pub(super) fn start_media_load(&mut self, kind: &PreviewKind, path: &Path) {
        // メディア自動再読込の基準時刻を記録(reload_media_if_changed が mtime 比較に使う)。
        self.preview_media_mtime = file_mtime(path);
        match kind {
            PreviewKind::Image(_) if Self::looks_like_gif(path) => {
                self.spawn_or_sync_media(MediaJob::Gif(path.to_path_buf()))
            }
            // 静止画(PNG/JPG 等)はデコードが速いので同期のまま(エンコードは描画時に worker が非同期化)。
            PreviewKind::Image(_) => self.load_image(path),
            PreviewKind::Svg(_) => {
                self.spawn_or_sync_media(MediaJob::Svg(path.to_path_buf(), self.cfg.ui.svg_max_px))
            }
            // `[external] video = false`: never spawn the job (never runs ffmpegthumbnailer/ffmpeg).
            // `image_src` stays None, so the render side falls back exactly like "tool not installed"
            // (VideoThumbUnavailable hint) — no new UI state needed.
            PreviewKind::Video(_) if self.cfg.external.video => {
                self.spawn_or_sync_media(MediaJob::Video(path.to_path_buf()))
            }
            PreviewKind::Video(_) => {}
            // PDF: always spawn the job — `hayro` (the primary renderer, `preview::pdf::render_page`)
            // is pure Rust and never touches an external process, so it must work regardless of
            // `[external] pdf`. That flag now only controls whether the job's *fallback* chain
            // (pdftocairo/pdftoppm/qlmanage/sips, tried only if hayro itself fails) may run.
            PreviewKind::Pdf(_) => self.spawn_or_sync_media(MediaJob::Pdf(
                path.to_path_buf(),
                self.tab.pdf_page,
                self.cfg.external.pdf,
            )),
            // 単体 .mmd/.mermaid: 画像モードなら純 Rust で SVG 化→ラスタライズ(別スレッド)。
            // テキストモード/バックエンド無しは何もしない=装飾テキスト経路が描く(原則#3)。
            PreviewKind::Mermaid(_) if self.mermaid_image_mode() => {
                self.spawn_or_sync_media(MediaJob::Mermaid(
                    path.to_path_buf(),
                    self.mermaid_px(),
                    self.cfg.ui.mermaid_theme.clone(),
                ))
            }
            // 全画面フェンス表示: md からフェンス本文を序数で取り直す(count-guard=コードコピーと同型)。
            PreviewKind::MermaidFence(ord) => {
                if let Some(code) = self.mermaid_fence_code(path, *ord) {
                    self.spawn_or_sync_media(MediaJob::MermaidSrc(
                        code,
                        self.mermaid_px(),
                        self.cfg.ui.mermaid_theme.clone(),
                    ));
                }
            }
            _ => {}
        }
    }

    /// Whether mermaid renders as an image: config says so **and** the terminal has an image backend.
    /// Everything else (config `"text"`, no backend, render failure) degrades to the text diagram.
    pub fn mermaid_image_mode(&self) -> bool {
        self.cfg.ui.mermaid != "text" && self.picker.is_some()
    }

    /// Base raster target (max edge px) for mermaid diagrams — same knob as SVG previews.
    pub(super) fn mermaid_px(&self) -> u32 {
        self.cfg.ui.svg_max_px
    }

    /// Whether math ($…$ / $$…$$) renders as an image (`[ui] math` != "text" and a graphics backend).
    /// Off → math stays as raw LaTeX inline (the delimiters are left in the text).
    pub fn math_image_mode(&self) -> bool {
        self.cfg.ui.math != "text" && self.picker.is_some()
    }

    /// Raster target (max edge px) for a math equation. Generous so the once-rendered raster stays
    /// crisp when the encode worker fits it into the (usually small) reserved cell area.
    pub(super) fn math_px(&self) -> u32 {
        1024
    }

    /// Validated `[ui] mermaid_rows`: max rows of an inline diagram (0/invalid → default 24).
    fn mermaid_rows_cap(&self) -> u16 {
        match self.cfg.ui.mermaid_rows {
            0 => 24,
            v => v,
        }
    }

    /// Effective target rows for an inline diagram: the `mermaid_rows` cap, shrunk so the whole
    /// fence block (caption + diagram + bottom margin) fits the preview viewport — the initial
    /// view shows the entire diagram without scrolling (fit-to-view). Viewport 0 (no render yet /
    /// tests without a render pass) keeps the cap.
    pub(super) fn mermaid_fit_rows(&self) -> u16 {
        let cap = self.mermaid_rows_cap();
        if self.tab.preview_viewport == 0 {
            return cap;
        }
        cap.min(self.tab.preview_viewport.saturating_sub(2)).max(4)
    }

    /// The Nth ```mermaid fence body of the Markdown file at `md`, re-extracted fresh so an external
    /// edit between focusing and opening can't render a stale diagram (None = gone/shifted).
    pub(super) fn mermaid_fence_code(&self, md: &Path, ord: usize) -> Option<String> {
        let src = std::fs::read_to_string(md).ok()?;
        crate::preview::markdown::collect_mermaid_fences(&src)
            .into_iter()
            .nth(ord)
    }

    /// Run a media-load job on a separate thread (when media_tx is present). Otherwise run it synchronously.
    /// If there is no backend (picker), do nothing (the render side falls back).
    fn spawn_or_sync_media(&mut self, job: MediaJob) {
        if self.picker.is_none() {
            return; // 端末非対応: 描画側がテキスト/メッセージへフォールバック
        }
        let Some(tx) = self.media_tx.clone() else {
            // 同期フォールバック(テスト/チャネル未装着)。
            if let Some(payload) = job.run() {
                self.apply_payload(payload);
            }
            return;
        };
        self.media_gen = self.media_gen.wrapping_add(1);
        self.media_loading = true;
        let gen = self.media_gen;
        std::thread::spawn(move || {
            // job.run() が panic(resvg ラスタ/画像デコードの病的入力)してもスレッドを殺さず
            // 必ず結果を返す: 返さないと media_loading が次のプレビュー遷移まで true 固着し、
            // 「Loading…」表示のまま run ループが 16ms ポーリングを続ける(アイドル 0% が崩れる)。
            let payload = crate::preview::markdown::catch_silent(move || job.run()).flatten();
            let _ = tx.send(MediaResult { gen, payload });
        });
    }

    /// Apply a media-load result from another thread. Stale results (from after moving to another file) are discarded.
    /// Returns true if the state changes from applying / staleness judgment (the caller re-renders).
    pub fn apply_media(&mut self, result: MediaResult) -> bool {
        if result.gen != self.media_gen {
            return false; // 陳腐化: 既に別ファイルへ移っている
        }
        self.media_loading = false;
        // 現世代の結果が届いた=再ラスタの inflight は解消(失敗 None でも解除して詰まらせない)。
        self.vector_reraster_inflight = false;
        match result.payload {
            Some(payload) => {
                self.apply_payload(payload);
                true
            }
            None => true, // 失敗: loading を解除し描画側のフォールバック(生XML/メッセージ)へ
        }
    }

    fn apply_payload(&mut self, payload: MediaPayload) {
        match payload {
            MediaPayload::Static(img) => self.set_static_image(img),
            MediaPayload::Gif(frames) => self.set_gif_frames(frames),
            MediaPayload::Vector { img, svg } => {
                use image::GenericImageView;
                // 初回ラスタ到着=このサイズが論理(レイアウト)サイズ。以降のシャープ再ラスタは
                // ピクセル密度だけを差し替える(clear_image が logical を消す=別ファイルでリセット)。
                if self.image_logical.is_none() {
                    self.image_logical = Some(img.dimensions());
                }
                self.vector_svg = Some(svg);
                self.set_static_image(img);
                // ジョブ実行中にズームがさらに進んでいたら次の 1 本を出して収束させる
                // (足りていれば/上限 4096 なら NotNeeded 側で即 return)。
                self.maybe_sharpen_vector();
            }
        }
    }

    /// Whether waiting on another thread's media load (used by the render side to decide on the "Loading…" display).
    pub fn is_media_loading(&self) -> bool {
        self.media_loading
    }

    /// Decide whether it is a GIF by the leading bytes (looks at the magic, not the extension).
    pub(super) fn looks_like_gif(path: &Path) -> bool {
        use std::io::Read;
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut head = [0u8; 6];
        if f.read_exact(&mut head).is_err() {
            return false;
        }
        &head[..4] == b"GIF8" // GIF87a / GIF89a
    }

    /// Drive the GIF animation. If the current frame's display time has elapsed, advance to the next frame index.
    /// The actual re-encode is done by prepare_gif on the next render (detecting the gif_idx change).
    /// Returns true if advanced (the caller re-renders). Always false if not a GIF.
    pub fn advance_gif_if_due(&mut self) -> bool {
        if self.gif_frames.len() < 2 {
            return false;
        }
        let now = std::time::Instant::now();
        let Some(shown_at) = self.gif_shown_at else {
            // 最初の tick: 計時を開始するだけ(先頭フレームは表示済み)。
            self.gif_shown_at = Some(now);
            return false;
        };
        let delay = self.gif_frames[self.gif_idx].1;
        if now.duration_since(shown_at) < delay {
            return false;
        }
        self.gif_idx = (self.gif_idx + 1) % self.gif_frames.len();
        self.gif_shown_at = Some(now);
        true
    }

    /// Whether a GIF animation is active (used for branching in the render path and footer/zoom checks).
    pub fn is_gif_active(&self) -> bool {
        self.gif_frames.len() >= 2
    }

    /// The render protocol of the current GIF frame synchronously encoded to the display size (referenced by the render side).
    pub fn gif_protocol(&self) -> Option<&Protocol> {
        self.gif_protocol.as_ref()
    }

    /// Call just before rendering (for GIF). Compute the display rectangle target and crop from the current (gif_idx, zoom, center, inner), and
    /// if the frame/crop has changed, **synchronously encode** the current frame and swap it into gif_protocol.
    /// Being synchronous, "render an unencoded protocol → empty" does not happen, and the frame switches atomically (no churn).
    /// The return value is the render-target target. None when there is no backend / size 0.
    pub fn prepare_gif(&mut self, inner: Rect) -> Option<Rect> {
        let picker = self.picker.as_ref()?;
        let scale = self.cfg.ui.image_render_scale;
        let src = &self.gif_frames.get(self.gif_idx)?.0;
        let ImageLayout {
            target,
            crop_rect,
            center,
            frac,
        } = image_layout(
            src,
            picker.font_size(),
            self.tab.image_zoom,
            self.tab.image_center,
            inner,
            scale,
            None, // GIF はラスタ実寸のまま(ベクタ由来でない)
        )?;
        let key = (self.gif_idx, crop_rect);
        // フレーム or crop(ズーム/パン/リサイズ)が変わったときだけ再エンコード。
        let built = if self.gif_proto_key != Some(key) {
            let (x0, y0, cw, ch) = crop_rect;
            let crop = src.crop_imm(x0, y0, cw, ch);
            let size = ratatui::layout::Size::new(target.width.max(1), target.height.max(1));
            // src/picker の借用はこの式で完結(結果は所有値)→以降 self を変更可。
            // GIF は UI スレッドで毎フレーム同期エンコードするため、軽量で滑らかな Triangle(bilinear) を使う
            // (最近傍 None だとアニメ各フレームがジャギる。最高品質の Lanczos3 は静止画側で使用)。
            Some(picker.new_protocol(crop, size, Resize::Scale(Some(FilterType::Triangle))))
        } else {
            None
        };
        if let Some(res) = built {
            match res {
                Ok(p) => {
                    self.gif_protocol = Some(p);
                    self.gif_proto_key = Some(key);
                }
                Err(_) => {
                    // エンコード失敗: 描画側はメッセージにフォールバック。次フレームで再試行。
                    self.gif_protocol = None;
                    self.gif_proto_key = None;
                }
            }
        }
        self.tab.image_center = center;
        self.image_vis_frac = frac;
        self.image_crop = Some(crop_rect);
        Some(target)
    }

    /// Wait time until the next frame while a GIF is playing (for the poll timeout). None when not playing.
    /// Clamped to [10ms, 100ms] for smoothness (100ms is the same as the normal idle-tick upper bound).
    pub fn gif_poll_timeout(&self) -> Option<std::time::Duration> {
        use std::time::Duration;
        if self.gif_frames.len() < 2 {
            return None;
        }
        let remaining = match self.gif_shown_at {
            None => Duration::ZERO, // まだ計時前: すぐ次の tick を回して計時開始
            Some(t) => self.gif_frames[self.gif_idx]
                .1
                .checked_sub(t.elapsed())
                .unwrap_or(Duration::ZERO),
        };
        Some(remaining.clamp(Duration::from_millis(10), Duration::from_millis(100)))
    }

    /// Whether the current preview is a (renderable) image. Used to route image-only keys (zoom/pan).
    /// Still images are judged by image_src, GIFs by gif_frames (image_src is not used). SVGs are already rasterized.
    pub fn is_image_preview(&self) -> bool {
        (self.image_src.is_some() || self.is_gif_active())
            && matches!(
                self.tab.preview_kind,
                Some(
                    PreviewKind::Image(_)
                        | PreviewKind::Svg(_)
                        | PreviewKind::Video(_)
                        | PreviewKind::Pdf(_)
                        | PreviewKind::Mermaid(_)
                        | PreviewKind::MermaidFence(_)
                )
            )
    }

    /// Whether the current preview is a PDF (used to enable page-navigation keys / footer hints).
    pub fn is_pdf_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Pdf(_)))
    }

    /// (current, total) page for the PDF preview, or None when not a PDF or the page count is
    /// unknown (page_count/hayro-syntax failed to parse the file at all — single-page fallback,
    /// navigation disabled). Used for the footer/status indicator.
    pub fn pdf_page_indicator(&self) -> Option<(u32, u32)> {
        if !self.is_pdf_preview() {
            return None;
        }
        let total = self.tab.pdf_pages?;
        Some((self.tab.pdf_page.min(total), total))
    }

    /// Whether PDF page navigation is possible (a known multi-page PDF). The page count comes from
    /// `page_count` (`hayro-syntax`, pure Rust, no poppler needed), and rendering any given page is
    /// `hayro`'s job too — poppler is only consulted as a fallback when `hayro` itself fails, so
    /// navigation is no longer coupled to poppler being installed.
    pub fn pdf_can_navigate(&self) -> bool {
        matches!(self.tab.pdf_pages, Some(n) if n > 1)
    }

    /// Go to the next PDF page (clamped to the last page). Re-rasterizes that page on demand (one at a time).
    pub fn pdf_next_page(&mut self) {
        self.pdf_goto(self.tab.pdf_page.saturating_add(1));
    }

    /// Go to the previous PDF page (clamped to page 1).
    pub fn pdf_prev_page(&mut self) {
        self.pdf_goto(self.tab.pdf_page.saturating_sub(1));
    }

    /// Jump to a 1-based page (clamped to [1, total]). No-op if not a navigable PDF or already there.
    /// Kicks off an off-thread rasterization of the new page and resets the view to fit (each page shows whole).
    fn pdf_goto(&mut self, page: u32) {
        if !self.pdf_can_navigate() {
            return;
        }
        let Some(total) = self.tab.pdf_pages else {
            return;
        };
        let page = page.clamp(1, total);
        if page == self.tab.pdf_page {
            return;
        }
        self.tab.pdf_page = page;
        // 新ページは全体が見えるよう fit に戻す(set_static_image は zoom/center を触らないので明示リセット)。
        self.tab.image_zoom = 1.0;
        self.tab.image_center = (0.5, 0.5);
        self.image_crop = None;
        if let Some(PreviewKind::Pdf(p)) = self.tab.preview_kind.clone() {
            // 旧ページの画像は到着まで表示したまま(media_gen で陳腐化判定・スピナーが重畳)。
            self.spawn_or_sync_media(MediaJob::Pdf(p, self.tab.pdf_page, self.cfg.external.pdf));
        }
    }

    /// Zoom (multiply the magnification by `factor`; clamped to 1.0–16.0). The actual crop is applied at render time.
    /// With no full-screen image showing, `+`/`-` instead zoom the **focused inline mermaid diagram**
    /// in place (the reserved area in the document never changes — the zoom crops within it).
    pub fn image_zoom_by(&mut self, factor: f64) {
        if self.image_src.is_none() && !self.is_gif_active() {
            if self.focused_mermaid_ordinal().is_some() {
                self.tab.fence_zoom = (self.tab.fence_zoom * factor).clamp(1.0, 16.0);
            }
            return;
        }
        self.tab.image_zoom = (self.tab.image_zoom * factor).clamp(1.0, 16.0);
        self.maybe_sharpen_vector();
    }

    /// Sharp zoom for vector-backed previews (SVG / mermaid): when the zoom outgrows the current
    /// raster's density, re-rasterize the retained SVG at the needed max-edge px on a worker thread
    /// and swap it in on arrival (latest-wins via media_gen). The pixel zoom shows instantly in the
    /// meantime, so this behaves like a map app: briefly soft, then crisp. The geometry never moves
    /// because `image_layout` works in the logical size (`image_logical`), not the raster size.
    /// Memory stays bounded by the rasterizer's HARD_MAX (4096px side ≈ 64 MiB RGBA).
    pub(super) fn maybe_sharpen_vector(&mut self) {
        use image::GenericImageView;
        // 1 本だけ: キーリピートの `+` 連打(ジョブ完了前に必要密度が伸び続ける)で同じ再ラスタを
        // 多重 spawn しない(1 本 ~数百 ms・過渡 ~128MiB)。到着時(apply_media→apply_payload)に
        // もう一度この関数が走り、その時点の最新ズームへ収束する。
        if self.vector_reraster_inflight {
            return;
        }
        let (Some(svg), Some(src), Some((lw, lh))) =
            (&self.vector_svg, &self.image_src, self.image_logical)
        else {
            return;
        };
        let cur_side = src.dimensions().0.max(src.dimensions().1);
        let want = ((lw.max(lh) as f64) * self.tab.image_zoom).ceil() as u32;
        // 現ラスタで足りている(+12% マージン)か、既に上限(4096)なら何もしない。
        if want <= cur_side + cur_side / 8 || cur_side >= 4096 {
            return;
        }
        let base = self.tab.preview_path.clone().unwrap_or_default();
        // 同期フォールバック(media_tx 無し=テスト)は spawn しない=多重の余地が無いので立てない。
        if self.media_tx.is_some() {
            self.vector_reraster_inflight = true;
        }
        self.spawn_or_sync_media(MediaJob::SvgReraster(svg.clone(), base, want));
    }

    /// Reset to 1x (fit). Zoom=1 and recenter. Applies to the focused inline diagram when no
    /// full-screen image is showing (same dual role as `image_zoom_by`).
    pub fn image_zoom_reset(&mut self) {
        if self.image_src.is_none() && !self.is_gif_active() {
            if self.focused_mermaid_ordinal().is_some() {
                self.tab.fence_zoom = 1.0;
                self.tab.fence_center = (0.5, 0.5);
            }
            return;
        }
        self.tab.image_zoom = 1.0;
        self.tab.image_center = (0.5, 0.5);
    }

    /// Current in-place zoom of the focused inline diagram (renderer/footer cue).
    pub fn fence_zoom_level(&self) -> f64 {
        self.tab.fence_zoom
    }

    /// Whether the focused inline diagram's reserved block is fully visible in the preview viewport
    /// (mirrors the renderer's in-place-zoom condition: `row_off == 0 && vis_rows >= rows`).
    /// Viewport 0 (no render pass yet — tests) counts as visible.
    fn focused_fence_fully_visible(&self) -> bool {
        let Some(ord) = self.focused_mermaid_ordinal() else {
            return false;
        };
        let Some((line, rows)) = self.mermaid_placement(ord) else {
            return false;
        };
        let vh = self.tab.preview_viewport as usize;
        if vh == 0 {
            return true;
        }
        let (top, _) = self.md_visual_span(line);
        let scroll = self.tab.preview_scroll as usize;
        top >= scroll && top + rows as usize <= scroll + vh
    }

    /// hjkl/arrows while an inline diagram is focused **and zoomed**: pan the diagram instead of
    /// scrolling the document; `0` fits (zoom reset — the image-view key). Returns true when
    /// consumed. At 1x nothing is consumed, so all keys scroll/navigate the document as usual.
    /// The diagram must also be on screen: after zooming and paging away (Ctrl-f/G), hjkl/j/k
    /// would otherwise pan an invisible diagram and the keys would appear dead.
    pub fn fence_pan_motion(&mut self, m: crate::keymap::Motion) -> bool {
        use crate::keymap::Motion as M;
        if self.tab.fence_zoom <= 1.001 || !self.focused_fence_fully_visible() {
            return false;
        }
        // 0 = フィット(全画面画像と同じキー)。ズーム中のみ奪う(等倍では従来の行頭のまま)。
        if matches!(m, M::LineHome) {
            self.tab.fence_zoom = 1.0;
            self.tab.fence_center = (0.5, 0.5);
            return true;
        }
        let (dx, dy) = match m {
            M::Left => (-1.0, 0.0),
            M::Right => (1.0, 0.0),
            M::Up => (0.0, -1.0),
            M::Down => (0.0, 1.0),
            _ => return false,
        };
        // 1ステップ=可視窓の 1/4(全画面画像のパンと同じ感覚)。端のクランプは描画側の crop 計算。
        let f = 1.0 / self.tab.fence_zoom;
        self.tab.fence_center.0 = (self.tab.fence_center.0 + dx * f * 0.25).clamp(0.0, 1.0);
        self.tab.fence_center.1 = (self.tab.fence_center.1 + dy * f * 0.25).clamp(0.0, 1.0);
        true
    }

    /// Pan. dx/dy are directions (-1/0/+1). Only clipped axes move the center, scaled by the visible fraction.
    /// Clamping is done at render time (prepare_image), looking at the visible fraction.
    pub fn image_pan(&mut self, dx: f64, dy: f64) {
        if self.image_src.is_none() && !self.is_gif_active() {
            return;
        }
        let (fw, fh) = self.image_vis_frac;
        // 1回で可視窓の約25%。見切れていない軸(frac>=1)は動かしても描画時にクランプされる。
        self.tab.image_center.0 += dx * 0.25 * fw;
        self.tab.image_center.1 += dy * 0.25 * fh;
    }

    /// Call just before rendering. From the current (zoom, center, display area inner), compute the image's display rectangle target
    /// (centered; z=1=fit, grows when zooming and clips once it exceeds the viewport) and the source-image crop of
    /// the visible portion; rebuild the protocol if the crop changed. The return value is the render-target target.
    /// Realizes "fit the render area to the image size (fit) → grow when zooming → clip + pan once exceeded".
    pub fn prepare_image(&mut self, inner: Rect) -> Option<Rect> {
        let src = self.image_src.as_ref()?;
        let picker = self.picker.as_ref()?;
        let scale = self.cfg.ui.image_render_scale;
        let ImageLayout {
            target,
            crop_rect,
            center,
            frac,
        } = image_layout(
            src,
            picker.font_size(),
            self.tab.image_zoom,
            self.tab.image_center,
            inner,
            scale,
            self.image_logical,
        )?;
        let crop_changed = self.image_crop != Some(crop_rect);
        if self.use_kitty {
            // konoma-native compressed transmit. Request a rebuild only when the wanted geometry —
            // the crop (zoom/pan) or the display cell size (a terminal resize at fit) — changes;
            // otherwise the current/in-flight image already targets it, so a static image just
            // re-emits cheap placeholders every frame. The cell-size part matters because at fit zoom
            // a resize changes the area without the crop, and ratatui-image re-encodes on area change
            // internally, so konoma's path must too.
            let want = (crop_rect, target.width, target.height);
            if self.kitty_want != Some(want) {
                self.kitty_want = Some(want);
                self.request_kitty_build(crop_rect, target, picker.font_size());
            }
        } else {
            // ratatui-image path (sixel/iterm2/halfblocks, or kitty when disabled): rebuild the
            // ThreadProtocol only on crop change to avoid a per-frame re-encode.
            let new_tp = if crop_changed {
                let (x0, y0, cw, ch) = crop_rect;
                let crop = src.crop_imm(x0, y0, cw, ch);
                let proto = picker.new_resize_protocol(crop);
                Some(ThreadProtocol::new(
                    self.img_tx.as_ref()?.clone(),
                    Some(proto),
                ))
            } else {
                None
            };
            if let Some(tp) = new_tp {
                self.image = Some(tp);
            }
        }
        self.tab.image_center = center;
        self.image_vis_frac = frac;
        self.image_crop = Some(crop_rect);
        Some(target)
    }

    /// Build the compressed kitty image for the given crop/target. The **first** build after opening
    /// a file (or after a failure left `kitty_image` None) runs synchronously so the image appears
    /// without a blank frame. Subsequent builds (zoom/pan/resize) run on a worker thread so rapid
    /// input does not hitch — the previous image keeps showing until the new one arrives (`apply_kitty`).
    /// With no `kitty_tx` (tests), everything runs synchronously.
    fn request_kitty_build(
        &mut self,
        crop_rect: (u32, u32, u32, u32),
        target: Rect,
        font: ratatui_image::FontSize,
    ) {
        let Some(src) = self.image_src.clone() else {
            return; // no source yet → render falls back
        };
        let (cols, rows) = (target.width, target.height);
        let is_tmux = self.kitty_is_tmux;
        // Every build bumps the generation so a stale async result is discarded by `apply_kitty`.
        self.kitty_gen = self.kitty_gen.wrapping_add(1);

        // Synchronous: the very first build (nothing to show yet) or when there is no worker channel.
        if self.kitty_image.is_none() || self.kitty_tx.is_none() {
            self.kitty_image = crate::preview::kitty::build_from_source(
                &src, crop_rect, cols, rows, font, is_tmux,
            );
            self.kitty_shown = self.kitty_want; // now showing the wanted geometry
            return;
        }

        // Async: keep the previous image visible; the newest spawn's result wins.
        let tx = self.kitty_tx.clone().unwrap();
        let gen = self.kitty_gen;
        std::thread::spawn(move || {
            // A pathological image must not kill the worker (else nothing is ever sent and the
            // previous image lingers); catch and report a failed build instead.
            let image = crate::preview::markdown::catch_silent(move || {
                crate::preview::kitty::build_from_source(&src, crop_rect, cols, rows, font, is_tmux)
            })
            .flatten();
            let _ = tx.send(KittyResult { gen, image });
        });
    }

    /// Apply a completed async kitty build. Stale results (superseded by a newer zoom/pan) and failed
    /// builds are dropped, keeping whatever image is currently shown rather than blanking it.
    pub fn apply_kitty(&mut self, result: KittyResult) -> bool {
        if result.gen != self.kitty_gen {
            return false; // superseded by a newer geometry
        }
        // Either way the newest generation has now settled — mark the wanted geometry as shown so
        // `kitty_build_pending()` clears. Otherwise a failed build (None) would leave it stuck true,
        // spinning the run loop at 16 ms forever (until the next geometry change). On failure the
        // previous image simply stays; on success we swap in the new one.
        self.kitty_shown = self.kitty_want;
        match result.image {
            Some(ki) => {
                self.kitty_image = Some(ki);
                true
            }
            None => false,
        }
    }

    /// Whether an async kitty build is in flight: the wanted geometry differs from what is shown.
    /// Covers pan (same cell size, different crop). The run loop polls faster while this is true so
    /// the result is picked up promptly.
    pub fn kitty_build_pending(&self) -> bool {
        self.use_kitty && self.kitty_want != self.kitty_shown
    }

    /// Whether still images are drawn via konoma's own compressed kitty transmit (kitty terminal).
    pub fn uses_kitty_image(&self) -> bool {
        self.use_kitty
    }

    /// The prepared compressed kitty image for the current crop, if built.
    pub fn kitty_image_ref(&self) -> Option<&crate::preview::kitty::KittyImage> {
        self.kitty_image.as_ref()
    }
}
