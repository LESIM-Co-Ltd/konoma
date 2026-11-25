---
title: プラグインプロトコル
description: Rust SDK を参照実装とする、言語非依存の子プロセスプロトコル第1版。
sidebar:
  order: 3
---

現在の Unix 子プロセス通信の仕様です。Rust の `konoma-plugin` crate が参照実装です。Rust での作り方は[プラグインを書く](../../guides/write-a-plugin/)を参照してください。通信プロトコルの版は**1**、別の値であるプラグイン API の版は `[0, 1]` です。

## 起動と終了

konoma は設定されたプログラムを `<command> <args> --serve` として起動し、最後に `--serve` を追加します。シェルは介しません。複数プラグインを持つプログラムでは `args` に `--plugin <id>` を入れます。1プロセスで1プラグインを提供します。`konoma --plugin <path>` はその実行だけでプログラムを読み込み、相対プログラムパスを絶対化します。製品はユーザーのカレントディレクトリではなく専用の非公開ディレクトリで起動するため、設定の `args` のスクリプト・ファイルには絶対パスを使ってください。SDK 単体の `CommandLaunch` は指定がなければ `/` を使います。一方 `konoma-plugin-test` は呼び出し元のディレクトリを継承し、相対スクリプトパスを使えます。

環境変数はホストから継承します。`CommandLaunch` は環境を消去・追加しません。実行ファイルの探索にはホストの `PATH` を使います。専用の必須環境変数はなく、言語・設定・端末能力はメッセージで受け取ります。パイプに端末があると仮定しないでください。stdin は受信フレーム、stdout は送信フレーム、stderr は診断用です。SDK の `serve` は通信 fd を close-on-exec で複製し、通常の stdout を stderr、通常の stdin を `/dev/null` に向けます。他言語ではログによる通信の破損や、補助プロセスへの fd 継承でパイプが残ることを自分で防いでください。ホストは専用プロセスグループで起動し、core dump を無効にします。プラグインはユーザーの権限で動きます。メッセージの読み取り範囲は協調動作とホストの読み取りに対する規則で、OS のサンドボックスではありません。

SDK は `--manifest`（wire manifest の整形 JSON）、`--version`（選択したプラグインごとに `<id> <version>` の1行）、`--help` も受け付けます。別プロセス起動時のモードで、通信メッセージではありません。複数プラグインの `--serve` と `--manifest` は `--plugin <id>` が必要です。`--version` 単独は全件を列挙します。1プラグインなら選択指定を省略できます。ファイル引数で独立した端末表示を起動できるのは SDK の `app` feature を組み込んだ場合だけです。製品は `--manifest` を呼ばず、handshake で宣言を取得します。konoma 本体の子プログラムの読み込み処理と `konoma-plugin-test` は、非 Rust プラグインの `--manifest`・`--version` を要求しません。任意の便利なモードです。この2つの利用者向けには `--serve`（複数プラグインを提供する場合は `--plugin` の選択も）を実装してください。

## 通信パイプを保護する

必要なのは元の stdin/stdout を通信バイトだけに使うことです。ホストは特定の fd 操作を要求・検査しません。SDK は次の Unix 手順で通信パイプを保護します。

1. 元の fd 1、次に fd 0 を `fcntl(fd, F_DUPFD_CLOEXEC, 3)` で複製し、通信の出力・入力用に保持します。最小 fd 3 により0・1・2を上書きしません。
2. `dup2(2, 1)` で通常の stdout を診断用 stderr に向けます。
3. `/dev/null` を `O_RDONLY | O_CLOEXEC` で開き、`dup2(null_fd, 0)` し、余分な null fd が2より大きければ閉じます。通常の読み込みは通信を消費せず EOF を読みます。
4. 保存した fd だけでフレームを読み書きします。close-on-exec により exec した補助プログラムが通信パイプを保持し続けるのを防ぎます。診断は stderr に書きます。

他言語の同等の fd 操作でも構いません。SDK は複製・stdout 切替のエラーを返しますが、`/dev/null` の切替は best-effort です（`open` 失敗時は fd 0 を維持し、この箇所の `dup2` の結果は未検査）。この fallback を、ライブラリが stdin を消費しない保証だと解釈しないでください。

## フレームと値の表記

各フレームの形式は次のとおりです。

```text
offset  bytes   意味
0       4       本体のバイト長、符号なし big-endian（この4バイトを含まない）
4       length  MessagePack の値1個
```

フレームごとに flush します。本体の上限は**67,108,864 バイト（64 MiB）**。本体の読み込み前に検査します。デコードの深さは最大**128**、値の後の余剰バイトはエラーです。フレーム間の EOF は正常な通信終了、ヘッダ・本体途中の EOF は切断エラーです。magic prefix、区切り文字、JSON、圧縮、handshake 前の要求エンベロープはありません。

以下では `S` = UTF-8 の MessagePack string、`B` = MessagePack **bin**、`bool`、`u8/u16/u32/u64` = その範囲の非負整数、`i64`、`f64` を使います。`T?` は `nil` または `T`、`[T]` は配列、`(A,B)` は map ではなく2要素配列です。構造体は記載した綴りのキーを持つ map。参照 encoder と同じく全フィールドを送ってください（nullable は `nil` 可）。現在の serde decoder は省略された nullable も `nil` として読み、`Text.note` には明示的な既定値があります。nonnullable の欠落は失敗し、未知の構造体フィールドは無視しますが、フィールドの型を変更できる意味ではありません。map のキー（フィールド名・enum タグ）は文字列で、`[action_id, enabled]` などの tuple は配列です。例えば manifest の `Act.screen` は `screen:nil` と `screen` の省略をどちらも受け付け、主表示の context（`shows` で宣言した型）を使います。`screen:"text"` ならその context を選びます。必須の nonnullable である `Act.id` は省略・`nil` のどちらも不可。`Text.note` は明示的に `nil` が既定ですが、nonnullable の省略値すべてに既定値が付くわけではありません。

enum は外部タグ形式です。unit variant の `Shutdown` は文字列 `"Shutdown"`、1値の variant は `{"Text": value}`、名前付きフィールドは `{"Open": {fields...}}`、tuple variant は `{"Rgb": [r,g,b]}` です。名前の大文字小文字は区別します。`Option` に `Some` の包みはありません。文字型は Unicode scalar 1個の文字列です。整数のバイト幅は固定する必要がありません。`WPath` は Unix パスのバイト列を入れた **bin** そのもので、UTF-8 string ではありません。NUL と16,384バイト超を拒否し、読み取り対象のパスは絶対パスが必要です。ホストの入力パスは絶対化され、子の相対 `view` パスは変換エラーになります。SDK の byte visitor はデコード時に `u8` 配列も受け付けますが、参照符号化として bin を送ってください。

## 接続開始

最初はホストが `Hello` を送り、子はそれを受け取る前に何も送信しません。`Hello` は次の裸の map で、`{"Hello": ...}` ではありません。応答は `HelloReply` の `{"Ack": {version:..., manifest:...}}`（または `Refused`）です。`Ack` にはタグがありますが、どちらも通常の `ToChild` 呼び出しや `{rid,answer}` 応答ではありません。

```text
Hello = {versions:[u32], host_version:S, side_dir:WPath, lang:S}
HelloReply = Ack {version:u32, manifest:Manifest}
           | Refused {supported:[u32], reason:S}
```

`versions` は現在 `[1, 2]`。ホストは現在の通信版とその1つ前を提示します。プロジェクト・入力ダイアログ・読み取り専用・再取得対象を宣言するプラグインは版2だけに対応します。画面要求・操作情報の新フィールドと空でない行の値も版2が必要です。同じ変更内の未公開の版2を拡張するため、版3には上げません。`host_version` は参考情報のプログラム版で、通信版ではありません。`side_dir` は受け渡し用の絶対ディレクトリパス、`lang` は最初の言語です。後続呼び出しにも個別の言語が含まれます。

