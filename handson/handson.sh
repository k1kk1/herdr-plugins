#!/usr/bin/env bash
#
# Herdr プラグイン ハンズオン用の練習環境を組み立てるスクリプト。
#
#   ./handson.sh list              シナリオ一覧
#   ./handson.sh setup gather      1つ用意する
#   ./handson.sh setup all         全部用意する
#   ./handson.sh arm gather        Agentの状態だけ入れ直す（レイアウトは触らない）
#   ./handson.sh reset gather      作り直す
#   ./handson.sh order             ドリル順に並べ直す
#   ./handson.sh status            今どうなっているか
#   ./handson.sh clean             ハンズオン用Workspaceを片付ける
#   ./handson.sh clean --force     よそのPaneごと閉じる（危険）
#
# 安全装置:
#   clean が閉じるのは「ラベルが ハンズオン で始まり、かつ中のPaneが全部
#   このスクリプトの作ったものである」Workspaceだけです。練習中に自分の作業Pane
#   （Claude Code が動いているPaneなど）を持ち込んでしまったWorkspaceは、
#   中身を巻き添えにしないためスキップして警告を出します。
#
# done は「見られたら消える」状態なので、Workspaceを一度表示すると idle に戻ります。
# 練習をやり直すときは arm を打てば、レイアウトはそのままで状態だけ戻ります。

set -uo pipefail

PREFIX="ハンズオン"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$SCRIPT_DIR/.state"
SOCK="${HERDR_SOCKET_PATH:-$HOME/.config/herdr/herdr.sock}"

# setup 中のシナリオ名。agent() がこれを見て状態を記録する。
CURRENT=""

# 自分がどこで動いているか。clean が自分の居る Workspace を閉じると
# 途中でシェルごと死んで「なぜか全部片付かない」ことになるので、ここは絶対に閉じない。
#
# HERDR_WORKSPACE_ID は Pane が作られた時点の値で、Pane が別 Workspace へ
# 移されても更新されない。Pane ID は移動しても変わらないので、そちらを起点に
# 実行時に引き直す。
SELF_PANE="${HERDR_PANE_ID:-}"
SELF_WS=""

resolve_self() {
  [ -n "$SELF_PANE" ] || { SELF_WS="${HERDR_WORKSPACE_ID:-}"; return 0; }
  SELF_WS=$(herdr pane get "$SELF_PANE" 2>/dev/null | jget result.pane.workspace_id 2>/dev/null) || SELF_WS=""
  [ -n "$SELF_WS" ] || SELF_WS="${HERDR_WORKSPACE_ID:-}"
  return 0
}

SCENARIOS="gather move extract merge swap layout navigator"

# ---------------------------------------------------------------------------
# 下ごしらえ
# ---------------------------------------------------------------------------

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
say() { printf '\033[36m▍\033[0m %s\n' "$*"; }
ok()  { printf '\033[32m✓\033[0m %s\n' "$*"; }
warn(){ printf '\033[33m!\033[0m %s\n' "$*"; }

need() {
  command -v herdr   >/dev/null 2>&1 || die "herdr が PATH にありません"
  command -v python3 >/dev/null 2>&1 || die "python3 が PATH にありません"
  herdr workspace list >/dev/null 2>&1 || die "herdr サーバに接続できません。herdr を起動してから実行してください"
}

