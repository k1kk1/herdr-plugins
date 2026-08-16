# Herdr Pane Manager 仕様書

- 文書種別: プラグイン機能・UX・技術仕様
- 対象: Herdr
- 想定形態: Herdr Plugin
- ステータス: MVP Draft
- 目的: Herdr の Pane / Tab 再編成操作を、CLI の ID 指定なしで直感的かつ高速に行えるようにする

---

# 1. 概要

## 1.1 背景

Herdr では Pane を別 Tab へ移動したり、Tab をまたいでレイアウトを再構成したりできるが、複雑な操作では `pane_id` / `tab_id` を確認したうえで CLI を実行する必要がある。

例:

```bash
herdr pane list
herdr tab list
herdr pane move <pane_id> --tab <tab_id> --target-pane <pane_id> --split right
```

これは柔軟性が高い一方、日常的な Pane 整理としては操作コストが高い。

本プラグインでは、以下のような操作を Herdr UI から簡単に実行できるようにする。

- Pane を別 Tab へ移動する
- Pane を新規 Tab へ切り出す
- 別 Tab を含む Pane 同士を交換する
- 2つの Tab をマージする
- 右クリックだけでなくキーボード操作でも同じ処理を行う
- Command Palette 等の既存プラグインからも呼び出せる

---

# 2. プラグインの責務

## 2.1 中核責務

Pane Manager の責務は以下に限定する。

```text
既存 Pane / Tab の所属・配置関係を変更する
```

中核操作は4つ。

1. Move Pane
2. Swap Pane
3. Extract Pane
4. Merge Tab

## 2.2 非責務

以下は既存プラグインの責務と重複するため、原則として本プラグインには内包しない。

- Pane レイアウトプリセット
- Equalize / Grid / Main Left などの高度な Layout 操作
- Workspace / Tab / Pane を横断する汎用 Navigator
- 全 Plugin Action を横断する Command Palette
- Workspace テンプレート
- Git Worktree 自動構築
- Agent 起動フロー自動生成

## 2.3 既存プラグインとの棲み分け

| 領域 | 担当 |
|---|---|
| Pane を別 Tab へ移動 | Pane Manager |
| Pane 同士を交換 | Pane Manager |
| Pane を新規 Tab へ切り出し | Pane Manager |
| Tab をマージ | Pane Manager |
| 同一 Tab 内レイアウト調整 | Layout Tools 系プラグイン |
| Pane / Tab / Workspace 検索 | Navigator 系プラグイン |
| 全 Plugin Action 検索 | Command Palette 系プラグイン |
| Workspace 初期構築 | Workspace Manager 系プラグイン |

## 2.4 Optional Integration

既存プラグインは必須依存にしない。

例:

```text
Merge 完了
↓
Layout Tools が存在する
↓
[Equalize] アクションを追加表示
```

インストールされていない場合でも Move / Swap / Extract / Merge は完全に動作すること。

---

# 3. 設計原則

## 3.1 プロセスを停止しない

Pane 内で実行中の Codex / Claude / shell / server 等のプロセスを維持したまま再配置する。

```text
Codex: working
↓
Move to Tab 2
↓
Codex: working
```

Move / Swap / Merge / Extract のために Agent を再起動してはならない。

## 3.2 Herdr 内部 ID を通常 UI に露出しない

内部では以下を利用してよい。

```text
w1:p3
w1:t2
```

ただしユーザー向けには以下を優先して表示する。

1. Pane label
2. Agent 名
3. cwd basename / Project 名
4. terminal title
5. Tab label / number

ID は Debug / Details でのみ表示する。

## 3.3 操作経路を統一する

同じ操作を以下3経路から実行可能にする。

- Mouse / Context Menu
- Keyboard
- Plugin Action

内部ロジックは共通化する。

```text
Context Menu ─────┐
Keyboard ─────────┼→ Operation Action
Command Palette ──┘
```

## 3.4 状態は必要時に Herdr から取得する

独自キャッシュを真実の情報源にしない。

操作直前に Herdr から最新の以下を取得する。

- Workspace list
- Tab list
- Pane list
- Focused pane
- Agent state

ユーザーや Agent が外部から Pane を移動しても破綻しない設計とする。

---

# 4. 機能一覧

## 4.1 Move Pane to Tab

現在の Pane を既存 Tab へ移動する。

### 例

Before:

```text
Tab 1
└─ Claude

Tab 2
└─ Codex
```

Codex で:

```text
Move to Tab
→ Tab 1
```

After:

```text
Tab 1
├─ Claude
└─ Codex
```

