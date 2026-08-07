import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type WheelEvent,
} from 'react';

import type { PhotoSummary } from '../../shared/commands/library';
import { PreviewImage } from './preview';

const FILMSTRIP_ITEM_HEIGHT = 106;
const FILMSTRIP_ROW_HEIGHT = 112;
const OVERSCAN = 3;

const filmstripSizing = {
  '--filmstrip-item-height': `${FILMSTRIP_ITEM_HEIGHT}px`,
  '--filmstrip-row-height': `${FILMSTRIP_ROW_HEIGHT}px`,
} as CSSProperties;

interface VirtualFilmstripProps {
  readonly photos: readonly PhotoSummary[];
  readonly selectedPhotoId: string | null;
  readonly selectedIndex: number;
  readonly onSelectIndex: (index: number) => void;
  readonly onWheel: (event: WheelEvent) => void;
  readonly reviewMode?: boolean;
}

export function VirtualFilmstrip({
  photos,
  selectedPhotoId,
  selectedIndex,
  onSelectIndex,
  onWheel,
  reviewMode = false,
}: VirtualFilmstripProps) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(700);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport === null) return;
    const update = () => setHeight(viewport.clientHeight || 700);
    update();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(update);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport === null || selectedIndex < 0) return;
    const top = selectedIndex * FILMSTRIP_ROW_HEIGHT;
    const bottom = top + FILMSTRIP_ROW_HEIGHT;
    let nextScrollTop = viewport.scrollTop;
    if (top < viewport.scrollTop) nextScrollTop = top;
    else if (bottom > viewport.scrollTop + viewport.clientHeight)
      nextScrollTop = Math.max(0, bottom - viewport.clientHeight);
    if (nextScrollTop !== viewport.scrollTop) {
      viewport.scrollTop = nextScrollTop;
      setScrollTop(nextScrollTop);
    }
  }, [selectedIndex]);

  const start = Math.max(
    0,
    Math.floor(scrollTop / FILMSTRIP_ROW_HEIGHT) - OVERSCAN,
  );
  const end = Math.min(
    photos.length,
    Math.ceil((scrollTop + height) / FILMSTRIP_ROW_HEIGHT) + OVERSCAN,
  );
  const visible = photos.slice(start, end);

  return (
    <aside
      className={reviewMode ? 'filmstrip review-filmstrip' : 'filmstrip'}
      aria-label={reviewMode ? '待删除照片缩略图' : '照片缩略图'}
      style={filmstripSizing}
    >
      <div
        ref={viewportRef}
        className="filmstrip-viewport"
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        onWheel={onWheel}
      >
        <div
          className="filmstrip-track"
          style={{ height: photos.length * FILMSTRIP_ROW_HEIGHT }}
        >
          {visible.map((photo, offset) => {
            const index = start + offset;
            return (
              <button
                key={photo.photoId}
                className={
                  photo.photoId === selectedPhotoId
                    ? 'filmstrip-item is-selected'
                    : 'filmstrip-item'
                }
                style={{
                  transform: `translateY(${index * FILMSTRIP_ROW_HEIGHT}px)`,
                }}
                type="button"
                aria-label={`第 ${index + 1} 张：${photo.fileName}`}
                aria-current={
                  photo.photoId === selectedPhotoId ? 'true' : undefined
                }
                onClick={() => onSelectIndex(index)}
              >
                <PreviewImage reference={photo.preview} alt="" thumbnail />
                <span className="filmstrip-index">{index + 1}</span>
                {photo.deletionState !== null && (
                  <span
                    className={
                      photo.deletionState === 'Problem'
                        ? 'deletion-badge is-problem'
                        : 'deletion-badge'
                    }
                    aria-label={
                      photo.deletionState === 'Problem'
                        ? '待删除标记需要处理'
                        : '已标记待删除'
                    }
                  >
                    D
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>
    </aside>
  );
}
