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
| `graph_base_branches` | `[]` | 基準ブランチの候補(優先順)。例 `["main", "develop"]`。グラフでは最初に存在するものが基準(レーン0固定)になり、配列順が表示優先順になります。ワークツリーの差分(`o` `w` → `d`)ではこれらを候補として扱い、**そのワークツリーと共通祖先が最も新しいもの**を選ぶので、配列順は同点時の優先順だけに効きます。 |
| `commit_meta_align` | `"right"` | log/グラフの author・日付: `"right"`(右端の揃った列) / `"inline"`(件名の直後)。 |
| `confirm_quit` | `true` | 終了前に確認(`q`/`y`/`Enter`=終了・`n`/`Esc`=取消・`qq` で素早く)。`false` = 即終了。 |
| `confirm_jj_sync` | `true` | ハブで `R` を押して jj に作業コピーを取り込ませる前に確認する。konoma が jj リポジトリに書き込むのはここだけで、しかも指示されたときのみ。konoma 自身では気づけない「更新時刻が変わらないまま中身が変わった」ファイルのための逃げ道。`false` = 確認なしで取り込む。 |
| `confirm_bookmark_overwrite` | `true` | ブックマーク(`m`)が既存の**別のパス**を上書きするときに確認(`y`/`Enter`=上書き・`n`/`Esc`=取消)。同じパスの再登録・未使用キーは確認なし。`false` = 確認なしで即上書き。 |
| `csv_rainbow` | `true` | CSV/TSV テーブルの列レインボー。`false` = 単色(整列・セル移動はそのまま)。 |
| `filter_mode` | `"fuzzy"` | `/` のツリー絞り込みの照合方法。`"fuzzy"` = 飛び石一致をスコア順に並べる(fzf 流・空白区切りの語は AND)、`"substring"` = 従来の単純な部分一致。どちらも大文字小文字は無視。 |
| `follow_view` | `"diff"` | フォローモード(`F`)がジャンプ先をどう開くか: `"diff"`(**フォロー開始時点からの**全画面 diff — 開始前からあった未コミット変更は隠れる。diff 内 `f` でフル git diff に切替え可能) / `"file"`(通常プレビューで最初の変更ハンクへスクロール)。diff の無いファイルとメディアは常に `"file"` 相当。 |
| `busy_indicator` | `true` | バックグラウンド処理(git 無視ファイルスキャン・メディア読込・ハイライト準備・画像取得)の実行中だけ、右上にスピナーとジョブ名を表示。アイドル時は何も出ず負荷もゼロ。**実行中のファイル操作**(コピー/移動/複製/削除とその `N/M` 進捗)だけは、この設定に関わらず常に表示する — 裏の家事ではなく、待っているものそのものなので。 |
| `mermaid` | `"image"` | mermaid 図の描画方法。`"image"` は純 Rust でプロセス内ラスタライズ(mermaid.js 品質・ブラウザ/Node 不要): 単体 `.mmd` は全画面(ズームはズーム率に合わせて再ラスタライズ=拡大してもシャープ)、Markdown 内の ```mermaid フェンスはインライン表示(`Tab` でフォーカス=シアンの枠+図全体へ自動スクロール。`+`/`-`=**その場ズーム**(レイアウト不変)・ズーム中 `hjkl`=パン・`0`=フィット・`Enter`=全画面・`q`=戻る)。`"text"` は従来の Unicode 罫線描画。未対応の図・描画失敗・画像非対応端末は自動でテキストに降格。 |
| `mermaid_theme` | `"dark"` | 画像モードの図の配色テーマ: `"dark"`・`"light"`・`"classic"`(mermaid.js 既定)・`"forest"`・`"neutral"`。背景は常に透過(端末背景に馴染む)。 |
| `mermaid_rows` | `24` | Markdown 内インライン mermaid 図の表示高さの目標(行数)。**拡大方向にも効く**=ベクタ由来なので必要密度へ自動再ラスタライズされシャープなまま(幅は本文幅で頭打ち・縦横比維持)。初期表示は**ビューポートにもフィット**=窓が目標より低いときは全体が見える高さへ縮む。0/不正値は既定に戻る。 |
| `math` | `"image"` | LaTeX 数式の描画方法。`"image"`(既定)は `$…$` / `$$…$$` を RaTeX(純 Rust・KaTeX 品質・ブラウザ/Node 不要)でプロセス内ラスタライズしインライン画像で表示(ターミナルは画像を文中に置けないのでインライン数式は自前の行に持ち上がる・ディスプレイ数式は中央寄せ)。`"text"` は生 LaTeX を素テキストのまま。画像非対応端末・描画失敗は自動で生 LaTeX へ降格。 |
| `math_color` | `"#d0d0d0"` | 画像モード数式のグリフ色。RaTeX は数式を純黒で塗るため、ダーク端末では不可視。konoma はダーク端末前提なので、透過背景の上にこの色で塗り替える(端末背景が透ける=mermaid と同じ)。ライト端末では暗い色(例 `"#202020"`)を指定する。usvg が解釈する色(`#hex`・`rgb(…)`・CSS 色名)を受け付け、不明な値や完全透過はタイプミスでも数式を空白にせず既定へフォールバックする。 |
| `restore_tabs` | `true` | **起動ディレクトリ毎**に前回のタブ構成(各タブの root・ツリーカーソル・プレビュー)を保存し、同じディレクトリでの次回起動時に復元。保存はタブの開閉/切替と終了時、保存先は `~/.config/konoma/sessions/`。`false` で常にまっさらに起動(読みも書きもしない)。 |
| `restore_single_tab` | `true` | `restore_tabs` が有効なときのみ意味を持つ: タブが1枚だけのセッションも保存/復元するか。`false` にすると単一タブのセッションは保存しない — 1枚だけの状態で終了するとそのディレクトリのセッションファイルが削除され、次回はまっさらに起動する(2枚以上のタブがあるセッションは引き続き保存/復元される)。 |
| `tree_cursor` | `"origin"` | 親ディレクトリへ移動した(`h`)あと、カーソルがどこに乗るか。`"origin"` は出てきたディレクトリの行に乗る(プレビューから `q` で戻る時と同じ発想=出てきた場所へ戻る。先頭ではない)。`"top"` は従来どおり常に先頭の行(旧来の挙動)。未知の値は `"origin"` として扱う。出てきたディレクトリが親の一覧に無い場合(隠しディレクトリで `show_hidden=false` 等)は先頭にフォールバックする。ディレクトリへ下りる(`l`)方は常に先頭から始まる(下りる先には「戻る場所」が無いため)、これは変わらない。 |
| `md_task_states` | `[" ", "x"]` | Markdown チェックボックスで `Space` が巡回する状態(順序どおり・各要素1文字)。例 `[" ", "/", "x"]` で Obsidian 流の作業中状態(`[/]` 表示)。不正な設定は既定へ。 |
| `md_autolink` | `true` | Markdown プレビューで裸の URL・メールを自動リンク化(GFM autolink・GitHub と同じ)。素の `https://…` / `www.…` / `foo@bar.com` がフォーカス可能なリンクに(`Tab` で移動・`Enter` で開く)。コード span / コードフェンス内はリンク化しない。`false` で素テキストのまま。 |
| `md_alerts` | `true` | GitHub 形式のアラートを色付きコールアウト箱(アイコン+ラベル)で描く(素の引用でなく)。マーカー(大小無視+一般的な別名): `> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]`。`false` で通常の引用(マーカーはそのまま)。 |
| `md_emoji` | `true` | Markdown プレビューで `:shortcode:` 絵文字を実 Unicode に変換(GitHub と同じ・`:rocket:` → 🚀)。Unicode を持たない GitHub 独自ショートコード(`:shipit:` 等)とコード内はそのまま。絵文字幅が桁揃えを崩す場合は `false`。 |
| `md_frontmatter` | `true` | 先頭の YAML front matter(`---` … `---` が文書の最初)を認識し、罫線+生YAML でなくコンパクトな dim メタデータ block として表示。`false` なら通常の Markdown として描く。 |
| `md_footnotes` | `true` | GFM 脚注を描く: `text[^1]` の参照は上付き番号になり、`[^1]: …` の定義は末尾の番号付き脚注節にまとまる。`false` ならリテラル表示。 |
| `md_inline_html` | `true` | Markdown エンジンが剥がす一般的なインライン HTML を描く: `<del>`/`<s>`/`<strike>`=打消し線・`<kbd>`=インラインコードのキーキャップ・`<sup>`/`<sub>`=Unicode(対応する文字のみ)・`<br>`=ハード改行。(`<mark>`/`<ins>` はどちらでもテキストのみ。)`false` なら全タグを剥がす。 |
| `md_details` | `"auto"` | `<details>` の初期表示。`"auto"` は GitHub と同じく open 属性を尊重(`<details>` 折りたたみ / `<details open>` 展開)・`"open"` 常に展開・`"closed"` 常に折りたたみ。いずれも `Tab` で `<summary>` にフォーカス→`Space`/`Enter` でトグル。 |
| `md_table_align` | `"left"` | Markdown プレビューで**表の箱**をどこに置くか: `"left"`(左端から罫線が始まる)・`"center"`・`"right"`。GFM のパイプ表も HTML の `<table>` も箱ごと動き、セル内の画像も一緒に動く。**セルの中の整列は変えない**(列の `:---:` や HTML の `align=` がそのまま支配する)。プレビュー幅以上の表は左寄せのまま。 |
| `md_image_align` | `"center"` | Markdown プレビューで**ブロック画像**をどこに置くか: `"left"`・`"center"`・`"right"`。単独の `![alt](url)`・バッジの並び・mermaid フェンスの図(キャプションとフォーカス枠も追従)が対象。**表のセル内の画像**(セル自身の整列が支配)と**数式**(display の中央寄せは組版の約束事)は対象外。プレビュー幅以上の画像は左寄せのまま。 |

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
| `image` | 端末のグラフィックプロトコルで全画面表示(ズーム/パン・GIF は自動アニメ)。 |
| `svg` | プロセス内でラスタライズ(resvg・純 Rust)して画像表示。 |
| `video` | 代表フレーム1枚をサムネイル表示。**`.mp4`/`.m4v`/`.mov` と `.mkv`/`.webm` の H.264 と HEVC は純 Rust でデコード=外部ツール不要**(HEVC=iPhone の既定録画形式)。それ以外(VP9・AV1・旧世代コーデック・`.avi` コンテナ、および稀なプロファイル=H.264 の 10bit/4:2:2/4:4:4/モノクロ、Main/Main 10 4:2:0 以外の HEVC)は `ffmpegthumbnailer`/`ffmpeg` があれば使い、無ければヒント表示。いずれも端末内再生はしないので、再生したい場合は `command` で `mpv` へ。 |
| `pdf` | 純 Rust(`hayro`)でネイティブにラスタライズ(外部ツール不要・1ページずつ) — `J`/`K` で任意のページへ。macOS に限り、hayro が描画できない PDF(暗号化・破損など)は OS 同梱の `qlmanage`/`sips`(常在・導入不要)へフォールバックしますが、これらは**1ページ目**しか出せません。 |
| `csv` / `tsv` | 列レインボー+セルカーソルの整列テーブル(`hjkl` 移動・`y →` でコピー)。 |
| `archive` | `.zip`/`.tar`/`.tar.gz`/`.tgz` のエントリ(名前 / サイズ / 更新日時)を CSV/TSV と同じ整列テーブルで一覧表示 — メタデータのみで**展開はしません**(`hjkl` / `y →` c/r/C も同様に効きます)。 |
| `code` | シンタックスハイライト(文法は 拡張子 → ファイル名 → 先頭行 で解決)。 |
| `text` | 素のテキスト。テキストらしいファイルの自動フォールバック先でもあります。 |

