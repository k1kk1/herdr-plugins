# Herdr Layout Tools

Herdr の **同一 Tab 内のレイアウト**を整えるプラグインです。
Pane Manager が「その Pane はどの Tab に属するか」を扱うのに対し、
Layout Tools は「その Tab の中でどこに置くか」だけを扱います。

Pane Manager 仕様書 §2.3 / §17 / §27 で Pane Manager の責務外とされている領域を担当します。

---

## できること

| 操作 | 内容 |
|---|---|
| **Equalize** | Tab 内の全 Pane を等しい面積にする |
| **Grid** | だいたい正方形になるよう格子状に並べる |
| **Columns** | 全 Pane を横一列に並べる |
| **Rows** | 全 Pane を縦一列に並べる |
| **Main Left / Right** | 現在の Pane を左右いずれかに大きく、残りを反対側へ縦に積む |
| **Main Top** | 現在の Pane を上に大きく、残りを下に横一列で並べる |
| **Zoom** | 現在の Pane のズームを切り替える |

いずれもプロセスは止まりません。

---

## 動作要件

- Herdr 0.8.0 以降
- Rust ツールチェーン (`cargo`)
- macOS / Linux

## インストール

```bash
cd herdr-layout-tools
cargo build --release
herdr plugin link "$PWD"
```

---

## 使い方

### キーボード

`~/.config/herdr/config.toml` に追加します。

```toml
[[keys.command]]
key = "prefix+alt+l"
type = "popup"
command = "~/src/herdr-plugins/herdr-layout-tools/target/release/herdr-layout-tools ui"
width = "50%"
height = "60%"
```

メニューのキー:

```
e  Equalize
z  Zoom current pane
g  Grid
c  Columns
r  Rows
h  Main Left
l  Main Right
t  Main Top
q / Esc  Cancel
```

現在の Tab がすでにその配置になっている項目には `current` と表示されます。

`j` `k` は常にカーソル移動です。Main Top は以前 `k` でしたが、`k` を押すと
配置が変わってしまい上へ移動できなかったため `t` に変えました。

### 右クリック

Pane / Tab の右クリックメニューに `Layout Tools` と各配置が並びます。

### Plugin Action

```bash
herdr plugin action invoke equalize   --plugin layout-tools
herdr plugin action invoke grid       --plugin layout-tools
herdr plugin action invoke main-left  --plugin layout-tools
```

### CLI

```bash
LT=./target/release/herdr-layout-tools

$LT equalize [--tab w1:t2]
$LT arrange grid|columns|rows|main-left|main-right|main-top [--tab ID] [--pane ID]
$LT doctor    # 現在 Tab の split tree と各 split の比率を表示
```

`--pane` は `main-*` で大きく表示する Pane を指定します。省略するとフォーカス中の Pane です。

---

## 実装メモ: なぜ `layout.apply` を使わないか

Herdr の socket API には `layout.apply` があり、一見すると Tab の再構成に使えそうですが、
**既存の pane_id を無視して Tab ごと作り直します**。実測すると:

```text
layout.apply(tab_id=w6:t1, root=<既存 pane_id を並べた木>)
→ w6:t1 が閉じ、新しい w6:t2 に新しい pane が4つ生成される
→ 元の Pane で動いていたプロセスは全て死ぬ
```

これは「プロセスを止めない」という前提に反するため、Layout Tools では使いません。代わりに:

- **比率調整** — `layout.set_split_ratio` (非破壊)
- **構造変更** — 一時 Tab へ退避し、`pane.move` で目的の順序に戻す

`pane.move` は pane_id を保持するので、シェルも Agent も生き残ります。
一時 Tab は空になると Herdr が自動的に閉じます。

### Equalize の比率計算

split の比率は「第1分岐が取る割合」なので、均等にするには `0.5` ではなく
`葉の数(第1分岐) / 葉の数(split全体)` を設定します。

```text
(r p1 (r p2 (r p3 p4)))
  root      1/4 → 0.25
  右の子    1/3 → 0.333
  その子    1/2 → 0.5
→ 4等分
```

### 配置順序が重要な理由

Pane `X` を split すると `(方向 X 新Pane)` になり、`X` は第1分岐に残ります。
したがって「列全体にまたがる Pane」は、その列が細分化される**前**に切り出す必要があります。
Grid では先に全列の先頭を作り、そのあとで各列を縦に分割しています。

---

## 開発

```bash
cargo test    # 各配置の形状を split tree レベルで検証
```

`src/arrange.rs` は Herdr なしでテストできる純粋なロジックです。
配置ミスは動いている Agent を巻き込むため、形状は全て単体テストで固定してあります。

## ライセンス

MIT