# JSON レスポンスから 1 つの値を取り出す。jq に依存しない。
jget() { python3 -c 'import json,sys;d=json.load(sys.stdin)
p=sys.argv[1].split(".")
for k in p: d = d[int(k)] if k.isdigit() else d[k]
print(d)' "$1"; }

# CLI に無いメソッド用の生ソケット呼び出し。Herdr は1接続1リクエスト。
api() { printf '%s\n' "$1" | nc -U "$SOCK" 2>/dev/null; }

# ---------------------------------------------------------------------------
# Herdr 操作
# ---------------------------------------------------------------------------

ws_create() { herdr workspace create --label "$1" --no-focus | jget result.workspace.workspace_id; }

tab_create() { herdr tab create --workspace "$1" --label "$2" --no-focus | jget result.tab.tab_id; }

first_pane() {
  herdr pane list --workspace "${1%%:*}" |
    python3 -c 'import json,sys
tab=sys.argv[1]
for p in json.load(sys.stdin)["result"]["panes"]:
    if p["tab_id"]==tab: print(p["pane_id"]); break' "$1"
}

split() { herdr pane split "$1" --direction "$2" --no-focus | jget result.pane.pane_id; }

name() { herdr pane rename "$1" "$2" >/dev/null; }

banner() {
  local pane="$1" title="$2" note="$3"
  herdr pane run "$pane" \
    "clear; printf '\033[1;36m▍%s\033[0m\n\033[2m%s\033[0m\n\n' '$title' '$note'" >/dev/null
}

# 擬似Agentの状態を作る。
#   working / blocked / idle : そのまま報告する
#   done                     : working → idle と報告すると Herdr が done に遷移させる
agent() {
  local pane="$1" kind="$2" state="$3" msg="${4:-}"
  if [ -n "$CURRENT" ]; then
    mkdir -p "$STATE_DIR"
    printf '%s\t%s\t%s\t%s\n' "$pane" "$kind" "$state" "$msg" >> "$STATE_DIR/$CURRENT.agents"
  fi
  apply_agent "$pane" "$kind" "$state" "$msg"
}

apply_agent() {
  local pane="$1" kind="$2" state="$3" msg="${4:-}"
  case "$state" in
    done)
      herdr pane report-agent "$pane" --source handson --agent "$kind" --state working >/dev/null
      sleep 0.3
      herdr pane report-agent "$pane" --source handson --agent "$kind" --state idle >/dev/null
      ;;
    *)
      herdr pane report-agent "$pane" --source handson --agent "$kind" --state "$state" \
        ${msg:+--message "$msg"} >/dev/null
      ;;
  esac
}

# ---------------------------------------------------------------------------
# シナリオの台帳
#
# .state/<名前>.ws     このシナリオが作った Workspace ID
# .state/<名前>.panes  このシナリオが作った Pane ID
# .state/<名前>.agents 擬似Agentの状態（arm 用）
#
# clean はこの台帳と実際の Pane を突き合わせ、載っていない Pane が1つでも
# あれば、そのWorkspaceを閉じません。
# ---------------------------------------------------------------------------

drill_index() {
  local i=1 s
  for s in $SCENARIOS; do
    [ "$s" = "$1" ] && { echo "$i"; return; }
    i=$(( i + 1 ))
  done
  echo 99
}

label_for() { printf '%s %s: %s' "$PREFIX" "$(drill_index "$1")" "$1"; }

# ラベルがハンズオンのものか（旧形式 "ハンズオン: x" も拾う）
is_handson_label() { case "$1" in "$PREFIX"*) return 0 ;; *) return 1 ;; esac; }

# シナリオ名から Workspace ID を引く。ラベル末尾の ": <名前>" で照合するので、
# 番号付きでも旧形式でも当たる。
ws_of() {
  herdr workspace list | python3 -c 'import json,sys
pre, want = sys.argv[1], sys.argv[2]
for w in json.load(sys.stdin)["result"]["workspaces"]:
    label = w.get("label") or ""
    if not label.startswith(pre): continue
    if label.split(":", 1)[-1].strip() == want:
        print(w["workspace_id"]); break' "$PREFIX" "$1"
}

# シナリオが今持っている Workspace 群（本体と "名前 2"）
ws_all_of() {
  local id
  for suffix in "" " 2"; do
    id=$(ws_of "$1$suffix")
    [ -n "$id" ] && echo "$id"
  done
  return 0
}

# 台帳を書き出す。setup が終わった時点で Workspace の中に居る Pane は
# すべてこのスクリプトが作ったもの。
record_scenario() {
  local scenario="$1" ws
  mkdir -p "$STATE_DIR"
  : > "$STATE_DIR/$scenario.ws"
  : > "$STATE_DIR/$scenario.panes"
  while read -r ws; do
    [ -n "$ws" ] || continue
    echo "$ws" >> "$STATE_DIR/$scenario.ws"
    herdr pane list --workspace "$ws" |
      python3 -c 'import json,sys
for p in json.load(sys.stdin)["result"]["panes"]: print(p["pane_id"])' \
      >> "$STATE_DIR/$scenario.panes"
  done < <(ws_all_of "$scenario")
}

