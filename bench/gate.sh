#!/usr/bin/env bash
# Lanes' P1 gate: five Claude Code agents work in five lanes on one repo, made through
# `claude -w`, with no manual steps. The macOS and Linux twin of gate.ps1; bench/GATE.md
# explains each check.
#
# Usage: bench/gate.sh --repo <path> [--lane <path>] [--claude <path>] [--timeout <seconds>]
#                      [--no-agents] [--json]
#
# Checks that `lane setup` has put Claude Code in lanes, that the repo is clean and that nothing
# called gate-* exists. Then starts five `claude -w gate-N -p "..."` sessions at once, each
# adding a comment to a different file, and checks the lanes, their ports, that each change
# stayed in its own lane, that Git has no worktree outside the lanes folder, that
# `git worktree add` is turned down, and that `lane savings` counts the lanes. Then it removes
# the lanes the way Claude Code does (its WorktreeRemove hook) and checks they're gone and still
# counted.
#
# Exits 0 when every check passes, 1 when one fails and 2 when the arguments are wrong. It never
# prompts. Needs bash 3.2 or later, git and jq.

set -u

# shellcheck disable=SC2016 # backticks are part of the text
USAGE='Usage: bench/gate.sh --repo <path> [--lane <path>] [--claude <path>] [--timeout <seconds>] [--no-agents] [--json]

  --repo PATH      The repo to run the gate on: a clean Git checkout (the real gate uses withastro/astro)
  --lane PATH      The lane program to use (default: the one on PATH); Claude Code'"'"'s hooks get it too
  --claude PATH    The claude program to use (default: the one on PATH)
  --timeout SECS   How long to wait for the five sessions (default 1200)
  --no-agents      Make the lanes with `lane new gate-N` and a scripted edit instead of Claude Code
  --json           Print one JSON object with every check
  -h, --help       Show this help'

REPO='' LANE_ARG='' CLAUDE_ARG='' TIMEOUT=1200 NO_AGENTS=0 JSON=0
NAMES='gate-1 gate-2 gate-3 gate-4 gate-5'
US=$(printf '\037') # the unit separator, between fields that may be empty

say() { printf 'gate: %s\n' "$*" >&2; }

usage_error() {
  say "$1"
  if [ "$JSON" = 1 ] && command -v jq >/dev/null 2>&1; then
    jq -n --arg e "$1" '{passed: false, error: $e}'
  fi
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --repo|--lane|--claude|--timeout)
      [ $# -ge 2 ] || usage_error "$1 needs a value, like $1 <path>. Run bench/gate.sh --help."
      case "$1" in
        --repo) REPO=$2 ;; --lane) LANE_ARG=$2 ;; --claude) CLAUDE_ARG=$2 ;; --timeout) TIMEOUT=$2 ;;
      esac
      shift 2 ;;
    --repo=*) REPO=${1#*=}; shift ;;
    --lane=*) LANE_ARG=${1#*=}; shift ;;
    --claude=*) CLAUDE_ARG=${1#*=}; shift ;;
    --timeout=*) TIMEOUT=${1#*=}; shift ;;
    --no-agents) NO_AGENTS=1; shift ;;
    --json) JSON=1; shift ;;
    -h|--help) printf '%s\n' "$USAGE"; exit 0 ;;
    *) usage_error "unknown option \`$1\`. Run bench/gate.sh --help." ;;
  esac
done
[ -n "$REPO" ] || usage_error 'which repo? Run it as: bench/gate.sh --repo <path to a clean checkout> [--no-agents] [--json]'
case "$TIMEOUT" in
  ''|*[!0-9]*|0) usage_error '--timeout is in seconds and must be 1 or more, like --timeout 1200.' ;;
esac
command -v jq >/dev/null 2>&1 || {
  say 'the gate reads JSON with jq: install it (brew install jq, or apt install jq), then run it again'
  exit 1
}

if [ "$NO_AGENTS" = 1 ]; then MODE=no-agents; else MODE=agents; fi
LOG_DIR=$(mktemp -d "${TMPDIR:-/tmp}/lane-gate.XXXXXX") || { say "couldn't make a folder for logs in ${TMPDIR:-/tmp}"; exit 1; }
CHECKS="$LOG_DIR/checks.jsonl"
: > "$CHECKS"
REPO_ROOT='' LANE='' LANE_VERSION='' CLAUDE='' GIT_BIN=''
MADE=0 CLEANED=0 PIDS=''

# ---------- output ----------

