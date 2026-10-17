// The guide page (/guide/): from install to agents in lanes, written once per system. Every block
// can be limited to some systems with `only`; the page shows the visitor's system and hides the
// rest, and without JavaScript every system shows, each labelled. Text takes `code` and
// [links](url). Every command here is real and does what it says in Lanes 0.0.4.
import { install, release, type Os } from './site';

export type Block =
  | { kind: 'cmd'; cmd: string; only?: Os[]; label?: string }
  | { kind: 'out'; text: string; only?: Os[] }
  | { kind: 'note'; text: string; only?: Os[] };

// inRepo: the step's commands run in the repo's folder (the card's bar says where).
export type Step = { id: string; title: string; text: string; blocks: Block[]; inRepo?: boolean };

export const systems: { os: Os; name: string; shell: string; repo: string }[] = [
  { os: 'macos', name: 'macOS', shell: 'Terminal (zsh)', repo: '~/code/your-app' },
  { os: 'linux', name: 'Linux', shell: 'bash or zsh', repo: '~/code/your-app' },
  { os: 'windows', name: 'Windows', shell: 'PowerShell', repo: 'C:\\code\\your-app' },
];

const unix: Os[] = ['macos', 'linux'];
const win: Os[] = ['windows'];
const every: Os[] = ['macos', 'linux', 'windows'];

// Where the example repo lives, as `lane` prints it (absolute, in the system's own spelling).
const repoAt: Record<Os, string> = {
  macos: '/Users/you/code/your-app',
  linux: '/home/you/code/your-app',
  windows: 'C:\\code\\your-app',
};
const sep = (os: Os) => (os === 'windows' ? '\\' : '/');
const lanePath = (os: Os, name: string) => `${repoAt[os]}.lanes${sep(os)}${name}`;

// The "Next:" block, laid out exactly as `lane` does it (lane_core::next::render): commands in
// a column, and a command too long for it gets its why on the line below.
function nextBlock(steps: [string, string][]): string {
  const width = Math.max(0, ...steps.map(([r]) => r.length).filter((w) => w <= 48));
  const line = ([run, why]: [string, string]) =>
    run.length > 48 ? `  ${run}\n  ${''.padEnd(width)}  ${why}` : `  ${run.padEnd(width)}  ${why}`;
  return ['Next:', ...steps.map(line)].join('\n');
}

const initOut = (os: Os) => `Found: npm, Vite
  install  npm ci
  dev      npm run dev -- --port $PORT (usually port 5173)
Wrote ${repoAt[os]}${sep(os)}lane.toml.
Agents:
  Git hook     post-checkout: worktrees other tools make become lanes
  AGENTS.md    a note on working in lanes
  Claude Code  hooks in .claude/settings.local.json, kept out of Git
${nextBlock([
  ['lane new fix-auth', 'make a lane; dependencies install once, the first time'],
  ['claude -w fix-auth', 'or start Claude Code in a lane of its own'],
  ['git add lane.toml AGENTS.md', 'check it in, so teammates get the same lanes'],
])}`;

const newOut = (os: Os) => `Made lane fix-auth in 2.4 s (1,284 files cloned in 0.3 s).
  folder  ${lanePath(os, 'fix-auth')}
  ports   4100-4109
  base    main at 4f3863f, set up just now; dependencies installed
${nextBlock([
  [`cd ${lanePath(os, 'fix-auth')}`, 'start your agent here, so it works only in this lane'],
  ['lane exec fix-auth -- npm run dev -- --port 4100', 'run the app at http://fix-auth.localhost:4100'],
])}`;

const listOut = (os: Os) => {
  const rows = [['fix-auth', '4100-4109'], ['dark-mode', '4110-4119']];
  const w = Math.max(4, ...rows.map(([n]) => n.length));
  const head = `${'NAME'.padEnd(w)}  ${'PORTS'.padEnd(11)}  ${'STATE'.padEnd(10)}  PATH`;
  const row = ([n, p]: string[]) => `${n.padEnd(w)}  ${p.padEnd(11)}  ${'ready'.padEnd(10)}  ${lanePath(os, n)}`;
  return [head, ...rows.map(row)].join('\n');
};

const perSystem = (make: (os: Os) => string): Block[] => every.map((os) => ({ kind: 'out', text: make(os), only: [os] }));

