//! konoma — a full-screen preview-focused terminal file browser.
//!
//! Pick something in the tree, then preview it full-screen. The core model is a transition
//! between two full-screen modes — Tree and Preview — with no in-between split view.
//! See `docs/PRD.md` for the specification.
//!
//! This file is the entry point: argument handling, terminal initialization, and the event
//! loop. Heavy work (image resize/encode, the git `ignored` set) is offloaded to worker
//! threads so the draw loop stays responsive; the screen is redrawn only when something changes.

mod app;
mod bookmarks;
mod config;
#[cfg(test)]
mod e2e_tests;
mod fileops;
mod git;
mod i18n;
mod keymap;
#[cfg(test)]
mod mem_tests;
mod preview;
mod session;
#[cfg(test)]
mod speed_tests;
mod ui;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui_image::errors::Errors;
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use app::{
    App, IgnoredResult, KittyResult, MdEncodeRequest, MdEncodeResult, MdImageResult, MediaResult,
    RemoteFetch, SortKey,
};
use keymap::{Action, KeyPress, Motion, Resolution, Surface};

/// Re-encode result sent from the worker to the UI.
type ResizeResult = Result<ResizeResponse, Errors>;

fn main() -> Result<()> {
    // 引数: konoma [DIR]。無ければカレント。cwd 取得失敗は panic させず ? で穏当に返す。
    let dir = match std::env::args().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => std::env::current_dir().context("カレントディレクトリを取得できません")?,
    };
    // 相対指定(例: `konoma samples`)でもパスコピーが正しい絶対パスになるよう正規化する。
    // canonicalize 失敗時は cwd と連結してフォールバック。
    let dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| cwd.join(&dir))
            .unwrap_or(dir)
    });

    let (cfg, cfg_err) = config::Config::load_reporting();

    // syntect 資産ロード＋文法の正規表現コンパイル(言語あたり1回・debug で数秒)は重い。起動と同時に
    // 別スレッドで、現在 root の拡張子を**ファイル数の多い順**に温めておき、最初のコードプレビューの
    // フリーズを無くす(間に合わなければ初回だけ通常どおり待つ)。単一スレッド＋yield で他処理を妨げない。
    // ハイライト無効(ui.syntax_highlight=false)なら syntect を一切使わないのでウォーム不要。
    if cfg.ui.syntax_highlight {
        let root = dir.clone();
        std::thread::spawn(move || preview::code::warm_dir(root));
    }

    let mut terminal = ratatui::init();
    // ブラケットペーストを有効化: ファイルのドラッグ&ドロップ(端末がパスをペーストとして渡す)を
    // `Event::Paste` として受け、コピー/移動ダイアログに繋ぐため。非対応端末では無視され無害。
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);

    // 画像バックエンド: 端末問い合わせ(kitty 等の検出)。alt screen 入室後・イベント読取前に1回。
    // 失敗(非対応端末等)してもプレビュー以外は動くよう Option で握る。
    let picker = Picker::from_query_stdio().ok();

    // 画像が出せる端末なら、SVG のシステムフォント列挙(初回 ~数十ms)を起動の裏で先に温める
    // (初回 SVG 表示時の UI フリーズを隠す。構文ハイライトの warm_dir と同じ方針)。
    // mermaid 画像モードならレンダラ側の遅延フォント計測も同様に温める(初回図表示の数十ms を隠す)。
    if picker.is_some() {
        std::thread::spawn(preview::svg::warm_fontdb);
        if cfg.ui.mermaid != "text" {
            std::thread::spawn(preview::markdown::warm_mermaid);
        }
    }

    // リサイズ/エンコードのオフロード用チャネルとワーカースレッド。
    let (req_tx, req_rx) = unbounded_channel::<ResizeRequest>();
    let (resp_tx, resp_rx) = unbounded_channel::<ResizeResult>();
    let worker = std::thread::spawn(move || resize_worker(req_rx, resp_tx));

    // 重いメディア(SVG ラスタライズ / GIF 全フレームデコード)を別スレッドで読み、結果を run ループへ。
    let (media_tx, media_rx) = std::sync::mpsc::channel::<MediaResult>();
    // kitty 画像の resize+圧縮(ズーム/パン)を別スレッドで行い、結果を run ループへ(初回は同期)。
    let (kitty_tx, kitty_rx) = std::sync::mpsc::channel::<KittyResult>();
    // インライン Markdown 画像のデコードを別スレッドで行い、結果を run ループへ。
    let (md_img_tx, md_img_rx) = std::sync::mpsc::channel::<MdImageResult>();
    // リモート(http(s)) Markdown 画像の curl ダウンロード完了を run ループへ通知する。
    let (md_remote_tx, md_remote_rx) = std::sync::mpsc::channel::<RemoteFetch>();
    // インライン Markdown 画像のエンコード(リサイズ+プロトコル)を専用ワーカーへ逃がす(UI を塞がない)。
    let (md_enc_tx, md_enc_worker_rx) = std::sync::mpsc::channel::<MdEncodeRequest>();
    let (md_enc_res_tx, md_enc_res_rx) = std::sync::mpsc::channel::<MdEncodeResult>();
    // 重い git ignored(無視セット・大規模 repo で ~800ms)を別スレッドで計算し、結果を run ループへ。
    let (ignored_tx, ignored_rx) = std::sync::mpsc::channel::<IgnoredResult>();

    let start_dir = dir.clone();
    let mut app = App::new(dir, cfg)?;
    app.attach_media_loader(media_tx);
    app.attach_kitty_loader(kitty_tx);
    app.attach_md_image_loader(md_img_tx);
    app.attach_remote_md_loader(md_remote_tx);
    app.attach_git_loader(ignored_tx);
    // 設定の読み込みエラー + キーマップ衝突/無視した設定を起動時メッセージで知らせる
    // (黙って既定に戻ると気づけないため)。両方あれば結合して 1 行に出す。
    let km_report = app.keymap_report();
    app.flash = match (cfg_err, km_report) {
        (Some(a), Some(b)) => Some(format!("{a} / {b}")),
        (Some(a), None) => Some(a),
        (None, b) => b,
    };
    // インライン画像のエンコードワーカー: バックエンドがある時だけ起動する(Picker を clone して渡す)。
    // App を drop すれば md_enc_tx が消え、ワーカーの recv が Err を返して綺麗に終了する。
    if let Some(pk) = picker.clone() {
        std::thread::spawn(move || app::md_encode_worker(pk, md_enc_worker_rx, md_enc_res_tx));
        app.attach_md_encoder(md_enc_tx);
    }
    if let Some(picker) = picker {
        // tx を App に渡す(画像ごとの ThreadProtocol が clone して使う)。
        app.attach_image_backend(picker, req_tx);
    }
    // 注意: main 側に req_tx の clone を残さない。App を drop すれば全 Sender が消え、
    // ワーカーの recv が None を返して綺麗に終了できる。

    // タブセッション([ui] restore_tabs): 前回この起動ディレクトリで開いていたタブ構成を復元する。
    // 各ローダ/画像バックエンドを繋いだ後に行う(復元でプレビューを開き直すとメディアジョブが飛ぶため)。
    app.attach_session_store(session::SessionStore::load(&start_dir));
    app.restore_session();

    let result = run(
        &mut terminal,
        &mut app,
        resp_rx,
        WorkerRx {
            media: media_rx,
            kitty: kitty_rx,
            md_img: md_img_rx,
            md_remote: md_remote_rx,
            md_enc: md_enc_res_rx,
            ignored: ignored_rx,
        },
    );

    // 終了時にタブセッションを保存する(終了時点の最新状態が確定形。restore_tabs=false なら no-op)。
    app.save_session();

    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();

    // App を畳んで Sender を全て落とし、ワーカーを終了させる。
    app.detach_image_backend();
    drop(app);
    let _ = worker.join();

    result
}

/// Handle resize requests on a current-thread tokio runtime on a dedicated thread.
/// `resize_encode()` is CPU-heavy, but running it on this dedicated thread keeps the UI thread unblocked.
fn resize_worker(mut rx: UnboundedReceiver<ResizeRequest>, tx: UnboundedSender<ResizeResult>) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
        return;
    };
    rt.block_on(async move {
        while let Some(req) = rx.recv().await {
            // 1要求 = 1エンコード。完了を UI へ返す(UI が try_recv で反映)。
            if tx.send(req.resize_encode()).is_err() {
                break; // UI 側が終了
            }
        }
    });
}

/// Classify the paths from an fs event. Returns `(meaningful, ignore_rules_changed)`:
/// - `meaningful`: whether reloading is worthwhile. Any non-empty event is meaningful — **including
///   `.git/*.lock` churn from an external git operation**. konoma's own git reads take no locks (CLI
///   reads pass `--no-optional-locks`; git2 diffs never set `update_index`), so a `.git` lock event can
///   only come from an *external* git op (a commit/amend/reset by the user or an AI agent) whose result
///   we must reflect. (Historically we swallowed lock-only events to break a self-feedback loop in which
///   konoma's own `git status` created `.git/index.lock` → the watcher picked it up → git ran again.
///   `--no-optional-locks` (2026-07-07) removed that loop at the source, so swallowing now only *hides*
///   external commits — e.g. the stale change marker seen after an agent commits while you preview a file.)
/// - `ignore_rules_changed`: whether `.gitignore` or `.git/info/exclude` changed (= the heavy
///   ignore set must be rebuilt). For other changes, `ignored` is left alone and only the cheap `statuses` is updated.
fn classify_fs_paths(paths: &[PathBuf]) -> (bool, bool) {
    if paths.is_empty() {
        return (true, false); // パス不明のイベントは安全側(再読込する・ignored は触らない)
    }
    // どのパス変更も再読込に値する(バーストは run ループが1回にまとめる)。無視ルールが
    // 変わったイベントが含まれる時だけ、重い ignored セットの作り直しも要求する。
    let ignore_rules = paths.iter().any(|p| is_ignore_rule_file(p));
    (true, ignore_rules)
}

/// Paths from an fs event that follow mode may jump to: anything not under a `.git` directory
/// (repository internals — index/refs/lock churn — are never review targets). Existence/kind/root
/// checks happen later in `App::follow_jump` (the event may be a deletion, or outside the tree).
fn follow_candidates(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|p| !p.components().any(|c| c.as_os_str() == ".git"))
        .cloned()
        .collect()
}

