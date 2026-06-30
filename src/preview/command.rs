// 外部コマンド委譲レンダラ。
//
// 方針: `PreviewKind::Command` のテンプレ ({path} / {out}) を展開し、子プロセスを実行する。
//   - render_as = "image": 生成物(PNG 等)を image レンダラに渡して全画面表示。
//   - detached = true: 別プロセスで開き TUI をブロックしない (mpv 等の動画)。
// コマンド未導入・失敗時はクラッシュさせず CanNotPreview と同じフォールバックへ。
//
// M0 ではスタブ。テンプレ展開と子プロセス起動を後で実装する。