export const steps: Step[] = [
  {
    id: 'install',
    title: 'Install Lanes.',
    text: 'One command installs `lane` and its `laned` daemon. Every way of installing gets you the same two programs.',
    blocks: [
      { kind: 'cmd', cmd: install.brew, only: ['macos'] },
      { kind: 'note', text: 'No Homebrew? `curl -fsSL https://lanes.autter.dev/install.sh | sh` puts Lanes in `~/.local/bin` instead.', only: ['macos'] },
      { kind: 'cmd', cmd: install.sh, only: ['linux'] },
      { kind: 'note', text: 'It puts Lanes in `~/.local/bin`, on x64 or arm64. If that folder isn\'t on your PATH yet, the installer prints the line to add. Homebrew on Linux works too: `brew install autter-dev/tap/lanes`.', only: ['linux'] },
      { kind: 'cmd', cmd: install.ps1, only: win },
      { kind: 'note', text: 'Run it in PowerShell. It puts `lane.exe` and `laned.exe` in `%LOCALAPPDATA%\\Programs\\Lanes\\bin` and adds that to your PATH, so open a new terminal afterwards. Builds aren\'t code-signed yet, so Smart App Control on Windows 11 may block them: [here\'s what to do](/download/#dl-unsigned).', only: win },
      { kind: 'cmd', cmd: 'lane --version' },
      { kind: 'out', text: `lane ${release.version} (by Autter)` },
      { kind: 'note', text: 'Scoop, npm and plain archives are on the [download page](/download/).' },
    ],
  },
  {
    id: 'machine',
    title: 'Check your machine.',
    text: 'Run `lane` on its own. The first time, it checks Git, your disk and the agents you have, and shows a step-by-step fix for anything that\'s off. Nothing is sent while you look around.',
    blocks: [
      { kind: 'cmd', cmd: 'lane' },
      { kind: 'note', text: 'Every Mac disk is APFS, so a lane is a copy-on-write clone: instant, and it takes almost no extra space.', only: ['macos'] },
      { kind: 'note', text: 'On Btrfs or XFS, a lane is an instant copy-on-write clone. On ext4 it\'s a plain copy, which works but takes longer and uses more space; `lane setup` says which one you have.', only: ['linux'] },
      { kind: 'note', text: 'On a Dev Drive, a lane is an instant copy-on-write clone. On a regular NTFS drive it\'s a plain copy, which works but takes longer and uses more space. Keep your repos on a Dev Drive if you have one; `lane setup` shows how to make one, and the `git clone` command to move a repo there.', only: win },
      { kind: 'cmd', cmd: 'lane setup', label: 'Run the checks again any time' },
    ],
  },
  {
    id: 'init',
    inRepo: true,
    title: 'Set up a repo.',
    text: 'In a repo you work on, `lane init` finds your package manager, dev server and `.env` template, and writes `lane.toml`. It sets up your agents too: Claude Code\'s hooks, a Git hook and a short note in `AGENTS.md`. It runs and installs nothing, and running it again is safe.',
    blocks: [
      { kind: 'cmd', cmd: 'cd ~/code/your-app', only: unix },
      { kind: 'cmd', cmd: 'cd C:\\code\\your-app', only: win },
      { kind: 'cmd', cmd: 'lane init' },
      ...perSystem(initOut),
      { kind: 'cmd', cmd: 'git add lane.toml AGENTS.md', label: 'Then check them in' },
      { kind: 'note', text: 'Teammates get the same lanes from `lane.toml`. Claude Code\'s hooks stay on your machine, so nobody without Lanes is affected. `lane init --no-agents` writes `lane.toml` only.' },
    ],
  },
  {
    id: 'new',
    inRepo: true,
    title: 'Make a lane.',
    text: 'A lane is a ready-to-run copy of your repo: a real Git worktree that starts from a clean `main`, with dependencies installed and a block of ten ports of its own. No branch is made, and `git status` and `git diff` work as usual.',
    blocks: [
      { kind: 'cmd', cmd: 'lane new fix-auth' },
      ...perSystem(newOut),
      { kind: 'note', text: 'Your times and file count will differ. The first lane takes longest: Lanes makes a clean `_base` copy and installs dependencies there, once. Every lane after that is a copy of it. Every command ends with a "Next:" block like this one.' },
    ],
  },
  {
    id: 'run',
    inRepo: true,
    title: 'Run the app in it.',
    text: '`lane exec` runs a command in the lane with its own `PORT`, `VITE_PORT` and `LANE_URL`. So the same dev server can run in two lanes at once, without a port clash.',
    blocks: [
      { kind: 'cmd', cmd: 'lane exec fix-auth -- npm run dev' },
      { kind: 'note', text: 'Then open `http://fix-auth.localhost:4100`. Use your own dev command: `lane init` printed it.' },
      { kind: 'cmd', cmd: 'lane new dark-mode', label: 'In a second terminal' },
      { kind: 'cmd', cmd: 'lane exec dark-mode -- npm run dev' },
      { kind: 'note', text: 'The second lane gets ports 4110 to 4119, at `http://dark-mode.localhost:4110`. Both apps run side by side.' },
      { kind: 'note', text: 'Browsers and `curl.exe` open `*.localhost` addresses. PowerShell\'s `Invoke-WebRequest` can\'t; use `http://127.0.0.1:4100` there.', only: win },
      { kind: 'note', text: 'To put the port in an argument, write `\'$PORT\'` in single quotes and Lanes fills it in: `lane exec fix-auth -- pnpm dev --port \'$PORT\'`.' },
    ],
  },
  {
    id: 'agents',
    inRepo: true,
    title: 'Put agents in lanes.',
    text: 'Give each agent its own lane, and they never edit the same files or fight over a port.',
    blocks: [
      { kind: 'cmd', cmd: 'claude -w fix-auth', label: 'Claude Code' },
      { kind: 'note', text: 'After `lane init`, this makes a lane and starts Claude Code in it, with the lane\'s ports set. If it tries `git worktree add`, it\'s pointed to `lane new` instead. When you remove the worktree, the lane goes too.' },
      { kind: 'cmd', cmd: '/plugin marketplace add Autter-dev/lanes', label: 'Or in Claude Code, for every repo at once' },
      { kind: 'cmd', cmd: '/plugin install lanes@autter' },
      { kind: 'cmd', cmd: 'cd ~/code/your-app.lanes/fix-auth', only: unix, label: 'Any other agent: start it in a lane\'s folder' },
      { kind: 'cmd', cmd: 'cd C:\\code\\your-app.lanes\\fix-auth', only: win, label: 'Any other agent: start it in a lane\'s folder' },
      { kind: 'note', text: 'Then start Codex, Gemini CLI, Aider or any other agent there. When an agent makes its own Git worktree, the hook `lane init` added turns it into a lane.' },
    ],
  },
  {
    id: 'env',
    inRepo: true,
    title: 'Use a lane by hand.',
    text: 'To work in a lane yourself, load its variables into your shell. Then `PORT` and `LANE_URL` are set for everything you run.',
    blocks: [
      { kind: 'cmd', cmd: 'eval "$(lane env fix-auth --shell sh)"', only: unix },
      { kind: 'note', text: 'zsh, the macOS default, reads it as it is.', only: ['macos'] },
      { kind: 'note', text: 'bash, zsh and other POSIX shells read it as it is.', only: ['linux'] },
      { kind: 'cmd', cmd: 'lane env fix-auth --shell powershell | Invoke-Expression', only: win },
      { kind: 'note', text: 'In cmd, `lane env fix-auth --shell cmd` prints `set` lines to run instead.', only: win },
      { kind: 'cmd', cmd: 'lane env fix-auth', label: 'Or just look' },
    ],
  },
  {
    id: 'manage',
    inRepo: true,
    title: 'Keep track, and clean up.',
    text: 'Lanes keeps a list of every lane with its folder and ports. Removing one frees its ports.',
    blocks: [
      { kind: 'cmd', cmd: 'lane list' },
      ...perSystem(listOut),
      { kind: 'cmd', cmd: 'lane rm dark-mode' },
      { kind: 'note', text: 'This removes the lane\'s folder, changes and all. `--keep-files` keeps the folder and forgets the lane.' },
      { kind: 'cmd', cmd: 'lane adopt ../my-worktree', only: unix, label: 'Have a worktree already? Make it a lane' },
      { kind: 'cmd', cmd: 'lane adopt ..\\my-worktree', only: win, label: 'Have a worktree already? Make it a lane' },
    ],
  },
  {
    id: 'scripts',
    inRepo: true,
    title: 'Script it.',
    text: 'Every command takes `--json` and prints one JSON object, errors included, with a `next` field that says what to run next. Exit codes never change: 0 done, 1 failed, 2 wrong command line. Writes take `--request-id`, so a retry never makes a second lane.',
    blocks: [
      { kind: 'cmd', cmd: 'lane new fix-auth --json --request-id task-42' },
      { kind: 'cmd', cmd: 'lane --help', label: 'Every command, and every command takes --help' },
    ],
  },
];

// Update and remove, per system.
export const upkeep: { os: Os; update: string; remove: string; note?: string }[] = [
  { os: 'macos', update: 'brew upgrade lanes', remove: 'brew uninstall lanes', note: 'Installed with the script? Run it again to update, and delete `lane` and `laned` from `~/.local/bin` to remove it.' },
  { os: 'linux', update: install.sh, remove: 'rm ~/.local/bin/lane ~/.local/bin/laned', note: 'The script updates in place. Installed with Homebrew? Use `brew upgrade lanes`.' },
  { os: 'windows', update: install.ps1, remove: 'Remove-Item -Recurse "$env:LOCALAPPDATA\\Programs\\Lanes"', note: 'The script updates in place. Installed with Scoop? Use `scoop update lanes` and `scoop uninstall lanes`.' },
];

// Coming, not built: said plainly so nobody looks for it.
export const notYet = [
  'Other agents (Cursor, Codex, Gemini CLI, Copilot) get lanes through the Git hook, but no session context yet.',
  'No background daemon, checkpoints or undo, and no promote to one pull request.',
  'No database or Docker Compose project per lane yet.',
];