add_check() { # id passed(true|false) detail
  jq -nc --arg id "$1" --arg p "$2" --arg d "$3" '{id: $id, passed: ($p == "true"), detail: $d}' >> "$CHECKS"
  if [ "$JSON" = 0 ]; then
    if [ "$2" = true ]; then mark=PASS; else mark=FAIL; fi
    printf '%s  %-13s %s\n' "$mark" "$1" "$3"
  fi
}

finish() { # [note] -> exits
  note=${1-}
  total=$(wc -l < "$CHECKS" | tr -d ' ')
  failed=$(jq -s '[.[] | select(.passed | not)] | length' "$CHECKS")
  ids=$(jq -rs '[.[] | select(.passed | not) | .id] | join(", ")' "$CHECKS")
  passed=false
  if [ "$total" -gt 0 ] && [ "$failed" = 0 ] && [ -z "$note" ]; then passed=true; fi
  if [ "$JSON" = 1 ]; then
    jq -s --argjson passed "$passed" --arg mode "$MODE" --arg repo "$REPO_ROOT" --arg lane "$LANE" \
      --arg version "$LANE_VERSION" --arg claude "$CLAUDE" --arg logs "$LOG_DIR" \
      --argjson seconds "$SECONDS" --arg note "$note" \
      '{passed: $passed, mode: $mode, repo: $repo, lane: $lane, lane_version: $version,
        claude: (if $claude == "" then null else $claude end), logs: $logs, seconds: $seconds,
        checks: .} + (if $note == "" then {} else {note: $note} end)' "$CHECKS"
  else
    [ -z "$note" ] || printf '%s\n' "$note"
    if [ "$passed" = true ]; then
      printf 'Gate passed: %s of %s checks, in %s s. Logs: %s\n' "$total" "$total" "$SECONDS" "$LOG_DIR"
    elif [ -z "$note" ]; then
      printf 'Gate failed: %s of %s checks (%s), in %s s. Logs: %s\n' "$failed" "$total" "$ids" "$SECONDS" "$LOG_DIR"
    fi
  fi
  if [ "$passed" = true ]; then exit 0; fi
  exit 1
}

join() { # sep items...
  sep=$1; shift; out=''
  for x in "$@"; do if [ -z "$out" ]; then out=$x; else out="$out$sep$x"; fi; done
  printf '%s' "$out"
}

# ---------- paths ----------
# On Windows (Git Bash) lane prints C:\... and git C:/...: compare them one way, case and all.

if command -v cygpath >/dev/null 2>&1; then
  norm() { cygpath -m "$1" | sed 's:/*$::' | tr '[:upper:]' '[:lower:]'; }
  upath() { cygpath -u "$1"; }
else
  norm() { printf '%s\n' "$1" | sed 's:/*$::'; }
  upath() { printf '%s\n' "$1"; }
fi

# The folder a path is in, spelled the way the path is (lane prints C:\... on Windows).
parent() { printf '%s\n' "$1" | sed 's:[\\/][^\\/]*$::'; }

under() { # path dir
  case "$(norm "$1")" in "$(norm "$2")"/*) return 0 ;; esac
  return 1
}

# ---------- programs ----------

find_program() { # given name
  want=${1:-$2}
  case "$want" in
    */*) [ -f "$want" ] && [ -x "$want" ] || return 1
         printf '%s/%s\n' "$(cd "$(dirname "$want")" && pwd -P)" "$(basename "$want")" ;;
    *) command -v "$want" 2>/dev/null || return 1 ;;
  esac
}

# lane <args> --json in the repo; stdout in $OUT, stderr in $ERR, exit code in $CODE.
lane_json() {
  OUT=$(cd "$REPO_ROOT" && "$LANE" "$@" --json 2>"$LOG_DIR/stderr.txt"); CODE=$?
  ERR=$(cat "$LOG_DIR/stderr.txt")
}

# What lane said went wrong: its JSON error and hint, else stderr.
lane_error() {
  msg=$(printf '%s' "$OUT" | jq -r 'if .error then .error.message + (if .error.hint then " (" + .error.hint + ")" else "" end) else empty end' 2>/dev/null)
  [ -n "$msg" ] || msg=$(printf '%s %s' "$ERR" "$OUT" | tr '\n' ' ' | cut -c1-300)
  [ -n "$msg" ] || msg="exit $CODE"
  printf '%s' "$msg"
}

worktrees() { git -C "$REPO_ROOT" worktree list --porcelain | sed -n 's/^worktree //p'; }