# 台帳があるか。無い＝古い形式で作られたシナリオで、中身の素性が分からない。
has_ledger() { [ -s "$STATE_DIR/$1.panes" ]; }

# 台帳に載っていない Pane を列挙する
foreign_panes() {
  local ws="$1" ledger="$2"
  herdr pane list --workspace "$ws" |
    python3 -c 'import json,sys,os
ledger = set()
path = sys.argv[1]
if os.path.exists(path):
    ledger = {l.strip() for l in open(path) if l.strip()}
for p in json.load(sys.stdin)["result"]["panes"]:
    if p["pane_id"] not in ledger:
        print("%s\t%s" % (p["pane_id"], p.get("label") or p.get("terminal_title_stripped") or "-"))' \
    "$ledger"
}

# ---------------------------------------------------------------------------
# シナリオ
# ---------------------------------------------------------------------------

describe() {
  case "$1" in
    gather)    echo "5つのAgentが3つのTabに散らばり、blocked / done / working が混在した状態" ;;
    move)      echo "Tabが6つあり、明らかに置き場所を間違えたPaneが1つある状態" ;;
    extract)   echo "1つのTabに4Pane詰まっていて、1つを独立させたい状態" ;;
    merge)     echo "内容の近いTabが2つに分かれてしまっている状態" ;;
    swap)      echo "大事なPaneが右下の狭い位置にいる状態" ;;
    layout)    echo "同一Tab内で幅がバラバラ・Paneが5つある状態" ;;
    navigator) echo "Workspace 2つ・Tab 8つに紛れて目的のPaneが見つからない状態" ;;
    *)         echo "" ;;
  esac
}

setup_gather() {
  local ws t1 t2 t3 p
  ws=$(ws_create "$(label_for gather)")

  t1=$(first_pane "$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)")
  herdr tab rename "$(herdr pane get "$t1" | jget result.pane.tab_id)" "review" >/dev/null
  name "$t1" "auth review"
  banner "$t1" "auth review" "claude · blocked — 権限まわりの確認待ち"
  agent "$t1" claude blocked "この変更を適用してよいですか？"

  p=$(split "$t1" right)
  name "$p" "auth tests"
  banner "$p" "auth tests" "codex · working — テストを書いている最中"
  agent "$p" codex working

  t2=$(tab_create "$ws" "migration")
  p=$(first_pane "$t2")
  name "$p" "db migration"
  banner "$p" "db migration" "claude · done — 終わったのに気づかれていない"
  agent "$p" claude done

  # ここは working にしておく。done はタブを開いた時点で idle に落ちるので、
  # 「4つ集まる」という結果を done に依存させるとシナリオが壊れる。
  # 溶けない Agent を 4体（blocked 2 + working 2）確保しておき、
  # done は 1体だけ「増える側」として置く。
  t3=$(tab_create "$ws" "docs")
  p=$(first_pane "$t3")
  name "$p" "changelog"
  banner "$p" "changelog" "codex · working — まだ書いている"
  agent "$p" codex working

  p=$(split "$p" down)
  name "$p" "api docs"
  banner "$p" "api docs" "claude · blocked — 書き方の指示待ち"
  agent "$p" claude blocked "どの形式で書きますか？"

  p=$(tab_create "$ws" "server")
  p=$(first_pane "$p")
  name "$p" "dev server"
  banner "$p" "dev server" "Agentではないので Gather は連れて行かない"
}

setup_move() {
  local ws tab p first
  ws=$(ws_create "$(label_for move)")
  first=$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)
  herdr tab rename "$first" "editor" >/dev/null
  p=$(first_pane "$first")
  name "$p" "editor"
  banner "$p" "editor" "ここが起点。Tabは全部で6つある"

  local i=1
  for label in build test lint deploy notes; do
    tab=$(tab_create "$ws" "$label")
    p=$(first_pane "$tab")
    name "$p" "$label"
    i=$(( i + 1 ))
    banner "$p" "$label" "Tab $i · prefix+m → $i でここに飛ばせる"
  done

  p=$(split "$(first_pane "$first")" down)
  name "$p" "deploy log"
  banner "$p" "deploy log" "本当は deploy Tab に居るべき迷子のPane"
}

