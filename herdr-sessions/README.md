# Herdr Sessions

起動中・停止中のすべての Herdr セッションを一覧し、選んだものを新しいターミナル
ウィンドウで開きます。Alfred からも同じ一覧を引けます。

さらに **Claude Code / Codex の過去の会話**も一覧して、その場で再開できます。

他の4つのプラグインが **1つのセッションの中**を扱うのに対し、これだけが
**セッションの外**を見ます。

```
Open a session                              Opens in a new terminal window · Tab to manage instead
> ▏                                                                                             2

▸ 1  ● default   running · default · you are here
                 2 workspaces · 4 panes · 3 agents, 1 busy  —  Agent Recipes · dotfiles
  2  ○ scratch   stopped · 3 days ago
                 1 workspace · 2 panes  —  herdr-plugins

type to filter · 1-9 pick · ↑↓ move · Enter choose · Tab mode · Esc cancel
```

停止中のセッションも、**中身の要約付きで**出ます。開いたら何が戻ってくるかが
名前だけより分かるようにするためです。

| 記号 | 意味 |
|---|---|
| `●` 緑 | 起動中。working / blocked の Agent がいる |
| `●` シアン | 起動中 |
| `○` 灰 | 停止中 |

---

## 使い方

| 操作 | 動作 |
|---|---|
| `prefix+shift+s` | Herdr セッションのピッカー |
| `prefix+a` | 過去の会話のピッカー |
| 入力 | 絞り込み。**セッション名だけでなく Workspace 名にも当たります** |
| `1`–`9` / `↑↓` | 選択 |
| `Enter` | 新しいウィンドウで開く |
| `Tab` | **同じ一覧の中で絞り込む。** 会話なら All → Claude → Codex |
| `Shift+Tab` | **一覧そのものを切替。** Herdr セッション ⇄ 会話 |
| `Esc` | 閉じる |

Manage 側では、起動中のセッションを **Stop**（レイアウトは保存され、次の attach で
戻ります）、停止中のセッションを **Delete**（保存レイアウトごと破棄）できます。
どちらも y/n の確認が入ります。

### コマンド

```bash
herdr-sessions list                      # Herdr セッション一覧
herdr-sessions open <name>               # 新しいウィンドウで開く
herdr-sessions recent [all|claude|codex]  # 過去の会話一覧（既定 all）
herdr-sessions resume <id>               # 会話を再開（ツール名は省略可）
herdr-sessions alfred                    # Alfred Script Filter JSON
herdr-sessions alfred install            # Alfred ワークフローを導入
```

---

## 過去の Claude Code / Codex セッション

`Shift+Tab` で会話一覧に入ると、**Claude Code と Codex がまとめて**新しい順に並びます。
`Tab` で `all → claude → codex` と絞り込めます。

```
Resume a conversation
Resumes here, not in a new window · Shift+Tab for herdr sessions
 Tab ▸ ▊all▊ claude  codex
> ▏                                                                             91

▸ 1  Herdr pane manager プラグインをRustで制作  just now                  (Claude)
        ~/src/herdr-plugins · main  —  allの時は右寄せでCodex等を表示…
  2  Stackchan・ムシバトルの無線連携を実装してください  1 hour ago           (Codex)
        ~/src/mushi-battle
  3  Agent skill snippet アプリの開発  3 hours ago · looks open            (Claude)
        ~/src/agent-skill-snippet · main  —  悪くなさそうです ただ後ろの…
                                                                       ↓ 88 more

type to filter · 1-9 pick · ↑↓ move · Enter new workspace · Shift+Enter new tab · Opt+Enter split
```

### どの会話がどの LLM か

行の**右端の列**に `(Claude)` / `(Codex)` が出ます。

見出しの前ではなく後ろなのは、**見出しが左端で揃う**からです。前に置くと全行が
9桁ずつ押し出されたうえ、繰り返される同じ語が「目が最初に着く場所」を占めます。
読みたいのは見出しの方で、ツール名は**見つかればいい**だけです。

CJK が混ざっても桁はずれません（`char_width` で幅を数えてから右詰めしています）。
本文より先に右列の幅を確保するので、**タイトルが長い行でツール名が消えることもありません。**

**All のときだけ**付きます。Claude だけの一覧で全行に `(Claude)` が並んでも意味がないので。

絞り込みは `claude` / `codex` にも当たるので、打って絞っても同じことができます。

### Tab の現在地

`Tab ▸ ▊all▊ claude codex` という帯が見出しの下に出ます。
**選択中のものだけ反転**するので、押す前にどこにいるか分かります。

以前は副題に `Tab [all] claude codex` と文章で書いていましたが、
全部同じ灰色で埋もれていました。**キーが何を切り替えるかは、書くのではなく見せるべき**でした。

