## What this changes

<!-- One change per pull request. What it does, and why. Link the issue it closes. -->

Closes #

## How you checked it

<!-- The commands you ran and what you saw. Numbers only if you measured them. -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and `cargo test --all` pass
- [ ] New commands support `--json`, never prompt, and use the exit codes in `lane_core::exit`
- [ ] A line in `CHANGELOG.md` under today's UTC date, with the UTC time and the type of change
- [ ] For `site/`: `npm run build` and `npx astro check` pass