### 配置方向

MVP では以下のみサポート。

- Right
- Down

デフォルト値は設定可能。

```text
Default Move Direction
- Right
- Down
- Ask Every Time
```

初期値:

```text
Right
```

### Quick Move

移動先 Tab の詳細な target pane を選ばず、現在のフォーカス Pane または Herdr が返す適切な target を利用して移動する。

### Advanced Move

必要な場合のみ target pane を明示指定できる。

```text
Move to...
└─ Tab 1: Agents
   ├─ Claude
   │  ├─ Right
   │  └─ Down
   └─ Codex
      ├─ Right
      └─ Down
```

---

# 5. Extract Pane to New Tab

現在 Pane を新しい Tab へ切り出す。

Before:

```text
Tab 1
├─ Claude
└─ Codex
```

操作:

```text
Extract to New Tab
```

After:

```text
Tab 1
└─ Claude

Tab 2
└─ Codex
```

## 5.1 新規 Tab 名

以下の優先順位で自動決定する。

1. Pane label
2. terminal title stripped
3. cwd basename
4. Agent 名
5. `Tab <number>`

例:

```text
Pane label = Music
→ New Tab label = Music
```

ユーザー設定で自動命名を無効にしてもよい。

---

# 6. Swap Pane

現在 Pane と指定した別 Pane を交換する。

同一 Tab / 別 Tab の両方を対象とする。

## 6.1 Cross-tab Swap

例:

Before:

```text
Tab 1
├─ Claude
└─ Codex A

Tab 2
└─ Codex B
```

操作:

```text
Codex A
→ Swap with Pane
→ Tab 2 / Codex B
```

After:

```text
Tab 1
├─ Claude
└─ Codex B

Tab 2
└─ Codex A
```

## 6.2 内部処理

Herdr が cross-tab swap を直接提供しない場合、複数回の move でエミュレートする。

概念:

```text
A → temporary destination
B → A destination
A → B destination
```

一時的に Tab を新設する実装も許可するが、ユーザーに見える不要な flicker や空 Tab は可能な限り避ける。

## 6.3 失敗時

Swap は複数操作になるため、途中失敗時に可能な範囲で rollback を試みる。

rollback できない場合は、最終状態を再取得して明示的なエラーを表示する。

---

# 7. Merge Tab Into

現在 Tab の Pane 群を別 Tab へ統合する。

例:

Before:

```text
Tab 1
├─ Claude
└─ Codex A

Tab 2
└─ Codex B
```

Tab 2 で:

```text
Merge Tab Into
→ Tab 1
```

After:

```text
Tab 1
├─ Claude
├─ Codex A
└─ Codex B
```

## 7.1 空 Tab

Merge により元 Tab が空になった場合、Herdr の通常仕様に従って閉じる。

必要ならプラグイン側で空 Tab の close を実行する。

## 7.2 レイアウト

MVP では高度なレイアウト保持を保証しない。

以下のみ指定可能。

- Right
- Down

複数 Pane の場合は順番を維持しつつ target Tab に追加する。

高度な整形は Layout Tools 等へ委譲する。

---

# 8. Context Menu UI

## 8.1 Pane Context Menu

Pane 右クリック時に以下を追加する。

```text
Move to Tab >
├─ Tab 1  Agents
├─ Tab 2  Server
├─ Tab 3  Shell
├──────────────
└─ New Tab...

Swap with Pane >
├─ Tab 1
│  ├─ Claude / mushi-battle
│  └─ Codex / ComposerSketch
└─ Tab 2
   └─ Codex / agent-usage

Extract to New Tab
```

必要に応じて既存 Herdr の Split / Close 等と共存する。

## 8.2 Tab Context Menu

Tab 右クリック時:

```text
Merge Into >
├─ Tab 1  Agents
├─ Tab 2  Server
└─ Tab 3  Shell
```

現在 Tab 自身は候補から除外する。

## 8.3 表示内容

Pane は以下の形式を基本とする。

```text
● Claude
  mushi-battle
  Stackchan⇔CoreS3 ESP-NOW無線通信対応
```

短い Menu では:

```text
Claude · mushi-battle
Codex · ComposerSketch
```

---

# 9. Keyboard UX

## 9.1 基本方針

個別のグローバルショートカットを大量に予約しない。

デフォルトでは Pane Manager 用の入口を1つだけ提供する。

推奨:

```text
prefix + m
```

## 9.2 Pane Manager Key Mode

```text
prefix + m
```

で Pane Manager モード / Popup を開く。

