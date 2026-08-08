import { useRef, type RefObject, type WheelEvent } from 'react';

import type {
  PendingDeleteSnapshot,
  SortMode,
} from '../../shared/commands/library';
import { TopBar } from '../../shared/ui/TopBar';
import { PhotoCanvas } from '../library/PhotoCanvas';
import { VirtualFilmstrip } from '../library/VirtualFilmstrip';

interface PendingDeleteViewProps {
  readonly snapshot: PendingDeleteSnapshot;
  readonly selectedPhotoId: string | null;
  readonly selectedIndex: number;
  readonly sortMode: SortMode;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly mutationPending: boolean;
  readonly onSelectIndex: (index: number) => void;
  readonly onNavigate: (delta: number) => void;
  readonly onSortChange: (mode: SortMode) => void;
  readonly onOpenLibrary: () => void;
  readonly onOpenSettings: () => void;
  readonly onOpenImport: () => void;
  readonly onCancelCurrent: () => void;
  readonly onUndoDeletion: () => void;
  readonly onRescan: () => void;
  readonly onExecute: () => void;
  readonly onRetryRemaining: () => void;
  readonly onCancelRemaining: () => void;
  readonly recycleMessage: string | null;
}

export function PendingDeleteView({
  snapshot,
  selectedPhotoId,
  selectedIndex,
  sortMode,
  settingsButtonRef,
  mutationPending,
  onSelectIndex,
  onNavigate,
  onSortChange,
  onOpenLibrary,
  onOpenSettings,
  onOpenImport,
  onCancelCurrent,
  onUndoDeletion,
  onRescan,
  onExecute,
  onRetryRemaining,
  onCancelRemaining,
  recycleMessage,
}: PendingDeleteViewProps) {
  const current = snapshot.items[selectedIndex] ?? null;
  const wheelAccumulator = useRef(0);
  const wheelLocked = useRef(false);
  const handleWheel = (event: WheelEvent) => {
    event.preventDefault();
    if (mutationPending || wheelLocked.current || snapshot.items.length < 2)
      return;
    wheelAccumulator.current += event.deltaY;
    if (Math.abs(wheelAccumulator.current) < 36) return;
    const direction = wheelAccumulator.current > 0 ? 1 : -1;
    wheelAccumulator.current = 0;
    wheelLocked.current = true;
    onNavigate(direction);
    window.setTimeout(() => {
      wheelLocked.current = false;
    }, 90);
  };
  const dateContext =
    current?.captureLocalKey?.slice(0, 10).replaceAll('-', ' / ') ?? '日期未知';

  return (
    <div className="screen pending-delete-screen">
      <TopBar
        dateContext={dateContext}
        sortMode={sortMode}
        settingsButtonRef={settingsButtonRef}
        showLibraryControls
        currentPhotoView="pending-delete"
        pendingDeleteSummary={snapshot.summary}
        onOpenLibrary={onOpenLibrary}
        onOpenPendingDelete={() => undefined}
        onSortChange={onSortChange}
        onOpenSettings={onOpenSettings}
        onOpenImport={onOpenImport}
      />
      <main className="photo-workspace">
        <VirtualFilmstrip
          photos={snapshot.items}
          selectedPhotoId={selectedPhotoId}
          selectedIndex={selectedIndex}
          onSelectIndex={onSelectIndex}
          onWheel={handleWheel}
          reviewMode
        />
        <div className="pending-canvas-wrap">
          {current !== null && (
            <div className="pending-position">
              待删除 · 第 {selectedIndex + 1} / {snapshot.items.length} 张
            </div>
          )}
          <PhotoCanvas photo={current} onWheel={handleWheel} />
        </div>
        <aside
          className="photo-inspector pending-inspector"
          aria-label="待删除详情"
        >
          {current !== null && (
            <>
              <div className="inspector-scroll">
                <section className="photo-identity">
                  <p className="section-label">
                    {current.problem === null
                      ? '待删除 · 已持久化'
                      : '待删除 · 需要处理'}
                  </p>
                  <h2 title={current.fileName}>{current.fileName}</h2>
                  <div className="format-badges" aria-label="照片格式">
                    <span className="format-badge">
                      JPG {current.jpegPresent ? '✓' : '×'}
                    </span>
                    <span className="format-badge">
                      RAF {current.rafPresent ? '✓' : '×'}
                    </span>
                  </div>
                </section>
                {current.problem !== null && (
                  <div className="problem-panel" role="status">
                    <strong>需要重新确认</strong>
                    <span>{current.problem.message}</span>
                  </div>
                )}
                <section className="metadata-section">
                  <h3>复核信息</h3>
                  <dl>
                    <div className="metadata-row">
                      <dt>相对目录</dt>
                      <dd>{current.relativeDirectory}</dd>
                    </div>
                    <div className="metadata-row">
                      <dt>物理文件</dt>
                      <dd>{current.physicalFileCount}</dd>
                    </div>
                    <div className="metadata-row">
                      <dt>合计容量</dt>
                      <dd>{formatBytes(current.totalSizeBytes)}</dd>
                    </div>
                  </dl>
                </section>
              </div>
              <div className="inspector-spacer" />
              <section className="pending-actions" aria-label="待删除动作区">
                <button
                  className="neutral-button"
                  type="button"
                  disabled={mutationPending}
                  onClick={onCancelCurrent}
                >
                  {mutationPending
                    ? '正在提交…'
                    : current.problem === null
                      ? '取消当前待删除'
                      : '取消失效标记'}{' '}
                  <kbd>D</kbd>
                </button>
                {current.problem?.canRescan === true && (
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={mutationPending}
                    onClick={onRescan}
                  >
                    重新扫描确认
                  </button>
                )}
                {current.problem?.canRebind === true && (
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={mutationPending}
                    onClick={onOpenSettings}
                  >
                    前往照片文件夹设置
                  </button>
                )}
                {(current.problem?.kind === 'RetryableFailed' ||
                  current.problem?.kind === 'Partial') && (
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={mutationPending}
                    onClick={onRetryRemaining}
                  >
                    重试剩余文件…
                  </button>
                )}
                {current.problem !== null &&
                  ['RetryableFailed', 'Partial', 'NeedsAttention'].includes(
                    current.problem.kind,
                  ) && (
                    <button
                      className="neutral-button"
                      type="button"
                      disabled={mutationPending}
                      onClick={onCancelRemaining}
                    >
                      取消剩余删除
                    </button>
                  )}
                <div className="batch-summary">
                  <strong>
                    {snapshot.summary.executableCount} 张可复核删除
                  </strong>
                  <span>
                    {snapshot.summary.executableFileCount} 个文件 ·{' '}
                    {formatBytes(snapshot.summary.executableBytes)}
                  </span>
                  {snapshot.summary.problemCount > 0 && (
                    <span>{snapshot.summary.problemCount} 个问题待处理</span>
                  )}
                </div>
                <button
                  className="execute-delete-button"
                  type="button"
                  disabled={
                    mutationPending || snapshot.summary.executableCount === 0
                  }
                  onClick={onExecute}
                >
                  执行删除…
                </button>
                <p>
                  仅在应用生成的可丢弃 NTFS 测试 Root 上强制移入 Windows
                  回收站；不提供永久删除或降级。
                </p>
                {recycleMessage !== null && (
                  <div className="problem-panel" role="status">
                    <strong>本次删除结果</strong>
                    <span>{recycleMessage}</span>
                  </div>
                )}
              </section>
            </>
          )}
        </aside>
      </main>
      <footer className="status-bar">
        <div>
          {selectedIndex + 1} / {snapshot.items.length}
        </div>
        <div className="shortcut-hint">
          <kbd>←</kbd>
          <kbd>→</kbd> 复核 <span>·</span> <kbd>D</kbd> 取消 <span>·</span>{' '}
          <button
            className="status-link"
            type="button"
            disabled={mutationPending}
            onClick={onUndoDeletion}
          >
            <kbd>R</kbd> 撤销
          </button>
        </div>
        <div className="pending-status">
          待删除 {snapshot.summary.totalCount}
          {snapshot.summary.problemCount > 0 &&
            ` · 问题 ${snapshot.summary.problemCount}`}
        </div>
      </footer>
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit < 2 ? 0 : 1)} ${units[unit]}`;
}