/// Whether `p` is a file that holds ignore rules (a change requires recomputing the ignore set):
/// the `.gitignore` itself, or `.git/info/exclude`. A `.gitignore` under `node_modules` is excluded
/// because it sits inside a wholly-ignored tree and does not affect git's decision (avoiding a wasteful recompute).
fn is_ignore_rule_file(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        ".gitignore" => !p.components().any(|c| c.as_os_str() == "node_modules"),
        "exclude" => {
            // .git/info/exclude（親=info, 祖父=.git）
            p.parent()
                .and_then(|pp| pp.file_name())
                .and_then(|n| n.to_str())
                == Some("info")
                && p.parent()
                    .and_then(|pp| pp.parent())
                    .and_then(|g| g.file_name())
                    .and_then(|n| n.to_str())
                    == Some(".git")
        }
        _ => false,
    }
}

/// Background-worker result receivers drained each iteration of the run loop.
struct WorkerRx {
    media: std::sync::mpsc::Receiver<MediaResult>,
    kitty: std::sync::mpsc::Receiver<KittyResult>,
    md_img: std::sync::mpsc::Receiver<MdImageResult>,
    md_remote: std::sync::mpsc::Receiver<RemoteFetch>,
    md_enc: std::sync::mpsc::Receiver<MdEncodeResult>,
    ignored: std::sync::mpsc::Receiver<IgnoredResult>,
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut resp_rx: UnboundedReceiver<ResizeResult>,
    rx: WorkerRx,
) -> Result<()> {
    // ファイル監視: 現在の root 配下の変更でツリー/git status を自動更新する。
    // notify のコールバックは別スレッドから呼ばれるので、変更を std チャネルで run ループへ送る。
    // チャネルの bool = 「無視ルール(.gitignore / .git/info/exclude)が変わったか」。
    // 外部 git 操作の `.git/*.lock` churn も再読込対象(classify_fs_paths。konoma はロックフリー
    // なので自己フィードバックループは起きない)。
    // チャネルの Vec<PathBuf> = フォローモード用の変更パス候補(.git 配下を除く)。
    let (fs_tx, fs_rx) = std::sync::mpsc::channel::<(bool, Vec<PathBuf>)>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let (meaningful, ignore_rules) = classify_fs_paths(&ev.paths);
            if meaningful {
                let _ = fs_tx.send((ignore_rules, follow_candidates(&ev.paths)));
            }
        }
    })
    .ok();
    let mut watched_root: Option<PathBuf> = None;
    rewatch(watcher.as_mut(), &mut watched_root, &app.root);
    // Secondary watch: a file shown outside the tree root (a global-bookmark preview, or the repo-wide
    // git view when the root is a repo subdirectory) is not covered by the recursive root watch, so its
    // external/agent edits would never refresh. Watch its directory too (see `App::out_of_root_watch_dir`).
    let mut watched_extra: Option<PathBuf> = None;
    // root が repo のサブディレクトリのときは親の `.git` が再帰監視の外に出る。外部 git 操作
    // (作業ファイルを触らない commit / 外部 checkout)を拾えるよう `.git` を非再帰監視する
    // (`App::git_dir_watch`)。root が repo root のときは再帰監視に含まれるので None。
    let mut watched_git: Option<PathBuf> = None;

    // ローディング: 裏でコード文法をウォームし、完了をこのチャネルで run ループへ通知。
    // indicator/progressive とも UI を止めず(スピナーが回り続ける)、完了で着色版に差し替える。
    let (hl_tx, hl_rx) = std::sync::mpsc::channel::<()>();

    // フォローモードの切替 debounce: エージェントが複数ファイルを高速に書き替えても、
    // ビューの切替(ファイル跨ぎのジャンプ)は最短この間隔＝画面が跳ね回らず一瞥できる。
    // 保留中の最新ターゲット(latest-wins)は dwell 明けに拾う。同一ファイルの再読込は無制限(別経路)。
    const FOLLOW_MIN_DWELL: Duration = Duration::from_millis(1000);
    let mut pending_follow: Option<PathBuf> = None;
    let mut last_follow_jump: Option<std::time::Instant> = None;

    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            terminal.draw(|frame| ui::render(frame, app))?;
            needs_redraw = false;
            // インライン画像(kitty unicode placeholder)の表示位置が動いたフレームは、端末を
            // フル再描画して旧位置の残骸を掃除する。placeholder 行は「画像 ID を前景色に符号化
            // した文字列」を端末グリッドへ直接印字するが、ratatui の差分描画は空白→空白を
            // 再送しないため、スクロール等で画像が動くと旧 ID 行が色付きバーとして取り残される
            // (Ghostty 実機で青/ピンクのバーとして報告・ID が変わる毎に色も変わる)。
            if app.take_md_overlay_moved() {
                terminal.clear()?;
                terminal.draw(|frame| ui::render(frame, app))?;
            }
        }

        // 重いコードハイライト待ち(cold な言語の初回): 文法コンパイルを別スレッドへ逃がし UI を止めない。
        // indicator は中央スピナーを回し、progressive は素テキストを表示したまま、完了で着色に差し替え。
        if let Some((ext, path)) = app.take_warm_job() {
            let tx = hl_tx.clone();
            std::thread::spawn(move || {
                preview::code::warm_file(&ext, &path);
                let _ = tx.send(());
            });
        }

        // 入力待ち(タイムアウト付き)。タイムアウト中もワーカー結果やファイル変更を拾えるようにする。
        // キー長押し対策: 1イベント=1描画だと、描画が重いとき入力が溜まり「離した後もスクロール
        // し続ける」。保留中のイベントを**一括で処理してから 1 回だけ描画**する(最終状態へ収束)。
        // GIF 再生中は次フレーム期限まで(≤100ms)で起き、滑らかにコマ送りする。
        // 別スレッドのメディア読み込み待ちの間も、結果を即反映できるようこまめに起きる。
        let poll_timeout =
            if app.is_media_loading() || app.md_images_loading() || app.kitty_build_pending() {
                // 別スレッド待ち(メディア/kitty ビルド)の間はこまめに起きて結果を即反映する。
                Duration::from_millis(16)
            } else {
                app.gif_poll_timeout().unwrap_or(Duration::from_millis(100))
            };
        if event::poll(poll_timeout)? {
            let mut quit = false;
            loop {
                let ev = event::read()?;
                match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // キー処理中の回復可能な失敗(fs/git 操作: refresh/rebuild_tree/paste/
                        // delete/rename 等)で TUI を落とさない(設計原則#3「クラッシュさせない」)。
                        // handle_key は端末制御も描画も初期化もせず app 状態を変えるだけなので、
                        // ここで返る Err はすべて回復可能。`?` で run() ごと終了させず、握り潰さず
                        // flash でユーザに見せて継続する(真に致命的な描画/端末制御の `?` は run 本体に残す)。
                        let res = handle_key(app, key);
                        if resolve_key_result(app, res) {
                            quit = true;
                            break;
                        }
                        needs_redraw = true;
                    }
                    Event::Resize(_, _) => needs_redraw = true,
                    // ファイルの D&D はターミナルがパスをペーストとして渡す → 受けてコピー/移動へ。
                    // ただしダイアログ/モーダル/オーバーレイ表示中は横入りさせず無視する (#13)。
                    Event::Paste(s) if paste_accepted(app.surface()) => {
                        app.handle_paste(s);
                        needs_redraw = true;
                    }
                    _ => {}
                }
                // まだ即時に取れるイベントがあれば続けて処理(無ければ抜けて描画へ)。
                if !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
            if quit {
                break;
            }
        }

        // 外部エディタ要求(`e`): TUI を退避して同期起動し、終了後に復帰＋プレビュー再読込。
        // プレビューから開いた時は表示中の行(line)でエディタを開く。
        if let Some((path, line)) = app.take_pending_edit() {
            run_editor(terminal, app, &path, line)?;
            needs_redraw = true;
        }

        // 外部 git ツール要求(`O`): 設定 git.tool(既定 lazygit)を repo workdir で同期起動。
        if app.take_launch_git_tool() {
            run_git_tool(terminal, app)?;
            // 外部ツールが作業ツリー/git 状態を変えた可能性 → 一覧・git status・派生ビューを取り直す
            // (run_editor 復帰が reload_preview するのと対称。Git ビューの git_view_entries 陳腐化 #4 を防ぐ)。
            let _ = app.refresh();
            needs_redraw = true;
        }

        // ウォーム完了: 着色版に差し替えるため再描画(grammar は warm 済で即時)。
        while hl_rx.try_recv().is_ok() {
            app.clear_highlight_pending();
            needs_redraw = true;
        }

        // 中央スピナー表示中(コードハイライト待ち / SVG・GIF の別スレッド読み込み中)は、
        // 待機ティックごとにコマを進めて回す。読み込み中は poll が 16ms なので滑らかに回る。
        if (app.is_highlight_pending() && app.loading_is_indicator())
            || app.is_media_loading()
            || app.busy_indicator_active()
        {
            app.tick_spinner();
            needs_redraw = true;
        }

        // ワーカーからの再エンコード結果を反映(複数あれば全部)。
        while let Ok(resp) = resp_rx.try_recv() {
            if app.apply_image_resize(resp) {
                needs_redraw = true;
            }
        }

        // 別スレッドのメディア読み込み(SVG/GIF)完了を反映(複数あれば全部・古い世代は破棄)。
        while let Ok(result) = rx.media.try_recv() {
            if app.apply_media(result) {
                needs_redraw = true;
            }
        }

        // 別スレッドの kitty 画像ビルド(ズーム/パンの resize+圧縮)完了を反映(最新世代のみ適用)。
        while let Ok(result) = rx.kitty.try_recv() {
            if app.apply_kitty(result) {
                needs_redraw = true;
            }
        }

        // インライン Markdown 画像のデコード完了を反映(複数あれば全部)。
        while let Ok(result) = rx.md_img.try_recv() {
            if app.apply_md_image(result) {
                needs_redraw = true;
            }
        }

        // リモート Markdown 画像のダウンロード完了を反映(md_cache を無効化して再レイアウト)。
        while let Ok(result) = rx.md_remote.try_recv() {
            if app.apply_remote_fetch(result) {
                needs_redraw = true;
            }
        }

        // インライン Markdown 画像のエンコード完了(別ワーカー)を反映(複数あれば全部)。
        while let Ok(result) = rx.md_enc.try_recv() {
            if app.apply_md_encode(result) {
                needs_redraw = true;
            }
        }

        // 別スレッドの git ignored(無視セット)計算完了を反映(複数あれば全部・古い世代は破棄)。
        // 反映で暗転表示(gitignore 除外の dim)が現れるため再描画する。
        while let Ok(result) = rx.ignored.try_recv() {
            if app.apply_ignored(result) {
                needs_redraw = true;
            }
        }

        // GIF アニメ: 現フレームの表示時間が過ぎていれば次フレームへ進める(image_src 差し替え→
        // 次の描画で prepare_image が再構築→ワーカー再エンコード→上の分岐で反映)。
        if app.advance_gif_if_due() {
            needs_redraw = true;
        }

        // ファイル変更を拾って自動更新(バースト時は1回にまとめる)。
        // 無視ルール(.gitignore / .git/info/exclude)が変わったイベントが1つでもあれば、
        // 重い ignored セットも作り直す。それ以外は安い statuses+branch だけ更新する。
        let mut fs_changed = false;
        let mut ignore_rules_changed = false;
        // このバーストで変わったパス(監視側が `.git` 配下を除いて送ってくる)。空 = `.git` のみ
        // または不明で、その場合 `refresh_fs_changed` は安全側=プレビューも再読込する。
        let mut changed_paths: Vec<PathBuf> = Vec::new();
        while let Ok((b, paths)) = fs_rx.try_recv() {
            fs_changed = true;
            ignore_rules_changed |= b;
            // フォローモード: 有効ターゲットをセッション一覧(n/N レビューの母集合)へ記録しつつ、
            // 最後の1つを保留ターゲットに(latest-wins)。無効パスは dwell 枠を消費しない。
            for p in paths {
                if app.follow_enabled() && app.follow_note_change(&p) {
                    pending_follow = Some(p.clone());
                }
                if !changed_paths.contains(&p) {
                    changed_paths.push(p);
                }
            }
        }
        if fs_changed {
            // 一覧 + git status に加え、refresh_fs() がアクティブな派生ビューも一元的に取り直す
            // (Preview モードなら現プレビューを再読込 → 外部エディタでの編集でプレビューが古いまま
            //  残る既知バグを解消。Git ビューなら変更一覧を更新)。
            let _ = app.refresh_fs_changed(ignore_rules_changed, &changed_paths);
            needs_redraw = true;
        }
        // フォローの切替判定(ドレイン外=毎ループ)。dwell 中に来たターゲットは保留され、明けた時点の
        // 最新へ 1 回だけ跳ぶ。表示中ファイルへの変更は切替不要(上の refresh_fs の再読込が追従)。
        if pending_follow.is_some() && !app.follow_enabled() {
            pending_follow = None; // 解除されたら保留も破棄
        }
        if let Some(p) = pending_follow.clone() {
            if app.preview_path.as_deref() == Some(p.as_path()) {
                pending_follow = None;
            } else if last_follow_jump.is_none_or(|t| t.elapsed() >= FOLLOW_MIN_DWELL) {
                pending_follow = None;
                app.follow_jump(&p);
                last_follow_jump = Some(std::time::Instant::now());
                needs_redraw = true;
            }
        }

        // root が変わったら監視先を張り替える(h/l/タブ切替など)。
        if watched_root.as_deref() != Some(app.root.as_path()) {
            rewatch(watcher.as_mut(), &mut watched_root, &app.root);
        }
        // root 外に表示中のファイル(ブックマーク先/repo 全体の git ビュー)はその親ディレクトリも監視する。
        // 表示ファイルが変わる/root 内へ戻る/ツリーへ戻ると None になり監視は外れる。
        let want_extra = app.out_of_root_watch_dir();
        if watched_extra.as_deref() != want_extra.as_deref() {
            set_extra_watch(watcher.as_mut(), &mut watched_extra, want_extra.as_deref());
        }
        // サブディレクトリ root のとき、親 repo の `.git` を非再帰監視して外部 git 操作を拾う。
        let want_git = app.git_dir_watch();
        if watched_git.as_deref() != want_git.as_deref() {
            set_extra_watch(watcher.as_mut(), &mut watched_git, want_git.as_deref());
        }
    }
    Ok(())
}

