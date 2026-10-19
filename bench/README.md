# Benchmarks

## The rule

**We only ever publish numbers we measured.** Every number about Lanes' speed or disk use, in
the README, on the website or in a post, links to the results file it came from in
[`results/`](results/). A number without a results file doesn't get published, and neither
does a number from a smoke run (`"smoke": true` in the file, `-smoke` in its name).

When you quote a number:

- give the median, with the p95 next to it. Never the best run;
- say which machine and disk it's from (the results file has the details);
- compare Lanes against what a careful user does today: `git worktree add` plus the install,
  with a warm package cache. Never against a cold `npm install`.

## Benchmark 1: a new copy, ready to run

How long does it take to get a ready-to-run copy of a repo, and how much disk does it take,
when an agent needs one? Two ways of making copies, for 1, 5 and 10 copies at once:

- **worktree**: `git worktree add`, then the repo's dependency install in the new folder, with
  a warm package cache;
- **lane**: `lane new`.

Three measurements:

| | What | How |
| --- | --- | --- |
| Wall time | making N copies, one after another | hyperfine, `--warmup 1 --runs 10`; median and p95 |
| Disk added | what N copies take | the drop in the volume's free space: `df` on macOS and Linux, `Get-Volume` on Windows. Never `du`, which counts a cloned file at full size |
| Time to the first passing test | making one fresh copy and running a fixed test subset in it, until it passes | hyperfine, as above |

The first `lane new` in a repo also sets up `_base`, the clean copy that Lanes clones from, and
runs the install there once. That one-off cost is measured separately, in one run, and
reported as `lane_base_setup` with its time and disk. It is never left out.

### The repos

Each is pinned to a commit in [`repos.ini`](repos.ini), with its install command and a fixed test
subset small enough that a run takes minutes, not hours.

