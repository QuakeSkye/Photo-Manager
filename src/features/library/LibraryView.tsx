import {
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
  type SortMode,
} from '../../shared/commands/library';
import {
  choosePhotoshop,
  getPhotoshopStatus,
  openPhotoFileLocation,
  openRafInPhotoshop,
  type PhotoshopStatus,
} from '../../shared/commands/settings';
import { Modal } from '../../shared/ui/Modal';
import { TopBar } from '../../shared/ui/TopBar';
import { VirtualFilmstrip } from './VirtualFilmstrip';
import { PhotoCanvas } from './PhotoCanvas';
import { PhotoInspector } from './PhotoInspector';

interface LibraryViewProps {
  readonly snapshot: LibrarySnapshot;
  readonly selectedPhotoId: string | null;
  readonly selectedIndex: number;
  readonly sortMode: SortMode;
  readonly settingsButtonRef: RefObject<HTMLButtonElement | null>;
  readonly onSelectIndex: (index: number) => void;
  readonly onNavigate: (delta: number) => void;
  readonly onSortChange: (mode: SortMode) => void;
  readonly onOpenSettings: () => void;
  readonly onOpenScan: () => void;
  readonly mutationPending: boolean;
  readonly onToggleDeletion: () => void;
  readonly onUndoDeletion: () => void;
  readonly onOpenPendingDelete: () => void;
}

export function LibraryView({
  snapshot,
  selectedPhotoId,
  selectedIndex,
  sortMode,
  settingsButtonRef,
  onSelectIndex,
  onNavigate,
  onSortChange,
  onOpenSettings,
  onOpenScan,
  mutationPending,
  onToggleDeletion,
  onUndoDeletion,
  onOpenPendingDelete,
}: LibraryViewProps) {
  const current = snapshot.photos[selectedIndex] ?? null;
  const [detail, setDetail] = useState<PhotoDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [toolStatus, setToolStatus] = useState<PhotoshopStatus | null>(null);
  const [externalPending, setExternalPending] = useState(false);
  const [externalError, setExternalError] = useState<string | null>(null);
  const [confirmPhotoshop, setConfirmPhotoshop] = useState(false);
  const wheelAccumulator = useRef(0);
  const wheelLocked = useRef(false);

  useEffect(() => {
    void getPhotoshopStatus()
      .then(setToolStatus)
      .catch((reason) => setExternalError(readError(reason)));
  }, []);

  useEffect(() => {
    if (current === null) {
      setDetail(null);
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
    if (wheelLocked.current || snapshot.photos.length < 2) return;
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
  const rootOffline = snapshot.binding?.status !== 'Online';

  const requestPhotoshop = async () => {
    if (detail?.raf === null || detail === null || externalPending) return;
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
    if (detail === null || externalPending) return;
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
      {snapshot.scan?.kind === 'incremental' &&
        snapshot.scan.phase !== 'completed' && (
          <button className="scan-banner" type="button" onClick={onOpenScan}>
            <span className="activity-dot" aria-hidden="true" />
            <span>
              {snapshot.scan.phase === 'running'
                ? '正在更新图库'
                : scanPhaseText(snapshot.scan.phase)}
            </span>
            <span className="scan-banner-detail">
              已检查 {snapshot.scan.filesChecked.toLocaleString('zh-CN')} 个文件
            </span>
            <strong>查看进度</strong>
          </button>
        )}
      <main className="photo-workspace">
        <VirtualFilmstrip
          photos={snapshot.photos}
          selectedPhotoId={selectedPhotoId}
          selectedIndex={selectedIndex}
          onSelectIndex={onSelectIndex}
          onWheel={handleWheel}
        />
        <PhotoCanvas photo={current} onWheel={handleWheel} />
        <PhotoInspector
          detail={detail}
          loading={current !== null && detail === null && detailError === null}
          error={detailError}
          marked={current?.deletionState !== null}
          mutationPending={mutationPending}
          toolStatus={toolStatus}
          externalActionPending={externalPending}
          externalError={externalError}
          onOpenPhotoshop={() => void requestPhotoshop()}
          onOpenFileLocation={() => void revealCurrent()}
          onOpenSettings={onOpenSettings}
          onToggleDeletion={onToggleDeletion}
        />
      </main>
      <footer className="status-bar">
        <div>
          {snapshot.photos.length === 0
            ? '0 / 0'
            : `${(selectedIndex + 1).toLocaleString('zh-CN')} / ${snapshot.photos.length.toLocaleString('zh-CN')}`}
        </div>
        <div className="shortcut-hint">
          <kbd>←</kbd>
          <kbd>→</kbd> 浏览 <span>·</span> <kbd>D</kbd> 标记 <span>·</span>{' '}
          <button
            className="status-link"
            type="button"
            disabled={mutationPending}
            onClick={onUndoDeletion}
          >
            <kbd>R</kbd> 撤销
          </button>
        </div>
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
