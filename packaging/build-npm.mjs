// Builds the npm packages for one release from its archives:
//   <out>/lanes-<os>-<cpu>/   @autter/lanes-<os>-<cpu>, the two binaries for one platform
//   <out>/lanes/              @autter/lanes, which installs the right one and runs it
//
//   node packaging/build-npm.mjs --version 0.1.0 --dist dist --out npm

import { execFileSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');

// Rust target → npm's os and cpu names, and how people call the platform.
const TARGETS = {
  'aarch64-apple-darwin': ['darwin', 'arm64', 'macOS on Apple silicon'],
  'x86_64-apple-darwin': ['darwin', 'x64', 'macOS on Intel'],
  'aarch64-unknown-linux-musl': ['linux', 'arm64', 'Linux on arm64'],
  'x86_64-unknown-linux-musl': ['linux', 'x64', 'Linux on x64'],
  'aarch64-pc-windows-msvc': ['win32', 'arm64', 'Windows on arm64'],
  'x86_64-pc-windows-msvc': ['win32', 'x64', 'Windows on x64'],
};

const args = {};
for (let i = 2; i < process.argv.length; i += 2) args[process.argv[i].replace(/^--/, '')] = process.argv[i + 1];
const version = (args.version ?? '').replace(/^v/, '');
const dist = args.dist ?? 'dist';
const out = args.out ?? 'npm';
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error('build-npm: pass the release version, like --version 0.1.0');
  process.exit(2);
}

const common = {
  version,
  license: 'AGPL-3.0-only',
  author: 'Autter <contact@autter.dev>',
  homepage: 'https://lanes.autter.dev',
  repository: { type: 'git', url: 'git+https://github.com/Autter-dev/lanes.git' },
};
const writeJson = (file, value) => writeFileSync(file, JSON.stringify(value, null, 2) + '\n');

function unpack(archive, into) {
  mkdirSync(into, { recursive: true });
  // Forward slashes, because GNU tar on Windows (Git's) misreads backslashes.
  const [a, d] = [archive, into].map((p) => p.replaceAll('\\', '/'));
  if (!a.endsWith('.zip')) return execFileSync('tar', ['-xzf', a, '-C', d]);
  try {
    execFileSync('unzip', ['-q', a, '-d', d], { stdio: 'ignore' });
  } catch {
    // No unzip: PowerShell on Windows, and bsdtar (which reads zip files) on macOS.
    if (process.platform === 'win32') {
      execFileSync('powershell', ['-NoProfile', '-Command', `Expand-Archive -LiteralPath '${archive}' -DestinationPath '${into}' -Force`]);
    } else {
      execFileSync('tar', ['-xf', a, '-C', d]);
    }
  }
}

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const scratch = join(out, '.unpack');
const optional = {};

for (const [target, [os, cpu, label]] of Object.entries(TARGETS)) {
  const windows = os === 'win32';
  const archive = join(dist, `lanes-${version}-${target}.${windows ? 'zip' : 'tar.gz'}`);
  if (!existsSync(archive)) {
    console.error(`build-npm: ${archive} is missing. Build every target into ${dist} first.`);
    process.exit(1);
  }
  const name = `@autter/lanes-${os}-${cpu}`;
  const dir = join(out, `lanes-${os}-${cpu}`);
  unpack(archive, join(scratch, target));
  mkdirSync(join(dir, 'bin'), { recursive: true });
  for (const bin of ['lane', 'laned']) {
    const file = windows ? `${bin}.exe` : bin;
    const dest = join(dir, 'bin', file);
    cpSync(join(scratch, target, `lanes-${version}-${target}`, file), dest);
    if (!windows) chmodSync(dest, 0o755);
  }
  cpSync(join(root, 'LICENSE'), join(dir, 'LICENSE'));
  writeJson(join(dir, 'package.json'), {
    name,
    ...common,
    description: `The Lanes binaries for ${label}. Install @autter/lanes instead; it picks this for you.`,
    os: [os],
    cpu: [cpu],
    files: ['bin'],
    preferUnplugged: true,
  });
  writeFileSync(
    join(dir, 'README.md'),
    `# ${name}\n\nThe \`lane\` and \`laned\` binaries for ${label}. Install [\`@autter/lanes\`](https://www.npmjs.com/package/@autter/lanes) instead; npm picks this package for you on ${label}.\n`,
  );
  optional[name] = version;
}
rmSync(scratch, { recursive: true, force: true });

const main = join(out, 'lanes');
cpSync(join(here, 'npm', 'lanes'), main, { recursive: true });
cpSync(join(root, 'LICENSE'), join(main, 'LICENSE'));
const pkg = JSON.parse(readFileSync(join(main, 'package.json'), 'utf8'));
writeJson(join(main, 'package.json'), { ...pkg, version, optionalDependencies: optional });

console.log(`build-npm: @autter/lanes ${version} and ${Object.keys(optional).length} platform packages are in ${out}.`);