| Repo | What it is | Install | Test subset | Runs on |
| --- | --- | --- | --- | --- |
| [withastro/astro](https://github.com/withastro/astro) | pnpm monorepo: 572 workspace projects | `pnpm install --frozen-lockfile`, measured twice: stock pnpm, and with pnpm's global virtual store (`virtualStoreType: global`) | build and test `@astrojs/internal-helpers` (90 tests) | macOS, Linux, Windows |
| [astral-sh/ruff](https://github.com/astral-sh/ruff) | Rust workspace: 55 crates, 553 packages in `Cargo.lock` | `cargo fetch --locked` | `cargo test -p ruff_python_ast` | macOS, Linux, Windows |
| [saleor/saleor](https://github.com/saleor/saleor) | Python and Django with uv; needs Postgres | `uv sync --locked` | two test files in `saleor/core/tests` | macOS, Linux |
| [mastodon/mastodon](https://github.com/mastodon/mastodon) | Rails; needs Postgres and Redis | `bundle install` | `spec/lib/hashtag_normalizer_spec.rb` | macOS, Linux |
| this repo (`lanes`) | Rust workspace: the quick target | `cargo fetch --locked` | `cargo test -p lane-core --lib project::tests` | macOS, Linux, Windows |

For the Rust repos the install is a fetch, so the time to the first test includes compiling, in
every copy, for both methods.

Astro's `pnpm-workspace.yaml` turns on pnpm 11's check of every lockfile entry against the
registry. That check runs again in every new folder and spends about 45 s on the network, so
the bench sets `trustLockfile` for both methods. This favors the baseline, and keeps the network
out of the numbers.

Saleor's install needs memray, which has no Windows build, so on Windows run Saleor in WSL2 with
`run.sh`. Mastodon is macOS and Linux only.

## Running it

You need [hyperfine](https://github.com/sharkdp/hyperfine) 1.13 or later and Git, plus `jq` 1.6
or later for `run.sh` (macOS 15 and later ship it), `lane` itself, and what each repo needs:

| Repo | Programs | Services |
| --- | --- | --- |
| astro | Node 22 or later, and pnpm 11.28.4, the version in Astro's `packageManager` (`corepack enable` makes `pnpm` pick it) | |
| ruff, lanes | rustup and cargo (rustup installs the toolchain each repo pins) | |
| saleor | uv, recent enough for its `uv.lock` | Postgres 15 or later, with a superuser |
| mastodon | Ruby (from Mastodon's `.ruby-version`), Bundler, a C compiler, libvips, libpq, ICU and libidn | Postgres 14 or later, Redis 7 or later |

```sh
bench/run.sh --repo lanes --runs 2               # macOS, Linux: a quick smoke run
bench/run.sh --work /Volumes/Work/lanes-bench    # the full matrix, on the disk you name
```

```powershell
.\bench\run.ps1 -Repo lanes -Runs 2              # Windows: a quick smoke run
.\bench\run.ps1 -Work D:\lanes-bench             # the full matrix, on a Dev Drive
```

If PowerShell refuses to run the script, run it with
`powershell -ExecutionPolicy Bypass -File .\bench\run.ps1 ...`.

| `run.sh` | `run.ps1` | Default | What |
| --- | --- | --- | --- |
| `--repo NAME` | `-Repo NAME` | every repo for this OS except `lanes` | which repos (repeat, or a list) |
| `--runs N` | `-Runs N` | 10 | timed runs per measurement |
| `--warmup N` | `-Warmup N` | 1 | warmup runs, not counted |
| `--copies "1 5 10"` | `-Copies 1,5,10` | 1, 5 and 10 | how many copies at once |
| `--work DIR` | `-Work DIR` | `~/lanes-bench`; on Windows the first ReFS volume (a Dev Drive), else `%USERPROFILE%\lanes-bench` | where clones and copies go: the disk you're measuring |
| `--lane PATH` | `-Lane PATH` | `lane` on PATH | the lane binary |
| `--machine NAME` | `-Machine NAME` | OS, CPU and filesystem | the machine's name in the results file. Never the hostname |
| `--out FILE` | `-Out FILE` | `results/<date>-<machine>.json` | where the results go |
| `--settle SECS` | `-Settle SECS` | 5; 15 on Windows | how long free space must hold still before it's read |
| `--no-first-test` | `-NoFirstTest` | | skip the time to the first passing test |
| `--smoke` | `-Smoke` | | mark the results as a smoke run |

Fewer than 10 runs, no warmup, or copies other than 1, 5 and 10 make it a smoke run, named
`-smoke` and marked `"smoke": true`, whatever you meant.

### What it does

For each repo:

1. Clones it once into the work folder at the pinned commit (shallow, and kept for next time),
   and writes a `lane.toml` there (kept out of `git status`) so that Lanes follows that commit
   and runs the same install as the worktrees.
2. Runs the install once in that clone, for each baseline variant, so the package caches are
   warm.
3. Makes the first lane, timing it and the disk it takes: that's `_base`'s one-off setup. The
   lane stays until the end, so no delete is still settling while disk is measured.
4. Times making 1, 5 and 10 copies, each way, and records the disk they took in every run.
5. Times the first passing test in a fresh copy, for each way of making copies. It comes last
   because a copy with build output in it gives its space back slowly once removed.
6. Removes everything but the clone, and waits until the disk has the space back.

hyperfine times a small generated script (in `<work>/<repo>/steps/`), so you can read exactly what
was timed, or run it by hand. Before every run, its `--prepare` step removes the last run's copies;
that's also when the free space is read, once the disk has settled, so removing copies is never
timed. Each step's output goes to `<work>/<repo>/logs/`.

The bench keeps its lanes apart from yours (`LANE_STATE_DIR` and `LANE_CONFIG_DIR` point into the
work folder), turns the install count off for its runs, and only ever deletes inside a work folder
it made. It unsets `CARGO_TARGET_DIR` so that every copy builds in its own `target` folder, as it
would for an agent.

## Running it fairly

- **Plug in, and close other apps.** Browsers, editors, Docker Desktop, chat apps and sync
  clients (Dropbox, OneDrive, iCloud Drive) all write to disk and take CPU, which shows up in
  both wall time and free space. Don't let backups, updates or a virus scan start mid-run.
- **The disk under test holds the work folder.** Put it on the disk you're describing. On
  Windows that's a Dev Drive for the fast path (Lanes clones files there) or an NTFS drive for
  the plain copy; on Linux, Btrfs or XFS for clones, ext4 for copies; on macOS, APFS. The
  results file records the filesystem, the disk, and whether a Windows Dev Drive is a virtual
  disk file (it often is) and which disk holds that file.
- **macOS: turn off Time Machine local snapshots.** They keep deleted copies' blocks, so free
  space never comes back and disk added is wrong. Run `sudo tmutil disable` (no automatic
  backups) and `tmutil deletelocalsnapshots /`, check with `tmutil listlocalsnapshots /`, and
  turn backups back on with `sudo tmutil enable` afterwards. `run.sh` warns if it finds local
  snapshots. Keeping Spotlight out of the work folder (System Settings, Spotlight, Privacy)
  helps too.
- **Windows:** keep the work folder out of OneDrive and Windows Search indexing, and don't run
  on battery saver.
- **Package caches are warm, always.** That's what a careful user has today, so the bench runs
  each install once before measuring, and both methods use the same caches. Comparing against a
  cold install would flatter Lanes, so we don't. Where the cache lives matters: pnpm, uv and
  Cargo link or copy from their caches, and a cache on another disk means real copies. The bench
  uses your normal caches, wherever they are.
- **The OS file cache is warm too.** The warmup run, which isn't counted, reads everything the
  timed runs read. A cold-cache number (after a reboot) is a different benchmark, and benchmark
  1 doesn't report one.
- **Disk added needs a quiet disk.** Filesystems allocate written blocks, and free deleted ones,
  seconds later. On a Windows Dev Drive, deleting ten worktrees of this repo gave nothing back
  for 40 seconds to well over a minute, then all of it at once, so no fixed wait is safe. So the
  bench:
  - reads free space only once it has held still, within 1 MiB, for `--settle` seconds (5, or
    15 on Windows), after a flush;
  - after removing a run's copies, waits until the free space is back where it was before they
    were made (less 2 MiB and 1% of what they took), so one run's deletes never land in the next
    run's number;
  - starts each measurement after a longer quiet spell (four settle windows).

  If free space doesn't come back within 15 minutes, or never holds still, the bench says so, and
  the warning is kept with that measurement in the results file. All this waiting makes a run
  slow, Windows most of all; don't shorten it for numbers you'll publish. Anything else writing
  to the disk still shows up as noise of a few MiB, which matters for a small repo like this one
  and hardly for a big one.
- **Postgres and Redis run on the same machine** as the copies for Saleor and Mastodon, and the
  same for both methods. Their test databases live outside the copies, so the time to the first
  test doesn't include making a database: Saleor's warmup run migrates its test database once,
  and the timed runs reuse it.

### Services

The bench checks that Postgres answers on localhost:5432 (or `$PGPORT`) and Redis on 6379.
With Docker, for example:

```sh
docker run -d --name bench-postgres -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:15
docker run -d --name bench-redis -p 6379:6379 redis:7
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/saleor   # Saleor
export DB_HOST=localhost DB_USER=postgres DB_PASS=postgres                 # Mastodon
```

A repo's `env` in `repos.ini` are defaults, so what you export wins. Mastodon's specs empty Redis
database 1 after every example: use a Redis you don't mind losing.

## The results file

`results/<date>-<machine>.json` holds:

- `machine`: OS, CPU, memory, the disk and filesystem the work folder is on, and the Lanes, Git
  and hyperfine versions. No hostname, user name or paths;
- `settings` (what this run used) and `protocol` (what a publishable run uses), and `smoke`;
- for each repo, the commit, install and test commands and tool versions, `lane_base_setup`,
  and `results`: one entry per measurement, method, variant and number of copies, with
  `wall_seconds` and `disk_added_bytes` as `{n, median, p95, runs}`, any `warnings` from
  reading free space, and hyperfine's own result for that measurement. A measurement with
  warnings needs a closer look before anyone quotes its disk number.

p95 is by linear interpolation between the closest ranks (numpy's default), so with 10 runs it
lies between the two slowest. Both scripts compute it the same way, and write the same format.

## Adding a repo

Add a section to `repos.ini`: the URL, the commit to pin (the default branch's HEAD on the day,
with the date in `pinned`), the install command, a test subset that passes on a fresh copy in
well under a minute once installed, and what it needs. Run it once with `--runs 2` to check,
then for real.

## Limits

- So far only the quick target (`lanes`) has been run, in smoke runs of `run.ps1` on a Windows
  Dev Drive. The other four repos' install and test commands come from reading their CI at the
  pinned commits, so their first run may need a fix in `repos.ini`. `run.sh` is
  syntax-checked and passes shellcheck, but hasn't run on macOS or Linux yet.
- The clone is shallow. `git worktree add` doesn't read history, so it makes no difference to
  either method.
- Copies are made one after another, not in parallel, the way an agent asking for a lane gets
  one.
- The timed script runs in `bash` (or `cmd` on Windows), whose start-up, a few milliseconds, is
  in every run of both methods.
- The worktrees are detached, like lanes. Making a branch with `-b` costs about the same.
