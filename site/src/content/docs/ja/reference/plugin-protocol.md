---
title: プラグインプロトコル
description: Rust SDK を参照実装とする、言語非依存の子プロセスプロトコル第1版。
sidebar:
  order: 3
---

現在の Unix 子プロセス通信の仕様です。Rust の `konoma-plugin` crate が参照実装です。
Rust での作り方は[プラグインを書く](../../guides/write-a-plugin/)を参照してください。
通信プロトコルの版は **1**、別の値であるプラグイン API の版は **[0, 1]** です。

## 起動と終了

konoma は設定されたプログラムを `<command> <args> --serve` として起動し、最後に
`--serve` を追加します。シェルは介しません。複数プラグインを持つプログラムでは `args` に
`--plugin <id>` を入れます。1プロセスで1プラグインを提供します。`konoma --plugin <path>`
はその実行だけでプログラムを読み込み、相対プログラムパスを絶対化します。製品はユーザーの
カレントディレクトリではなく専用の非公開ディレクトリで起動するため、設定の `args` のスクリプト・
ファイルには絶対パスを使ってください。SDK 単体の `CommandLaunch` は指定がなければ `/` を使います。
一方 `konoma-plugin-test` は呼び出し元のディレクトリを継承し、相対スクリプトパスを使えます。

環境変数はホストから継承します。`CommandLaunch` は環境を消去・追加しません。実行ファイルの
探索にはホストの `PATH` を使います。専用の必須環境変数はなく、言語・設定・端末能力はメッセージで
受け取ります。パイプに端末があると仮定しないでください。stdin は受信フレーム、stdout は送信
フレーム、stderr は診断用です。SDK の `serve` は通信 fd を close-on-exec で複製し、通常の
stdout を stderr、通常の stdin を `/dev/null` に向けます。他言語ではログによる通信の破損や、
補助プロセスへの fd 継承でパイプが残ることを自分で防いでください。ホストは専用プロセスグループで
起動し、core dump を無効にします。プラグインはユーザーの権限で動きます。メッセージの読み取り範囲は
協調動作とホストの読み取りに対する規則で、OS のサンドボックスではありません。

SDK は `--manifest`（wire manifest の整形 JSON）、`--version`（選択したプラグインごとに
`<id> <version>` の1行）、`--help` も受け付けます。別プロセス起動時のモードで、通信メッセージでは
ありません。複数プラグインの `--serve` と `--manifest` は `--plugin <id>` が必要です。
`--version` 単独は全件を列挙します。1プラグインなら選択指定を省略できます。ファイル引数で独立した
端末表示を起動できるのは SDK の `app` feature を組み込んだ場合だけです。製品は `--manifest`
を呼ばず、handshake で宣言を取得します。
konoma 本体の子プログラムの読み込み処理と konoma-plugin-test は、非Rustプラグインの --manifest・--version を
要求しません。任意の便利なモードです。この2つの利用者向けには --serve（複数プラグインを提供する
場合は --plugin の選択も）を実装してください。

## 通信パイプを保護する

必要なのは元の stdin/stdout を通信バイトだけに使うことです。ホストは特定の fd 操作を要求・検査
しません。SDK の推奨 Unix 手順は process.rs でなく `proc/server.rs::take_protocol_fds` にあります。

1. 元の fd 1、次に fd 0 を `fcntl(fd, F_DUPFD_CLOEXEC, 3)` で複製し、通信の出力・入力用に保持します。
   最小 fd 3 により0・1・2を上書きしません。
2. `dup2(2, 1)` で通常の stdout を診断用 stderr に向けます。
3. `/dev/null` を `O_RDONLY | O_CLOEXEC` で開き、`dup2(null_fd, 0)` し、余分な null fd が2より大きければ
   閉じます。通常の読み込みは通信を消費せず EOF を読みます。
4. 保存した fd だけでフレームを読み書きします。close-on-exec により exec した補助プログラムが通信
   パイプを保持し続けるのを防ぎます。診断は stderr に書きます。

他言語の同等の fd 操作でも構いません。SDK は複製・stdout 切替のエラーを返しますが、/dev/null の
切替は best-effort です（open 失敗時は fd 0 を維持し、この箇所の dup2 の結果は未検査）。
この fallback を、ライブラリが stdin を消費しない保証だと解釈しないでください。

## フレームと値の表記

各フレームの形式は次のとおりです。