外部コマンド委譲:

```toml
[[preview.rules]]
glob = "*.{mp4,mov}"
command = "mpv {path}"      # {path} = 対象ファイル, {out} = 一時出力パス
detached = true             # TUI をブロックしない(別プロセスで開く)

[[preview.rules]]
glob = "*.{puml,plantuml}"  # PlantUML は内蔵が無いので委譲する。
render_as = "image"         # (mermaid はルール不要＝konoma が内蔵で描く。) render_as=出力を画像として表示
command = "plantuml -tpng -pipe < {path} > {out}.png"
```

`render_as` を省略する(または `"image"` 以外を指定する)と、コマンドの出力をキャプチャして
通常の windowed リーダーでテキスト表示します。コマンドが不在/失敗(非ゼロ終了・`{out}` 未生成)
した場合はクラッシュせず、安全に `[can not preview: <ext>]` へ降格し理由が添えられます。

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
| `tool` | `"lazygit"` | 変更ハブ内の `!` で起動する外部 git ツール(コマンド+引数)。 |
| `diff` | `"unified"` | diff の初期レイアウト: `"unified"`(縦) / `"split"`(左右) / `"auto"`(幅で判断)。実行時は diff 内 `s` で巡回。 |
| `worktree_dir` | `"../"` | ワークツリー一覧の `n` が新しいワークツリーを作る場所。**メイン**ワークツリー基準で解決されるので、どのワークツリーから作っても位置がぶれません。ディレクトリ名はブランチ名（`/` は `-` に置換）。リポジトリ内を指すこともできますが、その場合は `.gitignore` か `.git/info/exclude` への登録が**自分で**必要です（konoma は書き足しません）。 |


