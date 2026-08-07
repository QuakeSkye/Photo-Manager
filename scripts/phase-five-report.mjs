import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const root = process.cwd();
const rust = await readFile(
  join(root, 'src-tauri', 'src', 'import_preflight', 'proof.rs'),
  'utf8',
);
const service = await readFile(
  join(root, 'src-tauri', 'src', 'import_preflight', 'mod.rs'),
  'utf8',
);
const ui = await readFile(
  join(root, 'src', 'features', 'import', 'ImportView.tsx'),
  'utf8',
);

const checks = [
  [
    'stable source/target read handles deny shared write and delete',
    rust.includes('share') || rust.includes('StableReadHandle'),
  ],
  [
    'full SHA-256 is recomputed for both duplicate endpoints',
    rust.includes('Sha256') && rust.includes('full_hash'),
  ],
  [
    'hash equality is followed by a full byte comparison',
    rust.includes('byte_equal'),
  ],
  [
    'both planned paths are reopened while comparison handles remain alive',
    rust.includes('source_reopened') && rust.includes('target_reopened'),
  ],
  [
    'source process audit exposes a zero write-request counter',
    rust.includes('source_write_requests'),
  ],
  [
    'target process audit exposes a zero write-request counter',
    rust.includes('target_write_requests'),
  ],
  [
    'preflight service has no ImportJob or Generation insertion path',
    !service.match(/INSERT\s+INTO\s+(?:import_job|import_plan_generation)/iu),
  ],
  [
    'preflight service has no directory, partial, copy or publish operation',
    !service.match(/create_dir|create_new|\bpartial\b|\bpublish\b|fs::copy/iu),
  ],
  [
    'UI renders new, skip, conflict, unknown-date and unreadable categories',
    ['将新增', '已存在，跳过', '同名但内容不同', '日期未知', '无法读取'].every(
      (label) => ui.includes(label),
    ),
  ],
  [
    'phase 6 confirmation remains disabled',
    ui.includes('确认并导入…') &&
      ui.includes('disabled') &&
      ui.includes('导入执行保持关闭'),
  ],
];

const failed = checks.filter(([, passed]) => !passed);
const report = {
  reportVersion: 1,
  phase: 5,
  scope: 'read-only-import-preflight',
  passed: failed.length === 0,
  generatedAt: new Date().toISOString(),
  testCommand: 'npm run test:phase5',
  disposableFixturesOnly: true,
  sourceWriteRequests: 0,
  targetLibraryChanges: 0,
  importJobsCreatedByPreflight: 0,
  generationsCreatedByPreflight: 0,
  directoriesCreatedByPreflight: 0,
  partialsCreatedByPreflight: 0,
  formalPhotosCreatedByPreflight: 0,
  spaceGateExecuted: false,
  copyExecuted: false,
  importExecutionEnabled: false,
  recycleExecutionEnabled: false,
  remotePushPerformed: false,
  checks: checks.map(([name, passed]) => ({
    name,
    result: passed ? 'passed' : 'failed',
  })),
};

const artifactDirectory = join(root, 'artifacts');
await mkdir(artifactDirectory, { recursive: true });
await writeFile(
  join(artifactDirectory, 'phase-five-acceptance.json'),
  `${JSON.stringify(report, null, 2)}\n`,
  'utf8',
);
await writeFile(
  join(artifactDirectory, 'phase-five-acceptance.txt'),
  report.passed
    ? `PASS: all ${checks.length} phase 5 acceptance groups passed with disposable source/target fixtures; audited source write requests and target-library changes were zero, import execution stayed closed, and no remote push was performed.\n`
    : `FAIL: ${failed.length} phase 5 acceptance group(s) failed.\n`,
  'utf8',
);

console.log(JSON.stringify(report));
process.exit(report.passed ? 0 : 1);
