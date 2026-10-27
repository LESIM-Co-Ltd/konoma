//! The `?` help and the footer of each kind of preview, as literal tables in English and Japanese
//! (plugin stage 3, step 0: pins what the host shows today, so moving the key, help, table, canvas
//! and image code into shared crates can be shown not to change it). A line of the help is
//! `key column | description`; the footer is the tokens (`ops::footer::tokens`) and the last row
//! of the drawn screen. Regenerate on purpose with
//! `KONOMA_PRINT_SNAPSHOTS=1 cargo nextest run -p konoma help_footer --no-capture`
//! and read the output before pasting it in.

use super::canvas::{open_with, Draw, Opened, Painter};
use super::*;

/// What is previewed.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Text,
    Document,
    Table,
    Image,
    Canvas,
}

/// A session on a file of this kind, plus whatever keeps its sandbox alive.
fn open_kind(kind: Kind, lang: &str) -> (Sim, Box<dyn std::any::Any>) {
    let mut cfg = Config::default();
    cfg.ui.lang = lang.into();
    if let Kind::Canvas = kind {
        let o = open_with(
            "hf_canvas",
            Painter::new("hf_canvas", Draw::Pattern { cols: 20, rows: 6 }).with_acts(),
            90,
            26,
            lang,
        );
        let Opened { s, p, _dir } = o;
        return (s, Box::new((p, _dir)));
    }
    let dir = sandbox("hf_snapshot");
    seed_files(&dir);
    crate::test_support::write_solid_png(&dir.join("pic.png"), 8, 8, [10, 120, 200]);
    let mut s = Sim::with_config(&canon(&dir), cfg).with_picker();
    let file = match kind {
        Kind::Text => "notes.txt",
        Kind::Document => "readme.md",
        Kind::Table => "data.csv",
        Kind::Image => "pic.png",
        Kind::Canvas => unreachable!(),
    };
    s.select(file);
    s.enter();
    (s, Box::new(dir))
}

/// The footer (tokens, then the last two drawn rows) and then the `?` help body of a preview.
fn capture(kind: Kind, lang: &str) -> String {
    let (mut s, _guard) = open_kind(kind, lang);
    let mut out = String::new();
    let ctx = s.app.base_context();
    out.push_str(&format!("context: {ctx:?}\n"));
    out.push_str("[footer tokens]\n");
    for t in crate::ops::footer::tokens(&s.app, ctx) {
        out.push_str(&format!("{t}\n"));
    }
    out.push_str("[footer row]\n");
    // The drawn last row, with a wide glyph counted as the terminal shows it (`Sim::screen` would
    // leave the cell behind a wide glyph in the text).
    let dump = crate::test_support::buffer_dump(s.term.backend().buffer());
    let last = dump
        .lines()
        .take_while(|l| *l != "--- styles ---")
        .last()
        .unwrap();
    out.push_str(&format!("{last}\n"));
    s.key('?');
    assert_eq!(
        s.app.key_context(),
        Some(crate::keymap::Context::Help),
        "{kind:?}: `?` opens the help"
    );
    out.push_str("[help]\n");
    for line in crate::ui::help::help_lines(&s.app) {
        let text: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        out.push_str(&format!("|{}|\n", text.trim_end()));
    }
    out
}

/// Compare with the literal; with `KONOMA_PRINT_SNAPSHOTS` set, print the capture instead.
#[track_caller]
fn check(name: &str, got: &str, want: &str) {
    if std::env::var_os("KONOMA_PRINT_SNAPSHOTS").is_some() {
        println!("=====BEGIN {name}=====\n{got}=====END {name}=====");
        return;
    }
    assert_eq!(got, want, "{name} changed");
}

macro_rules! snap {
    ($test:ident, $kind:expr, $lang:expr, $name:expr, $want:expr) => {
        #[test]
        fn $test() {
            check($name, &capture($kind, $lang), $want);
        }
    };
}