提示された版から選んで `Ack` を返します。対応する版がなければ対応版一覧と理由を付けた `Refused` を返します。提示されていない版、読めない manifest、設定の `id` と異なる manifest `id` はホストが拒否します。拒否すると起動は失敗し、ホストはその接続を閉じてプロセスグループを終了させ、通常の呼び出しを送りません。SDK は `Refused` を flush して正常終了します。検査ツールは正常終了または拒否後も生存しているプロセスを認めますが、クラッシュは拒否します。同じ接続での再 handshake を要求しません。SDK server は `Hello.host_version` と `Hello.lang` を検証しません。API の互換検査は別で、major 一致、major 0 では minor も完全一致、それ以外ではプラグインの minor がホスト以下であることが必要です。konoma はプラグインの登録時に API 版を比較し、互換でなければプラグインを無効にして起動時に知らせます。

SDK は独立したメッセージ識別子で判定せず、最初の値を直接 `Hello` としてデコードします。例えば最初が `"Shutdown"`、必須フィールド欠落、型違いなどでデコードできない場合は `Refused` や call error を返さず、serving が失敗し、CLI が stderr に診断を書いて終了コード1で終了します。余分なフィールドがあっても schema が合う map は `Hello` として読めるため、送信側が別メッセージのつもりだっただけでは拒否しません。最初のフレーム前の正常な EOF は、何も送らず正常終了します。

## 呼び出しと応答

`Ack` 後はホストが次の enum を送ります。`sid` はホストが選ぶセッション `id`、`rid` は接続内で一意の要求 `id` です。SDK ホストの両カウンタは1から始まります。全セッションの処理は到着順に直列実行し、別の reader が実行中にもキャンセルを受信します。

| ホストのメッセージ | フィールド | 成功時の応答・効果 |
|---|---|---|
| `Open` | `sid:u64, rid:u64, input:Input, cx:Cx` | `Opened {state:State}`、セッション作成 |
| `View` | `sid:u64, rid:u64, req:ViewReq, cx:Cx` | `Viewed {view:View, state:State}`、現在の表示を取得 |
| `Act` | `sid:u64, rid:u64, action:S, cx:Cx` | `Acted {outcome:Outcome, state:State}`、宣言済み操作 id |
| `Key` | `sid:u64, rid:u64, key:Key, cx:Cx` | `Acted {outcome:Outcome, state:State}`、通常 canvas 入力中の生キー |
| `Cancel` | `rid:u64` | 単独の応答なし、元の呼び出しの終了を要求 |
| `Close` | `sid:u64` | 応答なし、そのセッションを破棄 |
| `Shutdown` | なし（unit string） | 応答なし、セッションを破棄し正常終了 |

呼び出しごとに `FromChild = {rid:u64, answer:Answer}` を1回返します。`Answer = Ok(Reply) | Err(Failure)` で、`Reply` は上の3種です。全呼び出しで `Err` を返せます。エラーに新しい状態は含みません。成功応答の種別違い、未知・重複した応答 `rid` は接続を終了させます。現在のホストは `rid` で振り分け、応答順を明示検査しません。直列処理は参照 server の動作で、期限計算の前提です。server は要求 `id`・セッション `id` の重複を独立には検査せず、同じ `Open.sid` は旧セッションを破棄して置き換えます。未知セッションの呼び出しは `Err(Corrupt)`、未知の `Close` と `Cancel` は無視します。参照 server の上限は1,024セッションで、それ以上の新規 `open` は `TooHeavy` です。

`Cancel` は即時応答ではなくフラグを立てます。待機中に中断された呼び出しは worker に到達するとプラグインコードを実行せず `Err(Cancelled)` を返します。完了済み `view` の成功も許されます。実行中はフラグを確認して速やかに応答してください。SDK は `Shutdown` で実行中・待機中の呼び出しも中断し、キュー処理後に `sid` 昇順でセッションを破棄します（ホストの割り当てでは `open` 順）。stdin の EOF でも全フラグを立てて終了し、処理が止まっていれば `serve` が2秒後にプロセスを終了させます。不正な入力では server がエラー終了します。SDK 呼び出し内の panic は `Err(External)`、`manifest()` の panic は起動失敗になります。panic の捕捉は SDK 固有の補助動作です。

```text
State = {layout:Layout, enabled:[(S,bool)]}
Layout = {reprs:[(S,Tr)], default_repr:S?, default_repr_without_pixels:S?, pages:Pages?}
Pages = Separate {count:u64?, titles:[S]} | Anchored {heading_level:u8, count:u64}
ViewReq = {repr:S?, page:u64?, cols:u16, rows:u16, cell_px:(u16,u16)?}
Outcome = Unhandled | Redraw | Flash(Tr) | Reload
Failure = {kind:FailureKind, message:Tr, detail:S?}
FailureKind = Unsupported | Encrypted | Corrupt | TooLarge | TooHeavy | Timeout
            | Cancelled | Io | External
Key = {code:KeyCode, mods:u8}
KeyCode = Char(character) | Enter | Backspace | Delete | Tab | BackTab | Left | Right
        | Up | Down | Home | End | PageUp | PageDown | Esc | F(u8) | Paste(S)
```

`repr` は表現を選び、`nil` は既定です。`page` は0起点で `nil` は最初または全体。wire 変換は `page` が受信側の index 型に収まるかだけを検査し、宣言済みページ数との比較はしません。`cols/rows` は0も可、`cell_px` は判明している場合のセル幅・高さのピクセル数です。`layout` と `enabled` は成功応答で更新するスナップショット。`enabled` にない操作は `true`、重複 `id` は最後の値が有効です。`Manifest.actions` は操作ごとのキー・ラベル・ヘルプを、`State.enabled` は現在使えるかを伝えます。konoma はこの2つから、使用可能な操作をフッター・ヘルプ・メニューに表示します。有効な操作には空でない英語のラベルとヘルプを用意してください。checker の `hints` 検査は入力を開いて `view` を要求し、宣言キーと、その状態からホストが表示するラベル・ヘルプを検査します。`Viewed` の状態を使い、正しい `View` エラー応答なら `Opened` の状態を維持します。`Unhandled` は未使用、`Redraw` は `view` 再取得、`Flash` は翻訳メッセージ表示、`Reload` は入力の再 `open` 要求です。生キーの修飾ビットは Ctrl=1、Alt=2、Shift=4、未知ビットは除去。変換はファンクションキー番号を制限しません。`layout` は64表現と100,000ページ・タイトルに切り詰め、長すぎる表現 `id` は捨てます。利用者向け翻訳文は512文字までに切り詰め、改行・タブ以外の制御文字と U+10EEEE を除去し、英語以外を最大16言語保持します。`Failure.detail` も切り詰めます。

<a id="minimal-loop"></a>

## 実装者向けの最小ループ

テキスト専用プラグインの通信上の責務は次の順です。

1. 何も送らず `Hello` を読み、提示された版を選び manifest 付き `Ack` を返します。一致する版がなければ `Refused` を返します。下記 manifest の nonnullable フィールドを揃え、optional は `nil`、使わない宣言配列は空にできます。ファイルビューアは file bind と `shows:["Text"]` を宣言します。設定・操作・独自画面・起動 verb は必須ではありません。通信例の manifest が完全な最小例です。
2. 受信順に直列処理し、`sid` ごとにセッションを保持します。`Open` は入力を準備し、`layout`/`enabled` を持つ `Opened`、または `Err` を返します。`View` は `Text` `view` と現在の状態を持つ `Viewed`、または `Err` を返します。サイズ非依存・表現1つ・ページなしなら `repr`・`page`・`cols`・`rows`・`cell_px` によって出力を変える必要はありません。ただし wire 型は宣言どおりにします。
3. 操作・入力を実装しなくても `Act`・`Key` に応答します。`outcome:"Unhandled"` と現在の state を持つ `Acted` で十分です。call を黙って捨ててはいけません。未 `open` の `sid` の call は失敗にし、SDK は `Err(Corrupt)` を使います。
4. `Cancel` 自体に応答はせず、対象の call を中断猶予内に終了させます。`Err(Cancelled)` でも、既に完了した `view` の成功でも構いません。未知の cancel `id` は無視します。`Close` は応答なしでセッションを破棄し、未知の close `id` は無視できます。call が長く止まり得るなら、その間も中断を受信できるようにします。処理が直列だからといって、パイプを無期限に読まなくてよいわけではありません。
5. `Shutdown` には応答せず、セッションを解放し、終了猶予内に終了コード0で終了します。フレーム間の正常な EOF は新しい処理を止めて資源を解放します。SDK は通常この場合も0で終了し、`Hello` 前の EOF も同様です。フレーム途中の切断・不正・過大フレームは正常 EOF ではなくエラーで、serving を止めます（SDK CLI は1）。検査ツールは不正フレームも実際に送ります。