setup_extract() {
  local ws root a b c
  ws=$(ws_create "$(label_for extract)")
  root=$(first_pane "$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)")
  herdr tab rename "$(herdr pane get "$root" | jget result.pane.tab_id)" "everything" >/dev/null

  name "$root" "editor"
  banner "$root" "editor" "本命の作業Pane"

  a=$(split "$root" right)
  b=$(split "$root" down)
  c=$(split "$a" down)

  name "$a" "long build"
  banner "$a" "long build" "10分かかる。独立させたいのはこれ"
  agent "$a" codex working

  name "$b" "git status"
  banner "$b" "git status" "ちら見するだけ"

  name "$c" "log tail"
  banner "$c" "log tail" "流しっぱなし"
}

setup_merge() {
  local ws t1 t2 p q
  ws=$(ws_create "$(label_for merge)")
  t1=$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)
  herdr tab rename "$t1" "frontend" >/dev/null
  p=$(first_pane "$t1")
  name "$p" "vite"
  banner "$p" "vite" "統合先。ここに backend を畳み込む"

  t2=$(tab_create "$ws" "backend")
  p=$(first_pane "$t2")
  name "$p" "api"
  banner "$p" "api" "上下2段の構造。Merge してもこの形は保たれる"
  q=$(split "$p" down)
  name "$q" "worker"
  banner "$q" "worker" "api の下。Merge 後も api の下に残る"
}

setup_swap() {
  local ws root a b
  ws=$(ws_create "$(label_for swap)")
  root=$(first_pane "$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)")
  herdr tab rename "$(herdr pane get "$root" | jget result.pane.tab_id)" "swap" >/dev/null

  name "$root" "log tail"
  banner "$root" "log tail" "広い左側にいるが、実はあまり見ない"

  a=$(split "$root" right)
  name "$a" "notes"
  banner "$a" "notes" "右上"

  b=$(split "$a" down)
  name "$b" "main work"
  banner "$b" "main work" "★ 本命。右下の狭い場所にいる。log tail と入れ替えたい"
  agent "$b" claude working
}

setup_layout() {
  local ws root a b c d
  ws=$(ws_create "$(label_for layout)")
  root=$(first_pane "$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)")
  herdr tab rename "$(herdr pane get "$root" | jget result.pane.tab_id)" "lopsided" >/dev/null

  # 形は (r (d pane1 pane5) (r pane2 (d pane3 pane4)))
  #   左列 : pane 1 / pane 5
  #   中央 : pane 2（全高）
  #   右列 : pane 3 / pane 4
  name "$root" "pane 1"
  banner "$root" "pane 1" "Equalize / Grid / Main Left を試す場所"
  a=$(split "$root" right); name "$a" "pane 2"; banner "$a" "pane 2" "中央・全高"
  b=$(split "$a" right);    name "$b" "pane 3"; banner "$b" "pane 3" "prefix+alt+l → e で揃う"
  c=$(split "$b" down);     name "$c" "pane 4"; banner "$c" "pane 4" "g で格子に"
  d=$(split "$root" down);  name "$d" "pane 5"; banner "$d" "pane 5" "h で pane 1 を主役に"

  herdr pane resize "$a" --amount 18 >/dev/null 2>&1 || true
}

setup_navigator() {
  local ws ws2 p tab
  ws=$(ws_create "$(label_for navigator)")
  tab=$(herdr tab list --workspace "$ws" | jget result.tabs.0.tab_id)
  herdr tab rename "$tab" "alpha" >/dev/null
  p=$(first_pane "$tab")
  name "$p" "alpha"
  banner "$p" "alpha" "この Workspace には Tab が 5 つある"

  for label in bravo charlie delta echo; do
    tab=$(tab_create "$ws" "$label")
    p=$(first_pane "$tab")
    name "$p" "$label"
    banner "$p" "$label" "prefix+f で名前から直接飛べる"
  done

  p=$(split "$p" down)
  name "$p" "遠くの codex"
  banner "$p" "遠くの codex" "★ 目的のPane。prefix+f → Tab キーで Agent 検索に切り替えて探す"
  agent "$p" codex blocked "ここに来られましたか？"

  ws2=$(ws_create "$(label_for navigator) 2")
  tab=$(herdr tab list --workspace "$ws2" | jget result.tabs.0.tab_id)
  herdr tab rename "$tab" "foxtrot" >/dev/null
  p=$(first_pane "$tab")
  name "$p" "foxtrot"
  banner "$p" "foxtrot" "別 Workspace。prefix+f → Tab キー 3 回で Workspace 検索"
  for label in golf hotel; do
    tab=$(tab_create "$ws2" "$label")
    p=$(first_pane "$tab")
    name "$p" "$label"
    banner "$p" "$label" "Workspace をまたいだ検索の練習用"
  done
}

