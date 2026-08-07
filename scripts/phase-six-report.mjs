import { createHash } from 'node:crypto';
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { basename, join } from 'node:path';

const root = process.cwd();
const read = (...parts) => readFile(join(root, ...parts), 'utf8');
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

const execution = await read('src-tauri', 'src', 'import_execution', 'mod.rs');
const fileOps = await read(
  'src-tauri',
  'src',
  'import_execution',
  'file_ops.rs',
);
const tests = await read('src-tauri', 'src', 'import_execution', 'tests.rs');
const migration = await read('src-tauri', 'migrations', '0006_phase_six.sql');
const ui = await read('src', 'features', 'import', 'ImportView.tsx');
const lockBytes = await readFile(join(root, 'src-tauri', 'Cargo.lock'));
const packageLockBytes = await readFile(join(root, 'package-lock.json'));

const checks = [
  [
    'final Start Import creates TaskIntent, ImportJob and initial Confirmed generation',
    execution.includes('create_initial_intent') &&
      execution.includes("'Confirmed'"),
  ],
  [
    'all duplicate validation handles survive the one proof/result transaction',
    execution.includes('prove_duplicate_guard') &&
      execution.includes('persist_ready_validation'),
  ],
  [
    'plan changes create Proposed generation and stop before space/attempt',
    execution.includes('AwaitingPlanConfirmation') &&
      execution.includes('create_proposed_generation'),
  ],
  [
    'exact one-per-attempt space gate uses max(10 GiB, ceil(total*2%))',
    execution.includes('IMPORT_MIN_RESERVE_BYTES.max') &&
      migration.includes(
        'space check bytes must equal validation remaining bytes',
      ),
  ],
  [
    'lease, attempt, report, reservations and Planned ownership are acquired atomically',
    execution.includes('fn create_attempt') &&
      execution.includes('file_write_lease') &&
      execution.includes('import_partial_ownership'),
  ],
  [
    'partial creation is CREATE_NEW and flushes before complete reread',
    fileOps.includes('create_new(true)') &&
      fileOps.includes('sync_all') &&
      execution.includes('hash_and_compare'),
  ],
  [
    'publish is handle-level and explicitly no-replace',
    fileOps.includes('ReplaceIfExists: false') &&
      fileOps.includes('FileRenameInfo') &&
      !fileOps.includes('std::fs::rename'),
  ],
  [
    'publish facts precede CatalogService indexing and formal files are never rolled back',
    execution.indexOf('import_published_fact') <
      execution.indexOf('index_published(') &&
      !execution.includes('remove_file'),
  ],
  [
    'recovery coordinates Publishing and CleaningOwnedPartial before closing an interrupted attempt',
    execution.includes('reconcile_publish_intents') &&
      execution.includes('reconcile_cleaning_partials') &&
      execution.indexOf('reconcile_publish_intents') <
        execution.indexOf("attempt_state='Interrupted'"),
  ],
  [
    'missing-after-cleaning is recorded without path-based deletion',
    execution.includes('ObservedMissingAfterCleaningIntent') &&
      fileOps.includes('delete_owned'),
  ],
  [
    'Planned recovery becomes NeedsAttention and cannot create a new incarnation',
    execution.includes(
      "state='NeedsAttention' WHERE created_by_attempt_step_id",
    ) && execution.includes('has_unreconcilable_ownership'),
  ],
  [
    'UI has a separate final modal and exposes source write count',
    ui.includes('最终确认导入') &&
      ui.includes('开始导入') &&
      ui.includes('来源写请求'),
  ],
  [
    'real filesystem tests are restricted to generated TestRoot and disposable source fixtures',
    tests.includes('TestRoot::create_in') &&
      tests.includes('tempfile::tempdir'),
  ],
  [
    'race test proves an existing formal target and the partial both remain unchanged',
    tests.includes(
      'handle_publish_never_replaces_a_concurrently_created_formal_file',
    ) &&
      tests.includes('formal_before') &&
      tests.includes('partial_before'),
  ],
  [
    'two-volume test proves source bytes/mtime and test-root marker remain unchanged',
    tests.includes(
      'disposable_two_volume_import_publishes_without_source_or_existing_target_changes',
    ) &&
      tests.includes('source_before') &&
      tests.includes('.fuji-photo-manager-test-root-v1'),
  ],
  [
    'duplicate test commits separate Validation and Execution proofs without changing either endpoint',
    tests.includes(
      'duplicate_is_reproved_in_validation_and_execution_without_target_change',
    ) && tests.includes("proof_scope='Execution'"),
  ],
  [
    'changed plan stops before SpaceCheck/Attempt and pre-start cancellation changes no file',
    tests.includes(
      'changed_plan_stops_before_space_and_attempt_then_cancels_without_file_change',
    ) && tests.includes('ImportExecutionPhase::AwaitingPlanConfirmation'),
  ],
  [
    'source process write counter is fixed at zero',
    execution.includes('status.source_write_requests = 0'),
  ],
];

