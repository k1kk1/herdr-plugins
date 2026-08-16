# Herdr Navigator

Herdr のセッション全体から Pane / Agent / Tab / Workspace を文字入力で絞り込んで移動するプラグインです。

Pane Manager が「Pane を動かす」のに対し、Navigator は **フォーカスを動かすだけ** です。
何かを移動したり閉じたりすることはありません (Pane Manager 仕様書 §2.3, §27)。

---

## できること

| Scope | 内容 |
|---|---|
| **Panes** | 全 Workspace の全 Pane |
| **Agents** | Agent が動いている Pane だけ |
| **Tabs** | 全 Workspace の全 Tab |
| **Workspaces** | 全 Workspace |

`Tab` キーで Scope を巡回します。

検索対象は Workspace label / Tab label / Agent 名 / Pane label / terminal title / cwd basename です。
部分一致ではなく順序を保った部分列マッチなので、`cldmb` で `Claude · mushi-battle` が引けます。

---

## インストール

```bash
cd herdr-navigator
cargo build --release
herdr plugin link "$PWD"
```

## 使い方

### キーボード

```toml
[[keys.command]]
key = "prefix+f"
type = "popup"
command = "~/src/herdr-plugins/herdr-navigator/target/release/herdr-navigator ui panes"
width = "70%"
height = "70%"
```

```
文字入力   絞り込み
↑ ↓        選択移動
Tab        Scope 切替 (Panes → Agents → Tabs → Workspaces)
Enter      移動
Esc        キャンセル
```

マウスのクリック / ホイールも使えます。

### Plugin Action

```bash
herdr plugin action invoke open       --plugin navigator   # Panes
herdr plugin action invoke agents     --plugin navigator
herdr plugin action invoke tabs       --plugin navigator
herdr plugin action invoke workspaces --plugin navigator
```

### CLI

```bash
NAV=./target/release/herdr-navigator

$NAV list panes|agents|tabs|workspaces   # id<TAB>表示名<TAB>状態
$NAV focus w1:p3                          # id の種類は自動判別
```

---

## 表示

```
agent-skill-snippet · Tab 2: herdr
   ● Claude · herdr-plugins  working
       Herdr pane manager プラグインをRustで制作
```

Workspace と Tab は見出しにまとめますが、検索対象には含まれるので
`herdr claude` のような横断的な絞り込みができます。

## ライセンス

MIT
