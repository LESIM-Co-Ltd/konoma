---
title: プラグインを書く
description: Rust SDK で小さなバイナリビューアを作り、テストして konoma にインストールする。
sidebar:
  order: 6
---

プラグインは開く対象を宣言して表示データを返し、konoma が描画します。このガイドは実在する `crates/konoma-sample-hexdump` を使います。`*.bin` を offset・16進数・ASCII の列で表示し、<kbd>w</kbd> で1行16/8バイトを切り替えます。全ファイルをメモリに読み、`view` ごとに全行を整形する、小さなファイル向けの教材です。

## テンプレートから始める

Rust 1.92以降と、利用可能な `cargo generate` コマンドが必要です。konoma の checkout から：

```sh
cargo generate --path templates/konoma-plugin --name my-hexdump
cd my-hexdump
```

テンプレートは crates.io での発見用に `keywords = ["konoma-plugin"]`、SDK に版指定の `konoma-plugin = "0.1.0"` 依存、独立した端末表示と通信を含む `app` feature を設定します。**SDK の crate はまだ公開されていません**。このテンプレートは公開済み SDK が必要です。今すぐ開発する場合は下のワークスペースサンプルを使うか、`[patch.crates-io]` で `konoma-plugin` を checkout の `crates/konoma-plugin` に向けてください。

SDK が利用可能になったら（またはローカルで上書きしたら）、生成したプロジェクト内で：

```sh
cargo run -- a.bin
cargo build
konoma --plugin ./target/debug/my-hexdump a.bin
```

既存の `.bin` ファイルを使います。これは既定の `target` ディレクトリの例です。`CARGO_TARGET_DIR` を設定している場合は、その実行ファイルのパスに替えてください。`cargo run -- a.bin` は SDK の独立した端末を開きます。konoma 内ではプラグインがテキストを返し、スクロール・検索・選択・コピーはホストが提供します。<kbd>w</kbd> は宣言済み `width` 操作を実行し、<kbd>?</kbd> でそのヘルプを表示します。

## `Manifest`・`open`・セッション

以下はサンプルの `src/lib.rs` 全体をそのままコピーしたものです。生成するライブラリも同じコードで始まり、`id`・`version` は自分の Cargo package から取るため、生成後の `id` は `my-hexdump` です。

```rust
use konoma_plugin::prelude::*;

pub struct Hexdump;

impl Plugin for Hexdump {
    fn manifest(&self) -> Manifest {
        Manifest::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Hex dump", ja: "16進ダンプ"))
            .bind(Bind::files(["*.bin"]))
            .shows(ViewKind::Text)
            .action(
                Act::new("width")
                    .key("w")
                    .label(t!(en: "width", ja: "幅"))
                    .help(t!(en: "Switch 8/16 bytes per row", ja: "1行のバイト数を8/16に切り替え"))
                    .footer(1),
            )
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(
                t!(en: "Expected a file", ja: "ファイルが必要です"),
            ));
        };
        Ok(Box::new(HexSession {
            bytes: std::fs::read(path)?,
            width: 16,
        }))
    }
}

struct HexSession {
    bytes: Vec<u8>,
    width: usize,
}

impl Session for HexSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let lines = self
            .bytes
            .chunks(self.width)
            .enumerate()
            .map(|(row, bytes)| {
                let hex = bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let ascii: String = bytes
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                format!(
                    "{:08x}  {hex:<pad$}  |{ascii}|",
                    row * self.width,
                    pad = self.width * 3 - 1
                )
            })
            .collect::<Vec<_>>();
        Ok(View::text(lines))
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        if action != "width" {
            return Ok(Outcome::Unhandled);
        }
        self.width = if self.width == 16 { 8 } else { 16 };
        Ok(Outcome::Redraw)
    }
}

pub fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![Box::new(Hexdump)]
}
```

`Manifest::bind` が対象ファイル、`shows` が表示型とホストの予約キーを宣言します。両方必要です。`Act` は操作 `id`・既定キー・短いラベル・ヘルプ・フッター優先度を宣言します。`t!` は英日文言を持ち、翻訳がなければ英語に戻ります。ホストは同じ宣言からキー・フッター・ヘルプ・<kbd>Space</kbd> メニューを作ります。`Session::enabled` の既定は `true`。操作が一時的に使えなければ override して、キーとヒントの状態を揃えます。

`open` はファイル以外を拒否し、セッションを準備します。セッションはバイト列と行幅を所有します。`view` は状態を進めず読みます。resize・redraw などでいつでも呼ばれ得るためです。`View::text` は整形済み行を返し、描画はホストに任せます。最後の短い行も ASCII 列を揃え、非表示バイトは `.`、空ファイルは0行になります。`act` は状態変更後に `Redraw`、未知 `id` には `Unhandled` を返します。このサンプルは生キー入力を要求せず、konoma が <kbd>w</kbd> を `act("width", ...)` に変換します。

## 2つの入口

サンプルの `src/main.rs` は次の1行です。

```rust
konoma_plugin::export!(@program konoma_sample_hexdump::plugins());
```

