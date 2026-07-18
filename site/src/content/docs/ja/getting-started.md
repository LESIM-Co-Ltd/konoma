---
title: はじめる
description: インストール・2画面モデル・5分ツアー。
---

端末と Rust が既にあるなら、インストールはこれだけです:

```sh
cargo binstall konoma        # プレビルドバイナリ(cargo-binstall は Homebrew で導入)
# または
cargo install konoma         # Rust ツールチェーンでソースからビルド
```

cargo を使わない場合は
[GitHub のリリースページ](https://github.com/LESIM-Co-Ltd/konoma/releases)から
tarball を直接ダウンロードできます。**何もインストールされていない状態から**始める
なら、下の [ゼロからのセットアップ](#ゼロからのセットアップ) を参照してください。

## 動作要件

konoma の本領を発揮するゲートは OS ではなく **端末** です:

- konoma は **macOS と Linux**(Unix)で動きます。Windows は非対応(Unix 専用 API を
  使い、Windows の端末は kitty graphics プロトコルに非対応のため)。
- 全画面の **画像 / PDF / SVG / 動画プレビューには
  [kitty graphics プロトコル](https://sw.kovidgoyal.net/kitty/graphics-protocol/)対応端末**が
  必要です — [Ghostty](https://ghostty.org)・[kitty](https://sw.kovidgoyal.net/kitty/)・
  [WezTerm](https://wezterm.org)・Konsole など。**テキスト系プレビュー**(Markdown・
  コード・git diff)は **どの端末でも** 動きます。
- OS/アーキの組合せでは **macOS (Apple Silicon) が最も実績があります**。Intel macOS も
  動作し、**Linux x86_64** は CI で全テストが通りプレビルドも配布・プレビューが kitty
  graphics で描画されることも検証済みですが、macOS より新しいため **beta** です。

**フォント** — 2種類のグリフが関係します:

- **アイコン**(`ui.icons = true`・既定)には **Nerd Font** グリフが必要。端末の
  フォールバックに `Symbols Nerd Font Mono` を足すか、Nerd Font 内蔵フォントを使用。
  無ければ `ui.icons = false` でプレーン記号(豆腐を出さない)。
- **CJK 文字**(`jp` UI・CJK のファイル名/ファイル内容)には端末フォントに **CJK
  グリフ**が必要です — 無いと CJK が豆腐(□)になります。konoma は表示幅を正しく
  計算しますが、グリフ自体はフォント側が担います。**Nerd Font 内蔵の CJK フォント**
  (**HackGen Console NF**・**UDEV Gothic NF** など)なら、アイコンと CJK の両方を
  1つのフォントで賄えます。

**任意ツール**(無くてもその機能以外は動作): `git`(git スイート)・`poppler`
(PDF の複数ページ)・`ffmpegthumbnailer`/`ffmpeg`(動画サムネイル)・
`lazygit`(`O` の外部 git ツール)。

## ゼロからのセットアップ

何もインストールされていないマシンから、画像プレビューが動く konoma まで一通りの手順です。

### macOS (Apple Silicon)

1. **Homebrew**(あればスキップ)— パッケージマネージャ:
   ```sh
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
   ```
2. **kitty-graphics 対応端末** — Ghostty:
   ```sh
   brew install --cask ghostty
   ```
3. **Nerd Font + CJK グリフ入りフォント**(アイコンと日本語を1つで):
   ```sh
   brew install --cask font-hackgen-console-nf
   ```
   Ghostty の設定(`~/.config/ghostty/config`)に:
   ```
   font-family = "HackGen Console NF"
   ```
4. **konoma** — 最速はプレビルドバイナリ:
   ```sh
   brew install cargo-binstall
   cargo binstall konoma
   ```
   ソースからビルドするなら、先に Rust を入れて `cargo install konoma`:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   cargo install konoma
   ```
5. **任意ツール**(リッチなプレビュー用):
   ```sh
   brew install poppler ffmpeg git lazygit
   ```
6. **起動** — Ghostty を開いて:
   ```sh
   konoma            # カレントディレクトリ
   konoma ~/work     # 任意のディレクトリ · ? でヘルプ
   ```

### Linux (x86_64 · beta)

以下は `apt`(Ubuntu/Debian)の例です。お使いのパッケージマネージャに読み替えてください。

1. **kitty-graphics 対応端末** — Linux では kitty が最も簡単:
   ```sh
   sudo apt install kitty
   ```
   (Ghostty・WezTerm も可)
2. **フォント** — CJK グリフ + Nerd Font:
   ```sh
   sudo apt install fonts-noto-cjk        # CJK グリフ
   ```
   アイコンと CJK を1フォントで賄うなら、Nerd Font 内蔵 CJK フォント(HackGen NF 等)を
   `~/.local/share/fonts/` に入れて `fc-cache -f`、kitty の `font_family` に設定。
3. **konoma** — 最速は
   [リリースページ](https://github.com/LESIM-Co-Ltd/konoma/releases)の
   プレビルド(`konoma-x86_64-unknown-linux-gnu.tar.gz`)を展開して `PATH` に置く。
   ソースからビルドするなら、Rust と依存 C ライブラリのヘッダを入れます:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   sudo apt install pkg-config cmake libssl-dev libssh2-1-dev zlib1g-dev \
     libdbus-1-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev git
   cargo install konoma
   ```
4. **任意ツール**:
   ```sh
   sudo apt install poppler-utils ffmpeg git
   ```
5. **起動** — kitty-graphics 対応端末の中で:
   ```sh
   konoma            # ? でヘルプ
   ```

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