```text
offset  bytes   意味
0       4       本体のバイト長、符号なし big-endian（この4バイトを含まない）
4       length  MessagePack の値1個
```

フレームごとに flush します。本体は最大 **67,108,864 バイト（64 MiB）** で、本体の読み込み前に
検査します。デコードの深さは最大 **128**、値の後の余剰バイトはエラーです。フレーム間の EOF は
正常な通信終了、ヘッダ・本体途中の EOF は切断エラーです。magic prefix、区切り文字、JSON、圧縮、
handshake 前の要求エンベロープはありません。

以下では `S` = UTF-8 の MessagePack string、`B` = MessagePack **bin**、`bool`、
`u8/u16/u32/u64` = その範囲の非負整数、`i64`、`f64` を使います。`T?` は `nil` または `T`、
`[T]` は配列、`(A,B)` は map ではなく2要素配列です。構造体は記載した綴りのキーを持つ map。
参照 encoder と同じく全フィールドを送ってください（nullable は `nil` 可）。現在の serde decoder
は省略された nullable も nil として読み、Text.note には明示的な既定値があります。nonnullable の
欠落は失敗し、未知の構造体フィールドは無視しますが、フィールドの型を変更できる意味ではありません。
map のキー（フィールド名・enum タグ）は文字列で、`[action_id, enabled]` などの tuple は配列です。
例えば manifest の Act.screen は `screen:nil` と screen の省略をどちらも受け付け、主表示の context
（shows で宣言した型）を使います。`screen:"text"` ならその context を選びます。必須の nonnullable
である Act.id は省略・nil のどちらも不可。Text.note は明示的に nil が既定ですが、nonnullable の
省略値すべてに既定値が付くわけではありません。

enum は外部タグ形式です。unit variant の `Shutdown` は文字列 `"Shutdown"`、1値の variant
は `{"Text": value}`、名前付きフィールドは `{"Open": {fields...}}`、tuple variant は
`{"Rgb": [r,g,b]}` です。名前の大文字小文字は区別します。Option に `Some` の包みはありません。
文字型は Unicode scalar 1個の文字列です。整数のバイト幅は固定する必要がありません。
`WPath` は Unix パスのバイト列を入れた **bin** そのもので、UTF-8 string ではありません。
NUL と16,384バイト超を拒否し、読み取り対象のパスは絶対パスが必要です。ホストの入力パスは
絶対化され、子の相対 view パスは変換エラーになります。
SDK の byte visitor はデコード時に u8 配列も受け付けますが、参照符号化として bin を送ってください。

## Handshake

最初はホストが Hello を送り、子はそれを受け取る前に何も送信しません。Hello は次の裸の map で、
`{"Hello": ...}` ではありません。応答は HelloReply の `{"Ack": {version:..., manifest:...}}`
（または Refused）です。Ack にはタグがありますが、どちらも通常の ToChild 呼び出しや
`{rid,answer}` 応答ではありません。

```text
Hello = {versions:[u32], host_version:S, side_dir:WPath, lang:S}
HelloReply = Ack {version:u32, manifest:Manifest}
           | Refused {supported:[u32], reason:S}
```

`versions` は現在 `[1]`。ホストは現在の通信版とその1つ前を受け付けます（1より前の版はありません）。
`host_version` は参考情報のプログラム版で、通信版ではありません。`side_dir` は受け渡し用の絶対
ディレクトリパス、`lang` は最初の言語です。後続呼び出しにも個別の言語が含まれます。

提示された版から選んで `Ack` を返します。対応する版がなければ対応版一覧と理由を付けた `Refused`
を返します。提示されていない版、読めない manifest、設定の id と異なる manifest id はホストが
拒否します。拒否すると起動は失敗し、ホストはその接続を閉じてプロセスグループを終了させ、通常の
呼び出しを送りません。SDK は `Refused` を flush して正常終了します。検査ツールは正常終了または
拒否後も生存しているプロセスを認めますが、クラッシュは拒否します。同じ接続での再 handshake を
要求しません。SDK server は `Hello.host_version` と `Hello.lang` を検証しません。
API の互換検査は別で、major 一致、major 0 では minor も完全一致、それ以外ではプラグインの
minor がホスト以下であることが必要です。この検査は `manifest_from_wire` でなく registry が行います。

