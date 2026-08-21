# Herdr socket API の実測メモ

Herdr 0.8.0 の plugin API には公開ドキュメントがないので、バイナリの文字列・同梱スキーマ
(`herdr api schema --json`)・使い捨て Workspace での実験から挙動を確かめました。
ここはその記録です。**踏むと痛い罠**を中心にまとめています。

---

## 接続

`$HERDR_SOCKET_PATH`（既定 `~/.config/herdr/herdr.sock`）の Unix socket に、
改行区切り JSON を送ります。

```json
{"id":"1","method":"pane.list","params":{"workspace_id":"w1"}}
```

### 1接続 = 1リクエスト

**サーバは1つ応答すると接続を閉じます。** 同じ接続で2回目を送ると broken pipe になります。
`herdr-plugin-kit` の `Herdr` は呼び出しごとに接続し直します。

```rust
fn dial(&self) -> Result<UnixStream>   // 呼び出しのたびに接続
```

`herdr` CLI を毎回起動する代わりに socket を直接使うのは、CLI が公開していない
`layout.export` / `layout.set_split_ratio` / `tab.move` / `workspace.move` に届くからです。

---

## Pane を動かす

### 安全なのは `pane.move` と `layout.set_split_ratio` だけ

この2つは `pane_id` を保持するので、中のプロセスが生き残ります。
プラグインの Pane 再配置は全てこの2つの組み合わせです。

```json
{"pane_id":"w1:p3",
 "destination":{"type":"tab","tab_id":"w1:t2","target_pane_id":"w1:p1","split":"right","ratio":0.6},
 "focus":false}
```

`destination` は `{"type":"tab"}` / `{"type":"new_tab","label":…}` /
`{"type":"new_workspace","label":…,"tab_label":…}` の3種類です。

### `layout.apply` は破壊的

名前に反して **Tab ごと作り直します。** 実測では `w6:t1` が閉じられ、
`w6:t2` が brand-new な `p5`–`p8` を持って現れ、元の Pane のプロセスは全て死にました。
**一切使っていません。**

### 同一 Tab 内への move は no-op

`pane.move` で移動先に「その Pane が今いる Tab」を指定しても何も起きません。
つまり **Tab の中身をその場で組み替えることはできません。**

Layout Tools の Grid / Equalize と Gather の再構成は、この制約を
**一度退避用 Tab へ出して、順番に戻す**ことで回避しています。

```
park → 退避用 Tab へ全部出す → 目的の順で split しながら戻す
```

### Tab は最後の Pane が出ると自動的に閉じる

退避すると元の Tab は消えます。戻すときに Tab が無ければ
**元の名前・元の並び順で作り直す**必要があります（`tab.move` で位置を復元）。

Gather の Refresh で `Active Agents` Tab を使い回すときは、
**先頭の Pane を1つ残したまま**他を退避させ、Tab が空にならないようにしています。

### `pane.swap` は同一 Tab 内のみ

別 Tab 同士だと `changed:false, reason:"cross_tab"` を返します。
Pane Manager の Cross-tab Swap は `pane.move` を4通りに組み合わせて実現しています
（[../herdr-pane-manager/INTERNALS.md](../herdr-pane-manager/INTERNALS.md)）。

### 分割方向は right と down だけ

`split` に取れるのは `"right"` と `"down"` です。
Left / Up は **右|下に分割してから swap** して作ります。ユーザーには4方向に見せています。

---

## 分割ツリー

`layout.export` が Tab の split tree を返します。

```json
{"type":"split","direction":"right","ratio":0.5,
 "first":{"type":"pane","pane_id":"w1:pS"},
 "second":{"type":"pane","pane_id":"w1:pQ"}}
```

`layout.set_split_ratio` は root からの経路で1つの split を指します
（`path: [false,true]` = first → second）。`ratio` は **first 側の取り分**です。

### 分割の順序が形を決める

ある Pane が列をまたいで伸びるべきなら、**その列が細分化される前に**分割しておく必要があります。
順序を間違えると、`(r (d p1 p2) (d p3 p4))` を作りたいのに
`(d (r p1 (d p3 p4)) p2)` になります。

`herdr-plugin-kit/src/layout.rs` がこの往復を担当します。

- `Plan::simulate()` — 分割手順 → 木
- `Plan::from_shape()` — 木 → 分割手順

両方向をテストで固定してあるので、レイアウトの崩れは画面ではなく `cargo test` で落ちます。

---

## Agent の状態

`agent.list` が返す `agent_status` は5値です。

```
idle / working / blocked / done / unknown
```

### `done` は報告できない

`pane.report_agent` が受け付けるのは **4値だけ**（`done` を送ると
`unknown variant "done"` で拒否されます）。

`done` は Herdr が内部で作る状態で、**`working` → `idle` の遷移**として発生します。

```bash
herdr pane report-agent PANE --source x --agent claude --state working
herdr pane report-agent PANE --source x --agent claude --state idle
# → agent.list では done になる
```

### `done` は Tab を開くと消える

`done` は「終わったのに **まだ見ていない**」という意味なので、
**その Pane のある Tab を focus した瞬間に `idle` へ落ちます。**

```
[開始]              p3=done  p4=done
[migration を開く]  p3=idle  p4=done   ← 開いた Tab の done だけ消えた
```

時間経過では消えません（14秒放置しても `done` のまま）。引き金は Tab を開くことだけです。
**`done` を前提にした挙動を設計してはいけません。**

### 並び順の安定化

`state_change_seq` が単調増加するので、同一状態内の並びに使えます。
Gather は `status.priority()` → `state_change_seq` 降順 → `pane_id` の順でソートし、
状態が変わらなければ2回走らせても同じ配置になるようにしています。