参照 SDK は reader/worker 分離・cancel flag・panic guard・fd 手順を提供します。他言語では同等の通信動作を自分で実装します。特に SDK はホスト EOF で直ちに中断し、処理が止まったままなら2秒後に終了コード1で強制終了します。これは正常 EOF の通常の成功終了とは別です。ホストは子の予期しない EOF を終了コードによらず接続失敗とし、検査ツールは `Shutdown` に終了コード0を要求します。操作を持つプラグインは manifest に操作宣言を、各状態スナップショットに現在の使用可否を入れます。ホストはそこから操作ラベルとヘルプを表示します。

## 入力・コンテキスト・manifest

```text
Input = File {path:WPath, hint:S?}
      | Content {content_type:S, text:S, resources:[(S,Bin)], origin:Origin}
      | Embed {kind:S, source:S, display:bool}
Origin = File(WPath) | Converted {from:WPath}
Cx = {settings:[(S,Value)], lang:S, pixels:bool, colors:u8, external:bool, read_dirs:[WPath]?}
Value = Bool(bool) | Int(i64) | Float(f64) | Str(S) | List([Value]) | Table([(S,Value)])
Tr = {en:S, others:[(S,S)]}
```

`File` は読むファイルで `hint` は任意の content-type ヒント。`Content` は変換済みテキストと名前付きバイトリソース・元の場所、`Embed` は埋め込みソース（`display` はブロック表示の区別）です。SDK の lazy resource は送信せず省略します。`settings` はプラグイン設定。`Value` の変換では深さ32超を拒否します（根は0）。`pixels` はピクセル表示可否。`colors` は0=ANSI16、1=ANSI256、2=truecolor、未知値は ANSI16。`external` は外部 fallback への協調的な許可です。`read_dirs=nil` は無制限、`[]` は許可ディレクトリなし、その他は絶対パスのルート一覧。ホストがプラグインのために読める範囲を表し、自身で読むプラグインも従うべき値です。`Tr.en` は英語、`others` は `[ ["ja", "幅"] ]` のような言語・文言の組で、訳がなければ英語に fallback します。

同じ manifest map を `Ack` と SDK の JSON `--manifest` が使います（JSON は任意の path/bin を表せませんが、この manifest の schema はそれらを含みません）。

```text
Manifest = {
  id:S, version:S, api:(u16,u16), name:Tr,
  binds:[Bind], actions:[Act], verbs:[Verb], settings:[Setting], screens:[S], shows:[ViewKind],
  work_group:S?, converts:bool, file_text:bool, sized:bool, unavailable:Unavailable
}
Bind = {targets:[Target], option:bool}
Target = Glob(S) | FileName(S) | Mime(S) | Content(S) | Embed(S)
Act = {id:S, key:S?, label:Tr, help:Tr, footer:u8?, screen:S?, writes:bool, destructive:bool}
Verb = {open:bool, chain:[Launcher], gui:bool, line_token:bool, label:Tr?}
Launcher = Editor | MacApp {bundle_id:S} | Command([S]) | OsDefault
Setting = {name:S, default:Value, help:Tr}
ViewKind = Text | Document | Table | Image | Diff | List | Canvas
Unavailable = Message | RawText {with_reason:bool} | Notice(Tr) | Picture
```

`id` は識別子、`version` はリリース版、`api` は SDK API の契約、`name` は表示名です。`Bind` はファイル名 glob・正確なファイル名・MIME・変換済み content・embed kind を選びます。`option=true` は既定でなく明示選択の候補。`shows` は主表示の型で、ファイル binding にこれがないと画面が付きません。`screens` は独自キーコンテキスト。操作の `key` は `KeyCode` でなくキー表記（`"w"`、`"ctrl-g"`、`"F2"` など）。`footer` は小さい順の任意優先度、`screen` は有効コンテキスト。`writes` は read-only 制御用の書き込み宣言です。`destructive` はデータ破壊の宣言ですが、現在の API にはその確認宣言がまだないため、このフラグから確認フローがあると解釈しないでください。`label` はフッター・メニュー用の短文、`help` は説明です。`Verb` は起動 fallback の順序で、`open=true` が `Open`、`false` が Edit、`gui` は GUI/端末、`line_token` はエディタの行番号トークン。`Setting` は既定値とヘルプ。`work_group` は排他的な処理グループ、`converts` は変換済み内容、`file_text` は元ファイルのテキスト、`sized` は領域サイズ依存の `view`、`unavailable` は表示不能時のホスト表示を指定します。konoma はプラグインの登録時に、各フィールドの読み取りに加えて `id`・glob・操作キーを検証します。下記の検査ツールでそれらも検査できます。

登録時の `id`・操作 `id`・設定名・`screen`（操作の context も含む）・work group は `[a-z0-9._-]` の ASCII 1〜64文字です。最大256操作・256設定・1,024 bind・64 `screen`・bind ごと512 `target`。`target` は最大1,024バイトで制御文字・U+10EEEE なし、glob はコンパイル可能な必要があります。設定の既定値は最大1,024 value node、リストや表の入れ子8段（最外の container が1段目）、表のキーは最大256バイトで制御文字・U+10EEEE なしです。これは wire の深さ32より厳しい登録時の検査です。`version` は64文字に切り詰めますが、ここでは semver 検証しません。後から重複した操作 `id` と `destructive` 操作は key-validation issue を伴って除去します。不正・衝突・予約済みの操作キーは、操作をメニューに残してキーだけ除去することがあります。`check_manifest` と通信検査ツールはそれらを問題なしとして扱わず、報告します。

## 表示データ

```text
View = Text(Text) | Document(Document) | Table(Table) | Image(ImageView)
     | List([ListRow]) | Diff([[DiffLine]]) | Canvas(Canvas)
     | Reopen(Reopen) | Message([Line])
Style = {fg:Color?, bg:Color?, add:u16, sub:u16}
Color = Reset | Black | Red | Green | Yellow | Blue | Magenta | Cyan | Gray | DarkGray
      | LightRed | LightGreen | LightYellow | LightBlue | LightMagenta | LightCyan | White
      | Indexed(u8) | Rgb(u8,u8,u8)
Line = {spans:[(S,Style)], style:Style}
Text = {source:TextSource, file_lines:bool, syntax_name:S?, note:S?}
TextSource = File(WPath) | Owned(S) | Lines([Line])
Document = {markdown:S, origin:Origin, resources:[(S,Bin)], options:DocOptions, title:S?, truncated:bool}
DocOptions = {front_matter:bool, footnotes:bool, inline_html:bool, autolink:bool, emoji:bool,
  alerts:bool, details:S, task_states:[character], tasks_writable:bool, table_align:S, image_align:S}
Table = {title:S?, rainbow:bool, addressed:bool, ncols:u64, header:[S?], truncated:bool, rows:[[S]]}
ImageView = {data:Image, sharp_zoom:bool, raster_max_px:u32?}
Image = One(Leaf) | Frames([(Leaf,u64)])
Leaf = File(WPath) | Encoded(Bin) | Svg {data:Bin, base_dir:WPath?, trusted:bool}
     | Rgba {width:u32, height:u32, pixels:Bin}
ListRow = {cells:[Line], kind:S, target:WPath?, checked:bool?}
DiffLine = {kind:DiffKind, old_no:u64?, new_no:u64?, text:S}
DiffKind = Context | Added | Removed
Reopen = {content_type:S, text:S, resources:[(S,Bin)], origin:Origin, truncated:bool}
Canvas = {cols:u16, rows:u16, styles:Bin, runs:[[(u32,S)]], images:[CanvasImage],
  redraw_after_ms:u32?, text_input:bool}
CanvasImage = {id:u64, x:u16, y:u16, cols:u16, rows:u16, image:Image?}
```

