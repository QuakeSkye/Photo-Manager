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
  {
    rule: 'rust-handle-delete',
    path: 'src-tauri/src/cache/mod.rs',
    excerpt: 'SetFileInformationByHandle(',
    reason:
      'Phase 1 cache eviction deletes only a registered CacheEntry through the same verified, write/delete-share-denying handle.',
  },
];

const reviewedPermissions = new Map([
  [
    'dialog:allow-open',
    'Phase 3 uses only the native folder picker; the selected path is revalidated by the Rust Root service before binding.',
  ],
]);

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
    pattern:
      /std::process::Command|Command::new\s*\(|IFileOperation|SHCreateItemFromParsingName|ShellExecute|SHFileOperation/,
  },
  {
    id: 'rust-handle-delete',
    include: ['.rs'],
    pattern: /SetFileInformationByHandle\s*\(/,
  },
  {
    id: 'readonly-library-layer-write',
    include: ['.rs'],
    pathPrefixes: [
      'src-tauri/src/catalog/',
      'src-tauri/src/media/',
      'src-tauri/src/roots/',
    ],
    pattern:
      /\.write\s*\(\s*true\s*\)|\.append\s*\(\s*true\s*\)|\.truncate\s*\(\s*true\s*\)|\.create_new\s*\(\s*true\s*\)|fs::create_dir|fs::write|SetFileInformationByHandle/,
  },
  {
    id: 'phase-three-photo-mutation',
    include: ['.rs'],
    pathPrefixes: [
      'src-tauri/src/pending_delete/',
      'src-tauri/src/commands/pending_delete.rs',
    ],
    pattern:
      /std::fs|fs::write|fs::rename|fs::remove|OpenOptions|SetFileInformationByHandle|IFileOperation|SHFileOperation/,
  },
  {
    id: 'phase-three-batch-attempt-creation',
    include: ['.rs'],
    pathPrefixes: ['src-tauri/src/'],
    pattern: /INSERT\s+INTO\s+(?:deletion_batch|deletion_attempt)/i,
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

    const path = relative(root, absolute).replaceAll('\\', '/');
    if (path.endsWith('_tests.rs') || path.endsWith('/tests.rs')) {
      continue;
    }
    const extension = extname(entry.name);
    const applicable = rules.filter(
      (rule) =>
        rule.include.includes(extension) &&
        (!rule.pathPrefixes ||
          rule.pathPrefixes.some((prefix) => path.startsWith(prefix))),
    );
    if (applicable.length === 0) {
      continue;
    }

    const text = await readFile(absolute, 'utf8');
    const productionText =
      extension === '.rs' ? text.split(/\r?\n#\[cfg\(test\)\]/u)[0] : text;
    const lines = productionText.split(/\r?\n/u);

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
  (permission) =>
    permission !== 'core:default' && !reviewedPermissions.has(permission),
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
  phase: 3,
  passed: findings.length === 0,
  scannedAt: new Date().toISOString(),
  reviewedAllowlist,
  reviewedPermissions: Object.fromEntries(reviewedPermissions),
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
    ? 'PASS: phase 3 keeps photo-file writes, Windows Shell calls, deletion execution and batch creation outside the implemented boundary.\n'
    : `FAIL: ${findings.length} forbidden API finding(s). See safety-audit.json.\n`,
  'utf8',
);

console.log(JSON.stringify(result));
process.exit(findings.length === 0 ? 0 : 1);
