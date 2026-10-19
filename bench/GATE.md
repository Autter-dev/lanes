# The P1 gate

The gate is the last check before Lanes 0.1.0. It asks one question: can five Claude Code
agents work on one real monorepo at the same time, each in its own lane, with nobody setting
anything up by hand?

`bench/gate.ps1` (Windows) and `bench/gate.sh` (macOS and Linux) answer it. Each one starts five
`claude -w gate-N -p "..."` sessions at once on a repo you name. Each session gets a one-line
task: add a comment naming its lane to a different file. Then the script checks what happened,
removes the lanes and prints a pass or fail line for every check.

## What a pass proves

- `claude -w` lands in a lane. You don't have to run anything first. Once `lane setup` has run on
  the machine, Claude Code's own worktree flag makes a lane.
- Five lanes can be made at once in the same repo, each with its own folder and its own block
  of ports.
- An agent's edits stay in its lane. The other lanes and your main checkout don't change.
- Agents don't make Git worktrees of their own. Every worktree Git knows about is either a lane
  or your main checkout, and an agent's `git worktree add` is turned down with a pointer to
  `lane new`.
- `lane savings` counts the lanes while they exist. It still counts them after they're removed.
- Removing a lane the way Claude Code does it (its WorktreeRemove hook) leaves nothing behind:
  no folder, no worktree and no ports in use.

The gate doesn't measure speed or disk use. The benchmarks do that (`bench/README.md`).

## Run it

You need:

- Lanes installed, with `lane setup --yes` run once on this machine. That puts Claude Code's hooks
  in its user settings.
- Claude Code installed and logged in. The gate runs five short sessions on your account. Each
  one reads one file and adds one line.