SDK は独立したメッセージ識別子で判定せず、最初の値を直接 Hello としてデコードします。例えば最初が
`"Shutdown"`、必須フィールド欠落、型違いなどでデコードできない場合は Refused や call error を返さず、
serving が失敗し、CLI が stderr に診断を書いて終了コード1で終了します。余分なフィールドがあっても
schema が合う map は Hello として読めるため、送信側が別メッセージのつもりだっただけでは拒否しません。
最初のフレーム前の正常な EOF は、何も送らず正常終了します。

## 呼び出し・応答・ヒント

`Ack` 後はホストが次の enum を送ります。`sid` はホストが選ぶセッション id、`rid` は接続内で一意の
要求 id です。SDK ホストの両カウンタは1から始まります。全セッションの処理は到着順に直列実行し、
別の reader が実行中にもキャンセルを受信します。

| ホストのメッセージ | フィールド | 成功時の応答・効果 |
|---|---|---|
| `Open` | `sid:u64, rid:u64, input:Input, cx:Cx` | `Opened {state:State}`、セッション作成 |
| `View` | `sid:u64, rid:u64, req:ViewReq, cx:Cx` | `Viewed {view:View, state:State}`、現在の表示を取得 |
| `Act` | `sid:u64, rid:u64, action:S, cx:Cx` | `Acted {outcome:Outcome, state:State}`、宣言済み操作 id |
| `Key` | `sid:u64, rid:u64, key:Key, cx:Cx` | `Acted {outcome:Outcome, state:State}`、通常 canvas 入力中の生キー |
| `Cancel` | `rid:u64` | 単独の応答なし、元の呼び出しの終了を要求 |
| `Close` | `sid:u64` | 応答なし、そのセッションを破棄 |
| `Shutdown` | なし（unit string） | 応答なし、セッションを破棄し正常終了 |

呼び出しごとに `FromChild = {rid:u64, answer:Answer}` を1回返します。
`Answer = Ok(Reply) | Err(Failure)` で、`Reply` は上の3種です。全呼び出しで `Err` を返せます。
エラーに新しい状態は含みません。成功応答の種別違い、未知・重複した応答 `rid` は接続を終了させます。
現在のホストは `rid` で振り分け、応答順を明示検査しません。直列処理は参照 server の動作で、期限
計算の前提です。server は要求 id・セッション id の重複を独立には検査せず、同じ `Open.sid` は
旧セッションを破棄して置き換えます。未知セッションの呼び出しは `Err(Corrupt)`、未知の `Close` と
`Cancel` は無視します。参照 server の上限は1,024セッションで、それ以上の新規 open は `TooHeavy` です。

Cancel は即時応答ではなくフラグを立てます。待機中に中断された呼び出しは worker に到達すると
プラグインコードを実行せず `Err(Cancelled)` を返します。完了済み view の成功も許されます。
実行中はフラグを確認して速やかに応答してください。SDK は Shutdown で実行中・待機中の呼び出しも
中断し、キュー処理後に sid 昇順でセッションを破棄します（ホストの割り当てでは open 順）。
stdin の EOF でも全フラグを立てて終了し、処理が止まっていれば `serve` が2秒後にプロセスを終了
させます。不正な入力では server がエラー終了します。SDK 呼び出し内の panic は `Err(External)`、
`manifest()` の panic は起動失敗になります。panic の捕捉は SDK 固有の補助動作です。

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

`repr` は表現を選び、`nil` は既定です。page は0起点で `nil` は最初または全体。wire 変換は page が
受信側の index 型に収まるかだけを検査し、宣言済みページ数との比較はしません。`cols/rows` は0も可、
`cell_px` は判明している場合のセル幅・高さのピクセル数です。`layout` と `enabled` は成功応答で更新する
スナップショット。enabled にない操作は true、重複 id は最後の値が有効です。
**ヒント要求・ヒント応答はありません**。ホストは manifest の操作と enabled からフッター・ヘルプ・
メニューを導出します。`Unhandled` は未使用、`Redraw` は view 再取得、`Flash` は翻訳メッセージ表示、
`Reload` は入力の再 open 要求です。生キーの修飾ビットは Ctrl=1、Alt=2、Shift=4、未知ビットは除去。
変換はファンクションキー番号を制限しません。layout は64表現と100,000ページ・タイトルに切り詰め、
長すぎる表現 id は捨てます。利用者向け翻訳文は512文字までに切り詰め、改行・タブ以外の制御文字と
U+10EEEE を除去し、英語以外を最大16言語保持します。Failure.detail も切り詰めます。

<a id="minimal-loop"></a>

