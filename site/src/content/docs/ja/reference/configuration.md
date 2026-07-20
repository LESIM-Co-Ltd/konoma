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
全キーにコメント付きの実例が
[`config.example.toml`](https://github.com/LESIM-Co-Ltd/konoma/blob/main/config.example.toml)
にあります(英語)。日本語コメント版は
[`config.example.ja.toml`](https://github.com/LESIM-Co-Ltd/konoma/blob/main/config.example.ja.toml)
です。出発点としてコピーしてください:

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
| `confirm_bookmark_overwrite` | `true` | ブックマーク(`m`)が既存の**別のパス**を上書きするときに確認(`y`/`Enter`=上書き・`n`/`Esc`=取消)。同じパスの再登録・未使用キーは確認なし。`false` = 確認なしで即上書き。 |
| `csv_rainbow` | `true` | CSV/TSV テーブルの列レインボー。`false` = 単色(整列・セル移動はそのまま)。 |
| `follow_view` | `"diff"` | フォローモード(`F`)がジャンプ先をどう開くか: `"diff"`(全画面 git diff・未追跡は全行追加) / `"file"`(通常プレビューで最初の変更ハンクへスクロール)。diff の無いファイルとメディアは常に `"file"` 相当。 |
| `busy_indicator` | `true` | バックグラウンド処理(git 無視ファイルスキャン・メディア読込・ハイライト準備・画像取得)の実行中だけ、右上にスピナーとジョブ名を表示。アイドル時は何も出ず負荷もゼロ。 |
| `mermaid` | `"image"` | mermaid 図の描画方法。`"image"` は純 Rust でプロセス内ラスタライズ(mermaid.js 品質・ブラウザ/Node 不要): 単体 `.mmd` は全画面(ズームはズーム率に合わせて再ラスタライズ=拡大してもシャープ)、Markdown 内の ```mermaid フェンスはインライン表示(`Tab` でフォーカス=シアンの枠+図全体へ自動スクロール。`+`/`-`=**その場ズーム**(レイアウト不変)・ズーム中 `hjkl`=パン・`0`=フィット・`Enter`=全画面・`q`=戻る)。`"text"` は従来の Unicode 罫線描画。未対応の図・描画失敗・画像非対応端末は自動でテキストに降格。 |
| `mermaid_theme` | `"dark"` | 画像モードの図の配色テーマ: `"dark"`・`"light"`・`"classic"`(mermaid.js 既定)・`"forest"`・`"neutral"`。背景は常に透過(端末背景に馴染む)。 |
| `mermaid_rows` | `24` | Markdown 内インライン mermaid 図の表示高さの目標(行数)。**拡大方向にも効く**=ベクタ由来なので必要密度へ自動再ラスタライズされシャープなまま(幅は本文幅で頭打ち・縦横比維持)。初期表示は**ビューポートにもフィット**=窓が目標より低いときは全体が見える高さへ縮む。0/不正値は既定に戻る。 |
| `math` | `"image"` | LaTeX 数式の描画方法。`"image"`(既定)は `$…$` / `$$…$$` を RaTeX(純 Rust・KaTeX 品質・ブラウザ/Node 不要)でプロセス内ラスタライズしインライン画像で表示(ターミナルは画像を文中に置けないのでインライン数式は自前の行に持ち上がる・ディスプレイ数式は中央寄せ)。`"text"` は生 LaTeX を素テキストのまま。画像非対応端末・描画失敗は自動で生 LaTeX へ降格。 |
| `math_color` | `"#d0d0d0"` | 画像モード数式のグリフ色。RaTeX は数式を純黒で塗るため、ダーク端末では不可視。konoma はダーク端末前提なので、透過背景の上にこの色で塗り替える(端末背景が透ける=mermaid と同じ)。ライト端末では暗い色(例 `"#202020"`)を指定する。usvg が解釈する色(`#hex`・`rgb(…)`・CSS 色名)を受け付け、不明な値や完全透過はタイプミスでも数式を空白にせず既定へフォールバックする。 |
| `restore_tabs` | `true` | **起動ディレクトリ毎**に前回のタブ構成(各タブの root・ツリーカーソル・プレビュー)を保存し、同じディレクトリでの次回起動時に復元。保存はタブの開閉/切替と終了時、保存先は `~/.config/konoma/sessions/`。`false` で常にまっさらに起動(読みも書きもしない)。 |
| `md_task_states` | `[" ", "x"]` | Markdown チェックボックスで `Space` が巡回する状態(順序どおり・各要素1文字)。例 `[" ", "/", "x"]` で Obsidian 流の作業中状態(`[/]` 表示)。不正な設定は既定へ。 |
| `md_autolink` | `true` | Markdown プレビューで裸の URL・メールを自動リンク化(GFM autolink・GitHub と同じ)。素の `https://…` / `www.…` / `foo@bar.com` がフォーカス可能なリンクに(`Tab` で移動・`Enter` で開く)。コード span / コードフェンス内はリンク化しない。`false` で素テキストのまま。 |
| `md_alerts` | `true` | GitHub 形式のアラートを色付きコールアウト箱(アイコン+ラベル)で描く(素の引用でなく)。マーカー(大小無視+一般的な別名): `> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]`。`false` で通常の引用(マーカーはそのまま)。 |
| `md_emoji` | `true` | Markdown プレビューで `:shortcode:` 絵文字を実 Unicode に変換(GitHub と同じ・`:rocket:` → 🚀)。Unicode を持たない GitHub 独自ショートコード(`:shipit:` 等)とコード内はそのまま。絵文字幅が桁揃えを崩す場合は `false`。 |
| `md_frontmatter` | `true` | 先頭の YAML front matter(`---` … `---` が文書の最初)を認識し、罫線+生YAML でなくコンパクトな dim メタデータ block として表示。`false` なら通常の Markdown として描く。 |
| `md_footnotes` | `true` | GFM 脚注を描く: `text[^1]` の参照は上付き番号になり、`[^1]: …` の定義は末尾の番号付き脚注節にまとまる。`false` ならリテラル表示。 |
| `md_inline_html` | `true` | Markdown エンジンが剥がす一般的なインライン HTML を描く: `<del>`/`<s>`/`<strike>`=打消し線・`<kbd>`=インラインコードのキーキャップ・`<sup>`/`<sub>`=Unicode(対応する文字のみ)・`<br>`=ハード改行。(`<mark>`/`<ins>` はどちらでもテキストのみ。)`false` なら全タグを剥がす。 |
| `md_details` | `"auto"` | `<details>` の初期表示。`"auto"` は GitHub と同じく open 属性を尊重(`<details>` 折りたたみ / `<details open>` 展開)・`"open"` 常に展開・`"closed"` 常に折りたたみ。いずれも `Tab` で `<summary>` にフォーカス→`Space`/`Enter` でトグル。 |

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
| `mermaid` | 単体 `.mmd`/`.mermaid` を図として表示。既定は実画像(純 Rust・全画面ズーム/パン)。`[ui] mermaid = "text"` で Unicode 罫線図に切替。 |
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
rs = "nvim +{line} {path}"  # {line} = プレビューで見ていた行
```

解決順: `[editor.ext]` → `editor.command` → `$VISUAL` → `$EDITOR` → `vim`。
値はコマンド+引数(空白区切り)。`{path}` があれば置換、無ければ末尾に追加されます。

**プレビューの表示行で開く。** windowed プレビュー(素のテキスト・コード・`R` の生 Markdown)から
`e` を押すと、キャレット行でエディタが開きます。`{line}` トークンで行の渡し方を明示できます
(`code -g {path}:{line}`・`hx {path}:{line}`・`nvim +{line} {path}`)。`{line}` が無くても
主要エディタは自動対応します — vim 系(`+N`+その行を画面先頭へスクロールする `zt`)・
VS Code(`-g path:N`)・Sublime/Helix/Zed(`path:N`)。それ以外のエディタは先頭で開きます。
装飾 Markdown はソースを折返し直すため、画面先頭に見えているテキストをソースから探して
該当行に着地します(`R` の生ソースなら正確なキャレット行)。リンク/チェックボックス/
コードブロックを `Tab` でフォーカスして画面内にあるときは、そのアイテムの行で開きます。
Mermaid と画像は常に先頭です。

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
- **ツリー**: `quit`・`close_tab_or_quit`・`tree_descend`・`tree_leave`・`tree_activate`・`filter_start`・`toggle_hidden`・`refresh`・`open_sort_menu`・`toggle_info`・`request_edit`・`cycle_path_style`・`set_anchor`・`reset_anchor`・`enter_visual`・`toggle_select`・`open_in_new_tab`(`Ctrl-t`=カーソル下のエントリを別タブ(前面)で開く)
- **ブックマーク**: `mark_set`(`m`)・`mark_jump`(`'` = 一覧を開く。一覧内の素の英字はジャンプ)・`bookmark_edit`(`ctrl-e`)・`bookmark_delete`(`ctrl-d`)・`bookmark_close`。`m`/`'` はツリーとプレビューの両面に既定割当(プレビューでは表示中ファイルを登録)。
- **パスコピー**(`y` リーダー): `copy_name`・`copy_relative`・`copy_full`・`copy_parent`・`copy_at_ref`(AI チャット用 `@相対パス`)
- **ファイル管理**(`Space` リーダー): `file_create`・`file_rename`・`file_delete`・`file_copy`・`file_cut`・`file_paste`・`file_duplicate`(`Space→D`=カーソル/選択をその場に複製。例 `note copy.md`)
- **プレビュー**: `preview_back`・`search_start`・`search_next`・`search_prev`・`preview_enter_visual`(`v`)・`preview_enter_visual_line`(`V`)・`preview_copy_selection`・`preview_copy_selection_ref`(`Y` = `@path#L12-34`)・`toggle_markdown_raw`(`R`)・`link_focus_next/prev`・`link_open`(`Enter`=同タブ)・`open_link_new_tab`(`Ctrl-t`=別タブ)・`image_zoom_in/out/reset`・`pdf_next_page`・`pdf_prev_page`・`preview_next_file` / `preview_prev_file`(`Ctrl-n` / `Ctrl-p`=ツリー表示順で次/前のファイルへ。ディレクトリはスキップ・端で wrap)・`table_copy_cell/row/column`
- **Agent Watch**: `toggle_follow`(`F`)・`toggle_changed_filter`(`C`)・`jump_next_change`(`n`)・`jump_prev_change`(`N`)
- **Git**: `open_git_view`(`o`)・`open_git_diff_cursor`(`d`)・`git_stage`・`git_unstage`・`git_stage_all`・`git_unstage_all`・`git_discard`・`git_commit`・`git_open_log`・`git_open_graph`・`git_open_branches`・`git_launch_tool`(`O`)・`cycle_diff_layout`・`git_copy_*`・`branch_*`
- **パス貼り付けジャンプ**(`global`): `paste_jump`(`P`) — クリップボードのパス/GitHub リンクを読んでその場所へジャンプ(reveal+プレビュー)。ローカルの絶対/相対パス・GitHub `blob`/`raw` URL・`#L123` / `:123` の行アンカーに対応。対象が root 外ならそのリポジトリへ root を切替えます。
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
| `~/.config/konoma/sessions/<dir>.toml` | タブセッション(`restore_tabs`)。起動ディレクトリごとに1ファイル。 |
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
