# Changelog

A running log of notable changes to Lanes, newest first. All times are UTC.

Each entry has a time, a type (**Added**, **Changed**, **Fixed**, **Removed** or **Security**) and, once committed, its commit. When a release ships, its entries move under a version heading.

## Unreleased

### 2026-10-18

- `13:59` **Added**: the install count's backend, in `services/install-count`: a Cloudflare Worker that keeps counts in Postgres (`b1f8c8e`).
  - `POST /v1/installs` accepts the five fields `lane` sends and nothing else. `DELETE /v1/installs/<id>` deletes everything kept about one install.
  - It never keeps IP addresses, user agents or request logs. Cloudflare's rate limiter allows 20 requests a minute per address.
  - It runs on any Postgres, directly or through Hyperdrive, and creates its tables on the first count. `queries.sql` has the usual questions, and the README says how to host it at `lanes-count.autter.dev`, where release builds of `lane` now send the count.
  - CI tests its SQL against a real Postgres on every push.
- `13:19` **Added**: the anonymous install count, and `lane telemetry` to see and switch it (`04e5ef2`).
  - Once Lanes is set up (the welcome or `lane setup`), and once after each upgrade, it sends exactly what the welcome lists: a random install id, the Lanes version, the OS and CPU type, and how Lanes was installed (Homebrew, npm, Scoop, winget, cargo or the install scripts). Nothing is sent before the setting is chosen.
  - Never from CI, and never with `lane telemetry off`, `LANE_TELEMETRY=0` or `DO_NOT_TRACK=1`. It's sent in the background with a 1-second limit, and no command, or anything reading its output, waits for it.
  - `lane telemetry` shows exactly what this machine sends; `lane telemetry on` and `off` switch it, and `--json` prints it for scripts.
  - Release builds send it to the service below; development builds and test runs never do.
- `12:09` **Added**: the terminal app's home screen shows your lanes live, and what they saved (`677d880`).
  - **Every lane** with its agent (Claude Code, Codex, Cursor), status (running when its app answers on its port), URL, changed files, last test run through `lane exec`, and the extra disk it takes. Narrow terminals drop the agent and tests columns.
  - **Saved by lanes:** disk as two bars on one scale, a worktree plus its own install for each lane against what the lanes really take, plus the installs lanes skipped and the time that saved. Lanes measures each new lane as it makes it, so lanes made before this show nothing rather than a guess.
  - **A status bar:** sign-in, the daemon, cores, apps running and disk saved. The daemon doesn't run yet, so the bar says so, and the screen watches Lanes' state instead, redrawing only when something changed.
  - **Keys:** `?` shows them all; `:` or Ctrl-K opens a command palette you can filter; `n` makes a lane and `d` removes one (asking first), in the background; `o` opens the lane's app, `c` copies its folder and `l` shows the log.
- `07:36` **Added**: setup does everything it can by itself and asks Y/n where the choice is yours, then says what's left.
  - **`lane init`** in a terminal walks through each step and asks before it: write `lane.toml` for your team; put this machine's agents (Claude Code, Codex, Cursor) in lanes; add or update the Git hook; add the note to `AGENTS.md`; set up Claude Code for this repo only, if not for the machine; and install dependencies now in a clean copy, so the first lane is ready in seconds. Steps already in place aren't asked about. It ends with what's left: trusting the hooks in Codex once, the `git add`, the first `claude -w`, and getting instant lanes where this disk copies them. `--yes` takes every default without asking.
  - **`lane doctor --fix`** fixes what Lanes can fix by itself, asking first: agents not in lanes yet, this repo's Git hook, and lanes whose folders have gone. What only you can do (install Git, move a repo to a Dev Drive, change PATH) keeps its steps. `--fix --yes` makes every fix without asking.
  - **The install scripts** ask at the end whether to set Lanes up now, and run `lane setup` if you say yes. `--no-setup` or `LANES_NO_SETUP=1` skips it.
  - **After the welcome,** Lanes prints what's next: `lane init` in the repo you're in, trusting Codex's hooks once, and how to get instant lanes.
  - Agents, scripts and CI are never asked anything: without a terminal, `lane init` does what it did before. `LANE_INTERACTIVE=1` asks even where Lanes can't tell it's a terminal (some Windows terminals); `LANE_INTERACTIVE=0` never asks.
- `07:22` **Changed**: the website offers 0.0.6, and the guide has a step for when something's off: `lane doctor` and `lane logs`.

## 0.0.6 (2026-10-18)

**See what Lanes did, and what could stop it.** Two new commands, `lane doctor` and `lane logs`. A small update to 0.0.5, and still a preview.

