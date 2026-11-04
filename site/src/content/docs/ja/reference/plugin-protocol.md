---
title: プラグインの通信規約
description: 子プロセスとの通信規約と、版2のプロジェクト宣言。
---

参照実装は `crates/konoma-plugin/src/proc/{proto,wire}.rs` です。
任意の言語で実装し、`konoma-plugin-test PROGRAM --file FILE` で検査できます。
SDK の `export!`、`--manifest`、`--serve` も同じ変換を使います。

## 通信と版の交渉

ホストは `PROGRAM ARGS --serve` を起動します。標準入力と標準出力は通信専用、
診断は標準エラー出力です。各フレームは4バイトの符号なし big-endian 長と
MessagePack 本文で構成されます。本文の上限は64 MiB、入れ子の上限は128です。
構造体はフィールド名付き map、列挙型は variant 名か単一要素の map です。
例は `"Reload"` や `{"OpenDiff":{"path":…}}` です。
パスは UTF-8 文字列ではなく Unix のパスのバイト列を表す MessagePack **bin** です。
JSON の manifest 出力では整数の配列になります。

最初にホストが `Hello { versions, host_version, side_dir, lang }` を送り、
子が `Ack { version, manifest }` または `Refused { supported, reason }` を返します。
`side_dir` は絶対パスです。大きなバイナリは既存の `WBin::Side` で受け渡します。
プロジェクトのための新しい受け渡し方式はありません。

現在の版は **2**、ホストは `[1, 2]` を提示します。SDK は共通の版の最大を選びます。
プロジェクト・入力ダイアログ・読み取り専用・再取得対象を宣言するプラグインは版2だけに対応します。
それ以外は版1と2に対応するため、版1だけのホストでも従来のファイルプラグインを
使えます。プロジェクトプラグインはそのホストに `Refused` を返します。
版2のホストは版1の子も受け付けます。

新しい `Project` 入力と遷移結果の variant は旧実装が読めないため、版を上げています。
確認の宣言を無視すると操作の意味も変わります。任意フィールドだけではこの交渉を
表せません。新しい manifest・操作フィールドは未指定なら省略し、行の印は false なら
省略します。省略時の既定値もそれぞれ未指定と false です。従来のメッセージの
版1のバイト表現を保ちます。SDK の両端と検査ツールは、版1の接続に
プロジェクトデータが流れることを拒否します。
画面要求・操作情報の新フィールドと空でない行の値も版2が必要です。
同じ変更内の未公開の版2を拡張するため、版3には上げません。

## 呼び出しと応答

呼び出しは `Open { sid, rid, input, cx }`、`View { sid, rid, req, cx }`、
`Act { sid, rid, action, cx }`、`Key { sid, rid, key, cx }` です。
`Close { sid }`、`Cancel { rid }`、`Shutdown` に応答はありません。
子は `FromChild { rid, answer }` を返し、`answer` は `Ok(reply)` または `Err(failure)` です。
成功時は `Opened { state }`、`Viewed { view, state }`、`Acted { outcome, state }` です。
`state` は `layout` と各宣言済み操作の有効状態を持ちます。
従来の file/content/embed 入力、表示型、キー、設定、エラー、取り消し、
ファイル受け渡しの規則は同じです。

翻訳文は `Tr = { en: string, others: [[language, string], …] }`。
英語が必須で、翻訳のない言語は英語を使います。

### プロジェクトの画面要求と操作（版2）

`View.req.project` はファイルプレビューでは省略します。プロジェクトでは
`{ screen: string, selection: string | null, tree_root?: Path, open_dir?: Path, home_dir?: Path }` です。`selection` は `PushScreen` の
不透明な値です。同じプロジェクトセッション内で画面が変わるため、入力ではなく
画面要求に載せます。SDKでは `ViewReq::new(cols, rows).project(ProjectView::new(screen).selection(value))` と組み立てます。

`Act.cx.project`・`Key.cx.project` はプロジェクト操作以外では省略し、次の形を取ります。

```text
ProjectAct {
  screen: string | null,
  selected: { kind: string, value: string, name?: string } | null,
  marked: [{ kind: string, value: string, name?: string }],
  confirmation: string | null,
  text: string | null,
  choice: string | null
}
```