# ---------------------------------------------------------------------------
# コマンド
# ---------------------------------------------------------------------------

cmd_list() {
  printf '\n\033[1mシナリオ\033[0m\n\n'
  local i=1
  for s in $SCENARIOS; do
    printf '  \033[2m%02d\033[0m \033[36m%-10s\033[0m %s\n' "$i" "$s" "$(describe "$s")"
    i=$(( i + 1 ))
  done
  printf '\n     \033[36m%-10s\033[0m %s\n\n' "all" "上を全部"
}

cmd_setup() {
  local target="${1:-}"
  [ -n "$target" ] || die "シナリオ名が必要です。./handson.sh list で一覧"

  if [ "$target" = all ]; then
    for s in $SCENARIOS; do cmd_setup "$s"; done
    cmd_order
    return
  fi

  case " $SCENARIOS " in
    *" $target "*) ;;
    *) die "知らないシナリオ: $target" ;;
  esac

  if [ -n "$(ws_of "$target")" ]; then
    say "$(label_for "$target") は既にあります。作り直すなら reset を使ってください"
    return
  fi

  say "用意中: $target — $(describe "$target")"
  mkdir -p "$STATE_DIR"
  : > "$STATE_DIR/$target.agents"
  CURRENT="$target"
  "setup_${target}"
  CURRENT=""
  record_scenario "$target"
  ok "できました: $(label_for "$target")"
}

# レイアウトはそのままに、Agentの状態だけ入れ直す。
cmd_arm() {
  local target="${1:-}"
  [ -n "$target" ] || die "シナリオ名が必要です"

  if [ "$target" = all ]; then
    # 中身のある台帳だけ。Agentの居ないシナリオ（move など）は黙って飛ばす。
    for s in $SCENARIOS; do
      [ -s "$STATE_DIR/$s.agents" ] && cmd_arm "$s"
    done
    return 0
  fi

  local file="$STATE_DIR/$target.agents"
  [ -s "$file" ] || die "$target には入れ直す状態がありません（setup がまだ、またはAgent無しのシナリオ）"

  local n=0 pane kind state msg
  while IFS=$'\t' read -r pane kind state msg; do
    [ -n "$pane" ] || continue
    herdr pane get "$pane" >/dev/null 2>&1 || continue
    apply_agent "$pane" "$kind" "$state" "$msg"
    n=$(( n + 1 ))
  done < "$file"
  ok "$target: $n 個のAgent状態を入れ直しました"
}

# ハンズオンWorkspaceを、実作業のWorkspaceの後ろにドリル順で並べる。
cmd_order() {
  local total moved=0 ws
  total=$(herdr workspace list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]["workspaces"]))')

  for s in $SCENARIOS; do
    while read -r ws; do
      [ -n "$ws" ] || continue
      api "{\"id\":\"order\",\"method\":\"workspace.move\",\"params\":{\"workspace_id\":\"$ws\",\"insert_index\":$(( total - 1 ))}}" >/dev/null
      moved=$(( moved + 1 ))
    done < <(ws_all_of "$s")
  done

  [ "$moved" -gt 0 ] && ok "$moved 個の Workspace をドリル順に並べ直しました"
  return 0
}

cmd_reset() {
  local target="${1:-}"
  [ -n "$target" ] || die "シナリオ名が必要です"
  if [ "$target" = all ]; then
    cmd_clean
    cmd_setup all
    return
  fi
  teardown_one "$target" || return 1
  cmd_setup "$target"
  cmd_order
}

