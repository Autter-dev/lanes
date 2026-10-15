# Changelog

A running log of notable changes to Lanes, newest first. All times are UTC.

Each entry has a time, a type (**Added**, **Changed**, **Fixed**, **Removed** or **Security**) and, once committed, its commit. When a release ships, its entries move under a version heading.

## Unreleased

### 2026-10-15

- `18:50` **Changed**: README restyled with a centered header, badges, a feature grid, status markers and a collapsible FAQ.
- `18:50` **Added**: this changelog, plus a rule in `AGENTS.md` and `CONTRIBUTING.md` to keep it updated.
- `18:41` **Added**: README footer linking to [Autter](https://autter.dev). (`026670c`)
- `17:18` **Changed**: README says `lane login` signs in to Autter and connects the Lanes gateway. (`ffd255b`)
- `16:34` **Added**: README covers the terminal app, optional sign-in and the anonymous install count, which `lane telemetry off` or `DO_NOT_TRACK=1` turns off. (`835c156`)
- `16:29` **Added**: agent instructions (`AGENTS.md`, `CLAUDE.md`), `CONTRIBUTING.md` and `SECURITY.md`. (`66176ce`)
- `16:29` **Added**: CI on Linux and macOS running rustfmt, clippy and tests, plus a `cargo-deny` license allowlist. (`e24e977`)
- `16:29` **Added**: Rust workspace with the `lane` CLI, the `laned` daemon and `lane-core`. Both binaries print their version and help; other commands exit with code 3 (not built yet). Exit codes are stable from here on. (`54f0b2f`)
- `16:29` **Added**: initial README, Apache-2.0 license and NOTICE. (`660998a`)