## 実装者向けの最小ループ

テキスト専用プラグインの通信上の責務は次の順です。

1. 何も送らず Hello を読み、提示された版を選び manifest 付き Ack を返します。一致する版がなければ
   Refused を返します。下記 manifest の nonnullable フィールドを揃え、optional は nil、使わない宣言配列は
   空にできます。ファイルビューアは file bind と `shows:["Text"]` を宣言します。設定・操作・独自画面・
   起動verbは必須ではありません。通信例の manifest が完全な最小例です。
2. 受信順に直列処理し、sid ごとにセッションを保持します。Open は入力を準備し、layout/enabled を持つ
   Opened、または Err を返します。View は Text view と現在の状態を持つ Viewed、または Err を返します。
   サイズ非依存・表現1つ・ページなしなら repr・page・cols・rows・cell_px によって出力を変える必要は
   ありません。ただし wire 型は宣言どおりにします。
3. 操作・入力を実装しなくても Act・Key に応答します。`outcome:"Unhandled"` と現在の state を持つ
   Acted で十分です。call を黙って捨ててはいけません。未openの sid の call は失敗にし、SDK は
   Err(Corrupt) を使います。
4. Cancel 自体に応答はせず、対象の call を中断猶予内に終了させます。Err(Cancelled) でも、既に完了した
   view の成功でも構いません。未知の cancel id は無視します。Close は応答なしでセッションを破棄し、
   未知の close id は無視できます。call が長く止まり得るなら、その間も中断を受信できるようにします。
   処理が直列だからといって、パイプを無期限に読まなくてよいわけではありません。
5. Shutdown には応答せず、セッションを解放し、終了猶予内に終了コード0で終了します。フレーム間の正常な
   EOF は新しい処理を止めて資源を解放します。SDK は通常この場合も0で終了し、Hello 前の EOF も同様です。
   フレーム途中の切断・不正・過大フレームは正常 EOF ではなくエラーで、serving を止めます（SDK CLI は1）。
   検査ツールは不正フレームも実際に送ります。

参照 SDK は reader/worker 分離・cancel flag・panic guard・fd 手順を提供します。他言語では同等の通信
動作を自分で実装します。特に SDK はホスト EOF で直ちに中断し、処理が止まったままなら2秒後に終了コード1で
強制終了します。これは正常 EOF の通常の成功終了とは別です。ホストは子の予期しない EOF を終了コードに
よらず接続失敗とし、検査ツールは Shutdown に終了コード0を要求します。別の hints メッセージは不要です。

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

`File` は読むファイルで `hint` は任意の content-type ヒント。`Content` は変換済みテキストと名前付き
バイトリソース・元の場所、`Embed` は埋め込みソース（`display` はブロック表示の区別）です。
SDK の lazy resource は送信せず省略します。`settings` はプラグイン設定。Value の変換では深さ32超
を拒否します（根は0）。`pixels` はピクセル表示可否。`colors` は0=ANSI16、1=ANSI256、2=truecolor、
未知値はANSI16。`external` は外部 fallback への協調的な許可です。`read_dirs=nil` は無制限、`[]`
は許可ディレクトリなし、その他は絶対パスの root 一覧。ホストがプラグインのために読める範囲を表し、
自身で読むプラグインも従うべき値です。`Tr.en` は英語、others は `[ ["ja", "幅"] ]` のような
言語・文言の組で、訳がなければ英語に fallback します。

同じ manifest map を Ack と SDK の JSON `--manifest` が使います（JSON は任意の path/bin を
表せませんが、この manifest の schema はそれらを含みません）。

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

`id` は識別子、`version` はリリース版、`api` は SDK API の契約、`name` は表示名です。
Bind はファイル名 glob・正確なファイル名・MIME・変換済み content・embed kind を選びます。
`option=true` は既定でなく明示選択の候補。`shows` は主表示の型で、ファイル binding にこれがないと
画面が付きません。`screens` は独自キーコンテキスト。操作の `key` は KeyCode でなくキー表記
（`"w"`、`"ctrl-g"`、`"F2"` など）。`footer` は小さい順の任意優先度、`screen` は有効コンテキスト。
`writes` は read-only 制御用の書き込み宣言です。`destructive` はデータ破壊の宣言ですが、現在の API
にはその確認宣言がまだないため、このフラグから確認フローがあると解釈しないでください。
`label` はフッター・メニュー用の短文、`help` は説明です。Verb は起動 fallback の順序で、
`open=true` が Open、false が Edit、`gui` は GUI/端末、`line_token` はエディタの行番号トークン。
Setting は既定値とヘルプ。`work_group` は排他的な処理グループ、`converts` は変換済み内容、
`file_text` は元ファイルのテキスト、`sized` は領域サイズ依存の view、`unavailable` は表示不能時の
ホスト表示を指定します。wire 変換は翻訳の切り詰めと値変換をしますが、id・glob・key の検証は
registry と key validator の仕事です。下記の検査ツールでそれらも検査できます。

