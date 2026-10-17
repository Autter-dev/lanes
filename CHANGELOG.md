# Changelog

A running log of notable changes to Lanes, newest first. All times are UTC.

Each entry has a time, a type (**Added**, **Changed**, **Fixed**, **Removed** or **Security**) and, once committed, its commit. When a release ships, its entries move under a version heading.

## Unreleased

### 2026-10-17

- `11:42` **Added**: Lanes is on npm: `npm install -g @autter/lanes` installs only the build for your system, with signed provenance. A new `npm` workflow publishes an existing release from its own archives, and the installers check now installs through every live channel (both scripts, Homebrew, Scoop and npm) on macOS, Linux and Windows. The download page lists npm as live; winget is still waiting for Microsoft's review.
- `11:19` **Added**: Lanes 0.0.1 is out as a preview. It installs with `brew install autter-dev/tap/lanes`, with Scoop from the Autter bucket, with the one-line scripts (`curl -fsSL https://lanes.autter.dev/install.sh | sh`, `irm https://lanes.autter.dev/install.ps1 | iex`), or from GitHub Releases. Each one was checked by a real install on macOS, Linux and Windows. The download page and the README show the live options; npm and winget are marked "coming soon" until they're published. A new installers check reinstalls every release the way users do.

## 0.0.1 (2026-10-17)

**The first preview.** In this version:
- `lane` opens the first-run welcome, then the home screen.
- `lane setup` checks your machine, walks you through fixes, and saves your settings (also with `--json`).
- `--help` and `--version` work.
- The lane commands (`init`, `new`, `list`, `exec`, `rm`) are listed but not built yet, and exit with code 3.

It runs on macOS, Linux and Windows, on x64 and arm64.

### 2026-10-17

- `10:26` **Added**: release pipeline and a download page. (`81fe630`)
  - **Release workflow:** pushing a `v<version>` tag builds macOS, Linux and Windows on x64 and arm64, and publishes a GitHub release. It includes `lanes-<version>-<target>` archives, SHA-256 checksums, build attestations, and one-line installers (`lanes-installer.sh`, `lanes-installer.ps1`) with the version stamped in.
  - **Channel files:** the same run renders the Homebrew formula, the Scoop manifest and the winget manifests. Once enabled, it also publishes `@autter/lanes` to npm, with one package per platform so npm downloads only yours.
  - **Tap and bucket:** starting files for the Homebrew tap and the Scoop bucket in `packaging/seeds/`. Each copies its file from the latest release and tests an install.
  - **Website:** a download page at lanes.autter.dev/download that puts your system first, with every channel, the archives and checksums, how to verify a file, and how to uninstall. The home page's install command now matches your system, and short URLs `lanes.autter.dev/install.sh` and `/install.ps1` point at the latest installers.
