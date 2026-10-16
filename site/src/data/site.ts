// All visible copy for the Lanes website. Every claim here is already made in the repo README.
//
// Assumptions (placeholder: true below), mirrored in the hand-off message:
//   - url: the production domain is not decided; https://lanes.autter.dev is a guess.
//   - install: `brew install lane` and `cargo install lane` go live with the first release.
//   - the demo lane names and agents in the hero race are examples, not real activity.

export const site = {
  name: 'Lanes',
  tagline: 'Lighter agents',
  url: 'https://lanes.autter.dev', // placeholder: true
  description:
    'Lanes gives every coding agent its own instant, ready-to-run copy of your repo, with its own ports, URL and database. Free and open source.',
  repo: 'https://github.com/autter-dev/lanes',
  readme: 'https://github.com/autter-dev/lanes#readme',
  changelog: 'https://github.com/autter-dev/lanes/blob/main/CHANGELOG.md',
  license: 'https://github.com/autter-dev/lanes/blob/main/LICENSE',
  x: { label: '@sagnik_ghosh_11', href: 'https://x.com/sagnik_ghosh_11' },
  maker: { name: 'Autter', href: 'https://autter.dev' },
  install: { command: 'brew install lane', alt: 'cargo install lane', live: false }, // placeholder: true
};

export const nav = [
  { label: 'How it helps', href: '/#helps' },
  { label: 'How it works', href: '/#works' },
  { label: 'Get started', href: '/#start' },
  { label: 'Gateway', href: '/#gateway' },
  { label: 'With Autter', href: '/#autter' },
  { label: 'Status', href: '/#status' },
];

// Hero race. f0: where each agent starts; fs: the still frame shown without motion
// (the moment Lanes flags two lanes on the same function). placeholder: true (examples).
export const race = [
  { n: '01', lane: 'fix-auth', agent: 'Claude Code', f0: 0.16, fs: 0.5, lead: true },
  { n: '02', lane: 'add-tests', agent: 'Codex', f0: 0.1, fs: 0.5 },
  { n: '03', lane: 'new-billing', agent: 'Cursor', f0: 0.28, fs: 0.56 },
  { n: '04', lane: 'refactor-db', agent: 'Gemini CLI', f0: 0.06, fs: 0.38 },
];

export const helps = [
  { today: 'A full copy of your repo for every agent', lanes: 'An instant lane that uses almost no extra disk space' },
  { today: 'A fresh npm install (or cargo build, or pip install) for every copy', lanes: 'Dependencies ready in every lane, from the start' },
  { today: 'Five dev servers fighting over port 3000', lanes: 'Its own ports and URL, like fix-auth.localhost' },
  { today: "Agents breaking each other's database with their migrations", lanes: 'Its own database copy (Postgres, SQLite) and Docker Compose project' },
  { today: 'Finding out two agents edited the same function only when you merge', lanes: 'A warning while they are still working' },
];

export const extras = [
  { title: 'Undo anything', text: 'A checkpoint after every agent step. lane undo takes back any step; lane restore jumps back to any checkpoint.' },
  { title: 'Best of N', text: 'lane race gives one task to several agents, runs the tests in each and shows a scoreboard. You keep the best one.' },
  { title: 'Kind to your laptop', text: "Builds and tests take turns, so five agents don't freeze your machine. Dev servers start only when you open them." },
  { title: 'Stays on your machine', text: 'Works offline. The only thing Lanes sends is an anonymous install count, and lane telemetry off stops it.' },
];

// How it works: four chapters of one scroll-driven diagram (src/components/HowItWorks.astro).
export const steps = [
  { title: 'Your repo', text: 'Keep your Git repo, your GitHub and your agents as they are. Nothing to migrate.', cmd: 'lane init' },
  { title: 'A lane per agent', text: 'Each agent gets an instant, ready-to-run lane with its own URL, ports and database. Any Git worktree an agent makes becomes a lane on its own.', cmd: 'lane new fix-auth' },
  { title: 'A warning, not a merge conflict', text: 'The moment two agents open the same function, you see it on both lanes. They can keep going, or you can step in.', cmd: 'lane list' },
  { title: 'One clean PR', text: 'Keep the best lane and turn it into one pull request, with who (and which agent) wrote what. The rest stay out of your branch.', cmd: 'lane promote fix-auth' },
];