`Text` はパス・所有テキスト・装飾済み行のいずれかです。`file_lines` は元ファイルと行番号が一致すること、`syntax_name` は任意のホスト highlighter 名。`note` は SDK の装飾 fallback 診断で子の stderr に記録し、ホストには表示しません。styler callback は渡らず、子が8 MiB までのテキストを装飾します。装飾の符号化が40 MiB を超えると平文・ファイルに fallback します。`Table` は文字列セル・`header`・任意タイトル・rainbow/address 表示・切り詰め印を送り、セル `detail` callback は渡りません。短い行は可、`ncols` より広い行・`header` は不可です。最大65,536列、`max(行長,1)` の合計は最大4,000,000です。

`Document` は Markdown・resource・表示スイッチです。`details` と alignment の文字列は変換時には enum 検証せずそのまま渡します。`Reopen` は変換済み content を別プラグインへ委譲し、Message はホストが描く説明行です。`List` は装飾セル・kind・任意の絶対ジャンプ先・任意 checkbox 状態を持ちます。Diff は hunk の配列で、各 hunk は context/added/removed と任意の新旧行番号を持つ行の配列です。

`Image` の `Encoded` は画像形式のバイト、`Rgba` は隙間のない RGBA8 で、幅・高さは非0、バイト数は厳密に `width*height*4`。アニメーション遅延は**マイクロ秒**、最大5,000 leaf、入れ子は不可です。lazy image は渡せません。SVG の `trusted` は送信値によらず受信時に `false` にします。ファイル・基準ディレクトリ・元の場所・ジャンプ先のパスは絶対パスが必要です。canvas 外の wire RGBA 変換は非0とバイト数を検査し、canvas の4,096ピクセル上限は検査しません。後段のホスト描画にも独自の制限があります。

`Canvas` は1辺最大4,096セル、全体最大4,194,304セルです。`runs` は厳密に `rows` 個の配列で、run は `[0起点のstyle番号, セル記号の連結文字列]`。Unicode grapheme cluster に分割してセルを復元し、幅広文字の継続セルを U+0000 で表します。各行は `cols` 個ちょうど必要です。通常の記号は1 grapheme、UTF-8 で最大32バイト、幅1か2、制御文字と U+10EEEE を含めず、幅2なら同じ行の直後に継続セルが必要。幅は Unicode の表示幅に U+FF9E・U+FF9F 1文字につき1列を追加した値です。継続セルは必ず幅広文字の直後に置きます。隣接記号が1 grapheme に融合する場合は run を分けます。SDK は復元後の canvas を検証します。

`styles` は12バイトの record：前景4、背景4、追加 modifier の little-endian 2、除去 modifier の little-endian 2です。色は `[tag,a,b,c]` で、0=なし、1..17=上記 `Color` の名前付き variant の順（Reset〜White）、18=indexed（`a` が番号）、19=RGB（`a,b,c`）。未使用バイトは0を送りますが受信時に検査しません。未知 tag は拒否。record は端数なし、最大 `max(cols*rows,1)` 個です。modifier の bit 0..8 は bold、dim、italic、underlined、slow blink、rapid blink、reversed、hidden、crossed out。未知 bit は除去します（通常の `Style` も同様）。

canvas 画像は最大4,096配置・256種類の `id`。配置は非空で canvas 内に収まり、同じ `id` は1つのセルサイズを使います。`image=nil` はその `id` の前の画像を再利用します。canvas RGBA はアニメーション leaf を含め1辺最大4,096ピクセルです。`redraw_after_ms` は任意のミリ秒（SDK は切り上げ）。wire 変換は `u32` の値を clamp せず受け付け、canvas 表示の契約では全体が見える間だけ33 ms〜24時間で再描画します。`text_input` は生キー入力要求で、<kbd>Esc</kbd> はホストが保持して入力を終了します。通常の `view` 型が使えれば、ホストのスクロール・検索・選択・コピーを利用できます。

## 大きなバイト列

```text
Bin = Inline(B) | File {name:S, len:u64}
```

ホストが新しい受け渡しディレクトリ（0700）を作り、`Hello.side_dir` で伝えます。SDK は1 MiB 以下を inline、それより大きいバイト列を side file にします。これは送信側の閾値で、受信側が大きな inline を禁じるわけではありません（フレーム上限は適用）。0600 の通常ファイルを排他的に新規作成し、書き終えてからメッセージを送ります。SDK の名前はホストが `h-0`、`h-1`、…、子が `c-0`、`c-1`、…で接続内で一意です。受信側は prefix を要求せず、1〜64バイトの単一名、`.`・`..` 以外、`/`・NUL なしを要求します。`len` は実ファイル長と厳密一致。link と通常ファイル以外を拒否し、link を辿らず読み、短い読み込みも含め読み取り後に削除します。1ファイル最大256 MiB、ホストの1メッセージの side-file 合計最大512 MiB、子の1応答は256 MiB。inline はこの合計に数えません。resource はデコード時に最大100,000項目。変換失敗の未読ファイルは破棄し、接続終了時にディレクトリも削除します。これらは resource・画像・canvas style のバイト列用で、大きな text/table 文字列が自動的に side file になるわけではありません。なおフレーム上限を超える応答は参照 server が `Err(TooLarge)` にします。

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

`ProjectScreen::filter(false)` で一覧の絞り込みとフッター・ヘルプの案内を無効にします。
線上の `filter?: boolean` は既定が true で、true の時は省略します。
`ProjectScreen::transient()` は、ホストのタブから離れた時に重ねた画面を閉じます。
`transient?: boolean` は既定が false で、false の時は省略します。下の画面と
カーソルは残ります。モーダルの仮状態は、開き直す操作で初期化してください。

`Outcome::PopScreen`（線上は `"PopScreen"`、protocol 2）は、重ねた画面を閉じ、
下の画面のカーソルを保ちます。通常のタブ画面では何もしません。変更した下の画面の
データには取り直し先を宣言します。単体アプリではプロジェクトの画面遷移を未対応と
通知します。既存の結果のバイト列は変わりません。

画面の操作キーは、共通の画面ジャンプキーより優先します。重ねた画面では、Tab の
巡回と同様に共通の画面ジャンプキーも何もしません。プロジェクト画面には
表現やページの巡回がないため、`Enter`（空の一覧も含む）と `J`/`K` を操作に
宣言できます。ほかのホストキーは予約のままです。

`ProjectScreen.marks(bool)` と `.checks(bool)` で印（複数選択）とチェックの列を
固定します。既定は false で、線上では省略します。行の内容では幅を変えず、
チェック状態のない行もチェック列の空白4セルを取ります。行の `kind` は操作用の
メタデータで、文字として描きません。`ListRow::selectable(false)` は文言や見出しを
表し、反転・印・操作の対象にしません。空の一覧には、`Tr` で翻訳してスタイルを
付けた非選択行を返します。既定は true で、既存の一覧のバイト列を維持します。

`konoma-vcs` のタブ順は changes・log・graph・branches・worktrees（Git のみ）です。branches は Git で「ブランチ」、jj は「ブックマーク」と表示します（`b`）。Git は切替・作成・安全削除・強制削除を宣言し、jj は名前コピーだけを宣言します。切替と作成の後もこのタブに留まります。旧ハブ(今は削除済み)は変更画面へ戻りました。フッターはホスト共通の移動・画面切替・戻る・絞り込みを先に表示し、その後にプラグイン操作を置きます。コピーのメニュー名は共通のものを使います。空または一致なしの一覧は薄い案内行を表示し、空でもタブを開けます。jj では実行できない書き込みキー（`Enter`・`c`・`d`）を表示しません。旧ハブはキーを表示した上で書き込みを拒否していました。すべてのプロジェクト確認ダイアログは、従来の `key: label` に代えて `key = label` で表示し、破壊的と宣言した選択肢（ブランチの `!` 強制削除を含む）を赤く、破壊的な操作のダイアログの枠も赤く表示します。

