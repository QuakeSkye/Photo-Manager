import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type RefObject,
  type WheelEvent,
} from 'react';

import {
  getPhotoDetail,
  type LibrarySnapshot,
  type PhotoDetail,
  type PhotoSummary,
  type SortMode,
} from '../../shared/commands/library';
import type { DuplicateReviewStatus } from '../../shared/commands/duplicate-review';
import {
  choosePhotoshop,
  getPhotoshopStatus,
  openPhotoFileLocation,
  openRafInPhotoshop,
  type PhotoshopStatus,
} from '../../shared/commands/settings';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';
import {
  VirtualFilmstrip,
  type FilmstripFilterControls,
} from './VirtualFilmstrip';
import { PhotoCanvas } from './PhotoCanvas';
import { PhotoInspector } from './PhotoInspector';

interface LibraryViewProps {
  readonly snapshot: LibrarySnapshot;
  readonly photos?: readonly PhotoSummary[];
  readonly selectedPhotoId: string | null;
  readonly selectedIndex: number;
  readonly sortMode: SortMode;
  readonly filterControls?: FilmstripFilterControls;
  readonly feedbackMessage?: string | null;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onSelectIndex: (index: number) => void;
  readonly onNavigate: (delta: number) => void;
  readonly onSortChange: (mode: SortMode) => void;
  readonly onOpenSettings: () => void;
  readonly onOpenImport: () => void;
  readonly duplicateReviewStatus?: DuplicateReviewStatus;
  readonly onOpenDuplicateReview?: () => void;
  readonly importEnabled?: boolean;
  readonly onOpenScan: () => void;
  readonly mutationPending: boolean;
  readonly onToggleDeletion: () => void;
  readonly onUndoDeletion: () => void;
  readonly onOpenPendingDelete: () => void;
}