## `[jj]` — jj (Jujutsu) 連携

**プレビュー版。** ツリー・diff・ハブは一通り揃っているが、`jj workspace` の一覧は未対応で、
jj 自体が 1.0 前・月次で破壊的変更がある。konoma はバージョンを決め打ちせず、起動時に一度
`jj` を試して、答えられなければ git に落ちる。

git に対応するものが無い設定だけを置く。jj リポジトリの扱いはそれ以外すべて `[external] vcs` と
jj 自身が決める。**konoma は jj リポジトリを読むだけ**で、全呼び出しに `--ignore-working-copy` が付くため
作業コピーをスナップショットせず、書き込み系のキーは提示しない。

| キー | 既定 | 説明 |
|---|---|---|
| `tool` | `"lazyjj"` | ハブで `!` を押したときに起動する外部 jj ツール(`[git] tool` の jj 版)。未導入なら起動できなかった旨を flash する。 |

## `[external]` — 外部プロセスの on/off スイッチ

konoma が起動する外部プロセスを1個ずつ on/off できます。全キーの既定は `true`。
`[external]` セクション自体が無い、あるいは中の項目が無い場合も既定は `true` のまま
なので、何も書かなければ挙動は変わりません。

| キー | 既定 | 説明 |
|---|---|---|
| `git` | `true` | git 連携: status の色・ガター・Git ビュー・stage/unstage/commit/checkout/branch(`src/git.rs`・git CLI + 組込み git2/libgit2 経由)。`false` は `--no-default-features`(git feature 無し)でビルドしたのと**全く同じ挙動**(読み取りは全て空/`None`、書き込みは全てエラー)。`o`(Git ビューを開く)は「repo でない」とは別の文言で無効を知らせます。なお**この設定に関わらず、`git` 実行ファイルが見つからない環境では git 連携は自動でオフになります**(初回使用時に一度だけ判定)。その場合 `o` は「ディレクトリが repo でない」ではなく「git が見つかりません」と知らせます。 |
| `git_tool` | `true` | `!` で起動する外部 git ツール(上の `[git] tool`・既定 lazygit)。 |
| `vcs` | `"auto"` | どのバージョン管理システムが答えるか。**jj 対応はプレビュー版**([git スイートのガイド](/ja/guides/git/))。`"auto"` は git が答えられる場所を git のままにするので、今まで動いていたリポジトリの見え方は変わらない。jj が答えるのは git のリポジトリが無い場所だけ(`jj git init --no-colocate` で作ったもの・`jj workspace`)。`"git"` は常に git。`"jj"` は `.jj` があれば colocated でも jj——実際に jj で作業しているならこれ。`jj` バイナリが無い環境では git に落ちる。 |
| `pdf` | `true` | **外部フォールバック**のラスタライザ(macOS 同梱の `qlmanage`/`sips`)。主レンダラ(`hayro`・純 Rust・このフラグに関係なくプロセス内で解析/描画)がその PDF の1ページ目を描画できなかった時(暗号化・破損など)だけ試されます。`false` にするとこれらの外部ツールは一切起動しませんが、PDF プレビュー自体(ページ描画・ページ数取得)は `hayro` により動作し続けます。**macOS 以外ではこのフラグは実質無効**です(起動する外部 PDF ツールがそもそも無いため)。 |
| `video` | `true` | **外部フォールバック**の抽出ツール(`ffmpegthumbnailer`/`ffmpeg`)。内蔵デコーダ(純 Rust・このフラグに関係なくプロセス内で常に動く)が扱えないファイル=`.mp4`/`.m4v`/`.mov` と `.mkv`/`.webm` の H.264/HEVC 以外の時だけ使う。`false` でもそれらを起動しないだけで、これらのコンテナの H.264/HEVC のサムネイルは出る(上の `pdf` と `hayro` の関係と同じ)。 |
| `remote_images` | `true` | Markdown 内の `http(s)://` 画像取得。konoma が行う唯一の外向きネットワーク通信です。`curl` 等の外部プロセスではなく `ureq`(rustls)でプロセス内実行します。 |
| `open_links` | `true` | URL/ファイルを OS のハンドラで開く(macOS は `open`、それ以外は `xdg-open`)。Markdown リンク・パス貼付ジャンプ(`P`)等。 |
| `preview_commands` | `true` | `[[preview.rules]] command = "..."` への委譲。`false` にすると、そのルールは「マッチしなかった」扱いになり `[can not preview]` へ落ちます(`markdown`/`image`/`pdf` 等の builtin レンダラには影響しません)。 |

