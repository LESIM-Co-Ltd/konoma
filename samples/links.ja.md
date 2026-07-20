# Link navigation demo

`Tab` / `⇧Tab` でリンクをフォーカス、`Enter` で開く。
（URL=ブラウザ / ローカルファイル=konoma プレビュー / ディレクトリ=そこへ移動）

## ローカルリンク（konoma 内で開く）

- コード: [hello.rs](./code/hello.rs) ／ [config.toml](./code/config.toml)
- テキスト: [japanese.txt](./japanese.txt)
- ディレクトリ: [code/](./code)
- 別の Markdown: [markdown.ja.md](./markdown.ja.md)

## 外部リンク（ブラウザで開く）

- [ratatui](https://ratatui.rs)
- [Rust](https://www.rust-lang.org)
- 自動リンク: <https://github.com>

## 表の中のリンク

セル内のリンクもラベルだけが表示され、`Tab`/`Enter` が段落リンクと同じように効く。

| 種別 | リンク | 備考 |
|------|--------|------|
| ローカル | [hello.rs](./code/hello.rs) | konoma 内で開く |
| 外部 | [ratatui](https://ratatui.rs) | ブラウザで開く |
| title 付き | [japanese.txt](./japanese.txt "日本語サンプル") | title は URL に混ざらない |
| 1セルに複数 | [markdown.ja.md](./markdown.ja.md) と [code/](./code) | 順にフォーカスできる |
| CJK 混在 | 前置き [README](./README.md) 後置き | 桁揃えが崩れない |

## 存在しないリンク（エラー表示）

- [missing](./does-not-exist.md)