# This repo's gate lanes, one line each: name, id, port start, port count, status, agent, path.
# Fields are split by $US, so an empty one stays in its place.
gate_lanes() {
  lane_json list
  printf '%s' "$OUT" | jq -r '.lanes[]? | select(.name | test("^gate-[1-5]$"))
    | "\(.name)\u001f\(.id)\u001f\(.ports.start)\u001f\(.ports.count)\u001f\(.status)\u001f\(.agent // "")\u001f\(.path)"'
}

lane_field() { # name field-number -> from $LANES_NOW
  printf '%s\n' "$LANES_NOW" | awk -F "$US" -v n="$1" -v f="$2" '$1 == n { print $f }'
}

savings_counts() { # -> "now total removed", or nothing
  lane_json savings
  [ "$CODE" = 0 ] || return 1
  printf '%s' "$OUT" | jq -r 'select(.total != null) | "\(.now.lanes) \(.total.lanes) \(.total.removed)"'
}

# Claude Code's user settings: where `lane setup` puts its hooks.
claude_settings() {
  if [ -n "${LANE_AGENTS_HOME-}" ]; then printf '%s/.claude/settings.json\n' "$LANE_AGENTS_HOME"
  elif [ -n "${CLAUDE_CONFIG_DIR-}" ]; then printf '%s/settings.json\n' "$CLAUDE_CONFIG_DIR"
  else printf '%s/.claude/settings.json\n' "$HOME"; fi
}

# Five tracked files to edit, one per lane: README.md files first, then other small text files,
# shallowest first. Paths with only plain characters, so they pass through any shell untouched.
gate_files() {
  git -C "$REPO_ROOT" ls-files -s -z | tr '\0' '\n' | awk -F'\t' '
    $1 ~ /^100644 [0-9a-f]+ 0$/ {
      p = $2
      if (p !~ "^[A-Za-z0-9_./-]+$" || p ~ "(^|/)[.]") next
      n = split(p, parts, "/"); leaf = parts[n]
      if (index(leaf, ".") == 0) next
      ext = tolower(leaf); sub(/^.*\./, "", ext)
      if (ext !~ /^(md|txt|ts|js|mjs|py|rs|go|toml|yml|yaml|sh)$/) next
      rank = (tolower(leaf) == "readme.md") ? 0 : (ext == "md" ? 1 : 2)
      printf "%d %03d %s\n", rank, n, p
    }' | LC_ALL=C sort -k1,1n -k2,2n -k3,3 | while read -r _ _ p; do
      f="$(upath "$REPO_ROOT")/$p"
      [ -f "$f" ] && [ "$(wc -c < "$f")" -le 204800 ] && printf '%s\n' "$p"
    done | head -n 5
}

# The line the scripted edit adds (--no-agents): a comment in the file's own syntax.
comment_line() { # file name
  case "$1" in
    *.md) printf '<!-- edited in lane %s -->' "$2" ;;
    *.ts|*.js|*.mjs|*.rs|*.go) printf '// edited in lane %s' "$2" ;;
    *.txt) printf 'edited in lane %s' "$2" ;;
    *) printf '# edited in lane %s' "$2" ;;
  esac
}

# Remove the gate's lanes the way Claude Code does when it's done with a worktree: its
# WorktreeRemove hook. `lane rm` takes any the hook leaves. Sets VIA_HOOK, VIA_RM, RM_FAILED.
remove_gate_lanes() {
  VIA_HOOK=0 VIA_RM=0 RM_FAILED=''
  LANES_NOW=$(gate_lanes)
  for name in $NAMES; do
    path=$(lane_field "$name" 7)
    [ -n "$path" ] || continue
    input=$(jq -nc --arg cwd "$REPO_ROOT" --arg p "$path" \
      '{session_id: "lane-gate", cwd: $cwd, hook_event_name: "WorktreeRemove", worktree_path: $p}')
    if printf '%s' "$input" | (cd "$REPO_ROOT" && "$LANE" hook claude worktree-remove --json) >/dev/null 2>&1 \
      && [ ! -e "$(upath "$path")" ]; then
      VIA_HOOK=$((VIA_HOOK + 1)); continue
    fi
    lane_json rm "$name"
    if [ "$CODE" = 0 ]; then VIA_RM=$((VIA_RM + 1)); else RM_FAILED="$RM_FAILED; $name: $(lane_error)"; fi
  done
  CLEANED=1
}

# Interrupted or failed part-way: stop the sessions and take the lanes away, so the next run
# starts clean.
# shellcheck disable=SC2329 # run by the trap below
on_exit() {
  for pid in $PIDS; do stop "$pid" TERM; done
  if [ "$MADE" = 1 ] && [ "$CLEANED" = 0 ]; then remove_gate_lanes >/dev/null 2>&1; fi
}
trap on_exit EXIT
trap 'exit 130' INT TERM