無効化された機構は、任意ツールが不在のときと**同じ形で安全に降格**します: PDF/動画は
既存の「表示できません」ヒントへ、無効化したリモート画像は画像の代わりにテキストの
プレースホルダへ、無効化した `command` ルールは `[can not preview]` へ — クラッシュは
しません。

「1つだけ止めたい」場合も `[[preview.rules]]` で個別に対処しようとしないこと。上で
触れたとおり、ユーザー定義ルールを1つでも書くと**既定のルール一覧が丸ごと置き換わる**
ため、PDF や動画だけを狙って止めることができず、Markdown・画像・CSV 等も一緒に
効かなくなります。

`[ui] lang` は既に「明示 / `"auto"`」の切替を持っています(OS 言語取得は
`sys-locale` クレート経由 — 外部プロセスは起動しません)。`lang` を明示
(`"en"`/`"jp"`)すればこの取得自体が呼ばれないため、`[external]` に専用フラグは
足していません。

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
`preview_text_visual`・`preview_image`・`preview_table`・`table_cell`
(`preview_table` で `Enter` を押して開くセル全文ポップアップ)・`sort`・`bookmarks`・
`tabs`・`outline`・`info`・`help`、および(git ビルドで)`preview_git_diff`・
`git_changes`・`git_log`・`git_graph`・`git_graph_picker`・`git_branches`・
`git_worktrees`(変更ハブの `w` で開くリンクワークツリー一覧)・`git_detail`。

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
- **パスコピー**(`y` リーダー): `copy_name`・`copy_relative`・`copy_full`・`copy_parent`・`copy_at_ref`(AI チャット用 `@相対パス`)・`copy_code_block`(`y c` = `Tab` でフォーカス中の Markdown コードブロックをコピー。フォーカス中のときだけメニューに出ます)
- **ファイル管理**(`Space` リーダー): `file_create`・`file_rename`・`file_delete`・`file_copy`・`file_cut`・`file_paste`・`file_duplicate`(`Space→D`=カーソル/選択をその場に複製。例 `note copy.md`)
- **プレビュー**: `preview_back`・`search_start`・`search_next`・`search_prev`・`preview_enter_visual`(`v`)・`preview_enter_visual_line`(`V`)・`preview_copy_selection`・`preview_copy_selection_ref`(`Y` = `@path#L12-34`)・`toggle_markdown_raw`(`R`)・`link_focus_next/prev`・`link_open`(`Enter`=同タブ)・`open_link_new_tab`(`Ctrl-t`=別タブ)・`image_zoom_in/out/reset`・`pdf_next_page`・`pdf_prev_page`・`preview_next_file` / `preview_prev_file`(`Ctrl-n` / `Ctrl-p`=ツリー表示順で次/前のファイルへ。ディレクトリはスキップ・端で wrap)
- **テーブル**(`preview_table`): `table_copy_cell/row/column`(`y` リーダー)・`toggle_table_cell`(`Enter`= カーソル位置のセルの全文ポップアップ。グリッドは幅の広いセルを `…` で切り詰めるので、こちらは切り詰めない全文を折返しで表示し `j`/`k`/`g`/`G`/PageUp/PageDown でスクロールできます。`q`/Esc/`Enter` で閉じる。ポップアップ自身のキーは `[keys.table_cell]`)
- **Agent Watch**: `toggle_follow`(`F`)・`toggle_changed_filter`(`C`)・`jump_next_change`(`n`)・`jump_prev_change`(`N`)・`toggle_follow_diff_scope`(`f`、フォロー diff 内で「開始以降」⇄ フル git diff を切替)
- **Git**: `open_git_view`(`o`)・`open_git_diff_cursor`(`d`)・`git_stage`・`git_unstage`・`git_stage_all`・`git_unstage_all`・`git_discard`・`git_commit`・`git_open_log`・`git_open_graph`・`git_open_branches`・`git_launch_tool`(`!`・変更ハブ内)・`cycle_diff_layout`・`git_copy_*`・`branch_*`
- **jj**(`git_changes`): `jj_sync`(`R`= jj に作業コピーを取り込ませる。konoma が jj リポジトリに書き込む唯一の操作で、確認ダイアログが先に出る。git リポジトリでは何もしない)
- **Git グラフ**(`git_graph`): `git_graph_toggle_all`(`a`= バックエンドの既定範囲でなく全リビジョンを表示。狭い既定を持つのは jj だけ)・`git_graph_set_base`・`git_graph_clear_base`・`git_graph_open_picker`(`b`= ブランチ選択パネルを開く)。パネル内(`[keys.git_graph_picker]`)は `git_graph_picker_toggle`・`git_graph_picker_all`・`git_graph_picker_current_only`・`git_graph_picker_move_up`・`git_graph_picker_move_down`
- **Git ワークツリー**(`git_worktrees` — `git worktree add` で作る**リンク**ワークツリーのこと。このページの他の箇所で言う「ワークツリー」= 未コミットの作業ツリーとは別概念): `git_open_worktrees`(`w`・変更ハブ内)・`worktree_filter_start`(`/`= ブランチ名/パスで絞り込み)・`worktree_goto`(`Enter`・固定キー: このタブの root(と `open_dir`)を選択ワークツリーへ切替。config で別キーにも割当可)・`worktree_goto_new_tab`(`Ctrl-t`= 選択を別タブで開く。現在のタブは不変)・`worktree_create`(`n`= ブランチ名の入力ダイアログを開く。新規/既存は自動判別で追加の質問はしません → 上の `[git] worktree_dir` の位置にメインワークツリーの隣として `git worktree add` し、このタブをそこへ切替)・`worktree_show_changes`(`d`= 選択ワークツリーの base ブランチからの diff。**コミット済みと未コミットをまとめて**表示します — ワークツリーで作業するエージェントは途中でコミットしてしまうことが多く、未コミットのみの diff だと空になるためです。base が解決できない/何も積み上がっていない場合は未コミットのみの diff にフォールバックします → 上の `graph_base_branches` 参照)・`worktree_close`(`q`/Esc)。bare のメインワークツリー・locked/prunable なもの・現在アクティブなものは切替を拒否します(理由は flash で表示)。`worktree_show_changes` は現在アクティブなものでも動きます。
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