ツリー・diffへの操作では画面IDを省略できます。選択行と印付き行は、表示セルや
行番号ではなく、一覧で宣言した `ListRow.kind`・`ListRow.value` をそのまま渡します。
確認と一覧選択の回答は宣言された `Choice.value`、文字入力の回答は入力した文字列です。
空文字の回答と未回答は区別します。SDKでは `ActCx::with_project(ProjectAct::new()...)` を使います。

## プロジェクト宣言（版2）

### 画面の表示とパスの文脈

`ProjectScreen.marks(bool)` と `.checks(bool)` で印（複数選択）とチェックの列を
固定します。既定は false で、線上では省略します。行の内容では幅を変えず、
チェック状態のない行もチェック列の空白4セルを取ります。行の `kind` は操作用の
メタデータで、文字として描きません。`ListRow::selectable(false)` は文言や見出しを
表し、反転・印・操作の対象にしません。空の一覧には、`Tr` で翻訳してスタイルを
付けた非選択行を返します。既定は true で、既存の一覧のバイト列を維持します。

`ProjectView` は省略可能な絶対パス `tree_root`・`open_dir`・`home_dir` も持ちます。
現在のツリーの root は `Input::Project` の根と違う場合もあり、`open_dir` はタブの
起動ディレクトリです。ホストは毎回の要求に渡し、パスの文脈が変われば表示を
取り直します。古い相手は省略できます。表示の好みはプラグイン設定として読み、
git と jj は `[plugins.git]` / `[plugins.jj]` の
`path_style = "relative" | "home" | "full"` を宣言します。
指定がない場合、ホストの既存のパス表示設定を使います。

### セッションの監視パス

`Session::watch_paths(&ViewCx) -> Result<Option<Vec<PathBuf>>, Failure>` は絶対パスを
返します。`None` は宣言の根からの相対 `watch_paths` を使い、`Some(vec![])` は
データの監視パスなしを意味します。ホストは開いた時と無効化の後に、セッションを
所有するワーカーで呼び、通知用ワーカーで監視を更新します。通知の判定とチップの
取り直しにも実効パスを使います。Git の linked worktree は、根の外にある gitdir の
参照先と共通メタデータのディレクトリも返します。gitdir ファイルが変わったら、
パスを解決し直します。

プロトコル2の子はハンドシェイクの manifest に `session_watch: true` を宣言します。
呼び出しは `WatchPaths { sid, rid, cx }`、返答は
`WatchPaths { paths: Path[] | null, state }` です。パスは絶対パスで、ホストの上限は
512件です。能力のない古い子には新しい要求を送らず、クライアントで `None` に
戻します。通常のプロトコル1のデータと既定値のフィールドのバイト列は維持します。

### 固定の宣言

任意の `Manifest.project` を、従来のファイル関連付けや `screens: string[]` の
キー文脈とは独立して追加できます。

```text
ProjectDecl {
  markers: [{ name: string, kind: "Any" | "File" | "Directory" }],
  screens: [{ id: string, title: Tr, key: character | null, row_kinds: string[], marks?: boolean, checks?: boolean }],
  chip: Tr | null,
  watch_paths: Path[],
  version_control: boolean
}
```

目印は大小文字を区別するファイル名の glob です。ツリーのルートから祖先へ探し、
複数の目印・プラグインが当たって構いません。辿るパスではなく項目名を表します。
`Any` は `.git` がファイルでもディレクトリでも当たります。
画面の順番はタブの順番、任意のキーは単一文字です。行の種類は画面ごとに宣言します。
chip はツリーの題の短い要約、監視パスはプロジェクトの根からの相対パスです。
変化するとプロジェクトのデータを取り直します。`version_control` は能力の有無だけを
表します。**能力の呼び出しは静的リンク専用**で、通信の RPC は追加しません。

プロジェクトのセッションでは、ホストは開いた直後、`RefreshTarget::Chip` を宣言した
操作の後、監視パスによる無効化の時に、ワーカーで `Session::chip` を呼びます。
結果を保持し、キー操作や描画のたびには呼びません。`Ok(None)` はホストの言語の
`ProjectDecl.chip` を使い、宣言が無ければ何も出しません。
空の `Some(Chip::default())` は宣言のチップも隠します。失敗や panic は flash で知らせ、
セッションを保って宣言のチップに戻ります。ファイルだけのセッションでは呼びません。