# Start a command in the background, in its own process group, so stopping it stops whatever it
# started too. Adds its pid to $PIDS.
start() {
  set -m
  "$@" &
  PIDS="$PIDS $!"
  set +m
}

stop() { # pid signal
  kill -"$2" -- "-$1" 2>/dev/null || kill -"$2" "$1" 2>/dev/null
}

# One Claude Code session, as `claude -w <name> -p <prompt>` from the repo. Its hooks run `lane`
# by name, so they get the same one as the gate.
# shellcheck disable=SC2329 # run through start
session() { # name prompt
  cd "$REPO_ROOT" || exit 1
  PATH="$(dirname "$LANE"):$PATH"
  export PATH
  exec "$CLAUDE" -w "$1" --permission-mode acceptEdits --tools Read,Edit --output-format json -p "$2" \
    </dev/null >"$LOG_DIR/$1.json" 2>"$LOG_DIR/$1.err.txt"
}

# With --no-agents: `lane new <name>` from the repo.
# shellcheck disable=SC2329 # run through start
new_lane() { # name
  cd "$REPO_ROOT" || exit 1
  exec "$LANE" new "$1" --json </dev/null >"$LOG_DIR/$1.json" 2>"$LOG_DIR/$1.err.txt"
}

# Wait for every pid in $PIDS until the deadline; a pid still running then is stopped. Sets
# CODE_<n> (exit code, or "timeout") for the nth pid.
wait_all() { # deadline (in $SECONDS)
  while :; do
    running=0
    for pid in $PIDS; do kill -0 "$pid" 2>/dev/null && running=1; done
    [ "$running" = 0 ] && break
    if [ "$SECONDS" -ge "$1" ]; then
      for pid in $PIDS; do kill -0 "$pid" 2>/dev/null && { : > "$LOG_DIR/timeout-$pid"; stop "$pid" TERM; }; done
      sleep 3
      for pid in $PIDS; do stop "$pid" KILL; done
      break
    fi
    sleep 1
  done
  n=0
  for pid in $PIDS; do
    n=$((n + 1))
    wait "$pid" 2>/dev/null; code=$?
    [ -e "$LOG_DIR/timeout-$pid" ] && code=timeout
    eval "CODE_$n=\$code"
  done
  PIDS=''
}

# ---------- the gate ----------

GIT_BIN=$(command -v git 2>/dev/null)
LANE=$(find_program "$LANE_ARG" lane)
[ "$NO_AGENTS" = 1 ] || CLAUDE=$(find_program "$CLAUDE_ARG" claude)
problems=''
versions=''
[ -n "$GIT_BIN" ] || problems="$problems; git isn't on PATH: install Git"
[ -n "$LANE" ] || problems="$problems; no lane program found: install Lanes (lanes.autter.dev/download) or pass --lane <path>"
[ -d "$REPO" ] || problems="$problems; $REPO isn't a folder: pass --repo <path to a clean checkout>"
if [ -z "$problems" ]; then
  REPO_ROOT=$(cd "$REPO" && pwd -P)
  if LANE_VERSION=$(cd "$REPO_ROOT" && "$LANE" --version 2>&1); then
    versions="$LANE_VERSION ($LANE)"
  else
    problems="$problems; $LANE --version failed: $LANE_VERSION"
  fi
  (cd "$REPO_ROOT" && "$LANE" savings --help >/dev/null 2>&1) \
    || problems="$problems; this lane has no \`lane savings\`: install a Lanes that has it, or pass --lane <path>"
  versions="$versions, $(git --version)"
  if [ "$NO_AGENTS" = 0 ]; then
    if [ -z "$CLAUDE" ]; then
      problems="$problems; no claude program found: install Claude Code or pass --claude <path>"
    elif v=$(cd "$REPO_ROOT" && "$CLAUDE" --version 2>&1 </dev/null); then
      versions="$versions, claude $v ($CLAUDE)"
    else
      problems="$problems; $CLAUDE --version failed: $v"
    fi
  fi
fi
if [ -n "$problems" ]; then
  add_check tools false "${problems#; }"
  finish 'Gate not run: fix what failed above, then run it again.'
fi
add_check tools true "$versions"

# The repo: its main checkout.
if ! git -C "$REPO_ROOT" rev-parse --show-toplevel >/dev/null 2>&1; then
  add_check clean false "$REPO_ROOT isn't a Git repo: pass --repo <path to a clean checkout>"
  finish 'Gate not run: fix what failed above, then run it again.'
