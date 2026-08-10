import { useEffect, useMemo, useState, type RefObject } from 'react';

import {
  addDuplicateSubsetToPendingDelete,
  cancelDuplicateReviewScan,
  getDuplicateReviewResults,
  openDuplicateAssetLocation,
  startDuplicateReviewScan,
  type DuplicateReviewResults,
  type DuplicateReviewStatus,
  type DuplicateSetResult,
  type DuplicateSubsetSelection,
} from '../../shared/commands/duplicate-review';
import type { PendingDeleteSummary } from '../../shared/commands/library';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';

interface DuplicateReviewViewProps {
  readonly status: DuplicateReviewStatus;
  readonly pendingDeleteSummary: PendingDeleteSummary;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onStatusChange: (status: DuplicateReviewStatus) => void;
  readonly onOpenLibrary: () => void;
  readonly onOpenSettings: () => void;
  readonly onPendingAdded: () => Promise<void>;
}

export function DuplicateReviewView({
  status,
  pendingDeleteSummary,
  settingsButtonRef,
  onStatusChange,
  onOpenLibrary,
  onOpenSettings,
  onPendingAdded,
}: DuplicateReviewViewProps) {
  const [results, setResults] = useState<DuplicateReviewResults | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [retained, setRetained] = useState<ReadonlyMap<string, string>>(
    new Map(),
  );
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
        if (next.expired) setSelected(new Set());
      })
      .catch((reason) => setError(readError(reason)));
  }, [status.runId, status.state]);

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
  const invalidWholeSet = sets.some(
    (set) =>
      set.members.length > 0 &&
      set.members.every((member) => selected.has(member.assetId)),
  );
  const selections = buildSelections(sets, selected, retained);

  const begin = async () => {
    setBusy(true);
    setError(null);
    setSelected(new Set());
    try {
      onStatusChange(await startDuplicateReviewScan());
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  const submit = async () => {
    if (status.runId === null || selections.length === 0 || invalidWholeSet)
      return;
    setBusy(true);
    setError(null);
    try {
      await addDuplicateSubsetToPendingDelete(status.runId, selections);
      setSelected(new Set());
      setConfirming(false);
      await onPendingAdded();
    } catch (reason) {
      setError(readError(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="screen duplicate-review-screen">
      <TopBar
        context="重复复核"
        settingsButtonRef={settingsButtonRef}
        pendingDeleteSummary={pendingDeleteSummary}
        onOpenLibrary={onOpenLibrary}
        onOpenPendingDelete={onOpenLibrary}
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
            <strong>上次检测未发现重复</strong>
          </div>
        )}
        {results !== null && !results.expired && (
          <>
            <Summary results={results} />
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
                        retained={
                          retained.get(set.setId) ??
                          set.suggestedRetainedAssetId
                        }
                        onRetain={(assetId) =>
                          setRetained((current) =>
                            new Map(current).set(set.setId, assetId),
                          )
                        }
                        onToggle={(assetId) =>
                          setSelected((current) => toggleSet(current, assetId))
                        }
                        onSelectDirectory={(directory) =>
                          setSelected((current) => {
                            const next = new Set(current);
                            for (const member of set.members)
                              if (
                                member.directory === directory &&
                                member.selectable &&
                                member.assetId !==
                                  (retained.get(set.setId) ??
                                    set.suggestedRetainedAssetId)
                              )
                                next.add(member.assetId);
                            return next;
                          })
                        }
                      />
                    ))}
                </article>
              ))}
            </div>
            <div className="duplicate-selection-bar">
              <span>
                {selections.length} 个文件 ·{' '}
                {formatBytes(
                  selections.reduce(
                    (sum, item) =>
                      sum + (members.get(item.deletedAssetId)?.sizeBytes ?? 0),
                    0,
                  ),
                )}
              </span>
              {invalidWholeSet && (
                <span className="selection-warning">
                  每个集合必须至少保留一份。
                </span>
              )}
              <button
                type="button"
                disabled={selections.length === 0 || invalidWholeSet || busy}
                onClick={() => setConfirming(true)}
              >
                加入待删除…
              </button>
            </div>
          </>
        )}
      </main>
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
            {selections.length} 个文件，共{' '}
            {formatBytes(
              selections.reduce(
                (sum, item) =>
                  sum + (members.get(item.deletedAssetId)?.sizeBytes ?? 0),
                0,
              ),
            )}
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
  retained,
  onRetain,
  onToggle,
  onSelectDirectory,
}: {
  readonly set: DuplicateSetResult;
  readonly selected: ReadonlySet<string>;
  readonly retained: string | null;
  readonly onRetain: (assetId: string) => void;
  readonly onToggle: (assetId: string) => void;
  readonly onSelectDirectory: (directory: string) => void;
}) {
  const directories = [
    ...new Set(set.members.map((member) => member.directory)),
  ];
  return (
    <section className="duplicate-set">
      <div className="duplicate-set-heading">
        <strong>{setTitle(set)}</strong>
        {directories.map((directory) => (
          <button
            key={directory}
            type="button"
            onClick={() => onSelectDirectory(directory)}
          >
            选择这一整套副本 · {directory}
          </button>
        ))}
      </div>
      {set.members.map((member) => (
        <div className="duplicate-member" key={member.assetId}>
          <input
            aria-label={`选择删除 ${member.fullPath}`}
            type="checkbox"
            checked={selected.has(member.assetId)}
            disabled={!member.selectable || retained === member.assetId}
            onChange={() => onToggle(member.assetId)}
          />
          <div>
            <code>{member.fullPath}</code>
            <span>
              {member.directory} · {formatBytes(member.sizeBytes)}
              {member.captureLocalKey !== null
                ? ` · ${member.captureLocalKey}`
                : ''}
            </span>
          </div>
          {member.suggestedRetain && (
            <span className="suggestion">建议保留</span>
          )}
          <label>
            保留{' '}
            <input
              type="radio"
              name={`retain-${set.setId}`}
              checked={retained === member.assetId}
              disabled={!member.selectable || selected.has(member.assetId)}
              onChange={() => onRetain(member.assetId)}
            />
          </label>
          <button
            type="button"
            onClick={() =>
              void openDuplicateAssetLocation(
                member.assetId,
                member.assetVersion,
              )
            }
          >
            在资源管理器中查看
          </button>
        </div>
      ))}
    </section>
  );
}

function buildSelections(
  sets: readonly DuplicateSetResult[],
  selected: ReadonlySet<string>,
  retained: ReadonlyMap<string, string>,
): DuplicateSubsetSelection[] {
  return sets.flatMap((set) => {
    const retainedId = retained.get(set.setId) ?? set.suggestedRetainedAssetId;
    const retainedMember = set.members.find(
      (member) => member.assetId === retainedId,
    );
    if (retainedMember === undefined) return [];
    return set.members
      .filter(
        (member) =>
          selected.has(member.assetId) && member.assetId !== retainedId,
      )
      .map((member) => ({
        setId: set.setId,
        deletedAssetId: member.assetId,
        deletedAssetVersion: member.assetVersion,
        retainedAssetId: retainedMember.assetId,
        retainedAssetVersion: retainedMember.assetVersion,
      }));
  });
}

function toggleSet(
  current: ReadonlySet<string>,
  assetId: string,
): ReadonlySet<string> {
  const next = new Set(current);
  if (next.has(assetId)) next.delete(assetId);
  else next.add(assetId);
  return next;
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