### なぜ Tab と Shift+Tab に分けたか

「何を並べるか」と「そのうちどれに絞るか」は**別の軸**です。1つの Tab で全部を
巡回させると、会話一覧でツールを絞ろうとして Tab を押し続けたときに
**Herdr のセッション一覧に着いてしまいます。** それは絞り込みではありません。

- `Tab` — いま見ているものの**中で**facet を変える
- `Shift+Tab` — **見ているものそのもの**を変える

見出しの下に `Tab [all] claude codex · Shift+Tab herdr sessions` と現在地を出しているので、
押さなくても何が起きるか分かります。Shift+Tab は常にもう一方の**一番広い**ビューへ行きます
（前回どこにいたかを覚えない）ので、いつ押しても同じ動きになります。

`Shift+Tab` は Shift+Enter と違い、**どの端末でも独立したコードで届く**ので
キーボードプロトコルの交渉は不要です。

**新しいウィンドウは開きません。** `claude --resume` も `codex resume` もただのプログラムなので、
`herdr session attach` と違って**今いるセッションの中で**動きます。
Pane を作って `agent.start` を撃つだけです。

### Enter の使い分け

会話にどれだけ場所を与えるかを、Enter の修飾で選びます。

| キー | 置き場所 |
|---|---|
| `Enter` | **新しい Workspace**（会話名が付きます） |
| `Shift+Enter` | 今の Workspace に**新しい Tab** |
| `Opt+Enter` | 今の Pane を**分割** |

`Shift+Enter` は Kitty キーボードプロトコルに対応したターミナルでしか
`Enter` と区別できません。**区別できない環境ではフッターに出しません** —
押しても届かないキーを案内するのは嘘なので。Herdr の Pane 端末は対応しています。

修飾なしの `Enter` の意味は `resume_in` で変えられます。

### 一覧に出る情報

| 項目 | Claude Code | Codex |
|---|---|---|
| 行の右端 | `(Claude)` | `(Codex)` |
| 見出し | `ai-title`（Claude 自身が付けた名前） | **無いので**最初のプロンプト |
| 場所 | `cwd` · `gitBranch` | `cwd` |
| 2行目 | 最後のプロンプト | — |
| 絞り込み | 見出し・パス・ブランチ・プロンプト本文・ツール名 | 同左 |

`● looks open` は「この会話がもう画面に出ていそう」という印です。
**断定していないのには理由があります**（下記）。

### 既定で全件

件数の上限はありません。この環境での実測です。

| | 件数 | 時間 |
|---|---|---|
| Claude Code | 31 | 20ms |
| Codex | 60 | 60ms |
| All | 91 | 80ms |

トランスクリプトは合計 515MB ありますが、**読むのは各ファイルの先頭か末尾だけ**なので、
全部並べても一瞬です。29日前の会話まで遡れます。

遅くなってきたら上限を設定できます。

```toml
[sessions]
recent = 50
```

これは**取得件数の上限でもあります。** 更新時刻で並べ替えてから、
残ったものだけを読むので、50 なら50回の読み取りで済みます。

---

## なぜ「新しいウィンドウ」なのか

`herdr session attach` は実行したターミナルを乗っ取ります。そして Herdr は
**自分の Pane の中で自分を起動することを既定で拒否します。**

```
error: nested herdr is disabled by default.
```

`[experimental] allow_nested` を有効にすれば通りますが、入れ子のセッションは
外側の prefix キーを食うので既定が off なのは妥当です。
つまり **今いるセッションの中に別のセッションを開くことはできません。**
手でやる場合と同じく、外側のターミナルの新しいウィンドウで開きます。

Alfred から呼ぶときはそもそも Herdr の Pane がないので、答えは同じです。
プラグインと Alfred が [`src/open.rs`](src/open.rs) を共有しているのはそのためです。

### 環境変数を落とす必要がある

macOS の `open -na` は **呼び出し元の環境をアプリに渡します。** そのため Herdr の
Pane から開いた新しいウィンドウにも `HERDR_ENV=1` が付いてきて、真新しい
トップレベルのターミナルなのに nested 判定で弾かれます。

`open.rs` の `disinherit()` が `HERDR_` で始まる変数をすべて外しています。
ついでに古い `HERDR_PANE_ID` / `HERDR_SOCKET_PATH` も落ちるので、
新しいセッションの Pane が **こちらのサーバ**を向いてしまう事故も防げます。

---

## 設定

```
herdr plugin config-dir sessions
```

既定では `$TERM_PROGRAM` から開き方を決め、見つからなければ `/Applications` に
あるものを使います。それ以外のターミナルは `command` を書いてください。