/// Re-point the watcher at `root`. Failure is not fatal (auto-refresh just stops working).
fn rewatch(
    watcher: Option<&mut notify::RecommendedWatcher>,
    watched: &mut Option<PathBuf>,
    root: &std::path::Path,
) {
    use notify::{RecursiveMode, Watcher};
    let Some(w) = watcher else {
        return;
    };
    if let Some(old) = watched.take() {
        let _ = w.unwatch(&old);
    }
    if w.watch(root, RecursiveMode::Recursive).is_ok() {
        *watched = Some(root.to_path_buf());
    }
}

/// Add/replace/remove the **secondary, non-recursive** watch for a file shown outside the tree root
/// (`App::out_of_root_watch_dir`). `want=None` removes it. Non-recursive so it never overlaps the
/// recursive root watch (no duplicate events). Failure is non-fatal (that file just won't auto-refresh).
fn set_extra_watch(
    watcher: Option<&mut notify::RecommendedWatcher>,
    watched: &mut Option<PathBuf>,
    want: Option<&std::path::Path>,
) {
    use notify::{RecursiveMode, Watcher};
    let Some(w) = watcher else {
        return;
    };
    if let Some(old) = watched.take() {
        let _ = w.unwatch(&old);
    }
    if let Some(dir) = want {
        if w.watch(dir, RecursiveMode::NonRecursive).is_ok() {
            *watched = Some(dir.to_path_buf());
        }
    }
}

/// Edit `path` in an external editor. Temporarily suspends the TUI (drops raw/alt screen), runs the
/// editor **synchronously**, then restores the screen on return. The command is resolved from the
/// `[editor]` config (per-extension → default → $VISUAL → $EDITOR → vim). Both terminal-grabbing TUI
/// editors (vim etc.) and GUI editors (`code -w`) take the same path (both block and wait).
///
/// On return, it **explicitly resets the terminal input modes an editor (especially vim) may leave
/// enabled** (kitty keyboard protocol / bracketed paste / mouse reporting), and further **drains and
/// discards leftover input bytes (terminal-query responses, etc.)** to recover input-decoder sync.
/// Without this, keys can become garbled or stop working after return (ratatui::init assumes legacy
/// input, so we realign the state).
fn run_editor(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    path: &std::path::Path,
    line: Option<usize>,
) -> Result<()> {
    let argv = app.cfg.editor.resolve(path, line);
    let Some((prog, args)) = argv.split_first() else {
        return Ok(());
    };
    let cwd = path.parent().unwrap_or(path).to_path_buf();
    let status = run_external(terminal, app, prog, args, &cwd)?;
    match status {
        Ok(_) => app.reload_preview(), // 書き換え結果を反映(キャッシュ破棄＋ウィンドウ開き直し)
        Err(e) => {
            app.flash = Some(format!(
                "{}{e}",
                i18n::tr(app.lang, crate::i18n::Msg::EditorFailed)
            ))
        }
    }
    Ok(())
}

/// Launch the configured `git.tool` (default lazygit) synchronously in the repo workdir. The command
/// is split on whitespace into prog + args. The cwd is the workdir discovered from app.root via git
/// (or root if none). A launch failure (not installed, etc.) is reported as an error via flash.
fn run_git_tool(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let tmpl = app.cfg.git.tool.trim();
    let tmpl = if tmpl.is_empty() { "lazygit" } else { tmpl };
    let mut parts = tmpl.split_whitespace().map(|s| s.to_string());
    let Some(prog) = parts.next() else {
        return Ok(());
    };
    let args: Vec<String> = parts.collect();
    let cwd = git_workdir(&app.root);
    let status = run_external(terminal, app, &prog, &args, &cwd)?;
    if let Err(e) = status {
        app.flash = Some(format!(
            "{}{prog}: {e}",
            i18n::tr(app.lang, crate::i18n::Msg::GitToolFailed)
        ));
    }
    Ok(())
}

/// Return the workdir of the repo containing `root` (when the git feature is enabled). Returns root itself if none is found.
#[cfg(feature = "git")]
fn git_workdir(root: &std::path::Path) -> PathBuf {
    git2::Repository::discover(root)
        .ok()
        .and_then(|r| r.workdir().map(|w| w.to_path_buf()))
        .unwrap_or_else(|| root.to_path_buf())
}

#[cfg(not(feature = "git"))]
fn git_workdir(root: &std::path::Path) -> PathBuf {
    root.to_path_buf()
}

/// Generic helper that launches an arbitrary external command **synchronously** in `cwd`. Suspends the
/// TUI → launches → restores, returning the process exit status (an io::Error on launch failure). Shared
/// by the editor and git tool. See `run_editor` for suspend/restore details (alternate screen, input-mode reset, draining leftover bytes).
fn run_external(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    prog: &str,
    args: &[String],
    cwd: &std::path::Path,
) -> Result<std::io::Result<std::process::ExitStatus>> {
    use crossterm::event::{DisableMouseCapture, PopKeyboardEnhancementFlags};
    use crossterm::execute;
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
        EnterAlternateScreen,
    };

    // --- TUI を退避 ---
    // 代替画面は**抜けない**(raw モード解除のみ)。vim 等は自分で代替画面に入る(smcup)が、
    // 端末は既に alt なので画面切替が起きず、起動時に素のターミナル画面が一瞬見える現象を防ぐ
    // (Ratatui フォーラムの定石)。vim は終了時に必ず rmcup でプライマリへ戻すため、復帰側で
    // EnterAlternateScreen して入り直す。
    disable_raw_mode()?;

    // --- 外部コマンドを同期実行(stdio を継承してブロック) ---
    let status = std::process::Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status();

    // --- TUI を復帰 ---
    // 代替画面へ戻った直後に即再描画し、さらに「切替＋再描画」を同期出力(DEC 2026)で原子的に反映する。
    // これで外部ツール終了→konoma 復帰の間に素のターミナル画面/空画面が一瞬見える現象を無くす(対応端末のみ。
    // 非対応端末では ?2026 が無視され、即再描画だけが効く＝無害)。clear/draw は装飾的なので best-effort
    // にして、エラー時でも EndSynchronizedUpdate を必ず送る(端末が同期待ちで固まらないように)。
    enable_raw_mode()?;
    let _ = execute!(std::io::stdout(), BeginSynchronizedUpdate);
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let _ = terminal.clear(); // ratatui の前バッファを捨てて全描画させる(外部ツールの残骸を消す)
    let _ = terminal.draw(|frame| ui::render(frame, app)); // 復帰直後に即描画(次ループまで遅延させない)
    let _ = execute!(std::io::stdout(), EndSynchronizedUpdate);

    // 入力モードのレガシー化＋残バイトのドレインは画面復帰後でよい(不可視)。
    // kitty キーボードプロトコルを pop し、(外部ツールが残した)マウス報告を無効化する。
    // ブラケットペーストは konoma 本体が使う(D&D 受信)ので、掃除後に**有効化し直す**。
    let _ = execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    );
    // 残った入力(端末問い合わせ応答・余分なキー)を短時間ドレインして読み捨て、デコーダの同期を回復。
    while event::poll(Duration::from_millis(10)).unwrap_or(false) {
        let _ = event::read();
    }
    Ok(status)
}