export function LibraryView({
  snapshot,
  photos,
  selectedPhotoId,
  selectedIndex,
  sortMode,
  filterControls,
  feedbackMessage = null,
  settingsButtonRef,
  onSelectIndex,
  onNavigate,
  onSortChange,
  onOpenSettings,
  onOpenImport,
  duplicateReviewStatus,
  onOpenDuplicateReview,
  importEnabled = true,
  onOpenScan,
  mutationPending,
  onToggleDeletion,
  onUndoDeletion,
  onOpenPendingDelete,
}: LibraryViewProps) {
  const activePhotos = photos ?? snapshot.photos;
  const current = activePhotos[selectedIndex] ?? null;
  const [detail, setDetail] = useState<PhotoDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [toolStatus, setToolStatus] = useState<PhotoshopStatus | null>(null);
  const [externalPending, setExternalPending] = useState(false);
  const [externalError, setExternalError] = useState<string | null>(null);
  const [confirmPhotoshop, setConfirmPhotoshop] = useState(false);
  const wheelAccumulator = useRef(0);
  const wheelLocked = useRef(false);
  const wheelUnlockTimer = useRef<number | null>(null);

  const resetWheelNavigation = useCallback(() => {
    wheelAccumulator.current = 0;
    wheelLocked.current = false;
    if (wheelUnlockTimer.current !== null) {
      window.clearTimeout(wheelUnlockTimer.current);
      wheelUnlockTimer.current = null;
    }
  }, []);

  useEffect(() => {
    resetWheelNavigation();
    return resetWheelNavigation;
  }, [current?.photoId, current?.photoVersion, resetWheelNavigation]);

  useEffect(() => {
    void getPhotoshopStatus()
      .then(setToolStatus)
      .catch((reason) => setExternalError(readError(reason)));
  }, []);

  useEffect(() => {
    if (current === null) {
      setDetail(null);
      setDetailError(null);
      return;
    }
    let active = true;
    setDetail(null);
    setDetailError(null);
    void getPhotoDetail(current.photoId, current.photoVersion)
      .then((next) => {
        if (active) setDetail(next);
      })
      .catch((reason) => {
        if (active) setDetailError(readError(reason));
      });
    return () => {
      active = false;
    };
  }, [current]);

  const handleWheel = (event: WheelEvent) => {
    event.preventDefault();
    if (wheelLocked.current || activePhotos.length < 2) return;
    wheelAccumulator.current += event.deltaY;
    if (Math.abs(wheelAccumulator.current) < 36) return;
    const direction = wheelAccumulator.current > 0 ? 1 : -1;
    wheelAccumulator.current = 0;
    wheelLocked.current = true;
    onNavigate(direction);
    wheelUnlockTimer.current = window.setTimeout(() => {
      wheelLocked.current = false;
      wheelUnlockTimer.current = null;
    }, 90);
  };

  const dateContext =
    current?.captureLocalKey?.slice(0, 10).replaceAll('-', ' / ') ?? '日期未知';
  const rootOffline = snapshot.binding?.status !== 'Online';
  const isFilteredEmpty =
    snapshot.photos.length > 0 && activePhotos.length === 0;
  const canOpenFileLocation =
    !rootOffline &&
    detail !== null &&
    (detail.jpeg !== null || detail.raf !== null) &&
    !externalPending;
  const showPhotoshopAction =
    detail !== null
      ? detail.raf !== null
      : current?.preview?.mediaKind === 'Raf';
  const canOpenPhotoshop =
    !rootOffline &&
    detail !== null &&
    detail.raf !== null &&
    !externalPending &&
    !(toolStatus?.configured === true && !toolStatus.available);

  const requestPhotoshop = async () => {
    if (
      rootOffline ||
      detail?.raf === null ||
      detail === null ||
      externalPending
    )
      return;
    setExternalPending(true);
    setExternalError(null);
    try {
      let status = toolStatus ?? (await getPhotoshopStatus());
      setToolStatus(status);
      if (!status.configured) {
        const selected = await choosePhotoshop();
        if (selected === null) return;
        status = selected;
        setToolStatus(selected);
      }
      if (!status.available || status.toolId === null) {
        setExternalError('Photoshop 需要重新选择，请在设置中完成验证。');
        return;
      }
      setConfirmPhotoshop(true);
    } catch (reason) {
      setExternalError(readError(reason));
      void getPhotoshopStatus()
        .then(setToolStatus)
        .catch(() => undefined);
    } finally {
      setExternalPending(false);
    }
  };

  const confirmOpenPhotoshop = async () => {
    if (detail === null || toolStatus?.toolId === null || toolStatus === null)
      return;
    setExternalPending(true);
    setExternalError(null);
    try {
      await openRafInPhotoshop(
        toolStatus.toolId,
        detail.photoId,
        detail.photoVersion,
      );
      setConfirmPhotoshop(false);
    } catch (reason) {
      setConfirmPhotoshop(false);
      setExternalError(readError(reason));
      void getPhotoshopStatus()
        .then(setToolStatus)
        .catch(() => undefined);
    } finally {
      setExternalPending(false);
    }
  };

  const revealCurrent = async () => {
    if (rootOffline || detail === null || externalPending) return;
    setExternalPending(true);
    setExternalError(null);
    try {
      await openPhotoFileLocation(detail.photoId, detail.photoVersion);
    } catch (reason) {
      setExternalError(readError(reason));
    } finally {
      setExternalPending(false);
    }
  };

  return (
    <div className="screen library-screen">
      <TopBar
        dateContext={dateContext}
        sortMode={sortMode}
        settingsButtonRef={settingsButtonRef}
        showLibraryControls
        currentPhotoView="library"
        pendingDeleteSummary={snapshot.pendingDeleteSummary}
        onOpenLibrary={() => undefined}
        onOpenPendingDelete={onOpenPendingDelete}
        onSortChange={onSortChange}
        onOpenSettings={onOpenSettings}
        onOpenImport={onOpenImport}
        duplicateReviewStatus={duplicateReviewStatus}
        onOpenDuplicateReview={onOpenDuplicateReview}
        importEnabled={importEnabled}
      />
      {rootOffline && (
        <div className="offline-banner" role="status">
          <strong>图库当前离线</strong>
          <span>仍可查看已缓存信息；请在设置中重新选择照片文件夹。</span>
          <button type="button" onClick={onOpenSettings}>
            打开设置
          </button>
        </div>
      )}
      <main className="photo-workspace">
        <VirtualFilmstrip
          photos={activePhotos}
          selectedPhotoId={selectedPhotoId}
          selectedIndex={selectedIndex}
          onSelectIndex={onSelectIndex}
          onWheel={handleWheel}
          filterControls={filterControls}
        />
        <PhotoCanvas
          photo={current}
          onWheel={handleWheel}
          onZoomModeChange={resetWheelNavigation}
          isFilteredEmpty={isFilteredEmpty}
          onClearFilters={filterControls?.onClearFilters}
          feedbackMessage={feedbackMessage}
          rootOffline={rootOffline}
          canOpenFileLocation={canOpenFileLocation}
          canOpenPhotoshop={canOpenPhotoshop}
          showPhotoshopAction={showPhotoshopAction}
          externalActionPending={externalPending}
          onOpenFileLocation={() => void revealCurrent()}
          onOpenPhotoshop={() => void requestPhotoshop()}
        />
        <PhotoInspector
          detail={detail}
          loading={current !== null && detail === null && detailError === null}
          error={detailError}
          marked={current !== null && current.deletionState !== null}
          deletionState={current?.deletionState ?? null}
          rootOffline={rootOffline}
          mutationPending={mutationPending}
          toolStatus={toolStatus}
          externalActionPending={externalPending}
          externalError={externalError}
          onOpenPhotoshop={() => void requestPhotoshop()}
          onOpenFileLocation={() => void revealCurrent()}
          onOpenSettings={onOpenSettings}
          onToggleDeletion={onToggleDeletion}
          onUndoDeletion={onUndoDeletion}
        />
      </main>
      <footer className="status-bar">
        <div>
          {activePhotos.length === 0
            ? '0 / 0'
            : `${(selectedIndex + 1).toLocaleString('zh-CN')} / ${activePhotos.length.toLocaleString('zh-CN')}`}
          {filterControls?.hasActiveFilter === true && (
            <span className="status-filter-total">
              {' '}
              （全库 {snapshot.photos.length.toLocaleString('zh-CN')}）
            </span>
          )}
        </div>
        {snapshot.scan?.kind === 'incremental' &&
        snapshot.scan.phase !== 'completed' ? (
          <button
            className="status-link scan-status-center"
            type="button"
            onClick={onOpenScan}
          >
            {snapshot.scan.phase === 'running'
              ? '正在更新图库'
              : scanPhaseText(snapshot.scan.phase)}
            {' · '}已检查 {snapshot.scan.filesChecked.toLocaleString('zh-CN')}{' '}
            个文件 · 查看进度
          </button>
        ) : (
          <div />
        )}
        {snapshot.pendingDeleteSummary.totalCount > 0 ? (
          <button
            className="pending-status"
            type="button"
            onClick={onOpenPendingDelete}
          >
            待删除 {snapshot.pendingDeleteSummary.totalCount}
            {snapshot.pendingDeleteSummary.problemCount > 0 &&
              ` · 问题 ${snapshot.pendingDeleteSummary.problemCount}`}
          </button>
        ) : (
          <div />
        )}
      </footer>
      {confirmPhotoshop && detail !== null && (
        <Modal
          title="把 RAF 交给 Photoshop？"
          confirmLabel="用 Photoshop 打开 RAF"
          busy={externalPending}
          onConfirm={() => void confirmOpenPhotoshop()}
          onCancel={() => setConfirmPhotoshop(false)}
        >
          <p>{detail.fileName}</p>
          <p>
            文件交给 Photoshop 后，其修改、保存和附属文件行为由 Photoshop
            管理，不受本应用控制。本应用不会监控或替代 Photoshop 的保存行为。
          </p>
          <p className="safe-note">
            本应用在启动外部程序前只读取并验证照片，不会修改照片。
          </p>
        </Modal>
      )}
    </div>
  );
}

function scanPhaseText(phase: string): string {
  if (phase === 'cancelled') return '图库更新已取消';
  if (phase === 'failed') return '图库更新未完成';
  return '正在停止图库更新';
}

function readError(reason: unknown): string {
  if (typeof reason === 'object' && reason !== null && 'userMessage' in reason)
    return String(reason.userMessage);
  return reason instanceof Error ? reason.message : '无法读取当前照片信息。';
}