計算した `Chip` は styled `Line` を持ちます。ホストは保持する前に `Chip::clipped` を
1 回通し、最大64 span・512文字に収めます。制御文字と端末の画像用プレースホルダーを
除き、文字を切った場合は末尾を `…` にします。題は span の色を保ち、CJK を含む
端末のセル幅で切り詰めます。複数のチップは検出順と ` · ` の区切りを保ち、
各タブで検出したプロジェクトだけを表示します。

通信の版2の子は `session_chip: true` で呼び出しへの対応を広告します。
広告しない古い子には呼び出しを送らず、SDK の client が `Ok(None)` を返すため、
ホストは宣言のチップを表示します。現在、ホストのプロジェクト検出は静的に登録した
プロバイダーだけが対象です。

入力は `{"Project":{"root":Path,"marker":string}}` です。
検出した根の絶対パスと、当たった実際の項目名を渡します。
たとえば `*.xcworkspace` ではなく `MyApp.xcworkspace` です。
ファイルのバイト列を読む入力ではありません。

`Manifest.read_only` は静的な真偽値の宣言です。既定はfalseで、falseなら送信時に省略します。
ホストは `[plugins.<id>] read_only` とのOR条件で `writes` 操作を止めます。
このAPIには実行中の読み取り専用状態を上書きする仕組みを設けません。

## 操作とダイアログ（版2）

既存の `id`、`key`、`label`、`help`、`footer`、`screen`、`writes`、`destructive` の
意味は同じです。次の任意フィールドを追加します。

```text
project_context:
  { "Screen": { screen: string, row_kinds: string[] } }
  | "TreeSelection" | "Diff" | null
confirmation: { text: Tr, choices: [{ key: character, choice: Choice }], setting?: string } | null
text_input: { label: Tr, initial: string, keep_on_failure?: boolean } | null
choice_input: { label: Tr, choices: Choice[] } | null
refresh: RefreshTarget[]
Choice = { value: string, label: Tr }
```

空の `row_kinds` はその画面全体、`TreeSelection` はツリー選択のメニュー操作です。
`Enter` は `key: "Enter"` と、行の種類を1つ以上指定した画面の文脈で宣言します。
通常の操作と同じ宣言・検査の経路を通り、行の種類が異なれば別の Enter 操作を持てます。
確認の選択肢は `y` → `-d`、`!` → `-D` のように値を分けられます。

ホストは言語を選んだ後、確認文のすべての `{row}` を、操作を呼び出した行の
`name`（未指定なら `value`）に置き換えます。選択行がなければ空文字です。
呼び出し時の行を保持するため、ダイアログを開いている間にカーソルが動いたり
一覧が更新されたりしても、確認文の行名は変わりません。他の置換記法や
エスケープはなく、`{row}` のない文はそのまま表示します。

`Confirmation::setting(key)` は `[plugins.<id>]` の真偽値設定で確認の有無を
切り替えます。`cx.settings` と同じ設定を使い、未指定なら宣言された既定値を使います。
`false` の場合だけ確認を省略し、有効な値が見つからない場合や真偽値以外では確認します。
後続の文字入力・一覧選択は表示し、操作は通常どおり実行します。`setting` の既定は
`None` で送信時に省略するため、古い子プロセスの確認は常に表示します。
破壊的操作の確認への設定指定と、真偽値の既定値を持つ `SettingDecl` がないキーは
登録時に拒否し、それぞれのエラーに操作名とキーを含めます。破壊的操作の確認は必須です。

`TextInput::keep_on_failure()` を指定すると、`act` が `Failure` を返した際に
文字入力を開き直します。panic や子プロセスの失敗も対象です。ホストは失敗を通知し、
送信した文字列を復元してカーソルを末尾に置きます。呼び出し時の行・画面・回答を
保持するため、Enter で同じ操作を再試行できます。文字入力だけを復元し、前段の
確認は繰り返しません。呼び出したタブがアクティブで、同じプロジェクト画面を表示し、
他のダイアログや文字入力がない場合だけ復元します。それ以外は失敗の通知のみです。
成功や `Flash` を含むすべての `Ok` では開き直しません。操作成功後の監視パス取得や
表示更新の失敗でも開き直しません。既定は false で、false の
場合は送信時に省略するため、古い子プロセスの動作は変わりません。

