# Herdr Pane Manager

Herdr の Pane / Tab を、**中で動いているプロセスを止めずに**別の場所へ移籍・交換・統合するための操作レイヤーです。
`pane_id` / `tab_id` を手で調べて CLI を叩く代わりに、右クリック・キーボード・Plugin Action のどれからでも同じ操作を実行できます。

tmux の `join-pane` / `break-pane` / `swap-pane` に相当する操作を、Herdr の UI から直感的に行えるようにするのが目的です。

対応する仕様書: `../herdr-pane-manager-spec.md` (MVP + 追加仕様)

---


## ホットキーと Shift

行を選ぶ手段は2つあり、**どちらも同じ2つの結果**に届きます。

| | 既定 | もう一方 |
|---|---|---|
| ホットキーで選ぶ | `m` | `Shift+M` |
| ↑↓ で選ぶ | `Enter` | `Shift+Enter` |

`Shift+M` は **`m` の行に対する `Shift+Enter`** です。押した結果が同じ画面に
見えてしまわないよう、位置指定が有効なときは副題に「位置を指定します」と出ます。

### Quick move の行は既定で出しません

```toml
[pane-manager]
show_quick_move = true    # 出したい場合
```

メニューで一番長い部分で、しかも `Move to…` から辿れるものと同じ場所へ行くだけ
なので、既定では出しません。**Tab の一覧ではなく操作の一覧**として読めるように
するためです。

出さなくても、同じ即移動は `quick-move-1`…`9` の **Action として残ります**
（Command Palette や右クリックから呼べます）。フッターの `1-9` の表示も、
行があるときだけ出ます。

### どちらを既定にするかは設定できます

```toml
[pane-manager]
default_action = "quick"      # または "detailed"
```

**Shift は常に「もう一方」**です。どちらに設定しても両方が1キーで届くので、
いつも位置を指定する人が毎回 Shift を押さずに済みます。フッターの表示も
設定に追随します。

```
1-9・英字・Enter すぐ移動    ·  Shift+英字 位置を指定    ·  Esc 閉じる
1-9・英字・Enter 位置を指定  ·  Shift+英字 すぐ移動      ·  Esc 閉じる
```

Tab 選択画面で Shift を押せば、そこでも**もう一方に戻せます**。
`Shift+M` で入っても、最後に気が変わったら取り消せるということです。

数字のホットキー（Quick move の `1`–`9`）には効きません。**数字の Shift は
キーボード配列によって別の記号になる**ので、`Shift+2` を約束できないためです。
そちらは今までどおり `Shift+Enter` です。

なお `Shift+M` は**ただの大文字**なので、`Shift+Enter` を報告できない端末でも
使えます。

## できること

中核操作は4つだけです。レイアウト調整・検索・Command Palette は既存プラグインの責務なので実装していません (spec §2.2, §27)。

| 操作 | 内容 |
|---|---|
| **Move** | 現在の Pane を別の Tab へ移動する |
| **Swap** | 現在の Pane と別の Pane を交換する (別 Tab 可) |
| **Extract** | 現在の Pane を新しい Tab へ切り出す |
| **Merge** | 現在の Tab の Pane 群を別の Tab へ統合する |
| **Gather** | 対応が必要な Agent Pane を `Active Agents` Tab へ集約する (追加仕様 §1–§19) |
| **Undo** | 直前の Move / Extract / Merge / Swap を元に戻す (1段階) |

いずれも `pane.move` ベースの非破壊操作で、Codex / Claude / server などのプロセスは再起動されません (spec §3.1)。

---

## 動作要件

- Herdr 0.8.0 以降 (plugin API と `pane.move` を使用)
- Rust ツールチェーン (`cargo`) — link 時にビルドされます
- macOS / Linux

---

## インストール

```bash
git clone <this repo>
cd herdr-pane-manager
cargo build --release
herdr plugin link "$PWD"
```

`herdr plugin link` はマニフェストの `[[build]]` を実行するので、`cargo build --release` は初回だけ手動で走らせておけば十分です。

確認:

```bash
herdr plugin list --json
herdr plugin action list --plugin pane-manager
```

アンインストール:

```bash
herdr plugin unlink pane-manager
```

---

## 使い方

### 1. 右クリック (Context Menu)

Pane を右クリックすると以下が並びます (spec §8.1)。

```
Pane Manager
Move to Tab...
Swap with Pane...
Extract to New Tab
Merge Into...
```

Tab を右クリックすると `Merge Into...` が出ます (spec §8.2)。
現在の Tab 自身は候補から除外されます。

