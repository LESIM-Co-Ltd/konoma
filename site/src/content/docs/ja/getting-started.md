---
title: はじめる
description: インストール・2画面モデル・5分ツアー。
---

## インストール

```sh
cargo binstall konoma        # プレビルドバイナリ(cargo-binstall は Homebrew で導入)
# または
cargo install konoma         # Rust ツールチェーンでソースからビルド
```

cargo を使わない場合は
[GitHub のリリースページ](https://github.com/LESIM-Co-Ltd/konoma/releases)から
tarball を直接ダウンロードできます。

**動作要件**

- 主対象は macOS (Apple Silicon)。Intel macOS も動作し、Linux x86_64 は
  CI で全テストが通り(clippy + test 両 feature)プレビルドも配布し、画像・PDF・
  Markdown のプレビューが kitty graphics で描画されることを検証済み — macOS より
  新しいため beta 扱いです。Windows は非対応です。
- 画像 / SVG / 動画サムネイル / PDF のプレビューには **kitty graphics
  プロトコル**対応端末([Ghostty](https://ghostty.org) か kitty)が必要です。
  テキスト系プレビューはどの端末でも動きます。
- アイコンには Nerd Font グリフが必要: 端末のフォールバックフォントに
  `Symbols Nerd Font Mono` を足すか、NF 内蔵フォント(HackGen Console NF・
  UDEV Gothic NF など)を使ってください。無い場合は設定で `ui.icons = false`
  にすればプレーン記号で動きます。

任意ツール(無くてもその機能以外は動作): `git`(git スイート)・`poppler`
(PDF の複数ページ)・`ffmpegthumbnailer`/`ffmpeg`(動画サムネイル)・
`lazygit`(`O` の外部 git ツール)。

## 2画面モデル

```sh
konoma            # カレントディレクトリを開く
konoma ~/work     # 任意のディレクトリ
```

konoma のメイン画面は2つだけで、分割ペインはありません:

1. **ツリー** — 全画面のファイルツリー。`j`/`k` で移動、`l` か `Enter` で
   展開/潜行、`h` で親へ。
2. **プレビュー** — 選んだファイルの全画面表示。`q`(または `Esc`)でツリーへ。

git ビューやブックマーク一覧・ヘルプは、この2画面の上に重なります。
覚えるべき習慣は2つ:

- **`?` = いま見ている画面のヘルプ。** すべてのビューが自分のキーを説明します。
- **`q` = 一段戻る。** `Q` はどこからでも終了(確認付き)。

## 5分ツアー

1. プロジェクトのディレクトリで `konoma` を起動。
2. `/` を押して数文字入力 — ツリーが絞り込まれます。`Esc` で解除。
3. Markdown ファイルを選んで `Enter` — 見出しや表つきで描画されます。
   `Tab` でリンクをフォーカス、`Enter` で辿り、`q` で戻る。
4. 画像や PDF を選ぶ — 実ピクセルの全画面表示。`+`/`-` でズーム、
   PDF は `J`/`K` でページ送り。
5. git リポジトリ内で `o` — 変更ハブが開きます。ファイル上の `Enter` で
   全画面 diff、`l` で log、`g` でコミットグラフ。`q` で戻る。
6. `m` → `a` で現在地をブックマーク。`'` で一覧が開き、英字キーでジャンプ。

## 組み込みチュートリアル

リポジトリには **konoma 自身で読む**ハンズオンツアーが同梱されています —
実際に辿れるリンクと、実際にトグルできるチェックボックス付きです:

```sh
git clone https://github.com/LESIM-Co-Ltd/konoma
konoma konoma/samples        # tutorial.ja.md を開く
```

## 次に読む

- [チュートリアル](../tutorial/) — 7ステップのガイド付きツアー。
- [AI エージェントと働く](../guides/agent-watch/) — konoma の看板ワークフロー。
- [プレビューを使い倒す](../guides/preview/) — Markdown・テーブル・メディア・コピー。
- [git スイート](../guides/git/) — ハブ・diff・log・グラフ・ブランチ。
- [ファイル・ブックマーク・タブ](../guides/files/) — ファイルマネージャとしての顔。
- [設定リファレンス](../reference/configuration/) — 全オプションを1ページで。
