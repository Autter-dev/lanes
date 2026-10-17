# Autter's Scoop bucket

Scoop manifests for [Lanes](https://lanes.autter.dev), which gives every coding agent its own lane of your repo.

```powershell
scoop bucket add autter https://github.com/Autter-dev/scoop-bucket
scoop install lanes
```

That installs `lane.exe` and `laned.exe` on Windows (x64 and arm64). Update with `scoop update lanes`.

`bucket/lanes.json` is copied from each [Lanes release](https://github.com/Autter-dev/lanes/releases) by the workflow in `.github/workflows/update.yml`, so don't edit it here: change `packaging/render.mjs` in the Lanes repo instead.

Lanes is free and open source under the AGPL-3.0.
