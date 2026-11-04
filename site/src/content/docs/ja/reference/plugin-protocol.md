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
プロジェクトか入力ダイアログを宣言するプラグインは版2だけに対応します。
それ以外は版1と2に対応するため、版1だけのホストでも従来のファイルプラグインを
使えます。プロジェクトプラグインはそのホストに `Refused` を返します。
版2のホストは版1の子も受け付けます。

新しい `Project` 入力と遷移結果の variant は旧実装が読めないため、版を上げています。
確認の宣言を無視すると操作の意味も変わります。任意フィールドだけではこの交渉を
表せません。新しい manifest・操作フィールドは未指定なら省略し、行の印は false なら
省略します。省略時の既定値もそれぞれ未指定と false です。従来のメッセージの
版1のバイト表現を保ちます。SDK の両端と検査ツールは、版1の接続に
プロジェクトデータが流れることを拒否します。

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

## プロジェクト宣言（版2）

任意の `Manifest.project` を、従来のファイル関連付けや `screens: string[]` の
キー文脈とは独立して追加できます。

```text
ProjectDecl {
  markers: [{ name: string, kind: "Any" | "File" | "Directory" }],
  screens: [{ id: string, title: Tr, key: character | null, row_kinds: string[] }],
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

入力は `{"Project":{"root":Path,"marker":string}}` です。
検出した根の絶対パスと、当たった実際の項目名を渡します。
たとえば `*.xcworkspace` ではなく `MyApp.xcworkspace` です。
ファイルのバイト列を読む入力ではありません。

## 操作とダイアログ（版2）

既存の `id`、`key`、`label`、`help`、`footer`、`screen`、`writes`、`destructive` の
意味は同じです。次の任意フィールドを追加します。

```text
project_context:
  { "Screen": { screen: string, row_kinds: string[] } }
  | "TreeSelection" | "Diff" | null
confirmation: { text: Tr, choices: [{ key: character, choice: Choice }] } | null
text_input: { label: Tr, initial: string } | null
choice_input: { label: Tr, choices: Choice[] } | null
Choice = { value: string, label: Tr }
```

空の `row_kinds` はその画面全体、`TreeSelection` はツリー選択のメニュー操作です。
`Enter` は `key: "Enter"` と、行の種類を1つ以上指定した画面の文脈で宣言します。
通常の操作と同じ宣言・検査の経路を通り、行の種類が異なれば別の Enter 操作を持てます。
確認の選択肢は `y` → `-d`、`!` → `-D` のように値を分けられます。

登録時には、空でない確認の選択肢を持たない破壊的操作、画面 id の重複（従来の
画面名との衝突を含む）、画面キーの重複、新しい型付き文脈の未宣言の画面・行の種類、
検出目印のないプロジェクトを拒否します。従来の `screen` キー文脈は従来どおりです。
SDK の型付き理由をホストが英語・日本語に翻訳します。名前と宣言数の上限は、
プロジェクトの画面・行の名前、目印と行の種類の数にも適用します。

この段階は宣言と通信までです。ホストのダイアログ、入力の回答、画面の描画、
書き込みの一元ガードは後の変更で接続します。

## 一覧と遷移（版2）

一覧は従来どおり `{"List":[row, …]}` です。各行は `cells: Line[]`、`kind: string`、
`target: Path | null`、`checked: boolean | null` と、新しい `marked: boolean` を持ちます。
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
しません。この変更では宣言と通信を追加し、ホストの動作は次の段階で接続します。

検査する例は `crates/konoma-plugin-test/tests/vectors/project-v2.json` と
`tests/project_vectors.rs`、各新型と各結果の往復テストは SDK にあります。
