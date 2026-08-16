# Herdr プラグイン ハンズオン

「困っている状況」を実際に組み立てて、そこから抜け出す操作を1回ずつ試すための練習環境です。

```
handson.sh     状況を組み立てる / 片付けるスクリプト
handson.html   7つのドリルと、各シナリオで効く設定
```

`handson.html` はブラウザで開くか、Artifact として公開したものを読んでください。

---

## 使い方

```bash
cd ~/src/herdr-plugins/handson

./handson.sh list          # シナリオ一覧
./handson.sh setup all     # 7つ全部を用意する
./handson.sh setup gather  # 1つだけ用意する
./handson.sh status        # 今なにがあるか
./handson.sh arm gather    # Agentの状態だけ入れ直す
./handson.sh order         # ドリル順に並べ直す
./handson.sh reset gather  # 作り直す
./handson.sh clean         # 片付ける
./handson.sh clean --force # よそのPaneごと閉じる（危険）
```

## 並び順

Workspace は `ハンズオン 1: gather` … `ハンズオン 7: navigator` という名前で作られ、
`workspace.move` で**実作業の Workspace の後ろにドリル順で**並べられます。
`reset` などで崩れたら `order` で並べ直せます。

## 安全性

`clean` が Workspace を閉じるのは、次の3つを全部満たすときだけです。

1. ラベルが **`ハンズオン`** で始まる
2. 中の Pane が**全部このスクリプトの作ったもの**（`.state/<名前>.panes` の台帳と照合）
3. **スクリプト自身が動いている Workspace ではない**（`$HERDR_WORKSPACE_ID` で判定）

2 は、練習中に自分の作業 Pane を持ち込んでしまったときに巻き添えで閉じないための防波堤です。
Swap や Move の練習では、Pane ピッカーに自分の Claude Code の Pane も出てきます。

3 は、`clean` が自分の足元の Workspace を閉じてシェルごと死に、
「なぜか途中までしか片付かない」状態になるのを防ぐためのものです。

条件を満たさない Workspace は理由付きでスキップされ、残りの片付けは続行します。
中身ごと閉じてよいと分かっているときだけ `clean --force` を使ってください
（それでも 3 だけは無視されません）。

`./handson.sh status` は、よその Pane が混ざっている Workspace を名指しで教えてくれます。

## シナリオ

| 名前 | 状況 | 練習する操作 |
|---|---|---|
| `gather` | 5 Agent が 3 Tab に散らばり、blocked 2 / working 2 / done 1 | Gather / Refresh / Restore |
| `move` | Tab が 6 つ、置き場所を間違えた Pane が 1 つ | Quick Move / Move picker |
| `extract` | 1 Tab に 4 Pane、1 つを独立させたい | Extract / New Workspace |
| `merge` | 内容の近い Tab が 2 つに分かれている | Merge（構造の保持） |
| `swap` | 大事な Pane が右下の狭い位置にいる | Swap |
| `layout` | 同一 Tab 内で幅がバラバラ・5 Pane | Equalize / Grid / Main Left / Zoom |
| `navigator` | Workspace 2 つ・Tab 8 つに紛れている | Navigator / Command Palette |

## `arm` が要る理由 — `done` はTabを開くと消える

Herdr の `done` は「終わったのに、**まだ見ていない**」という意味の状態です。
`working → idle` の遷移として作られ、**その Pane のある Tab を開いた瞬間に `idle` へ落ちます**。

実測:

```
[開始]              p3=done  p4=done
[migration を開く]  p3=idle  p4=done   ← 開いたTabの done だけ消える
```

時間経過では消えません（14秒放置しても `done` のまま）。引き金は Tab を開くことです。

このため `gather` シナリオは、**溶けない Agent を 4 体**（blocked 2 + working 2）置いてあります。
Tab を全部開いて回ったあとでも `prefix+m → g → 4` で 2×2 がきっちり埋まります。
`done` はその上に乗る 5 体目という位置づけです。

`arm` はレイアウトを一切触らずに、記録しておいた Agent 状態だけを報告し直します。

```bash
./handson.sh arm gather   # done が復活して 5 体に戻る
./handson.sh status       # 「Gatherが今すぐ拾う数」で実数を確認できる
```

状態の記録は `.state/<シナリオ>.agents` に置かれ、`clean` で消えます。

## 擬似 Agent について

実際に Codex や Claude を起動する代わりに、`herdr pane report-agent` で
Agent の状態を報告しています。Herdr から見ると本物の Agent と区別が付かないので、
Gather の優先度順やフィルタの挙動をそのまま確認できます。

`done` だけは直接報告できないので、`working` → `idle` の順に報告して
Herdr 側に遷移させています。
