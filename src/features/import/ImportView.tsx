import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type RefObject,
} from 'react';

import {
  cancelImportPreflight,
  getImportRecoveryItems,
  excludeImportRecoveryItems,
  reselectImportSource,
  type ImportRecoveryItem,
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
import {
  DraftResults,
  ExecutionPanel,
  ImportLocations,
  PreflightProgress,
  executionPhaseLabel,
  formatBytes,
} from './ImportPanels';

interface ImportViewProps {
  readonly pendingDeleteSummary: PendingDeleteSummary;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onOpenLibrary: () => void;
  readonly onOpenPendingDelete: () => void;
  readonly onOpenSettings: () => void;
  readonly onBindTarget: (path: string) => Promise<void>;
  readonly writeEnabled?: boolean;
  readonly safetyMessage?: string;
}

export function ImportView({
  pendingDeleteSummary,
  settingsButtonRef,
  onOpenLibrary,
  onOpenPendingDelete,
  onOpenSettings,
  onBindTarget,
  writeEnabled = true,
  safetyMessage,
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
  const [recoveryItems, setRecoveryItems] = useState<ImportRecoveryItem[]>([]);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [confirmIsolation, setConfirmIsolation] = useState(false);
  const requestVersion = useRef(0);
  const [taskLoading, setTaskLoading] = useState(true);
  const [actionPending, setActionPending] = useState(false);
  const [stopRequested, setStopRequested] = useState(false);
  const actionLock = useRef(false);
  const preflightLock = useRef(false);
  const applyExecution = useCallback((value: ImportExecutionStatus) => {
    setExecution(value.phase === 'idle' ? null : value);
    if (value.phase !== 'running' && value.phase !== 'validatingPlan')
      setStopRequested(false);
    setRecovery(null);
    if (value.updatedPlan !== null) setDraft(value.updatedPlan);
  }, []);
  const [startingImport, setStartingImport] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [targetCandidate, setTargetCandidate] = useState<{
    path: string;
    candidate: LibraryCandidate;
  } | null>(null);
  const [bindingTarget, setBindingTarget] = useState(false);
  const sourceButtonRef = useRef<HTMLButtonElement>(null);
  const runAction = async (operation: () => Promise<void>) => {
    if (actionLock.current) return;
    actionLock.current = true;
    setActionPending(true);
    setError(null);
    try {
      await operation();
    } catch (reason) {
      setError(readError(reason));
    } finally {
      actionLock.current = false;
      setActionPending(false);
    }
  };

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
    setConfirmImport(false);
  }, [draft?.draftDigest]);

  useEffect(() => {
    let active = true;
    const version = ++requestVersion.current;
    const live =
      '__TAURI_INTERNALS__' in window
        ? getImportExecutionStatus()
        : Promise.resolve(null);
    void Promise.all([live, getImportRecoverySummary()])
      .then(([value, summary]) => {
        if (!active || version !== requestVersion.current) return;
        if (value !== null && value.phase !== 'idle') applyExecution(value);
        else setRecovery(summary.hasUnfinishedImport ? summary : null);
      })
      .catch((reason) => active && setError(readError(reason)))
      .finally(() => {
        if (active) setTaskLoading(false);
      });
    return () => {
      active = false;
      requestVersion.current++;
    };
  }, [applyExecution]);

  useEffect(() => {
    let active = true;
    void getImportRecoveryItems()
      .then((items) => active && setRecoveryItems(items))
      .catch((reason) => active && setError(readError(reason)));
    return () => {
      active = false;
    };
  }, [execution?.phase]);

  useEffect(() => {
    if (!preflightRunning) return;
    let active = true;
    let inFlight = false;
    const timer = window.setInterval(() => {
      if (inFlight) return;
      inFlight = true;
      void getImportPreflightStatus()
        .then((value) => {
          if (active)
            setStatus((current) =>
              current?.phase === 'cancelling' && value.phase === 'running'
                ? { ...value, phase: 'cancelling' }
                : value,
            );
        })
        .catch((reason) => {
          if (active) setError(readError(reason));
        })
        .finally(() => {
          inFlight = false;
        });
    }, 250);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [preflightRunning]);

  useEffect(() => {
    if (execution?.phase !== 'running' && execution?.phase !== 'validatingPlan')
      return;
    let active = true;
    let inFlight = false;
    const timer = window.setInterval(() => {
      if (inFlight || actionLock.current) return;
      inFlight = true;
      const version = ++requestVersion.current;
      void getImportExecutionStatus()
        .then((value) => {
          if (active && version === requestVersion.current)
            applyExecution(value);
        })
        .catch((reason) => active && setError(readError(reason)))
        .finally(() => {
          inFlight = false;
        });
    }, 250);
    return () => {
      active = false;
      requestVersion.current++;
      window.clearInterval(timer);
    };
  }, [execution?.phase, applyExecution]);

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
    setConfirmImport(false);
    setDraft(null);
    setStatus(null);
    setError(null);
    try {
      await onBindTarget(targetCandidate.path);
      setTargetCandidate(null);
      setSetup(await getImportSetup());
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBindingTarget(false);
    }
  };

  const startPreflight = async () => {
    if (
      preflightLock.current ||
      actionLock.current ||
      !writeEnabled ||
      !setup?.canStartPreflight
    )
      return;
    preflightLock.current = true;
    setConfirmImport(false);
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
      preflightLock.current = false;
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
    setConfirmImport(false);
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
    if (draft === null || !writeEnabled) return;
    requestVersion.current++;
    setStartingImport(true);
    setError(null);
    try {
      const next = await startImport(draft.draftDigest);
      requestVersion.current++;
      applyExecution(next);
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
    requestVersion.current++;
    setError(null);
    try {
      const next = await retryImport();
      requestVersion.current++;
      applyExecution(next);
      setRecovery(null);
      if (next.updatedPlan !== null) setDraft(next.updatedPlan);
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const cancelRemaining = async () => {
    setStopRequested(true);
    requestVersion.current++;
    try {
      await cancelRemainingImport();
      requestVersion.current++;
      applyExecution(await getImportExecutionStatus());
      setConfirmCancel(false);
    } catch (reason) {
      setStopRequested(false);
      setError(readError(reason));
    }
  };

  const reconnectSource = async () => {
    const path = await chooseImportSourceFolder();
    if (path === null) return;
    try {
      requestVersion.current++;
      setSetup(await reselectImportSource(path));
      const latest = await getImportExecutionStatus();
      if (latest.phase === 'idle') {
        setExecution(null);
        setRecovery(null);
        setDraft(null);
        setError(
          '原来源身份不足以直接恢复。旧任务已结束，照片和临时文件保留，请重新检查导入内容并确认。',
        );
      } else applyExecution(latest);
    } catch (reason) {
      setError(readError(reason));
    }
  };
  const isolateAndContinue = async () => {
    try {
      await excludeImportRecoveryItems(
        recoveryItems
          .filter((item) => !item.excluded && item.canExclude)
          .map((item) => item.ownershipId),
      );
      setConfirmIsolation(false);
      setRecoveryItems(await getImportRecoveryItems());
      await resumeImport();
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const reportVisible = execution !== null && terminalExecution(execution);
  const selectionVisible =
    !execution && !recovery && !draft && !preflightRunning;
  const step =
    execution?.phase === 'awaitingPlanConfirmation'
      ? 2
      : reportVisible
        ? 4
        : execution || recovery
          ? 3
          : draft || preflightRunning
            ? 2
            : 1;
  const busy =
    loading || taskLoading || actionPending || bindingTarget || startingImport;
  const activeExecution =
    execution?.phase === 'running' || execution?.phase === 'validatingPlan';
  const canCancel =
    recovery !== null ||
    (execution !== null &&
      !['idle', 'completed', 'cancelled'].includes(execution.phase));
  const canReset =
    execution?.phase === 'completed' || execution?.phase === 'cancelled';
  const sourcePath = draft?.sourcePath ?? setup?.source?.displayPath ?? null;
  const targetPath = draft?.targetPath ?? setup?.target?.displayPath ?? null;
  const newImport = async () => {
    const next = await getImportSetup();
    requestVersion.current++;
    setSetup(next);
    setExecution(null);
    setRecovery(null);
    setDraft(null);
    setStatus(null);
    setRecoveryItems([]);
    setStopRequested(false);
    setConfirmImport(false);
    window.requestAnimationFrame(() => sourceButtonRef.current?.focus());
  };

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
      <main className="import-workspace">
        <nav className="import-stepper" aria-label="导入步骤">
          {['选择来源与目标', '检查导入计划', '执行导入', '查看结果'].map(
            (label, index) => (
              <div
                key={label}
                aria-current={index + 1 === step ? 'step' : undefined}
                className={index + 1 === step ? 'is-current' : ''}
              >
                <span>{index + 1}</span>
                <strong>{label}</strong>
              </div>
            ),
          )}
        </nav>
        <section className="import-content" aria-label="导入内容">
          {loading || taskLoading ? (
            <p role="status">正在读取导入位置与任务…</p>
          ) : (
            <>
              {!writeEnabled && (
                <p className="import-notice" role="status">
                  暂时无法导入：{safetyMessage ?? '请检查图库与磁盘状态。'}
                </p>
              )}
              {selectionVisible ? (
                <>
                  <div className="import-heading">
                    <h1>选择导入位置</h1>
                    <p>从存储卡或其他文件夹复制照片到图库。</p>
                  </div>
                  <div className="import-location-cards">
                    <section className="import-location-card">
                      <span className="section-label">来源文件夹</span>
                      <h2>从哪里导入</h2>
                      <p className="long-path">
                        {setup?.source?.displayPath ??
                          (setup?.sourceNeedsReselection
                            ? '请重新选择来源文件夹'
                            : '尚未选择来源文件夹')}
                      </p>
                      {setup?.source && (
                        <p className="import-muted">
                          {setup.source.volumeName} · 包含子文件夹中的照片
                        </p>
                      )}
                      <button
                        ref={sourceButtonRef}
                        type="button"
                        disabled={busy || !writeEnabled}
                        onClick={() => void runAction(chooseSource)}
                      >
                        {setup?.source ? '更改来源文件夹' : '选择来源文件夹'}
                      </button>
                    </section>
                    <section className="import-location-card">
                      <span className="section-label">目标图库</span>
                      <h2>保存到哪里</h2>
                      <p className="long-path">
                        {setup?.target?.displayPath ?? '尚未选择图库'}
                      </p>
                      <p className="import-muted">
                        新文件按拍摄日期保存，现有照片保持原位。
                      </p>
                      <button
                        type="button"
                        disabled={busy || !writeEnabled}
                        onClick={() => void runAction(chooseTarget)}
                      >
                        {setup?.target ? '更改目标图库' : '选择目标图库'}
                      </button>
                    </section>
                  </div>
                  <p className="import-source-note">
                    本应用会将来源所在磁盘登记为只读来源，此后不能把它用于本应用的图库或缓存。
                  </p>
                  <details className="import-details">
                    <summary>查看保存规则与位置要求</summary>
                    <p>
                      来源和目标需要位于不同磁盘卷；此只读约束仅适用于本应用，不改变
                      Windows 磁盘权限。
                    </p>
                    <p>
                      JPG/JPEG 保存到 年份／拍摄日期，RAF 保存到对应日期的 raw
                      子文件夹。无法取得可靠拍摄日期时保存到 Unknown-Date。
                    </p>
                    <p>
                      相同内容经核验后跳过；同名不同内容使用计划中的新名称，不覆盖已有文件。
                    </p>
                  </details>
                  {setup?.blockedReason && (
                    <p className="import-notice" role="status">
                      {setup.blockedReason}
                    </p>
                  )}
                </>
              ) : (
                <ImportLocations source={sourcePath} target={targetPath} />
              )}
              {preflightRunning && status && (
                <PreflightProgress status={status} />
              )}
              {!preflightRunning && !execution && !recovery && draft && (
                <DraftResults key={draft.draftDigest} draft={draft} />
              )}
              {!preflightRunning && (execution || recovery) && (
                <ExecutionPanel
                  execution={execution}
                  recovery={recovery}
                  stopping={stopRequested}
                />
              )}
              {reportVisible && draft && (
                <>
                  {draft.summary.unreadableFiles > 0 && (
                    <p className="import-notice" role="status">
                      检查时有 {draft.summary.unreadableFiles}{' '}
                      个文件无法读取，未纳入新增导入。可在下方计划中查看。
                    </p>
                  )}
                  <details className="import-details">
                    <summary>查看本次导入计划</summary>
                    <p>以下是执行前的计划，实际完成数量以上方结果为准。</p>
                    <DraftResults key={draft.draftDigest} draft={draft} />
                  </details>
                </>
              )}
              {recoveryItems.length > 0 && (
                <section
                  className="import-recovery-files"
                  aria-label="保留文件与异常项"
                >
                  <h2>保留文件与异常项</h2>
                  <p>
                    以下文件未自动删除。请核对位置后处理；显示的大小是已记录值。
                  </p>
                  {recoveryItems.map((item) => (
                    <div key={item.ownershipId}>
                      <strong className="long-path">{item.sourcePath}</strong>
                      <p>
                        {item.excluded
                          ? '已排除，将保留'
                          : item.state === 'Owned'
                            ? '临时文件保留'
                            : '结果待核对'}{' '}
                        ·{' '}
                        {item.observedBytes === null
                          ? '大小未知'
                          : formatBytes(item.observedBytes)}
                      </p>
                      <p className="long-path">保留位置：{item.retainedPath}</p>
                      {item.targetPath && (
                        <p className="long-path">
                          原计划目标：{item.targetPath}
                        </p>
                      )}
                    </div>
                  ))}
                </section>
              )}
            </>
          )}
          {error && (
            <p className="inline-error import-error" role="alert">
              {error}
            </p>
          )}
        </section>
        <footer className="import-action-bar">
          <div className="import-action-summary" aria-live="polite">
            <strong>
              {stopRequested && activeExecution
                ? '正在停止导入…'
                : execution
                  ? executionPhaseLabel(execution)
                  : recovery
                    ? '等待处理未完成任务'
                    : preflightRunning
                      ? '正在检查导入内容'
                      : draft
                        ? `完整计划：新增 ${draft.summary.newFiles} 个文件 · ${formatBytes(draft.summary.newBytes)}`
                        : '选好位置后，先检查导入内容'}
            </strong>
            <span>
              {draft && !execution && !preflightRunning
                ? `已有相同 ${draft.summary.skippedFiles} 个 · 无法读取 ${draft.summary.unreadableFiles} 个`
                : execution
                  ? `新增到图库 ${execution.publishedFiles} 个 · 未开始 ${execution.notStartedFiles} 个 · 需处理 ${execution.failedFiles} 个`
                  : '来源照片不会被移动或删除'}
            </span>
          </div>
          <div className="import-actions">
            {preflightRunning ? (
              <button
                type="button"
                disabled={actionPending || status?.phase === 'cancelling'}
                onClick={() => void runAction(cancelPreflight)}
              >
                {status?.phase === 'cancelling' ? '正在停止检查…' : '取消检查'}
              </button>
            ) : (
              <>
                {canCancel && (
                  <button
                    type="button"
                    disabled={busy || stopRequested}
                    onClick={() => setConfirmCancel(true)}
                  >
                    {stopRequested
                      ? '正在停止…'
                      : activeExecution
                        ? '停止导入…'
                        : '取消剩余导入'}
                  </button>
                )}
                {(recovery ||
                  execution?.phase === 'awaitingSource' ||
                  execution?.phase === 'needsAttention') && (
                  <button
                    type="button"
                    disabled={busy || !writeEnabled}
                    onClick={() => void runAction(reconnectSource)}
                  >
                    重新选择原来源
                  </button>
                )}
                {(recovery ||
                  execution?.phase === 'needsAttention' ||
                  execution?.phase === 'completedWithErrors') &&
                  recoveryItems.some(
                    (item) => !item.excluded && item.canExclude,
                  ) && (
                    <button
                      type="button"
                      disabled={busy || !writeEnabled}
                      onClick={() => setConfirmIsolation(true)}
                    >
                      保留异常项并继续…
                    </button>
                  )}
                {execution?.phase === 'awaitingPlanConfirmation' ? (
                  <button
                    type="button"
                    className="button-primary"
                    disabled={busy || !writeEnabled || !execution.updatedPlan}
                    onClick={() => setConfirmImport(true)}
                  >
                    重新确认并导入…
                  </button>
                ) : !activeExecution &&
                  (recovery || (execution && retryableExecution(execution))) ? (
                  <button
                    type="button"
                    className="button-primary"
                    disabled={busy || !writeEnabled}
                    onClick={() => void runAction(resumeImport)}
                  >
                    {execution?.phase === 'awaitingSpace'
                      ? '重新检查空间'
                      : '检查并继续'}
                  </button>
                ) : !execution && !recovery && draft ? (
                  <>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void runAction(returnToSelection)}
                    >
                      更改来源或目标
                    </button>
                    <button
                      type="button"
                      className="button-primary"
                      disabled={busy || !writeEnabled}
                      onClick={() => setConfirmImport(true)}
                    >
                      确认导入…
                    </button>
                  </>
                ) : selectionVisible ? (
                  <button
                    type="button"
                    className="button-primary"
                    disabled={
                      busy || !writeEnabled || !setup?.canStartPreflight
                    }
                    onClick={() => void startPreflight()}
                  >
                    检查导入内容
                  </button>
                ) : null}
                {canReset && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void runAction(newImport)}
                  >
                    导入其他照片
                  </button>
                )}
                {reportVisible && (
                  <button
                    type="button"
                    className={canReset ? 'button-primary' : undefined}
                    disabled={busy}
                    onClick={onOpenLibrary}
                  >
                    返回图库
                  </button>
                )}
              </>
            )}
          </div>
        </footer>
      </main>
      <footer className="status-bar import-status">
        <span>
          {step === 1
            ? '选择位置'
            : step === 2
              ? '核对导入内容'
              : step === 3
                ? '处理导入任务'
                : '查看导入结果'}
        </span>
        <span>JPG / JPEG / RAF</span>
        <span />
      </footer>
      {confirmCancel && (
        <Modal
          title={activeExecution ? '停止导入' : '取消剩余导入'}
          confirmLabel="保留文件并取消"
          busy={actionPending}
          onConfirm={() => void runAction(cancelRemaining)}
          onCancel={() => setConfirmCancel(false)}
        >
          <p>
            已导入照片和临时文件均保留。等待当前操作结束后停止，不会继续处理剩余文件。
          </p>
        </Modal>
      )}
      {confirmIsolation && (
        <Modal
          title="保留异常项并继续"
          className="import-recovery-modal"
          confirmLabel="保留异常项并继续"
          busy={actionPending}
          confirmDisabled={!writeEnabled}
          onConfirm={() => void runAction(isolateAndContinue)}
          onCancel={() => setConfirmIsolation(false)}
        >
          <p>
            下列项目将保留并从后续导入中排除。其余文件会重新检查；计划变化时仍需再次确认。
          </p>
          {recoveryItems
            .filter((item) => !item.excluded && item.canExclude)
            .map((item) => (
              <p className="long-path" key={item.ownershipId}>
                {item.sourcePath} → {item.retainedPath}
              </p>
            ))}
        </Modal>
      )}
      {targetCandidate && (
        <Modal
          title={
            targetCandidate.candidate.reconnectsExistingLibrary
              ? '重新连接原图库'
              : '切换到新的图库'
          }
          confirmLabel={
            targetCandidate.candidate.reconnectsExistingLibrary
              ? '确认重新连接'
              : '确认切换'
          }
          busy={bindingTarget || actionPending}
          confirmDisabled={!writeEnabled}
          onConfirm={() => void runAction(bindTarget)}
          onCancel={() => setTargetCandidate(null)}
        >
          <p className="long-path">{targetCandidate.candidate.displayPath}</p>
          <p>
            {targetCandidate.candidate.reconnectsExistingLibrary
              ? '已识别为原图库，保留原索引、浏览位置和标记。'
              : '旧图库状态保留，新图库需要完成首次扫描。不会移动现有照片。'}
          </p>
        </Modal>
      )}
      {confirmImport && draft && (
        <Modal
          title="确认导入"
          confirmLabel="开始导入"
          busy={startingImport || actionPending}
          confirmDisabled={!writeEnabled}
          onConfirm={() => void runAction(beginImport)}
          onCancel={() => setConfirmImport(false)}
        >
          <ImportLocations
            source={draft.sourcePath}
            target={draft.targetPath}
          />
          <p>
            将按完整计划新增{' '}
            <strong>
              {draft.summary.newFiles} 个文件（
              {formatBytes(draft.summary.newBytes)}）
            </strong>
            ，跳过 {draft.summary.skippedFiles} 个已有相同文件。
          </p>
          {draft.summary.conflictGroups > 0 && (
            <p>
              {draft.summary.conflictGroups}{' '}
              组存在同名不同内容的文件，将使用计划中的新名称导入。
            </p>
          )}
          {draft.summary.unreadableFiles > 0 && (
            <p>
              {draft.summary.unreadableFiles} 个文件无法读取，本次不会导入。
            </p>
          )}
          <p>确认后开始导入。来源或目标发生变化时，会要求重新检查计划。</p>
        </Modal>
      )}
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
    'awaitingSpace',
    'awaitingSource',
    'needsAttention',
    'completedWithErrors',
  ].includes(status.phase);
}
function readError(reason: unknown): string {
  if (typeof reason === 'string') return reason;
  if (reason && typeof reason === 'object') {
    if ('userMessage' in reason && typeof reason.userMessage === 'string')
      return reason.userMessage;
    if ('message' in reason && typeof reason.message === 'string')
      return reason.message;
  }
  return '暂时无法完成导入操作，请检查位置后重试。';
}
