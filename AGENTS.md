# Working on Lanes

Lanes is a Rust workspace:

- `crates/lane`: the `lane` command-line tool
- `crates/laned`: the background daemon, one per user
- `crates/lane-core`: code shared by both

## Commands

- Build: `cargo build`
- Test: `cargo test --all`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt --all`

Run format, lint and tests before you finish.

## Rules for every command

- Supports `--json`, never prompts, never opens a pager.
- Uses the exit codes in `lane_core::exit`. Never renumber them.
- Error messages say what to do next.
- Writes are safe to retry.
- No network calls unless the user ran `lane login`.

## Dependencies

Check a new dependency's license against `deny.toml` before adding it. Prefer fewer, well-maintained crates.

## Parallel work

Once `lane new` works, use lanes for parallel work on this repo: run `lane new <name>` instead of `git worktree add` or copying the repo.
