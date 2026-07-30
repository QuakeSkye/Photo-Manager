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
}: LibraryViewProps) {
  const current = snapshot.photos[selectedIndex] ?? null;
  const [detail, setDetail] = useState<PhotoDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const wheelAccumulator = useRef(0);
  const wheelLocked = useRef(false);

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

  return (
    <div className="screen library-screen">
      <TopBar
        dateContext={dateContext}
        sortMode={sortMode}
        settingsButtonRef={settingsButtonRef}
        showLibraryControls
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
          <kbd>→</kbd> 浏览 <span>·</span> 鼠标滚轮切换
        </div>
        <div className="readonly-status">
          <span aria-hidden="true">◉</span> 只读浏览
        </div>
      </footer>
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
