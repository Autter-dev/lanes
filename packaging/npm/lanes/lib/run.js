'use strict';

// Runs the native `lane` or `laned` from the platform package npm installed next to this one
// (@autter/lanes-<os>-<cpu>), passing every argument, the terminal and the exit code through.

const { spawnSync } = require('node:child_process');

const SUPPORTED = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-arm64', 'win32-x64'];

module.exports = function run(name) {
  const key = `${process.platform}-${process.arch}`;
  const pkg = `@autter/lanes-${key}`;
  const exe = process.platform === 'win32' ? `${name}.exe` : name;

  let bin;
  try {
    bin = require.resolve(`${pkg}/bin/${exe}`);
  } catch {
    const fix = SUPPORTED.includes(key)
      ? `npm skipped it, which happens with --omit=optional or --no-optional. Reinstall with: npm install -g @autter/lanes`
      : `Lanes has builds for macOS, Linux and Windows on x64 and arm64. See https://lanes.autter.dev/download for other ways to install.`;
    process.stderr.write(`lanes: the ${key} build (${pkg}) isn't installed. ${fix}\n`);
    process.exit(1);
  }

  const result = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) {
    process.stderr.write(`lanes: couldn't start ${bin}: ${result.error.message}. Reinstall with: npm install -g @autter/lanes\n`);
    process.exit(1);
  }
  if (result.signal) {
    process.kill(process.pid, result.signal);
    return;
  }
  process.exit(result.status === null ? 1 : result.status);
};