生成後は代わりに `my_hexdump::plugins()` を使います。`export!` は `--serve`・`--manifest`・`--version`・`--help` と、`app` feature がある場合の単独端末用ファイル引数を実装します。通信の stdout にはフレーム以外を書かないでください。SDK は serving 中の通常の print を stderr に向けます。公開ライブラリ関数 `plugins() -> Vec<Box<dyn Plugin>>` は別の利用者のためです。`konoma plugin install` が生成するホストは `<crate>::plugins()` を呼びます。プログラムの入口とライブラリの入口の両方を残してください。Rust crate のインストールは再ビルドしたホストへのリンク、`--plugin` による実行ファイルの読み込みは子プロセス通信です。

checkout で実際のサンプルを動かす場合、選んだビルドディレクトリを使って：

```sh
cargo build -p konoma-sample-hexdump
cargo run -p konoma-sample-hexdump -- a.bin
konoma --plugin "$CARGO_TARGET_DIR/debug/konoma-sample-hexdump" a.bin
cargo run -p konoma-sample-hexdump -- --manifest
```

パスの例では `CARGO_TARGET_DIR` をビルドディレクトリに設定してください（未設定なら `target`）。開発用 `--plugin` は複数指定でき、その konoma の実行だけに有効です。

## 端末なしでテストする

以下はサンプルの `tests/hexdump.rs` のコピーです。`open_bytes` は隔離した一時ファイルを作り、`open_file` はそのパスで `open` を実行します。`view_text` は表示行を取り出します。`send_keys` は生の `Session::key` を直接呼び、manifest のキー割り当ては**解決しません**。このテキストプラグインは生キーを `Unhandled` にするため、宣言キーのテストは宣言を調べて `act` を呼びます。`check_manifest` はキーと `shows` の欠落、`check_hints` は宣言キーと、操作宣言・現在の使用可否からホストが表示する操作ラベル・ヘルプを検査します。

```rust
use konoma_plugin::prelude::*;
use konoma_plugin::testing::{check_hints, check_manifest, open_bytes, send_keys, view_text};
use konoma_sample_hexdump::Hexdump;

#[test]
fn hex_lines_include_a_short_final_row() {
    let (mut session, _file) =
        open_bytes(&Hexdump, "a.bin", b"ABCDEFGHIJKLMNOP\0\x7f\xff ").unwrap();
    assert_eq!(
        view_text(&mut *session, &ViewReq::new(100, 20)).unwrap(),
        vec![
            "00000000  41 42 43 44 45 46 47 48 49 4a 4b 4c 4d 4e 4f 50  |ABCDEFGHIJKLMNOP|",
            "00000010  00 7f ff 20                                      |... |",
        ]
    );
}

#[test]
fn empty_file_has_no_rows() {
    let (mut session, _file) = open_bytes(&Hexdump, "empty.bin", b"").unwrap();
    assert!(view_text(&mut *session, &ViewReq::new(100, 20))
        .unwrap()
        .is_empty());
}

#[test]
fn width_action_toggles_and_redraws() {
    let (mut session, _file) = open_bytes(&Hexdump, "a.bin", b"ABCDEFGHIJKLMNOP").unwrap();
    let manifest = Hexdump.manifest();
    let action = manifest
        .actions
        .iter()
        .find(|a| a.key.as_deref() == Some("w"))
        .unwrap();
    assert_eq!(action.id, "width");
    let req = ViewReq::new(100, 20);
    let before = view_text(&mut *session, &req).unwrap();
    assert!(matches!(
        session.act(&action.id, &ActCx::bare()).unwrap(),
        Outcome::Redraw
    ));
    assert_eq!(
        view_text(&mut *session, &req).unwrap(),
        vec![
            "00000000  41 42 43 44 45 46 47 48  |ABCDEFGH|",
            "00000008  49 4a 4b 4c 4d 4e 4f 50  |IJKLMNOP|",
        ]
    );
    assert!(matches!(
        session.act("width", &ActCx::bare()).unwrap(),
        Outcome::Redraw
    ));
    assert_eq!(view_text(&mut *session, &req).unwrap(), before);
}

#[test]
fn unknown_action_is_unhandled() {
    let (mut session, _file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    assert!(matches!(
        session.act("unknown", &ActCx::bare()).unwrap(),
        Outcome::Unhandled
    ));
}

#[test]
fn manifest_and_hints_are_clean() {
    let (session, _file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    assert!(check_manifest(&Hexdump).is_empty());
    assert!(check_hints(&Hexdump, &*session).is_clean());
    let action = &Hexdump.manifest().actions[0];
    assert_eq!(action.label.get("en"), "width");
    assert_eq!(action.label.get("ja"), "幅");
    assert_eq!(action.help.get("en"), "Switch 8/16 bytes per row");
    assert_eq!(action.help.get("ja"), "1行のバイト数を8/16に切り替え");
}

#[test]
fn sdk_helpers_open_view_and_send_raw_keys() {
    let (mut session, file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    let mut reopened = konoma_plugin::testing::open_file(&Hexdump, file.path()).unwrap();
    assert_eq!(
        view_text(&mut *reopened, &ViewReq::new(100, 20))
            .unwrap()
            .len(),
        1
    );
    let outcomes = send_keys(&mut *session, &[Key::char('x')])
        .outcomes()
        .unwrap();
    assert!(matches!(outcomes.as_slice(), [Outcome::Unhandled]));
}
```