fi
REPO_ROOT=$(worktrees | head -n 1)

# 1. Claude Code is in lanes on this machine: `lane doctor` says so, and its settings have the
# four hooks `lane setup` adds.
lane_json doctor
doctor=$OUT
problems=''
hooks_status=$(printf '%s' "$doctor" | jq -r '.checks[]? | select(.id == "agent-hooks") | .status' 2>/dev/null)
hooks_summary=$(printf '%s' "$doctor" | jq -r '.checks[]? | select(.id == "agent-hooks") | .summary' 2>/dev/null)
path_status=$(printf '%s' "$doctor" | jq -r '.checks[]? | select(.id == "lane-path") | .status' 2>/dev/null)
path_summary=$(printf '%s' "$doctor" | jq -r '.checks[]? | select(.id == "lane-path") | .summary' 2>/dev/null)
case "$hooks_status" in
  '') problems="$problems; lane doctor --json has no agent-hooks check ($(lane_error))" ;;
  ok|warn)
    not_set_up=$(printf '%s' "$hooks_summary" | sed -n 's/^\(.*\) not set up.*/\1/p' | awk -F';' '{ print $NF }')
    case "$not_set_up" in *'Claude Code'*) problems="$problems; lane doctor: $hooks_summary" ;; esac ;;
  *) problems="$problems; lane doctor: agents in lanes: $hooks_summary" ;;
esac
if [ -n "$path_status" ] && [ "$path_status" != ok ]; then problems="$problems; lane doctor: $path_summary"; fi
settings=$(claude_settings)
missing=''
for pair in WorktreeCreate:worktree-create WorktreeRemove:worktree-remove SessionStart:session-start PreToolUse:pre-tool-use; do
  event=${pair%%:*} hook=${pair#*:}
  found=0
  if [ -f "$(upath "$settings")" ]; then
    found=$(jq -r --arg e "$event" --arg c "lane hook claude $hook" \
      '[.hooks[$e][]?.hooks[]?.command // empty | select(contains($c))] | length' "$(upath "$settings")" 2>/dev/null)
  fi
  [ "${found:-0}" -gt 0 ] || missing="$missing, $event"
done
[ -z "$missing" ] || problems="$problems; Claude Code's settings ($settings) have no Lanes hook for ${missing#, }"
if [ -n "$problems" ]; then
  add_check setup false "${problems#; }. Run \`lane setup --yes\`, then run the gate again."
else
  add_check setup true "lane doctor: $hooks_summary; Claude Code's hooks are in $settings"
fi

# 2. The repo is clean.
dirty=$(git -C "$REPO_ROOT" status --porcelain 2>&1); status_code=$?
HEAD=$(git -C "$REPO_ROOT" rev-parse HEAD)
SHORT=$(printf '%s' "$HEAD" | cut -c1-7)
branch=$(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD)
if [ "$status_code" != 0 ]; then
  add_check clean false "git status failed in $REPO_ROOT: $dirty"
elif [ -n "$dirty" ]; then
  n=$(printf '%s\n' "$dirty" | wc -l | tr -d ' ')
  if [ "$n" = 1 ]; then what='1 changed or untracked file'; else what="$n changed or untracked files"; fi
  add_check clean false "$what in $REPO_ROOT: commit or stash them (or use a fresh clone), then run the gate again"
else
  add_check clean true "$REPO_ROOT on $branch at $SHORT, nothing to commit"
fi

# 3. Nothing else called gate-*: no lane, no worktree, no folder where the lanes go.
WORKTREES_BEFORE=$(worktrees)
found=''
lane_json list
if [ "$CODE" != 0 ]; then found="$found, lane list failed: $(lane_error)"; fi
for name in $(printf '%s' "$OUT" | jq -r '.lanes[]?.name | select(startswith("gate-"))' 2>/dev/null); do
  found="$found, the lane $name"
done
while IFS= read -r w; do
  [ -n "$w" ] || continue
  case "$(basename "$w")" in gate-*) found="$found, the worktree $w" ;; esac
done <<EOF
$WORKTREES_BEFORE
EOF
dirs=$( { printf '%s.lanes\n' "$REPO_ROOT"; printf '%s\n' "$WORKTREES_BEFORE" | sed 1d | while IFS= read -r w; do dirname "$w"; done; } | sort -u)
while IFS= read -r d; do
  [ -n "$d" ] && [ -d "$(upath "$d")" ] || continue
  for f in "$(upath "$d")"/gate-*; do
    [ -d "$f" ] || continue
    case "$found" in *"$(basename "$f")"*) ;; *) found="$found, the folder $f" ;; esac
  done