- `git`. For `gate.sh`, you also need `jq` and bash 3.2 or later (macOS's own bash works).
- A clean clone of the repo, with nothing to commit. The real gate uses
  [withastro/astro](https://github.com/withastro/astro). On Windows, put it on a Dev Drive so
  lanes are clones, not copies.
- A plain terminal, not a Claude Code session.
- Nothing else making or removing lanes in that repo while the gate runs. The gate counts lanes,
  so other lanes would change the totals.

You don't need `lane init` in the repo, and you shouldn't run it there. It writes `lane.toml`, and
the gate wants a clean checkout.

Windows (PowerShell 5.1 or 7):

```powershell
git clone https://github.com/withastro/astro D:\src\astro
powershell -NoProfile -ExecutionPolicy Bypass -File bench\gate.ps1 -Repo D:\src\astro
```

macOS and Linux:

```bash
git clone https://github.com/withastro/astro ~/src/astro
bench/gate.sh --repo ~/src/astro
```

The first lane in a repo installs the repo's dependencies once, into a clean copy called `_base`.
On Astro that takes a few minutes. The gate waits for it, and so does Claude Code: it gives its
WorktreeCreate hook up to 10 minutes. Later runs reuse `_base` and finish much sooner.

### Options

| PowerShell | bash | What it does |
| --- | --- | --- |
| `-Repo <path>` | `--repo <path>` | The repo to run on. Required. |
| `-Lane <path>` | `--lane <path>` | The `lane` to use. Default: the one on PATH. Claude Code's hooks get this one too: the gate puts its folder first on their PATH. |
| `-Claude <path>` | `--claude <path>` | The `claude` to use. Default: the one on PATH. |
| `-Timeout <seconds>` | `--timeout <seconds>` | How long to wait for the five sessions. Default 1200. A session still running then is stopped, and the `made` check fails. |
| `-NoAgents` | `--no-agents` | Don't run Claude Code (see [Testing the gate](#testing-the-gate)). |
| `-Json` | `--json` | Print one JSON object with every check instead of a line per check. |

The gate never prompts. It exits 0 when every check passes, 1 when any check fails and 2 when the
arguments are wrong.

## What each check means

The gate runs the checks in this order. If any of the first four fail, it stops before making
any lanes. It prints what to fix, and nothing on the machine changes.

| Check | Passes when |
| --- | --- |
| `tools` | `git`, `lane` and `claude` run, and this `lane` has `lane savings`. |
| `setup` | `lane setup` has put Claude Code in lanes on this machine. `lane doctor --json` doesn't list Claude Code as not set up, and it finds `lane` on PATH. Claude Code's user settings (`~/.claude/settings.json`, or `$CLAUDE_CONFIG_DIR/settings.json`) also have the four Lanes hooks: WorktreeCreate, WorktreeRemove, SessionStart and PreToolUse. |
| `clean` | The repo is a Git checkout with nothing to commit: no changed or untracked files. The gate uses the repo's main checkout, even when you point it at a lane. |
| `no-gate` | Nothing is already called `gate-*`: no lane, no Git worktree, and no folder where the lanes go. A leftover from an earlier run would make the counts wrong. |
| `files` | Shown only when it fails. The gate needs five small tracked text files to edit. It picks `README.md` files first, then other small text files, shallowest first. Each session edits a different one. |
| `made` | All five sessions finished before the timeout, exited 0 and answered without an error. With `-NoAgents`: all five `lane new` runs succeeded and the script edited a file in each. |
| `lanes` | `lane list --json` shows `gate-1` to `gate-5`, all `ready`, all in one lanes folder. In a real run, each one also says it was made by Claude Code's WorktreeCreate hook (`agent: claude`). The five port blocks don't overlap each other, or any other lane on the machine. |
| `isolation` | In each lane, `git status` shows exactly one changed tracked file: the one that lane was given. That file names the lane. The main checkout's `git status` is still clean, and its HEAD hasn't moved. |
| `worktrees` | `git worktree list` shows nothing outside the lanes folder except the main checkout. Worktrees that were already there before the gate are listed in the detail, but they don't fail the check. |
| `deny` | `git worktree add` from inside a session is turned down. The gate runs Claude Code's PreToolUse hook (`lane hook claude pre-tool-use`) the way Claude Code does, with JSON on stdin and `cwd` set to the `gate-1` lane. The hook must answer `"permissionDecision": "deny"`. `gate.ps1` tries both Claude Code shell tools, Bash and PowerShell. `gate.sh` tries Bash. |
| `savings` | `lane savings --json` counts five more lanes than before the gate, both now and in total. |
| `cleanup` | The gate removed the five lanes the way Claude Code does when it's done with a worktree: through its WorktreeRemove hook (`lane hook claude worktree-remove`). It falls back to `lane rm` for any lane the hook left. Then no `gate-N` is in `lane list`, no lane folder or Git worktree is left, and the main checkout is still clean. |
| `savings-total` | After the removal, `lane savings --json` counts as many lanes now as before the gate. Its total still has the five more lanes, and five more of those are marked removed. |

When an interactive `-w` session ends, Claude Code removes its worktree, or asks first if there
are changes. A `-p` session doesn't ask and leaves its worktree in place, so the gate runs the
same WorktreeRemove hook Claude Code would have run.

Each session's prompt is:

> Add one line at the end of `<file>`: a comment, in that file's comment syntax, that says edited
> in lane gate-N. Change nothing else, then reply with the line you added.

The sessions run with `--permission-mode acceptEdits --tools Read,Edit --output-format json`.
They can read and edit files in their lane, and nothing else.

## Output

Each check prints one line, then a summary:

```text
PASS  tools         lane 0.1.0 (...), git version 2.51.0, claude 2.1.300 (Claude Code) (...)
PASS  setup         lane doctor: Claude Code put their work in lanes; Claude Code's hooks are in ...
...
PASS  savings-total the removed lanes still count: 12 lanes in total, 7 of them removed; 0 now
Gate passed: 12 of 12 checks, in 214 s. Logs: C:\Users\you\AppData\Local\Temp\lane-gate-20261019-101500
```

Progress notes (`gate: ...`) go to stderr. With `-Json` / `--json`, stdout gets exactly one
object:

```json
{
  "passed": true,
  "mode": "agents",
  "repo": "D:\\src\\astro",
  "lane": "C:\\...\\lane.exe",
  "lane_version": "lane 0.1.0 (by Autter)",
  "claude": "C:\\...\\claude.cmd",
  "logs": "C:\\...\\lane-gate-20261019-101500",
  "seconds": 214,
  "checks": [
    { "id": "tools", "passed": true, "detail": "..." }
  ]
}
```

When the gate stops before making lanes, the object also has a `note`. When the arguments are
wrong, it's just `{"passed": false, "error": "..."}`.

The logs folder has each session's JSON answer (`gate-N.json`) and its stderr
(`gate-N.err.txt`). With `-NoAgents`, it has each `lane new` result instead.

If the gate is stopped part-way, by Ctrl+C or an error, it still stops the sessions and removes
the `gate-N` lanes, so the next run starts clean.

## Testing the gate

`-NoAgents` / `--no-agents` makes the five lanes with five `lane new gate-N` runs at once,
without Claude Code. The script then edits one file in each lane and runs the same checks and
cleanup. It needs no Claude Code account. To keep your real settings out of it, give Lanes a
private home for the run:

```bash
export LANE_STATE_DIR=/tmp/gate-test/state LANE_CONFIG_DIR=/tmp/gate-test/config
export LANE_AGENTS_HOME=/tmp/gate-test/home LANE_DAEMON=0 LANE_TELEMETRY_URL=http://127.0.0.1:9/
mkdir -p /tmp/gate-test/home/.claude   # so `lane setup` finds "Claude Code" there
lane setup --yes
bench/gate.sh --repo /path/to/a/scratch/repo --no-agents
```

`LANE_AGENTS_HOME` points `lane setup`, `lane doctor` and the gate's settings check at a stand-in
home folder instead of yours. `LANE_DAEMON=0` keeps `laned` from starting. The telemetry URL is a
closed local port, so the install count goes nowhere.

The scratch repo needs a commit with at least five small text files, such as five `README.md`
files in different folders.

## If it fails

- `setup`: run `lane setup --yes`. If Claude Code keeps its settings somewhere other than
  `~/.claude`, set `CLAUDE_CONFIG_DIR` to that folder for both `lane setup` and the gate.
- `made` with a session that exited non-zero: read `gate-N.err.txt` in the logs folder. A session
  that hits the timeout on the first run is usually still waiting on `_base`'s dependency
  install. Raise `-Timeout`, or run the gate again once `_base` exists.
- `lanes` with `agent` other than `claude`: the lane wasn't made by Claude Code's WorktreeCreate
  hook. Check that `lane setup` ran for the same `claude` the gate started.
- `worktrees` with a folder under `.claude/worktrees`: Claude Code made its own worktree, which
  means its WorktreeCreate hook didn't run. See `setup`.
- `cleanup`: remove what's left with `lane rm gate-N`, then run the gate again.