snap!(
    help_footer_text_en,
    Kind::Text,
    "en",
    "text_en",
    r####"context: Text
[footer tokens]
j/k:scroll
/:search
v/V:select
Y:@ref
F:FOLLOW
C-n/C-p:next/prev file
q:back
?:help
e:edit
o:open in app
g/G:ends
h/l:hscroll
0/$:line ends
[/]:tab
,:view
C-f/C-b/PgDn/PgUp:page
[footer row]
25|j/k:scroll  /:search  v/V:select  Y:@ref  F:FOLLOW  C-n/C-p:next/prev file  q:back …|
[help]
|Preview: text / Markdown|
|  g / G           top / bottom|
|  h / l / ← →     horizontal scroll|
|  0 / $           line start / line end (horizontal)|
|  /  n / N        search; next / prev match or change|
|  v / V           v: select a character range   V: select whole lines (copy with y)|
|  Y               copy @path#L reference of caret / selection|
|  Ctrl-n / Ctrl-p preview the next / previous file (tree order, wraps)|
|  m / '           bookmark this file (a-z local / A-Z global) / open bookmark list|
|  r               refresh listing & git status|
|  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  page|
|  Ctrl-d / Ctrl-u half page|
|  q / Esc         back to tree|
||
|Movement|
|  j / k / ↑ ↓     scroll|
||
|Launch|
|  e               edit in external editor ($EDITOR / config)|
|  o               open in another app (OS default / Office)|
||
|Actions|
|  Space /         search|
|  Space v         select characters|
|  Space V         select lines|
|  Space Y         @ref|
|  Space F         FOLLOW|
|  Space a         next file|
|  Space b         previous file|
|  Space c         back|
|  Space ?         help|
|  Space e         edit|
|  Space o         open in app|
|  Space [         previous tab|
|  Space ]         next tab|
|  Space n         next match|
|  Space N         previous match|
|  Space m         mark|
|  Space '         bookmarks|
|  Space r         refresh|
|  Space d         path|
|  Space w         wrap|
|  Space t         new tab|
|  Space T         tab list|
|  Space P         goto path|
|  Space Q         quit konoma|
||
|Sort & display|
|  , p             path|
|  , w             wrap|
||
|Copy path|
|  y n             name|
|  y r             relative|
|  y f             full|
|  y p             parent|
|  y @             @ref|
||
|Tabs|
|  t               new tab right of this one|
|  [ / ]           prev / next tab|
|  1 - 9           jump to tab by number|
|  T               tab list (switch / close)|
||
|Global|
|  P               paste a path / GitHub link and jump there (reveal + preview)|
|  ?               toggle this help|
|  F               follow mode: auto-preview externally changed files|
|  Q               quit (back to tree in preview)|
"####
);
snap!(
    help_footer_text_ja,
    Kind::Text,
    "ja",
    "text_ja",
    r####"context: Text
[footer tokens]
j/k:スクロール
/:検索
v/V:選択
Y:@参照
F:追尾
C-n/C-p:次/前ファイル
q:戻る
?:ヘルプ
e:編集
o:アプリで開く
g/G:先頭/末尾
h/l:横移動
0/$:行頭/末
[/]:タブ
,:表示
C-f/C-b/PgDn/PgUp:ページ
[footer row]
25|j/k:スクロール  /:検索  v/V:選択  Y:@参照  F:追尾  C-n/C-p:次/前ファイル  q:戻る …|
[help]
|プレビュー: テキスト/Markdown|
|  g / G           先頭 / 末尾|
|  h / l / ← →     横スクロール(非折返し時)|
|  0 / $           行頭 / 行末へ(横)|
|  /  n / N        検索; 次 / 前の一致、未検索なら変更箇所|
|  v / V           v: 文字範囲を選択   V: 行を選択(y でコピー)|
|  Y               キャレット / 選択範囲の @path#L 参照をコピー|
|  Ctrl-n / Ctrl-p ツリー表示順で次/前のファイルをプレビュー(端で wrap)|
|  m / '           表示中ファイルをブックマーク(a-z ローカル/A-Z グローバル) / 一覧を開く|
|  r               一覧と git status を再読込|
|  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  ページ|
|  Ctrl-d / Ctrl-u 半ページ|
|  q / Esc         ツリーへ戻る|
||
|移動 (Movement)|
|  j / k / ↑ ↓     スクロール|
||
|起動 (Launch)|
|  e               外部エディタで編集 ($EDITOR / 設定)|
|  o               他のアプリで開く(Office 文書は Office アプリ)|
||
|操作|
|  Space /         検索|
|  Space v         文字を選択|
|  Space V         行を選択|
|  Space Y         @参照|
|  Space F         追尾|
|  Space a         次のファイル|
|  Space b         前のファイル|
|  Space c         戻る|
|  Space ?         ヘルプ|
|  Space e         編集|
|  Space o         アプリで開く|
|  Space [         前のタブ|
|  Space ]         次のタブ|
|  Space n         次の一致|
|  Space N         前の一致|
|  Space m         登録|
|  Space '         ブックマーク|
|  Space r         再読込|
|  Space d         パス|
|  Space w         折返し|
|  Space t         別タブ|
|  Space T         タブの一覧|
|  Space P         パス移動|
|  Space Q         konoma を終了|
||
|並べ替えと表示|
|  , p             パス|
|  , w             折返し|
||
|パスをコピー|
|  y n             名前|
|  y r             相対|
|  y f             フル|
|  y p             親|
|  y @             @参照|
||
|タブ (Tabs)|
|  t               右隣に新規タブ|
|  [ / ]           前 / 次のタブ|
|  1 - 9           番号でタブへジャンプ|
|  T               タブ一覧(切替 / 閉じる)|
||
|共通 (Global)|
|  P               パス / GitHub リンクを貼り付けてその位置へ移動(reveal + preview)|
|  ?               このヘルプの開閉|
|  F               追尾モード: 外部で変更されたファイルを自動プレビュー|
|  Q               終了 (プレビュー中はツリーへ戻る)|
"####
);
snap!(
    help_footer_document_en,
    Kind::Document,
    "en",
    "document_en",
    r####"context: Document
[footer tokens]
j/k:scroll
Tab:focus
O:outline
R:raw source
/:search
F:FOLLOW
C-n/C-p:next/prev file
q:back
?:help
e:edit
o:open in app
g/G:ends
[/]:tab
,:view
C-f/C-b/PgDn/PgUp:page
[footer row]
25|j/k:scroll  Tab:focus  O:outline  R:raw source  /:search  F:FOLLOW …|
[help]
|Preview: text / Markdown|
|  g / G           top / bottom|
|  h / l / ← →     horizontal scroll|
|  0 / $           line start / line end (horizontal)|
|  /  n / N        search; next / prev match or change|
|  Y               copy the @path reference|
|  R               toggle rendered / raw source (raw is selectable)|
|  O               outline|
|  Tab / ⇧Tab      focus md link / checkbox / code block / diagram / <details> (y c = copy code)|
|  Enter           open link (URL=browser / local=konoma / anchor=jump in place) / toggle checkbox or <details> / open diagram full screen|
|  Ctrl-t          open the focused local link in a new tab (anchors jump in place, URLs open in the browser)|
|  + / - / =       zoom the focused diagram in place (hjkl pan while zoomed, 0 fits)|
|  Enter           toggle focused checkbox (writes to the file)|
|  Enter           expand/collapse the focused <details>|
|  Ctrl-n / Ctrl-p preview the next / previous file (tree order, wraps)|
|  m / '           bookmark this file (a-z local / A-Z global) / open bookmark list|
|  r               refresh listing & git status|
|  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  page|
|  Ctrl-d / Ctrl-u half page|
|  q / Esc         back to tree|
||
|Movement|
|  j / k / ↑ ↓     scroll|
||
|Launch|
|  e               edit in external editor ($EDITOR / config)|
|  o               open in another app (OS default / Office)|
||
|Actions|
|  Space a         focus next item|
|  Space O         outline|
|  Space R         raw source|
|  Space /         search|
|  Space F         FOLLOW|
|  Space b         next file|
|  Space c         previous file|
|  Space d         back|
|  Space ?         help|
|  Space e         edit|
|  Space o         open in app|
|  Space [         previous tab|
|  Space ]         next tab|
|  Space n         next match|
|  Space N         previous match|
|  Space Y         @ref|
|  Space f         focus previous item|
|  Space m         mark|
|  Space '         bookmarks|
|  Space r         refresh|
|  Space h         path|
|  Space w         wrap|
|  Space t         new tab|
|  Space T         tab list|
|  Space P         goto path|
|  Space Q         quit konoma|
||
|Sort & display|
|  , p             path|
|  , w             wrap|
||
|Copy path|
|  y n             name|
|  y r             relative|
|  y f             full|
|  y p             parent|
|  y @             @ref|
||
|Tabs|
|  t               new tab right of this one|
|  [ / ]           prev / next tab|
|  1 - 9           jump to tab by number|
|  T               tab list (switch / close)|
||
|Global|
|  P               paste a path / GitHub link and jump there (reveal + preview)|
|  ?               toggle this help|
|  F               follow mode: auto-preview externally changed files|
|  Q               quit (back to tree in preview)|
"####
);
snap!(
    help_footer_document_ja,
    Kind::Document,
    "ja",
    "document_ja",
    r####"context: Document
[footer tokens]
j/k:スクロール
Tab:フォーカス
O:アウトライン
R:ソース表示
/:検索
F:追尾
C-n/C-p:次/前ファイル
q:戻る
?:ヘルプ
e:編集
o:アプリで開く
g/G:先頭/末尾
[/]:タブ
,:表示
C-f/C-b/PgDn/PgUp:ページ
[footer row]
25|j/k:スクロール  Tab:フォーカス  O:アウトライン  R:ソース表示  /:検索  F:追尾 …|
[help]
|プレビュー: テキスト/Markdown|
|  g / G           先頭 / 末尾|
|  h / l / ← →     横スクロール(非折返し時)|
|  0 / $           行頭 / 行末へ(横)|
|  /  n / N        検索; 次 / 前の一致、未検索なら変更箇所|
|  Y               @パス参照をコピー|
|  R               装飾表示 / ソース表示 を切替(ソースは選択可)|
|  O               アウトライン|
|  Tab / ⇧Tab      Markdown リンク/チェックボックス/コードブロック/mermaid図/<details>をフォーカス (y c=コードをコピー)|
|  Enter           リンクを開く (URL=ブラウザ/ローカル=konoma/アンカー=その場で移動)・チェックボックス/<details>はトグル・mermaid図は全画面|
|  Ctrl-t          フォーカス中のローカルリンクを別タブで開く(アンカーはその場で移動・URL はブラウザ)|
|  + / - / =       フォーカス中の図をその場でズーム(ズーム中 hjkl=パン・0=フィット)|
|  Enter           フォーカス中のチェックボックスをトグル(ファイルに書込み)|
|  Enter           フォーカス中の <details> を開閉|
|  Ctrl-n / Ctrl-p ツリー表示順で次/前のファイルをプレビュー(端で wrap)|
|  m / '           表示中ファイルをブックマーク(a-z ローカル/A-Z グローバル) / 一覧を開く|
|  r               一覧と git status を再読込|
|  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  ページ|
|  Ctrl-d / Ctrl-u 半ページ|
|  q / Esc         ツリーへ戻る|
||
|移動 (Movement)|
|  j / k / ↑ ↓     スクロール|
||
|起動 (Launch)|
|  e               外部エディタで編集 ($EDITOR / 設定)|
|  o               他のアプリで開く(Office 文書は Office アプリ)|
||
|操作|
|  Space a         次の項目へ|
|  Space O         アウトライン|
|  Space R         ソース表示|
|  Space /         検索|
|  Space F         追尾|
|  Space b         次のファイル|
|  Space c         前のファイル|
|  Space d         戻る|
|  Space ?         ヘルプ|
|  Space e         編集|
|  Space o         アプリで開く|
|  Space [         前のタブ|
|  Space ]         次のタブ|
|  Space n         次の一致|
|  Space N         前の一致|
|  Space Y         @参照|
|  Space f         前の項目へ|
|  Space m         登録|
|  Space '         ブックマーク|
|  Space r         再読込|
|  Space h         パス|
|  Space w         折返し|
|  Space t         別タブ|
|  Space T         タブの一覧|
|  Space P         パス移動|
|  Space Q         konoma を終了|
||
|並べ替えと表示|
|  , p             パス|
|  , w             折返し|
||
|パスをコピー|
|  y n             名前|
|  y r             相対|
|  y f             フル|
|  y p             親|
|  y @             @参照|
||
|タブ (Tabs)|
|  t               右隣に新規タブ|
|  [ / ]           前 / 次のタブ|
|  1 - 9           番号でタブへジャンプ|
|  T               タブ一覧(切替 / 閉じる)|
||
|共通 (Global)|
|  P               パス / GitHub リンクを貼り付けてその位置へ移動(reveal + preview)|
|  ?               このヘルプの開閉|
|  F               追尾モード: 外部で変更されたファイルを自動プレビュー|
|  Q               終了 (プレビュー中はツリーへ戻る)|
"####
);
snap!(
    help_footer_table_en,
    Kind::Table,
    "en",
    "table_en",
    r####"context: Table
[footer tokens]
h/j/k/l:cell
↵:view cell
/:search
y:copy
g/G:ends
C-n/C-p:next/prev file
q:back
?:help
e:edit
o:open in app
[/]:tab
,:view
[footer row]
25|h/j/k/l:cell  ↵:view cell  /:search  y:copy  g/G:ends  C-n/C-p:next/prev file  q:back …|
[help]
|CSV / TSV / spreadsheet / archive|
|  h / j / k / l / arrows  move between cells|
|  g / G           top / bottom|
|  0 / $           first / last column|
|  /  n / N        search cells; n / N jump to the next / previous match|
|  Enter           view the full cell text (wraps; scroll with j/k)|
|  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  page|
|  Ctrl-d / Ctrl-u half page|
|  Ctrl-n / Ctrl-p preview the next / previous file (tree order, wraps)|
|  m / '           bookmark this file (a-z local / A-Z global) / open bookmark list|
|  r               refresh listing & git status|
|  q / Esc         back to tree|
||
|Launch|
|  e               edit in external editor|
|  o               open in another app (OS default / Office)|
||
|Actions|
|  Space a         view cell|
|  Space /         search|
|  Space b         next file|
|  Space c         previous file|
|  Space d         back|
|  Space ?         help|
|  Space e         edit|
|  Space o         open in app|
|  Space [         previous tab|
|  Space ]         next tab|
|  Space n         next match|
|  Space N         previous match|
|  Space m         mark|
|  Space '         bookmarks|
|  Space r         refresh|
|  Space f         path|
|  Space t         new tab|
|  Space T         tab list|
|  Space P         goto path|
|  Space F         FOLLOW|
|  Space Q         quit konoma|
||
|Sort & display|
|  , p             path|
||
|Copy from table|
|  y c             cell|
|  y r             row|
|  y C             column|
|  y f             full|
||
|Tabs|
|  t               new tab right of this one|
|  [ / ]           prev / next tab|
|  1 - 9           jump to tab by number|
|  T               tab list (switch / close)|
||
|Global|
|  P               paste a path / GitHub link and jump there (reveal + preview)|
|  ?               toggle this help|
|  F               follow mode: auto-preview externally changed files|
|  Q               quit (back to tree in preview)|
"####
);
snap!(
    help_footer_table_ja,
    Kind::Table,
    "ja",
    "table_ja",
    r####"context: Table
[footer tokens]
h/j/k/l:セル
↵:セル表示
/:検索
y:コピー
g/G:先頭/末尾
C-n/C-p:次/前ファイル
q:戻る
?:ヘルプ
e:編集
o:アプリで開く
[/]:タブ
,:表示
[footer row]
25|h/j/k/l:セル  ↵:セル表示  /:検索  y:コピー  g/G:先頭/末尾  C-n/C-p:次/前ファイル  q:戻る …|
[help]
|CSV / TSV / 表計算 / アーカイブ|
|  h / j / k / l / arrows  セル間を移動|
|  g / G           先頭 / 末尾|
|  0 / $           先頭 / 末尾の列|
|  /  n / N        セルを検索(n / N で次/前の一致へ)|
|  Enter           セルの全文を表示(折返し・j/k でスクロール)|
|  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  ページ|
|  Ctrl-d / Ctrl-u 半ページ|
|  Ctrl-n / Ctrl-p ツリー表示順で次/前のファイルをプレビュー(端で wrap)|
|  m / '           表示中ファイルをブックマーク(a-z ローカル/A-Z グローバル) / 一覧を開く|
|  r               一覧と git status を再読込|
|  q / Esc         ツリーへ戻る|
||
|起動 (Launch)|
|  e               外部エディタで編集|
|  o               他のアプリで開く(Office 文書は Office アプリ)|
||
|操作|
|  Space a         セル表示|
|  Space /         検索|
|  Space b         次のファイル|
|  Space c         前のファイル|
|  Space d         戻る|
|  Space ?         ヘルプ|
|  Space e         編集|
|  Space o         アプリで開く|
|  Space [         前のタブ|
|  Space ]         次のタブ|
|  Space n         次の一致|
|  Space N         前の一致|
|  Space m         登録|
|  Space '         ブックマーク|
|  Space r         再読込|
|  Space f         パス|
|  Space t         別タブ|
|  Space T         タブの一覧|
|  Space P         パス移動|
|  Space F         追尾|
|  Space Q         konoma を終了|
||
|並べ替えと表示|
|  , p             パス|
||
|テーブルからコピー|
|  y c             セル|
|  y r             行|
|  y C             列|
|  y f             フル|
||
|タブ (Tabs)|
|  t               右隣に新規タブ|
|  [ / ]           前 / 次のタブ|
|  1 - 9           番号でタブへジャンプ|
|  T               タブ一覧(切替 / 閉じる)|
||
|共通 (Global)|
|  P               パス / GitHub リンクを貼り付けてその位置へ移動(reveal + preview)|
|  ?               このヘルプの開閉|
|  F               追尾モード: 外部で変更されたファイルを自動プレビュー|
|  Q               終了 (プレビュー中はツリーへ戻る)|
"####
);
snap!(
    help_footer_image_en,
    Kind::Image,
    "en",
    "image_en",
    r####"context: Image
[footer tokens]
+/-:zoom
=:fit
h/j/k/l:pan
C-n/C-p:next/prev file
q:back
?:help
e:edit
o:open in app
[/]:tab
,:view
[footer row]
25|+/-:zoom  =:fit  h/j/k/l:pan  C-n/C-p:next/prev file  q:back  ?:help  e:edit …|
[help]
|Preview: image|
|  + / -           zoom|
|  =               reset to fit|
|  h / j / k / l / arrows  pan (when zoomed/clipped)|
|  Ctrl-n / Ctrl-p preview the next / previous file (tree order, wraps)|
|  m / '           bookmark this file (a-z local / A-Z global) / open bookmark list|
|  r               refresh listing & git status|
|  q / Esc         back to tree|
||
|Launch|
|  e               edit in external editor|
|  o               open in another app (OS default / Office)|
||
|Actions|
|  Space +         zoom in|
|  Space -         zoom out|
|  Space =         fit|
|  Space a         next file|
|  Space b         previous file|
|  Space c         back|
|  Space ?         help|
|  Space e         edit|
|  Space o         open in app|
|  Space [         previous tab|
|  Space ]         next tab|
|  Space m         mark|
|  Space '         bookmarks|
|  Space r         refresh|
|  Space d         path|
|  Space t         new tab|
|  Space T         tab list|
|  Space P         goto path|
|  Space F         FOLLOW|
|  Space Q         quit konoma|
||
|Sort & display|
|  , p             path|
||
|Copy path|
|  y n             name|
|  y r             relative|
|  y f             full|
|  y p             parent|
|  y @             @ref|
||
|Tabs|
|  t               new tab right of this one|
|  [ / ]           prev / next tab|
|  1 - 9           jump to tab by number|
|  T               tab list (switch / close)|
||
|Global|
|  P               paste a path / GitHub link and jump there (reveal + preview)|
|  ?               toggle this help|
|  F               follow mode: auto-preview externally changed files|
|  Q               quit (back to tree in preview)|
"####
);
snap!(
    help_footer_image_ja,
    Kind::Image,
    "ja",
    "image_ja",
    r####"context: Image
[footer tokens]
+/-:ズーム
=:フィット
h/j/k/l:パン
C-n/C-p:次/前ファイル
q:戻る
?:ヘルプ
e:編集
o:アプリで開く
[/]:タブ
,:表示
[footer row]
25|+/-:ズーム  =:フィット  h/j/k/l:パン  C-n/C-p:次/前ファイル  q:戻る  ?:ヘルプ  e:編集 …|
[help]
|プレビュー: 画像|
|  + / -           ズーム|
|  =               フィットに戻す|
|  h / j / k / l / arrows  パン(拡大して見切れた時)|
|  Ctrl-n / Ctrl-p ツリー表示順で次/前のファイルをプレビュー(端で wrap)|
|  m / '           表示中ファイルをブックマーク(a-z ローカル/A-Z グローバル) / 一覧を開く|
|  r               一覧と git status を再読込|
|  q / Esc         ツリーへ戻る|
||
|起動 (Launch)|
|  e               外部エディタで編集|
|  o               他のアプリで開く(Office 文書は Office アプリ)|
||
|操作|
|  Space +         拡大|
|  Space -         縮小|
|  Space =         フィット|
|  Space a         次のファイル|
|  Space b         前のファイル|
|  Space c         戻る|
|  Space ?         ヘルプ|
|  Space e         編集|
|  Space o         アプリで開く|
|  Space [         前のタブ|
|  Space ]         次のタブ|
|  Space m         登録|
|  Space '         ブックマーク|
|  Space r         再読込|
|  Space d         パス|
|  Space t         別タブ|
|  Space T         タブの一覧|
|  Space P         パス移動|
|  Space F         追尾|
|  Space Q         konoma を終了|
||
|並べ替えと表示|
|  , p             パス|
||
|パスをコピー|
|  y n             名前|
|  y r             相対|
|  y f             フル|
|  y p             親|
|  y @             @参照|
||
|タブ (Tabs)|
|  t               右隣に新規タブ|
|  [ / ]           前 / 次のタブ|
|  1 - 9           番号でタブへジャンプ|
|  T               タブ一覧(切替 / 閉じる)|
||
|共通 (Global)|
|  P               パス / GitHub リンクを貼り付けてその位置へ移動(reveal + preview)|
|  ?               このヘルプの開閉|
|  F               追尾モード: 外部で変更されたファイルを自動プレビュー|
|  Q               終了 (プレビュー中はツリーへ戻る)|
"####
);
snap!(
    help_footer_canvas_en,
    Kind::Canvas,
    "en",
    "canvas_en",
    r####"context: Canvas
[footer tokens]
q:back
j:bump
x:star
C-n/C-p:next/prev file
?:help
e:edit
o:open in app
[/]:tab
,:view
[footer row]
25|q:back  j:bump  x:star  C-n/C-p:next/prev file  ?:help  e:edit  o:open in app  [/]:tab …|
[help]
|Preview: plugin view|
|  Ctrl-n / Ctrl-p preview the next / previous file (tree order, wraps)|
|  m / '           bookmark this file (a-z local / A-Z global) / open bookmark list|
|  r               refresh listing & git status|
|  Y               copy the @path reference|
|  q / Esc         back to tree|
||
|hf_canvas|
|  j               Add one to the counter|
|  x               Draw a star|
||
|Launch|
|  e               edit in external editor|
|  o               open in another app (OS default / Office)|
||
|Actions|
|  Space a         back|
|  Space j         bump|
|  Space x         star|
|  Space b         next file|
|  Space c         previous file|
|  Space ?         help|
|  Space e         edit|
|  Space o         open in app|
|  Space [         previous tab|
|  Space ]         next tab|
|  Space m         mark|
|  Space '         bookmarks|
|  Space r         refresh|
|  Space Y         @ref|
|  Space d         path|
|  Space t         new tab|
|  Space T         tab list|
|  Space P         goto path|
|  Space F         FOLLOW|
|  Space Q         quit konoma|
||
|Sort & display|
|  , p             path|
||
|Copy path|
|  y n             name|
|  y r             relative|
|  y f             full|
|  y p             parent|
|  y @             @ref|
||
|Tabs|
|  t               new tab right of this one|
|  [ / ]           prev / next tab|
|  1 - 9           jump to tab by number|
|  T               tab list (switch / close)|
||
|Global|
|  P               paste a path / GitHub link and jump there (reveal + preview)|
|  ?               toggle this help|
|  F               follow mode: auto-preview externally changed files|
|  Q               quit (back to tree in preview)|
"####
);
snap!(
    help_footer_canvas_ja,
    Kind::Canvas,
    "ja",
    "canvas_ja",
    r####"context: Canvas
[footer tokens]
q:戻る
j:加算
x:星
C-n/C-p:次/前ファイル
?:ヘルプ
e:編集
o:アプリで開く
[/]:タブ
,:表示
[footer row]
25|q:戻る  j:加算  x:星  C-n/C-p:次/前ファイル  ?:ヘルプ  e:編集  o:アプリで開く  [/]:タブ …|
[help]
|プレビュー: プラグインの表示|
|  Ctrl-n / Ctrl-p ツリー表示順で次/前のファイルをプレビュー(端で wrap)|
|  m / '           表示中ファイルをブックマーク(a-z ローカル/A-Z グローバル) / 一覧を開く|
|  r               一覧と git status を再読込|
|  Y               @パス参照をコピー|
|  q / Esc         ツリーへ戻る|
||
|hf_canvas|
|  j               カウンタを 1 増やす|
|  x               星を描く|
||
|起動 (Launch)|
|  e               外部エディタで編集|
|  o               他のアプリで開く(Office 文書は Office アプリ)|
||
|操作|
|  Space a         戻る|
|  Space j         加算|
|  Space x         星|
|  Space b         次のファイル|
|  Space c         前のファイル|
|  Space ?         ヘルプ|
|  Space e         編集|
|  Space o         アプリで開く|
|  Space [         前のタブ|
|  Space ]         次のタブ|
|  Space m         登録|
|  Space '         ブックマーク|
|  Space r         再読込|
|  Space Y         @参照|
|  Space d         パス|
|  Space t         別タブ|
|  Space T         タブの一覧|
|  Space P         パス移動|
|  Space F         追尾|
|  Space Q         konoma を終了|
||
|並べ替えと表示|
|  , p             パス|
||
|パスをコピー|
|  y n             名前|
|  y r             相対|
|  y f             フル|
|  y p             親|
|  y @             @参照|
||
|タブ (Tabs)|
|  t               右隣に新規タブ|
|  [ / ]           前 / 次のタブ|
|  1 - 9           番号でタブへジャンプ|
|  T               タブ一覧(切替 / 閉じる)|
||
|共通 (Global)|
|  P               パス / GitHub リンクを貼り付けてその位置へ移動(reveal + preview)|
|  ?               このヘルプの開閉|
|  F               追尾モード: 外部で変更されたファイルを自動プレビュー|
|  Q               終了 (プレビュー中はツリーへ戻る)|
"####
);