done <<EOF
$dirs
EOF
if [ -n "$found" ]; then
  add_check no-gate false "found ${found#, }: remove them (\`lane rm <name>\`, or delete the folder), then run the gate again"
else
  add_check no-gate true 'no lane, worktree or folder called gate-*'
fi

FILES=$(gate_files)
if [ "$(printf '%s\n' "$FILES" | grep -c .)" -lt 5 ]; then
  add_check files false "the repo needs five small tracked text files to edit (README.md files are best); found $(printf '%s\n' "$FILES" | grep -c .)"
fi
if [ "$(jq -s '[.[] | select(.passed | not)] | length' "$CHECKS")" != 0 ]; then
  finish 'Gate not run: fix what failed above, then run it again.'
fi
file_for() { printf '%s\n' "$FILES" | sed -n "${1}p"; }

SAVINGS_BEFORE=$(savings_counts)

# Make the five lanes: five Claude Code sessions at once, or five `lane new`s and an edit.
MADE=1
t0=$SECONDS
deadline=$((SECONDS + TIMEOUT))
problems=''
if [ "$NO_AGENTS" = 0 ]; then
  say "starting five Claude Code sessions in $REPO_ROOT (claude -w gate-1 ... gate-5); this can take a few minutes"
  i=0
  for name in $NAMES; do
    i=$((i + 1))
    prompt="Add one line at the end of $(file_for $i): a comment, in that file's comment syntax, that says edited in lane $name. Change nothing else, then reply with the line you added."
    start session "$name" "$prompt"
  done
  wait_all "$deadline"
  i=0
  for name in $NAMES; do
    i=$((i + 1))
    eval "code=\$CODE_$i"
    if [ "$code" = timeout ]; then problems="$problems; $name was still running after $TIMEOUT s"
    elif [ "$code" != 0 ]; then problems="$problems; $name exited with $code (see $LOG_DIR/$name.err.txt)"
    elif [ "$(jq -r 'if .is_error == false then "ok" else "no" end' "$LOG_DIR/$name.json" 2>/dev/null)" != ok ]; then
      problems="$problems; $name didn't answer (see $LOG_DIR/$name.json)"
    fi
  done
  if [ -n "$problems" ]; then add_check made false "${problems#; }"
  else add_check made true "five claude -w sessions each edited a file and answered, in $((SECONDS - t0)) s"; fi
else
  say "making five lanes with lane new in $REPO_ROOT (no agents)"
  for name in $NAMES; do start new_lane "$name"; done
  wait_all "$deadline"
  i=0
  for name in $NAMES; do
    i=$((i + 1))
    eval "code=\$CODE_$i"
    path=$(jq -r '.lane.path // empty' "$LOG_DIR/$name.json" 2>/dev/null)
    if [ "$code" != 0 ] || [ -z "$path" ]; then
      OUT=$(cat "$LOG_DIR/$name.json") ERR=$(cat "$LOG_DIR/$name.err.txt") CODE=$code
      problems="$problems; lane new $name failed: $(lane_error)"
      continue
    fi
    f="$(upath "$path")/$(file_for $i)"
    if [ -s "$f" ] && [ -n "$(tail -c 1 "$f")" ]; then printf '\n' >> "$f"; fi
    printf '%s\n' "$(comment_line "$f" "$name")" >> "$f"
  done
  if [ -n "$problems" ]; then add_check made false "${problems#; }"
  else add_check made true "five lane new ran at once and the script edited a file in each, in $((SECONDS - t0)) s (no agents)"; fi
fi

# 4a. Five ready lanes, gate-1 to gate-5, in one folder, with port blocks no other lane has.
LANES_NOW=$(gate_lanes)
lane_json list --all
ALL=$OUT
problems='' ports='' LANES_DIR='' parents=''
for name in $NAMES; do
  id=$(lane_field "$name" 2)
  if [ -z "$id" ]; then problems="$problems; $name isn't in lane list"; continue; fi
  status=$(lane_field "$name" 5) agent=$(lane_field "$name" 6) path=$(lane_field "$name" 7)
  start=$(lane_field "$name" 3) count=$(lane_field "$name" 4)
  [ "$status" = ready ] || problems="$problems; $name is $status, not ready"
  if [ "$NO_AGENTS" = 0 ] && [ "$agent" != claude ]; then
    problems="$problems; $name wasn't made by Claude Code's WorktreeCreate hook (agent: ${agent:-none})"
  fi
  ports="$ports, $start-$((start + count - 1))"
  parents="$parents