```text
Pane Manager

m  Move current pane
s  Swap current pane
 e Extract current pane to new tab
j  Merge current tab
1-9 Quick move current pane to tab
q  Cancel
```

※ 実際のキーは設定可能とする。

## 9.3 Move Picker

```text
prefix + m → m
```

表示:

```text
Move current pane to

1  Tab 1: Agents
2  Tab 2: Server
3  Tab 3: Shell
n  New Tab
Esc Cancel
```

選択後、必要なら:

```text
r  Right
d  Down
```

を選択する。

## 9.4 Quick Move

```text
prefix + m → 1..9
```

現在 Pane を指定 Tab 番号へ直接移動する。

配置方向は設定の default move direction を使用する。

例:

```text
Ctrl+B → m → 2
```

意味:

```text
現在 Pane を Tab 2 へ移動
```

## 9.5 Swap

```text
prefix + m → s
```

表示:

```text
Swap current pane with

Tab 1
1 Claude · mushi-battle
2 Codex  · ComposerSketch

Tab 2
3 Codex  · agent-usage
```

番号選択で Swap を実行。

## 9.6 Extract

```text
prefix + m → e
```

原則即時実行。

確認ダイアログは不要。

## 9.7 Merge

```text
prefix + m → j
```

表示:

```text
Merge current tab into

1 Tab 1: Agents
2 Tab 2: Server
3 Tab 3: Shell
```

必要に応じて Right / Down を選択する。

---

# 10. Direct Shortcuts

上級者向けに個別 Action への直接ショートカットを設定可能にする。

デフォルトでは無効。

設定例:

```toml
[pane-manager.keys]
open = "prefix+m"
move = "prefix+alt+m"
swap = "prefix+alt+s"
extract = "prefix+alt+e"
merge = "prefix+alt+j"
```

他プラグインや Herdr 本体との衝突を避けるため、個別キーはユーザー opt-in とする。

---

# 11. Plugin Actions

Command Palette や他プラグイン、自動化から呼べる Action を公開する。

最低限:

```text
pane-manager.open
pane-manager.move
pane-manager.swap
pane-manager.extract
pane-manager.merge-tab
```

追加候補:

```text
pane-manager.quick-move-1
pane-manager.quick-move-2
...
pane-manager.move-right
pane-manager.move-down
```

ただし Action 数を必要以上に増やさない。

---

# 12. Command Palette との連携

Pane Manager 自身では汎用 Command Palette を再実装しない。

既存 Command Palette 系プラグインから以下が見える状態を目指す。

```text
Pane Manager: Move Pane
Pane Manager: Swap Pane
Pane Manager: Extract Pane
Pane Manager: Merge Tab
```

Pane Manager の Action は単独でも呼べること。

---

# 13. Agent 情報表示

対象 Pane を選びやすくするため Herdr の Agent metadata を利用する。

表示候補:

```text
● working
! blocked
✓ done
○ idle
? unknown
```

例:

```text
Swap with...

! Claude · mushi-battle
  blocked

○ Codex · ComposerSketch
  idle

● Codex · agent-usage
  working
```

状態はあくまで識別補助であり、Move / Swap の可否を Agent state で制限しない。

---

# 14. Safety

## 14.1 確認不要操作

以下は原則確認なしで実行する。

- Move
- Swap
- Extract
- Merge

理由:

プロセスを停止しない非破壊操作として扱うため。

## 14.2 確認が必要な操作

将来 Close Pane / Close Tab を Pane Manager に追加する場合、Agent が `working` のときは確認を出す。

例:

```text
Claude is currently working.
Close this pane?
```

MVP では Close 操作自体を本プラグインの責務外としてよい。

---

# 15. エラー処理

## 15.1 対象が消えた場合

Picker 表示後に対象 Pane / Tab が消えた場合:

1. 最新 state を再取得
2. 操作を中止
3. 非破壊エラーを表示

```text
Destination tab no longer exists.
```

## 15.2 Move 失敗

```text
Could not move pane to Tab 2.
```

詳細表示では Herdr CLI / API の error を表示可能。

## 15.3 Swap 部分失敗

途中まで移動された可能性があるため:

1. rollback を試行
2. 最新状態を再取得
3. 最終結果を通知

例:

```text
Swap could not be completed.
Pane locations were refreshed.
```

---

# 16. Undo

## 16.1 Phase 2

MVP 後に Move / Swap / Extract / Merge の1段階 Undo を検討する。

操作前に以下を保持する。

- source workspace/tab/pane
- target workspace/tab/pane
- split direction
- relative placement