`ProjectView` は省略可能な絶対パス `tree_root`・`open_dir`・`home_dir` も持ちます。
現在のツリーの root は `Input::Project` の根と違う場合もあり、`open_dir` はタブの
起動ディレクトリです。ホストは毎回の要求に渡し、パスの文脈が変われば表示を
取り直します。古い相手は省略できます。表示の好みはプラグイン設定として読み、
git と jj は `[plugins.git]` / `[plugins.jj]` の
`path_style = "relative" | "home" | "full"` を宣言します。
指定がない場合、ホストの既存のパス表示設定を使います。

### タブの文脈と一覧の凡例

プロジェクトの要求は `ProjectView::tab(u64)` と `ProjectView::icons(bool)` も持ちます。
`ProjectAct::tab(u64)` は操作元のタブを示します。セッションはプロジェクトごとに共有されるため、
タブ固有の画面状態にはこの不透明な識別子を使います。表示キャッシュもタブごとに分かれます。
wire で省略された時は `tab = 0`、`icons = true` です。既定値は送信時に省略します。
icons はホストの `ui.icons` に従います。

アクティブ／非アクティブのタブを閉じると、ホストはそのタブの表示・文脈・再取得待ちの
キャッシュを削除し、遅れて返った表示も破棄します。先行ジョブの終了後、所有ワーカーで
`Session::tab_closed(tab: u64)` を呼びます。既定の実装は何もしません。
状態を持つプラグインはそのタブの状態だけを解放し、共有セッションと他のタブは残します。
プロトコル2のプロジェクト用サーバーは `session_tab_closed: true` を通知します。
未指定は false で、false は送信時に省略します。対応する子だけに、返答なしの
`{"TabClosed":{"sid":N,"tab":N}}` を送ります。古い子には未知の通知を送らず、
従来の `Close { sid }` はセッション全体を閉じる意味のままです。

`ListView.title: Option<Line>` は画面タブの横にスタイル付きタイトルを加えます。
`ListView.bottom: Option<ListStrip>` はスクロールする一覧の下に固定の区切りと凡例を加えます。
`ListStrip::new(chips: Vec<Line>, estimated_width: usize)` のチップ間は空白2個で、
折り返しの空白は除きます。幅の見積もりが本文幅に収まれば1行、そうでなければ2行を確保し、
本文の高さが `3 + chip_lines` 以上の時だけ表示します。両フィールドの既定は `None` です。
装飾付き一覧はプロトコル2の
`{"ListDecorated":{"rows":[…],"title":Line|null,"bottom":{"chips":[Line,…],"estimated_width":N}|null}}`
を使います。装飾なしの `{"List":[…]}` は従来のバイト列を保ち、旧 JSON では装飾なしになります。
wire の送受信は、`LIST_STRIP_MAX_CHIPS`（1024個）または `LIST_STRIP_MAX_WIDTH`
（見積もり幅1,048,576セル）を超える凡例を、行へ変換する前に `WireError::TooLarge`
で拒みます。上限ちょうどの値は受け付けます。

グラフの設定は `[plugins.git]` / `[plugins.jj]` の `graph_max_branches = 12`（`0` は無制限）、`graph_base_branches = []`、`commit_meta_align = "right"`（`"inline"` は件名の後ろに配置）です。旧 `[ui]` 名も読まれ、プラグインのテーブルが勝ちます。

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
  screens: [{ id: string, title: Tr, key: character | null, row_kinds: string[], marks?: boolean, checks?: boolean, pushed?: boolean }],
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

## ヒントのグループ

`Manifest::hint_group(HintGroup::new(id, label).help(description))` で、複数の操作を
フッターとヘルプの1つのヒントにまとめます。`Act::hint_group(id)` で所属を指定します。
`label` と `description` は `Tr` で、ヘルプ文が空なら選択言語のラベルを使います。
送信する任意フィールドは次のとおりです。

```text
Manifest.hint_groups: [{ id: string, label: Tr, help: Tr }]
Act.hint_group: string | null
```

古い子プロセスでフィールドがなければ、グループは空、所属はなしになります。
空の値は送信時に省略し、JSONとMessagePackで同じ宣言を渡します。
グループidはプラグイン内で一意です。未宣言の所属先、グループidの重複、
メンバー間の `screen`/`context` の違い、プロジェクト画面の違いはそれぞれ別の
登録エラーとしてグループと該当操作を示します（空の重複グループには操作名がありません）。
同じプロジェクト画面で行の種類が異なる場合と、メンバーのないグループは許可します。

フッターでは最小の優先順位を持つメンバーの位置に1つのトークンを置き、
同順位では従来どおり操作の宣言順を使います。フッターの優先順位のないメンバーは
寄与しません。有効なメンバーのキーだけを操作の宣言順で表示し、ユーザーの
割り当て変更とホストの通常の表記を使います（`s/S:stage(all)`）。
キーのある有効なメンバーがいなければトークンは表示しません。
ヘルプも従来の順序で最初のメンバーの位置に1行を置きます。フッターの優先順位の
ないメンバーも含め、有効なメンバーのキーをホストの複数操作行と同じ表記
（`s / S`）で書き、グループのヘルプ文を使います。キーのある有効なメンバーが
いなければ行を表示しません。これは旧hubの有効メンバーの規則と同じです。
`Space` メニューは各操作の項目とラベルを維持します。

## 操作とダイアログ（版2）

既存の `id`、`key`、`label`、`help`、`footer`、`screen`、`writes`、`destructive` の
意味は同じです。次の任意フィールドを追加します。

```text
project_context:
  { "Screen": { screen: string, row_kinds: string[] } }
  | "TreeSelection" | "Diff" | null
confirmation: { text: Tr, choices: [{ key: character, choice: Choice, status?: Tr, destructive?: boolean }], setting?: string } | null
text_input: { label: Tr, initial: string, keep_on_failure?: boolean } | null
choice_input: { label: Tr, choices: Choice[] } | null
refresh: RefreshTarget[]
Choice = { value: string, label: Tr }
```

空の `row_kinds` はその画面全体、`TreeSelection` はツリー選択のメニュー操作です。
`Enter` は `key: "Enter"` と、行の種類を1つ以上指定した画面の文脈で宣言します。
通常の操作と同じ宣言・検査の経路を通り、行の種類が異なれば別の Enter 操作を持てます。
確認の選択肢は `y` → `-d`、`!` → `-D` のように値を分けられます。

確認がどの入力段階にあっても、`n`・`N`・`Esc` はダイアログ全体を閉じ、操作を
実行せず、ホストの現在の言語で取消メッセージ（`canceled` / `取消`）を表示します。
これらのキーは選択肢として宣言できません。宣言したキーとの完全一致を優先し、
一致がない場合だけ ASCII 大文字を小文字に対応する選択肢として扱います
（例：`Y` で `y` を選択）。`Enter` と未宣言のほかのキーは何もしません。
文字入力・一覧入力・プロジェクト選択の `Esc` はメッセージを表示せずに閉じます。

ホストは言語を選んだ後、確認文のすべての `{row}` を、操作を呼び出した行の
`name`（未指定なら `value`）に置き換えます。選択行がなければ空文字です。
呼び出し時の行を保持するため、ダイアログを開いている間にカーソルが動いたり
一覧が更新されたりしても、確認文の行名は変わりません。他の置換記法や
エスケープはなく、`{row}` のない文はそのまま表示します。

