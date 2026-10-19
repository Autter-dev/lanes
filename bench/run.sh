#!/usr/bin/env bash
# Benchmark 1 for Lanes: ready-to-run copies of a repo, made with `git worktree add` plus the
# repo's install, against `lane new`. macOS and Linux; bench/run.ps1 is the Windows version.
# What it measures and how to run it fairly: bench/README.md.
#
#   bench/run.sh --repo lanes --runs 2    # a quick smoke run against this repo
#   bench/run.sh                          # the full matrix: every repo in bench/repos.ini
#
# Works with the bash 3.2 that macOS ships, so no associative arrays here.
set -euo pipefail

bench_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
self="$bench_dir/run.sh"
repo_root=$(cd "$bench_dir/.." && pwd)
repos_file="$bench_dir/repos.ini"

# The protocol every published number uses. Anything else is a smoke run.
std_runs=10
std_warmup=1
std_copies="1 5 10"
std_disk_runs=3

usage() {
  cat <<'EOF'
Usage: bench/run.sh [options]

Makes 1, 5 and 10 copies of each repo in bench/repos.ini, two ways: `git worktree add` plus the
repo's install (with a warm package cache), and `lane new`. Measures wall time, disk added
(the drop in the volume's free space) and the time to the first passing test in a fresh copy,
with hyperfine. Writes bench/results/<date>-<machine>.json.

Options:
  --repo NAME       Only this repo (repeat for more). Default: every repo in bench/repos.ini
                    that runs on this OS, except `lanes` (this repo, the quick target)
  --runs N          Timed runs per measurement in the time pass (default 10; fewer marks a
                    smoke run)
  --warmup N        Warmup runs, not counted (default 1)
  --copies "LIST"   How many copies to make at once (default "1 5 10")
  --work DIR        Where the clones and copies go: on the disk you want to measure
                    (default ~/lanes-bench)
  --lane PATH       The lane binary (default: `lane` on PATH)
  --machine NAME    The machine's name in the results file (default: from the OS, CPU and
                    filesystem; never the hostname)
  --out FILE        Write the results here instead of bench/results/
  --settle SECS     Read free space once it has held still this long, within 1 MiB
                    (default 5; whole seconds)
  --no-first-test   Skip the time to the first passing test
  --no-copies       Skip the copies measurements: to check a repo's test subset, or to
                    measure only that
  --pass PASS       time (wall time, runs back to back), disk (disk added, each run on a
                    settled disk) or both (default)
  --disk-runs N     Runs per copies measurement in the disk pass (default 3; fewer marks a
                    smoke run)
  --smoke           Mark the results as a smoke run whatever the settings
  -h, --help        Show this help
EOF
}

die() {
  echo "run.sh: $1" >&2
  [[ -n ${2:-} ]] && echo "  $2" >&2
  exit 1
}

say() { echo "==> $*" >&2; }

# warn MESSAGE: to stderr, and to the measurement's warnings file, which goes in the results
# (hyperfine hides what its --prepare step prints).
warn() {
  echo "Warning: $1" >&2
  if [[ -n ${BENCH_WARNINGS:-} ]]; then echo "$1" >>"$BENCH_WARNINGS"; fi
}

# --- bench/repos.ini -------------------------------------------------------------------------

# conf SECTION KEY: the value, preferring KEY.<os> (like `test.macos`) over KEY.
conf() {
  local v
  v=$(ini_get "$1" "$2.$os")
  [[ -n $v ]] || v=$(ini_get "$1" "$2")
  printf '%s' "$v"
}

