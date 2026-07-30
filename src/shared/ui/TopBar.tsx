import type { RefObject } from 'react';

import type { PendingDeleteSummary, SortMode } from '../commands/library';

interface TopBarProps {
  readonly context?: string;
  readonly dateContext?: string;
  readonly sortMode?: SortMode;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly showLibraryControls?: boolean;
  readonly currentPhotoView?: 'library' | 'pending-delete';
  readonly pendingDeleteSummary?: PendingDeleteSummary;
  readonly onOpenLibrary?: () => void;
  readonly onOpenPendingDelete?: () => void;
  readonly onSortChange?: (mode: SortMode) => void;
  readonly onOpenSettings: () => void;
  readonly onBack?: () => void;
}

export function TopBar({
  context,
  dateContext,
  sortMode,
  settingsButtonRef,
  showLibraryControls = false,
  currentPhotoView = 'library',
  pendingDeleteSummary,
  onOpenLibrary,
  onOpenPendingDelete,
  onSortChange,
  onOpenSettings,
  onBack,
}: TopBarProps) {
  return (
    <header className="top-bar">
      <div className="brand" aria-label="Fuji Library">
        <span className="brand-mark" aria-hidden="true">
          F
        </span>
        <span>FUJI 图库</span>
      </div>
      <nav className="photo-tabs" aria-label="照片视图">
        <button
          className={
            currentPhotoView === 'library' ? 'photo-tab is-active' : 'photo-tab'
          }
          type="button"
          onClick={onOpenLibrary}
        >
          图库
        </button>
        {(pendingDeleteSummary?.totalCount ?? 0) > 0 && (
          <button
            className={
              currentPhotoView === 'pending-delete'
                ? 'photo-tab danger-tab is-active'
                : 'photo-tab danger-tab'
            }
            type="button"
            onClick={onOpenPendingDelete}
          >
            待删除 {pendingDeleteSummary?.totalCount}
            {(pendingDeleteSummary?.problemCount ?? 0) > 0 && (
              <span className="tab-warning" aria-label="含待处理问题">
                !
              </span>
            )}
          </button>
        )}
      </nav>
      {context !== undefined && <div className="tool-context">{context}</div>}
      <div className="top-spacer" />
      {showLibraryControls && (
        <>
          <div className="date-context" aria-label="当前照片日期">
            {dateContext ?? '日期未知'}
          </div>
          <label className="sort-control">
            <span className="sr-only">照片排序</span>
            <select
              value={sortMode}
              onChange={(event) =>
                onSortChange?.(event.target.value as SortMode)
              }
            >
              <option value="captureDesc">最新优先</option>
              <option value="captureAsc">最早优先</option>
            </select>
          </label>
        </>
      )}
      <div className="tool-actions">
        <button
          ref={settingsButtonRef}
          className="tool-button"
          type="button"
          onClick={onOpenSettings}
        >
          <span aria-hidden="true">⚙</span> 设置
        </button>
        {onBack !== undefined && (
          <button className="tool-button" type="button" onClick={onBack}>
            返回图库
          </button>
        )}
      </div>
    </header>
  );
}