/// File operations from the Visual surface (Space→d/r/c/x …) first commit the range to the selection before running.
/// This absorbs, via the keymap, the old direct keys' (D/R/Y/X) behavior of calling `exit_visual_commit()` before acting.
fn commit_visual_if_needed(app: &mut App, sfc: Surface) {
    if sfc == Surface::Visual {
        app.exit_visual_commit();
    }
}

/// Whether the surface may accept a D&D paste (a file path the terminal delivers via Event::Paste).
/// While a dialog / confirm modal / any overlay (Help/Sort/Bookmarks/Info/Git/Visual …) is shown it returns
/// `false` to prevent interference. Text-input surfaces pass it through to capture as text, and only the basic
/// full-screen surfaces (Tree / Preview), where a drop is meaningful, accept it (actual drop handling is Tree-only inside `App::handle_paste`).
fn paste_accepted(sfc: Surface) -> bool {
    sfc.is_text_input()
        || matches!(
            sfc,
            Surface::Tree | Surface::PreviewText | Surface::PreviewImage
        )
}

/// Map `SortSet(key)` to the key char used to apply it through the existing `sort_menu_key`.
fn sort_key_char(k: SortKey) -> char {
    match k {
        SortKey::Name => 'n',
        SortKey::Size => 's',
        SortKey::Modified => 'm',
        SortKey::Ext => 'e',
    }
}