checkout からサンプルの全テスト（実行ファイルの通信テストとテンプレートテストも含む）を実行します。

```sh
cargo nextest run -p konoma-sample-hexdump
cargo run -p konoma-plugin-test -- --file a.bin "$CARGO_TARGET_DIR/debug/konoma-sample-hexdump"
```

`konoma-plugin-test` は konoma の通常の期限を適用し、プログラムの `--serve` モードで子プロセス通信を検査します。ファイルも渡して入力を開く処理・表示・操作・操作ラベルとヘルプ・中断を確認します。ファイルなしではそれらは SKIP です。ライブラリの入口は `run(&Options, Limits::default())` で、サンプルの実行ファイルテストでも使います。詳細な検査とデータ形式は[プロトコル仕様](../../reference/plugin-protocol/)を参照してください。

## インストールと共有

ソースを読んだうえで、生成した crate のディレクトリからインストールします。

```sh
konoma plugin install my-hexdump --path .
konoma plugin list
```

ワークスペースのサンプルなら checkout から：

```sh
konoma plugin install konoma-sample-hexdump --path ./crates/konoma-sample-hexdump
```

CLI は `--path` があっても crate 名が必要です。ソースと作者を表示し、`Install it? [y/N]` と尋ねます。再ビルド中の build script は利用者のマシンで実行され、プラグインはリンク・子プロセスのどちらでも利用者の権限で、**サンドボックスなし**で動きます。信頼できるコードだけをインストールしてください。`--yes` は確認を省略し、端末がない場合は必須です。対応する konoma ライブラリと SDK の依存先も利用可能である必要があり、ローカルのプラグインパスを指定するだけでそれらが公開されるわけではありません。

自作 crate を公開したら、利用者は crate 名（任意で `@version`）でインストールできます。crates.io keyword の `konoma-plugin` を残してください。`konoma plugin update my-hexdump` は互換な新版で再ビルド、`konoma plugin remove my-hexdump` は削除して再ビルド、`konoma plugin clean` は保持したビルドキャッシュの削除、`list` は crate と取得元の表示です。

標準ビューアを置き換えるなら、独自の `id` と適切な binding・表示宣言を与え、設定で標準プラグインを無効にします。例えば CSV ビューアの置き換えでは `csv` を無効にします。

```toml
[plugins.csv]
enabled = false
```

[設定リファレンス](../../reference/configuration/)に `[plugins.<id>]`、`read_only`・プラグイン設定・キー上書きがあります。実行ファイルを直接設定する場合の例：

```toml
[plugins.hex]
command = "/absolute/path/to/viewer"
args = ["--plugin", "hex"]
read_dirs = ["/absolute/path/to/assets"]
```

これは `id` が `hex` のプラグインを選び、追加の asset ディレクトリを許可します。子は専用ディレクトリで動くため、`args` のスクリプト・ファイルは絶対パスにします。読み取り範囲はホストがプラグインのために読む対象の制限で、プラグイン自身のファイル操作を sandbox 化しません。置き換えのために標準の `id` を再利用しないでください。

Rust は必須ではありません。他言語のテキスト専用プログラムは、ホストの `Hello` を受け、manifest を含む `Ack` を返すところから始め、`Open`・`View`・`Shutdown` を処理します。未対応の `Act`/`Key` にも応答し、`Close`/`Cancel` を処理して、要求を黙って捨てないでください。EOF・不正フレームも含む[実装者向けの最小ループ](../../reference/plugin-protocol/#minimal-loop)に従います。manifest に必要なのは `id`・`version`・`api`・`name`・`binds`・`actions`・`verbs`・`settings`・`screens`・`shows`・`converts`・`file_text`・`sized`・`unavailable` で、`work_group` は nullable。使わない宣言配列は空にでき、file binding と `shows:["Text"]` で最小ファイルビューアを作れます。`konoma-plugin-test` で検査し、`--plugin` または `[plugins.<id>] command` で読み込みます。Rust の `plugins()` を使うインストール入口は Rust crate 向けです。

Rust を読まずサンプルの出力を再現するには、最初は1行16バイト、<kbd>w</kbd> で8と16を切り替えます。各行は小文字16進数のバイトオフセット（最低8桁、0埋め）、空白2個、空白1個区切りの小文字2桁16進数バイト、16進数列全体が `width*3-1` 文字になるまでの末尾空白、さらに空白2個、最後に縦棒で囲む ASCII 列です。`0x21`〜`0x7e` と空白バイト `0x20` はそのまま残し、その他はすべて `.` にします。最後の短い行も16進数列を同じ幅に埋め、ASCII の縦棒の内側は埋めません。空ファイルは0行です。平文の各行は `Lines` 内の1項目になり、1つの span と行それぞれに既定の style を持ちます。プロトコル通信例にその形があります。
