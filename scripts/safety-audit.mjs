import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { extname, join, relative } from 'node:path';

const root = process.cwd();
const findings = [];
const reviewedAllowlist = [
  {
    rule: 'rust-permanent-delete',
    path: 'src-tauri/src/db/backup.rs',
    excerpt: 'fs::remove_file(expired)',
    reason:
      'Retention only removes enumerated .sqlite3 snapshots from the application-owned backup directory.',
  },
];

const rules = [
  {
    id: 'frontend-general-fs-shell',
    include: ['.ts', '.tsx', '.js', '.jsx'],
    pattern:
      /@tauri-apps\/plugin-(?:fs|shell)|from\s+['"](?:node:)?fs(?:\/promises)?['"]|child_process/,
  },
  {
    id: 'rust-overwrite-rename',
    include: ['.rs'],
    pattern: /std::fs::rename|fs::rename\s*\(/,
  },
  {
    id: 'rust-permanent-delete',
    include: ['.rs'],
    pattern:
      /std::fs::remove_file|fs::remove_file\s*\(|remove_dir_all|DeleteFile[AW]?\s*\(|SHFileOperation/,
  },
  {
    id: 'rust-general-shell',
    include: ['.rs'],
    pattern: /std::process::Command|Command::new\s*\(/,
  },
];

const excludedDirectories = new Set([
  '.git',
  '.superpowers',
  'artifacts',
  'dist',
  'node_modules',
  'target',
  'tests',
]);

async function walk(directory) {
  const entries = await readdir(directory, { withFileTypes: true });

  for (const entry of entries) {
    if (excludedDirectories.has(entry.name)) {
      continue;
    }

    const absolute = join(directory, entry.name);
    if (entry.isDirectory()) {
      await walk(absolute);
      continue;
    }

    const extension = extname(entry.name);
    const applicable = rules.filter((rule) => rule.include.includes(extension));
    if (applicable.length === 0) {
      continue;
    }

    const path = relative(root, absolute).replaceAll('\\', '/');
    const text = await readFile(absolute, 'utf8');
    const lines = text.split(/\r?\n/u);

    for (const rule of applicable) {
      lines.forEach((line, index) => {
        if (rule.pattern.test(line)) {
          const reviewed = reviewedAllowlist.some(
            (entry) =>
              entry.rule === rule.id &&
              entry.path === path &&
              line.includes(entry.excerpt),
          );
          if (reviewed) {
            return;
          }
          findings.push({
            rule: rule.id,
            path,
            line: index + 1,
            excerpt: line.trim().slice(0, 180),
          });
        }
      });
    }
  }
}

await walk(root);

const capabilityText = await readFile(
  join(root, 'src-tauri', 'capabilities', 'default.json'),
  'utf8',
);
const capability = JSON.parse(capabilityText);
const unexpectedPermissions = capability.permissions.filter(
  (permission) => permission !== 'core:default',
);
for (const permission of unexpectedPermissions) {
  findings.push({
    rule: 'tauri-capability-not-allowlisted',
    path: 'src-tauri/capabilities/default.json',
    line: 1,
    excerpt: String(permission),
  });
}

const result = {
  reportVersion: 1,
  phase: 0,
  passed: findings.length === 0,
  scannedAt: new Date().toISOString(),
  reviewedAllowlist,
  findings,
};

await mkdir(join(root, 'artifacts'), { recursive: true });
await writeFile(
  join(root, 'artifacts', 'safety-audit.json'),
  `${JSON.stringify(result, null, 2)}\n`,
  'utf8',
);
await writeFile(
  join(root, 'artifacts', 'safety-audit.txt'),
  findings.length === 0
    ? 'PASS: no forbidden filesystem, permanent-delete, overwrite-rename, or general-shell API was found in production paths.\n'
    : `FAIL: ${findings.length} forbidden API finding(s). See safety-audit.json.\n`,
  'utf8',
);

console.log(JSON.stringify(result));
process.exit(findings.length === 0 ? 0 : 1);
