// The guide page (/guide/): from install to agents in lanes, written once per system. Every block
// can be limited to some systems with `only`; the page shows the visitor's system and hides the
// rest, and without JavaScript every system shows, each labelled. Text takes `code` and
// [links](url). Every command here is real and does what it says in Lanes 0.0.5.
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
const homeAt: Record<Os, string> = { macos: '/Users/you', linux: '/home/you', windows: 'C:\\Users\\you' };
const configAt: Record<Os, string> = {
  macos: '/Users/you/.config/lane/config.toml',
  linux: '/home/you/.config/lane/config.toml',
  windows: 'C:\\Users\\you\\AppData\\Roaming\\lane\\config.toml',
};
// Codex puts a worktree at <CODEX_HOME>/worktrees/<id>/<repo folder>.
const codexPath = (os: Os) => [homeAt[os], '.codex', 'worktrees', 'a1b2', 'your-app'].join(sep(os));
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
  const rows = [['fix-auth', '4100-4109', lanePath(os, 'fix-auth')], ['codex-a1b2', '4110-4119', codexPath(os)]];
  const w = Math.max(4, ...rows.map(([n]) => n.length));
  const head = `${'NAME'.padEnd(w)}  ${'PORTS'.padEnd(11)}  ${'STATE'.padEnd(10)}  PATH`;
  const row = ([n, p, at]: string[]) => `${n.padEnd(w)}  ${p.padEnd(11)}  ${'ready'.padEnd(10)}  ${at}`;
  return [head, ...rows.map(row)].join('\n');
};