`Extract to New Tab` だけは即時実行で、ピッカーも確認ダイアログも出ません (spec §9.6, §14.1)。
それ以外はピッカーが popup で開きます。

### 2. キーボード

`prefix + m` で Pane Manager モードを開きます。Herdr 本体には plugin action 用のキーバインドがないので、`~/.config/herdr/config.toml` に1行だけ追加してください (spec §9.1: 入口はひとつだけ)。

```toml
[[keys.command]]
key = "prefix+m"
type = "popup"
command = "~/src/herdr-plugins/herdr-pane-manager/target/release/herdr-pane-manager ui manager"
width = "60%"
height = "70%"
```

パスは実際の clone 先に合わせてください。反映は `herdr server reload-config`、または `prefix+shift+r`。

開いたモードのキー (spec §9.2, §29):

```
m     Move picker
s     Swap picker
e     Extract to new tab   (即時実行)
c     Merge current tab
u     Undo 直前の操作          (戻せるものがあるときのみ表示)
g     Gather active agents
r     Restore gathered agents  (集約中のみ表示)
1..9  Quick move current pane to tab
q/Esc Cancel
```

どのピッカーでも `↑` `↓` / `Enter` / `Esc` / マウスクリック・ホイールが使えます。
ホットキーが割り当てられていない画面では `j` `k` も選択移動になります
(`j` `k` は必ずカーソル移動です。方向ピッカーだけは `h` `j` `k` `l` の4つとも方向として使います)。

- **Quick Move** — `prefix+m → 2` で現在 Pane を Tab 2 へ即移動。確認は一切出ません (追加仕様 §2)
- **Move picker** — 文字入力で Workspace / Tab / Agent / Pane label / terminal title / cwd を横断 fuzzy 検索 (追加仕様 §4)
- **New Tab / New Workspace** — Move picker の `n` / `w`。検索文字列がそのまま名前になります (追加仕様 §5)

```
Move current pane to

> review    2

n  + New Tab "review"
w  + New Workspace "review"
```

- **Swap picker** — Tab ごとにグループ化された Pane 一覧。別 Workspace の Pane も対象 (spec §9.5)
- **Merge picker** — 統合先 Tab を選択 (spec §9.7)

`default_move_direction = "ask"` のときは配置方向を、`default_split_ratio = "ask"` のときは
サイズ比率を続けて聞かれます。方向は `Left` / `Right` / `Up` / `Down` の4方向です (追加仕様 §9)。

Advanced Move (移動先 Tab のどの Pane を分割するか毎回選ぶ) は
`advanced_move = true` で有効になります。

#### 個別ショートカット (opt-in)

上級者向けに、各操作へ直接バインドすることもできます。Herdr 本体や他プラグインとの衝突を避けるため既定では無効です (spec §10)。

```toml
[[keys.command]]
key = "prefix+alt+m"
type = "popup"
command = "~/src/herdr-plugins/herdr-pane-manager/target/release/herdr-pane-manager ui move"
width = "60%"
height = "70%"

# swap / merge も ui swap / ui merge で同様に。
# extract は UI 不要なので type = "shell" が使えます:
[[keys.command]]
key = "prefix+alt+e"
type = "shell"
command = "~/src/herdr-plugins/herdr-pane-manager/target/release/herdr-pane-manager extract"
```

### 3. Plugin Action

Command Palette 系プラグインや自動化から呼べます (spec §11, §12)。

```bash
herdr plugin action invoke open       --plugin pane-manager
herdr plugin action invoke move       --plugin pane-manager
herdr plugin action invoke swap       --plugin pane-manager
herdr plugin action invoke extract    --plugin pane-manager
herdr plugin action invoke merge-tab  --plugin pane-manager
herdr plugin action invoke quick-move-3 --plugin pane-manager

herdr plugin action invoke undo            --plugin pane-manager
herdr plugin action invoke gather-active   --plugin pane-manager
herdr plugin action invoke refresh-gather  --plugin pane-manager
herdr plugin action invoke restore-gather  --plugin pane-manager
herdr plugin action invoke gather-active-2 --plugin pane-manager   # 2 / 3 / 4
```

> 仕様書では `pane-manager.move` という ID ですが、Herdr は action id にドットを許可しません (`invalid_plugin_action_id`)。
> plugin id が既に名前空間なので、ここでは `move` / `swap` / `extract` / `merge-tab` と綴り、`--plugin pane-manager` で修飾します。

---

## 設定

`config.example.toml` を、`herdr plugin config-dir pane-manager` が出力するディレクトリに `config.toml` として置きます。

```bash
mkdir -p "$(herdr plugin config-dir pane-manager)"
cp config.example.toml "$(herdr plugin config-dir pane-manager)/config.toml"
```