操作完了後:

```text
Moved "ComposerSketch / Codex" to Tab 1
[Undo]
```

Undo は Herdr state が大きく変化している場合は拒否してよい。

---

# 17. Optional Layout Tools Integration

Layout Tools 系プラグインが導入されている場合のみ、操作後の追加 Action を提示可能。

例:

```text
Merged Tab 2 into Tab 1

[Equalize]
```

この機能は optional integration とし、Pane Manager の必須依存にしない。

Pane Manager 内部で Grid / Main Left / Equalize を再実装しない。

---

# 18. Workspace 境界

## 18.1 MVP

同一 Workspace 内の Tab / Pane 操作のみサポートする。

理由:

- 操作モデルが単純
- 誤操作を減らせる
- 主要ユースケースを十分カバー

## 18.2 将来

Phase 3 で Workspace をまたぐ Move / Swap を検討する。

```text
Move to...

Workspace: ComposerSketch
├ Agents
├ Server
└ Shell

Workspace: mushi-battle
├ Agents
└ Build
```

Workspace 間操作では確認や cwd / environment 差異の扱いを別途検討する。

---

# 19. Drag & Drop

Phase 3 以降の候補。

Sidebar 上で:

```text
Tab 1
├ Claude
└ Codex

Tab 2
└ Shell
```

Codex を Tab 2 へ drag & drop して移動できるようにする。

ただし Herdr の UI extension API に深く依存する可能性があるため、MVP では採用しない。

右クリック + Keyboard で十分な UX を先に完成させる。

---

# 20. 設定項目

想定設定:

```toml
[pane-manager]
default_move_direction = "right" # right | down | ask
auto_name_new_tab = true
show_agent_state = true
show_terminal_title = true
confirm_merge = false

[pane-manager.keys]
open = "prefix+m"

# Optional direct shortcuts
# move = "prefix+alt+m"
# swap = "prefix+alt+s"
# extract = "prefix+alt+e"
# merge = "prefix+alt+j"
```

必要に応じて将来追加:

```toml
[pane-manager.integration]
layout_tools = "auto" # auto | off
```

---

# 21. 内部構成

```text
Pane Manager
│
├─ State
│  ├─ workspace list
│  ├─ tab list
│  ├─ pane list
│  ├─ focused pane
│  └─ agent metadata
│
├─ Operations
│  ├─ movePane()
│  ├─ swapPane()
│  ├─ extractPane()
│  └─ mergeTab()
│
├─ UI
│  ├─ paneContextMenu
│  ├─ tabContextMenu
│  ├─ movePicker
│  ├─ swapPicker
│  └─ mergePicker
│
├─ Keyboard
│  ├─ managerMode
│  ├─ quickMove
│  └─ directActions
│
├─ Integration
│  ├─ pluginActions
│  └─ optionalLayoutTools
│
└─ Error Handling
   ├─ refreshState
   ├─ rollbackSwap
   └─ notifyResult
```

---

# 22. Operation API 設計

UI と処理を分離する。

概念 API:

```text
movePane(sourcePane, destinationTab, targetPane?, direction)

swapPane(sourcePane, destinationPane)

extractPane(sourcePane, newTabLabel?)

mergeTab(sourceTab, destinationTab, direction)
```

Context Menu / Keyboard / Plugin Action はこれらを呼ぶだけとする。

---

# 23. UX シナリオ

## 23.1 Pane を Tab 1 へ移動

### Mouse

```text
Pane Right Click
→ Move to Tab
→ Tab 1
```

### Keyboard

```text
prefix+m → 1
```

### Command Palette

```text
Pane Manager: Move Pane
→ Tab 1
```

すべて同じ operation を実行する。

---

## 23.2 2つの Tab を左右マージ

Before:

```text
Tab 1
Claude

Tab 2
Codex
```

### Mouse

```text
Tab 2 Right Click
→ Merge Into
→ Tab 1
→ Right
```

### Keyboard

```text
prefix+m → j → 1 → r
```

After:

```text
Tab 1

Claude | Codex
```

---

## 23.3 右 Pane と次 Tab の Pane を交換

Before:

```text
Tab 1
Claude | Codex A

Tab 2
Codex B
```

Codex A をフォーカス。

### Mouse

```text
Right Click
→ Swap with Pane
→ Tab 2 / Codex B
```

### Keyboard

```text
prefix+m → s
→ Codex B を選択
```

After:

```text
Tab 1
Claude | Codex B

Tab 2
Codex A
```

---

## 23.4 Pane を一時的に独立

