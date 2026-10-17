# Autter's Homebrew tap

Homebrew formulas for [Lanes](https://lanes.autter.dev), which gives every coding agent its own lane of your repo.

```sh
brew install autter-dev/tap/lanes
```

That installs the `lane` command and the `laned` daemon on macOS (Apple silicon and Intel) and Linux (x64 and arm64). Update with `brew upgrade lanes`.

`Formula/lanes.rb` is copied from each [Lanes release](https://github.com/Autter-dev/lanes/releases) by the workflow in `.github/workflows/update.yml`, so don't edit it here: change `packaging/render.mjs` in the Lanes repo instead.

Lanes is free and open source under the AGPL-3.0.