```sh
lane doctor          # what could stop lanes working, and how to fix each thing
lane logs            # what Lanes did: lanes made and removed, agents' hooks, installs
lane logs fix-auth   # only what happened to one lane
```

In this version:
- `lane doctor` checks your machine (Git, whether your disk clones files, free space), that `lane` is on PATH for agents' hooks, which agents put their work in lanes, this repo's Git hook, lanes whose folders have gone, the daemon, and recent errors. Each problem says what to do next. It exits 1 only when something must be fixed, and takes `--json`.
- `lane logs` shows what Lanes did, newest last: lanes made and removed, agents' worktrees that became lanes, copies of the repo it turned down, and dependency installs. Lanes keeps the log in a file next to its state. Inside an agent's hook, problems and install output now go there instead of into the agent's view.
- CI runs every test on Btrfs, XFS and ReFS disks too, and a lane there must be cloned, never copied.

Not yet:
- Gemini CLI, Copilot and other agents get lanes only when they make a Git worktree, through the hook `lane init` adds.
- There's no background daemon, no checkpoints or undo, no promote to a pull request, and no database per lane.

### 2026-10-18

- `03:00` **Added**: `lane doctor` checks what could stop lanes working, and says how to fix each thing. Exit code 1 when something must be fixed, and `--json` for scripts. (`d742748`)
  - Everything `lane setup` checks (Git, whether this disk clones files, disk space).
  - That `lane` is on PATH, because agents' hooks run it by name.
  - Which agents put their work in lanes, and which aren't set up yet.
  - This repo's Git hook: in place, an older copy to update, or missing (Husky and lefthook included).
  - Lanes whose folders have gone (a tool cleaned up its own worktrees) or that were left half-made, with the `lane rm` to run.
  - The daemon, and errors in the log.
- `03:00` **Added**: `lane logs [<lane>]` shows what Lanes did, newest last: lanes made and removed, agents' hooks (worktrees that became lanes, copies turned down), dependency installs and failures. Lanes keeps one line per event in `lane.log` next to its state, and moves it aside past 2 MB. Inside an agent's hook, problems and an install's own output now go to the log instead of the agent's view. `-n`, `--path` and `--json` too. (`d742748`)
- `03:00` **Changed**: CI runs every test, not only the clone tests, on its Btrfs, XFS and ReFS disks, with the test repos and their lanes on those disks. Lanes there must be cloned, never copied; macOS (APFS) is held to the same rule. (`d742748`)

### 2026-10-17

