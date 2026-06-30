# Code highlighting samples

シンタックスハイライト確認用のサンプル集。各ファイルは konoma の既定 `code` ルール
(`*.{rs,ts,tsx,js,py,go,toml,json,sh,yaml,yml,c,cpp,h}`) にマッチし、純 Rust の syntect で着色される。

| ファイル | 種別 |
|------------------|------------------------|
| `hello.rs`       | Rust |
| `script.py`      | Python |
| `app.ts`         | TypeScript |
| `util.js`        | JavaScript |
| `main.go`        | Go |
| `calc.c`         | C |
| `widget.cpp`     | C++ |
| `types.h`        | C ヘッダ |
| `build.sh`       | シェルスクリプト |
| `config.toml`    | TOML 設定 |
| `config.yaml`    | YAML 設定 |
| `ci.yml`         | YAML 設定 (.yml) |
| `data.json`      | JSON データ |
| `large_generated.rs` | **1MB超の生成 Rust**（less 風ウィンドウ読みの確認用・連番マーカー入り） |

```sh
cargo run -- samples/code
```

`large_generated.rs`(約1.8MB/52512行)は閾値(256KiB)超なので **less 風ウィンドウ読み**で開く。
`j`/`Space`/`G` で末尾まで辿れ、タイトルの `[NN%]` が進捗を示す。一時用途なら削除してよい:
`rm samples/code/large_generated.rs`

各ファイルを開いて、キーワード・文字列・コメント・数値が色分けされることを確認する。
この README 自体の Markdown コードフェンスも同じ syntect 経路で着色される。