`ConfirmChoice::status(Tr)` はステータス行の短いラベルを指定します。省略時は
選択肢の通常のラベルを使います。ホストは宣言されたキーとラベルを並べ、最後に
翻訳済みの取消キーを表示します。`!` のような文字以外のキーも同じ扱いです。
線上の `status` は省略可能で、古い JSON では未指定として扱います。
ホストはカタログの取消案内の幅を先に確保し、CJK を含む表示セル幅で選択肢を
トークン単位に収めます。取消案内全体が収まらない場合も、3セル以上あれば `Esc` を
表示します。不正なプロンプト段階は描画せず、入力時に操作を呼ばず取り消します。

`ConfirmChoice::destructive()` はキー文字に関係なく選択肢を警告の赤色にします。
線上の `destructive` は省略可能で既定は false、false の場合は省略して古い JSON を
維持します。この表示用の印は `Act::destructive()` や書き込みガードの代わりには
なりません。破壊的なダイアログの赤い枠と確認必須の宣言は引き続き操作側で指定します。

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
`"TreeStatus"`・`"Chip"` のいずれかです。操作の `refresh` は既定で空、空なら送信時に省略します。
実行後は結果を通知し、宣言した対象を取り直します。指定する画面名は
同じプラグインのプロジェクト宣言に存在する必要があり、再取得対象は操作ごとに最大64件です。

登録時には、空でない確認の選択肢を持たない破壊的操作、画面 id の重複（従来の
画面名との衝突を含む）、画面キーの重複、新しい型付き文脈の未宣言の画面・行の種類、
検出目印のないプロジェクトを拒否します。従来の `screen` キー文脈は従来どおりです。
SDK の型付き理由をホストが英語・日本語に翻訳します。名前と宣言数の上限は、
プロジェクトの画面・行の名前、目印と行の種類の数にも適用します。

監視パスは空でない相対パスで、親・ルート・接頭辞の要素を含めず、最大512件です。
検出目印は `.`・`..` 以外の空でない単一のファイル名要素で、最大1024バイトです。
画面キーには既存の一覧用予約表を使います。確認キーにはそのGlobal層と `q`・`n`・`N`・Escを
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
`ListRow::name(impl Into<String>)` は確認用の短い名前を任意で指定します
（`Option<String>`、既定は `None`）。未指定の線上フィールドは省略し、古い子プロセス
では `None`、古いホストでは無視されます。選択行と印付き行の `ProjectRow` にも渡り、
`ProjectRow::new(kind, value).name(name)` で直接設定できます。
`ListRow::right(Line)` は、本文の右端へ寄せるスタイル付きの1行の連
`right: Line | null`（SDK では `Option<Line>`、既定は `None`）を加えます。
右の表示幅に2を足すと本文幅を超える場合は右を隠し、左のセルを `…` で
切り詰めます。収まる場合は空白1つ分を残して左を切り詰め、間を空白で埋め、
右のスタイルを保ちます。CJK の表示幅も数え、選択行は全体を反転します。
`ListRow::filter_text(text)` は照合用の `filter_text: string | null`
（SDK では `Option<String>`、既定は `None`）を加えます。ホストは小文字での
部分一致と、再読み込み後の選択行の追従にこの文字を使います。未指定なら従来の
セルと任意の右の連の表示文字を使い、右寄せ用の空白は含めません。明示した空文字は
空の問い合わせだけに一致します。どちらも未指定なら線上から省略され、古い子では
`None`、古いホストでは無視されます。従来の行の符号化は変わりません。
`ListRow::initial_selection(true)` は画面を初めて開く時のカーソル行を指定します。
表示中かつ選択可能な行の最初の指定を使い、再読み込みでは選択した識別子を保ちます。
`ListRow::filter_fallback(true)` は行を選択不能にし、通常の行が1つも一致しない時だけ
宣言順にすべて表示し、案内文は検索しません。空の検索にも適用され、
`ListView::visible_rows(query)` がこの規則を実装します。子から受信した fallback 行は、
`selectable: true` が同時に指定されても非選択に正規化します。
通信上の両フィールドは真偽値で、既定の `false` は省略されます。
旧 JSON と MessagePack の行を維持し、fallback 行を選択や操作の対象にはしません。
従来の行の種類・行き先・プラグインのチェック状態の意味を保ち、`marked` は独立した
ホストの複数選択の印です。不透明な行の値 `value: string` も持ち、SDKの `ListRow::value` で指定します。
既定の空文字は送信時に省略し、従来の行のバイト表現を保ちます。セルは既に
`Line { spans: [[text, Style], …], style: Style }` で、グラフの罫線もこの span で表せます。
通信上の各 span は2要素の配列です。`Style` は `fg`、`bg`、`add`、`sub` を持ち、
後ろの2つは文字修飾のビットマスクです。
`ProjectScreen::pushed()` は `pushed: true` を設定します（未指定は `false`）。
この画面は `PushScreen` だけで開き、タブの表示と巡回から除外します。
表示中の `Tab` / `Shift-Tab` は何もせず、ジャンプ用の `key` を宣言すると
画面名付きの検証エラーになります。`q` / `Esc` は下の画面のカーソルを保って戻ります。
リストの Right 移動（既定は `l` / 右矢印、テキスト層のキー変更を反映）は、
カーソル行の `Enter` アクションを同じガード経由で実行します。
プラグインは `Enter` だけを宣言し、その行で実行できなければ開くヒントも表示しません。

`DiffView::new(hunks).header(lines)` は差分の上に装飾付きの `header: Line[]` を表示し、
空でなければ直後に薄い全幅の `─` 区切りを描きます。
`DiffHunk::new(lines).path(path)` はファイルを指定し、前の hunk とパスが異なる時に
ファイル見出しを描き、その拡張子で構文を色付けします。同じパスの連続する hunk は
見出しを共有します。既定は空のヘッダーと `None` のパスです。

差分の通信データは `{ header?: Line[], hunks: [{ path?: string | null,
lines: DiffLine[] }] }` です。従来の `DiffLine[][]` も読み取れ、行の符号化は変わりません。
ホスト共通の縦／横／Auto 描画を使い、可視範囲だけを構文色付けします。
内容・幅・並び・縦の表示範囲が同じ間は描画結果を再利用し、左右分割では横位置も含めます。
上下配置の横スクロールは色付け済みの行を再利用し、ヘッダーも差分と一緒に動かします。
総行数と横位置の上限は、構文色付けせずに計算します。
空の差分では、ヘッダーがあれば区切りの後に薄い変更なしの行、なければ中央に変更なしを表示します。

テキストと差分画面は先頭行をスクロールし、最終ページを埋めて止めます。
テキスト層のキー（less 方式と `[keys.text]` の変更を含む）で、1行、半ページ（10行）、
ページ（20行）、先頭／末尾、横4桁、行頭／行末へ移動します。リストのカーソル移動は従来どおりです。
差分画面の `s` は共通の並びを巡回して横位置を0に戻します。テキスト画面にはありません。
`cycle_diff_layout` のキーは差分ビューのキー文脈で解決します。
`[keys.diff]` の変更はファイル差分プレビューとすべてのプロジェクト差分画面に適用され、
フッターとヘルプにも解決後のキーを表示します。Git機能なしのプロジェクト差分でも使えます。
`r` は表示中のプロジェクト画面を読み直します。

`Act::copy()` は、その文脈のホストの `y` メニュー項目を宣言します。
wire の `copy: boolean` は省略時に `false`、出力時も `false` なら省略します。
コピー項目はプロジェクト画面に属し、`project_context` に `Screen` が必要です。
ビュー文脈だけの宣言、ツリー選択、汎用 diff 文脈の宣言は、操作名を含むエラーで拒否します。
`key` は制御文字・空白以外の1文字です。不正なキー、同じ文脈で重複するコピーキーは、
操作名を含むエラーでマニフェスト全体を拒否します。
メニューを閉じる `q` と `Esc` は予約され、コピー項目には使えません。
コピーキーには直接キーの予約・衝突検査を適用しません。
コピー項目は直接キー、個別のフッターヒント、`Space` 項目を持ちません。
フッター優先度を指定しても同じです。通常の操作の動作と wire データは変わりません。

