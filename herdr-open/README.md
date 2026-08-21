# Herdr Open

いま見ている Pane の**作業ディレクトリ**を、ターミナルの外へ渡すプラグインです。
Finder で開く、エディタで開く、パスをクリップボードに取る。

他の5つが「セッションの中／外」を扱うのに対し、これだけが**外のアプリ**を向いています。
Pane は動かしませんし、フォーカスも変えません。

```
Open
~/src/herdr-plugins · repository root

▸ f  Reveal in Finder
  e  Open in VS Code
  c  Copy Path
  ───────────────────
  from ● Claude · herdr-plugins

↑↓ move · Enter open · g git root · q / Esc cancel
```

---

## どのディレクトリを開くか

Herdr は Pane について2つの cwd を返します。`foreground_cwd`（動いているプログラムの)
を優先し、無ければ `cwd`（シェルの）を使います。Agent が別の場所で起動されていても、
**画面に映っているほうの**ディレクトリになります。

git リポジトリの中にいるときは `g` でルートに切り替わります。
最初からルートを既定にしたいときは `prefer_git_root = true`。

---

## インストール

```bash
cd herdr-open
cargo build --release
herdr plugin link "$PWD"
```

## 使い方

### 右クリックメニュー / Command Palette

Pane を右クリックすると出ます。設定は要りません。

| Action | 内容 |
|---|---|
| **Open Directory...** | ピッカーを開く |
| **Reveal in Finder** | 即 Finder |
| **Open in Editor** | 即エディタ |
| **Open Repository in Editor** | git ルートをエディタで |
| **Copy Directory Path** | パスをクリップボードへ |

### キーボード

```toml
[[keys.command]]
key = "prefix+d"
type = "popup"
command = "exec \"$HOME/src/herdr-plugins/herdr-open/target/release/herdr-open\" ui"
description = "Pane の作業ディレクトリを開く"
width = "50%"
height = "40%"

# ピッカーを挟まず一発で開くなら、popup ではなく shell で。
[[keys.command]]
key = "prefix+i"
type = "shell"
command = "exec \"$HOME/src/herdr-plugins/herdr-open/target/release/herdr-open\" open editor --git-root"
description = "リポジトリをエディタで開く"
```

`type = "shell"` の場合、Herdr が `HERDR_ACTIVE_PANE_ID` を渡すので、
どの Pane で押したかは正しく解決されます。

### コマンドライン

```bash
herdr-open list                 # 設定されている行と、入っているかどうか
herdr-open where --git-root     # 開く先のパスだけ出す（スクリプト用）
herdr-open open finder --pane w1:p3
```

---

## 設定

```bash
herdr plugin config-dir open
```

に `config.toml` を置きます。[config.example.toml](config.example.toml) が雛形です。

```toml
prefer_git_root = true

[[target]]
id = "editor"
title = "Open in Cursor"
hotkey = "e"
command = ["cursor", "{dir}"]
```

`[[target]]` を1つでも書くと組み込みは置き換わります（消せるようにするためです）。
`command` は argv で、シェルを通しません。パスに空白や `'` が入っていても壊れません。

### 入っていないアプリは出しません

各行は `requires`（省略時は `command` の1つ目）が PATH にあるかを見て、
無ければ**一覧に出しません**。GUI アプリの起動は detach するので、押しても何も
起きなかったことを後から知る手段が無いためです。

---

## 外へ出すときは HERDR_* を落とす

macOS の `open` は呼び出し元の環境変数をアプリに渡します。Herdr の Pane から
そのまま起動すると、エディタが `HERDR_ENV=1` と他人の `HERDR_PANE_ID` を
持ったまま立ち上がり、その統合ターミナルが「自分は Herdr の中にいる」と
誤認します（`herdr` は入れ子を拒否します）。

なので子プロセスからは `HERDR_*` を全部消してから spawn します。
Sessions プラグインが踏んだのと同じ罠です。