- `18:51` **Changed**: the website offers 0.0.5, and the [guide](https://lanes.autter.dev/guide/) is rewritten around agents making lanes on their own: set it up once, start your agents as usual (Claude Code, Codex, Cursor), see the lanes and run the app. Making a lane by hand and `lane init` are now optional steps.

## 0.0.5 (2026-10-17)

**Your agents put their work in lanes on their own.** Run `lane` once, and Claude Code, Codex and Cursor get a lane for each task, in every repo, ready to run. Nobody types `lane new`. Still a preview.

```sh
lane                 # once: checks your machine and puts your agents in lanes
cd your-repo
claude -w fix-auth   # or start a Codex or Cursor agent in a worktree of its own
lane list            # each agent's lane, with its ports and folder
```

In this version:
- Setup happens once per machine. The welcome (and `lane setup --yes`) adds Lanes to each agent's own user settings. It's a switch, and `--agents off` takes it back out exactly. Set up Lanes before 0.0.5? Run `lane setup` once more.
- **Claude Code:** `claude -w`, subagents and background sessions get lanes in every repo, without `lane init`. A `lanes` skill has Claude start a task in a lane of its own.
- **Codex and Cursor:** when a session starts in a worktree they made, it becomes a lane (`codex-<id>`, `cursor-<id>`), and the agent is told which lane it's in. In Codex, open `/hooks` once to trust Lanes' hooks.
- Lanes from agents are ready to run: dependencies are cloned from a clean copy, and each lane gets its own ports and `.env`. A repo doesn't need a `lane.toml` for this.
- Agents are turned down when they try to copy the repo (`git worktree add`, a `git clone` of it, `cp -r`), with a pointer to `lane new`.
- `lane init` handles repos whose Git hooks belong to Husky or lefthook.

Not yet:
- Gemini CLI, Copilot and other agents get lanes only when they make a Git worktree, through the hook `lane init` adds.
- There's no background daemon, no checkpoints or undo, no promote to a pull request, and no database per lane.

### 2026-10-17

- `18:32` **Added**: the home screen says when an agent installed here isn't in lanes yet (say, after an update from 0.0.4), and to run `lane setup`.
- `18:18` **Added**: agents get lanes on their own, with nothing to set up per repo: Claude Code, Codex and Cursor. (`dd754be`, `cc529ae`)
  - **Once per machine:** the first-run welcome and `lane setup --yes` put your agents in lanes, in each agent's own user settings. It's a switch on the welcome's last screen (`a`), on by default. `lane setup --yes --agents off` takes back exactly what was added, and nothing else in those files changes.
    - **Claude Code:** its hooks in `~/.claude/settings.json` and the `lanes` skill in `~/.claude/skills/lanes`, so `claude -w` makes a lane in every repo without `lane init`.
    - **Codex:** hooks in `~/.codex/hooks.json`. Codex makes its worktrees with Git hooks off, so when a Codex session starts in one, it becomes a lane (`codex-<id>`), ready to run, and Codex is told which lane it's in. Copies of the repo are turned down. Codex runs new hooks only once you trust them, so `lane setup` says to open `/hooks` in Codex once.
    - **Cursor:** hooks in `~/.cursor/hooks.json`. When a Cursor agent starts in a worktree Cursor made, it becomes a lane (`cursor-<id>`), ready to run, and the agent is told which lane it's in. Copies of the repo are turned down with a message the agent reads.
    - Codex and Cursor share the skill in `~/.agents/skills/lanes`.
  - **A `lanes` skill:** tells an agent to start each task in a lane of its own (`lane new <task> --print-path`, or Claude Code's EnterWorktree), to run dev servers with `lane exec`, and never to copy the repo. The Claude Code plugin carries it too.
  - **No `lane.toml` needed:** in a repo without one, Lanes uses what `lane init` would find (install steps, dev command, `.env` template), so a lane an agent makes is ready to run. A `lane.toml` still wins when there is one.
  - **Worktrees other tools make are ready to run:** the Git hook now makes each one ready, like a lane from `lane new`. Dependencies (`node_modules`, `.venv`) are cloned from the clean `_base` copy when the lockfiles match, and installed in the worktree when they don't. The lane gets its own `.env` too. `lane adopt --ready` does the same by hand. The hook `lane init` added in 0.0.4 is upgraded the next time `lane init` runs.
- `17:17` **Added**: a guide on the website, [lanes.autter.dev/guide](https://lanes.autter.dev/guide/), from install to two agents in their own lanes, in nine steps. It's written for your system: it picks macOS, Linux or Windows from your browser, with that system's commands, shell, folders and example output, and a switch to see another. It remembers your pick. On a phone it says Lanes runs on a computer. Without JavaScript, every system shows, each labelled. The nav's "Get started", the home page and the download page link to it.
- `17:15` **Added**: `lane init` sets up the post-checkout hook in repos whose Git hooks belong to Husky or lefthook, so worktrees other tools make become lanes there too. (`047f922`)
  - **Husky 9** (`core.hooksPath` is `.husky/_`): one line goes at the top of `.husky/post-checkout`, which is made if it isn't there. The file is checked in, so `lane init` says so and puts it in its `git add` step. Where `lane` isn't installed the line does nothing, and it never fails a checkout.
  - **lefthook** with a post-checkout hook: nothing is written, because lefthook moves aside hooks it didn't write and never runs them. `lane init` prints the `lanes` command to add to lefthook's config, in its format (YAML, TOML or JSON). When lefthook doesn't run post-checkout, the hook goes in `.git/hooks` as before.
  - **Any other `core.hooksPath`**, Husky 8 and older included: nothing is written, and `lane init` prints the line to add.
  - **`lane hook git post-checkout`:** what that line and that command run. A new worktree becomes a lane. Branch switches, the repo's own folder and Lanes' own worktrees are left alone.
  - **In `--json`,** an entry in `agents` has `check_in` when its file belongs in Git, and `snippet` with the text to add by hand.
- `17:15` **Added**: Claude Code is turned down when it tries to copy the repo, not only when it runs `git worktree add`. In a Lanes repo, the `pre-tool-use` hook also turns down the two below. (`047f922`)
  - **A `git clone` of this repo:** by one of its folders (`.`, `..`, a path, `cd .. && git clone app app-2`, `git -C`), by its `origin` URL however it's written (`https://`, `git@host:`, `ssh://`), by `"$(git remote get-url origin)"`, or with `gh repo clone owner/repo`. Clones of other repos, and `--bare` or `--mirror` clones, run as usual.
  - **A recursive copy of one of the repo's folders:** `cp -r`, `-R` or `-a`; `xcopy /E` or `/S`; `robocopy /E`, `/S` or `/MIR`; `Copy-Item -Recurse` and PowerShell's `cp -r`; also inside `bash -c`, `cmd /c` and `pwsh -Command`. Copies of a subfolder, and copies that leave `.git` out (`robocopy /XD .git`), run as usual.
  - **Each has its own reason**, pointing to `lane new <name> --print-path`. A copy's reason also points to `git archive` for an export.
  - Most commands still return before any Git or disk work. A session told about lanes now hears that clones and copies are out too.
- `17:01` **Changed**: the website offers 0.0.4: the download page's archives, and a 0.0.4 entry on the releases page.

## 0.0.4 (2026-10-17)

**Claude Code works in lanes.** Run `lane init` once, then `claude -w <name>` starts a session in a lane of its own, with its own ports. Still a preview.

```sh
cd your-repo
lane init            # writes lane.toml, and sets up Claude Code's hooks
claude -w fix-auth   # a new lane; the session starts in it with PORT and LANE_URL set
lane list            # fix-auth, with its ports and folder
```

In this version:
- `claude -w <name>` makes a lane and works in it. Subagents with `isolation: worktree` and background sessions get lanes too, and removing the worktree removes the lane.
- A session in a lane starts with `PORT`, `VITE_PORT` and `LANE_URL` set, and knows which lane it's in.
- If Claude Code tries `git worktree add` in a Lanes repo, it's pointed to `lane new` instead.
- `lane init` puts Claude Code's hooks in `.claude/settings.local.json`, which stays on your machine and out of Git. It also adds a Git hook that turns any other tool's new worktree into a lane, and a short note in `AGENTS.md`. `--no-agents` skips all of it.
- For every repo at once, there's a Claude Code plugin: `/plugin marketplace add Autter-dev/lanes`, then `/plugin install lanes@autter`.

Not yet:
- Other agents (Cursor, Codex, Gemini CLI, Copilot) get lanes only through the Git hook: no session context and no `git worktree add` deny.
- Repos with Git hooks under `core.hooksPath` (Husky, lefthook) don't get the Git hook yet; `lane init` says so.
- There's no background daemon, no checkpoints or undo, no promote to a pull request, and no database per lane.

### 2026-10-17

- `16:43` **Added**: Claude Code works in lanes. `claude -w fix-auth` makes a lane, and the session works in it, with no other steps once `lane init` has run. Checked end to end with Claude Code 2.1.300 on Windows. (`20d009c`)
  - **`lane hook claude <event>`:** what Claude Code's hooks run, reading its JSON on stdin.
    - **`worktree-create`:** makes the lane (or finds the one with that name) and prints its folder. Names like `Fix Auth` become `fix-auth`. Subagents with `isolation: worktree` and background sessions get lanes too.
    - **`worktree-remove`:** removes the lane and frees its ports.
    - **`session-start`:** puts `PORT`, `VITE_PORT`, `LANE_URL` and the rest in the session's environment, and tells the agent which lane it's in and how to start the app.
    - **`pre-tool-use`:** turns down `git worktree add` in a Lanes repo, pointing to `lane new <name> --print-path`.
    - Session start and the deny never stop a session: outside a Lanes repo, or if anything goes wrong, they stay silent.
  - **`lane init` sets up agents:**
    - Claude Code's hooks go in `.claude/settings.local.json`, kept out of Git, so a teammate without `lane` isn't affected. This happens only when the repo has `.claude/` or `CLAUDE.md`, or `claude` is on your PATH.
    - A Git `post-checkout` hook turns a worktree any other tool makes into a lane. An existing hook still runs after it. With `core.hooksPath` set (Husky, lefthook), nothing is written, and init says why.
    - `AGENTS.md` gets a short note on working in lanes, and `CLAUDE.md` gets an `@AGENTS.md` line.
    - Running it again changes nothing. `--no-agents` skips it all.
  - **`lane new`** copies `.claude/settings.local.json` into the lane, so a session started inside a lane has the hooks too.
  - **A Claude Code plugin** in this repo, for every repo at once: `/plugin marketplace add Autter-dev/lanes`, then `/plugin install lanes@autter`. It runs the same hooks.
- `16:28` **Changed**: the website offers 0.0.3: the download page's archives, and a 0.0.3 entry on the releases page. (`3f0efa4`)

## 0.0.3 (2026-10-17)

**Lanes now tells you what to run next.** A small update to 0.0.2, for testers.

```sh
lane new fix-auth
# Made lane fix-auth in 0.4 s ...
# Next:
#   cd ~/code/app.lanes/fix-auth                 start your agent here, so it works only in this lane
#   lane exec fix-auth -- npm run dev -- --port 4100   run the app at http://fix-auth.localhost:4100
```

In this version:
- `lane new`, `lane adopt`, `lane init`, `lane rm` and an empty `lane list` end with a short "Next:" block. It names the lane's own folder, dev command and URL. With `--json`, the same steps are in `next`, so agents can follow them too.
- The home screen lists your lanes with their URLs, and says what to do next: `lane init` in a repo that isn't set up yet, `lane new` in one with no lanes.
- The welcome's last screen no longer calls `lane init` and `lane new` "soon".
- `lane --help` starts you off in three commands.
- The download page puts the quickest install for your system at the top.

### 2026-10-17

- `16:18` **Added**: every command that changes something ends by saying what to run next. (`f489625`)
  - **After `lane new` and `lane adopt`:** `cd` into the lane to start your agent there, and `lane exec <name> -- <dev command>` with the port written out, and the URL it serves.
  - **After `lane init`:** `lane new fix-auth`, and `git add lane.toml` so teammates get the same lanes.
  - **After `lane rm`, or an empty `lane list`:** `lane list`, or how to make the first lane.
  - **A repo without `lane.toml`** is told to run `lane init` first.
  - **With `--json`:** the same steps are in `next`, as `{"run", "why"}`.
- `16:18` **Added**: the home screen lists the lanes of the repo you're in (every lane outside a repo), with their URLs and state. It also gives one next step: set up the repo, make the first lane, or run the app in one. (`f489625`)
- `16:18` **Fixed**: the welcome's "You're set" screen marked `lane init` and `lane new` as "soon", though both work since 0.0.2. (`f489625`)
- `16:18` **Fixed**: the README and the website said to open `http://fix-auth.localhost`. Until the daemon serves lane URLs, the address needs the lane's port, like `http://fix-auth.localhost:4100`. The README's command table now splits what works from what's coming. `lane exec --help` quotes `'$PORT'`, so your own shell leaves it for Lanes. (`f489625`)
- `16:18` **Changed**: the download page starts with the quickest install for your system, then each system's options. On Windows the PowerShell script comes first, with nothing to install beforehand. The note about unsigned builds moved below the archives, where its one extra step applies. (`f489625`)
- `15:25` **Changed**: the website offers 0.0.2: the download page's archives, and a 0.0.2 entry on the releases page above 0.0.1.

## 0.0.2 (2026-10-17)

**The lane commands work: try them on your own repo.** Still a preview: agents don't land in lanes on their own yet, which comes next.

```sh
cd your-repo
lane init                        # writes lane.toml from what it finds
lane new fix-auth                # an instant, ready-to-run copy with its own ports
lane exec fix-auth -- npm run dev
lane list
lane rm fix-auth
```

In this version:
- `lane new <name>` makes a lane from a clean copy of `main`, with dependencies installed once.
  - It's a real Git worktree: `git status` and `git diff` work there, and no branch is created.
  - Files are copy-on-write clones on APFS, Btrfs, XFS and Windows Dev Drives, and plain copies elsewhere, with a note saying so.
- `lane exec`, `lane env`, `lane list`, `lane rm` and `lane adopt` all work.
  - Each lane gets its own `PORT`, `VITE_PORT` and `LANE_URL` (`http://<name>.localhost:<port>`), so two lanes can run the same dev server at once.
- `lane init` finds your package manager, dev command and `.env` template, and writes `lane.toml`.
- Every command has `--json` and stable exit codes. Writes take `--request-id`, so a retried command never makes a second lane.

Not yet:
- Agents don't land in lanes on their own: start them inside a lane's folder for now.
- There's no background daemon, no checkpoints or undo, and no promote to a pull request.
- There's no database or Compose project per lane.
- `*.localhost` URLs work in browsers and curl, but not in every Windows tool.

### 2026-10-17

- `15:07` **Added**: lanes run things. Every command in the CLI now works.
  - **`lane exec [<name>] -- <command>`:** runs a command in a lane with its own ports and URL. `PORT`, `VITE_PORT`, `LANE_URL`, `LANE_NAME`, `LANE_ID`, `LANE_PORTS` and `LANE_PATH` are set.
    - `$PORT`, `${PORT}` and `%PORT%` in arguments are filled in without a shell.
    - It exits with the command's own code.
    - On Windows it finds npm-style `.cmd` and `.ps1` launchers.
    - Inside a lane, the name is optional.
    - Two lanes ran the same dev server at once on ports 4100 and 4110.
  - **`lane env [<name>]`:** prints those variables for sh, PowerShell, cmd or dotenv, or as `--json`.
  - **`lane init`:** looks at the repo and writes a commented `lane.toml`, running and installing nothing. `--print` and `--force` are there too.
    - It finds the package manager (pnpm, npm, Yarn, Bun, uv, Poetry, Bundler, Go, Cargo), the dev command and its usual port, the `.env` template, a Compose file and migration tools (Prisma, Drizzle, Rails, Django, Alembic).
    - Frameworks that don't read `PORT` get `--port $PORT`.
  - **The lane's `.env`:** a lane gets one from `[env] template`, with its own `PORT` and `LANE_URL` and every other line kept.
  - **`[dev] command`** in `lane.toml`.
- `14:21` **Added**: the lane engine: `lane new`, `lane rm` and `lane adopt` work.
  - **`lane new <name>`:** makes a lane from a clean `_base` copy of the base branch (`main`), with dependencies installed once from `[setup] install` in `lane.toml`.
    - Files are cloned where the disk can, and copied otherwise, with a note saying so.
    - The lane is a real Git worktree on a detached HEAD: `git status` and `git diff` work there at once, and no branch is created.
    - Each lane gets its own block of ports.
    - `_base` follows `main`, and reinstalls only when a lockfile or the install commands change.
    - Python virtualenv paths are pointed at the lane.
    - `--print-path` prints just the folder, for agent hooks.
  - **`lane rm <name>`:** removes the folder and worktree record and frees the ports. `--keep-files` keeps the folder.
  - **`lane adopt [<path>]`:** turns a worktree you already have into a lane.
  - **Retries and failures:** all three take `--json` and `--request-id`. A failed `lane new` removes everything it made, so a retry starts clean.
  - **`lane list`:** shows each lane's state, and works from inside a lane too.
- `14:05` **Added**: clone backends, the part of `lane new` that copies files.
  - **How it copies:** every file is a copy-on-write clone where the disk supports it (APFS, Btrfs, XFS, and ReFS on a Windows Dev Drive), and a plain parallel copy everywhere else (NTFS, ext4, …), behind one `CloneBackend` trait.
  - **Probe and report:** a probe tests each folder for real, and the report counts cloned, copied and empty files separately, so Lanes never claims a clone it didn't get.
  - **Faithful copies:** modification times and permissions are kept, so builds don't redo everything, and symlinks stay symlinks.
  - **Measured on a 13,826-file tree** (this repo plus the website's `node_modules`, 247 MB):
    - Windows Dev Drive: about 2.7 s per lane, every file cloned, about 120 MB of disk per lane;
    - NTFS plain copy: 7.6 s and a full copy.
  - **New CI job:** runs the clone tests on Btrfs and XFS loop disks and a ReFS virtual disk; APFS is covered by the macOS job.
- `13:33` **Added**: the foundations the lane engine builds on, plus `lane list`.
  - **Errors:** one error type for every command. Each has a stable `code`, its exit code and a hint saying what to do next.
  - **`--json` contract:** exactly one JSON object on stdout, the result or `{"error": {code, message, hint, exit}}`, and nothing else. Unbuilt and unknown commands answer the same way.
  - **`lane.toml` v1:** `[lanes]`, `[ports]`, `[setup]`, `[env]` and `[cleanup]`. A missing file means the defaults; typos and files from a newer Lanes are refused with what to fix.
  - **State store:** per-user SQLite (WAL mode) with a single-writer lock, so port blocks never clash across repos. Reads never create files.
  - **`--request-id` / `LANE_REQUEST_ID`:** a retried write returns the first result instead of running twice.
  - **`lane list`:** shows this repo's lanes (`--all` for every repo) with their ports, and has `--json`.
  - The minimum Rust version is now 1.89.
- `13:25` **Changed**: `AGENTS.md` names the anonymous install count as the one exception to "no network calls unless the user ran `lane login`". It's on by default, opt-out with `lane telemetry off`, `LANE_TELEMETRY=0` or `DO_NOT_TRACK=1`, and never sent from CI. Nothing sends it yet.
- `12:34` **Added**: the download page says up front that builds aren't code-signed yet, and what that means on each system. macOS needs one `xattr` command, only for archives downloaded in a browser. Windows 11 with Smart App Control on may block `lane.exe` (WSL2 works meanwhile). Linux needs nothing extra.
- `11:47` **Added**: an "All releases" page at lanes.autter.dev/download/releases. It lists every version, newest first, with its summary, the six archives and their checksums, the installers, a link to the release notes, and the command to install that exact version. The download page has a "Past releases" list linking to it, and the footer links it too. On phones, file tables stack one file per row.
- `11:42` **Added**: Lanes is on npm: `npm install -g @autter/lanes` installs only the build for your system, with signed provenance. A new `npm` workflow publishes an existing release from its own archives, and the installers check now installs through every live channel (both scripts, Homebrew, Scoop and npm) on macOS, Linux and Windows. The download page lists npm as live; winget is still waiting for Microsoft's review.
- `11:19` **Added**: Lanes 0.0.1 is out as a preview. It installs with `brew install autter-dev/tap/lanes`, with Scoop from the Autter bucket, with the one-line scripts (`curl -fsSL https://lanes.autter.dev/install.sh | sh`, `irm https://lanes.autter.dev/install.ps1 | iex`), or from GitHub Releases. Each one was checked by a real install on macOS, Linux and Windows. The download page and the README show the live options; npm and winget are marked "coming soon" until they're published. A new installers check reinstalls every release the way users do.

## 0.0.1 (2026-10-17)

**The first preview.** In this version:
- `lane` opens the first-run welcome, then the home screen.
- `lane setup` checks your machine, walks you through fixes, and saves your settings (also with `--json`).
- `--help` and `--version` work.
- The lane commands (`init`, `new`, `list`, `exec`, `rm`) are listed but not built yet, and exit with code 3.

It runs on macOS, Linux and Windows, on x64 and arm64.

### 2026-10-17

- `10:26` **Added**: release pipeline and a download page. (`81fe630`)
  - **Release workflow:** pushing a `v<version>` tag builds macOS, Linux and Windows on x64 and arm64, and publishes a GitHub release. It includes `lanes-<version>-<target>` archives, SHA-256 checksums, build attestations, and one-line installers (`lanes-installer.sh`, `lanes-installer.ps1`) with the version stamped in.
  - **Channel files:** the same run renders the Homebrew formula, the Scoop manifest and the winget manifests. Once enabled, it also publishes `@autter/lanes` to npm, with one package per platform so npm downloads only yours.
  - **Tap and bucket:** starting files for the Homebrew tap and the Scoop bucket in `packaging/seeds/`. Each copies its file from the latest release and tests an install.
  - **Website:** a download page at lanes.autter.dev/download that puts your system first, with every channel, the archives and checksums, how to verify a file, and how to uninstall. The home page's install command now matches your system, and short URLs `lanes.autter.dev/install.sh` and `/install.ps1` point at the latest installers.
- `09:51` **Added**: the website, [lanes.autter.dev](https://lanes.autter.dev), is linked from the top of the README and from the repo's About box. (`22e5ab1`)
- `09:37` **Changed**: install commands now name the six channels Lanes ships on, called `lanes` everywhere: `brew install autter-dev/tap/lanes`, `winget install Autter.Lanes`, `scoop install lanes` (from the Autter bucket), `npm install -g @autter/lanes`, one-line installers for macOS, Linux and Windows, and archives on GitHub Releases. Each installs the same `lane` command. `cargo install` is no longer listed. On phones, the website's install command now wraps only between words. (`a8c012d`)
- `06:06` **Fixed**: CI is green again. The website stays on TypeScript 6 (`@astrojs/check` doesn't support 7 yet, so `npm ci` failed), Dependabot skips TypeScript major versions until it does, and clippy no longer fails on macOS over a cast that's only redundant there. (`202ef78`)
- `05:52` **Added**: guided fixes on the "Your machine" step. Each `!` or `✗` that has a fix shows one (`↑↓` to choose, `f` to open), as plain numbered steps written for this machine: move the repo to the Dev Drive you already have (with the exact `git clone` command), or create one (`o` opens Settings › Disks & volumes, with the free space it needs); move a repo from `/mnt/c` into WSL; install or update Git with the right command for your OS (winget, Homebrew, Xcode tools, apt, dnf, pacman…); free up disk with Storage Sense. `c` copies the command, and `r` checks again live and says whether it's fixed. Lanes never runs a command for you. Right after a Git install on Windows, it spots Git that this terminal can't see yet and says to open a new one. `lane setup` prints the same steps, and `lane setup --json` includes them as `fix`. (`15d1209`)
- `05:26` **Added**: motion and character in the terminal app: "Lanes." in big block letters on the welcome and home screens, a step marker that glides between stops, small running lanes while checks finish, a packet lane on the privacy step, and Autter's otter in its captain's hat (swimming on the welcome, blinking beside sign-in, reaching the finish on the last step). `LANE_NO_ANIM=1` keeps every screen still. (`6df848a`)

### 2026-10-16

- `20:43` **Added**: the first-run welcome. Running `lane` in a terminal the first time opens a six-step terminal app (ratatui): a welcome with moving lanes; live checks of this machine (OS, whether instant copies work on this disk, Git, the repo, the stack, agents on `PATH`, free disk), each with what to do next; a four-step "how a lane works"; a privacy step that shows exactly what the install count sends and respects `DO_NOT_TRACK`; an optional sign-in that's shown but disabled until the gateway opens; and next steps for the folder you're in. After that, `lane` opens a home screen. `lane setup` reruns it, `lane setup --json` prints the checks, and `lane setup --yes [--telemetry on|off]` saves without the app. It never opens in pipes, CI, `--json` or agent sessions. Settings live in `~/.config/lane/config.toml` (`%APPDATA%\lane` on Windows), saved atomically. (`6df848a`)
- `19:47` **Added**: repo setup: bug and feature issue forms (OS, filesystem and command asked up front), a pull request template with the checks to run, weekly grouped Dependabot updates for Cargo, the website and Actions, CI on Windows next to Linux and macOS, a website build job, and `.gitattributes` keeping LF line endings everywhere.
- `18:53` **Added**: website "Runs where your agents run" section (macOS, Linux, Windows and WSL2, cloud agent sandboxes, GitHub then GitLab and Bitbucket) and a "What's being built" status lane that fills as you scroll, plus a Status link in the nav.
- `18:27` **Added**: website "Built-in guardrails" segment (edits checked against the latest version, approval codes for protected files, named agents, lanes from any branch) with an animated approval session, and a "Coming next on the gateway" section (lanes your team can open, agents with their own access, merging after checks). (`5214dd3`)
- `05:31` **Changed**: README and website now say macOS, Linux and Windows at launch (fastest on a Windows 11 Dev Drive, regular NTFS drives supported, WSL2 uses the Linux build), instead of "Windows later". (`5214dd3`)
- `04:57` **Added**: website segments for the Lanes gateway (an animated flow from your lanes, through the opt-in gateway, to Autter's checks), how an Autter review runs on a lane, and how Lanes works with Autter (a pinned write, check, ship, fix loop). The README now explains that Autter reviews run entirely in Autter's cloud: the lane stays on your machine and the gateway lets the review read its changes.
- `04:57` **Changed**: website type is now Supreme and Martian Mono; the footer animates as you reach it; the hero puts "Star on GitHub" first and marks the install command as pre-release; hero headlines no longer wait for an animation to appear.
- `04:57` **Fixed**: the gateway flow animation now runs in order (lanes reach the gateway before anything leaves it), and the gateway page headline can no longer stay hidden when the page loads in a background tab.

### 2026-10-15

- `20:44` **Added**: the Lanes website in `site/` (Astro, GSAP, Lenis): a grainy, minimal home page where scrolling runs four agents in their own lanes, a pinned "How it works" diagram drawn by scroll in four chapters, grayscale lane photography, a gateway page, a designed 404, and full reduced-motion and keyboard support.
- `19:18` **Added**: README section on the Lanes gateway: how it connects your lanes to Autter, what it gives teams, where it's going, and a diagram in light and dark. The README and FAQ now say Autter is never required, and that Lanes is maintained independently of Autter or any other product. (`f5c6ba2`)
- `19:13` **Changed**: README redesigned in a minimal, editorial style: no emoji, numbered sections, and monochrome images with one mint accent, each in light and dark versions that follow the viewer's GitHub theme. The footer shows the Autter wordmark instead of the illustrated banner. (`d2f7d68`)
- `19:08` **Added**: README illustrations in `assets/`: an animated hero, a "How it works" diagram, a preview of the terminal app and an Autter banner in the footer. (`7318e17`)
- `19:04` **Changed**: Lanes is now licensed under the GNU AGPL-3.0 (`AGPL-3.0-only`) instead of Apache-2.0. Code released before this change stays available under Apache-2.0. (`7318e17`)
- `18:53` **Fixed**: README links to [@sagnik_ghosh_11](https://x.com/sagnik_ghosh_11) on X instead of a placeholder handle. (`c259a35`)
- `18:50` **Changed**: README restyled with a centered header, badges, a feature grid, status markers and a collapsible FAQ. (`41e651c`)
- `18:50` **Added**: this changelog, plus a rule in `AGENTS.md` and `CONTRIBUTING.md` to keep it updated. (`41e651c`)
- `18:41` **Added**: README footer linking to [Autter](https://autter.dev). (`026670c`)
- `17:18` **Changed**: README says `lane login` signs in to Autter and connects the Lanes gateway. (`ffd255b`)
- `16:34` **Added**: README covers the terminal app, optional sign-in and the anonymous install count, which `lane telemetry off` or `DO_NOT_TRACK=1` turns off. (`835c156`)
- `16:29` **Added**: agent instructions (`AGENTS.md`, `CLAUDE.md`), `CONTRIBUTING.md` and `SECURITY.md`. (`66176ce`)
- `16:29` **Added**: CI on Linux and macOS running rustfmt, clippy and tests, plus a `cargo-deny` license allowlist. (`e24e977`)
- `16:29` **Added**: Rust workspace with the `lane` CLI, the `laned` daemon and `lane-core`. Both binaries print their version and help; other commands exit with code 3 (not built yet). Exit codes are stable from here on. (`54f0b2f`)
- `16:29` **Added**: initial README, Apache-2.0 license and NOTICE. (`660998a`)