プロジェクト画面では、現在実行できる項目を宣言順、宣言キー、表示言語のラベルで表示します。
タイトルはホストの汎用コピー表記を使います。ヘルプに `y` の項目を列挙し、メニューが開く時だけ
フッターに `y` を表示します。項目がなければ `y` は何もしません。重ねたリスト以外の画面では、
コピー操作文脈の選択行は `selection` 種別で、重ねる時に渡された選択識別子を値として持ちます。

従来の `Unhandled`、`Redraw`、`Flash(Tr)`、`Reload` に、次の結果を追加します。

| 結果 | データ | ホストの動作 |
|---|---|---|
| `Copy` | `{ text: string }` | 空文字を含む本文をそのままコピーし、共通の成功プレビューまたは失敗通知を表示する |
| `PushScreen` | `{ screen, selection: string \| null }` | 不透明な行識別子を渡し、同じプラグインの画面を重ねる |
| `OpenFile` | `{ path: Path, line: integer \| null }` | プレビューを開く。行番号は1始まり |
| `OpenDiff` | `{ path: Path }` | ホストの全画面 diff を開く |
| `SwitchRoot` | `{ root: Path, message?: Tr }` | ルートと起点ディレクトリを切り替え、任意の成功メッセージを表示する |
| `OpenTab` | `{ path: Path }` | ホストの新しいタブで開く |

遷移先は絶対パス、NUL バイトなし、従来のパス長上限内とします。
**`q` で画面を1つ戻すのはホストの動作**です。新しい結果やガードを迂回する操作には
しません。ワーカーが結果を返してから、ホストが遷移を適用します。

`SwitchRoot.message` は未指定なら `None` で、通信から省略されます。
旧形式と既定の切替メッセージを保ち、指定したメッセージは切替成功後だけ表示します。
project リストの `Ctrl-t` は選択行の `target` を使い、ディレクトリなら新しいタブの
ルートと起点ディレクトリ、ファイルならプレビューになります。元のタブと画面は保ちます。
行に `target` がある時だけフッターとヘルプに表示し、project 用フッタープランは
ホストの開くヒントと宣言アクションをまとめ、各キーを一度だけ表示します。

### 同梱 VCS のワークツリー画面

`ProjectScreen::filter_label(Tr)` はフッターの検索ラベルだけを指定できます。
通信上の `filter_label?: Tr` は既定の `None` を省略し、既存画面の共通ラベルと旧形式を保ちます。
共有キーの順序は変えず、その後に行アクションを宣言順で表示します。

git プラグインは `worktrees`（`w`）を、branches があればその後、なければ log の後に
宣言します。git の順序で現在行、bare、detached、locked、prunable を表示します。
パスはホーム相対表示で、`/` はブランチ名または完全パスの大文字小文字を無視した部分一致です。
`Enter`（`l`・右矢印も同じ）は切替、`Ctrl-t` は別タブ、`c` は新規または既存ブランチからの作成、
`d` はホスト描画の差分を重ねます。作成名の前後の空白を除き、配置名の `/` を `-` に置換します。
メインワークツリーを基準に配置し、成功時は正規化したパスへ切り替え、「ワークツリー作成」を表示します。
空の名前と git の失敗では入力を開き直しません。書き込みは作成だけで、read-only では拒否します。

| `[plugins.git]` の設定 | 既定 | 意味 |
|---|---|---|
| `worktree_dir` | `"../"` | メインワークツリーからの配置先 |
| `graph_base_branches` | `[]` | 順序付きの base 候補。最後にメインワークツリーのブランチを追加 |

差分では自身のブランチを除き、最新の merge-base を選びます。同時刻なら最初の候補を保ちます。base 以後のコミット済み・未コミットの変更を表示し、なければ未コミットだけに戻ります。両方空なら変更なしの通知だけです。`s` で並びを変更し、テキスト層のキーで移動し、`q` / Esc でカーソルを保って一覧へ戻ります。旧 `[git] worktree_dir` と`[ui] graph_base_branches` の名前も読まれ、プラグインのテーブルが勝ちます。

旧組み込みハブは削除されました。旧ハブとの差は、project のタイトル・タブ・枠と移動ヒント、空の一覧を開けない代わりに空行を表示すること、現在行で Enter を押しても一覧に残ること、現在・bare・利用不能行の `Ctrl-t` が通知もヒントもなく何もしないことです。jj は旧画面の拒否通知に代わり、この画面も `w` も宣言しません。フッターでは `/` を先に置き、その後の行操作は宣言順とし、ワークツリーのラベルと説明は旧文言を使います。差分タイトルは旧画面の外枠に代わって差分ヘッダーと区切りに表示します。

検査する例は `crates/konoma-plugin-test/tests/vectors/project-v2.json`・`project-calls-v2.msgpack.hex` と
`tests/project_vectors.rs`、各新型と各結果の往復テストは SDK にあります。

## 期限と失敗

| 制限 | 製品の既定値 |
|---|---|
| キャッシュ済み manifest がない初回探索の handshake | 5秒 |
| 通常起動・再起動の handshake | 30秒 |
| Open / View | 30秒 |
| Act / Key | 10秒 |
| Cancel 後の応答 | 2秒 |
| Shutdown 後の終了 | 2秒 |
| セッション・呼び出しなしの idle 終了 | 5分（idle reap 実行時） |
| 起動失敗後の再試行抑制 | 10秒 |
| 子の常駐メモリ | 2 GiB、100 ms ごとに確認 |
| 保持する stderr 末尾 | 4,096バイト（失敗 detail はさらに無害化・切り詰め） |

RSS の制限は OS helper が測定値を返せる場合に適用し、測定できなければこの検査は無効で期限のみ有効です。RSS 上限は監視によるもので、OS が強制するメモリ割当上限ではありません。

`FailureKind` の使い分けはプラグイン・SDK の慣例で、要求ごとの必須のエラー種別表はありません。サンプルは `std::fs::read` のエラーを SDK の `From<std::io::Error>` で変換し、`Io`、英日 I/O エラーメッセージ、`detail` のエラー文字列を返します（読めないファイルも含む）。ファイル以外の入力には明示的に `Unsupported` を使います。SDK は未知セッションに `Corrupt`、実行前の中断に `Cancelled` を使います。これは参照実装の慣例で、他言語の同等の失敗すべてに同じエラー種別を強制する通信規則ではありません。`Answer::Err` は列挙済みの任意の `FailureKind` を運べ、読めないファイルや未対応入力へのエラー種別の割り当てを wire は固定していません。独自の通信エラー形式を作らず、プラグインに合うエラー種別と役立つ文言を選んでください。

呼び出し期限は送信時でなく最古の未完了呼び出しになった時から数えます。`Cancel` の猶予は中断要求と先頭到達の両方が成立してからです。これらはネゴシエーション値でなくホストの既定です。検査ツールは通常の `Limits`（handshake 30秒を含む）を使い、CLI に timeout 上書きはありません。

timeout、クラッシュ・EOF、不正・過大フレーム、対応しない応答 `id`、応答種別違い、メモリ超過は子のプロセスグループ全体を終了させます。待機呼び出しは失敗し、他のセッションも使用不能になり、次の `open` は新しい子を起動します（起動失敗は再試行抑制の対象）。失敗には stderr の制限付き診断が付きます。stdout の余計なバイトはヘッダ・本体として解釈するため、フレームエラーや期限切れになります。復旧用の走査はありません。正常な `Err` または変換不正の `view`（壊れた canvas・相対画像パスなど）はその呼び出しだけが失敗します。意図した停止は `Shutdown` を先に送り、終了猶予後に残りのグループを kill します。leader が終了しても子孫を終了させます。SIGKILL 後の reap は500 ms を上限とし、回収できない子は非同期 reaper が所有し続けます。

## 通信例（デコード後の MessagePack）