| キー | 既定値 | 意味 |
|---|---|---|
| `default_move_direction` | `"right"` | `right` / `down` / `left` / `up` / `ask` |
| `default_split_ratio` | `"50:50"` | `50:50` / `60:40` / `40:60` / `ask` |
| `focus_after_operation` | `true` | 操作後に対象PaneへFocusする |
| `advanced_move` | `false` | 移動先Tabのどのpaneを分割するか毎回選ぶ |
| `preserve_merge_layout` | `true` | Merge時に元Tabのsplit構造を維持 |
| `auto_name_new_tab` | `true` | Extractした Tab を Pane 名から自動命名 |
| `show_agent_state` | `true` | ピッカーに `● ! ✓ ○ ?` を表示 |
| `show_terminal_title` | `true` | Pane が今なにをしているかを2行目に表示 |
| `confirm_merge` | `false` | Merge 前に確認する |
| `show_ids` | `false` | `w1:p3` をピッカーに表示 (デバッグ用) |

`[pane-manager.gather]` テーブル:

| キー | 既定値 | 意味 |
|---|---|---|
| `statuses` | `["blocked", "done", "working"]` | 集約対象の Agent 状態 |
| `max_panes_per_tab` | `4` | 1 Tab あたりの Pane 数 (2 / 3 / 4) |
| `scope` | `"workspace"` | `workspace` / `all` |
| `focus_highest_priority` | `true` | Gather 後に最優先 Agent へ Focus |
| `agents` | `[]` | Agent 種別で絞る (空 = すべて) |
| `tab_label` | `"Active Agents"` | 生成する Tab の名前 |

設定ファイルが壊れている場合は既定値で動作し、Pane Manager モードの末尾に警告を出します。操作が止まることはありません。

---

## スクリプトから使う

ピッカーを介さない headless 版もあります (spec §22 の Operation API に対応)。

```bash
PM=./target/release/herdr-pane-manager

$PM move   --pane w1:p3 --tab w1:t2 [--target-pane w1:p1] \
           [--side left|right|up|down] [--ratio 50:50|60:40|40:60]
$PM move   --pane w1:p3 --new-workspace --label "review"
$PM swap   --pane w1:p3 --with w1:p7
$PM merge  --source-tab w1:t2 --tab w1:t1 [--side S] [--ratio R] [--flatten]
$PM extract --pane w1:p3 [--new-workspace] [--label "review"]
$PM quick-move 2 --pane w1:p3

$PM undo          # 直前の Move / Extract / Merge / Swap を戻す

$PM gather [2|3|4] [--scope workspace|all]
$PM refresh-gather
$PM restore-gather

$PM doctor        # 全 workspace / tab / pane と split 構造を ID 付きで表示
```

`--direction` は `--side` の別名です (旧仕様の綴り)。

`--pane` を省略すると、フォーカス中の Pane が対象になります。

---

## 表示について

内部 ID (`w1:p3`) は通常 UI に出しません (spec §3.2)。Pane はこの優先順で表示されます。

1. Pane label
2. Agent 名
3. cwd basename / Project 名
4. terminal title

```
● Claude · mushi-battle
    Stackchan⇔CoreS3 ESP-NOW無線通信対応
```

Agent state (`● working` / `! blocked` / `✓ done` / `○ idle` / `? unknown`) は識別の補助であり、
Move / Swap の可否を制限しません (spec §13)。

---

## 安全性とエラー処理

- **確認なし** — Move / Swap / Extract / Merge はプロセスを止めない非破壊操作として扱います (spec §14.1)
- **キャッシュしない** — ピッカー表示直前と操作直前に Herdr から状態を取り直します。ユーザーや Agent が外から Pane を動かしていても破綻しません (spec §3.4)
- **消えた対象** — `Destination tab no longer exists.` を出して中止します (spec §15.1)
- **Swap の途中失敗** — 実行済みの move を逆順に巻き戻します。巻き戻せない場合は最新状態を取り直したうえで、なにが起きたかを明示します (spec §6.3, §15.3)

## Active Agent Gather (追加仕様 §1–§19)

複数の Agent を走らせていると、`blocked` (入力待ち) の Pane が別 Tab に埋もれて気づけません。
Gather は対応が必要な Agent Pane だけを `Active Agents` Tab へ集めます。

```
prefix+m → g → 4        # 4 Pane / Tab で集約 (追加仕様 §10)
prefix+m → g → w / a    # Scope を Current Workspace / All Workspaces に
prefix+m → r            # 全部を元の場所へ戻す
```