$(norm "$(parent "$path")")"
  [ -n "$LANES_DIR" ] || LANES_DIR=$(parent "$path")
  # Every other lane on this machine, the other gate lanes included.
  clash=$(printf '%s' "$ALL" | jq -r --arg id "$id" --argjson s "$start" --argjson c "$count" '
    .lanes[]? | select(.id != $id)
    | select(.ports.start <= ($s + $c - 1) and $s <= (.ports.start + .ports.count - 1))
    | "\(.name) in \(.repo)"' 2>/dev/null)
  [ -z "$clash" ] || problems="$problems; $name ($start-$((start + count - 1))) shares ports with $(printf '%s' "$clash" | tr '\n' ',' | sed 's/,$//; s/,/, /g')"
done
[ "$(printf '%s\n' "$parents" | sed '/^$/d' | sort -u | wc -l | tr -d ' ')" -le 1 ] \
  || problems="$problems; the lanes aren't in one folder"
if [ -n "$problems" ]; then
  add_check lanes false "${problems#; }"
else
  by=''; [ "$NO_AGENTS" = 1 ] || by=', made by Claude Code'
  add_check lanes true "gate-1 to gate-5 ready in $LANES_DIR$by, ports ${ports#, }"
fi

# 4b. Each lane's change is in its own folder only, and the main checkout is untouched.
problems=''
i=0
for name in $NAMES; do
  i=$((i + 1))
  path=$(lane_field "$name" 7) file=$(file_for $i)
  if [ -z "$path" ]; then problems="$problems; $name is missing"; continue; fi
  changed=$(git -C "$path" status --porcelain --untracked-files=no | cut -c4-)
  if [ "$changed" != "$file" ]; then
    problems="$problems; $name changed $(printf '%s' "${changed:-nothing}" | tr '\n' ',' | sed 's/,/, /g') (expected only $file)"
    continue
  fi
  grep -Eq "$name([^0-9]|\$)" "$(upath "$path")/$file" || problems="$problems; $name's $file doesn't name the lane"
done
dirty=$(git -C "$REPO_ROOT" status --porcelain)
[ -z "$dirty" ] || problems="$problems; the main checkout changed: $(printf '%s' "$dirty" | tr '\n' ',' | sed 's/,/, /g')"
head_now=$(git -C "$REPO_ROOT" rev-parse HEAD)
[ "$head_now" = "$HEAD" ] || problems="$problems; the main checkout moved from $SHORT to $head_now"
if [ -n "$problems" ]; then
  add_check isolation false "${problems#; }"
else
  # shellcheck disable=SC2086 # one file per line, split on purpose
  add_check isolation true "each lane changed only its own file ($(join ', ' $FILES)); the main checkout is clean at $SHORT"
fi

# 4c. Git has no worktree outside the lanes folder (besides the main checkout, and any that were
# there before the gate).
if [ -z "$LANES_DIR" ]; then
  add_check worktrees false 'no lanes, so no lanes folder to check against'
else
  new='' earlier='' inside=0
  while IFS= read -r w; do
    [ -n "$w" ] || continue
    if [ "$(norm "$w")" = "$(norm "$REPO_ROOT")" ]; then continue; fi
    if under "$(upath "$w")" "$LANES_DIR"; then inside=$((inside + 1)); continue; fi
    if printf '%s\n' "$WORKTREES_BEFORE" | while IFS= read -r b; do [ "$(norm "$b")" = "$(norm "$w")" ] && echo yes; done | grep -q yes; then
      earlier="$earlier, $w"
    else
      new="$new, $w"
    fi
  done <<EOF
$(worktrees)
EOF
  if [ -n "$new" ]; then
    add_check worktrees false "worktrees outside $LANES_DIR: ${new#, }"
  else
    note=''; [ -z "$earlier" ] || note="; some from before the gate are elsewhere: ${earlier#, }"
    add_check worktrees true "the main checkout and $inside worktrees in $LANES_DIR, none elsewhere$note"
  fi
fi

# 4d. `git worktree add` from inside a session is turned down: Claude Code's PreToolUse hook,
# run the way Claude Code runs it, from inside gate-1.
cwd=$(lane_field gate-1 7)
[ -n "$cwd" ] || cwd=$REPO_ROOT
input=$(jq -nc --arg cwd "$cwd" '{session_id: "lane-gate", cwd: $cwd, hook_event_name: "PreToolUse",
  tool_name: "Bash", tool_input: {command: "git worktree add ../gate-extra"}}')
answer=$(printf '%s' "$input" | (cd "$(upath "$cwd")" && "$LANE" hook claude pre-tool-use) 2>"$LOG_DIR/stderr.txt"); code=$?
decision=$(printf '%s' "$answer" | jq -r '.hookSpecificOutput.permissionDecision // empty' 2>/dev/null)
if [ "$code" = 0 ] && [ "$decision" = deny ]; then
  reason=$(printf '%s' "$answer" | jq -r '.hookSpecificOutput.permissionDecisionReason' | sed 's/\. .*/./')
  add_check deny true "git worktree add in gate-1 is turned down: $reason"
else
  add_check deny false "\`git worktree add ../gate-extra\` wasn't turned down (exit $code, $(printf '%s %s' "$answer" "$(cat "$LOG_DIR/stderr.txt")" | tr '\n' ' '))"
fi

# 4e. `lane savings` counts the five lanes.
SAVINGS=$(savings_counts)
if [ -z "$SAVINGS_BEFORE" ] || [ -z "$SAVINGS" ]; then
  add_check savings false 'lane savings --json failed: run it in the repo to see why'
else
  # shellcheck disable=SC2086 # three numbers, split on purpose
  set -- $SAVINGS_BEFORE; now0=$1 total0=$2
  # shellcheck disable=SC2086
  set -- $SAVINGS; now1=$1 total1=$2
  if [ "$now1" = $((now0 + 5)) ] && [ "$total1" = $((total0 + 5)) ]; then
    add_check savings true "lane savings counts the five: $now1 lanes now (was $now0), $total1 in total (was $total0)"
  else
    add_check savings false "lane savings counts $now1 lanes now (expected $((now0 + 5))) and $total1 in total (expected $((total0 + 5)))"
  fi
fi

# 5. Clean up through Claude Code's WorktreeRemove hook, then check the lanes are gone.
say "removing the lanes through Claude Code's WorktreeRemove hook"
OURS=$LANES_NOW
remove_gate_lanes
problems=${RM_FAILED#; }
LANES_NOW=$(gate_lanes)
for name in $NAMES; do
  [ -z "$(lane_field "$name" 2)" ] || problems="$problems; $name is still in lane list"
done
WORKTREES_NOW=$(worktrees)
while IFS= read -r line; do
  [ -n "$line" ] || continue
  path=$(printf '%s' "$line" | awk -F "$US" '{ print $7 }')
  [ ! -e "$(upath "$path")" ] || problems="$problems; $path is still there"
  if printf '%s\n' "$WORKTREES_NOW" | while IFS= read -r w; do [ "$(norm "$w")" = "$(norm "$path")" ] && echo yes; done | grep -q yes; then
    problems="$problems; Git still has the worktree $path"
  fi
done <<EOF
$OURS
EOF
[ -z "$(git -C "$REPO_ROOT" status --porcelain)" ] || problems="$problems; the main checkout has changes"
problems=${problems#; }
if [ -n "$problems" ]; then
  add_check cleanup false "$problems. Remove what is left with \`lane rm <name>\`."
else
  how="$VIA_HOOK through Claude Code's WorktreeRemove hook"
  [ "$VIA_RM" = 0 ] || how="$how, $VIA_RM with lane rm"
  add_check cleanup true "the five lanes are gone ($how): no lane, folder or worktree left, and the main checkout is clean"
fi

# 6. Removed lanes still count in `lane savings`' total.
SAVINGS=$(savings_counts)
if [ -z "$SAVINGS_BEFORE" ] || [ -z "$SAVINGS" ]; then
  add_check savings-total false 'lane savings --json failed: run it in the repo to see why'
else
  # shellcheck disable=SC2086 # three numbers, split on purpose
  set -- $SAVINGS_BEFORE; now0=$1 total0=$2 removed0=$3
  # shellcheck disable=SC2086
  set -- $SAVINGS; now1=$1 total1=$2 removed1=$3
  if [ "$now1" = "$now0" ] && [ "$total1" = $((total0 + 5)) ] && [ "$removed1" = $((removed0 + 5)) ]; then
    add_check savings-total true "the removed lanes still count: $total1 lanes in total, $removed1 of them removed; $now1 now"
  else
    add_check savings-total false "lane savings counts $now1 lanes now (expected $now0), $total1 in total (expected $((total0 + 5))), $removed1 removed (expected $((removed0 + 5)))"
  fi
fi

finish