```text
prefix+m → e
```

または:

```text
Right Click
→ Extract to New Tab
```

すぐに新しい Tab として独立させる。

---

# 24. MVP

## 24.1 必須機能

- [ ] Herdr から Pane / Tab 一覧取得
- [ ] focused pane 取得
- [ ] Move Pane to existing Tab
- [ ] Move direction: Right / Down
- [ ] Extract Pane to New Tab
- [ ] Cross-tab Swap
- [ ] Merge Tab Into
- [ ] Pane Context Menu
- [ ] Tab Context Menu
- [ ] `prefix+m` Pane Manager entry
- [ ] Quick Move `prefix+m → 1..9`
- [ ] Keyboard Move Picker
- [ ] Keyboard Swap Picker
- [ ] Keyboard Merge Picker
- [ ] Plugin Actions 公開
- [ ] Agent / cwd / title を利用した識別表示
- [ ] stale state 検出と refresh
- [ ] 基本エラー表示

## 24.2 MVP 非対象

- [ ] Undo
- [ ] Drag & Drop
- [ ] Workspace 間 Move
- [ ] Workspace 間 Swap
- [ ] Grid Layout
- [ ] Equalize
- [ ] Main Left / Main Right
- [ ] 独自 Command Palette
- [ ] 独自 Pane Navigator
- [ ] Worktree 管理
- [ ] Workspace テンプレート

---

# 25. Phase 2

- Undo
- Recent destinations
- Favorite destination Tabs
- Optional Layout Tools integration
- 操作完了 Toast
- Custom direct shortcuts
- Merge 後 Action 提案
- Advanced target-pane picker

例:

```text
Recent Destinations
1 Agents
2 Server
3 Review
```

---

# 26. Phase 3

- Workspace 間 Move
- Workspace 間 Swap
- Drag & Drop
- 複数 Pane 選択
- 複数 Pane 一括 Move
- Tab group 操作
- より高度な rollback / transaction

---

# 27. 既存プラグインとの競合回避方針

Pane Manager は以下を再実装しない。

```text
Layout Tools
→ 同一 Tab 内レイアウト

Navigator
→ Herdr 全体検索 / 移動

Command Palette
→ Plugin Action 横断検索

Workspace Manager
→ Workspace / Worktree 初期構築
```

Pane Manager は以下に集中する。

```text
Move
Swap
Extract
Merge
```

これにより既存 ecosystem と競合するのではなく補完する。

---

# 28. 成功条件

MVP 完了時、以下が満たされること。

1. `pane_id` / `tab_id` を手入力せず Pane を別 Tab へ移動できる
2. Pane 右クリックから 2〜3操作以内で Move できる
3. `prefix+m → <tab number>` で Quick Move できる
4. 別 Tab の Pane 同士をユーザーから見て1操作として Swap できる
5. 2つの Tab を UI / Keyboard から Merge できる
6. 実行中 Codex / Claude を停止しない
7. Context Menu / Keyboard / Plugin Action で挙動が一致する
8. Layout / Navigator / Command Palette 系既存プラグインと責務が重複しない
9. Pane Manager 単体インストールだけで中核4操作がすべて利用できる

---

# 29. 推奨初期ショートカット

```text
Herdr Prefix
Ctrl+B

Pane Manager
Ctrl+B → m

Inside Pane Manager
m     Move picker
s     Swap picker
e     Extract to new tab
j     Merge current tab
1..9  Quick move current pane to tab
q     Cancel
Esc   Cancel
```

既存 Herdr 操作との思想も揃える。

```text
Ctrl+B → h/j/k/l
現在 Tab 内の Pane 移動

Ctrl+B → Shift+H/J/K/L
現在 Tab 内の Pane 入れ替え

Ctrl+B → m
Tab をまたぐ Pane Manager 操作
```

この役割分担により覚えやすくする。

---

# 30. 最終コンセプト

Pane Manager は Herdr のレイアウトエディタではない。

**「現在動いている Pane を、止めずに別の場所へ移籍・交換・統合するための操作レイヤー」** とする。

初心者には:

```text
右クリック
→ Move / Swap / Extract / Merge
```

慣れたユーザーには:

```text
prefix+m → 1
```

他プラグイン・自動化には:

```text
pane-manager.move
pane-manager.swap
pane-manager.extract
pane-manager.merge-tab
```

を提供する。

これにより、Herdr CLI の柔軟性を維持しながら、tmux の `join-pane / break-pane / swap-pane` に相当する操作をより直感的に行えるようにする。
