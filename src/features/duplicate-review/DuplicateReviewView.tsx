import { useEffect, useMemo, useState, type RefObject } from 'react';

import {
  addDuplicateSubsetToPendingDelete,
  cancelDuplicateReviewScan,
  getDuplicateReviewResults,
  openDuplicateAssetLocation,
  startDuplicateReviewScan,
  type DuplicateMemberResult,
  type DuplicateReviewResults,
  type DuplicateReviewStatus,
  type DuplicateSetResult,
} from '../../shared/commands/duplicate-review';
import type { PendingDeleteSummary } from '../../shared/commands/library';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';
import {
  applyBulkSelection,
  buildDuplicateSelections,
  getBulkSelectionState,
  selectionBytes,
  toggleMemberSelection,
} from './selection';

interface DuplicateReviewViewProps {
  readonly status: DuplicateReviewStatus;
  readonly pendingDeleteSummary: PendingDeleteSummary;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onStatusChange: (status: DuplicateReviewStatus) => void;
  readonly onOpenLibrary: () => void;
  readonly onOpenPendingDelete: () => void;
  readonly onOpenSettings: () => void;
  readonly onPendingAdded: () => Promise<void>;
}

export function DuplicateReviewView({
  status,
  pendingDeleteSummary,
  settingsButtonRef,
  onStatusChange,
  onOpenLibrary,
  onOpenPendingDelete,
  onOpenSettings,
  onPendingAdded,
}: DuplicateReviewViewProps) {
  const [results, setResults] = useState<DuplicateReviewResults | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [selectionNotice, setSelectionNotice] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (
      status.state !== 'completedWithDuplicates' &&
      status.state !== 'completedEmpty' &&
      status.state !== 'expired'
    ) {
      if (status.state === 'running') setResults(null);
      return;
    }
    void getDuplicateReviewResults()
      .then((next) => {
        setResults(next);
        setSelected(new Set());
      })
      .catch((reason) => setError(readError(reason)));
  }, [status.resultVersionId, status.runId, status.state]);

  const sets = useMemo(
    () =>
      (results?.families ?? []).flatMap((family) =>
        [family.jpg, family.raf].filter(
          (set): set is DuplicateSetResult => set !== null,
        ),
      ),
    [results],
  );
  const members = useMemo(
    () =>
      new Map(
        sets.flatMap((set) =>
          set.members.map((member) => [member.assetId, member]),
        ),
      ),
    [sets],
  );
  const selections = buildDuplicateSelections(sets, selected);
  const selectedBytes = selectionBytes(selections, members);
  const bulkSelection = getBulkSelectionState(sets, selected);

  const begin = async () => {
    setBusy(true);
    setError(null);
    setSelected(new Set());
    setSelectionNotice(null);
    try {
      onStatusChange(await startDuplicateReviewScan());
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const submit = async () => {
    if (status.runId === null || selections.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      await addDuplicateSubsetToPendingDelete(status.runId, selections);
      setSelected(new Set());
      setSelectionNotice(null);
      setConfirming(false);
      await onPendingAdded();
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const toggleMember = (assetId: string) => {
    const result = toggleMemberSelection(sets, selected, assetId);
    setSelected(result.selected);
    setError(result.error);
    if (result.error === null) setSelectionNotice(null);
  };

  const toggleBulkSelection = () => {
    if (bulkSelection.mode === 'disabled') return;
    setSelected(applyBulkSelection(bulkSelection, selected));
    setError(null);
    setSelectionNotice(
      bulkSelection.mode === 'clear'
        ? `已取消 ${bulkSelection.changeCount} 个文件的勾选。`
        : `已勾选 ${bulkSelection.changeCount} 个非建议保留文件。`,
    );
  };

  return (
    <div className="screen duplicate-review-screen">
      <TopBar
        context="重复复核"
        settingsButtonRef={settingsButtonRef}
        pendingDeleteSummary={pendingDeleteSummary}
        onOpenLibrary={onOpenLibrary}
        onOpenPendingDelete={onOpenPendingDelete}
        onOpenSettings={onOpenSettings}
        onBack={onOpenLibrary}
      />
      <main className="duplicate-review-main">
        <header className="duplicate-review-heading">
          <div>
            <p className="section-label">只读检测 · 用户手动启动</p>
            <h1>检查图库中的重复文件</h1>
            <p>先按角色与大小筛选，再以完整 SHA-256 和逐字节比较证明相同。</p>
          </div>
          {status.state === 'running' ? (
            <button
              type="button"
              onClick={() => void cancelDuplicateReviewScan()}
            >
              取消检测
            </button>
          ) : (
            <button type="button" disabled={busy} onClick={() => void begin()}>
              {busy ? '正在启动…' : '开始检测'}
            </button>
          )}
        </header>
        {error !== null && (
          <div className="inline-error" role="alert">
            {error}
          </div>
        )}
        {status.lastReceipt !== null && (
          <section
            className={
              status.lastReceipt.state === 'applied'
                ? 'duplicate-receipt'
                : 'duplicate-receipt is-warning'
            }
            role="status"
          >
            <strong>重复文件已处理</strong>
            <span>
              已移入 Windows 回收站 {status.lastReceipt.recycledFiles} 个文件 ·{' '}
              {formatBytes(status.lastReceipt.recycledBytes)}
            </span>
            <span>{status.lastReceipt.message}</span>
          </section>
        )}
        {status.state === 'running' && <ScanProgress status={status} />}
        {status.state === 'expired' && (
          <div className="problem-panel" role="status">
            <strong>重复检查过期</strong>
            <span>
              {status.expiredReason ?? '图库内容或绑定已改变，请重新检测。'}
            </span>
          </div>
        )}
        {status.state === 'completedEmpty' && (
          <div className="duplicate-empty">
            <strong>
              {status.lastReceipt?.state === 'applied'
                ? '本次发现的重复文件已处理完'
                : '上次检测未发现重复'}
            </strong>
            {status.lastReceipt?.state === 'applied' && (
              <div className="duplicate-empty-actions">
                <button type="button" onClick={onOpenLibrary}>
                  返回图库
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void begin()}
                >
                  再次检查重复
                </button>
              </div>
            )}
          </div>
        )}
        {results !== null &&
          !results.expired &&
          results.families.length > 0 && (
            <>
              <Summary results={results} />
              <div className="duplicate-bulk-toolbar">
                <span role="status">{selectionNotice}</span>
                <button
                  className="duplicate-bulk-button"
                  type="button"
                  aria-pressed={bulkSelection.pressed}
                  disabled={bulkSelection.mode === 'disabled'}
                  onClick={toggleBulkSelection}
                >
                  {bulkSelection.label}
                </button>
              </div>
              <div className="duplicate-family-list">
                {results.families.map((family) => (
                  <article
                    className="duplicate-family-card"
                    key={family.familyId}
                  >
                    <h2>
                      {family.jpg !== null && setTitle(family.jpg)}
                      {family.jpg !== null && family.raf !== null ? ' · ' : ''}
                      {family.raf !== null && setTitle(family.raf)}
                    </h2>
                    <p>
                      状态：
                      {family.jpg !== null && family.raf !== null
                        ? 'JPG 与 RAF 都有重复'
                        : family.jpg !== null
                          ? '仅 JPG 重复'
                          : '仅 RAF 重复'}
                    </p>
                    {[family.jpg, family.raf]
                      .filter((set): set is DuplicateSetResult => set !== null)
                      .map((set) => (
                        <DuplicateSet
                          key={set.setId}
                          set={set}
                          selected={selected}
                          onToggle={toggleMember}
                        />
                      ))}
                  </article>
                ))}
              </div>
            </>
          )}
      </main>
      {results !== null && !results.expired && results.families.length > 0 && (
        <footer className="duplicate-selection-bar" aria-label="待删除选择摘要">
          <div>
            <strong>
              {selections.length === 0
                ? '尚未选择文件'
                : `已选择 ${selections.length} 个文件 · ${formatBytes(selectedBytes)}`}
            </strong>
            <span>仅加入待删除列表，不会立即移动文件</span>
          </div>
          <button
            type="button"
            disabled={selections.length === 0 || busy}
            onClick={() => setConfirming(true)}
          >
            加入待删除（{selections.length}）…
          </button>
        </footer>
      )}
      {confirming && (
        <Modal
          title="确认加入待删除"
          confirmLabel="加入待删除"
          busy={busy}
          onConfirm={() => void submit()}
          onCancel={() => setConfirming(false)}
        >
          <p>此操作只建立复核标记，不会移动或修改文件。</p>
          <ul className="duplicate-confirm-list">
            {selections.map((selection) => {
              const deleted = members.get(selection.deletedAssetId);
              const kept = members.get(selection.retainedAssetId);
              return (
                <li key={selection.deletedAssetId}>
                  <strong>{deleted?.role}</strong>
                  <span>将回收：{deleted?.fullPath}</span>
                  <span>保留：{kept?.fullPath}</span>
                </li>
              );
            })}
          </ul>
          <p>
            {selections.length} 个文件，共 {formatBytes(selectedBytes)}
          </p>
        </Modal>
      )}
    </div>
  );
}

function ScanProgress({ status }: { readonly status: DuplicateReviewStatus }) {
  return (
    <section className="duplicate-progress" aria-live="polite">
      <strong>{phaseLabel(status.phase)}</strong>
      <span>
        已检查 {status.checkedFiles} 个文件 · 候选 {status.candidateFiles} 个 ·
        已完整验证 {status.hashedFiles} 个
      </span>
      {status.currentRelativePath !== null && (
        <code>{status.currentRelativePath}</code>
      )}
    </section>
  );
}

function Summary({ results }: { readonly results: DuplicateReviewResults }) {
  const entries = [
    ['重复集合', results.summary.equivalenceSets],
    ['仅 JPG 重复', results.summary.jpgOnlyFamilies],
    ['仅 RAF 重复', results.summary.rafOnlyFamilies],
    ['两种都重复', results.summary.jpgAndRafFamilies],
    ['冗余副本', results.summary.redundantFiles],
    ['理论可释放', formatBytes(results.summary.reclaimableBytes)],
  ];
  return (
    <dl className="duplicate-summary">
      {entries.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function DuplicateSet({
  set,
  selected,
  onToggle,
}: {
  readonly set: DuplicateSetResult;
  readonly selected: ReadonlySet<string>;
  readonly onToggle: (assetId: string) => void;
}) {
  return (
    <section className="duplicate-set">
      <div className="duplicate-set-heading">
        <strong>{setTitle(set)}</strong>
      </div>
      {set.members.map((member) => {
        const isSelected = selected.has(member.assetId);
        return (
          <DuplicateFileRow
            key={member.assetId}
            member={member}
            selected={isSelected}
            onToggle={() => onToggle(member.assetId)}
          />
        );
      })}
    </section>
  );
}

function DuplicateFileRow({
  member,
  selected,
  onToggle,
}: {
  readonly member: DuplicateMemberResult;
  readonly selected: boolean;
  readonly onToggle: () => void;
}) {
  return (
    <div
      className={selected ? 'duplicate-member is-selected' : 'duplicate-member'}
    >
      <input
        aria-label={
          selected
            ? `取消 ${member.fullPath} 的待删除标记`
            : `将 ${member.fullPath} 标记为待删除`
        }
        type="checkbox"
        checked={selected}
        disabled={!member.selectable}
        onChange={onToggle}
      />
      <div className="duplicate-member-info">
        <div className="duplicate-member-title">
          <code className="duplicate-member-name">{fileName(member)}</code>
          {member.suggestedRetain && (
            <span className="suggestion">建议保留</span>
          )}
        </div>
        <span className="duplicate-member-details" title={member.fullPath}>
          <code>{member.fullPath}</code> · {formatBytes(member.sizeBytes)}
          {member.captureLocalKey !== null
            ? ` · ${member.captureLocalKey}`
            : ''}
        </span>
      </div>
      <span className="duplicate-delete-state">{selected ? '待删除' : ''}</span>
      <button
        type="button"
        onClick={() =>
          void openDuplicateAssetLocation(member.assetId, member.assetVersion)
        }
      >
        在资源管理器中查看
      </button>
    </div>
  );
}

function fileName(member: DuplicateMemberResult): string {
  return member.relativePath.split(/[\\/]/).at(-1) ?? member.relativePath;
}
function setTitle(set: DuplicateSetResult): string {
  return `${set.role.toUpperCase()}：${set.members.length} 份（重复 ${set.redundantFiles} 份）`;
}
function phaseLabel(phase: string | null): string {
  return (
    (
      {
        Enumerating: '正在枚举',
        Hashing: '正在完整哈希',
        Comparing: '正在逐字节比较',
        Publishing: '正在发布结果',
      } as Record<string, string>
    )[phase ?? ''] ?? '正在检测'
  );
}
function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit < 2 ? 0 : 1)} ${units[unit]}`;
}
function readError(reason: unknown): string {
  if (typeof reason === 'object' && reason !== null && 'userMessage' in reason)
    return String(reason.userMessage);
  return reason instanceof Error ? reason.message : '重复复核操作未完成。';
}