---

## プラグインのマニフェスト

### Action ID にドットは使えない

仕様書の `pane-manager.move` という綴りは `invalid_plugin_action_id` で拒否されます。
コロンは通りますが、plugin id が既に名前空間なので **ドット無しの短い id** にしています。

```
✗ pane-manager.move
✓ move          （herdr plugin action invoke move --plugin pane-manager）
```

### 新規 Tab は `--label`、新規 Workspace は `--tab-label`

`pane move --new-tab --tab-label X` は拒否されます。`--label` が正解です。
`--tab-label` は `--new-workspace` のときに、その中の Tab を名付けるためのものです。

---

## Pane に入る環境変数

```
HERDR_SOCKET_PATH  HERDR_CLIENT_SOCKET_PATH  HERDR_SESSION  HERDR_BIN_PATH
HERDR_ENV  HERDR_WORKSPACE_ID  HERDR_TAB_ID  HERDR_PANE_ID
```

### `HERDR_WORKSPACE_ID` / `HERDR_TAB_ID` は古くなる

Pane が作られた時点の値で固定され、**Pane が別の Tab / Workspace へ移動しても更新されません。**
`HERDR_PANE_ID` は移動しても変わらないので、現在地が要るときは
そちらを起点に `pane.get` で引き直します。

```bash
herdr pane get "$HERDR_PANE_ID"   # ここの workspace_id が本当の現在地
```

プラグイン側の起動コンテキストは `HERDR_PLUGIN_ROOT` / `HERDR_PLUGIN_CONFIG_DIR` /
`HERDR_PLUGIN_STATE_DIR` / `HERDR_PLUGIN_CONTEXT_JSON` / `HERDR_ACTIVE_PANE_ID` で渡ってきます。

---

## セッション

Herdr のセッションはそれぞれ**独立したサーバ**で、独立した socket を持ちます。
名前付きセッションは `~/.config/herdr/sessions/<name>/` に置かれます。

### socket からは他のセッションが見えない

**`session.list` は存在しません。** あるのは `session.snapshot` だけで、
これは今つないでいるセッションしか答えません。

一覧を取るには `herdr session list --json` を使います（CLI がディスクを読みます）。

```json
{"sessions":[{"default":true,"name":"default","running":true,
  "session_dir":"/Users/…/.config/herdr",
  "socket_path":"/Users/…/.config/herdr/herdr.sock"}]}
```

他のセッションの中身を知りたければ、**その `socket_path` に直接つなぎます。**
`session.snapshot` は `workspaces` / `tabs` / `panes` / `agents` を
**1往復でまとめて**返すので、要約1件につき接続1回で済みます。

停止中のセッションには socket がないので、`<session_dir>/session.json` を読みます。
これは Herdr の内部フォーマット（現在 `"version": 3`）です。

### 入れ子起動は既定で禁止

Pane の中で `herdr` を起動すると拒否されます。

```
error: nested herdr is disabled by default.
```

`[experimental] allow_nested = true` で解除できますが、既定は off です。
**Pane の中に別のセッションを開くことはできません。**

### macOS の `open` は環境変数を引き継ぐ

`open -na Ghostty.app --args -e …` は **呼び出し元の環境をアプリに渡します。**
Pane から実行すると、新しいトップレベルのウィンドウにも `HERDR_ENV=1` が付いてきて、
上の nested 判定で弾かれます。

```bash
# 弾かれる
open -na Ghostty.app --args -e herdr session attach probe

# 通る
env -u HERDR_ENV -u HERDR_PANE_ID -u HERDR_SOCKET_PATH \
  open -na Ghostty.app --args -e herdr session attach probe
```

`HERDR_` で始まる変数をすべて落とすのが安全です。古い `HERDR_PANE_ID` /
`HERDR_SOCKET_PATH` が残ると、新しいセッションの Pane が**元のサーバ**を向きます。

### Agent の session id は内部にしか無い

`pane.report_agent_session` は `agent_session_id` / `agent_session_path` を
受け取りますが、**`pane.get` にも `agent.list` にも出てきません。**
イベント (`events.subscribe`) の `AgentSessionInfo` にだけ現れます。

Pane と Claude Code / Codex のトランスクリプトを突き合わせたいときは、
`cwd` と `terminal_title_stripped` の一致で推定するしかありません。

### `agent.start` で任意の引数を渡せる

```json
{"pane_id":"w1:p3","name":"claude","kind":"claude","args":["--resume","<uuid>"]}
```

`args` はそのまま argv になるので、`claude --resume <id>` / `codex resume <id>` に届きます。
`tab.create` / `pane.split` / `workspace.create` はどれも `root_pane`（split は `pane`）を
返すので、**作る → 撃つ**の2手で再開できます。

**`cwd` を必ず渡します。** 会話の作業ディレクトリ以外で起動すると、Agent が
「このフォルダを信頼しますか？」を出して止まります。

### GUI から起動されたプロセスに Homebrew の PATH は無い

`open` 経由のターミナルや Alfred のワークフローには `/opt/homebrew/bin` が
入っていないことがあります。**`herdr` は絶対パスに解決してから渡します。**

---

## CLI に無い API

`herdr` CLI のサブコマンドに出ていなくても socket にはあります。

| メソッド | 用途 |
|---|---|
| `layout.export` | Tab の split tree を読む |
| `layout.set_split_ratio` | 分割比を変える（Equalize の実体） |
| `tab.move` | Tab を並び替える（Restore で位置を戻す） |
| `workspace.move` | Workspace を並び替える |

`workspace.move` は `{"workspace_id":…,"insert_index":N}`、
`tab.move` は `{"tab_id":…,"insert_index":N}` です。どちらも 0 始まり。
