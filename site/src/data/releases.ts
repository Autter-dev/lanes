// Every published release, newest first, read from the repo's CHANGELOG.md at build time. A
// release is a `## <version> (<YYYY-MM-DD>)` section; its text before the first `###` is the
// summary. Versions newer than release.version (still being built) are left out.

import changelog from '../../../CHANGELOG.md?raw';
import { archivesFor, installVersion, release, releaseFile, releaseNotes } from './site';

export type Release = {
  version: string;
  date: string;
  latest: boolean;
  preview: boolean;
  summary: string; // HTML
  notes: string;
  archives: ReturnType<typeof archivesFor>;
  sums: string;
  installers: { name: string; url: string }[];
  install: ReturnType<typeof installVersion>;
};

const parts = (v: string) => v.split(/[-+]/)[0].split('.').map(Number);
const compare = (a: string, b: string) => {
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  // 1.0.0-rc.1 comes before 1.0.0
  return (b.includes('-') ? 1 : 0) - (a.includes('-') ? 1 : 0);
};

const escape = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const inline = (s: string) =>
  escape(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a class="link" href="$2">$1</a>');

// Paragraphs and one level of bullets: all a release summary uses.
function render(md: string): string {
  const out: string[] = [];
  let list: string[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push(`<p>${inline(para.join(' '))}</p>`);
    if (list.length) out.push(`<ul>${list.map((l) => `<li>${inline(l)}</li>`).join('')}</ul>`);
    para = [];
    list = [];
  };
  for (const line of md.split('\n')) {
    const item = line.match(/^\s*-\s+(.*)$/);
    if (item) {
      if (para.length) { out.push(`<p>${inline(para.join(' '))}</p>`); para = []; }
      list.push(item[1]);
    } else if (!line.trim()) {
      flush();
    } else if (list.length && /^\s+/.test(line)) {
      list[list.length - 1] += ` ${line.trim()}`;
    } else {
      if (list.length) { out.push(`<ul>${list.map((l) => `<li>${inline(l)}</li>`).join('')}</ul>`); list = []; }
      para.push(line.trim());
    }
  }
  flush();
  return out.join('\n');
}

function parse(): Release[] {
  const lines = changelog.split('\n');
  const found: Release[] = [];
  lines.forEach((line, i) => {
    const m = line.match(/^## \[?v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\]?\s*\((\d{4}-\d{2}-\d{2})\)/);
    if (!m) return;
    const [, version, date] = m;
    if (compare(version, release.version) > 0) return;
    const rest = lines.slice(i + 1);
    const end = rest.findIndex((l) => /^##\s/.test(l));
    const body = (end < 0 ? rest : rest.slice(0, end)).join('\n');
    const summary = body.split(/^### /m)[0].trim();
    found.push({
      version,
      date,
      latest: version === release.version,
      preview: version.startsWith('0.0.'),
      summary: render(summary),
      notes: releaseNotes(version),
      archives: archivesFor(version),
      sums: releaseFile(version, 'SHA256SUMS'),
      installers: ['lanes-installer.sh', 'lanes-installer.ps1'].map((name) => ({ name, url: releaseFile(version, name) })),
      install: installVersion(version),
    });
  });
  return found.sort((a, b) => compare(b.version, a.version));
}

export const allReleases = parse();

// Long dates for people: "17 October 2026".
export const longDate = (iso: string) =>
  new Date(`${iso}T00:00:00Z`).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' });
