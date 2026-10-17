---
name: lanes
description: Work in your own lane (a Git worktree with its own ports and .env, made by Lanes) instead of the shared checkout. Use at the start of any coding task in a repo where `lane` is installed and other agents or people may be working, before you would make a Git worktree, clone or copy the repo, and before you start a dev server.
---

# Lanes

Lanes gives every agent its own lane: a real Git worktree of the repo, with dependencies already installed and a block of ten ports of its own. Working in a lane means you never edit the same files as another agent, and your dev server never fights theirs for a port.

## 1. Check where you are

Run `lane env`. If it prints `PORT=...` and `LANE_NAME=...`, you're already in a lane: skip to step 3.

If it says you're not inside a lane, you're in the shared checkout. Unless the user asked you to work right here, make a lane first.

## 2. Make your lane

Name it after the task, in lowercase letters, digits and hyphens, at most 40 characters, like `fix-login-redirect`.

**In Claude Code,** use the EnterWorktree tool with that name. Lanes' hooks make the lane and move the whole session into it, so every later command and edit happens there. The user may be asked once to approve the move.

**Anywhere else,** or if EnterWorktree isn't available, run:

```sh
lane new fix-login-redirect --print-path
```

It prints one line: the lane's folder. Do all of the task's work there: change into it, or use paths inside it.

The first lane in a repo takes longer, because Lanes installs dependencies once; after that, a lane takes seconds or less. In Cursor, a worktree it makes for you becomes a lane on its own when your session starts, so step 1 will already show you're in one.

Never make a copy of the repo yourself, with `git worktree add`, a `git clone` of it, or a recursive copy of its folder. Lanes turns those down, and a lane does the same job with ports and dependencies set up.

A lane starts at the base branch, with no branch of its own. When you're ready to commit, make one: `git switch -c <branch>`.

## 3. Run things in the lane

- Start the dev server with `lane exec -- <dev command>` from inside the lane, for example `lane exec -- npm run dev`. `PORT`, `VITE_PORT` and `LANE_URL` are set for it, so it doesn't clash with other lanes. The app is at `LANE_URL`, like `http://fix-login-redirect.localhost:4100`.
- To put the port in an argument, write `'$PORT'` in single quotes and Lanes fills it in: `lane exec -- pnpm dev --port '$PORT'`.
- The lane has its own `.env`, with its own `PORT`. Other settings in it came from the repo's template.
- Tests, linters and builds run as usual inside the lane's folder.

## 4. Finish

- Tell the user the lane's name and folder, and its URL if you started the app, so they can look at your work.
- Commit on your own branch in the lane, if the user wants commits.
- Don't remove a lane unless the user asks: `lane rm <name>` deletes its folder, changes and all.

## Reference

- `lane list` shows every lane in the repo with its ports and folder, including other agents' lanes.
- `lane env` prints the lane's variables; `lane env --shell sh` prints them as `export` lines.
- Every command takes `--json` and `--help`. Exit codes: 0 done, 1 failed, 2 wrong command line. A failed command's message says what to do next.
- With `--json`, each result has a `next` field: the commands to run next, with why.