- **画像 / SVG / Mermaid / LaTeX 数式 / 動画サムネイル / PDF** は、グラフィック
  プロトコルを話す端末なら**実ピクセル**で描かれます — **kitty graphics**
  (Ghostty・kitty・WezTerm・Konsole)・**iTerm2**・**sixel**。konoma は kitty 向けに
  自前の圧縮転送を持つので、kitty 系がいちばん速く出ます。それ以外の端末では
  **ハーフブロックの近似表示**に落ちます(粗いですが映ります)。テキスト系の
  プレビュー(Markdown・コード・git diff・CSV・表)はどの端末でも完全に動きます。
- **アイコン**(`ui.icons = true`・既定)には Nerd Font グリフが必要:
  端末のフォールバックに `Symbols Nerd Font Mono` を足すか、NF 内蔵フォント
  (HackGen Console NF・UDEV Gothic NF …)を使用。無ければ `ui.icons = false`。
- **任意ツール**: `ffmpegthumbnailer`/`ffmpeg`(konoma 自身がデコードできない
  動画=VP9・AV1・旧世代コーデック・`.avi` のサムネイル)・`git` +
  `lazygit`(git スイート/外部ツール)。PDF・画像・SVG・Markdown・Mermaid・
  LaTeX 数式・CSV・`.mp4`/`.m4v`/`.mov` と `.mkv`/`.webm` の H.264/HEVC 動画
  サムネイルは**追加インストールが一切不要**で、純 Rust で描画されます。
  すべて不在時は安全に降格します。