const setupOut = (os: Os) => `Saved: install count on. Settings are in ${configAt[os]}.
Agents in lanes:
  Claude Code  set up: its worktrees become lanes, in every repo
  Codex        set up: its worktrees become lanes, in every repo
               Codex runs new hooks only once you trust them: open /hooks in Codex and trust Lanes' two hooks.
  Cursor       set up: its worktrees become lanes, in every repo`;

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
    id: 'setup',
    title: 'Set it up once.',
    text: 'Run `lane` on its own. The first time, it checks Git, your disk and your agents, and puts the agents it finds in lanes: Claude Code, Codex and Cursor get Lanes\' hooks and a `lanes` skill in their own user settings. From then on, every repo gets lanes, with nothing more to set up. Nothing is sent while you look around.',
    blocks: [
      { kind: 'cmd', cmd: 'lane' },
      { kind: 'note', text: 'Every Mac disk is APFS, so a lane is a copy-on-write clone: instant, and it takes almost no extra space.', only: ['macos'] },
      { kind: 'note', text: 'On Btrfs or XFS, a lane is an instant copy-on-write clone. On ext4 it\'s a plain copy, which works but takes longer and uses more space; `lane setup` says which one you have.', only: ['linux'] },
      { kind: 'note', text: 'On a Dev Drive, a lane is an instant copy-on-write clone. On a regular NTFS drive it\'s a plain copy, which works but takes longer and uses more space. Keep your repos on a Dev Drive if you have one; `lane setup` shows how to make one, and the `git clone` command to move a repo there.', only: win },
      { kind: 'cmd', cmd: 'lane setup --yes', label: 'Or without the welcome, for a script or a new machine' },
      ...perSystem(setupOut),
      { kind: 'note', text: 'Set up Lanes before 0.0.5? Run `lane setup` once more; the home screen says when an agent isn\'t in lanes yet. Changed your mind? `lane setup --yes --agents off` takes back exactly what was added, and nothing else in those files changes.' },
    ],
  },
  {
    id: 'agents',
    inRepo: true,
    title: 'Let your agents make lanes.',
    text: 'Start your agents the way you always do. Each task gets a lane of its own: a ready-to-run copy of the repo with its own ports, dependencies and `.env`. You don\'t run anything, and agents never edit the same files or fight over a port.',
    blocks: [
      { kind: 'cmd', cmd: 'claude -w fix-auth', label: 'Claude Code' },
      { kind: 'note', text: 'Or just ask Claude for the work: the `lanes` skill has it move into a lane of its own first, and you approve the move once. Subagents with `isolation: worktree` and background sessions get lanes too. If Claude tries `git worktree add` or copies the repo, it\'s pointed to `lane new` instead.' },
      { kind: 'cmd', cmd: 'codex --worktree', label: 'Codex' },
      { kind: 'note', text: 'Or pick a worktree for a new chat in the Codex app. When the session starts, the worktree becomes a lane named `codex-<id>`, ready to run, and Codex is told which lane it\'s in. The first time, open `/hooks` in Codex and trust Lanes\' hooks.' },
      { kind: 'cmd', cmd: 'agent -w fix-auth', label: 'Cursor' },
      { kind: 'note', text: 'Or start an agent in a worktree from Cursor\'s Agents window, or with `/worktree`. When it starts, the worktree becomes a lane named `cursor-<name>`, ready to run.' },
      { kind: 'note', text: 'Any other tool: a worktree it makes with Git becomes a lane, dependencies and all, through the Git hook `lane init` adds (step 6).' },
    ],
  },
  {
    id: 'run',
    inRepo: true,
    title: 'See them, and run the app.',
    text: '`lane list` shows every lane with its agent\'s ports and folder. `lane exec` runs a command in one with its own `PORT`, `VITE_PORT` and `LANE_URL`, so the same dev server runs in two lanes at once.',
    blocks: [
      { kind: 'cmd', cmd: 'lane list' },
      ...perSystem(listOut),
      { kind: 'cmd', cmd: 'lane exec fix-auth -- npm run dev' },
      { kind: 'note', text: 'Then open `http://fix-auth.localhost:4100`. Use your own dev command: `lane exec` runs whatever you give it.' },
      { kind: 'cmd', cmd: 'lane exec codex-a1b2 -- npm run dev', label: 'In a second terminal' },
      { kind: 'note', text: 'Codex\'s lane has ports 4110 to 4119, at `http://codex-a1b2.localhost:4110`. Both apps run side by side.' },
      { kind: 'note', text: 'Browsers and `curl.exe` open `*.localhost` addresses. PowerShell\'s `Invoke-WebRequest` can\'t; use `http://127.0.0.1:4100` there.', only: win },
      { kind: 'note', text: 'To put the port in an argument, write `\'$PORT\'` in single quotes and Lanes fills it in: `lane exec fix-auth -- pnpm dev --port \'$PORT\'`.' },
    ],
  },
  {
    id: 'new',
    inRepo: true,
    title: 'Make a lane yourself.',
    text: 'You don\'t need to, but you can. A lane is a real Git worktree that starts from a clean `main`, with dependencies installed and a block of ten ports of its own. No branch is made, and `git status` and `git diff` work as usual.',
    blocks: [
      { kind: 'cmd', cmd: 'lane new fix-auth' },
      ...perSystem(newOut),
      { kind: 'note', text: 'Your times and file count will differ. The first lane takes longest: Lanes makes a clean `_base` copy and installs dependencies there, once. Every lane after that is a copy of it, and so are the dependencies of a lane an agent makes. Every command ends with a "Next:" block like this one.' },
    ],
  },
  {
    id: 'init',
    inRepo: true,
    title: 'Share it with your team.',
    text: 'Lanes finds your stack by itself. To write it down for teammates, run `lane init`: it writes `lane.toml`, adds a short note on working in lanes to `AGENTS.md` for every agent that reads it, and sets up the repo\'s Git hook, so any tool\'s worktrees become lanes. It runs and installs nothing, and running it again is safe.',
    blocks: [
      { kind: 'cmd', cmd: 'cd ~/code/your-app', only: unix },
      { kind: 'cmd', cmd: 'cd C:\\code\\your-app', only: win },
      { kind: 'cmd', cmd: 'lane init' },
      ...perSystem(initOut),
      { kind: 'cmd', cmd: 'git add lane.toml AGENTS.md', label: 'Then check them in' },
      { kind: 'note', text: 'Teammates get the same lanes from `lane.toml`. The hooks stay on each machine, so nobody without Lanes is affected. With Husky, the Git hook is one line in `.husky/post-checkout`, and `lane init` says to add that file too. `lane init --no-agents` writes `lane.toml` only.' },
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
    title: 'Clean up.',
    text: 'Removing a lane frees its ports. When Claude Code removes its worktree, its lane goes too.',
    blocks: [
      { kind: 'cmd', cmd: 'lane rm codex-a1b2' },
      { kind: 'note', text: 'This removes the lane\'s folder, changes and all. `--keep-files` keeps the folder and forgets the lane.' },
      { kind: 'cmd', cmd: 'lane adopt ../my-worktree --ready', only: unix, label: 'Have a worktree already? Make it a lane' },
      { kind: 'cmd', cmd: 'lane adopt ..\\my-worktree --ready', only: win, label: 'Have a worktree already? Make it a lane' },
      { kind: 'note', text: '`--ready` gives it its dependencies and `.env` too, like a lane from `lane new`.' },
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
  'Gemini CLI, Copilot and other agents get lanes only when they make a Git worktree, through the hook `lane init` adds.',
  'No background daemon, checkpoints or undo, and no promote to one pull request.',
  'No database or Docker Compose project per lane yet.',
];
