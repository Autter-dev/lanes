# Working on Lanes

Lanes is a Rust workspace:

- `crates/lane`: the `lane` command-line tool
- `crates/laned`: the background daemon, one per user
- `crates/lane-core`: code shared by both

Next to it, `services/install-count` is the install count's backend: a Cloudflare Worker that keeps counts in Postgres. It's TypeScript; `npm test` and `npm run check` there. Its README says how to host it.

## Commands

- Build: `cargo build`
- Test: `cargo test --all`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt --all`

Run format, lint and tests before you finish.

## Rules for every command

- Supports `--json`. Never prompts or opens a pager for agents, scripts or CI: it asks Y/n only when a person is at a terminal (`Env::can_ask`), and `--yes` takes every default without asking.
- Uses the exit codes in `lane_core::exit`. Never renumber them.
- Error messages say what to do next.
- Writes are safe to retry.
- No network calls unless the user ran `lane login`. The one exception is the anonymous install count. It's on by default, and off with `lane telemetry off`, `LANE_TELEMETRY=0` or `DO_NOT_TRACK=1`. It's never sent from CI, and it never slows a command down.

## Dependencies

Check a new dependency's license against `deny.toml` before adding it. Prefer fewer, well-maintained crates.

## Changelog

Add an entry to `CHANGELOG.md` for every notable change, newest first, under today's UTC date (`### YYYY-MM-DD`) in the Unreleased section. Each entry starts with the UTC time (`HH:MM`, from `date -u`) and a type: Added, Changed, Fixed, Removed or Security. Add the short commit hash once it exists.

## Parallel work

Once `lane new` works, use lanes for parallel work on this repo: run `lane new <name>` instead of `git worktree add` or copying the repo.