登録時の id・操作 id・設定名・screen（操作の context も含む）・work group は `[a-z0-9._-]` の
ASCII 1〜64文字です。最大256操作・256設定・1,024 bind・64 screen・bind ごと512 target。
target は最大1,024バイトで制御文字・U+10EEEE なし、glob はコンパイル可能な必要があります。
設定の既定値は最大1,024 value node、list/table の入れ子8段（最外の container が1段目）、
table key は最大256バイトで制御文字・U+10EEEE なしです。これは wire の深さ32より厳しい登録時の
検査です。version は64文字に切り詰めますが、ここでは semver 検証しません。後から重複した操作 id と
destructive 操作は key-validation issue を伴って除去します。不正・衝突・予約済みの操作キーは、
操作をメニューに残してキーだけ除去することがあります。check_manifest と通信検査ツールはそれらを
問題なしとして扱わず、報告します。

## View

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

Text はパス・所有テキスト・装飾済み行のいずれかです。`file_lines` は元ファイルと行番号が一致する
こと、`syntax_name` は任意のホスト highlighter 名。`note` は SDK の装飾 fallback 診断で子の
stderr に記録し、ホストには表示しません。styler callback は渡らず、子が8 MiBまでのテキストを
装飾します。装飾の符号化が40 MiBを超えると平文・ファイルに fallback します。
Table は文字列セル・header・任意タイトル・rainbow/address 表示・切り詰め印を送り、セル detail
callback は渡りません。短い行は可、ncols より広い行・header は不可です。最大65,536列、
`max(行長,1)` の合計は最大4,000,000です。

Document は Markdown・resource・表示スイッチです。`details` と alignment の文字列は変換時には
enum 検証せずそのまま渡します。Reopen は変換済み content を別プラグインへ委譲し、Message は
ホストが描く説明行です。List は装飾セル・kind・任意の絶対ジャンプ先・任意 checkbox 状態を持ちます。
Diff は hunk の配列で、各 hunk は context/added/removed と任意の新旧行番号を持つ行の配列です。

Image の Encoded は画像形式のバイト、Rgba は隙間のない RGBA8 で、幅・高さは非0、バイト数は
厳密に `width*height*4`。アニメーション遅延は **マイクロ秒**、最大5,000 leaf、入れ子は不可です。
lazy image は渡せません。SVG の `trusted` は送信値によらず受信時に false にします。
file/base/origin/target のパスは絶対パスが必要です。canvas 外の wire RGBA 変換は非0とバイト数を
検査し、canvas の4,096ピクセル上限は検査しません。後段のホスト描画にも独自の制限があります。

Canvas は1辺最大4,096セル、全体最大4,194,304セルです。runs は厳密に rows 個の配列で、run は
`[0起点のstyle番号, セル記号の連結文字列]`。Unicode grapheme cluster に分割してセルを復元し、
幅広文字の継続セルを U+0000 で表します。各行は cols 個ちょうど必要です。通常の記号は1 grapheme、
UTF-8で最大32バイト、幅1か2、制御文字とU+10EEEEを含めず、幅2なら同じ行の直後に継続セルが必要。
幅は Unicode display width に U+FF9E・U+FF9F 1文字につき1列を追加した値です。継続セルは必ず
幅広文字の直後に置きます。隣接記号が1 grapheme に融合する場合は run を分けます。SDK は復元後の
canvas を検証します。

styles は12バイトの record：前景4、背景4、追加 modifier の little-endian 2、除去 modifier の
little-endian 2です。色は `[tag,a,b,c]` で、0=なし、1..17=上記 Color の名前付き variant の順
（Reset〜White）、18=indexed（a が番号）、19=RGB（a,b,c）。未使用バイトは0を送りますが受信時に
検査しません。未知 tag は拒否。record は端数なし、最大 `max(cols*rows,1)` 個です。
modifier の bit 0..8 は bold、dim、italic、underlined、slow blink、rapid blink、reversed、
hidden、crossed out。未知 bit は除去します（通常の Style も同様）。