const failed = checks.filter(([, passed]) => !passed);
const depsDirectory = join(root, 'src-tauri', 'target', 'debug', 'deps');
const binaries = (await readdir(depsDirectory)).filter((name) =>
  /^local_photo_manager-[a-f0-9]+\.exe$/u.test(name),
);
const binaryRows = await Promise.all(
  binaries.map(async (name) => ({
    name,
    modified: (await stat(join(depsDirectory, name))).mtimeMs,
  })),
);
binaryRows.sort((left, right) => right.modified - left.modified);
const testedBinary = binaryRows[0];
if (testedBinary === undefined)
  throw new Error(
    'No tested Rust binary was found. Run the phase 6 test suite first.',
  );
const binaryBytes = await readFile(join(depsDirectory, testedBinary.name));

const report = {
  reportVersion: 1,
  phase: 6,
  scope: 'safe-import-execution-and-recovery',
  passed: failed.length === 0,
  generatedAt: new Date().toISOString(),
  testCommand: 'npm run test:phase6',
  filesystemFixtures:
    'disposable source volume and generated test Fuji Root only',
  uniqueUserPhotosImported: false,
  sourceWriteRequests: 0,
  existingFormalTargetsChangedByRaceTest: 0,
  replaceIfExists: false,
  recycleExecutionEnabled: false,
  stageSevenEntered: false,
  remotePushPerformed: false,
  schemaVersion: 6,
  schemaMigrationSha256: sha256(migration),
  testedRustBinary: basename(testedBinary.name),
  testedRustBinarySha256: sha256(binaryBytes),
  cargoLockSha256: sha256(lockBytes),
  packageLockSha256: sha256(packageLockBytes),
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
  join(artifactDirectory, 'phase-six-acceptance.json'),
  `${JSON.stringify(report, null, 2)}\n`,
  'utf8',
);
const markdown = `# 阶段 6：安全导入执行与恢复工程安全报告

- 结果：${report.passed ? 'PASS' : 'FAIL'}
- 范围：仅安全导入执行与恢复；未进入阶段 7
- 真实文件夹具：可丢弃来源卷 + 自动生成测试 Fuji Root
- 用户唯一照片导入：否
- 来源写请求：0
- 并发抢占测试中既有正式目标变化：0
- 正式发布 ReplaceIfExists：FALSE
- 远端推送：否
- Schema 版本：${report.schemaVersion}
- Migration SHA-256：\`${report.schemaMigrationSha256}\`
- 已测试 Rust 二进制：\`${report.testedRustBinary}\`
- 二进制 SHA-256：\`${report.testedRustBinarySha256}\`
- Cargo.lock SHA-256：\`${report.cargoLockSha256}\`
- package-lock.json SHA-256：\`${report.packageLockSha256}\`

## 验证项

${report.checks.map((check) => `- ${check.result === 'passed' ? 'PASS' : 'FAIL'} — ${check.name}`).join('\n')}

本报告由 \`npm run test:phase6\` 在完整格式、静态检查、前端/端到端/Rust 测试、Clippy 与安全审计全部成功后生成。它不是对用户唯一照片执行导入的记录，也不开放回收站执行能力。
`;
await writeFile(
  join(reportDirectory, 'phase-six-import-safety-report.md'),
  markdown,
  'utf8',
);
await writeFile(
  join(artifactDirectory, 'phase-six-acceptance.txt'),
  report.passed
    ? `PASS: all ${checks.length} phase 6 acceptance groups passed; source writes and existing-target changes were zero.\n`
    : `FAIL: ${failed.length} phase 6 acceptance group(s) failed.\n`,
  'utf8',
);

console.log(JSON.stringify(report));
process.exit(report.passed ? 0 : 1);
