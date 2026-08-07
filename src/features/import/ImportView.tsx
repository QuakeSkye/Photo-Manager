import { useEffect, useRef, useState, type RefObject } from 'react';

import {
  cancelImportPreflight,
  cancelRemainingImport,
  chooseImportSourceFolder,
  getImportExecutionStatus,
  getImportPreflightStatus,
  getImportRecoverySummary,
  getImportSetup,
  retryImport,
  selectImportSource,
  startImport,
  startImportPreflight,
  type ImportDraftItem,
  type ImportExecutionStatus,
  type ImportPreflightDraft,
  type ImportPreflightStatus,
  type ImportRecoverySummary,
  type ImportSetupSnapshot,
} from '../../shared/commands/import';
import {
  chooseLibraryFolder,
  validateLibraryFolder,
  type LibraryCandidate,
  type PendingDeleteSummary,
} from '../../shared/commands/library';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';

interface ImportViewProps {
  readonly pendingDeleteSummary: PendingDeleteSummary;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onOpenLibrary: () => void;
  readonly onOpenPendingDelete: () => void;
  readonly onOpenSettings: () => void;
  readonly onBindTarget: (path: string) => Promise<void>;
}

export function ImportView({
  pendingDeleteSummary,
  settingsButtonRef,
  onOpenLibrary,
  onOpenPendingDelete,
  onOpenSettings,
  onBindTarget,
}: ImportViewProps) {
  const [setup, setSetup] = useState<ImportSetupSnapshot | null>(null);
  const [draft, setDraft] = useState<ImportPreflightDraft | null>(null);
  const [status, setStatus] = useState<ImportPreflightStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [preflightRunning, setPreflightRunning] = useState(false);
  const [execution, setExecution] = useState<ImportExecutionStatus | null>(
    null,
  );
  const [recovery, setRecovery] = useState<ImportRecoverySummary | null>(null);
  const [confirmImport, setConfirmImport] = useState(false);
  const [startingImport, setStartingImport] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [targetCandidate, setTargetCandidate] = useState<{
    path: string;
    candidate: LibraryCandidate;
  } | null>(null);
  const [bindingTarget, setBindingTarget] = useState(false);
  const sourceButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let active = true;
    void getImportSetup()
      .then((value) => {
        if (active) setSetup(value);
      })
      .catch((reason) => active && setError(readError(reason)))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    void getImportRecoverySummary()
      .then((value) => setRecovery(value.hasUnfinishedImport ? value : null))
      .catch((reason) => setError(readError(reason)));
  }, []);

  useEffect(() => {
    if (!preflightRunning) return;
    const timer = window.setInterval(() => {
      void getImportPreflightStatus()
        .then(setStatus)
        .catch((reason) => setError(readError(reason)));
    }, 250);
    return () => window.clearInterval(timer);
  }, [preflightRunning]);

  useEffect(() => {
    if (execution?.phase !== 'running' && execution?.phase !== 'validatingPlan')
      return;
    const timer = window.setInterval(() => {
      void getImportExecutionStatus()
        .then(setExecution)
        .catch((reason) => setError(readError(reason)));
    }, 250);
    return () => window.clearInterval(timer);
  }, [execution?.phase]);

  const chooseSource = async () => {
    setError(null);
    const path = await chooseImportSourceFolder();
    if (path === null) return;
    try {
      const next = await selectImportSource(path);
      setSetup(next);
      setDraft(null);
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const chooseTarget = async () => {
    setError(null);
    const path = await chooseLibraryFolder();
    if (path === null) return;
    try {
      const candidate = await validateLibraryFolder(path);
      setTargetCandidate({ path, candidate });
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const bindTarget = async () => {
    if (targetCandidate === null) return;
    setBindingTarget(true);
    setError(null);
    try {
      await onBindTarget(targetCandidate.path);
      setTargetCandidate(null);
    } catch (reason) {
      setError(readError(reason));
      setBindingTarget(false);
    }
  };

  const startPreflight = async () => {
    setError(null);
    setDraft(null);
    setStatus({
      phase: 'running',
      scannedFiles: 0,
      supportedFiles: 0,
      currentRelativePath: null,
      errorMessage: null,
    });
    setPreflightRunning(true);
    try {
      const next = await startImportPreflight();
      setDraft(next);
      setStatus((current) =>
        current === null ? current : { ...current, phase: 'completed' },
      );
    } catch (reason) {
      const message = readError(reason);
      if (!message.includes('已取消')) setError(message);
      try {
        setSetup(await getImportSetup());
      } catch {
        // Keep the actionable preflight error already shown.
      }
    } finally {
      setPreflightRunning(false);
    }
  };

  const cancelPreflight = async () => {
    setStatus((current) =>
      current === null ? current : { ...current, phase: 'cancelling' },
    );
    try {
      await cancelImportPreflight();
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const returnToSelection = async () => {
    setDraft(null);
    setStatus(null);
    try {
      setSetup(await getImportSetup());
    } catch (reason) {
      setError(readError(reason));
    }
    window.requestAnimationFrame(() => sourceButtonRef.current?.focus());
  };

  const beginImport = async () => {
    if (draft === null) return;
    setStartingImport(true);
    setError(null);
    try {
      const next = await startImport(draft.draftDigest);
      setExecution(next);
      setRecovery(null);
      setConfirmImport(false);
      if (next.updatedPlan !== null) setDraft(next.updatedPlan);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setStartingImport(false);
    }
  };

  const resumeImport = async () => {
    setError(null);
    try {
      const next = await retryImport();
      setExecution(next);
      setRecovery(null);
      if (next.updatedPlan !== null) setDraft(next.updatedPlan);
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const cancelRemaining = async () => {
    try {
      await cancelRemainingImport();
      setExecution(await getImportExecutionStatus());
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const reportVisible = execution !== null && terminalExecution(execution);
  const step = reportVisible
    ? 4
    : execution !== null || recovery !== null
      ? 3
      : draft !== null || preflightRunning
        ? 2
        : 1;
  const targetPreview = setup?.target?.displayPath ?? 'FujiRoot';

  return (
    <div className="screen import-screen">
      <TopBar
        context="导入照片"
        settingsButtonRef={settingsButtonRef}
        pendingDeleteSummary={pendingDeleteSummary}
        onOpenLibrary={onOpenLibrary}
        onOpenPendingDelete={onOpenPendingDelete}
        onOpenSettings={onOpenSettings}
        onBack={onOpenLibrary}
      />
      <main className="tool-workspace import-workspace">
        <aside className="tool-sidebar" aria-label="导入步骤">
          <p className="sidebar-title">导入步骤</p>
          {['选择位置', '预检与确认', '正在导入', '导入报告'].map(
            (label, index) => (
              <div
                key={label}
                className={
                  index + 1 === step ? 'scan-step is-active' : 'scan-step'
                }
                aria-current={index + 1 === step ? 'step' : undefined}
              >
                <span>{index + 1}</span>
                <div>
                  <strong>{label}</strong>
                  {index + 1 < step && <small>已完成</small>}
                </div>
              </div>
            ),
          )}
        </aside>

        <section className="tool-content import-content">
          {loading && <p className="empty-records">正在读取导入位置…</p>}
          {!loading && preflightRunning && status !== null && (
            <PreflightProgress status={status} setup={setup} />
          )}
          {!loading && !preflightRunning && draft === null && (
            <>
              <div className="content-heading">
                <p className="eyebrow">导入 · 第 1 步</p>
                <h1>选择导入位置</h1>
                <p>
                  分别核对内存卡子目录和当前 Fuji
                  图库；选择位置不会扫描或复制照片。
                </p>
              </div>
              <div className="continuous-panel import-location-panel">
                <div className="setting-row import-location-row">
                  <div className="setting-copy">
                    <span className="setting-label">内存卡来源文件夹</span>
                    {setup?.source !== null && setup?.source !== undefined ? (
                      <>
                        <strong>{setup.source.volumeName}</strong>
                        <span className="long-path">
                          {setup.source.displayPath}
                        </span>
                      </>
                    ) : (
                      <strong className="path-empty">
                        {setup?.sourceNeedsReselection
                          ? '请重新选择来源文件夹'
                          : '尚未选择来源文件夹'}
                      </strong>
                    )}
                    <span className="setting-label">
                      将递归扫描此文件夹及其子目录
                    </span>
                  </div>
                  <button
                    ref={sourceButtonRef}
                    type="button"
                    onClick={() => void chooseSource()}
                    disabled={
                      setup?.blockedReason !== null &&
                      setup?.blockedReason !== undefined
                    }
                  >
                    {setup?.source === null || setup?.source === undefined
                      ? '选择文件夹'
                      : '更改'}
                  </button>
                </div>
                <div className="setting-row import-location-row">
                  <div className="setting-copy">
                    <span className="setting-label">Fuji 图库根目录</span>
                    <strong className="long-path">
                      {setup?.target?.displayPath ?? '尚未选择 Fuji 图库'}
                    </strong>
                    <span className="setting-label">
                      首次选择后记住；更改会重新绑定当前 Fuji 图库
                    </span>
                  </div>
                  <button
                    type="button"
                    onClick={() => void chooseTarget()}
                    disabled={
                      setup?.blockedReason !== null &&
                      setup?.blockedReason !== undefined
                    }
                  >
                    {setup?.target === null || setup?.target === undefined
                      ? '选择文件夹'
                      : '更改'}
                  </button>
                </div>
                <div className="import-path-preview">
                  <span>新照片预计保存位置</span>
                  <code>JPG → {targetPreview}\2026\20260729\</code>
                  <code>RAF → {targetPreview}\2026\20260729\raw\</code>
                  <small>没有可靠拍摄日期的文件将进入 Unknown-Date。</small>
                </div>
              </div>
              {setup?.blockedReason !== null &&
                setup?.blockedReason !== undefined && (
                  <p className="inline-error" role="status">
                    {setup.blockedReason}
                  </p>
                )}
            </>
          )}
          {!loading &&
            !preflightRunning &&
            execution === null &&
            recovery === null &&
            draft !== null && <DraftResults draft={draft} />}
          {!loading &&
            !preflightRunning &&
            (execution !== null || recovery !== null) && (
              <ExecutionPanel execution={execution} recovery={recovery} />
            )}
          {error !== null && (
            <p className="inline-error import-error" role="alert">
              {error}
            </p>
          )}
        </section>

        <aside className="tool-summary">
          {execution !== null || recovery !== null ? (
            <ExecutionSummary execution={execution} recovery={recovery} />
          ) : draft === null ? (
            <div>
              <p className="section-label">只读预检</p>
              <strong>
                {preflightRunning
                  ? '正在检查全部受支持文件'
                  : '此步骤不会复制文件'}
              </strong>
              <p>来源卷保持只读；预检不会创建日期目录、partial 或正式照片。</p>
            </div>
          ) : (
            <div>
              <p className="section-label">本次导入计划</p>
              <strong>{draft.summary.newGroups} 组将新增</strong>
              <p>
                {draft.summary.newFiles} 个真实文件 ·{' '}
                {formatBytes(draft.summary.newBytes)}
                <br />
                {draft.summary.skippedFiles} 个已存在 ·{' '}
                {draft.summary.unreadableFiles} 个无法读取
              </p>
            </div>
          )}
          <div className="summary-spacer" />
          <div className="summary-actions">
            {execution?.phase === 'running' ? (
              <button type="button" onClick={() => void cancelRemaining()}>
                取消剩余导入
              </button>
            ) : execution?.phase === 'awaitingPlanConfirmation' ? (
              <button
                className="button-primary"
                type="button"
                onClick={() => setConfirmImport(true)}
              >
                重新确认并导入…
              </button>
            ) : recovery !== null ||
              (execution !== null && retryableExecution(execution)) ? (
              <button
                className="button-primary"
                type="button"
                onClick={() => void resumeImport()}
              >
                {execution?.phase === 'awaitingSpace'
                  ? '重新检查空间'
                  : '确认并继续恢复'}
              </button>
            ) : execution !== null ? (
              <button type="button" onClick={onOpenLibrary}>
                返回图库
              </button>
            ) : preflightRunning ? (
              <button type="button" onClick={() => void cancelPreflight()}>
                {status?.phase === 'cancelling' ? '正在取消…' : '取消预检'}
              </button>
            ) : draft === null ? (
              <>
                <button type="button" onClick={onOpenLibrary}>
                  取消
                </button>
                <button
                  className="button-primary"
                  type="button"
                  onClick={() => void startPreflight()}
                  disabled={!setup?.canStartPreflight}
                >
                  开始只读预检
                </button>
              </>
            ) : (
              <>
                <button type="button" onClick={() => void returnToSelection()}>
                  返回选择位置
                </button>
                <button
                  className="button-primary"
                  type="button"
                  onClick={() => setConfirmImport(true)}
                >
                  确认并导入…
                </button>
              </>
            )}
          </div>
        </aside>
      </main>
      <footer className="status-bar tool-status">
        <span>
          {execution !== null
            ? executionPhaseLabel(execution)
            : draft !== null
              ? '只读计划草案已生成'
              : '内存卡只读'}
        </span>
        <span>来源卷永久拒绝应用写入</span>
        <span>
          {execution?.phase === 'running'
            ? '正式文件仅 no-replace 发布'
            : preflightRunning
              ? '正在只读检查'
              : '安全门已启用'}
        </span>
      </footer>

      {targetCandidate !== null && (
        <Modal
          title={
            targetCandidate.candidate.reconnectsExistingLibrary
              ? '重新连接原图库'
              : '切换到新的 Fuji 图库'
          }
          confirmLabel={
            targetCandidate.candidate.reconnectsExistingLibrary
              ? '确认重新连接'
              : '确认切换'
          }
          busy={bindingTarget}
          onConfirm={() => void bindTarget()}
          onCancel={() => setTargetCandidate(null)}
        >
          <p className="long-path">{targetCandidate.candidate.displayPath}</p>
          <p>
            {targetCandidate.candidate.reconnectsExistingLibrary
              ? '卷身份和根目录身份匹配；原索引、浏览位置、标记与历史会保留。'
              : '旧图库状态会保留但不会迁移；新图库需要完成首次全量扫描。'}
          </p>
        </Modal>
      )}
      {confirmImport && draft !== null && (
        <Modal
          title="最终确认导入"
          confirmLabel="开始导入"
          busy={startingImport}
          onConfirm={() => void beginImport()}
          onCancel={() => setConfirmImport(false)}
        >
          <p>
            将新增 {draft.summary.newFiles} 个文件（
            {formatBytes(draft.summary.newBytes)}），跳过{' '}
            {draft.summary.skippedFiles} 个已完整证明的重复文件。
          </p>
          <p>
            点击“开始导入”后才会创建 TaskIntent、ImportJob 和首个 Confirmed
            Generation；执行前仍会全量验证计划并检查空间。
          </p>
          <p>来源卷保持零写入，正式目标使用 ReplaceIfExists=FALSE 发布。</p>
        </Modal>
      )}
    </div>
  );
}

function ExecutionPanel({
  execution,
  recovery,
}: {
  execution: ImportExecutionStatus | null;
  recovery: ImportRecoverySummary | null;
}) {
  if (recovery !== null && execution === null) {
    return (
      <div className="content-heading" aria-live="polite">
        <p className="eyebrow">导入 · 恢复</p>
        <h1>检测到未终结导入</h1>
        <p>{recovery.message}</p>
        <p>
          {recovery.completedFiles} 个已完成，{recovery.unresolvedFiles}{' '}
          个待协调；不会自动恢复写入。
        </p>
      </div>
    );
  }
  if (execution === null) return null;
  if (
    execution.phase === 'awaitingPlanConfirmation' &&
    execution.updatedPlan !== null
  ) {
    return (
      <>
        <p className="inline-error" role="status">
          磁盘现状改变了未完成计划。请检查更新后的名称、分类与新增字节，再次明确确认。
        </p>
        <DraftResults draft={execution.updatedPlan} />
      </>
    );
  }
  return (
    <div className="content-heading" aria-live="polite">
      <p className="eyebrow">
        导入 · 第 {terminalExecution(execution) ? '4' : '3'} 步
      </p>
      <h1>{executionPhaseLabel(execution)}</h1>
      {execution.message !== null && <p>{execution.message}</p>}
      {execution.currentFile !== null && (
        <p className="long-path">
          {execution.currentAction}：{execution.currentFile}
        </p>
      )}
      {execution.space !== null && (
        <p className="inline-error" role="status">
          还缺 {formatBytes(execution.space.missingBytes)}；本轮没有创建
          Attempt，也没有写文件。
        </p>
      )}
      <dl className="preflight-progress-metrics">
        <div>
          <dt>已完成</dt>
          <dd>{execution.completedFiles}</dd>
        </div>
        <div>
          <dt>已发布</dt>
          <dd>{execution.publishedFiles}</dd>
        </div>
        <div>
          <dt>已跳过</dt>
          <dd>{execution.skippedFiles}</dd>
        </div>
        <div>
          <dt>需处理</dt>
          <dd>{execution.failedFiles}</dd>
        </div>
      </dl>
      <p>来源写请求：{execution.sourceWriteRequests}（必须为 0）</p>
    </div>
  );
}

function ExecutionSummary({
  execution,
  recovery,
}: {
  execution: ImportExecutionStatus | null;
  recovery: ImportRecoverySummary | null;
}) {
  return (
    <div>
      <p className="section-label">导入执行</p>
      <strong>
        {execution !== null ? executionPhaseLabel(execution) : '等待显式恢复'}
      </strong>
      <p>
        {execution !== null
          ? `${execution.completedFiles} 个完成 · ${execution.notStartedFiles} 个未开始`
          : `${recovery?.unresolvedFiles ?? 0} 个待协调`}
      </p>
    </div>
  );
}

function terminalExecution(status: ImportExecutionStatus): boolean {
  return ['completed', 'completedWithErrors', 'cancelled'].includes(
    status.phase,
  );
}

function retryableExecution(status: ImportExecutionStatus): boolean {
  return [
    'awaitingPlanConfirmation',
    'awaitingSpace',
    'awaitingSource',
    'needsAttention',
    'completedWithErrors',
  ].includes(status.phase);
}

function executionPhaseLabel(status: ImportExecutionStatus): string {
  const labels: Record<ImportExecutionStatus['phase'], string> = {
    idle: '等待开始',
    validatingPlan: '正在全量验证计划',
    awaitingPlanConfirmation: '计划已变化，等待重新确认',
    awaitingSpace: '空间不足',
    running: '正在安全导入',
    awaitingSource: '等待原始内存卡',
    needsAttention: '需要人工处理',
    completed: '导入完成',
    completedWithErrors: '导入部分完成',
    cancelled: '导入已取消',
  };
  return labels[status.phase];
}

function PreflightProgress({
  status,
  setup,
}: {
  status: ImportPreflightStatus;
  setup: ImportSetupSnapshot | null;
}) {
  return (
    <div className="import-progress" aria-live="polite">
      <div className="content-heading">
        <p className="eyebrow">导入 · 第 2 步</p>
        <h1>正在只读预检</h1>
        <p>不会复制、移动或删除内存卡中的文件。</p>
      </div>
      <div className="scan-orbit" aria-hidden="true">
        <i />
        <i />
        <span>只读</span>
      </div>
      <dl className="preflight-progress-metrics">
        <div>
          <dt>已扫描文件</dt>
          <dd>{status.scannedFiles.toLocaleString('zh-CN')}</dd>
        </div>
        <div>
          <dt>识别照片文件</dt>
          <dd>{status.supportedFiles.toLocaleString('zh-CN')}</dd>
        </div>
      </dl>
      <p className="long-path">
        <strong>来源：</strong> {setup?.source?.displayPath}
      </p>
      <p className="long-path">
        <strong>目标：</strong> {setup?.target?.displayPath}
      </p>
      <p className="current-scan-path">
        当前：{status.currentRelativePath ?? '正在验证所选位置'}
      </p>
    </div>
  );
}

function DraftResults({ draft }: { draft: ImportPreflightDraft }) {
  const groups = groupItems(draft.items);
  return (
    <>
      <div className="content-heading">
        <p className="eyebrow">导入 · 第 2 步</p>
        <h1>预检与确认</h1>
        <p>
          这是尚未创建导入任务的“本次导入计划”。执行时仍须重新验证已存在项目。
        </p>
      </div>
      <div className="preflight-summary-grid">
        <SummaryMetric
          label="将新增"
          value={`${draft.summary.newFiles} 个文件`}
        />
        <SummaryMetric
          label="已存在，跳过"
          value={`${draft.summary.skippedFiles} 个文件`}
        />
        <SummaryMetric
          label="同名但内容不同"
          value={`${draft.summary.conflictGroups} 组`}
        />
        <SummaryMetric
          label="日期未知"
          value={`${draft.summary.unknownDateFiles} 个文件`}
        />
        <SummaryMetric
          label="无法读取"
          value={`${draft.summary.unreadableFiles} 个文件`}
          warning={draft.summary.unreadableFiles > 0}
        />
      </div>
      {draft.summary.unreadableFiles > 0 && (
        <p className="inline-error" role="status">
          无法读取的文件不会被混入“跳过”；其余安全项目仍保留在本次计划中。
        </p>
      )}
      <div className="import-plan-list">
        {Array.from(groups.entries()).map(([group, items]) => (
          <details key={group} className="import-plan-group" open>
            <summary>
              <strong>{group}</strong>
              <span>{groupSummary(items)}</span>
            </summary>
            {items.map((item) => (
              <div className="import-plan-item" key={item.sourceRelativePath}>
                <span
                  className={`plan-badge disposition-${item.disposition.toLowerCase()}`}
                >
                  {dispositionLabel(item)}
                </span>
                <div>
                  <strong>{item.fileName}</strong>
                  <span>{item.sourceRelativePath}</span>
                  {item.targetRelativePath !== null && (
                    <code>→ {item.targetRelativePath}</code>
                  )}
                  {item.warning !== null && <em>{item.warning}</em>}
                </div>
                <span>{formatBytes(item.sizeBytes)}</span>
              </div>
            ))}
          </details>
        ))}
      </div>
      <p className="preflight-proof-note">
        “已存在”已由当前双端内容和路径绑定证明；不是由历史记录、文件名或哈希单独推断。
      </p>
    </>
  );
}

function SummaryMetric({
  label,
  value,
  warning = false,
}: {
  label: string;
  value: string;
  warning?: boolean;
}) {
  return (
    <div className={warning ? 'is-warning' : undefined}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function groupItems(items: readonly ImportDraftItem[]) {
  const groups = new Map<string, ImportDraftItem[]>();
  for (const item of items) {
    const current = groups.get(item.logicalGroup) ?? [];
    current.push(item);
    groups.set(item.logicalGroup, current);
  }
  return groups;
}

function groupSummary(items: readonly ImportDraftItem[]): string {
  if (items.some((item) => item.disposition === 'Unreadable'))
    return '无法读取';
  if (items.some((item) => item.disposition === 'Conflict'))
    return '使用安全后缀';
  if (items.every((item) => item.disposition === 'Skip')) return '已存在';
  return `${items.filter((item) => item.disposition === 'New').length} 个将新增`;
}

function dispositionLabel(item: ImportDraftItem): string {
  if (item.disposition === 'Skip') return '跳过';
  if (item.disposition === 'Conflict') return '改后缀';
  if (item.disposition === 'Unreadable') return '读取失败';
  if (item.dateUnknown) return '日期未知';
  return '新增';
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const index = Math.min(
    units.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024)),
  );
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function readError(reason: unknown): string {
  if (typeof reason === 'string') return reason;
  if (reason && typeof reason === 'object') {
    if ('userMessage' in reason && typeof reason.userMessage === 'string')
      return reason.userMessage;
    if ('message' in reason && typeof reason.message === 'string')
      return reason.message;
  }
  return '导入预检暂时不可用，请核对位置后重试。';
}