canvas 画像は最大4,096配置・256種類の id。配置は非空で canvas 内に収まり、同じ id は1つのセル
サイズを使います。`image=nil` はその id の前の画像を再利用します。canvas RGBA はアニメーション
leaf を含め1辺最大4,096ピクセルです。`redraw_after_ms` は任意のミリ秒（SDK は切り上げ）。
wire 変換は u32 の値を clamp せず受け付け、canvas 表示の契約では全体が見える間だけ33 ms〜24時間
で再描画します。text_input は生キー入力要求で、<kbd>Esc</kbd> はホストが保持して入力を終了します。
通常の view 型が使えれば、ホストのスクロール・検索・選択・コピーを利用できます。

## 大きなバイト列

```text
Bin = Inline(B) | File {name:S, len:u64}
```

ホストが新しい受け渡しディレクトリ（0700）を作り、Hello.side_dir で伝えます。SDK は1 MiB以下を
inline、それより大きいバイト列を side file にします。これは送信側の閾値で、受信側が大きな inline
を禁じるわけではありません（フレーム上限は適用）。0600 の通常ファイルを排他的に新規作成し、
書き終えてからメッセージを送ります。SDK の名前はホストが `h-0`、`h-1`、…、子が `c-0`、`c-1`、…
で接続内で一意です。受信側は prefix を要求せず、1〜64バイトの単一名、`.`・`..` 以外、`/`・NUL
なしを要求します。len は実ファイル長と厳密一致。link と通常ファイル以外を拒否し、link を辿らず読み、
短い読み込みも含め読み取り後に削除します。1ファイル最大256 MiB、ホストの1メッセージの side-file
合計最大512 MiB、子の1応答は256 MiB。inline はこの合計に数えません。resource はデコード時に
最大100,000項目。変換失敗の未読ファイルは破棄し、接続終了時にディレクトリも削除します。
これらは resource・画像・canvas style のバイト列用で、大きな text/table 文字列が自動的に side
file になるわけではありません。なおフレーム上限を超える応答は参照 server が Err(TooLarge) にします。

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
| 子の常駐メモリ | 2 GiB、100 msごとに確認 |
| 保持する stderr 末尾 | 4,096バイト（失敗 detail はさらに無害化・切り詰め） |

RSS の制限は OS helper が測定値を返せる場合に適用し、測定できなければこの検査は無効で期限のみ
有効です。RSS 上限は監視によるもので、OS が強制するメモリ割当上限ではありません。

FailureKind の使い分けはプラグイン・SDK の慣例で、要求ごとの必須kind表はありません。サンプルは
std::fs::read のエラーを SDK の From<std::io::Error> で変換し、Io、英日I/Oエラーメッセージ、detail の
エラー文字列を返します（読めないファイルも含む）。ファイル以外の入力には明示的に Unsupported を使います。
SDK は未知セッションに Corrupt、実行前の中断に Cancelled を使います。これは参照実装の慣例で、
他言語の同等の失敗すべてに同じkindを強制する通信規則ではありません。Answer::Err は列挙済みの任意の
FailureKind を運べ、読めないファイルや未対応入力へのkind割り当てを wire は固定していません。
独自のwireエラー形式を作らず、プラグインに合うkindと役立つ文言を選んでください。

呼び出し期限は送信時でなく最古の未完了呼び出しになった時から数えます。Cancel の猶予は中断要求と
先頭到達の両方が成立してからです。これらはネゴシエーション値でなくホストの既定です。検査ツールは
通常の Limits（handshake 30秒を含む）を使い、CLI に timeout 上書きはありません。

timeout、クラッシュ・EOF、不正・過大フレーム、対応しない応答 id、応答種別違い、メモリ超過は子の
プロセスグループ全体を終了させます。待機呼び出しは失敗し、他のセッションも使用不能になり、次の
open は新しい子を起動します（起動失敗は再試行抑制の対象）。失敗には stderr の制限付き診断が付きます。
stdout の余計なバイトはヘッダ・本体として解釈するため、フレームエラーや期限切れになります。復旧用の
走査はありません。正常な Err または変換不正の view（壊れた canvas・相対画像パスなど）はその呼び出し
だけが失敗します。意図した停止は Shutdown を先に送り、終了猶予後に残りのグループを kill します。
leader が終了しても子孫を終了させます。SIGKILL 後の reap は500 msを上限とし、回収できない子は
非同期 reaper が所有し続けます。