ini_get() {
  awk -v s="$1" -v k="$2" '
    { sub(/\r$/, "") }
    /^[ \t]*([#;]|$)/ { next }
    /^\[.*\][ \t]*$/ { sub(/[ \t]*$/, ""); sec = substr($0, 2, length($0) - 2); next }
    sec == s {
      i = index($0, "="); if (i == 0) next
      key = substr($0, 1, i - 1); val = substr($0, i + 1)
      gsub(/^[ \t]+|[ \t]+$/, "", key); gsub(/^[ \t]+|[ \t]+$/, "", val)
      if (key == k) { print val; exit }
    }' "$repos_file"
}

ini_sections() {
  awk '{ sub(/\r$/, "") } /^\[.*\][ \t]*$/ { sub(/[ \t]*$/, ""); print substr($0, 2, length($0) - 2) }' "$repos_file"
}

# --- Free space -----------------------------------------------------------------------------

# Free bytes on the volume holding $1. `df`, never `du`: `du` counts a cloned file at full size.
free_bytes() {
  local kb
  kb=$(df -Pk "$1" | awk 'NR == 2 { print $4 }')
  echo $((kb * 1024))
}

# settled_free DIR [FLOOR] [WINDOW]: flush, then wait until the free space holds still (within
# 1 MiB for WINDOW seconds, default $BENCH_SETTLE) and, given a FLOOR, is back up to it; print
# it. Filesystems allocate written blocks, and free deleted ones, seconds later: on a Windows Dev
# Drive, deleting a few thousand files gave nothing back for 40 quiet seconds and then all of it
# at once. So a quiet spell alone can't tell that a delete is done; getting back to the free
# space from before the copies can.
settled_free() {
  local floor=${2:-} window=${3:-${BENCH_SETTLE:-5}} tol=1048576 anchor now
  local start=$SECONDS since=$SECONDS cap=$((window * 4 > 120 ? window * 4 : 120))
  [[ -z $floor ]] || cap=900
  sync
  anchor=$(free_bytes "$1")
  now=$anchor
  while :; do
    sync
    now=$(free_bytes "$1")
    if ((now - anchor > tol || anchor - now > tol)); then
      anchor=$now
      since=$SECONDS
    fi
    if ((SECONDS - since >= window)) && { [[ -z $floor ]] || ((now >= floor)); }; then break; fi
    if ((SECONDS - start >= cap)); then
      if [[ -n $floor ]] && ((now < floor)); then
        warn "free space on $1 stayed $(((floor - now) / 1048576)) MiB short of where it was before the copies for $((cap / 60)) minutes; something else wrote to this disk."
      else
        warn "free space on $1 didn't hold still for $window s in $cap s; something else is writing to this disk."
      fi
      break
    fi
    sleep 0.5
  done
  echo "$now"
}

# --- Removing copies (hyperfine's --prepare and --cleanup) ------------------------------------

# reset_copies DIR SAMPLES [final]: remove every copy the bench made in DIR (hyperfine's
# --prepare, before every run, and --cleanup). With a SAMPLES file (the disk pass), free.before
# holds the free space read before the last run made its copies: record how much they took (the
# drop since then), remove them, and wait until the free space is back there, less a little
# slack, so their deletes never land in the next run; then, unless this is the last run, keep
# that as the next run's free.before. With SAMPLES `none`, just remove them and let the disk
# settle; with `fast` (the time pass), just remove them.
reset_copies() {
  local dir=$1 samples=${2:-none} final=${3:-} before="" after now d measuring=1
  local used=0
  [[ $samples != none && $samples != fast ]] || measuring=0
  if [[ $measuring == 1 && -f $dir/free.before ]]; then
    before=$(cat "$dir/free.before")
    after=$(settled_free "$dir")
    used=$((before - after))
    echo "$used" >>"$samples"
  fi
  rm -f "$dir/free.before"
  if [[ -d $dir/lanes ]]; then
    for d in "$dir"/lanes/bench-*; do
      [[ -e $d ]] || continue
      (cd "$dir/main" && "$BENCH_LANE" rm "$(basename "$d")" >/dev/null 2>&1) || rm -rf "$d"
    done
  fi
  if [[ -d $dir/wt ]]; then
    for d in "$dir"/wt/*; do
      [[ -e $d ]] || continue
      git -C "$dir/main" worktree remove --force --force "$d" >/dev/null 2>&1 || rm -rf "$d"
    done
  fi
  git -C "$dir/main" worktree prune
  if [[ -z $final ]]; then
    # The timed steps' logs start empty every run, so they don't grow from run to run. (Not
    # after the last run: a failure's log is read then.)
    for d in "$dir"/logs/copies-*.log "$dir"/logs/first-test-*.log; do
      if [[ -e $d ]]; then : >"$d"; fi
    done
  fi
  if [[ -n $before ]]; then
    ((used > 0)) || used=0
    now=$(settled_free "$dir" "$((before - 2 * 1048576 - used / 100))")
  elif [[ -z $final && $samples != fast ]]; then
    now=$(settled_free "$dir")
  fi
  if [[ $measuring == 1 && -z $final ]]; then echo "$now" >"$dir/free.before"; fi
}

# --- Machine details ----------------------------------------------------------------------

first_line() { "$@" 2>&1 | head -n 1 || true; }

machine_json() {
  local cpu cores mem os_name kernel fs device disk_model solid mount total
  mount=$(df -Pk "$work" | awk 'NR == 2 { print $6 }')
  total=$(($(df -Pk "$work" | awk 'NR == 2 { print $2 }') * 1024))
  kernel=$(uname -sr)
  if [[ $os == macos ]]; then
    cpu=$(sysctl -n machdep.cpu.brand_string)
    cores=$(sysctl -n hw.ncpu)
    mem=$(sysctl -n hw.memsize)
    os_name="$(sw_vers -productName) $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
    local info
    info=$(diskutil info "$mount" 2>/dev/null || true)
    fs=$(awk -F': *' '/File System Personality/ { print $2; exit }' <<<"$info")
    device=$(awk -F': *' '/Device Node/ { print $2; exit }' <<<"$info")
    disk_model=$(awk -F': *' '/Device \/ Media Name/ { print $2; exit }' <<<"$info")
    solid=$(awk -F': *' '/Solid State/ { print $2; exit }' <<<"$info")
  else
    cpu=$(awk -F': *' '/^model name/ { print $2; exit }' /proc/cpuinfo)
    [[ -n $cpu ]] || cpu=$(lscpu 2>/dev/null | awk -F': *' '/^Model name/ { print $2; exit }')
    cores=$(nproc)
    mem=$(($(awk '/^MemTotal/ { print $2 }' /proc/meminfo) * 1024))
    os_name=$(awk -F= '/^PRETTY_NAME=/ { gsub(/"/, "", $2); print $2 }' /etc/os-release 2>/dev/null || true)
    [[ -n $os_name ]] || os_name=Linux
    fs=$(findmnt -no FSTYPE -T "$work" 2>/dev/null || stat -f -c %T "$work")
    device=$(findmnt -no SOURCE -T "$work" 2>/dev/null || true)
    local parent=""
    [[ -n $device ]] && parent=$(lsblk -no PKNAME "$device" 2>/dev/null | head -n 1 || true)
    [[ -n $parent ]] || parent=$(basename "${device:-none}")
    disk_model=$(tr -s ' ' <"/sys/block/$parent/device/model" 2>/dev/null || true)
    solid=""
    if [[ -r /sys/block/$parent/queue/rotational ]]; then
      [[ $(cat "/sys/block/$parent/queue/rotational") == 0 ]] && solid=Yes || solid=No
    fi
  fi
  jq -n --arg os "$os_name" --arg kernel "$kernel" --arg arch "$(uname -m)" \
    --arg cpu "$cpu" --argjson cores "$cores" --argjson mem "$mem" \
    --arg fs "$fs" --arg mount "$mount" --argjson total "$total" --arg device "$device" \
    --arg model "$disk_model" --arg solid "$solid" \
    --arg lane "$(first_line "$BENCH_LANE" --version)" \
    --arg git "$(first_line git --version)" --arg hyperfine "$(first_line hyperfine --version)" \
    --arg name "$machine" \
    '{name: $name, os: $os, kernel: $kernel, arch: $arch,
      cpu: {model: $cpu, logical_cores: $cores}, memory_bytes: $mem,
      disk: {filesystem: $fs, mount: $mount, size_bytes: $total, device: $device,
             model: $model, solid_state: $solid},
      lanes_version: $lane, git_version: $git, hyperfine_version: $hyperfine}'
}

# A name for the results file from the OS, CPU and filesystem: never the hostname.
machine_slug() {
  local cpu fs
  if [[ $os == macos ]]; then
    cpu=$(sysctl -n machdep.cpu.brand_string)
    fs=$(diskutil info "$(df -Pk "$work" | awk 'NR == 2 { print $6 }')" 2>/dev/null |
      awk -F': *' '/Type \(Bundle\)/ { print $2; exit }')
  else
    cpu=$(awk -F': *' '/^model name/ { print $2; exit }' /proc/cpuinfo)
    [[ -n $cpu ]] || cpu=$(lscpu 2>/dev/null | awk -F': *' '/^Model name/ { print $2; exit }')
    fs=$(findmnt -no FSTYPE -T "$work" 2>/dev/null || stat -f -c %T "$work")
  fi
  printf '%s-%s-%s' "$os" "$cpu" "${fs:-unknown}" |
    tr '[:upper:]' '[:lower:]' | sed -E 's/\((r|tm)\)//g; s/(cpu|processor|with radeon graphics)//g;
      s/[^a-z0-9]+/-/g; s/^-+|-+$//g' | cut -c 1-60 | sed -E 's/-+$//'
}

# --- Statistics -----------------------------------------------------------------------------

# Median, and p95 by linear interpolation between the closest ranks (numpy's default). Never
# the best run. run.ps1 computes the same.
# shellcheck disable=SC2016 # jq code, not shell
jq_stats='def stats: sort as $s | length as $n |
  if $n == 0 then null else
  { n: $n,
    median: (if $n % 2 == 1 then $s[($n - 1) / 2 | floor] else ($s[$n / 2 - 1] + $s[$n / 2]) / 2 end),
    p95: ((0.95 * ($n - 1)) as $r | ($r | floor) as $lo | ($r | ceil) as $hi
          | $s[$lo] + ($s[$hi] - $s[$lo]) * ($r - $lo)),
    runs: . } end;
def wall_times: .results[0] | (.times // (.measurements | map(.time_wall_clock.value)));'

# --- Step scripts: exactly what hyperfine times ---------------------------------------------

# env_lines "K=V K2=V2": export lines for a step script. These are a repo's defaults: a variable
# already set in your shell wins (a database URL with your own password, say).
env_lines() {
  local kv
  for kv in $1; do
    # shellcheck disable=SC2016 # ${K-default} is for the step script to expand
    printf 'export %s=${%s-%q}\n' "${kv%%=*}" "${kv%%=*}" "${kv#*=}"
  done
}

# write_step FILE LOG BODY: a step script that runs BODY in the repo's main checkout.
write_step() {
  {
    echo '#!/usr/bin/env bash'
    echo "# Made by bench/run.sh. hyperfine times this script."
    echo 'set -eu'
    printf 'exec >>%q 2>&1\n' "$2"
    printf 'export LANE_STATE_DIR=%q LANE_CONFIG_DIR=%q LANE_TELEMETRY=0 DO_NOT_TRACK=1\n' \
      "$LANE_STATE_DIR" "$LANE_CONFIG_DIR"
    env_lines "$repo_env"
    printf 'cd %q\n' "$dir/main"
    printf '%s\n' "$3"
  } >"$1"
}

# make_copies METHOD N [VARIANT_ENV]: shell lines that make N copies named bench-1..N.
make_copies() {
  local method=$1 n=$2 i lines=""
  for ((i = 1; i <= n; i++)); do
    lines+=$(copy_lines "$method" "bench-$i" "${3:-}")$'\n'
  done
  printf '%s' "$lines"
}

# copy_lines METHOD NAME [VARIANT_ENV]: make one copy, ready to run.
copy_lines() {
  if [[ $1 == worktree ]]; then
    printf 'git worktree add --detach --quiet %q %s\n' "$dir/wt/$2" "$commit"
    printf '(cd %q && %s%s)\n' "$dir/wt/$2" "$(env_prefix "${3:-}")" "$install"
  else
    printf '%q new %q --json\n' "$BENCH_LANE" "$2"
  fi
}

# env_prefix "K=V ...": a variant's settings, which always apply.
env_prefix() {
  local kv out=""
  for kv in $1; do out+="export ${kv%%=*}=$(printf '%q' "${kv#*=}") && "; done
  printf '%s' "$out"
}

copy_path() {
  if [[ $1 == worktree ]]; then echo "$dir/wt/$2"; else echo "$dir/lanes/$2"; fi
}

# --- Measuring ------------------------------------------------------------------------------

# shell_cmd CMD: CMD for hyperfine's --prepare and --cleanup, which run in sh (cmd.exe under Git
# Bash, where a line starting with a quote loses its first and last quote: so add a pair).
shell_cmd() {
  if [[ $os == windows ]]; then printf '"%s"' "$1"; else printf '%s' "$1"; fi
}

# measure LABEL STEP MODE JSON [MAY_FAIL]: run STEP with hyperfine, removing the copies before
# every run. MODE time: $warmup and $runs runs back to back, for wall time. MODE disk:
# $disk_runs runs, each on a settled disk, for disk added; the free-space samples land in
# JSON.disk. A failure stops the bench, unless MAY_FAIL is 1: then it returns 1.
measure() {
  local label=$1 step=$2 mode=$3 json=$4 may_fail=${5:-0} samples=fast
  local warmup_runs=$warmup timed_runs=$runs
  rm -f "$dir/free.before"
  export BENCH_WARNINGS="$json.warnings"
  : >"$BENCH_WARNINGS"
  if [[ $mode == disk ]]; then
    samples="$json.disk"
    : >"$samples"
    warmup_runs=0
    timed_runs=$disk_runs
  fi
  # Start on a quiet disk: a longer quiet spell than between runs.
  settled_free "$dir" "" $((BENCH_SETTLE * 4)) >/dev/null
  say "$repo: $label"
  if ! hyperfine -N --warmup "$warmup_runs" --runs "$timed_runs" --export-json "$json" \
    --command-name "$label" \
    --prepare "$(shell_cmd "\"$bash_bin\" \"$self\" __reset \"$dir\" \"$samples\"")" \
    --cleanup "$(shell_cmd "\"$bash_bin\" \"$self\" __reset \"$dir\" \"$samples\" final")" \
    "\"$bash_bin\" \"$step\""; then
    reset_copies "$dir" none final || true
    echo "The last lines of $logs/$(basename "$step" .sh).log:" >&2
    tail -n 40 "$logs/$(basename "$step" .sh).log" >&2 || true
    if [[ $may_fail == 1 ]]; then
      say "$repo: $label failed; the results say so, and the bench goes on."
      return 1
    fi
    die "$repo: $label failed" "Fix the command in bench/repos.ini, or run $step by hand to see why."
  fi
}

# failed_result METHOD VARIANT STEP JSON: a first test that didn't pass in a fresh copy, for the
# results: no time, and the end of its log (lines cut to 300 characters: some tools print whole
# scripts in their errors).
failed_result() {
  local warn_file="$4.warnings"
  [[ -f $warn_file ]] || warn_file=/dev/null
  jq -n --arg method "$1" --arg variant "$2" --rawfile warn "$warn_file" \
    --arg err "$(tail -n 25 "$logs/$(basename "$3" .sh).log" 2>/dev/null | cut -c 1-300)" \
    '{measure: "first_test", method: $method,
      variant: (if $variant == "" then null else $variant end), copies: 1, failed: true,
      error: ($err | split("\n") | map(select(test("\\S")))),
      wall_seconds: null, disk_added_bytes: null,
      warnings: ($warn | split("\n") | map(select(length > 0))), hyperfine: null}'
}

# result METHOD VARIANT COPIES KIND TIME_JSON DISK_JSON: one entry of the results file, with
# wall time from the time pass and disk added from the disk pass ("" for a pass that didn't run).
result() {
  local time_json=${5:-} disk_json=${6:-} hf=/dev/null disk=/dev/null warn
  [[ -n $time_json ]] && hf=$time_json
  [[ -n $disk_json && -f $disk_json.disk ]] && disk="$disk_json.disk"
  warn=$(cat ${time_json:+"$time_json.warnings"} ${disk_json:+"$disk_json.warnings"} 2>/dev/null || true)
  jq -n --slurpfile hf "$hf" --rawfile disk "$disk" --arg warn "$warn" \
    --arg step "steps/$(basename "${time_json:-none}" .json).sh" \
    --arg method "$1" --arg variant "$2" --argjson copies "$3" --arg kind "$4" \
    "$jq_stats"'
    ($disk | split("\n") | map(select(length > 0) | tonumber)) as $d
    | {measure: $kind, method: $method,
       variant: (if $variant == "" then null else $variant end), copies: $copies,
       wall_seconds: (if ($hf | length) == 0 then null
         else ($hf[0] | wall_times | stats) + {pass: "time"} end),
       disk_added_bytes: (if ($d | length) == 0 then null else ($d | stats) + {pass: "disk"} end),
       warnings: ($warn | split("\n") | map(select(length > 0))),
       hyperfine: (if ($hf | length) == 0 then null else ($hf[0].results[0] | .command = $step) end)}'
}

# --- One repo -------------------------------------------------------------------------------

check_services() {
  local s port
  for s in $1; do
    case $s in
      postgres) port=${PGPORT:-5432} ;;
      redis) port=6379 ;;
      *) die "bench/repos.ini names an unknown service \`$s\`" "Use postgres or redis." ;;
    esac
    if ! (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
      die "$repo needs $s on localhost:$port, and nothing answers there" \
        "Start $s (bench/README.md says how), or leave $repo out with --repo."
    fi
  done
}

run_repo() {
  unset BENCH_WARNINGS
  repo=$1
  dir="$work/$repo"
  logs="$dir/logs"
  local url tool variants v n step json frag tools_json
  url=$(conf "$repo" url)
  commit=$(conf "$repo" commit)
  install=$(conf "$repo" install)
  test_cmd=$(conf "$repo" test)
  repo_env=$(conf "$repo" env)
  variants=$(conf "$repo" variants)
  [[ -n $variants ]] || variants=stock
  [[ -n $url && -n $commit && -n $install && -n $test_cmd ]] ||
    die "bench/repos.ini: [$repo] needs url, commit, install and test"
  [[ $url == . ]] && url=$repo_root

  for tool in $(conf "$repo" requires); do
    command -v "$tool" >/dev/null || die "$repo needs \`$tool\`, which isn't on PATH" \
      "Install it (bench/README.md lists what each repo needs), or leave $repo out with --repo."
  done
  check_services "$(conf "$repo" services)"

  mkdir -p "$dir" "$logs" "$dir/steps" "$dir/out"
  rm -f "$logs"/*.log "$dir"/out/*

  # The clone: shallow, at the pinned commit, kept between runs.
  if [[ ! -d $dir/main/.git ]]; then
    say "$repo: cloning $url at ${commit:0:12}"
    rm -rf "$dir/main"
    git init --quiet "$dir/main"
    git -C "$dir/main" remote add origin "$url"
  fi
  if [[ $(git -C "$dir/main" rev-parse -q --verify HEAD 2>/dev/null || true) != "$commit" ]]; then
    git -C "$dir/main" fetch --quiet --depth 1 origin "$commit"
    git -C "$dir/main" checkout --quiet --detach --force "$commit"
  fi
  # Lanes follows the pinned commit, with the same install as the worktrees. Kept out of
  # `git status`.
  {
    echo "# Written by bench/run.sh."
    echo "version = 1"
    echo "[lanes]"
    echo 'dir = "../lanes"'
    echo "base = \"$commit\""
    echo "[setup]"
    printf 'install = ["%s"]\n' "$(sed 's/\\/\\\\/g; s/"/\\"/g' <<<"$install")"
  } >"$dir/main/lane.toml"
  # Versions as the repo sees them (corepack and rustup pick theirs from files in it).
  tools_json='{}'
  for tool in $(conf "$repo" requires); do
    tools_json=$(jq --arg t "$tool" --arg v "$(cd "$dir/main" && first_line "$tool" --version)" \
      '. + {($t): $v}' <<<"$tools_json")
  done
  grep -qx lane.toml "$dir/main/.git/info/exclude" 2>/dev/null ||
    echo lane.toml >>"$dir/main/.git/info/exclude"

  # A clean start: no copies, no `_base`, so the first `lane new` pays for `_base` again. A
  # finished run leaves none; a stopped one may, and their deletes need time to finish.
  rm -f "$dir/free.before"
  if [[ -n $(find "$dir/lanes" "$dir/wt" -mindepth 1 -maxdepth 1 2>/dev/null | head -n 1) ]]; then
    say "$repo: removing what a stopped run left, then letting the disk settle"
    reset_copies "$dir" none final
    rm -rf "$dir/lanes" "$dir/wt"
    git -C "$dir/main" worktree prune
    # Their size is unknown, so no floor: a long quiet spell instead. A Dev Drive gave back a
    # deleted Astro lane well after 50 quiet seconds.
    settled_free "$dir" "" $((BENCH_SETTLE * 10 > 120 ? BENCH_SETTLE * 10 : 120)) >/dev/null
  fi
  mkdir -p "$dir/wt"

  # Warm package caches: what a careful user has today. Every variant's install runs once: the
  # first in the clone, the others each in a worktree of their own, as the baseline installs, so
  # the clone never switches layouts (pnpm won't remove node_modules without a terminal).
  local warm_dir="" first_variant=1 ok
  for v in $variants; do
    say "$repo: warming the package cache ($v): $install"
    warm_dir=""
    [[ $first_variant == 1 ]] || warm_dir="$dir/wt/warm-$v"
    first_variant=0
    ok=1
    (
      eval "$(env_lines "$repo_env")"
      cd "$dir/main"
      if [[ -n $warm_dir ]]; then
        git worktree add --detach --quiet "$warm_dir" "$commit"
        cd "$warm_dir"
      fi
      eval "$(env_prefix "$(conf "$repo" "env.$v")")$install"
    ) >"$logs/warm-$v.log" 2>&1 || ok=0
    if [[ -n $warm_dir ]]; then
      git -C "$dir/main" worktree remove --force --force "$warm_dir" >/dev/null 2>&1 || rm -rf "$warm_dir"
      git -C "$dir/main" worktree prune
    fi
    if [[ $ok == 0 ]]; then
      tail -n 40 "$logs/warm-$v.log" >&2
      die "$repo: \`$install\` failed in a plain checkout" "Fix the install in bench/repos.ini."
    fi
  done

  local frags=()

  # `_base`: the one-off cost of the first `lane new` in a repo (it installs there once). That
  # first lane stays until the end, so no delete is pending while disk is measured.
  step="$dir/steps/base.sh"
  write_step "$step" "$logs/base.log" "$(printf '%q new first --json >%q' "$BENCH_LANE" "$dir/out/base-made.json")"
  local before after
  before=$(settled_free "$dir" "" $((BENCH_SETTLE * 4)))
  say "$repo: the first lane new, which sets up _base"
  hyperfine -N --runs 1 --export-json "$dir/out/base.json" --command-name "first lane new" \
    "\"$bash_bin\" \"$step\"" || {
    tail -n 40 "$logs/base.log" >&2
    die "$repo: the first \`lane new\` failed" "Run $step by hand to see why."
  }
  after=$(settled_free "$dir")
  jq -n --slurpfile hf "$dir/out/base.json" --slurpfile made "$dir/out/base-made.json" \
    --argjson disk "$((before - after))" "$jq_stats"'
    {seconds: ($hf[0] | wall_times | .[0]), disk_added_bytes: $disk,
     clone_method: $made[0].clone.method, filesystem: $made[0].clone.filesystem,
     installed: $made[0].base.installed,
     note: "One run: the first lane new in the repo makes _base, installs there, then makes the lane. Its disk is both."}' \
    >"$dir/out/base-result.json"

  # Disk added and wall time for N copies at once. The disk pass goes first: the time pass
  # doesn't wait for its deletes to settle, and they'd land in a disk run's numbers.
  local counts=("${copy_counts[@]}") plan=() item
  [[ $copies_on == 1 ]] || counts=()
  for n in ${counts[@]+"${counts[@]}"}; do
    for v in $variants lane; do
      local method=worktree variant=$v venv=""
      if [[ $v == lane ]]; then method=lane variant=""; else venv=$(conf "$repo" "env.$v"); fi
      write_step "$dir/steps/copies-$v-$n.sh" "$logs/copies-$v-$n.log" "$(make_copies "$method" "$n" "$venv")"
      plan+=("$n|$v|$method|$variant")
    done
  done
  if [[ $pass != time ]]; then
    for item in ${plan[@]+"${plan[@]}"}; do
      IFS='|' read -r n v method variant <<<"$item"
      measure "$n x $method${variant:+ ($variant)}, disk pass" "$dir/steps/copies-$v-$n.sh" disk \
        "$dir/out/copies-$v-$n-disk.json"
    done
  fi
  if [[ $pass != disk ]]; then
    for item in ${plan[@]+"${plan[@]}"}; do
      IFS='|' read -r n v method variant <<<"$item"
      measure "$n x $method${variant:+ ($variant)}" "$dir/steps/copies-$v-$n.sh" time \
        "$dir/out/copies-$v-$n.json"
    done
  fi
  for item in ${plan[@]+"${plan[@]}"}; do
    IFS='|' read -r n v method variant <<<"$item"
    local tj="" dj=""
    [[ $pass == disk ]] || tj="$dir/out/copies-$v-$n.json"
    [[ $pass == time ]] || dj="$dir/out/copies-$v-$n-disk.json"
    frag="$dir/out/copies-$v-$n.result.json"
    result "$method" "$variant" "$n" copies "$tj" "$dj" >"$frag"
    frags+=("$frag")
  done

  # Time to the first passing test in a fresh copy: make one copy, then run the test there.
  # Part of the time pass, and last, because a copy with build output frees its space so late.
  if [[ $first_test == 1 && $pass != disk ]]; then
    for v in $variants lane; do
      local method=worktree variant=$v venv=""
      if [[ $v == lane ]]; then method=lane variant=""; else venv=$(conf "$repo" "env.$v"); fi
      step="$dir/steps/first-test-$v.sh"
      write_step "$step" "$logs/first-test-$v.log" "$(copy_lines "$method" bench-t "$venv")
cd $(printf '%q' "$(copy_path "$method" bench-t)")
$(env_prefix "$venv")$test_cmd"
      json="$dir/out/first-test-$v.json"
      frag="$dir/out/first-test-$v.result.json"
      # A test that fails in one kind of copy is a finding, not a reason to stop.
      if measure "first test, $method${variant:+ ($variant)}" "$step" time "$json" 1; then
        result "$method" "$variant" 1 first_test "$json" "" >"$frag"
      else
        failed_result "$method" "$variant" "$step" "$json" >"$frag"
      fi
      frags+=("$frag")
    done
  fi

  local variants_json='[]'
  for v in $variants; do
    variants_json=$(jq --arg v "$v" --arg env "$(conf "$repo" "env.$v")" \
      '. + [{name: $v, env: ($env | split(" ") | map(select(length > 0)))}]' <<<"$variants_json")
  done
  jq -n --arg name "$repo" --arg url "$(conf "$repo" url)" --arg commit "$commit" \
    --arg install "$install" --arg test "$test_cmd" --arg env "$repo_env" \
    --argjson variants "$variants_json" --argjson tools "$tools_json" \
    --slurpfile base "$dir/out/base-result.json" --slurpfile r <(cat "${frags[@]}") \
    '{name: $name, url: $url, commit: $commit, install: $install, test: $test,
      env: ($env | split(" ") | map(select(length > 0))),
      variants: $variants, tools: $tools,
      lane_base_setup: $base[0], results: $r}' >"$dir/out/repo.json"
  repo_results+=("$dir/out/repo.json")

  # Done: remove the first lane and `_base` too, and give the deletes time, so the next repo
  # starts on a quiet disk. No floor here: after many thousands of files made and deleted, a
  # filesystem can keep some of the space for good (ReFS did).
  unset BENCH_WARNINGS
  say "$repo: removing the lanes, then letting the disk settle"
  rm -rf "$dir/lanes" "$dir/wt"
  git -C "$dir/main" worktree prune
  settled_free "$dir" "" $((BENCH_SETTLE * 10)) >/dev/null
}

# --- Main -----------------------------------------------------------------------------------

main() {
  case $(uname -s) in
    Darwin) os=macos ;;
    Linux) os=linux ;;
    *)
      # Git Bash on Windows works well enough to test this script; real runs use run.ps1.
      [[ -n ${LANES_BENCH_ALLOW_ANY_OS:-} ]] || die "run.sh is for macOS and Linux" \
        "On Windows, run bench/run.ps1 in PowerShell."
      os=windows
      ;;
  esac

  runs=$std_runs warmup=$std_warmup copies=$std_copies
  work="$HOME/lanes-bench" lane_arg=lane machine="" out="" first_test=1 copies_on=1 smoke=""
  pass=both disk_runs=$std_disk_runs
  BENCH_SETTLE=5
  local wanted=()
  while [[ $# -gt 0 ]]; do
    case $1 in
      --repo) wanted+=("${2:?--repo needs a name}"); shift ;;
      --runs) runs=${2:?--runs needs a number}; shift ;;
      --warmup) warmup=${2:?--warmup needs a number}; shift ;;
      --copies) copies=${2:?--copies needs a list, like \"1 5 10\"}; shift ;;
      --work) work=${2:?--work needs a folder}; shift ;;
      --lane) lane_arg=${2:?--lane needs a path}; shift ;;
      --machine) machine=${2:?--machine needs a name}; shift ;;
      --out) out=${2:?--out needs a file}; shift ;;
      --settle) BENCH_SETTLE=${2:?--settle needs seconds}; shift ;;
      --no-first-test) first_test=0 ;;
      --no-copies) copies_on=0 ;;
      --pass) pass=${2:?--pass needs time, disk or both}; shift ;;
      --disk-runs) disk_runs=${2:?--disk-runs needs a number}; shift ;;
      --smoke) smoke=1 ;;
      -h | --help) usage; exit 0 ;;
      *) die "unknown option \`$1\`" "Run bench/run.sh --help to see the options." ;;
    esac
    shift
  done
  [[ $runs =~ ^[0-9]+$ && $runs -ge 1 ]] || die "--runs must be a whole number, 1 or more"
  [[ $warmup =~ ^[0-9]+$ ]] || die "--warmup must be a whole number"
  [[ $BENCH_SETTLE =~ ^[0-9]+$ ]] || die "--settle must be a whole number of seconds"
  [[ $pass == time || $pass == disk || $pass == both ]] || die "--pass takes time, disk or both"
  [[ $disk_runs =~ ^[0-9]+$ && $disk_runs -ge 1 ]] || die "--disk-runs must be a whole number, 1 or more"
  read -r -a copy_counts <<<"$copies"
  [[ ${#copy_counts[@]} -ge 1 ]] || die "--copies needs at least one number, like \"1 5 10\""
  for n in "${copy_counts[@]}"; do
    [[ $n =~ ^[0-9]+$ && $n -ge 1 ]] || die "--copies takes whole numbers, like \"1 5 10\""
  done
  export BENCH_SETTLE

  local tool
  for tool in hyperfine jq git; do
    command -v "$tool" >/dev/null || die "\`$tool\` isn't on PATH" \
      "Install it: brew install $tool (macOS), or your distribution's package (Linux)."
  done
  BENCH_LANE=$(command -v "$lane_arg" || true)
  [[ -n $BENCH_LANE ]] || die "can't find the lane binary \`$lane_arg\`" \
    "Install Lanes, or pass --lane with the path to a lane binary (cargo build --release puts one in target/release)."
  export BENCH_LANE

  mkdir -p "$work"
  work=$(cd "$work" && pwd)
  [[ $work != *[\"\$\`\\]* ]] || die "the work folder's path can't contain \", \$, \` or \\" "Pick another with --work."
  # The bash running this script runs the steps too, by a path hyperfine can start.
  bash_bin=$BASH
  if [[ $os == windows ]]; then bash_bin=$(cygpath -m "$BASH"); fi
  # The bench only ever deletes inside a work folder it made.
  if [[ ! -f $work/.lanes-bench ]]; then
    [[ -z $(ls -A "$work") ]] || die "$work isn't empty and wasn't made by the bench" \
      "Pick an empty or new folder with --work."
    echo "Made by bench/run.sh; everything here can be deleted." >"$work/.lanes-bench"
  fi

  # Lanes keeps the bench's lanes apart from yours, and sends nothing.
  export LANE_STATE_DIR="$work/.lane-state" LANE_CONFIG_DIR="$work/.lane-config"
  export LANE_TELEMETRY=0 DO_NOT_TRACK=1
  rm -rf "$LANE_STATE_DIR"
  mkdir -p "$LANE_STATE_DIR" "$LANE_CONFIG_DIR"
  if [[ -n ${CARGO_TARGET_DIR:-} ]]; then
    echo "Note: unsetting CARGO_TARGET_DIR, so each copy builds in its own target folder." >&2
    unset CARGO_TARGET_DIR
  fi
  if [[ $os == macos ]] && tmutil listlocalsnapshots / 2>/dev/null | grep -q com.apple; then
    echo "Warning: Time Machine has local snapshots, which keep deleted copies' blocks and skew" >&2
    echo "  disk added. bench/README.md says how to turn them off." >&2
  fi

  local all=() r
  while IFS= read -r r; do all+=("$r"); done < <(ini_sections)
  local selected=()
  if [[ ${#wanted[@]} -eq 0 ]]; then
    for r in "${all[@]}"; do
      [[ $r == lanes ]] && continue
      [[ " $(conf "$r" platforms) " == *" $os "* ]] && selected+=("$r")
    done
  else
    for r in "${wanted[@]}"; do
      [[ " ${all[*]} " == *" $r "* ]] || die "bench/repos.ini has no repo \`$r\`" "Pick one of: ${all[*]}."
      [[ " $(conf "$r" platforms) " == *" $os "* ]] ||
        die "$r doesn't run on $os in this benchmark" "bench/repos.ini lists where it runs."
      selected+=("$r")
    done
  fi

  local reason=""
  [[ $runs -ge $std_runs ]] || reason+="runs $runs < $std_runs; "
  [[ $warmup -ge $std_warmup ]] || reason+="warmup $warmup < $std_warmup; "
  [[ ${copy_counts[*]} == "$std_copies" ]] || reason+="copies \"${copy_counts[*]}\" instead of \"$std_copies\"; "
  [[ $pass == time || $disk_runs -ge $std_disk_runs ]] || reason+="disk runs $disk_runs < $std_disk_runs; "
  [[ -z $smoke ]] || reason+="--smoke; "
  reason=${reason%; }

  [[ -n $machine ]] || machine=$(machine_slug)
  if [[ -z $out ]]; then
    out="$bench_dir/results/$(date -u +%F)-$machine${reason:+-smoke}.json"
    local i=2 stem=${out%.json}
    while [[ -e $out ]]; do out="$stem-$i.json"; i=$((i + 1)); done
  fi

  say "Measuring in $work, ${#selected[@]} repo(s): ${selected[*]}"
  [[ -z $reason ]] || say "A smoke run ($reason): never publish these numbers."
  local started
  started=$(date -u +%FT%TZ)
  repo_results=()
  for r in "${selected[@]}"; do run_repo "$r"; done

  mkdir -p "$(dirname "$out")"
  local copies_json
  copies_json=$(printf '%s\n' "${copy_counts[@]}" | jq -s .)
  [[ $copies_on == 1 ]] || copies_json='[]'
  jq -n --argjson machine "$(machine_json)" --slurpfile repos <(cat "${repo_results[@]}") \
    --arg started "$started" --arg finished "$(date -u +%FT%TZ)" --arg reason "$reason" \
    --argjson runs "$runs" --argjson warmup "$warmup" --argjson copies "$copies_json" \
    --argjson first_test "$([[ $first_test == 1 ]] && echo true || echo false)" \
    --arg pass "$pass" --argjson disk_runs "$disk_runs" --argjson std_disk_runs "$std_disk_runs" \
    --argjson std_runs "$std_runs" --argjson std_warmup "$std_warmup" \
    --argjson std_copies "$(tr ' ' '\n' <<<"$std_copies" | jq -s .)" --arg script "bench/run.sh" \
    --arg bench_commit "$(git -C "$repo_root" rev-parse HEAD 2>/dev/null || true)" \
    '{schema: 1, benchmark: "new-copy", script: $script, bench_commit: $bench_commit,
      smoke: ($reason != ""), smoke_reason: (if $reason == "" then null else $reason end),
      started: $started, finished: $finished,
      settings: {runs: $runs, warmup: $warmup, copies: $copies, first_test: $first_test,
                 pass: $pass, disk_runs: $disk_runs},
      protocol: {runs: $std_runs, warmup: $std_warmup, copies: $std_copies,
                 disk_runs: $std_disk_runs},
      passes: "wall_seconds come from the time pass (runs back to back, copies removed between runs); disk_added_bytes from the disk pass (each run starts and ends on a settled disk)",
      stats: "median, and p95 by linear interpolation between the closest ranks; never the best run",
      disk: "disk added is the drop in the volume'"'"'s free space (df), never du",
      machine: $machine, repos: $repos}' >"$out"

  say "Results: $out"
  jq -r '.repos[] | .name as $r | .results[]
    | [$r, .measure, .method + (if .variant then " (" + .variant + ")" else "" end), (.copies | tostring),
       (if .wall_seconds then (.wall_seconds.median * 100 | round / 100 | tostring) + " s" else "failed" end),
       (if .wall_seconds then (.wall_seconds.p95 * 100 | round / 100 | tostring) + " s" else "-" end),
       (if .disk_added_bytes then (.disk_added_bytes.median / 1048576 | round | tostring) + " MiB" else "-" end)]
    | @tsv' "$out" |
    { printf 'repo\tmeasure\tmethod\tcopies\tmedian\tp95\tdisk (median)\n'; cat; } |
    if command -v column >/dev/null; then column -t -s $'\t'; else cat; fi >&2
  [[ -z $reason ]] || say "A smoke run: never publish these numbers."
}

if [[ ${1:-} == __reset ]]; then
  shift
  reset_copies "$@"
  exit 0
fi
main "$@"