- `09:51` **Added**: the website, [lanes.autter.dev](https://lanes.autter.dev), is linked from the top of the README and from the repo's About box. (`22e5ab1`)
- `09:37` **Changed**: install commands now name the six channels Lanes ships on, called `lanes` everywhere: `brew install autter-dev/tap/lanes`, `winget install Autter.Lanes`, `scoop install lanes` (from the Autter bucket), `npm install -g @autter/lanes`, one-line installers for macOS, Linux and Windows, and archives on GitHub Releases. Each installs the same `lane` command. `cargo install` is no longer listed. On phones, the website's install command now wraps only between words. (`a8c012d`)
- `06:06` **Fixed**: CI is green again. The website stays on TypeScript 6 (`@astrojs/check` doesn't support 7 yet, so `npm ci` failed), Dependabot skips TypeScript major versions until it does, and clippy no longer fails on macOS over a cast that's only redundant there. (`202ef78`)
- `05:52` **Added**: guided fixes on the "Your machine" step. Each `!` or `✗` that has a fix shows one (`↑↓` to choose, `f` to open), as plain numbered steps written for this machine: move the repo to the Dev Drive you already have (with the exact `git clone` command), or create one (`o` opens Settings › Disks & volumes, with the free space it needs); move a repo from `/mnt/c` into WSL; install or update Git with the right command for your OS (winget, Homebrew, Xcode tools, apt, dnf, pacman…); free up disk with Storage Sense. `c` copies the command, and `r` checks again live and says whether it's fixed. Lanes never runs a command for you. Right after a Git install on Windows, it spots Git that this terminal can't see yet and says to open a new one. `lane setup` prints the same steps, and `lane setup --json` includes them as `fix`. (`15d1209`)
- `05:26` **Added**: motion and character in the terminal app: "Lanes." in big block letters on the welcome and home screens, a step marker that glides between stops, small running lanes while checks finish, a packet lane on the privacy step, and Autter's otter in its captain's hat (swimming on the welcome, blinking beside sign-in, reaching the finish on the last step). `LANE_NO_ANIM=1` keeps every screen still. (`6df848a`)

### 2026-10-16

- `20:43` **Added**: the first-run welcome. Running `lane` in a terminal the first time opens a six-step terminal app (ratatui): a welcome with moving lanes; live checks of this machine (OS, whether instant copies work on this disk, Git, the repo, the stack, agents on `PATH`, free disk), each with what to do next; a four-step "how a lane works"; a privacy step that shows exactly what the install count sends and respects `DO_NOT_TRACK`; an optional sign-in that's shown but disabled until the gateway opens; and next steps for the folder you're in. After that, `lane` opens a home screen. `lane setup` reruns it, `lane setup --json` prints the checks, and `lane setup --yes [--telemetry on|off]` saves without the app. It never opens in pipes, CI, `--json` or agent sessions. Settings live in `~/.config/lane/config.toml` (`%APPDATA%\lane` on Windows), saved atomically. (`6df848a`)
- `19:47` **Added**: repo setup: bug and feature issue forms (OS, filesystem and command asked up front), a pull request template with the checks to run, weekly grouped Dependabot updates for Cargo, the website and Actions, CI on Windows next to Linux and macOS, a website build job, and `.gitattributes` keeping LF line endings everywhere.
- `18:53` **Added**: website "Runs where your agents run" section (macOS, Linux, Windows and WSL2, cloud agent sandboxes, GitHub then GitLab and Bitbucket) and a "What's being built" status lane that fills as you scroll, plus a Status link in the nav.
- `18:27` **Added**: website "Built-in guardrails" segment (edits checked against the latest version, approval codes for protected files, named agents, lanes from any branch) with an animated approval session, and a "Coming next on the gateway" section (lanes your team can open, agents with their own access, merging after checks). (`5214dd3`)
- `05:31` **Changed**: README and website now say macOS, Linux and Windows at launch (fastest on a Windows 11 Dev Drive, regular NTFS drives supported, WSL2 uses the Linux build), instead of "Windows later". (`5214dd3`)
- `04:57` **Added**: website segments for the Lanes gateway (an animated flow from your lanes, through the opt-in gateway, to Autter's checks), how an Autter review runs on a lane, and how Lanes works with Autter (a pinned write, check, ship, fix loop). The README now explains that Autter reviews run entirely in Autter's cloud: the lane stays on your machine and the gateway lets the review read its changes.
- `04:57` **Changed**: website type is now Supreme and Martian Mono; the footer animates as you reach it; the hero puts "Star on GitHub" first and marks the install command as pre-release; hero headlines no longer wait for an animation to appear.
- `04:57` **Fixed**: the gateway flow animation now runs in order (lanes reach the gateway before anything leaves it), and the gateway page headline can no longer stay hidden when the page loads in a background tab.

### 2026-10-15

- `20:44` **Added**: the Lanes website in `site/` (Astro, GSAP, Lenis): a grainy, minimal home page where scrolling runs four agents in their own lanes, a pinned "How it works" diagram drawn by scroll in four chapters, grayscale lane photography, a gateway page, a designed 404, and full reduced-motion and keyboard support.
- `19:18` **Added**: README section on the Lanes gateway: how it connects your lanes to Autter, what it gives teams, where it's going, and a diagram in light and dark. The README and FAQ now say Autter is never required, and that Lanes is maintained independently of Autter or any other product. (`f5c6ba2`)
- `19:13` **Changed**: README redesigned in a minimal, editorial style: no emoji, numbered sections, and monochrome images with one mint accent, each in light and dark versions that follow the viewer's GitHub theme. The footer shows the Autter wordmark instead of the illustrated banner. (`d2f7d68`)
- `19:08` **Added**: README illustrations in `assets/`: an animated hero, a "How it works" diagram, a preview of the terminal app and an Autter banner in the footer. (`7318e17`)
- `19:04` **Changed**: Lanes is now licensed under the GNU AGPL-3.0 (`AGPL-3.0-only`) instead of Apache-2.0. Code released before this change stays available under Apache-2.0. (`7318e17`)
- `18:53` **Fixed**: README links to [@sagnik_ghosh_11](https://x.com/sagnik_ghosh_11) on X instead of a placeholder handle. (`c259a35`)
- `18:50` **Changed**: README restyled with a centered header, badges, a feature grid, status markers and a collapsible FAQ. (`41e651c`)
- `18:50` **Added**: this changelog, plus a rule in `AGENTS.md` and `CONTRIBUTING.md` to keep it updated. (`41e651c`)
- `18:41` **Added**: README footer linking to [Autter](https://autter.dev). (`026670c`)
- `17:18` **Changed**: README says `lane login` signs in to Autter and connects the Lanes gateway. (`ffd255b`)
- `16:34` **Added**: README covers the terminal app, optional sign-in and the anonymous install count, which `lane telemetry off` or `DO_NOT_TRACK=1` turns off. (`835c156`)
- `16:29` **Added**: agent instructions (`AGENTS.md`, `CLAUDE.md`), `CONTRIBUTING.md` and `SECURITY.md`. (`66176ce`)
- `16:29` **Added**: CI on Linux and macOS running rustfmt, clippy and tests, plus a `cargo-deny` license allowlist. (`e24e977`)
- `16:29` **Added**: Rust workspace with the `lane` CLI, the `laned` daemon and `lane-core`. Both binaries print their version and help; other commands exit with code 3 (not built yet). Exit codes are stable from here on. (`54f0b2f`)
- `16:29` **Added**: initial README, Apache-2.0 license and NOTICE. (`660998a`)
