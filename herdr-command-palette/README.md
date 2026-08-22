# Herdr Command Palette

インストール済みプラグインの Action を1つの検索可能なリストから実行するプラグインです。

自前のコマンドは一切持ちません。`plugin.action.list` を読んで並べ、選ばれたものを
`plugin.action.invoke` で実行するだけです。プラグインが Action を公開すれば、
何もしなくてもここに出てきます (Pane Manager 仕様書 §12)。

---

## インストール

```bash
cd herdr-command-palette
cargo build --release
herdr plugin link "$PWD"
```

## 使い方

### キーボード

```toml
[[keys.command]]
key = "prefix+space"
type = "popup"
command = "~/src/herdr-plugins/herdr-command-palette/target/release/herdr-command-palette ui"
width = "70%"
height = "70%"
```

```
文字入力       絞り込み
Tab            プラグインを切り替え
Shift+Tab      逆順に切り替え
↑ ↓            選択移動
Enter          実行
Esc            キャンセル
```

### 表示例

```
Command Palette
on Claude · herdr-plugins
Tab ▸  [All]  Pane Manager  Layout Tools  Navigator  Sessions  Open

> pane mo    2

Pane Manager
  Move to Tab...
    Move this pane into another tab
  Quick Move to Tab 2
```

プラグイン名は見出しにまとめますが検索対象には含まれるので、
`pane mo` で `Pane Manager: Move to Tab...` が引けます。

### プラグインで絞る

`Tab` でプラグインを1つずつ辿ります。タブの並びは **Action を持つ
プラグインから作られる**ので、インストールすれば勝手に増え、Action の無い
プラグインで空の画面になることはありません。プラグインが1つしか無いときは
タブ自体が出ません。

絞り込んだ画面では見出しを出しません。いま選んでいるプラグイン名がすぐ上の
タブに出ているので、行ごとに繰り返す意味がないためです。

「Pane Manager に何ができるか」は言葉ではなくプラグインへの問いなので、
入力で当てるより辿る方が速い、という切り分けです。

### CLI

```bash
CP=./target/release/herdr-command-palette

$CP list                          # plugin<TAB>action<TAB>title
$CP run pane-manager extract      # 直接実行
```

---

## Context の受け渡し

Palette は popup として開くため、開いた時点でユーザーがフォーカスしていた Pane を
先に解決しておき、Action 実行時に `focused_pane_id` / `tab_id` / `workspace_id` として渡します。
`Pane Manager: Move to Tab...` を Palette から実行しても、Palette 自身ではなく
元の Pane が対象になります。

Palette 自身の Action は一覧から除外されます (開いている窓をもう一度開くだけなので)。

## ライセンス

MIT
