# Lanes

**Lighter agents.** Run as many coding agents as you want on one repo, without them getting in each other's way.

Every agent gets its own instant copy of your project, with dependencies ready and its own ports, URL and database. You keep your Git repo, your GitHub and your agents. Nothing to migrate.

> **Early and in active development.** We're building Lanes in public. Star or watch this repo for releases, and follow along at [@sagnik_ghosh_11](https://x.com/sagnik_ghosh_11).

---

## Why Lanes

Running several coding agents today usually means:

- a full copy of your repo for every agent
- a fresh `npm install` (or `cargo build`, or `pip install`) for every copy
- five dev servers fighting over port 3000
- agents breaking each other's database with their migrations
- finding out two agents edited the same function only when you merge

Lanes gives each agent a **lane** instead.

## Features

### Instant lanes
- A ready-to-run copy of your project for each agent, with dependencies included
- Uses almost no extra disk space
- A lane is a normal folder, so your editor, compiler and test runner all just work

### Every lane is a live app
- Its own ports and its own URL, like `fix-auth.localhost`
- Its own `.env`
- Its own database copy (Postgres, SQLite) and its own Docker Compose project
- Open five versions of your app side by side

### Works with the agents you already use
- Claude Code, Cursor, Codex, Gemini CLI, GitHub Copilot, Conductor, Amp, OpenCode, Cline and Aider
- When any of them creates a Git worktree, it becomes a lane automatically. Your agents don't need to learn anything new.
- A built-in MCP server, so agents can create lanes and read code in one call
- Works in cloud agents too: one line in your setup script

### A proper terminal app
- Run `lane` to open a full-screen app: every lane with its agent, changed files, tests, conflicts and disk use
- Start agents, watch them work, read diffs and logs, undo steps and open PRs without leaving it

### No more collisions
- A warning when two agents start editing the same file or function, while they're still working

### Undo anything
- A checkpoint after every agent step
- `lane undo` takes back any step; `lane restore` jumps back to any checkpoint

### Best of N
- `lane race -n 3 "fix the flaky checkout test"` gives one task to three agents, runs the tests in each and shows a scoreboard. You keep the best one.
- `lane combine a b` lets you try two lanes together before merging either

### Ship clean PRs
- `lane promote` turns an agent's work into one clean pull request, with who (and which agent) wrote what
- Protected paths, like `migrations/` or CI config, need a human to sign off

### Kind to your laptop
- Builds and tests take turns, so five agents don't freeze your machine
- Dev servers start only when you open them

### Your code stays on your machine
- Works offline
- Signing in is optional, and connecting a repo to the gateway is always your choice
- The only thing Lanes sends is an anonymous install count: version, OS and how you installed it. Never code, file paths or repo names. Turn it off with `lane telemetry off` or `DO_NOT_TRACK=1`

### Ready for teams
- `lane login` signs you into [Autter](https://autter.dev) and connects your lanes to the Lanes gateway on Autter: cloud agents and CI read your repo without cloning, and you get repo Q&A, security checks while agents work, and a safe-to-merge verdict on every PR

## Status

| Feature | Status |
| --- | --- |
| Instant lanes, `lane init`, ports and env per lane | Building now |
| Terminal app | Building now |
| Sign-in to Autter (`lane login`) | Next |
| Claude Code integration, MCP server | Building now |
| Cursor, Codex, Gemini CLI, Copilot and other agents | Next |
| Databases and Docker Compose per lane, `*.localhost` URLs | Next |
| Undo, conflict warnings, `lane race`, `lane promote` | Next |
| Cloud agent setup snippets, GitHub Action | Next |
| Lanes gateway on Autter | December |
| Windows | Later |

## Quick start (preview)

Install commands go live with the first release.

```sh
brew install lane            # or: cargo install lane / npm i -g <package>
cd your-repo
lane init                    # finds your stack and sets up your agents
lane new fix-auth            # a ready-to-run lane
lane exec fix-auth -- npm run dev
open http://fix-auth.localhost
```

## Commands

| Command | What it does |
| --- | --- |
| `lane init` | Sets up Lanes for your repo and the agents you use |
| `lane new <name>` | Creates a lane |
| `lane list` | Lists your lanes with their ports and URLs |
| `lane exec <name> -- <cmd>` | Runs a command inside a lane |
| `lane run -- <agent>` | Starts any agent in a fresh lane |
| `lane` | Opens the terminal app |
| `lane undo` / `lane restore` | Takes back a step, or returns to a checkpoint |
| `lane race -n <N> "<task>"` | One task, N agents, best result wins |
| `lane combine <a> <b>` | Tries two lanes together |
| `lane promote <name>` | Turns a lane into one clean PR |
| `lane rm <name>` / `lane gc` | Removes one lane, or every finished one |
| `lane login` | Signs in to Autter and connects the Lanes gateway (optional) |
| `lane telemetry off` | Turns off the anonymous install count |

## Platforms

macOS and Linux at launch. Windows later.

## FAQ

**Does it replace Git?**
No. Lanes works on top of the Git repo and the GitHub you already use.

**Does my code leave my machine?**
Not unless you choose to connect a repo to the Lanes gateway on Autter. Otherwise Lanes runs locally, and the only thing it sends is an anonymous install count (version, OS and how you installed it), which you can turn off.

**Is it free?**
Yes. Lanes is open source under Apache-2.0. Autter, for teams, is a paid product.

**Is this related to Anthropic's Scion?**
Lanes is inspired by ideas Anthropic shared about how it scaled source control for agents. We are not affiliated with Anthropic.

## Contributing

Lanes is early. Issues, ideas and benchmarks from your own repos are very welcome. Please open an issue before sending a large PR.

## License

Apache-2.0. See [LICENSE](LICENSE).

---

<p align="center">
  Lanes is built by <a href="https://autter.dev"><b>Autter</b></a>: code Q&amp;A, security checks and safe-to-merge verdicts for teams shipping with agents.<br>
  <a href="https://autter.dev">autter.dev</a>
</p>
