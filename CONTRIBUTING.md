# Contributing to Lanes

Thanks for your interest. Lanes is early, so the most useful contributions right now are:

- **Bug reports** with your OS, filesystem and the command you ran
- **Benchmarks from your own repos**: how long `lane new` takes and how much disk it uses
- **Agent setups** we don't support yet

## Before you send a pull request

1. Open an issue first for anything larger than a small fix, so we can agree on the approach.
2. Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and `cargo test --all`.
3. Keep each pull request to one change.

## License

By contributing, you agree that your contributions are licensed under the Apache License 2.0, the same license as the project.