`RefreshTarget` は `"CurrentScreen"`・`{"Screen":{"screen":string}}`・`"AllScreens"`・
`"TreeStatus"`・`"Chip"` のいずれかです。操作の `refresh` は既定で空、空なら送信時に
省略します。実行後は結果を通知し、宣言した対象を取り直します。指定する画面名は
同じプラグインのプロジェクト宣言に存在する必要があり、再取得対象は操作ごとに最大64件です。

登録時には、空でない確認の選択肢を持たない破壊的操作、画面 id の重複（従来の
画面名との衝突を含む）、画面キーの重複、新しい型付き文脈の未宣言の画面・行の種類、
検出目印のないプロジェクトを拒否します。従来の `screen` キー文脈は従来どおりです。
SDK の型付き理由をホストが英語・日本語に翻訳します。名前と宣言数の上限は、
プロジェクトの画面・行の名前、目印と行の種類の数にも適用します。

監視パスは空でない相対パスで、親・ルート・接頭辞の要素を含めず、最大512件です。
検出目印は `.`・`..` 以外の空でない単一のファイル名要素で、最大1024バイトです。
画面キーには既存の一覧用予約表を使います。確認キーにはそのGlobal層と `q`・Escを
予約し、`y`・`!` は使えます。両方とも制御文字と同じ集合内の重複キーを拒否します。
確認の選択肢は1〜64件、一覧入力の選択肢も最大64件です。選択肢の値は最大1024バイト、
ラベルは翻訳ごとに最大512文字です。画面IDが `tree` などでも、キー文脈は専用の名前空間です。
単体アプリがプロジェクト遷移に対応するまでは、5種類の結果に未対応の通知を表示します。
版の不一致ではmanifestから決まる実際の対応版を返し、manifest取得中のpanicも拒否応答にします。

ホストが画面とダイアログを描き、入力の回答を渡します。共通の書き込みガードを
通してから、プロジェクトのワーカーで操作を実行します。

## 一覧と遷移（版2）

一覧は従来どおり `{"List":[row, …]}` です。各行は `cells: Line[]`、`kind: string`、
`target: Path | null`、`checked: boolean | null` と、新しい `marked: boolean` を持ちます。
不透明な行の値 `value: string` も持ち、SDKの `ListRow::value` で指定します。
既定の空文字は送信時に省略し、従来の行のバイト表現を保ちます。
`ListRow::name(impl Into<String>)` は確認用の短い名前を任意で指定します
（`Option<String>`、既定は `None`）。未指定の線上フィールドは省略し、古い子プロセス
では `None`、古いホストでは無視されます。選択行と印付き行の `ProjectRow` にも渡り、
`ProjectRow::new(kind, value).name(name)` で直接設定できます。
従来の行の種類・行き先・プラグインのチェック状態の意味を保ち、`marked` は独立した
ホストの複数選択の印です。セルは既に
`Line { spans: [[text, Style], …], style: Style }` で、グラフの罫線もこの span で表せます。
通信上の各 span は2要素の配列です。`Style` は `fg`、`bg`、`add`、`sub` を持ち、
後ろの2つは文字修飾のビットマスクです。
差分の hunk と行の符号化は変わりません。

従来の `Unhandled`、`Redraw`、`Flash(Tr)`、`Reload` に、次の結果を追加します。

| 結果 | データ | ホストの動作 |
|---|---|---|
| `PushScreen` | `{ screen, selection: string \| null }` | 不透明な行識別子を渡し、同じプラグインの画面を重ねる |
| `OpenFile` | `{ path: Path, line: integer \| null }` | プレビューを開く。行番号は1始まり |
| `OpenDiff` | `{ path: Path }` | ホストの全画面 diff を開く |
| `SwitchRoot` | `{ root: Path }` | ツリーのルートを切り替える |
| `OpenTab` | `{ path: Path }` | ホストの新しいタブで開く |

遷移先は絶対パス、NUL バイトなし、従来のパス長上限内とします。
**`q` で画面を1つ戻すのはホストの動作**です。新しい結果やガードを迂回する操作には
しません。ワーカーが結果を返してから、ホストが遷移を適用します。

検査する例は `crates/konoma-plugin-test/tests/vectors/project-v2.json`・`project-calls-v2.msgpack.hex` と
`tests/project_vectors.rs`、各新型と各結果の往復テストは SDK にあります。