# 1シナリオを閉じる。よそのPaneが混ざっていたら閉じない。
teardown_one() {
  local scenario="$1" force="${2:-}" ws foreign blocked=0
  while read -r ws; do
    [ -n "$ws" ] || continue

    # 自分が座っている Workspace は、--force でも閉じない。
    # 閉じた瞬間にこのシェルが死ぬので、残りが片付かなくなる。
    if [ -n "$SELF_WS" ] && [ "$ws" = "$SELF_WS" ]; then
      # 全角括弧を変数名の直後に置かないこと。ロケールによっては bash が
      # マルチバイト文字の先頭バイトを変数名に取り込み、未定義変数になる。
      warn "スキップ: $scenario ($ws) — このスクリプトを実行しているPane ${SELF_PANE} が居ます"
      printf '        \033[2m別のWorkspaceのシェルから実行し直してください。\033[0m\n'
      blocked=1
      continue
    fi

    if ! has_ledger "$scenario"; then
      if [ "$force" != "--force" ]; then
        warn "スキップ: $scenario ($ws) — 台帳がないので中身の素性を確認できません"
        printf '        \033[2m古い形式で作られたシナリオです。中身を見て問題なければ clean --force。\033[0m\n'
        blocked=1
        continue
      fi
    else
      foreign=$(foreign_panes "$ws" "$STATE_DIR/$scenario.panes")
      if [ -n "$foreign" ] && [ "$force" != "--force" ]; then
        warn "スキップ: $scenario ($ws) — このスクリプトが作っていないPaneが入っています"
        printf '%s\n' "$foreign" | while IFS=$'\t' read -r pid plabel; do
          printf '        \033[33m%s\033[0m  %s\n' "$pid" "$plabel"
        done
        printf '        \033[2m先にこのPaneを別のWorkspaceへ移してください。\033[0m\n'
        printf '        \033[2m中身ごと閉じてよければ clean --force。\033[0m\n'
        blocked=1
        continue
      fi
    fi
    if herdr workspace close "$ws" >/dev/null 2>&1; then
      say "閉じた: $scenario ($ws)"
    else
      warn "閉じられませんでした: $scenario ($ws)"
      blocked=1
    fi
  done < <(ws_all_of "$scenario")
  [ "$blocked" -eq 0 ]
}

