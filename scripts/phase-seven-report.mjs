import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { basename, join } from 'node:path';

const root = process.cwd();
const read = (...parts) => readFile(join(root, ...parts), 'utf8');
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

const recycle = await read('src-tauri', 'src', 'recycle', 'mod.rs');
const shell = await read('src-tauri', 'src', 'recycle', 'shell.rs');
const tests = await read('src-tauri', 'src', 'recycle', 'tests.rs');
const migration = await read('src-tauri', 'migrations', '0007_phase_seven.sql');
const ui = await read('src', 'app', 'App.tsx');
const audit = JSON.parse(await read('artifacts', 'safety-audit.json'));
const windowsResults = JSON.parse(
  (await read('artifacts', 'phase-seven-windows-results.json')).replace(
    /^\uFEFF/u,
    '',
  ),
);
const cargoLock = await readFile(join(root, 'src-tauri', 'Cargo.lock'));
const packageLock = await readFile(join(root, 'package-lock.json'));
const binaryPath = join(
  root,
  'src-tauri',
  'target',
  'release',
  'local-photo-manager.exe',
);
const binary = await readFile(binaryPath);

const checks = [
  [
    'final confirmation freezes logical Photo count, physical file count and bytes',
    recycle.includes('logical_photo_count') &&
      recycle.includes('physical_file_count') &&
      recycle.includes('total_bytes'),
  ],
  [
    'TaskIntent, Batch, initial Attempt, Report, Lease and Reservations share one transaction',
    recycle.includes('fn create_initial_attempt') &&
      recycle.includes('INSERT INTO task_intent') &&
      recycle.includes('INSERT INTO deletion_batch') &&
      recycle.includes('INSERT INTO operation_report') &&
      recycle.includes('file_write_lease') &&
      recycle.includes('operation_reservation'),
  ],
  [
    'BatchAsset copies only immutable DeletionMarkAssetSnapshot identity',
    migration.includes('deletion batch asset plan is immutable') &&
      recycle.includes('mark_snapshot_id'),
  ],
  [
    'dedicated COM STA worker uses one IFileOperation per Photo',
    shell.includes('recycle-com-sta') &&
      recycle.includes('for photo in &attempt.photos') &&
      recycle.includes('recycle_photo(shell_photo'),
  ],
  [
    'recycle flags are mandatory and permanent-delete fallback is absent',
    shell.includes(
      'FOFX_RECYCLEONDELETE | FOF_NOERRORUI | FOFX_EARLYFAILURE | FOF_NOCONFIRMATION',
    ) &&
      !/FOF_WANTNUKEWARNING|SHFileOperation|DeleteFile[AW]?\s*\(|remove_file\s*\(/u.test(
        shell,
      ),
  ],
  [
    'PreDeleteItem revalidates the frozen identity',
    shell.includes('fn PreDeleteItem') &&
      recycle.includes('fn pre_delete') &&
      recycle.includes('validate_frozen_asset'),
  ],
  [
    'PostDeleteItem synchronously commits each RecycledFact before returning',
    shell.includes('fn PostDeleteItem') &&
      recycle.includes('fn post_delete') &&
      recycle.includes('deletion_recycled_fact') &&
      recycle.includes('database.writer()?.transaction'),
  ],
  [
    'unknown, partial and database-failure outcomes stop conservatively',
    tests.includes(
      'empty_recycle_object_and_predelete_replacement_stop_as_needs_attention',
    ) &&
      tests.includes(
        'postdelete_database_failure_stops_and_preserves_unknown_result',
      ) &&
      recycle.includes('CriticalSafetyViolation'),
  ],
  [
    'retry creates a new Attempt and cancel preserves old facts',
    tests.includes(
      'ordinary_failure_is_retryable_and_retry_creates_a_new_attempt',
    ) &&
      recycle.includes('fn create_retry_attempt') &&
      recycle.includes('CancelledRemaining'),
  ],
  [
    'startup reconciliation never automatically replays Shell',
    tests.includes(
      'interrupted_attempt_is_reconciled_without_automatic_shell_replay',
    ) && recycle.includes('fn reconcile_interrupted'),
  ],
  [
    'pending entry is derived to zero only after all intents resolve',
    tests.includes('clears_pending_entry') &&
      recycle.includes('release_batch_marks'),
  ],
  [
    'UI confirmation includes all three frozen quantities and recycle-only warning',
    ui.includes('逻辑照片') &&
      ui.includes('物理 JPG / RAF 文件') &&
      ui.includes('合计容量') &&
      ui.includes('不会永久删除'),
  ],
  [
    'real Windows STA pair recycle and restore test passed on disposable NTFS',
    windowsResults.test_root_only === true &&
      windowsResults.tests.some(
        (test) =>
          test.test ===
            'real_sta_worker_recycles_pair_and_restores_identical_content' &&
          test.result === 'passed',
      ),
  ],
  [
    'safety audit passed with no permanent-delete production path',
    audit.phase === 7 && audit.passed === true && audit.findings.length === 0,
  ],
];

const failed = checks.filter(([, passed]) => !passed);
const report = {
  reportVersion: 1,
  phase: 7,
  scope: 'windows-recycle-engineering-capability-on-disposable-ntfs-only',
  passed: failed.length === 0,
  generatedAt: new Date().toISOString(),
  testCommand: 'npm run test:phase7',
  testRootOnly: true,
  userUniquePhotosTouched: false,
  permanentDeleteAvailable: false,
  permanentDeleteFallbackAvailable: false,
  realFujiLibraryRecycleAdmissionCreated: false,
  realFujiLibraryRecycleGateOpened: false,
  phaseEightEntered: false,
  remotePushPerformed: false,
  schemaVersion: 7,
  schemaMigrationSha256: sha256(migration),
  testedReleaseBinary: basename(binaryPath),
  testedReleaseBinarySha256: sha256(binary),
  cargoLockSha256: sha256(cargoLock),
  packageLockSha256: sha256(packageLock),
  windowsResults,
  checks: checks.map(([name, passed]) => ({
    name,
    result: passed ? 'passed' : 'failed',
  })),
};

const artifactDirectory = join(root, 'artifacts');
const reportDirectory = join(root, 'docs', 'superpowers', 'reports');
await mkdir(artifactDirectory, { recursive: true });
await mkdir(reportDirectory, { recursive: true });
await writeFile(
  join(artifactDirectory, 'phase-seven-recycle-capability.json'),
  `${JSON.stringify(report, null, 2)}\n`,
  'utf8',
);
await writeFile(
  join(artifactDirectory, 'phase-seven-recycle-capability.txt'),
  report.passed
    ? `PASS: all ${checks.length} phase 7 recycle engineering capability groups passed on disposable NTFS fixtures.\n`
    : `FAIL: ${failed.length} phase 7 group(s) failed.\n`,
  'utf8',
);

const markdown = `# 阶段 7：Windows 回收站工程能力测试记录

- 结果：${report.passed ? 'PASS' : 'FAIL'}
- 范围：仅可丢弃 NTFS 测试 Root 上的 Recycle 工程能力
- 用户唯一照片操作：否
- 永久删除或降级：无
- 真实 FujiLibrary Recycle 准入：未生成
- 真实 FujiLibrary Recycle 门：未开启
- 阶段 8：未进入
- 远端推送：否
- Schema 版本：${report.schemaVersion}
- Migration SHA-256：\`${report.schemaMigrationSha256}\`
- 已测试 Release 二进制：\`${report.testedReleaseBinary}\`
- 二进制 SHA-256：\`${report.testedReleaseBinarySha256}\`
- Cargo.lock SHA-256：\`${report.cargoLockSha256}\`
- package-lock.json SHA-256：\`${report.packageLockSha256}\`

## 验证项

${report.checks.map((check) => `- ${check.result === 'passed' ? 'PASS' : 'FAIL'} — ${check.name}`).join('\n')}

本记录只证明当前精确构建在应用生成的可丢弃 NTFS TestRoot 上具备回收站工程能力。它不是阶段 8 的真实 FujiLibrary Recycle 准入，运行时仍会拒绝没有 TestRoot 标记的真实图库；测试后任何重新构建都会使本记录中的二进制哈希失效。
`;
await writeFile(
  join(reportDirectory, 'phase-seven-recycle-capability-report.md'),
  markdown,
  'utf8',
);

console.log(JSON.stringify(report));
process.exit(report.passed ? 0 : 1);