`0x41` の1バイトが入った `/data/a.bin` を開く最小の hexdump 風プラグインです。各矢印が1フレーム。先に `M`・`C`・`L`・`S` を定義して後で展開します。これは説明上の省略で、wire 上の参照ではありません。`bin("...")` はパスの実バイトです。ここでは全 optional フィールドも記載しています。

```text
M = {id:"hex", version:"0.1.0", api:[0,1], name:{en:"Hex",others:[]},
     binds:[{targets:[{Glob:"*.bin"}],option:false}], actions:[], verbs:[], settings:[],
     screens:[], shows:["Text"], work_group:nil, converts:false, file_text:false,
     sized:false, unavailable:"Message"}
C = {settings:[],lang:"en",pixels:false,colors:0,external:false,read_dirs:nil}
L = {reprs:[],default_repr:nil,default_repr_without_pixels:nil,pages:nil}
S = {fg:nil,bg:nil,add:0,sub:0}

host → {versions:[1],host_version:"0.31.2",side_dir:bin("/private/handoff"),lang:"en"}
child → {Ack:{version:1,manifest:M}}
host → {Open:{sid:1,rid:1,input:{File:{path:bin("/data/a.bin"),hint:nil}},cx:C}}
child → {rid:1,answer:{Ok:{Opened:{state:{layout:L,enabled:[]}}}}}
host → {View:{sid:1,rid:2,req:{repr:nil,page:nil,cols:80,rows:20,cell_px:nil},cx:C}}
child → {rid:2,answer:{Ok:{Viewed:{view:{Text:{source:{Lines:[{spans:[["00000000  41                                               |A|",S]],style:S}]},
          file_lines:false,syntax_name:nil,note:nil}},state:{layout:L,enabled:[]}}}}}
host → {Act:{sid:1,rid:3,action:"unknown",cx:C}}
child → {rid:3,answer:{Ok:{Acted:{outcome:"Unhandled",state:{layout:L,enabled:[]}}}}}
host → {Key:{sid:1,rid:4,key:{code:{Char:"x"},mods:0},cx:C}}
child → {rid:4,answer:{Ok:{Acted:{outcome:"Unhandled",state:{layout:L,enabled:[]}}}}}
host → {Cancel:{rid:2}}
host → {Close:{sid:1}}
host → "Shutdown"
```

`Lines` 形式のテキスト・装飾なしの span 1つ・行の style が実サンプルの `View::text(lines)` と同じ形です。`S` は2箇所それぞれ独立した style map に展開します。16バイト幅の16進数列は47文字に埋めます。この最小 manifest は操作なしなので、`Act` と生 `Key` は `Unhandled` です。ここで `Cancel` は応答済み `view` を指すため無視します。`Cancel`・`Close`・裸の文字列 `Shutdown` に子の応答はなく、`Shutdown` はプロセスを終了します。

例えば `Shutdown` の全フレームは `00 00 00 09 a8 53 68 75 74 64 6f 77 6e`。本体長9、MessagePack fixstr 長8（`a8`）、UTF-8 の `Shutdown` です。後ろに改行はありません。

## 参照テストベクター

4バイト長 prefix を含む、参照エンコーダが出力した完全なフレームです。上の `Hello` と、最小の `M`（サンプルの操作付き manifest ではない）を使う `Ack` を符号化しています。offset は各フレーム先頭からの16進バイト位置です。`Hello` 本体は67バイト（0x43）、`Ack` 本体は218バイト（0xda）。map のフィールド順は参照 encoder の出力順であり、受信側はこの順序に依存せず map キーを使ってください。

`Hello`:
<!-- vector:hello -->
```text
0000  00 00 00 43 84 a8 76 65 72 73 69 6f 6e 73 91 01
0010  ac 68 6f 73 74 5f 76 65 72 73 69 6f 6e a6 30 2e
0020  33 31 2e 32 a8 73 69 64 65 5f 64 69 72 c4 10 2f
0030  70 72 69 76 61 74 65 2f 68 61 6e 64 6f 66 66 a4
0040  6c 61 6e 67 a2 65 6e
```

`Ack`:
<!-- vector:ack -->
```text
0000  00 00 00 da 81 a3 41 63 6b 82 a7 76 65 72 73 69
0010  6f 6e 01 a8 6d 61 6e 69 66 65 73 74 8f a2 69 64
0020  a3 68 65 78 a7 76 65 72 73 69 6f 6e a5 30 2e 31
0030  2e 30 a3 61 70 69 92 00 01 a4 6e 61 6d 65 82 a2
0040  65 6e a3 48 65 78 a6 6f 74 68 65 72 73 90 a5 62
0050  69 6e 64 73 91 82 a7 74 61 72 67 65 74 73 91 81
0060  a4 47 6c 6f 62 a5 2a 2e 62 69 6e a6 6f 70 74 69
0070  6f 6e c2 a7 61 63 74 69 6f 6e 73 90 a5 76 65 72
0080  62 73 90 a8 73 65 74 74 69 6e 67 73 90 a7 73 63
0090  72 65 65 6e 73 90 a5 73 68 6f 77 73 91 a4 54 65
00a0  78 74 aa 77 6f 72 6b 5f 67 72 6f 75 70 c0 a8 63
00b0  6f 6e 76 65 72 74 73 c2 a9 66 69 6c 65 5f 74 65
00c0  78 74 c2 a5 73 69 7a 65 64 c2 ab 75 6e 61 76 61
00d0  69 6c 61 62 6c 65 a7 4d 65 73 73 61 67 65
```

## 実装を検査する

konoma の checkout で、存在する読み取り可能な `.bin` とビルド済みプログラムを指定します。

```sh
cargo run -p konoma-plugin-test -- --file a.bin /absolute/path/to/plugin
cargo run -p konoma-plugin-test -- --plugin hex --file a.bin python3 -- /absolute/path/plugin.py
```

実行時に Cargo を使わない場合は、先に checker をビルドし、その実行ファイルを使います。既定の開発用パスは `target/debug/konoma-plugin-test`、`CARGO_TARGET_DIR` 指定時は `$CARGO_TARGET_DIR/debug/konoma-plugin-test` です（`release` ビルドでは `debug` の代わりに `release`）。

```sh
cargo build -p konoma-plugin-test
"${CARGO_TARGET_DIR:-target}/debug/konoma-plugin-test" --file a.bin /absolute/path/to/plugin
"${CARGO_TARGET_DIR:-target}/debug/konoma-plugin-test" --plugin hex --file a.bin python3 -- ./plugin.py
```

checker は呼び出し元のディレクトリを継承するため、相対スクリプト `./plugin.py` を使えます。製品設定の `command` は絶対パス必須ではなく、`python3` は `PATH`、`./viewer` は konoma 起動ディレクトリから解決します。その後、子は専用ディレクトリで動くため、`args` には `args = ["/absolute/path/plugin.py"]` のように絶対スクリプトパスを使ってください。引数文字列はそのまま渡し、相対というだけでは拒否しません。[設定リファレンス](../configuration/)も参照してください。

ツールは選択時の `--plugin <id>`、続けて `--serve` を付けます。`--manifest` でなくフレームの handshake を検査します。manifest と宣言キーを検証し、版拒否・入力を開く処理・表示を検査します。`hints` 検査は manifest の操作宣言と返された `enabled` 状態から操作ラベル・ヘルプを組み立て、有効な操作の英語ラベル・ヘルプが空でないことを確認します。最初に有効な操作と生キー、未知セッション、中断、終了、不正 MessagePack、過大ヘッダも検査します。独立した probe ごとに新しいプロセスグループと非公開 handoff ディレクトリを使います。`--file` がないファイル依存の検査は成功でなく **SKIP**。操作・キーの `Err` は接続が生きていれば有効です。中断は完了済み `view` も認めます。終了コードは0=失敗なし、1=適合失敗、2=使用方法不正です。probe の成功は通った経路の証拠で、全描画・セキュリティ特性の証明ではありません。Rust からは `konoma_plugin_test::run(&Options, Limits::default())` を呼べます。