## 通信例（デコード後の MessagePack）

`0x41` の1バイトが入った `/data/a.bin` を開く最小の hexdump 風プラグインです。各矢印が1フレーム。
先に M・C・L・S を定義して後で展開します。これは説明上の省略で、wire 上の参照ではありません。
`bin("...")` はパスの実バイトです。ここでは全 optional フィールドも記載しています。

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

Lines source・装飾なしのspan 1つ・行styleが実サンプルの View::text(lines) と同じ形です。S は2箇所それぞれ
独立したstyle map に展開します。16バイト幅のhex列は47文字に埋めます。この最小manifestは操作なしなので、
Act と生Key は Unhandled です。ここで Cancel は応答済みviewを指すため無視します。Cancel・Close・
裸の文字列 Shutdown に子の応答はなく、Shutdown はプロセスを終了します。

例えば Shutdown の全フレームは `00 00 00 09 a8 53 68 75 74 64 6f 77 6e`。
本体長9、MessagePack fixstr 長8（a8）、UTF-8 の Shutdown です。後ろに改行はありません。

## 参照テストベクター

4バイト長prefixを含む完全なフレームです。konoma-sample-hexdump の tests/protocol_vectors.rs が
実際の frame::send で生成しました。上の Hello と、最小の M（サンプルの操作付きmanifestではない）を
使う Ack を符号化しています。offset は各フレーム先頭からの16進バイト位置です。Hello本体は67バイト
（0x43）、Ack本体は218バイト（0xda）。mapのフィールド順は参照encoderの出力順であり、受信側はこの順序に
依存せずmapキーを使ってください。テストは両言語のフレームを再符号化して照合し、通信例のviewも実サンプルと
照合します。

Hello:
<!-- vector:hello -->
```text
0000  00 00 00 43 84 a8 76 65 72 73 69 6f 6e 73 91 01
0010  ac 68 6f 73 74 5f 76 65 72 73 69 6f 6e a6 30 2e
0020  33 31 2e 32 a8 73 69 64 65 5f 64 69 72 c4 10 2f
0030  70 72 69 76 61 74 65 2f 68 61 6e 64 6f 66 66 a4
0040  6c 61 6e 67 a2 65 6e
```

Ack:
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

実行時に Cargo を使わない場合は、先にcheckerをビルドし、その実行ファイルを使います。既定の開発用パスは
target/debug/konoma-plugin-test、CARGO_TARGET_DIR 指定時は $CARGO_TARGET_DIR/debug/konoma-plugin-test
です（releaseビルドではdebugの代わりにrelease）。

```sh
cargo build -p konoma-plugin-test
"${CARGO_TARGET_DIR:-target}/debug/konoma-plugin-test" --file a.bin /absolute/path/to/plugin
"${CARGO_TARGET_DIR:-target}/debug/konoma-plugin-test" --plugin hex --file a.bin python3 -- ./plugin.py
```

checker は呼び出し元のディレクトリを継承するため、相対スクリプト ./plugin.py を使えます。
製品設定の command は絶対パス必須ではなく、python3 は PATH、./viewer は konoma 起動ディレクトリから
解決します。その後、子は専用ディレクトリで動くため、args には `args = ["/absolute/path/plugin.py"]`
のように絶対スクリプトパスを使ってください。引数文字列はそのまま渡し、相対というだけでは拒否しません。
[設定リファレンス](../configuration/)も参照してください。

ツールは選択時の `--plugin <id>`、続けて `--serve` を付けます。`--manifest` でなくフレームの
handshake を検査します。manifest・registry の検証、版拒否、open/view、ヒント、最初に enabled な
操作と生キー、未知セッション、中断、終了、不正 MessagePack、過大ヘッダを検査します。
独立した probe ごとに新しいプロセスグループと非公開 handoff ディレクトリを使います。`--file` がない
ファイル依存の検査は成功でなく **SKIP**。操作・キーの Err は接続が生きていれば有効です。
中断は完了済み view も認めます。終了コードは0=失敗なし、1=適合失敗、2=使用方法不正です。
probe の成功は通った経路の証拠で、全描画・セキュリティ特性の証明ではありません。Rust からは
`konoma_plugin_test::run(&Options, Limits::default())` を呼べます。
