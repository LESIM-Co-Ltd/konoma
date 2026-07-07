---
title: 設定リファレンス
description: konoma の全オプション — [ui]・配色・プレビュールール・エディタ・git・キーバインドのモデル。
sidebar:
  order: 1
---

konoma は 1 つの TOML ファイルを読みます:

```
~/.config/konoma/config.toml
```

すべて任意です — **設定ファイルが無くても動きます**。ファイルが無い/壊れていても
起動は失敗せず、以下の既定値へフォールバックします(不正な値はキー単位で既定へ)。
全キーに日本語コメント付きの実例が
[`config.example.toml`](https://github.com/LESIM-Co-Ltd/konoma/blob/main/config.example.toml)
にあります。出発点としてコピーしてください:

```sh
mkdir -p ~/.config/konoma
cp config.example.toml ~/.config/konoma/config.toml
```

## クイックスタート

```toml
[ui]
lang = "jp"                 # UI 言語("auto" は OS に追従)
wrap = false                # 折返しなし。長い行は h/l で横スクロール
line_numbers = true
details = ["size", "modified"]

[ui.theme]
bg = "#282c34"

[keys]
copy_prefix = "y"
```

## `[ui]` — 見た目とふるまい

| キー | 既定 | 説明 |
|---|---|---|
| `show_hidden` | `false` | 起動時から隠しファイルを表示(実行時は `.` で切替)。 |
| `tabbar` | `"auto"` | タブバー表示: `"always"` / `"auto"`(2枚以上のときだけ) / `"hidden"`。 |
| `icons` | `true` | ツリー・Markdown リンク・チェックボックスの Nerd Font アイコン。Nerd Font が無い端末では `false` に — プレーン記号にフォールバックします(豆腐は出ません)。 |
| `wrap` | `true` | テキストプレビューの折返し。`false` = 折返さず横スクロール(`h`/`l`・`0`/`$`)。 |
| `line_numbers` | `false` | コード/テキストプレビューの行番号ガター。 |
| `git_gutter` | `true` | 未コミット変更ファイルのプレビューにエディタ風の変更ガター(緑=追加/青=変更/赤=削除)。 |
| `tab_width` | `4` | プレビューのタブ幅(`0` で生タブのまま)。 |
| `syntax_highlight` | `true` | コードのシンタックスハイライト(`false` = 素のテキスト・最速)。 |
| `preview_loading` | `"indicator"` | 重いコードの初回表示: `"indicator"`(ロード表示) / `"progressive"`(先に素のテキスト、色は後から)。 |
| `path_style` | `"relative"` | タイトルのパス表示: `"relative"` / `"home"`(`~/...`) / `"full"`。実行時は `p` で巡回。 |
| `keys` | `"vim"` | プレビューのページ送り流儀: `"vim"`(`Ctrl-f/b`・`Ctrl-d/u`) / `"less"`(`f`/`b`・`d`/`u`・`Space`)。 |
| `lang` | `"auto"` | ヘルプ/ヒント/メッセージの言語: `"auto"`(OS の言語) / `"en"` / `"jp"`。 |
| `statusbar` | `"split"` | ステータス表示の配置: `"split"`(上=コンテキスト・下=ヒント) / `"bottom"` / `"top"`。 |
| `image_render_scale` | `1.0` | 画像表示のスケール(0.1〜1.0)。小さいほど端末への転送ピクセルが減り速い(表示も小さい)。 |
| `svg_max_px` | `800` | SVG ラスタライズの最大辺(px)。大きいほど精細だが重い。 |
| `details` | `[]` | ツリー各行のメタデータ列(順序どおり)。`"size"` `"modified"` `"perm"` `"type"` `"items"`(ディレクトリ内件数)。 |
| `graph_max_branches` | `12` | コミットグラフ(`o` → `g`)の同時描画ブランチ上限。`0` = 無制限。実行時はグラフ内 `b` で切替。 |
| `graph_base_branches` | `[]` | グラフの基準ブランチ候補(優先順)。例 `["main", "develop"]` — 最初に存在するものが基準(レーン0固定)になり、配列順が表示優先順になります。 |
| `commit_meta_align` | `"right"` | log/グラフの author・日付: `"right"`(右端の揃った列) / `"inline"`(件名の直後)。 |
| `confirm_quit` | `true` | 終了前に確認(`q`/`y`/`Enter`=終了・`n`/`Esc`=取消・`qq` で素早く)。`false` = 即終了。 |
| `csv_rainbow` | `true` | CSV/TSV テーブルの列レインボー。`false` = 単色(整列・セル移動はそのまま)。 |
| `follow_view` | `"diff"` | フォローモード(`F`)がジャンプ先をどう開くか: `"diff"`(全画面 git diff・未追跡は全行追加) / `"file"`(通常プレビューで最初の変更ハンクへスクロール)。diff の無いファイルとメディアは常に `"file"` 相当。 |
| `busy_indicator` | `true` | バックグラウンド処理(git 無視ファイルスキャン・メディア読込・ハイライト準備・画像取得)の実行中だけ、右上にスピナーとジョブ名を表示。アイドル時は何も出ず負荷もゼロ。 |
| `md_task_states` | `[" ", "x"]` | Markdown チェックボックスで `Space` が巡回する状態(順序どおり・各要素1文字)。例 `[" ", "/", "x"]` で Obsidian 流の作業中状態(`[/]` 表示)。不正な設定は既定へ。 |

## `[ui.sort]` — ツリーの既定並び順

| キー | 既定 | 説明 |
|---|---|---|
| `key` | `"name"` | 並び替え基準: `"name"` / `"size"` / `"modified"` / `"ext"`。 |
| `reverse` | `false` | 降順にする。 |
| `dirs_first` | `true` | ディレクトリを先頭にまとめる。 |

実行時は `s` メニュー(`n`/`s`/`m`/`e`・`r`=昇降・`.`=フォルダ先頭)で変更できます。

## `[ui.theme]` — 配色

色は `"#rrggbb"`・色名(`"black"`・`"lightblue"` …)・端末インデックス(`"8"`)・
`"none"` で指定します。

| キー | 既定 | 説明 |
|---|---|---|
| `bg` | `"none"` | アプリ背景。`"none"` は端末の既定背景のまま(透過設定も活きます)。 |
| `code_bg` | `"#2b303b"` | Markdown コード(インライン+ブロック)の背景帯。`"none"` で無し。 |
| `code_label_align` | `"right"` | コードブロックの言語バッジ位置: `"right"` / `"left"`。 |
| `code_label_bg` | `"auto"` | 言語バッジ背景: `"auto"`(`code_bg` を明るく) / `"none"` / 任意の色。 |
| `code_theme` | `"TwoDark"` | ハイライトテーマ(コードと md フェンス共通)。他に `"OneHalfDark"` `"Dracula"` `"Nord"` `"gruvbox-dark"` `"Catppuccin Mocha"` `"Monokai Extended"` `"Solarized (dark)"` `"GitHub"` など。区切り/大小文字は無視・不明名は TwoDark。 |

## `[[preview.rules]]` — ファイル種別ごとの表示方法

konoma の中核モデル: **フォーマット→ビューアを TOML で宣言**します。ルールは上から
評価され、最初にマッチしたものが使われます。`glob`(ファイル名・大文字小文字無視)か
`mime`(内容判定・例 `"image/*"`)でマッチし、内蔵レンダラか外部コマンドで描画します。

> **注意:** config に `[[preview.rules]]` を1つでも書くと、その一覧が既定ルールを
> **置き換えます**。1件だけ足すのではなく、`config.example.toml` の全ルールを
> コピーして編集してください。

内蔵レンダラ(`builtin = "..."`):

| 名前 | 描画するもの |
|---|---|
| `markdown` | 装飾 Markdown(見出し・表・リンク・チェックボックス・インライン画像・```` ```mermaid ```` フェンスは図に)。 |
| `mermaid` | 単体 `.mmd`/`.mermaid` を Unicode 罫線図に(純 Rust・外部ツール不要)。 |
| `image` | kitty graphics で全画面表示(ズーム/パン・GIF は自動アニメ)。 |
| `svg` | プロセス内でラスタライズ(resvg・純 Rust)して画像表示。 |
| `video` | `ffmpegthumbnailer`/`ffmpeg` で代表フレーム表示(任意ツール・無ければヒント)。再生したい場合は `command` で `mpv` へ。 |
| `pdf` | ページ単位でラスタライズ(`pdftocairo`/`pdftoppm`/`qlmanage`/`sips`・macOS は後者2つが常在)。`J`/`K` でページ送り(複数ページは poppler 必須)。 |
| `csv` / `tsv` | 列レインボー+セルカーソルの整列テーブル(`hjkl` 移動・`y →` でコピー)。 |
| `code` | シンタックスハイライト(文法は 拡張子 → ファイル名 → 先頭行 で解決)。 |
| `text` | 素のテキスト。テキストらしいファイルの自動フォールバック先でもあります。 |

外部コマンド委譲:

```toml
[[preview.rules]]
glob = "*.{mp4,mov}"
command = "mpv {path}"      # {path} = 対象ファイル, {out} = 一時出力パス
detached = true             # TUI をブロックしない(別プロセスで開く)

[[preview.rules]]
glob = "*.mmd"
command = "merman -i {path} -o {out}.png --scale 2"
render_as = "image"         # コマンドの出力を画像として表示
```

どのルールにも合わずテキストにも見えないファイルは、安全な
`[can not preview: <ext>]` 画面になります — konoma は未知の入力でクラッシュせず、
任意ツールの不在はヒント表示に降格します。

## `[editor]` — 外部エディタ

konoma はファイル内容を自分では編集しません。`e` はあなたのエディタに委譲します。

```toml
[editor]
command = "nvim"            # 全体の既定
[editor.ext]
md = "code -w"              # 拡張子ごとの上書き(ドット無し)
rs = "nvim"
```

解決順: `[editor.ext]` → `editor.command` → `$VISUAL` → `$EDITOR` → `vim`。
値はコマンド+引数(空白区切り)。`{path}` があれば置換、無ければ末尾に追加されます。

## `[git]` — git 連携

| キー | 既定 | 説明 |
|---|---|---|
| `tool` | `"lazygit"` | `O` で起動する外部 git ツール(コマンド+引数)。 |
| `diff` | `"unified"` | diff の初期レイアウト: `"unified"`(縦) / `"split"`(左右) / `"auto"`(幅で判断)。実行時は diff 内 `s` で巡回。 |

## `[keys]` — キーバインド

すべてのコマンドが再割当できます。モデルは「**画面(surface)ごとに キー → アクション
を割り当てる**」helix 流:

```toml
[keys.tree]
"J" = "navigate:half_down"     # 大文字 = Shift 込み
"ctrl-g" = "open_git_view"     # ctrl-x / c-x
"space d" = "file_delete"      # 2 トークン = 和音(リーダー + キー)
"o" = "noop"                   # 既定の割当を消す

[keys.global]                  # 入力面以外の全画面が継承
"Q" = "quit"
```

面(surface)名: `global`・`tree`・`tree_visual`・`preview_text`・
`preview_text_visual`・`preview_image`・`preview_table`・`sort`・`bookmarks`・
`info`・`help`、および(git ビルドで)`preview_git_diff`・`git_changes`・
`git_log`・`git_graph`・`git_branches`・`git_detail`。

キー表記: 単文字(大文字=Shift 込み)・`space`・リテラル `0 $ ! + - = . / '`・
修飾 `ctrl-<k>`(別名 `c-<k>`)・名前付き `tab enter esc backspace delete up down
left right home end pageup pagedown`。空白区切りの 2 トークンは和音
(`"y f"` = `y` のあと `f`)。`Esc`/`Enter`/`Tab`/矢印とテキスト入力中のキーは固定で
再割当できません。

アクション名は snake_case の文字列です — 注釈付きの全一覧は
[`config.example.toml`](https://github.com/LESIM-Co-Ltd/konoma/blob/main/config.example.toml)
にあります。主なグループ:

- **移動**: `navigate:down|up|top|bottom|page_down|page_up|half_down|half_up|left|right|line_home|line_end`
- **ツリー**: `quit`・`close_tab_or_quit`・`tree_descend`・`tree_leave`・`tree_activate`・`filter_start`・`toggle_hidden`・`refresh`・`open_sort_menu`・`toggle_info`・`request_edit`・`cycle_path_style`・`set_anchor`・`reset_anchor`・`enter_visual`・`toggle_select`
- **ブックマーク**: `mark_set`(`m`)・`mark_jump`(`'` = 一覧を開く。一覧内の素の英字はジャンプ)・`bookmark_edit`(`ctrl-e`)・`bookmark_delete`(`ctrl-d`)・`bookmark_close`。`m`/`'` はツリーとプレビューの両面に既定割当(プレビューでは表示中ファイルを登録)。
- **パスコピー**(`y` リーダー): `copy_name`・`copy_relative`・`copy_full`・`copy_parent`・`copy_at_ref`(AI チャット用 `@相対パス`)
- **ファイル管理**(`Space` リーダー): `file_create`・`file_rename`・`file_delete`・`file_copy`・`file_cut`・`file_paste`
- **プレビュー**: `preview_back`・`search_start`・`search_next`・`search_prev`・`preview_enter_visual`(`v`)・`preview_enter_visual_line`(`V`)・`preview_copy_selection`・`preview_copy_selection_ref`(`Y` = `@path#L12-34`)・`toggle_markdown_raw`(`R`)・`link_focus_next/prev`・`link_open`・`image_zoom_in/out/reset`・`pdf_next_page`・`pdf_prev_page`・`table_copy_cell/row/column`
- **Agent Watch**: `toggle_follow`(`F`)・`toggle_changed_filter`(`C`)・`jump_next_change`(`n`)・`jump_prev_change`(`N`)
- **Git**: `open_git_view`(`o`)・`open_git_diff_cursor`(`d`)・`git_stage`・`git_unstage`・`git_stage_all`・`git_unstage_all`・`git_discard`・`git_commit`・`git_open_log`・`git_open_graph`・`git_open_branches`・`git_launch_tool`(`O`)・`cycle_diff_layout`・`git_copy_*`・`branch_*`
- **タブ / アプリ**(`global`): `tab_new`(`t`)・`toggle_tab_list`(`T`=タブ一覧。一覧内 `tab_list_close`=`d`)・`tab_prev`/`tab_next`(`[`/`]`)・`quit`(`Q`)・`toggle_help`(`?`)。`tab_close` は既定キー無し(閉じるはツリーの `q`。`"w" = "tab_close"` で復活可)
- `noop`(別名 `disabled`)は既定の割当を消します。

重要な既定を壊す衝突設定(リーダー prefix を単発で潰す・タブキーの流用など)は
起動時に検知され、フッターで通知して既定に戻します — 壊れた設定で UI が
使えなくなることはありません。

パスコピーには `[keys]` 直下の後方互換エイリアス(`copy_prefix`・`copy_name`・
`copy_relative`・`copy_full`・`copy_parent`)もあります。

## データファイル

| パス | 内容 |
|---|---|
| `~/.config/konoma/config.toml` | この設定。 |
| `~/.config/konoma/bookmarks.toml` | グローバル(大文字)ブックマーク — 絶対パス。 |
| `~/.config/konoma/bookmarks/<dir>.toml` | ローカル(小文字)ブックマーク。起動ディレクトリごとに1ファイル。 |
| `~/.cache/konoma/remote-images/` | Markdown 内リモート画像のキャッシュ。 |

## フォントと端末の要件

- **画像 / SVG / 動画サムネイル / PDF** には kitty graphics プロトコル対応端末
  (Ghostty・kitty など)が必要。テキスト系はどの端末でも動きます。
- **アイコン**(`ui.icons = true`・既定)には Nerd Font グリフが必要:
  端末のフォールバックに `Symbols Nerd Font Mono` を足すか、NF 内蔵フォント
  (HackGen Console NF・UDEV Gothic NF …)を使用。無ければ `ui.icons = false`。
- **任意ツール**: `poppler`(PDF 複数ページ)・`ffmpegthumbnailer`/`ffmpeg`
  (動画サムネイル)・`git` + `lazygit`(git スイート/外部ツール)。
  すべて不在時は安全に降格します。