cmd_clean() {
  local force="${1:-}" closed=0 kept=0 s
  # 既に扱ったWorkspace。最後の掃き寄せで二重に報告しないため。
  local handled=""

  for s in $SCENARIOS; do
    local owned
    owned=$(ws_all_of "$s")
    [ -n "$owned" ] || continue
    handled="$handled $owned"
    if teardown_one "$s" "$force"; then
      closed=$(( closed + 1 ))
      rm -f "$STATE_DIR/$s.ws" "$STATE_DIR/$s.panes" "$STATE_DIR/$s.agents"
    else
      kept=$(( kept + 1 ))
    fi
  done

  # シナリオ名で引けなかったもの（旧形式のラベル、リネームされたもの）を拾う
  local orphan
  while :; do
    orphan=$(herdr workspace list | python3 -c 'import json,sys
pre = sys.argv[1]
seen = set(sys.argv[2].split())
for w in json.load(sys.stdin)["result"]["workspaces"]:
    label = w.get("label") or ""
    if label.startswith(pre) and w["workspace_id"] not in seen:
        print(w["workspace_id"]); break' "$PREFIX" "$handled")
    [ -n "$orphan" ] || break
    handled="$handled $orphan"

    if [ -n "$SELF_WS" ] && [ "$orphan" = "$SELF_WS" ]; then
      warn "スキップ: $orphan — このスクリプトを実行しているPaneが居ます"
      kept=$(( kept + 1 ))
      continue
    fi
    if [ "$force" != "--force" ]; then
      warn "スキップ: $orphan — 台帳が無く、中身の素性を確認できません（clean --force で閉じられます）"
      kept=$(( kept + 1 ))
      continue
    fi
    if herdr workspace close "$orphan" >/dev/null 2>&1; then
      closed=$(( closed + 1 ))
    else
      warn "閉じられませんでした: $orphan"
      kept=$(( kept + 1 ))
    fi
  done

  rmdir "$STATE_DIR" 2>/dev/null || true

  if [ "$closed" -gt 0 ]; then ok "$closed 個のシナリオを片付けました"; fi
  if [ "$kept" -gt 0 ]; then warn "$kept 個は残しました（上の理由を確認してください）"; fi
  if [ "$closed" -eq 0 ] && [ "$kept" -eq 0 ]; then say "片付けるものはありませんでした"; fi
}

cmd_status() {
  local rows ws s foreign

  rows=$(herdr workspace list | python3 -c 'import json,sys
pre = sys.argv[1]
out = [w for w in json.load(sys.stdin)["result"]["workspaces"] if (w.get("label") or "").startswith(pre)]
for w in out:
    print("%s\t%s\t%d\t%d" % (w["workspace_id"], w.get("label") or "", w["tab_count"], w["pane_count"]))' "$PREFIX")

  if [ -z "$rows" ]; then
    printf '\nハンズオン用のWorkspaceはありません。./handson.sh setup all で用意できます\n\n'
    return
  fi

  printf '\n\033[1m用意されているシナリオ\033[0m\n\n'
  printf '%s\n' "$rows" | while IFS=$'\t' read -r id label tabs panes; do
    printf '  \033[36m%-22s\033[0m %-4s tab=%-2s pane=%-2s\n' "$label" "$id" "$tabs" "$panes"
  done

  # よそのPaneが混ざっていないか
  printf '\n'
  local any=0 noledger=""
  for s in $SCENARIOS; do
    while read -r ws; do
      [ -n "$ws" ] || continue
      if ! has_ledger "$s"; then
        noledger="$noledger $s"
        continue
      fi
      foreign=$(foreign_panes "$ws" "$STATE_DIR/$s.panes")
      [ -n "$foreign" ] || continue
      if [ "$any" -eq 0 ]; then
        printf '\033[1m\033[33mよそのPaneが混ざっているWorkspace\033[0m\n\n'
        any=1
      fi
      printf '  \033[33m%s\033[0m (%s)\n' "$s" "$ws"
      printf '%s\n' "$foreign" | while IFS=$'\t' read -r pid plabel; do
        printf '      %s  %s\n' "$pid" "$plabel"
      done
    done < <(ws_all_of "$s")
  done
  if [ "$any" -eq 1 ]; then
    printf '\n  \033[2mclean はこれらを閉じません。先に別のWorkspaceへ移してください。\033[0m\n\n'
  fi
  if [ -n "$noledger" ]; then
    noledger=$(printf '%s\n' $noledger | sort -u | tr '\n' ' ')
    printf '\033[33m台帳のないシナリオ:\033[0m %s\n' "$noledger"
    printf '  \033[2m古い形式で作られています。clean --force で閉じるか、reset で作り直してください。\033[0m\n\n'
  fi
  if [ -n "$SELF_WS" ]; then
    printf '\033[2m実行中のPane: %s (%s)\033[0m\n\n' "$SELF_PANE" "$SELF_WS"
  fi

  # 今この瞬間 Gather が拾う数。done はタブを開くと消えるので、
  # キーを押す前にここで実数を確かめられるようにしておく。
  local gws
  gws=$(ws_of gather)
  herdr agent list | python3 -c 'import json,sys
gws = sys.argv[1]
a = json.load(sys.stdin)["result"]["agents"]
if not a:
    raise SystemExit
print("\033[1m検出されているAgent\033[0m\n")
color = {"blocked": "31", "done": "32", "working": "33"}
for x in a:
    here = "  \033[36m← gather\033[0m" if gws and x["workspace_id"] == gws else ""
    print("  %-8s \033[%sm%-8s\033[0m %s%s"
          % (x.get("agent") or "-", color.get(x["agent_status"], "2"), x["agent_status"], x["pane_id"], here))
print()
if gws:
    live = [x for x in a if x["workspace_id"] == gws
            and x["agent_status"] in ("blocked", "done", "working")]
    print("\033[1mGatherが今すぐ拾う数: %d\033[0m" % len(live))
    if len(live) < 5:
        print("  \033[2mdone はタブを開くと idle に落ちます。5体に戻すには ./handson.sh arm gather\033[0m")
    print()' "$gws"
}

need
resolve_self
case "${1:-list}" in
  list)   cmd_list ;;
  setup)  shift; cmd_setup "${1:-}" ;;
  arm)    shift; cmd_arm "${1:-}" ;;
  order)  cmd_order ;;
  reset)  shift; cmd_reset "${1:-}" ;;
  clean)  shift; cmd_clean "${1:-}" ;;
  status) cmd_status ;;
  *)      die "使い方: $0 {list|setup <名前>|arm <名前>|order|reset <名前>|clean [--force]|status}" ;;
esac