export const diagramLanes = [
  { lane: 'fix-auth', agent: 'Claude Code', url: 'fix-auth.localhost', port: ':3001', db: 'db fix_auth' },
  { lane: 'add-tests', agent: 'Codex', url: 'add-tests.localhost', port: ':3002', db: 'db add_tests' },
  { lane: 'new-billing', agent: 'Cursor', url: 'new-billing.localhost', port: ':3003', db: 'db new_billing' },
];

export const photos = {
  track: { alt: 'Curved lanes of a running track, seen from above, in black and white', credit: 'H&CO', href: 'https://unsplash.com/photos/photo-of-red-white-race-track-SOvLVb0lJso' },
  pool: { alt: 'An empty competition pool with five lane ropes, seen from directly above, in black and white', credit: 'Filip Filkovic Philatz', href: 'https://unsplash.com/photos/aerial-photography-of-olympic-pool-b99ZXcIUS-Q' },
};

export const agents = ['Claude Code', 'Cursor', 'Codex', 'Gemini CLI', 'GitHub Copilot', 'Conductor', 'Amp', 'OpenCode', 'Cline', 'Aider'];

export const quickstart = [
  { cmd: 'brew install lane', note: 'or cargo install lane' },
  { cmd: 'cd your-repo && lane init', note: 'finds your stack and sets up your agents' },
  { cmd: 'lane new fix-auth', note: 'a ready-to-run lane' },
  { cmd: 'lane exec fix-auth -- npm run dev', note: 'then open http://fix-auth.localhost' },
];

export const gateway = {
  gets: [
    { title: 'Repo Q&A', text: 'Ask about your repo, or any change in a lane, in plain English.' },
    { title: 'Security checks while agents work', text: 'Not after the pull request is open.' },
    { title: 'A safe-to-merge verdict on every PR', text: 'Including the ones lane promote opens.' },
    { title: 'Cloud agents and CI without cloning', text: 'They read the repo through the gateway.' },
  ],
  how: [
    { title: 'Sign in', text: 'Run lane login to sign in to Autter.', cmd: 'lane login' },
    { title: 'Choose repos', text: 'Pick which repos to connect. Nothing is shared until you do, and every other repo stays on your machine.' },
    { title: 'Work as usual', text: 'Lanes in connected repos reach Autter through the gateway, and Autter checks each lane as its agent works.' },
  ],
  loop: [
    { verb: 'Write', text: 'Agents work in lanes, on your machine or in the cloud.' },
    { verb: 'Check', text: "Autter reviews, tests and security-checks each lane in its cloud while the agent is still working." },
    { verb: 'Ship', text: "lane promote opens one clean PR, and Autter's verdict says when it's safe to merge." },
    { verb: 'Fix', text: 'When something breaks in production, Autter traces it and the fix comes back as a new lane, which goes through the same checks.' },
  ],
  same: [
    'Autter is never required. Lanes stays open source under the AGPL-3.0, works fully without Autter, and is maintained independently of it.',
    'Connecting is opt-in, one repo at a time.',
    "Repos you don't connect never leave your machine. For them, Lanes sends only the anonymous install count, which you can turn off.",
  ],
};

// How an Autter review runs on a lane. Reviews are hosted entirely in Autter's cloud: the lane
// stays on your machine and the gateway lets the cloud read its changes. The findings shown in
// the animation are examples (placeholder: true), not real output.
export const review = {
  steps: [
    { title: 'The agent works in its lane', text: 'On your machine, as usual. Nothing extra runs locally.' },
    { title: 'The gateway shares the changes', text: "For connected repos only, Autter's cloud reads the lane's changes through the gateway. No clone, no copy of the repo to keep in sync." },
    { title: 'Autter reviews in the cloud', text: 'Code review, security checks and tests run on Autter, while the agent is still working.' },
    { title: 'Findings come back to the lane', text: 'Comments land on the lane and in the terminal app, and the PR gets a safe-to-merge verdict.' },
  ],
  checks: ['Code review', 'Security checks', 'Tests'],
  findings: ['2 review comments', 'No security issues', 'Tests pass', 'Safe to merge'],
};

