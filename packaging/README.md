# Packaging and releases

Lanes ships on six channels, all named `lanes`. Each one installs the same two binaries, `lane` and `laned`.

| Channel | Users run | Comes from |
| --- | --- | --- |
| GitHub Releases | download from [the releases page](https://github.com/Autter-dev/lanes/releases) | `.github/workflows/release.yml` |
| Install scripts | `curl -fsSL https://github.com/Autter-dev/lanes/releases/latest/download/lanes-installer.sh \| sh` / `irm https://github.com/Autter-dev/lanes/releases/latest/download/lanes-installer.ps1 \| iex` | `installers/` |
| Homebrew | `brew install autter-dev/tap/lanes` | `render.mjs` → `lanes.rb` → [Autter-dev/homebrew-tap](https://github.com/Autter-dev/homebrew-tap) |
| Scoop | `scoop bucket add autter https://github.com/Autter-dev/scoop-bucket`, then `scoop install lanes` | `render.mjs` → `lanes.json` → [Autter-dev/scoop-bucket](https://github.com/Autter-dev/scoop-bucket) |
| npm | `npm install -g @autter/lanes` | `build-npm.mjs` and `npm/lanes/` |
| winget | `winget install Autter.Lanes` | `render.mjs` → winget manifests → a PR to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) |

## Cutting a release

1. Set the version under `[workspace.package]` in `Cargo.toml`, and run `cargo build` so `Cargo.lock` follows. It's a release like `0.1.0` or a release candidate like `0.1.0-rc.1` (below); the release workflow and `cargo test` turn down anything else.
2. Move the `CHANGELOG.md` entries under a `## <version>` heading. The release notes come from that section.
3. Commit, then tag and push: `git tag v<version> && git push origin v<version>`.
4. The release workflow then:
   - builds six targets (macOS, Linux and Windows, each on x64 and arm64);
   - packages `lanes-<version>-<target>.tar.gz` (or `.zip` on Windows) with checksums and build attestations;
   - stamps the version into both install scripts;
   - renders the Homebrew formula, the Scoop manifest and the winget manifests;
   - publishes the GitHub release, titled "Lanes 0.1.0" (0.0.x releases add "(preview)");
   - publishes to npm, once that's set up (below).
5. Within the hour, the tap and the bucket copy their file from the new release and test an install. To skip the wait, run their "Update Lanes" workflow by hand. Then run the Installers workflow (Actions → Installers → Run workflow) with the version: every channel installs it on every system and checks that `lane --version` says it.
6. **winget:** download the `channel-files` artifact from the release run (`gh run download <run-id> -R Autter-dev/lanes -n channel-files`). Then either open a PR to microsoft/winget-pkgs with `dist/winget/manifests/a/Autter/Lanes/<version>/`, or run `wingetcreate submit` on that folder.
7. On the website, set `version` in `site/src/data/site.ts` to the new release, and push. The releases page (`/download/releases/`) lists every `## <version> (<date>)` section of `CHANGELOG.md` up to that version, with its files, so it updates by itself.

To check everything without publishing, run the Release workflow by hand (Actions → Release → Run workflow). It builds every target and the channel files, and publishes nothing.

Every step is safe to rerun. An existing release gets its files replaced, and npm skips versions it already has.

## Release candidates

A release candidate, like `0.1.0-rc.1`, is for trying a version before it ships. Cut it like a release: `version = "0.1.0-rc.1"` in `Cargo.toml`, a `## 0.1.0-rc.1 (<date>)` section in `CHANGELOG.md`, and the tag `v0.1.0-rc.1`. Then each channel:

| Channel | What happens |
| --- | --- |
| GitHub Releases | A pre-release titled "Lanes 0.1.0-rc.1 (release candidate)". GitHub never makes a pre-release the latest release, so `releases/latest` stays on the last release. Its notes give the commands that install the candidate. |
| Install scripts | `releases/latest/download/lanes-installer.sh` and `lanes.autter.dev/install.sh` (and the `.ps1` scripts) still install the last release. The candidate's own scripts, at `releases/download/v0.1.0-rc.1/`, install it, and so do the usual ones with `--version 0.1.0-rc.1` or `$env:LANES_VERSION = '0.1.0-rc.1'`. |
| Homebrew, Scoop | Their "Update Lanes" workflows read the latest release, so they stay where they are. |
| npm | Published under the dist-tag `next`, so `npm install -g @autter/lanes@next` gets it and `npm install -g @autter/lanes` stays on the last release. |
| winget | Nothing. Don't submit a candidate. |
| Website | Nothing. Leave `version` in `site/src/data/site.ts` as it is; the releases page leaves candidates out. |

When the Release run finishes, the Installers workflow installs the candidate with the install scripts and npm on every system, and checks `lane --version`. Homebrew and Scoop skip it, saying why.

When the release itself ships, give it its own `## 0.1.0 (<date>)` section. Its notes never come from the candidate's section.

## One-time setup

- **Homebrew tap and Scoop bucket:** create the public repos `Autter-dev/homebrew-tap` and `Autter-dev/scoop-bucket`, and push the matching folder from `seeds/` to each. They need no secrets: they read the public release.
- **npm:**
  1. Create the npm organisation `autter` (free for public packages).
  2. Give the release workflow a way to publish. Either:
     - set up [trusted publishing](https://docs.npmjs.com/trusted-publishers) for `Autter-dev/lanes` and `.github/workflows/release.yml` on each package; or
     - add a granular npm token with publish rights to `@autter` as the repo secret `NPM_TOKEN`.
  3. Set the repo variable `NPM_PUBLISH` to `true`.
- **winget:** the first `Autter.Lanes` PR is reviewed by hand, which usually takes hours to a few days. Later versions can be automated.

## Testing locally

```sh
node packaging/render.mjs --version 0.0.1 --dist dist --notes notes.md   # needs dist/SHA256SUMS
node packaging/build-npm.mjs --version 0.0.1 --dist dist --out npm       # needs every archive in dist/
sh packaging/installers/lanes-installer.sh --help
```

CI and releases run only on GitHub-hosted runners.