**優先度順** — `blocked` → `done` → `working` の順に並べ、同一状態内では状態変化が新しいものが先です (§3)。
`idle` / `unknown` と、Agent が検出されていない Pane (shell / dev server / log) は対象外です (§2)。

**レイアウト** (§6) — 集約先 Tab は Pane 数で決まった形を取り、最優先の Agent が先頭に入ります。

```
2 →  a1 | a2          3 →  a1 | a2        4 →  a1 | a2
                           a1 | a3             a3 | a4
```

5つ以上は `Active Agents 1` / `Active Agents 2` … と Tab が増えます (§5)。

**Restore** (§11, §12) — Gather は Pane を物理的に動かすので、動かす**前**に元の位置
(Workspace / Tab / Tab 名 / Tab の並び順 / 隣接 Pane と方向 / 復元順) を state ディレクトリの
`gather-session.json` に書き出します。途中でクラッシュしても戻せます。
元 Tab が既に閉じられていた場合は、元の名前・元の位置で Tab を作り直します。

**Refresh** (§13) — 状態が変わった Agent を反映して組み直します。対象外になった Pane は先に元の場所へ戻ってから、
残りが再配置されます。

**自動 Refresh はしません** (§14)。`ユーザー操作なしに Pane を勝手に移動しない` を原則とし、
Gather / Refresh / Restore はすべて明示的な操作でのみ走ります。

Phase 2 に回した項目: Agent 種別 Filter の UI 化 (設定 `agents` では既に可能)、
All Workspaces の常用、状態変化通知、自動 Refresh。

---

## Undo (1段階)

Move / Extract / Merge / Swap の直前の1回を元に戻せます。

```
prefix+m  →  u
```

戻せるものがあるときだけメニューに出ます。「take back the Move bravo」のように、
何を戻すことになるかが副題に出ます。

**仕組み** — Gather の Restore と同じです。操作の直前に、動かす Pane の居場所
(Workspace / Tab / Tab名 / Tabの並び順 / 隣接Pane と方向 / 復元順) を記録し、
Undo でそれを再生します。Merge のように元 Tab が空になって Herdr に閉じられた場合は、
**同じ名前・同じ位置で Tab を作り直します**。

Swap だけは記録が要りません。もう一度同じ2つを交換すれば元に戻るからです。

**1段階だけなのは意図的です。** 2つ前より過去の記録は、その後の操作で
すでに動かされた世界を前提にしているので、再生するとユーザーが望んでいない場所に
Pane が飛びます。「いま失敗した1手を取り消す」が実際に欲しい機能なので、そこに絞っています。

記録は state ディレクトリの `undo.json` に置かれ、一度使うと消えます。
Pane が閉じられていた場合はそのぶんを飛ばし、残りは戻したうえで正直に報告します。

---

---

内部でどう実現しているかは [INTERNALS.md](INTERNALS.md) にまとめています
（Cross-tab Swap の分解、Left / Up の変換、Merge のレイアウト保持、実行後の検証、
Gather と Undo の記録形式）。

---

## 範囲外

Drag & Drop / Grid・Equalize などの高度な Layout / 独自 Command Palette / 独自 Navigator /
Workspace テンプレート / Worktree 管理は本プラグインの責務外です (spec §24.2, §27)。
Layout は `herdr-layout-tools`、検索は `herdr-navigator`、Action 横断検索は
`herdr-command-palette` に委譲します。

---

## 開発

```bash
cargo test          # 設定パース・引数解析・Merge のレイアウト保持
(cd ../herdr-plugin-kit && cargo test)   # 表示名・split tree・fuzzy filter・描画幅
cargo build --release
```

構成:

```
src/
├── main.rs      CLI 入口 (launch / ui / headless / doctor)
├── config.rs    設定 (spec §20, 追加仕様 §6 §11 §12)
├── state.rs     Herdr から取り直すスナップショット (spec §3.4)
├── ops/         共通 Operation Pipeline (追加仕様 §13)
│   ├── plan.rs      検証して実行計画を作る (追加仕様 §7)
│   ├── apply.rs     実行と rollback (追加仕様 §9 §10)
│   └── verify.rs    実行後の検証 (追加仕様 §8)
└── ui/          Overlay とピッカー (spec §9, 追加仕様 §1–§5)

../herdr-plugin-kit/   4プラグイン共通の socket client / 表示名 / split tree / TUI
```

Context Menu・Keyboard・Overlay・Plugin Action はすべて `ops::execute` を通るため、
経路によって挙動が変わることはありません (spec §3.3, 追加仕様 §13)。

```text
Input → Resolve Source → Refresh Topology → Validate → Plan
      → Execute → Verify → Focus → Notify
```

## ライセンス

MIT
