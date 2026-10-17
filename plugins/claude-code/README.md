# Lanes for Claude Code

This plugin puts Claude Code in lanes. A lane is a Git worktree with its own ports and `.env`, made in an instant by [Lanes](https://lanes.autter.dev).

- **`claude -w <name>`** makes a lane, and the session works in it. So do subagents with `isolation: worktree` and background sessions.
- **A session in a lane** starts with the lane's `PORT`, `VITE_PORT` and `LANE_URL` set, and knows which lane it's in.
- **`git worktree add`** is turned down in a Lanes repo, with a pointer to `lane new`.
- **Removing the worktree** removes the lane and frees its ports.

Outside a Lanes repo, `claude -w` still makes a lane (Lanes works without a `lane.toml`). The other hooks stay silent.

It needs the `lane` command: see [lanes.autter.dev/download](https://lanes.autter.dev/download).

## Install

In Claude Code:

```
/plugin marketplace add Autter-dev/lanes
/plugin install lanes@autter
```

You don't need the plugin in a repo where you ran `lane init`. `lane init` writes the same hooks into that repo's `.claude/settings.local.json`. Having both is harmless: the hooks are safe to run twice.