// Built-in guardrails (home page). Planned features; the terminal lines are an example session.
export const guardrails = {
  rows: [
    { title: 'Edits that can’t clobber', text: 'Every patch an agent sends names the version of the file it was made against. If another lane or tool changed it since, the patch is refused with the current lines, and the agent tries again. Nothing is overwritten silently.' },
    { title: 'A person approves what matters', text: 'Agents can’t stop and ask, so Lanes asks for them. A promote that touches protected files, like migrations or CI config, waits for a short code you approve in the terminal app.' },
    { title: 'Every agent has a name', text: 'Each lane records which agent works in it, with its model and session, all the way into the pull request. With Autter, an agent can also get its own access, limited to the repos you pick, that you can switch off at once.' },
    { title: 'Start from any branch', text: 'Make a lane from any branch or ref with lane new --from. When another tool makes its own worktree for a task, Lanes turns it into a lane too.' },
  ],
  terminal: [
    { who: 'agent', cmd: 'lane promote fix-auth' },
    { out: 'protected: migrations/0042_sessions.sql' },
    { out: 'needs approval · code 7KQ-2MD · expires in 10:00', tone: 'warn' },
    { who: 'you', cmd: 'lane approve 7KQ-2MD' },
    { out: 'approved by you · recorded in the PR', tone: 'ok' },
    { out: 'fix-auth promoted · pull request opened', tone: 'ok' },
  ],
};

// Coming next on the gateway (gateway page). Planned, after the gateway launches.
export const gatewayNext = [
  { title: 'Lanes your team can open', text: 'A connected lane’s checkpoints are kept with Autter, not pushed to GitHub. A teammate runs lane open to get your agent’s lane on their machine, with its own ports and database, before it’s a pull request.' },
  { title: 'Agents with their own access', text: 'Register an agent by name, give it access to the repos you choose, and see its name on every change. Switch it off and its writes stop at once.' },
  { title: 'Merged after checks', text: 'A promoted lane can merge through Autter once its checks pass. If it conflicts, you get the files and lines, and nothing is written.' },
];

// Where Lanes runs (home page). Matches the public README; nothing here is a private date.
export const platforms = [
  {
    title: 'Your laptop',
    note: 'macOS, Linux and Windows at launch',
    items: [
      { name: 'macOS', text: 'instant copies on APFS' },
      { name: 'Linux', text: 'instant on Btrfs and XFS; ext4 copies, with a warning and the time it took' },
      { name: 'Windows', text: 'fastest on a Windows 11 Dev Drive; regular NTFS drives work too' },
      { name: 'WSL2', text: 'uses the Linux build' },
    ],
  },
  {
    title: 'Cloud agent sandboxes',
    note: 'one line in the setup script',
    items: [
      { name: 'Claude Code on the web', text: '' },
      { name: 'Codex cloud', text: '' },
      { name: 'Cursor', text: '' },
      { name: 'GitHub Copilot', text: '' },
      { name: 'Jules and Devin', text: '' },
    ],
  },
  {
    title: 'Where your code lives',
    note: 'lane promote opens the pull request there',
    items: [
      { name: 'GitHub', text: 'first' },
      { name: 'GitLab', text: 'next' },
      { name: 'Bitbucket', text: 'next' },
    ],
  },
];

// Public status, mirroring the README's Status table.
export const status = [
  { when: 'Building now', items: ['Instant lanes, lane init, ports and env per lane', 'The terminal app', 'Claude Code integration and the MCP server'] },
  { when: 'Next', items: ['Cursor, Codex, Gemini CLI, Copilot and other agents', 'Databases and Docker Compose per lane, *.localhost URLs', 'Undo, conflict warnings, lane race and lane promote', 'Guardrails: approval codes for protected files', 'Windows: Dev Drive and NTFS', 'Cloud agent setup lines, the GitHub Action', 'Sign-in to Autter (lane login)'] },
  { when: 'December', items: ['The Lanes gateway on Autter', 'Autter reviews on lanes'] },
  { when: 'After the gateway', items: ['Lanes your team can open', 'Agents with their own access', 'Merging after checks'] },
];