/// Map `Navigate(Motion)` to a concrete move/scroll **per surface** (§1.1).
/// A missing arm means j/k do nothing on that surface, so this interpretation table is pinned in tandem with the keymap tests.
fn dispatch_navigate(app: &mut App, sfc: Surface, m: Motion) {
    match sfc {
        Surface::Tree | Surface::Visual => match m {
            Motion::Up => app.tree_prev(),
            Motion::Down => app.tree_next(),
            Motion::Top => app.tree_first(),
            Motion::Bottom => app.tree_last(),
            Motion::PageUp => app.tree_page(-1),
            Motion::PageDown => app.tree_page(1),
            Motion::HalfUp => app.tree_half_page(-1),
            Motion::HalfDown => app.tree_half_page(1),
            Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd => {}
        },
        // 通常のテキスト/コードプレビュー、およびその行選択(visual)サブモード。
        // windowed のときは preview_scroll/to_top 等が行カーソルを動かす(視覚選択中は範囲が伸びる)。
        // フォーカス中のインライン mermaid 図を**ズーム中**は hjkl/矢印を図のパンに割り当てる
        // (等倍に戻せば通常スクロールに復帰。埋め込み地図の操作感)。
        Surface::PreviewText | Surface::PreviewTextVisual if app.fence_pan_motion(m) => {}
        Surface::PreviewText | Surface::PreviewTextVisual => match m {
            Motion::Up => app.preview_scroll(-1),
            Motion::Down => app.preview_scroll(1),
            Motion::Top => app.preview_to_top(),
            Motion::Bottom => app.preview_to_bottom(),
            Motion::PageUp => app.preview_page(-1),
            Motion::PageDown => app.preview_page(1),
            Motion::HalfUp => app.preview_half_page(-1),
            Motion::HalfDown => app.preview_half_page(1),
            Motion::Left => app.preview_col_move(-1),
            Motion::Right => app.preview_col_move(1),
            Motion::LineHome => app.preview_col_home(),
            Motion::LineEnd => app.preview_col_end(),
        },
        Surface::PreviewImage => match m {
            Motion::Up => app.image_pan(0.0, -1.0),
            Motion::Down => app.image_pan(0.0, 1.0),
            Motion::Left => app.image_pan(-1.0, 0.0),
            Motion::Right => app.image_pan(1.0, 0.0),
            _ => {}
        },
        // テーブル(csv/tsv): hjkl=セルカーソル移動 / g・G=先頭末尾行 / 0・$=先頭末尾列 / ページ送り。
        Surface::PreviewTable => match m {
            Motion::Up => app.table_cursor_move(-1, 0),
            Motion::Down => app.table_cursor_move(1, 0),
            Motion::Left => app.table_cursor_move(0, -1),
            Motion::Right => app.table_cursor_move(0, 1),
            Motion::Top => app.table_row_to(false),
            Motion::Bottom => app.table_row_to(true),
            Motion::PageUp => app.table_page(-1),
            Motion::PageDown => app.table_page(1),
            Motion::HalfUp => app.table_half_page(-1),
            Motion::HalfDown => app.table_half_page(1),
            Motion::LineHome => app.table_col_to(false),
            Motion::LineEnd => app.table_col_to(true),
        },
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => match m {
            Motion::Up => app.preview_scroll(-1),
            Motion::Down => app.preview_scroll(1),
            Motion::Top => app.preview_to_top(),
            Motion::Bottom => app.preview_to_bottom(),
            Motion::PageUp => app.preview_page(-1),
            Motion::PageDown => app.preview_page(1),
            Motion::HalfUp => app.preview_half_page(-1),
            Motion::HalfDown => app.preview_half_page(1),
            Motion::Left => app.preview_hscroll(-4),
            Motion::Right => app.preview_hscroll(4),
            Motion::LineHome => app.preview_hscroll_home(),
            Motion::LineEnd => app.preview_hscroll_end(),
        },
        #[cfg(feature = "git")]
        Surface::GitDetail => match m {
            Motion::Up => app.git_detail_scroll_by(-1),
            Motion::Down => app.git_detail_scroll_by(1),
            Motion::Top => app.git_detail_scroll_to(false),
            Motion::Bottom => app.git_detail_scroll_to(true),
            Motion::PageUp => app.git_detail_scroll_by(-20),
            Motion::PageDown => app.git_detail_scroll_by(20),
            Motion::HalfUp => app.git_detail_scroll_by(-10),
            Motion::HalfDown => app.git_detail_scroll_by(10),
            Motion::Left => app.git_detail_hscroll_by(-4),
            Motion::Right => app.git_detail_hscroll_by(4),
            Motion::LineHome => app.git_detail_hscroll_home(),
            Motion::LineEnd => app.git_detail_hscroll_end(),
        },
        #[cfg(feature = "git")]
        Surface::GitChanges => match m {
            Motion::Up => app.git_view_move(-1),
            Motion::Down => app.git_view_move(1),
            Motion::Top => app.git_view_move(i32::MIN),
            Motion::Bottom => app.git_view_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitLog => match m {
            Motion::Up => app.git_log_move(-1),
            Motion::Down => app.git_log_move(1),
            Motion::Top => app.git_log_move(i32::MIN),
            Motion::Bottom => app.git_log_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitGraph => match m {
            Motion::Up => app.git_graph_move(-1),
            Motion::Down => app.git_graph_move(1),
            Motion::Top => app.git_graph_move(i32::MIN),
            Motion::Bottom => app.git_graph_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => match m {
            Motion::Up => app.git_graph_picker_move(-1),
            Motion::Down => app.git_graph_picker_move(1),
            Motion::Top => app.git_graph_picker_jump(true),
            Motion::Bottom => app.git_graph_picker_jump(false),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitBranches => match m {
            Motion::Up => app.git_branch_move(-1),
            Motion::Down => app.git_branch_move(1),
            Motion::Top => app.git_branch_move(i32::MIN),
            Motion::Bottom => app.git_branch_move(i32::MAX),
            _ => {}
        },
        Surface::Bookmarks => match m {
            // 旧挙動同様 j/k のみ (g/G/Home/End は割当無し)。
            Motion::Up => app.bookmark_list_move(-1),
            Motion::Down => app.bookmark_list_move(1),
            _ => {}
        },
        Surface::Tabs => match m {
            Motion::Up => app.tab_list_move(-1),
            Motion::Down => app.tab_list_move(1),
            _ => {}
        },
        Surface::Outline => match m {
            Motion::Up => app.outline_move(-1),
            Motion::Down => app.outline_move(1),
            Motion::Top => {
                let d = -(app.outline_sel() as i32);
                app.outline_move(d);
            }
            Motion::Bottom => {
                let d = (app.md_outline().len() as i32 - 1) - app.outline_sel() as i32;
                app.outline_move(d);
            }
            _ => {}
        },
        Surface::Help => match m {
            Motion::Up => app.help_scroll_by(-1),
            Motion::Down => app.help_scroll_by(1),
            Motion::Top => app.help_scroll = 0,
            Motion::Bottom => app.help_scroll = u16::MAX,
            _ => {}
        },
        // Sort / Info / 固定テキスト入力面など: Navigate は来ない (j/k が別意味 or 非 keymap)。
        _ => {}
    }
}

/// Central dispatch that runs a resolved `Action`. Only `Quit` requests exit, returning `Ok(true)`.
/// Surface-dependent behavior (motion amount, the q/Esc return target) branches on `sfc` (§5).
fn dispatch_action(app: &mut App, action: Action, sfc: Surface) -> Result<bool> {
    match action {
        Action::Noop => {}
        Action::Navigate(m) => dispatch_navigate(app, sfc, m),
        Action::TabNew => app.tab_new()?,
        Action::TabClose => app.tab_close(),
        Action::TabPrev => app.tab_cycle(-1),
        Action::TabNext => app.tab_cycle(1),
        Action::TabGoto(i) => app.tab_goto(i as usize),
        Action::ToggleHelp => app.toggle_help(),
        Action::CopyPath(kind) => app.copy_path(kind),
        Action::CopyCodeBlock => app.md_copy_focused_code(),
        Action::PasteJump => app.paste_jump(),
        Action::Quit => {
            // confirm_quit が ON なら確認ダイアログを開いてまだ終了しない。OFF なら即終了。
            if app.request_quit() {
                return Ok(false);
            }
            return Ok(true);
        }
        Action::CloseTabOrQuit => {
            // タブが複数あれば現在タブを閉じる。最後の1つなら通常の終了フロー(Q と同じ)。
            if app.tab_count() > 1 {
                app.tab_close();
            } else if app.request_quit() {
                return Ok(false);
            } else {
                return Ok(true);
            }
        }
        Action::FilterStart => app.start_filter(),
        Action::TreeDescend => app.tree_descend()?,
        Action::TreeActivate => app.tree_activate()?,
        Action::TreeLeave => app.tree_leave()?,
        Action::ToggleHidden => app.toggle_hidden()?,
        Action::ToggleInfo => app.toggle_info(),
        Action::RequestEdit => app.request_edit(),
        Action::OpenGitView => app.open_git_view(),
        Action::Refresh => app.refresh()?,
        Action::CyclePathStyle => app.cycle_path_style(),
        Action::OpenSortMenu => app.open_sort_menu(),
        Action::MarkSet => app.start_mark_set(),
        // `'`: 従来の不可視の「ジャンプ待ち」でなく、即ブックマーク一覧を開く(which-key 流)。
        Action::MarkJump => app.open_bookmark_list(),
        Action::SetAnchor => app.reanchor_root(),
        Action::ResetAnchor => app.reset_anchor(),
        Action::OpenGitDiffCursor => app.tree_open_git_diff(),
        Action::EnterVisual => app.enter_visual(),
        Action::ToggleSelect => app.toggle_select(),
        Action::ToggleChangedFilter => app.toggle_changed_filter(),
        Action::JumpNextChange => app.jump_changed(1),
        Action::JumpPrevChange => app.jump_changed(-1),
        Action::ToggleFollow => app.toggle_follow(),
        Action::FileCreate => {
            commit_visual_if_needed(app, sfc);
            app.start_create();
        }
        Action::FileRename => {
            // ビジュアル経由は範囲を確定してから。選択ありは一括(連番テンプレ)・無しは単体。
            commit_visual_if_needed(app, sfc);
            if app.has_selection() {
                app.start_batch_rename();
            } else {
                app.start_rename();
            }
        }
        Action::FileDelete => {
            commit_visual_if_needed(app, sfc);
            app.start_delete();
        }
        Action::FileCopy => {
            commit_visual_if_needed(app, sfc);
            app.copy_selection();
        }
        Action::FileCut => {
            commit_visual_if_needed(app, sfc);
            app.cut_selection();
        }
        Action::FilePaste => {
            commit_visual_if_needed(app, sfc);
            app.paste()?;
        }
        Action::FileDuplicate => {
            commit_visual_if_needed(app, sfc);
            app.duplicate_selection()?;
        }
        Action::VisualCommit => app.exit_visual_commit(),
        Action::VisualSelectSiblings => app.visual_select_scope(false),
        Action::VisualSelectAll => app.visual_select_scope(true),
        Action::PreviewBack => {
            // git diff プレビューは Git ビューへ戻す。それ以外はツリーへ。
            if app.is_git_diff_preview() {
                app.close_git_diff();
            } else {
                app.back_to_tree();
            }
        }
        Action::SearchStart => app.start_search(),
        Action::SearchNext => app.search_next(1),
        Action::SearchPrev => app.search_next(-1),
        Action::PreviewEnterVisual => app.preview_enter_visual(false),
        Action::PreviewEnterVisualLine => app.preview_enter_visual(true),
        Action::PreviewCopySelection => app.preview_copy_selection(),
        Action::PreviewCopySelectionRef => app.preview_copy_selection_ref(),
        Action::PreviewExitVisual => app.preview_exit_visual(),
        Action::ToggleMarkdownRaw => app.toggle_md_raw(),
        Action::LinkFocusNext => app.md_focus_move(1),
        Action::LinkFocusPrev => app.md_focus_move(-1),
        Action::LinkOpen => app.md_activate_focused()?,
        Action::OpenLinkNewTab => app.md_open_focused_link_new_tab()?,
        Action::OpenInNewTab => app.tab_new_from_selection()?,
        Action::ImageZoomIn => app.image_zoom_by(1.25),
        Action::ImageZoomOut => app.image_zoom_by(1.0 / 1.25),
        Action::ImageZoomReset => app.image_zoom_reset(),
        Action::PdfNextPage => app.pdf_next_page(),
        Action::PdfPrevPage => app.pdf_prev_page(),
        Action::PreviewFileNext => app.preview_jump_file(1),
        Action::PreviewFilePrev => app.preview_jump_file(-1),
        Action::TableCopy(kind) => app.table_copy(kind),
        Action::SortSet(k) => app.sort_menu_key(sort_key_char(k))?,
        Action::SortToggleReverse => app.sort_menu_key('r')?,
        Action::SortToggleDirsFirst => app.sort_menu_key('.')?,
        Action::ToggleTabList => app.toggle_tab_list(),
        Action::TabListClose => app.tab_list_close_selected(),
        Action::ToggleOutline => app.toggle_outline(),
        Action::BookmarkJump => app.bookmark_list_jump(),
        Action::BookmarkEdit => app.bookmark_list_edit(),
        Action::BookmarkDelete => app.bookmark_list_delete(),
        Action::BookmarkClose => app.close_bookmark_list(),
        Action::InfoClose => app.toggle_info(),
        #[cfg(feature = "git")]
        Action::GitDiffDiscard => app.git_diff_start_discard(),
        #[cfg(feature = "git")]
        Action::CycleDiffLayout => app.cycle_diff_layout(),
        #[cfg(feature = "git")]
        Action::GitStage => app.git_view_stage(),
        #[cfg(feature = "git")]
        Action::GitUnstage => app.git_view_unstage(),
        #[cfg(feature = "git")]
        Action::GitStageAll => app.git_view_stage_all(),
        #[cfg(feature = "git")]
        Action::GitUnstageAll => app.git_view_unstage_all(),
        #[cfg(feature = "git")]
        Action::GitDiscard => app.git_view_start_discard(),
        #[cfg(feature = "git")]
        Action::GitCommit => app.start_git_commit(),
        #[cfg(feature = "git")]
        Action::GitWorktreeDiff => app.open_worktree_detail(),
        #[cfg(feature = "git")]
        Action::GitOpenLog => app.open_git_log(),
        #[cfg(feature = "git")]
        Action::GitOpenGraph => app.open_git_graph(),
        #[cfg(feature = "git")]
        Action::GitOpenBranches => app.open_git_branches(),
        #[cfg(feature = "git")]
        Action::GitLaunchTool => app.launch_git_tool(),
        #[cfg(feature = "git")]
        Action::GitOpenSelectedDiff => {
            if let Some(p) = app.git_view_selected() {
                app.open_git_diff(&p);
            }
        }
        #[cfg(feature = "git")]
        Action::GitOpenDetail => match sfc {
            Surface::GitLog => app.open_git_commit_detail(),
            Surface::GitGraph => app.open_git_graph_detail(),
            _ => {}
        },
        #[cfg(feature = "git")]
        Action::GitGraphSetBase => app.git_graph_set_base(),
        #[cfg(feature = "git")]
        Action::GitGraphClearBase => app.git_graph_clear_base(),
        #[cfg(feature = "git")]
        Action::GitGraphOpenPicker => app.git_graph_open_picker(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerToggle => app.git_graph_picker_toggle(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerAll => app.git_graph_picker_all(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerCurrentOnly => app.git_graph_picker_current_only(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveUp => app.git_graph_picker_reorder(-1),
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveDown => app.git_graph_picker_reorder(1),
        #[cfg(feature = "git")]
        Action::BranchFilterStart => app.git_branch_start_filter(),
        #[cfg(feature = "git")]
        Action::BranchCheckout => app.checkout_selected_branch()?,
        #[cfg(feature = "git")]
        Action::BranchCreate => app.start_create_branch(),
        #[cfg(feature = "git")]
        Action::BranchDelete => app.start_delete_branch(),
        #[cfg(feature = "git")]
        Action::GitCopy(kind) => app.git_copy(kind),
        #[cfg(feature = "git")]
        Action::CopyBranchName => app.git_copy_branch_name(),
        #[cfg(feature = "git")]
        Action::GitClose => match sfc {
            Surface::GitDetail => app.close_git_detail(),
            Surface::GitLog => app.close_git_log(),
            Surface::GitGraphPicker => app.git_graph_picker_cancel(),
            Surface::GitGraph => app.close_git_graph(),
            Surface::GitBranches => app.close_git_branches(),
            Surface::GitChanges => app.close_git_view(),
            _ => {}
        },
    }
    Ok(false)
}

/// Key handling for text-input surfaces (filter / search / mark / branch filter / dialog input).
/// Intercepts character and editing keys and does not apply the keymap (requirement 4).
fn handle_text_input(app: &mut App, sfc: Surface, key: KeyEvent) -> Result<bool> {
    let code = key.code;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match sfc {
        Surface::DialogInput => match (code, ctrl) {
            (KeyCode::Esc, _) => app.dialog_cancel(),
            (KeyCode::Enter, _) => app.dialog_submit()?,
            (KeyCode::Backspace, _) => app.dialog_input_backspace(),
            (KeyCode::Delete, _) => app.dialog_input_delete(),
            (KeyCode::Left, _) => app.dialog_cursor_left(),
            (KeyCode::Right, _) => app.dialog_cursor_right(),
            (KeyCode::Home, _) => app.dialog_cursor_home(),
            (KeyCode::End, _) => app.dialog_cursor_end(),
            (KeyCode::Char(c), false) => app.dialog_input_push(c),
            _ => {}
        },
        Surface::Filter => match (code, ctrl) {
            (KeyCode::Esc, _) => app.filter_clear(),
            (KeyCode::Enter, _) => app.filter_commit(),
            (KeyCode::Backspace, _) => app.filter_input_backspace(),
            (KeyCode::Down, _) => app.tree_next(),
            (KeyCode::Up, _) => app.tree_prev(),
            (KeyCode::Char(c), false) => app.filter_input_push(c),
            _ => {}
        },
        Surface::Search => match (code, ctrl) {
            (KeyCode::Esc, _) => app.search_clear(),
            (KeyCode::Enter, _) => app.search_commit(),
            (KeyCode::Backspace, _) => app.search_input_backspace(),
            (KeyCode::Char(c), false) => app.search_input_push(c),
            _ => {}
        },
        Surface::Mark => match (code, ctrl) {
            (KeyCode::Esc, _) => app.cancel_mark(),
            (KeyCode::Char(c), false) => app.mark_input(c),
            _ => app.cancel_mark(),
        },
        #[cfg(feature = "git")]
        Surface::BranchFilter => match (code, ctrl) {
            (KeyCode::Esc, _) => app.git_branch_filter_clear(),
            (KeyCode::Enter, _) => app.git_branch_filter_commit(),
            (KeyCode::Backspace, _) => app.git_branch_filter_backspace(),
            (KeyCode::Down, _) => app.git_branch_move(1),
            (KeyCode::Up, _) => app.git_branch_move(-1),
            (KeyCode::Char(c), false) => app.git_branch_filter_push(c),
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

/// Key handling for confirm-modal surfaces (delete confirm / drop confirm / rename preview) (requirement 4).
fn handle_modal_confirm(app: &mut App, sfc: Surface, key: KeyEvent) -> Result<bool> {
    match sfc {
        Surface::DialogRenamePreview => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.dialog_preview_apply()?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_cancel(),
            KeyCode::Char('j') | KeyCode::Down => app.dialog_preview_scroll(1),
            KeyCode::Char('k') | KeyCode::Up => app.dialog_preview_scroll(-1),
            _ => {}
        },
        Surface::DialogConfirmDrop => match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') => app.drop_apply(false)?,
            KeyCode::Char('m') | KeyCode::Char('M') => app.drop_apply(true)?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_cancel(),
            _ => {}
        },
        Surface::DialogConfirmDelete => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.dialog_confirm(true)?,
            // `!`=完全削除(復元不可)。削除確認(allow_permanent)のときだけ受け付ける。
            KeyCode::Char('!') if app.dialog_allow_permanent() => app.dialog_delete_permanent()?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_confirm(false)?,
            _ => {}
        },
        // アプリ終了確認: y/q/Enter=終了(qq で素早く抜けられる) / n/Esc=取消。
        Surface::DialogConfirmQuit => match key.code {
            KeyCode::Char('y')
            | KeyCode::Char('Y')
            | KeyCode::Char('q')
            | KeyCode::Char('Q')
            | KeyCode::Enter => return Ok(true),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_cancel(),
            _ => {}
        },
        // ブックマーク上書き確認: y/Enter=上書き / n/Esc=取消。
        Surface::DialogConfirmBookmark => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.dialog_confirm(true)?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_confirm(false)?,
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

/// Per-surface fixed handling of Esc (clear search/filter, close overlays, cancel visual …). Always quit=false.
fn handle_esc(app: &mut App, sfc: Surface) -> bool {
    match sfc {
        Surface::Help => app.show_help = false,
        Surface::Visual => app.exit_visual_cancel(),
        Surface::Sort => app.close_sort_menu(),
        Surface::Info => app.toggle_info(),
        Surface::Bookmarks => app.close_bookmark_list(),
        Surface::Tabs => app.toggle_tab_list(),
        Surface::Outline => app.toggle_outline(),
        Surface::Tree => {
            // クエリありの Esc は絞り込み解除、無ければ選択クリア (旧 tree 挙動)。
            if app.filter_query().is_some() {
                app.filter_clear();
            } else if app.has_selection() {
                app.clear_selection();
            }
        }
        // 表も検索を持つので text/image と同じ流儀にする(以前は Esc が無反応だった＝検索の
        // 強調が消せず、ツリーにも戻れなかった)。
        Surface::PreviewText | Surface::PreviewImage | Surface::PreviewTable => {
            // 検索が効いていれば解除、無ければツリーへ戻る。
            if app.preview_search_query().is_some() {
                app.search_clear();
            } else {
                app.back_to_tree();
            }
        }
        // 行選択中の Esc は選択解除(ツリーへは戻らない)。
        Surface::PreviewTextVisual => app.preview_exit_visual(),
        #[cfg(feature = "git")]
        Surface::GitDetail => app.close_git_detail(),
        #[cfg(feature = "git")]
        Surface::GitLog => app.close_git_log(),
        #[cfg(feature = "git")]
        Surface::GitGraph => app.close_git_graph(),
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => app.git_graph_picker_cancel(),
        #[cfg(feature = "git")]
        Surface::GitBranches => {
            // クエリありの Esc は絞り込み解除、無ければ閉じる (ツリー絞り込みと同じ流儀)。
            if app.git_branch_query().is_empty() {
                app.close_git_branches();
            } else {
                app.git_branch_filter_clear();
            }
        }
        #[cfg(feature = "git")]
        Surface::GitChanges => app.close_git_view(),
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => app.close_git_diff(),
        _ => {}
    }
    false
}

/// Per-surface fixed handling of Enter (tree activate / preview link / confirm in each list).
fn handle_enter(app: &mut App, sfc: Surface) -> Result<bool> {
    match sfc {
        Surface::Tree => app.tree_activate()?,
        Surface::PreviewText => app.md_activate_focused()?,
        Surface::Bookmarks => app.bookmark_list_jump(),
        Surface::Tabs => app.tab_list_activate(),
        Surface::Outline => app.outline_jump(),
        #[cfg(feature = "git")]
        Surface::GitChanges => {
            if let Some(p) = app.git_view_selected() {
                app.open_git_diff(&p);
            }
        }
        #[cfg(feature = "git")]
        Surface::GitLog => app.open_git_commit_detail(),
        #[cfg(feature = "git")]
        Surface::GitGraph => app.open_git_graph_detail(),
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => app.git_graph_picker_apply(),
        #[cfg(feature = "git")]
        Surface::GitBranches => app.checkout_selected_branch()?,
        _ => {}
    }
    Ok(false)
}

/// Keys with a fixed meaning regardless of surface (arrows = Navigate alias / Esc / Enter / Tab/BackTab = link ops).
/// Returns `Some(quit)` when handled, or `None` if not a fixed key (falls through to keymap resolution). Requirements 3/4.
fn handle_fixed_key(app: &mut App, sfc: Surface, key: KeyEvent) -> Result<Option<bool>> {
    let done = match key.code {
        KeyCode::Up => {
            dispatch_navigate(app, sfc, Motion::Up);
            false
        }
        KeyCode::Down => {
            dispatch_navigate(app, sfc, Motion::Down);
            false
        }
        KeyCode::Left => {
            dispatch_navigate(app, sfc, Motion::Left);
            false
        }
        KeyCode::Right => {
            dispatch_navigate(app, sfc, Motion::Right);
            false
        }
        KeyCode::Home => {
            dispatch_navigate(app, sfc, Motion::Top);
            false
        }
        KeyCode::End => {
            dispatch_navigate(app, sfc, Motion::Bottom);
            false
        }
        KeyCode::Esc => handle_esc(app, sfc),
        KeyCode::Enter => handle_enter(app, sfc)?,
        // Markdown リンク/チェックボックス: Tab=次 / ⇧Tab=前 (装飾テキストプレビューのみ・
        // raw ソース表示(R)中は装飾表示のアイテムが stale なので動かさない・他面では無処理で飲む)。
        KeyCode::Tab => {
            if sfc == Surface::PreviewText && !app.is_raw_source() {
                app.md_focus_move(1);
            }
            false
        }
        KeyCode::BackTab => {
            if sfc == Surface::PreviewText && !app.is_raw_source() {
                app.md_focus_move(-1);
            }
            false
        }
        // Markdown チェックボックス: フォーカス中だけ Space=トグル。フォーカスが無ければ
        // keymap へフォールスルー(less 流儀の Space=PageDown を奪わない)。
        KeyCode::Char(' ') if sfc == Surface::PreviewText && app.md_focused_task() => {
            app.md_toggle_focused_task();
            false
        }
        // <details> の summary にフォーカス中は Space で折りたたみをトグル(Enter と同じ)。
        KeyCode::Char(' ') if sfc == Surface::PreviewText && app.md_focused_details().is_some() => {
            if let Some(ord) = app.md_focused_details() {
                app.toggle_details(ord);
            }
            false
        }
        _ => return Ok(None),
    };
    Ok(Some(done))
}

/// Key handling (Run2: keymap-driven). Returns `Ok(true)` to request exit. Kept thin, following the §5 flow:
/// fixed text input → confirm modal → which-key leader resolution → per-surface fixed keys → keymap resolution.
/// Mode-specific keys are owned by the keymap (`App::keymaps`) and `dispatch_action`.
fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    // 直前の一時メッセージ(flash)は次のキー入力で消す。
    app.flash = None;
    let sfc = app.surface();

    // フォロー中に F(ToggleFollow に解決されるキー)以外を押したら追尾を止める(Zed 流:
    // 手を置いた=閲覧、キーボードを取った=手動優先。F 一発で再開できる)。
    if app.follow_enabled() {
        let is_follow_key = !sfc.is_text_input()
            && !sfc.is_modal_confirm()
            && app.pending_leader.is_none()
            && matches!(
                app.keymaps.resolve(sfc, None, KeyPress::norm(&key)),
                Resolution::Action(Action::ToggleFollow)
            );
        if !is_follow_key {
            app.follow_break();
        }
    }

    // 1) 固定面 (テキスト入力 / 確認モーダル) は専用ハンドラへ (keymap 非適用・要件4)。
    if sfc.is_text_input() {
        return handle_text_input(app, sfc, key);
    }
    if sfc.is_modal_confirm() {
        return handle_modal_confirm(app, sfc, key);
    }

    let kp = KeyPress::norm(&key);

    // 2) which-key リーダー待ち: leaders から解決 (未知 suffix は取消してフォールスルーしない・§5)。
    if let Some(lead) = app.pending_leader.take() {
        return match app.keymaps.resolve(sfc, Some(lead), kp) {
            Resolution::Action(a) => dispatch_action(app, a, sfc),
            _ => Ok(false),
        };
    }

    // 3) 面に依らない固定キー (矢印=Navigate / Esc / Enter / Tab・BackTab) を先取り (要件3/4)。
    if let Some(done) = handle_fixed_key(app, sfc, key)? {
        return Ok(done);
    }

    // 4) keymap 解決 (1 入力 = HashMap 1〜2 参照・要件5)。
    match app.keymaps.resolve(sfc, None, kp) {
        Resolution::EnterLeader(id) => {
            // コピーリーダー(既定 `y`)は常に which-key メニューを開く。装飾 Markdown で
            // コードブロックにフォーカス中は、そのメニューに `c:code block` が現れて選べる
            // (whichkey_spans が面/フォーカスで出し分け)。従来のパスコピー n/r/f/p/@ と両立。
            app.pending_leader = Some(id);
            Ok(false)
        }
        Resolution::Action(a) => dispatch_action(app, a, sfc),
        Resolution::Unbound => {
            // ブックマーク一覧: keymap 未割当の素の英字はブックマーク名として直接ジャンプ
            // (a-z=ローカル / A-Z=グローバル)。q/j/k や global の t/T/F/Q 等は上で解決済み=対象外。
            if sfc == Surface::Bookmarks && !kp.ctrl {
                if let KeyCode::Char(c) = kp.code {
                    if c.is_ascii_alphabetic() {
                        app.bookmark_jump_letter(c);
                    }
                }
            }
            Ok(false)
        }
    }
}

/// Interpret `handle_key`'s result for the run loop. Returns `true` only when exit is requested.
///
/// Key handling only mutates app state (and does fs/git operations); it never touches terminal control,
/// drawing, or initialization, so every `Err` that reaches here is **recoverable** (e.g. a delete/paste
/// while another terminal removes the cwd makes the internal `refresh()?`/`rebuild_tree()?` fail). Rather
/// than crashing the whole run() or swallowing it, we show it to the user via flash and continue.
/// Unrecoverable failures (terminal control, drawing, initialization) are propagated with `?` in run()
/// itself (they never reach here).
fn resolve_key_result(app: &mut App, result: Result<bool>) -> bool {
    match result {
        Ok(quit) => quit,
        Err(e) => {
            app.flash = Some(format!(
                "{}{e:#}",
                i18n::tr(app.lang, crate::i18n::Msg::OperationFailed)
            ));
            false // 回復可能 → ループ継続
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::Mode;
    use config::Config;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn fs_event_classification_reacts_to_git_locks_and_detects_ignore_rules() {
        let pb = |s: &str| vec![PathBuf::from(s)];
        // `.git/*.lock` にも反応する(meaningful=true)。konoma はロックフリー(--no-optional-locks)
        // なので、これは外部 git 操作(コミット等)の合図であり、握り潰すと変更マーカーが陳腐化する。
        assert_eq!(
            classify_fs_paths(&pb("/repo/.git/index.lock")),
            (true, false)
        );
        assert_eq!(classify_fs_paths(&pb("/r/.git/HEAD.lock")), (true, false));
        // .git 本体(HEAD/refs/index)は反応する(外部 git 操作の追従)が ignored は触らない。
        assert_eq!(classify_fs_paths(&pb("/repo/.git/HEAD")), (true, false));
        assert_eq!(classify_fs_paths(&pb("/repo/.git/index")), (true, false));
        // 通常のファイル変更: 反応する・ignored は触らない。
        assert_eq!(classify_fs_paths(&pb("/repo/src/main.rs")), (true, false));
        // ユーザの *.lock(.git 外)は対象(meaningful)。
        assert_eq!(classify_fs_paths(&pb("/repo/Cargo.lock")), (true, false));
        // .gitignore / .git/info/exclude は ignored 再計算が必要(true, true)。
        assert_eq!(classify_fs_paths(&pb("/repo/.gitignore")), (true, true));
        assert_eq!(classify_fs_paths(&pb("/repo/sub/.gitignore")), (true, true));
        assert_eq!(
            classify_fs_paths(&pb("/repo/.git/info/exclude")),
            (true, true)
        );
        // node_modules 内の .gitignore は対象外(丸ごと無視されるので再計算不要)。
        assert_eq!(
            classify_fs_paths(&pb("/repo/node_modules/x/.gitignore")),
            (true, false)
        );
        // バーストに1つでも ignore ルール変更があれば true。
        assert_eq!(
            classify_fs_paths(&[
                PathBuf::from("/repo/.git/index.lock"),
                PathBuf::from("/repo/.gitignore"),
            ]),
            (true, true)
        );
        // パス不明イベントは安全側(再読込する・ignored は触らない)。
        assert_eq!(classify_fs_paths(&[]), (true, false));
    }

    #[test]
    fn follow_candidates_exclude_git_internals() {
        // フォロー対象は .git 配下以外(index/refs/lock の churn はレビュー対象でない)。
        let got = follow_candidates(&[
            PathBuf::from("/repo/.git/index"),
            PathBuf::from("/repo/.git/refs/heads/main"),
            PathBuf::from("/repo/src/main.rs"),
            PathBuf::from("/repo/README.md"),
        ]);
        assert_eq!(
            got,
            vec![
                PathBuf::from("/repo/src/main.rs"),
                PathBuf::from("/repo/README.md"),
            ]
        );
    }

    #[test]
    fn any_key_but_follow_toggle_breaks_follow() {
        // フォロー中に F 以外のキー → 解除(Zed 流)。F 自体はトグルに解決される。
        let dir = std::env::temp_dir().join("konoma_follow_break_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(app.follow_enabled(), "F で ON");
        // F をもう一度: follow_break でなくトグル経由の OFF(=見分けは付かないが OFF になる)。
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(!app.follow_enabled(), "F 再押下で OFF");
        // ON に戻して j(移動キー)→ 解除される。
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(app.follow_enabled());
        handle_key(&mut app, key('j')).unwrap();
        assert!(!app.follow_enabled(), "F 以外のキーで追尾解除");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quote_opens_bookmark_list_and_letters_jump() {
        // `'` 一発で一覧が開き(不可視の待ち受けは廃止)、一覧内の素の英字はブックマーク名として
        // 直接ジャンプ。旧 e/d(編集/削除)は Ctrl 修飾へ移設され、素の e もジャンプに使える。
        let root = std::env::temp_dir().join("konoma_quote_list_test");
        let _ = std::fs::remove_dir_all(&root);
        let proj = root.join("proj");
        std::fs::create_dir_all(proj.join("sub")).unwrap();
        std::fs::write(proj.join("f.txt"), b"x").unwrap();
        let proj = proj.canonicalize().unwrap();
        let mut app = App::new(proj.clone(), Config::default()).unwrap();
        app.bookmarks = bookmarks::Bookmarks::with_base(root.join("cfgbase"), &proj);
        app.bookmarks.set('a', proj.join("sub")).unwrap();
        app.bookmarks.set('e', proj.join("f.txt")).unwrap();

        handle_key(&mut app, key('\'')).unwrap();
        assert!(app.is_bookmark_list(), "' 一発で一覧が開く");
        assert!(!app.is_marking(), "ジャンプの待ち受け状態は無い");
        // 未登録の英字: flash して一覧は開いたまま。
        handle_key(&mut app, key('z')).unwrap();
        assert!(app.is_bookmark_list());
        assert!(app.flash.is_some(), "未登録は flash");
        // e はブックマーク名としてジャンプ(ファイル → プレビュー)。
        handle_key(&mut app, key('e')).unwrap();
        assert!(!app.is_bookmark_list(), "ジャンプで一覧が閉じる");
        assert_eq!(app.mode, Mode::Preview);
        assert!(app
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("f.txt")));
        // 戻って `'` 2度目=閉じる(トグル感)。
        handle_key(&mut app, key('q')).unwrap(); // preview → tree
        handle_key(&mut app, key('\'')).unwrap();
        assert!(app.is_bookmark_list());
        handle_key(&mut app, key('\'')).unwrap();
        assert!(!app.is_bookmark_list(), "' 再押下で閉じる");
        // Ctrl+D=選択行を削除(素の d はジャンプ用に予約)。j で e の行へ降りてから消す。
        handle_key(&mut app, key('\'')).unwrap();
        let before = app.bookmark_list_items().len();
        handle_key(&mut app, key('j')).unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        )
        .unwrap();
        assert_eq!(app.bookmark_list_items().len(), before - 1, "Ctrl+D で削除");
        assert!(app.bookmarks.get('e').is_none(), "消えたのは選択行の e");
        // ディレクトリのブックマークは root 移動。
        handle_key(&mut app, key('a')).unwrap();
        assert_eq!(app.root, proj.join("sub"));
        assert!(!app.is_bookmark_list());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn filter_input_captures_literal_keys() {
        // 絞り込み入力中は `?`/`c`/数字も「文字」として拾い、ヘルプ/コピー/タブにしない。
        let dir = std::env::temp_dir().join("konoma_filter_input_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key('/')).unwrap(); // 絞り込み開始
        assert!(app.is_filtering());
        // `c`(通常はコピーコード)・`?`(通常はヘルプ) を文字として取り込む。
        handle_key(&mut app, key('c')).unwrap();
        handle_key(&mut app, key('?')).unwrap();
        assert_eq!(app.filter_query(), Some("c?"));
        assert_eq!(app.pending_leader, None, "コピーリーダーは始まらない");
        assert!(!app.show_help, "ヘルプは開かない");
        // Esc で解除。
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(!app.is_filtering() && app.filter_query().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn help_opens_in_git_view_and_shows_git_keys() {
        let dir = std::env::temp_dir().join("konoma_git_help_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_view();
        assert!(app.is_git_view());

        // git ビューで `?` → ヘルプが開く(従来は git intercept に飲まれて開かなかった)。
        handle_key(&mut app, key('?')).unwrap();
        assert!(app.show_help, "git モードで ? がヘルプを開く");

        // ヘルプ内容が git 用(変更ハブの節・ステージ系キー)になっている。
        let lines = ui::help::help_lines(&app);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect();
        assert!(
            text.contains("Git changes") || text.contains("変更ハブ"),
            "git 節が出る: {text}"
        );
        assert!(
            text.contains("stage") || text.contains("ステージ"),
            "ステージ系キーが出る"
        );

        // ヘルプ表示中の `?` で閉じる。
        handle_key(&mut app, key('?')).unwrap();
        assert!(!app.show_help, "? で閉じる");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn tab_keys_per_tab_git_mode_and_literal_in_filter() {
        let dir = std::env::temp_dir().join("konoma_tab_gitview_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

        // 絞り込み入力中は t は「文字」として拾い、新タブにしない。
        handle_key(&mut app, key('/')).unwrap();
        let tc = app.tab_count();
        handle_key(&mut app, key('t')).unwrap();
        assert_eq!(app.tab_count(), tc, "絞り込み中は t で新タブにしない");
        assert_eq!(app.filter_query(), Some("t"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();

        // git ビュー中でも t で新タブが開ける。新タブは素の Tree(git モードはタブごと)。
        app.open_git_view();
        assert!(app.is_git_view());
        handle_key(&mut app, key('t')).unwrap();
        assert_eq!(app.tab_count(), tc + 1, "git ビュー中でも t で新タブ");
        assert!(!app.is_git_view(), "新タブは git ビュー無しで始まる");
        // 元のタブへ戻ると **git モードが復元される**(別タブでドキュメントを見て戻れる)。
        handle_key(&mut app, key('1')).unwrap(); // タブ0へ
        assert!(app.is_git_view(), "タブを戻ると git モードのまま");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_leader_sets_then_clears_pending() {
        use keymap::LeaderId;
        let dir = std::env::temp_dir().join("konoma_chord_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        // `y` でコピーリーダー開始 → pending_leader。
        handle_key(&mut app, key('y')).unwrap();
        assert_eq!(app.pending_leader, Some(LeaderId::Copy));
        // リーダーに無いキーは破棄され pending クリア(クリップボードには触れない)。
        handle_key(&mut app, key('x')).unwrap();
        assert_eq!(app.pending_leader, None);
    }

    #[test]
    fn file_leader_opens_on_space() {
        use keymap::LeaderId;
        let dir = std::env::temp_dir().join("konoma_fileleader_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        // 既定の `c` はもうリーダーにならない (旧 copy prefix は廃止)。
        handle_key(&mut app, key('c')).unwrap();
        assert_eq!(app.pending_leader, None);
        // `Space` でファイル管理リーダー開始。
        handle_key(&mut app, key(' ')).unwrap();
        assert_eq!(app.pending_leader, Some(LeaderId::File));
    }

    #[test]
    fn space_leader_n_opens_create_dialog() {
        // Space→n = ファイル作成 (旧 Tree `a` の移管先)。
        let dir = std::env::temp_dir().join("konoma_space_create_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key(' ')).unwrap();
        handle_key(&mut app, key('n')).unwrap();
        assert_eq!(app.pending_leader, None, "リーダーは確定で消える");
        assert!(app.is_dialog(), "Space→n で作成ダイアログが開く");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_keys_a_and_shift_a_dispatch() {
        // `a`=SetAnchor (旧 `:`), `A`=ResetAnchor (新規)。起動直後はどちらも「既に基準」flash。
        let dir = std::env::temp_dir().join("konoma_anchor_dispatch_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        handle_key(&mut app, key('a')).unwrap();
        let fa = app.flash.clone().unwrap_or_default();
        assert!(
            fa.contains("root") || fa.contains("ルート"),
            "a の flash: {fa}"
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
        )
        .unwrap();
        let fa2 = app.flash.clone().unwrap_or_default();
        assert!(
            fa2.contains("start") || fa2.contains("起動"),
            "A の flash: {fa2}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shift_q_opens_quit_confirm_then_qq_quits() {
        // 既定(confirm_quit=ON): Q で確認ダイアログ→まだ終了しない。もう一度 q(qq)で確定終了。
        let dir = std::env::temp_dir().join("konoma_quit_confirm_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let exit = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert!(!exit, "確認段階ではまだ終了しない");
        assert!(
            app.is_dialog() && app.confirm_is_quit(),
            "終了確認ダイアログが開く"
        );
        assert_eq!(app.surface(), Surface::DialogConfirmQuit);
        let exit2 = handle_key(&mut app, key('q')).unwrap();
        assert!(exit2, "qq の2打鍵目で終了");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quit_confirm_cancel_with_esc_keeps_running() {
        let dir = std::env::temp_dir().join("konoma_quit_cancel_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key('q')).unwrap(); // Tree の q=Quit→確認ダイアログ
        assert!(app.is_dialog() && app.confirm_is_quit());
        let exit = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(!exit, "Esc では終了しない");
        assert!(!app.is_dialog(), "Esc で確認ダイアログが閉じる");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quit_without_confirm_quits_immediately() {
        // confirm_quit=false: Q は即終了(ダイアログ無し)。
        let dir = std::env::temp_dir().join("konoma_quit_immediate_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.confirm_quit = false;
        let mut app = App::new(dir.clone(), cfg).unwrap();
        let exit = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert!(exit, "confirm_quit=false なら Q で即終了");
        assert!(!app.is_dialog(), "ダイアログは開かない");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shift_q_is_literal_while_filtering() {
        // 入力中(絞り込み)は Q を「文字」として取り込む。終了しない/確認も出さない。
        let dir = std::env::temp_dir().join("konoma_quit_filter_literal_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key('/')).unwrap();
        let exit = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert!(!exit, "絞り込み中の Q では終了しない");
        assert!(!app.is_dialog(), "終了確認も出ない");
        assert_eq!(app.filter_query(), Some("Q"), "Q は文字として入力される");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn visual_space_d_commits_range_and_confirms_delete() {
        // 旧・直 `D` 廃止 → ビジュアルでは Space→d。範囲を確定してから削除確認へ。
        let dir = std::env::temp_dir().join("konoma_visual_spaced_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"y").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();
        app.selected = 0;
        handle_key(&mut app, key('v')).unwrap();
        assert!(app.is_visual(), "v でビジュアル");
        handle_key(&mut app, key(' ')).unwrap();
        handle_key(&mut app, key('d')).unwrap();
        assert!(!app.is_visual(), "Space→d で範囲を確定しビジュアルを抜ける");
        assert!(app.is_dialog(), "削除確認ダイアログが開く");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dnd_paste_ignored_while_dialog_or_overlay_open() {
        // #13: ダイアログ/モーダル/オーバーレイ表示中の D&D ペーストは横入りさせない。
        // 基本全画面 (Tree/Preview) とテキスト入力面のみ受け付ける。
        let dir = std::env::temp_dir().join("konoma_dnd_guard_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        // 通常の Tree 面は受け付ける。
        assert_eq!(app.surface(), Surface::Tree);
        assert!(paste_accepted(app.surface()), "Tree ではドロップを受ける");
        // ファイル作成ダイアログ (入力) を開く → テキスト入力面なので文字として受ける。
        handle_key(&mut app, key(' ')).unwrap();
        handle_key(&mut app, key('n')).unwrap();
        assert!(app.is_dialog());
        assert!(
            paste_accepted(app.surface()),
            "入力ダイアログは文字として取り込むため通す"
        );
        // ヘルプ (オーバーレイ) 表示中は横入りさせない。
        let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app2, key('?')).unwrap();
        assert!(app2.show_help);
        assert!(
            !paste_accepted(app2.surface()),
            "ヘルプ表示中はドロップを無視する"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn help_toggles_and_swallows_keys() {
        let dir = std::env::temp_dir().join("konoma_help_key_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        assert!(!app.show_help);
        // ? で開く。
        handle_key(&mut app, key('?')).unwrap();
        assert!(app.show_help);
        // 開いている間は j がスクロール(モード操作には流れない)。
        handle_key(&mut app, key('j')).unwrap();
        assert_eq!(app.help_scroll, 1);
        // q で閉じる(アプリは終了しない)。
        assert!(!handle_key(&mut app, key('q')).unwrap());
        assert!(!app.show_help);
    }

    #[test]
    fn tab_keys_work_in_preview_and_preserve_mode() {
        // バグ修正: Preview 中でもタブ操作 (t/w/[/]/1-9) が効き、かつ各タブのモード
        // (Tree/Preview) を保持する。切替で Tree へ落とさず、戻れば Preview が復元される。
        let dir = std::env::temp_dir().join("konoma_tab_in_preview_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        // タブ0をプレビュー中にする。
        app.mode = Mode::Preview;
        // t で新規タブ (Preview 中でも効く)。新タブは Tree から始まる。
        handle_key(&mut app, key('t')).unwrap();
        assert_eq!(app.tab_count(), 2, "Preview 中でも新規タブが作れる");
        assert_eq!(app.mode, Mode::Tree, "新規タブは Tree から");
        let new_tab = app.active_tab_index();
        // [ で元のタブ0へ戻る → Tree に落とさず Preview が復元される。
        handle_key(&mut app, key('[')).unwrap();
        assert_ne!(app.active_tab_index(), new_tab, "タブが切り替わる");
        assert_eq!(app.mode, Mode::Preview, "戻ったタブの Preview が復元される");
    }

    #[test]
    fn flash_is_cleared_on_next_key() {
        let dir = std::env::temp_dir().join("konoma_flash_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        app.flash = Some("x".into());
        handle_key(&mut app, key('j')).unwrap();
        assert_eq!(app.flash, None, "次のキーで flash が消える");
    }

    #[test]
    fn recoverable_key_error_flashes_and_does_not_quit() {
        // 設計原則#3: キー処理中の回復可能な fs/git 失敗で TUI を落とさない。
        // handle_key が Err を返しても run ループは終了せず(quit=false)、エラーは
        // 握り潰さず flash でユーザに見せる。
        let dir = std::env::temp_dir().join("konoma_recoverable_err_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let quit = resolve_key_result(&mut app, Err(anyhow::anyhow!("boom: refresh 失敗")));
        assert!(!quit, "回復可能な Err でループは終了しない");
        let flash = app
            .flash
            .as_deref()
            .expect("Err は握り潰さず flash で見せる");
        assert!(
            flash.contains("boom"),
            "原因メッセージが flash に出る: {flash}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn close_tab_or_quit_closes_tab_when_multiple_else_quits() {
        // ツリー q: 複数タブなら現在タブを閉じる(終了しない)、最後の1枚なら終了要求。
        let dir = std::env::temp_dir().join("konoma_close_tab_or_quit_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.confirm_quit = false; // 最後の1枚は即終了で判定(ダイアログを挟まない)
        let mut app = App::new(dir.clone(), cfg).unwrap();

        // 2枚 → q はタブを閉じるだけ(quit=false)。
        app.tab_new().unwrap();
        assert_eq!(app.tab_count(), 2);
        let quit = dispatch_action(&mut app, Action::CloseTabOrQuit, Surface::Tree).unwrap();
        assert!(!quit, "複数タブでは終了しない");
        assert_eq!(app.tab_count(), 1, "現在タブが閉じて1枚に戻る");

        // 最後の1枚 → q は終了要求(confirm_quit=false なので即 Ok(true))。
        let quit = dispatch_action(&mut app, Action::CloseTabOrQuit, Surface::Tree).unwrap();
        assert!(quit, "最後の1枚では終了する");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_key_result_passes_through_quit_and_continue() {
        // Ok(true)=終了要求はそのまま伝える / Ok(false)=継続はそのまま、flash も汚さない。
        let dir = std::env::temp_dir().join("konoma_resolve_passthrough_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        assert!(
            resolve_key_result(&mut app, Ok(true)),
            "Ok(true) は終了要求を通す"
        );
        assert_eq!(app.flash, None, "終了要求では flash を立てない");
        assert!(!resolve_key_result(&mut app, Ok(false)), "Ok(false) は継続");
        assert_eq!(app.flash, None, "継続では flash を立てない");
        std::fs::remove_dir_all(&dir).ok();
    }
}
