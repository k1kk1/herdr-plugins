# Herdr Plugins

[Herdr](https://herdr.dev) 用のプラグイン集です。責務を分けた4つのプラグインと、それらが共有する1つのライブラリで構成されています。

**動いているプロセスを止めずに** Pane を組み替えることを最優先の制約にしています。
Codex / Claude / dev server / log tail は、移動・交換・統合・集約のどれを行っても再起動されません。

| プラグイン | 責務 | 既定キー | Action数 |
|---|---|---|---|
| [herdr-pane-manager](herdr-pane-manager/) | Pane が **どの Tab に属するか** | `prefix+m` | 21 |
| [herdr-layout-tools](herdr-layout-tools/) | Tab の **中のどこに置くか** | `prefix+alt+l` | 10 |
| [herdr-navigator](herdr-navigator/) | **探して飛ぶ**（何も動かさない） | `prefix+f` | 4 |
| [herdr-command-palette](herdr-command-palette/) | 全プラグインの Action を横断検索 | `prefix+alt+p` | 1 |
| [herdr-plugin-kit](herdr-plugin-kit/) | 共有ライブラリ（socket client / 表示名 / split tree / TUI） | — | — |

この分割は `herdr-pane-manager-spec.md` §2.3 / §27 の棲み分け方針に沿ったものです。
**どのプラグインも他のプラグインを必須依存にしません。** 単体でインストールしても動きます。

---

## 機能一覧

### Pane Manager

| 機能 | 内容 | 仕様 |
|---|---|---|
| **Move** | Pane を別の Tab へ移動。方向4種 × 比率3種 | spec §9.4 |
| **Quick Move** | `1`–`9` で Tab 1〜9 へ即移動。確認なし | 追加仕様 §2 |
| **Swap** | Pane 同士を交換。別 Tab・別 Workspace 可 | spec §9.5 / 追加仕様 §10 |
| **Extract** | Pane を新しい Tab / Workspace へ切り出し | spec §9.6 |
| **Merge** | Tab の Pane 群を別 Tab へ統合。split 構造を保持 | 追加仕様 §11 |
| **Gather** | 対応が必要な Agent を `Active Agents` Tab へ集約 | 追加仕様 §1–§19 |
| **Restore** | 集約した Pane を元の位置へ戻す | 追加仕様 §12 |
| **Undo** | 直前の Move / Extract / Merge / Swap を1段階戻す | — |

### Layout Tools

Equalize / Grid / Columns / Rows / Main Left / Main Right / Main Top / Zoom。
同一 Tab 内だけを扱い、Pane Manager とは責務が重なりません。

**保存レイアウト** — 今の形に名前を付けて保存し、あとで比率ごと復元できます。
Pane ID は保存しないので、同じ Pane 数の別 Tab にも適用できます。

### Navigator

Pane / Agent / Tab / Workspace を横断 fuzzy 検索して Focus を移すだけ。
`Tab` キーで検索対象が一巡します。**何も動かしません。**

### Command Palette

全プラグインが公開している Action を1つの一覧にまとめて実行します。
プラグインを追加すると設定なしで一覧に出ます。

---

## インストール

```bash
git clone <this repo> ~/src/herdr-plugins
cd ~/src/herdr-plugins

for p in herdr-pane-manager herdr-layout-tools herdr-navigator herdr-command-palette; do
  (cd "$p" && cargo build --release)
  herdr plugin link "$PWD/$p"
done

herdr plugin list --json
```

Rust ツールチェーン (`cargo`) と Herdr 0.8.0 以降が必要です。

Herdr 本体に plugin action 用のキーバインド機能がないため、
`~/.config/herdr/config.toml` に `[[keys.command]]` を追加します。

```toml
[[keys.command]]
key = "prefix+m"
type = "popup"
command = "~/src/herdr-plugins/herdr-pane-manager/target/release/herdr-pane-manager ui manager"
width = "60%"
height = "70%"
```

反映は `herdr server reload-config`、または `prefix+shift+r`。
残り3つのキーは各プラグインの README を参照してください。

---

## 共通の設計

詳細は [docs/herdr-api.md](docs/herdr-api.md) と [docs/ui-conventions.md](docs/ui-conventions.md) にあります。
ここは要点だけです。

### プロセスを止めない

Pane の再配置は全て `pane.move` で行います。`pane_id` が保持されるので、
中で動いているプロセスは生き残ります。

**`layout.apply` は使いません。** 名前に反して Tab ごと作り直し、中のプロセスを全て殺すことを
実測で確認しています。Layout Tools の Grid や Equalize も `pane.move` と分割比の変更だけで組み立てています。

### 状態をキャッシュしない

ピッカー表示直前と操作実行直前の2回、Herdr から状態を取り直します（追加仕様 §7）。
ユーザーや Agent が外から Pane を動かしていても破綻しません。
操作後にも Pane を読み直して着地を確認します（追加仕様 §8）。

### ユーザー操作なしに Pane を動かさない

Gather の基本原則（追加仕様 §14）ですが、全プラグインに適用しています。
Agent の状態が変わっても、監視して勝手に画面を組み替えることはしません。

### 戻せるようにする

Pane を物理的に動かす操作は、**動かす前に**元の位置
（Workspace / Tab / Tab名 / Tab の並び順 / 隣接 Pane と方向 / 復元順）を記録します。
Gather の Restore と Undo は同じ記録を再生しています（`herdr-pane-manager/src/place.rs`）。

### 内部 ID を出さない

`w1:p3` のような ID は `doctor` サブコマンドと `show_ids = true` のときだけ表示します。
通常は Pane label → Agent 名 → cwd basename → terminal title の優先順です（spec §3.2）。

---

## 練習環境

[handson/](handson/) に、7つの「困っている状況」を実際に組み立てるスクリプトと
ハンズオン資料があります。

```bash
cd handson
./handson.sh setup all
```

擬似 Agent を `herdr pane report-agent` で作るので、本物の Codex / Claude を起動せずに
Gather の優先度順やフィルタを試せます。

---

## 開発

```bash
for p in herdr-plugin-kit herdr-pane-manager herdr-layout-tools herdr-navigator herdr-command-palette; do
  (cd "$p" && cargo test)
done
```

現在 126 テスト。分割ツリーの組み立て・表示幅・fuzzy 検索・スクロール計算など、
壊れると画面で気づきにくいものを中心に単体テストにしています。

`herdr-plugin-kit` は path dependency で参照しています。cargo workspace には**していません** —
プラグインごとに `target/` を持たせて、マニフェストの
`$HERDR_PLUGIN_ROOT/target/release/...` が成立するようにするためです。

## ライセンス

MIT