```toml
[sessions]
command = ["open", "-na", "WezTerm.app", "--args", "start", "--",
           "{herdr}", "session", "attach", "{session}"]
```

`{session}` はセッション名、`{herdr}` は Herdr バイナリに置換されます。
**シェルを通さず argv として起動する**ので、名前に含まれるメタ文字は無害です。

既定で対応しているのは Ghostty と WezTerm です。iTerm2 と Terminal.app は
AppleScript 経由でしかコマンドを渡せないので、推測せずエラーで案内します
（[config.example.toml](config.example.toml) に例があります）。

---

## Alfred 連携

```bash
herdr-sessions alfred install
```

Alfred の workflows フォルダに Script Filter → Run Script のワークフローを書き、
Alfred が再起動なしで拾います。キーワードは2つです。

| キーワード | 一覧 | Enter |
|---|---|---|
| `hs` | Herdr セッション | 新しいターミナルウィンドウで開く |
| `hr` | Claude Code / Codex の会話 | **動いている Herdr の中で再開**し、ターミナルを前面に出す |

`hr` が新しいウィンドウを開かないのは、そこが作業場所だからです。
Herdr が動いていなければ入れる先がないので、そのときだけウィンドウを開きます。

**修飾キーもピッカーと同じです。** 同じ操作が Herdr の中でも Alfred でも
同じ意味になるように。

| キー | 置き場所 |
|---|---|
| `Enter` | 新しい Workspace |
| `Shift+Enter` | 新しい Tab |
| `Opt+Enter` | 直前にいた Pane の隣に分割 |

Alfred の修飾キーは**引数の文字列しか変えられない**ので、置き場所を
`tab:<id>` のように前置きして渡しています。加えて、修飾キーごとに
**接続を張らないと何も起きません**（`mods` は引数を決めるだけで、
行き先を決めるのは接続の方）。

### `scriptargtype` は 1 が argv

Run Script が引数を受け取れるかは、この数字ひとつで決まります。**読みが逆で、
`1` が「input as argv」、`0` はそうではありません。**

`0` にすると、スクリプトは `/bin/bash` として起動し、**位置パラメータが1つも
渡ってきません。** `{query}` の置換も起きないので、項目を Enter で選んでも
黙って何も起きません。Alfred はログを残さないので、外からは原因が見えません。

`1` にすると `$0` が Alfred のキャッシュしたスクリプトのパス、`$1` が選んだ
項目の引数になります。

```
argv=[~/Library/Caches/…/Workflow Scripts/F4B41383-…][tab:ea7133d5-…]
```

両方とも実測しました。**当てずっぽうで2往復むだにしたので**、ここに残します。

### アイコン

各行にはインストール済みアプリのアイコンを Alfred の `fileicon` で借ります。
**画像を同梱しないので、アプリが更新されれば絵柄も追従します。**

| 行 | 借りる先 |
|---|---|
| Claude の会話 | `Claude.app` |
| Codex の会話 | `Codex.app` → `ChatGPT.app` |
| Herdr セッション | 設定の `command` が開くターミナル（既定 `Ghostty.app`） |

Claude Code も Codex もターミナルのプログラムで自前のバンドルを持たないので、
デスクトップ版を借りています。`Codex.app` を先に見るのは、将来それが出たときに
自動で切り替わるようにです。**どれも無ければアイコンを付けません** —
間違ったアイコンを出すより、無い方がましなので。

導入済みのものはフォルダ名ではなく **bundle id で見つけて**上書きを拒否します
（キーワードを変えていることがあるため）。上書きするなら `--force`。

削除は Alfred の Preferences → Workflows から。

### パスを焼き込む理由

Alfred はワークフローのスクリプトを、良くてログインシェルの `PATH`、悪くて
`/usr/bin:/bin:/usr/sbin:/sbin` で走らせます。Homebrew の `bin` はどちらにも
入っていないことがあります。そのため生成されるワークフローには
**このバイナリと `herdr` の絶対パスが焼き込まれます。**

プラグインを別の場所へ移したら `alfred install --force` で焼き直してください。

一覧の絞り込みは Alfred 自身にやらせています（`alfredfiltersresults`）。
各項目の `match` に Workspace 名を入れてあるので、
**セッション名を忘れていても中身の名前で引けます。**

---

## 実装メモ

### 一覧は socket から取れない

セッションはそれぞれ独立したサーバで、**socket API に `session.list` はありません。**
あるのは `session.snapshot` だけで、これは今つないでいるセッションしか答えません。

なので一覧は `herdr session list --json`（ディスクを読む CLI）から取り、
要約はそのあと **各セッションの socket に個別につないで**集めています。
`herdr-plugin-kit` に `Herdr::at(path)` を足したのはこのためです。

### 再開できないトランスクリプトが混ざっている

