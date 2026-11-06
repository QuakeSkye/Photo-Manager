import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type WheelEvent,
} from 'react';

import type { PhotoSummary } from '../../shared/commands/library';
import type { DateFilterOption, DateFilterValue } from './filtering';
import { PreviewImage } from './preview';

const FILMSTRIP_ITEM_HEIGHT = 106;
const FILMSTRIP_ROW_HEIGHT = 112;
const OVERSCAN = 3;

const filmstripSizing = {
  '--filmstrip-item-height': `${FILMSTRIP_ITEM_HEIGHT}px`,
  '--filmstrip-row-height': `${FILMSTRIP_ROW_HEIGHT}px`,
} as CSSProperties;

export interface FilmstripFilterControls {
  readonly dateFilter: DateFilterValue;
  readonly dateOptions: readonly DateFilterOption[];
  readonly totalPhotoCount: number;
  readonly hasActiveFilter: boolean;
  readonly onDateFilterChange: (value: DateFilterValue) => void;
  readonly onClearFilters: () => void;
}

interface VirtualFilmstripProps {
  readonly photos: readonly PhotoSummary[];
  readonly selectedPhotoId: string | null;
  readonly selectedIndex: number;
  readonly onSelectIndex: (index: number) => void;
  readonly onWheel: (event: WheelEvent) => void;
  readonly reviewMode?: boolean;
  readonly filterControls?: FilmstripFilterControls;
}

export function VirtualFilmstrip({
  photos,
  selectedPhotoId,
  selectedIndex,
  onSelectIndex,
  onWheel,
  reviewMode = false,
  filterControls,
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
    if (viewport === null) return;
    if (selectedIndex < 0) {
      if (photos.length === 0 && viewport.scrollTop !== 0) {
        viewport.scrollTop = 0;
        setScrollTop(0);
      }
      return;
    }
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
  }, [photos.length, selectedIndex]);

  const start = Math.max(
    0,
    Math.floor(scrollTop / FILMSTRIP_ROW_HEIGHT) - OVERSCAN,
  );
  const end = Math.min(
    photos.length,
    Math.ceil((scrollTop + height) / FILMSTRIP_ROW_HEIGHT) + OVERSCAN,
  );
  const visible = photos.slice(start, end);
  const showFilterBar =
    filterControls !== undefined &&
    (filterControls.totalPhotoCount > 0 || filterControls.hasActiveFilter);

  return (
    <aside
      className={reviewMode ? 'filmstrip review-filmstrip' : 'filmstrip'}
      aria-label={reviewMode ? '待删除照片缩略图' : '照片缩略图'}
      style={filmstripSizing}
    >
      {showFilterBar && (
        <div
          className="filmstrip-filter-bar"
          role="group"
          aria-label="照片定位与筛选"
        >
          <label className="filmstrip-date-filter">
            <span className="sr-only">拍摄日期筛选</span>
            <select
              aria-label="拍摄日期筛选"
              value={filterControls.dateFilter}
              onChange={(event) =>
                filterControls.onDateFilterChange(event.target.value)
              }
            >
              {filterControls.dateOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label} ({option.count})
                </option>
              ))}
            </select>
          </label>
          <div className="filmstrip-filter-meta">
            <span className="filmstrip-filter-count" aria-live="polite">
              {filterControls.hasActiveFilter
                ? `结果 ${photos.length.toLocaleString('zh-CN')} / ${filterControls.totalPhotoCount.toLocaleString('zh-CN')}`
                : `共 ${photos.length.toLocaleString('zh-CN')} 张`}
            </span>
            {filterControls.hasActiveFilter && (
              <button
                type="button"
                className="filmstrip-clear-all"
                onClick={filterControls.onClearFilters}
              >
                清除筛选
              </button>
            )}
          </div>
        </div>
      )}
      <div
        ref={viewportRef}
        className="filmstrip-viewport"
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        onWheel={onWheel}
      >
        {photos.length === 0 &&
        filterControls !== undefined &&
        filterControls.totalPhotoCount > 0 ? (
          <div className="filmstrip-empty-filter" role="status">
            <p>没有匹配当前筛选的照片</p>
            <button
              type="button"
              className="secondary-button filmstrip-empty-reset"
              onClick={filterControls.onClearFilters}
            >
              清除全部筛选
            </button>
          </div>
        ) : (
          <div
            className="filmstrip-track"
            style={{ height: photos.length * FILMSTRIP_ROW_HEIGHT }}
          >
            {visible.map((photo, offset) => {
              const index = start + offset;
              const isProblem = photo.deletionState === 'Problem';
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
                        isProblem
                          ? 'deletion-badge is-problem'
                          : 'deletion-badge'
                      }
                      aria-label={
                        isProblem ? '待删除标记需要处理' : '已标记待删除'
                      }
                      title={
                        isProblem ? '待删除标记需要处理' : '已标记待删除 (D)'
                      }
                    >
                      {isProblem ? '!' : 'D 待删'}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        )}
      </div>
    </aside>
  );
}