上限をなくした時点で問題になりました。10件で切っていたときは
**ノイズが上位に来なかっただけ**です。

- Claude は **subagent** の記録を `projects/<slug>/<id>/subagents/agent-*.jsonl`
  に置きます。`claude --resume` では開けません。本物のセッションは
  プロジェクト直下の1階層目にあるので、**深さ**で判定しています
  （`subagents` という語に依存しないためです）。
- Codex は `rollout-` で始まるファイルだけがセッションです。

この環境では 34 → 31 件になりました。

### 会話の一覧も socket からは取れない

`claude --resume` も `codex resume` も**自前の TUI ピッカーを開くだけ**で、
機械可読な一覧を出すコマンドがありません。なのでディスク上のトランスクリプトが
唯一の情報源です。どちらも非公開フォーマットなので、緩く読んでいます。

| | 置き場所 | 読み方 |
|---|---|---|
| Claude Code | `~/.claude/projects/<slug>/<uuid>.jsonl` | **末尾 64KB** |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | 先頭から最大400行 |

Claude は `ai-title` と `last-prompt` を会話の進行に合わせて**追記していく**ので、
末尾に最新版があります。この文章を書いているセッションのファイルは 13MB ありますが、
**末尾 64KB だけでタイトル・最後のプロンプト・cwd・ブランチが全部取れました。**
一覧全体で 20ms です。

Codex は逆に1行目の `session_meta` に全部入っています。ただしタイトルが無いので、
最初のユーザー発言を探しに行きます。**バージョンによって記録形式が2種類あり**
（`event_msg`/`user_message` と `response_item`/`message`）、両方が同じディスクに
共存していたので両対応しています。Codex が自分に食わせる
`<environment_context>` と `AGENTS.md` は発言ではないので飛ばします。

### 「開いている」と断定できない

Herdr は Agent の session id を**内部では持っています**
（`pane.report_agent_session` にパラメータがあります）が、
**`pane.get` にも `agent.list` にも出てきません。** 突き合わせる id が無いので、
`cwd` と タイトルの両方が一致したときだけ印を付けています。
UI が `looks open` と濁しているのはそのためです。

### エージェント名は一意でなければならない

`agent.start` の `name` は Herdr 内で一意です。2つ目を同じ名前で起動すると
`agent_name_taken` で失敗します。名前の規則も厳しく、**小文字・数字・`-`・`_`
のみ、1〜32文字、先頭は小文字**でなければなりません（`w2B:p3` のような Pane ID
はコロンと大文字の両方で弾かれます）。

3段階で試しています。読みやすい順であり、確実に空いている順の逆です。

```
claude              単独ならこれ
claude-3ebd1a0d     2つ目の会話を再開した時点で埋まる
claude-w2b-p3       Pane ID 由来。同じ会話を3回開いても衝突しない
```

### 作った Pane にすぐエージェントを入れられない

`pane.split` / `tab.create` は Pane ができた時点で返りますが、**中のシェルは
まだ対話可能ではありません。** その状態で `agent.start` すると
`agent_pane_busy` で拒否されます。

実測では `/tmp` に開いた Pane は即座に使えるのに対し、プロジェクトのディレクトリ
（シェルが starship・git・direnv を走らせる）では **約200ms** かかりました。
`agent_pane_busy` の間は最大5秒までポーリングしています。

失敗したときは**作った Pane を閉じます。** ユーザーが求めたのは会話であって
Pane ではないので、空のシェルを残すのは違います。

### 再開先の cwd を必ず渡す

`tab.create` / `pane.split` の `cwd` に会話の作業ディレクトリを指定しないと、
Agent が無関係な場所で起動して、トランスクリプトの代わりに
**「このフォルダを信頼しますか？」を出して止まります。** 実測で踏みました。

### 起動中と停止中で情報源が違う

| 状態 | 情報源 | 取得内容 |
|---|---|---|
| 起動中 | そのセッションの socket に `session.snapshot` **1往復** | Workspace / Pane / Agent 数、busy 数、Workspace 名 |
| 停止中 | `<session_dir>/session.json` | Workspace / Pane 数、Workspace 名、最終更新時刻 |

`session.snapshot` が1往復で全部返すので、10セッションでも接続は10回で済みます。

停止中のほうは Herdr の内部フォーマット（現在 `"version": 3`）なので、
型を付けず `serde_json::Value` で読んでいます。
**バージョンが変わったときに落ちるより、行が少し薄くなるほうがましだからです。**
読めなければ行にその旨を出し、一覧全体は止めません。

### `session.json` の mtime を「最終使用」として使う

Herdr はセッションが変わるたびにこのファイルを書き直すので、
停止中のセッションが最後に生きていた時刻の近似になります。
