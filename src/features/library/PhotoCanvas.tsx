import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type WheelEvent,
} from 'react';

import type { PhotoSummary } from '../../shared/commands/library';
import { PreviewImage } from './preview';

export type CanvasZoomMode = 'fit' | 'actual';

interface Size {
  readonly width: number;
  readonly height: number;
}

export interface PhotoCanvasProps {
  readonly photo: PhotoSummary | null;
  readonly onWheel: (event: WheelEvent) => void;
  readonly onZoomModeChange?: (mode: CanvasZoomMode) => void;
  readonly isFilteredEmpty?: boolean;
  readonly onClearFilters?: () => void;
  readonly feedbackMessage?: string | null;
  readonly rootOffline?: boolean;
  readonly canOpenFileLocation?: boolean;
  readonly canOpenPhotoshop?: boolean;
  readonly showPhotoshopAction?: boolean;
  readonly externalActionPending?: boolean;
  readonly onOpenFileLocation?: () => void;
  readonly onOpenPhotoshop?: () => void;
}

export function PhotoCanvas({
  photo,
  onWheel,
  onZoomModeChange,
  isFilteredEmpty = false,
  onClearFilters,
  feedbackMessage = null,
  rootOffline = false,
  canOpenFileLocation = false,
  canOpenPhotoshop = false,
  showPhotoshopAction = false,
  externalActionPending = false,
  onOpenFileLocation,
  onOpenPhotoshop,
}: PhotoCanvasProps) {
  const [previewState, setPreviewState] = useState<
    'loading' | 'ready' | 'unavailable'
  >('loading');
  const [zoomMode, setZoomMode] = useState<CanvasZoomMode>('fit');
  const [naturalSize, setNaturalSize] = useState<Size | null>(null);
  const [viewportSize, setViewportSize] = useState<Size>({
    width: 0,
    height: 0,
  });
  const [retryToken, setRetryToken] = useState(0);
  const viewportRef = useRef<HTMLDivElement>(null);
  const centerFrameRef = useRef<number | null>(null);

  const resetToFit = useCallback(() => {
    setZoomMode('fit');
    setNaturalSize(null);
    setRetryToken(0);
    const viewport = viewportRef.current;
    if (viewport !== null) {
      viewport.scrollLeft = 0;
      viewport.scrollTop = 0;
    }
    onZoomModeChange?.('fit');
  }, [onZoomModeChange]);

  useEffect(() => {
    resetToFit();
  }, [photo?.photoId, photo?.photoVersion, resetToFit]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport === null) return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry === undefined) return;
      setViewportSize({
        width: entry.contentRect.width,
        height: entry.contentRect.height,
      });
    });
    observer.observe(viewport);
    return () => observer.disconnect();
  }, [photo]);

  const fitScale = useMemo(() => {
    if (
      naturalSize === null ||
      naturalSize.width <= 0 ||
      naturalSize.height <= 0 ||
      viewportSize.width <= 0 ||
      viewportSize.height <= 0
    )
      return null;
    return Math.min(
      1,
      viewportSize.width / naturalSize.width,
      viewportSize.height / naturalSize.height,
    );
  }, [naturalSize, viewportSize]);

  const displayScale = zoomMode === 'actual' ? 1 : fitScale;
  const displaySize =
    naturalSize === null || displayScale === null
      ? null
      : {
          width: naturalSize.width * displayScale,
          height: naturalSize.height * displayScale,
        };

  useLayoutEffect(() => {
    if (centerFrameRef.current !== null)
      cancelAnimationFrame(centerFrameRef.current);
    if (zoomMode !== 'actual' || displaySize === null) return;
    centerFrameRef.current = requestAnimationFrame(() => {
      centerFrameRef.current = null;
      const viewport = viewportRef.current;
      if (viewport === null) return;
      viewport.scrollLeft = Math.max(
        0,
        (viewport.scrollWidth - viewport.clientWidth) / 2,
      );
      viewport.scrollTop = Math.max(
        0,
        (viewport.scrollHeight - viewport.clientHeight) / 2,
      );
    });
    return () => {
      if (centerFrameRef.current !== null) {
        cancelAnimationFrame(centerFrameRef.current);
        centerFrameRef.current = null;
      }
    };
  }, [displaySize, zoomMode]);

  const toggleZoom = () => {
    if (previewState !== 'ready' || naturalSize === null) return;
    setZoomMode((current) => {
      const next = current === 'fit' ? 'actual' : 'fit';
      if (next === 'fit') {
        const viewport = viewportRef.current;
        if (viewport !== null) {
          viewport.scrollLeft = 0;
          viewport.scrollTop = 0;
        }
      }
      onZoomModeChange?.(next);
      return next;
    });
  };

  const onStateChange = useCallback(
    (state: 'loading' | 'ready' | 'unavailable') => {
      setPreviewState(state);
      if (state !== 'ready') setNaturalSize(null);
    },
    [],
  );

  const showZoomControls = previewState === 'ready' && displayScale !== null;

  const canRetryPreview =
    photo !== null &&
    !rootOffline &&
    photo.preview !== null &&
    photo.preview.state !== 'Unavailable';

  const hasUnavailableActions =
    canRetryPreview ||
    onOpenFileLocation !== undefined ||
    (showPhotoshopAction && onOpenPhotoshop !== undefined);

  return (
    <section
      className="photo-canvas"
      aria-label="当前照片"
      onWheel={zoomMode === 'fit' ? onWheel : undefined}
      data-zoom-mode={zoomMode}
    >
      {photo === null ? (
        isFilteredEmpty ? (
          <div className="empty-gallery empty-filter-result" role="status">
            <div className="empty-icon" aria-hidden="true">
              ⌕
            </div>
            <h2>当前筛选没有结果</h2>
            <p>图库中仍有照片，但没有匹配当前拍摄日期的项目。</p>
            {onClearFilters !== undefined && (
              <button
                type="button"
                className="secondary-button empty-filter-reset"
                onClick={onClearFilters}
              >
                清除全部筛选条件
              </button>
            )}
          </div>
        ) : (
          <div className="empty-gallery">
            <div className="empty-icon" aria-hidden="true">
              ◇
            </div>
            <h2>图库中还没有照片</h2>
            <p>本次扫描可靠完成，但没有发现 JPG、JPEG 或 RAF 照片。</p>
          </div>
        )
      ) : (
        <div
          ref={viewportRef}
          className="photo-stage"
          data-preview-state={previewState}
        >
          <div
            className="photo-scroll-content"
            style={
              displaySize === null
                ? undefined
                : {
                    width: `${displaySize.width}px`,
                    height: `${displaySize.height}px`,
                  }
            }
          >
            <PreviewImage
              reference={photo.preview}
              alt={photo.fileName}
              retryToken={retryToken}
              onStateChange={onStateChange}
              onImageLoad={({ width, height }) => {
                if (width > 0 && height > 0) setNaturalSize({ width, height });
              }}
              onImageDoubleClick={toggleZoom}
              imageStyle={
                displaySize === null
                  ? undefined
                  : {
                      width: `${displaySize.width}px`,
                      height: `${displaySize.height}px`,
                    }
              }
            />
          </div>
          {previewState === 'loading' && (
            <span className="canvas-feedback">正在准备预览…</span>
          )}
          {previewState === 'unavailable' && (
            <div className="canvas-unavailable" role="status">
              <strong>暂时无法生成这张照片的预览。</strong>
              <span>
                {rootOffline
                  ? '当前照片文件夹处于离线状态，恢复连接后可访问原文件。'
                  : photo.preview?.state === 'Unavailable'
                    ? photo.preview.mediaKind === 'Raf'
                      ? '未能读取该 RAF 文件的内嵌预览图像，可通过文件位置或外部工具查看。'
                      : '当前照片缺少可直接显示的预览图像，可通过文件位置查看。'
                    : '预览图像读取未成功完成，可重试或直接打开文件位置查看。'}
              </span>
              {hasUnavailableActions && (
                <div className="canvas-unavailable-actions">
                  {canRetryPreview && (
                    <button
                      type="button"
                      className="secondary-button"
                      onClick={() => setRetryToken((current) => current + 1)}
                    >
                      重试预览
                    </button>
                  )}
                  {onOpenFileLocation !== undefined && (
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!canOpenFileLocation || externalActionPending}
                      onClick={onOpenFileLocation}
                    >
                      打开当前文件位置
                    </button>
                  )}
                  {showPhotoshopAction && onOpenPhotoshop !== undefined && (
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!canOpenPhotoshop || externalActionPending}
                      onClick={onOpenPhotoshop}
                    >
                      用 Photoshop 打开 RAF
                    </button>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      )}
      {(showZoomControls || feedbackMessage !== null) && (
        <div className="canvas-overlay-footer">
          {feedbackMessage !== null ? (
            <div
              className="canvas-action-toast"
              role="status"
              aria-live="polite"
            >
              {feedbackMessage}
            </div>
          ) : (
            <span />
          )}
          {showZoomControls && (
            <div className="zoom-controls" role="group" aria-label="缩放方式">
              <button
                type="button"
                className="zoom-mode-button"
                aria-pressed={zoomMode === 'fit'}
                title="完整显示照片，不裁切"
                onClick={() => {
                  if (zoomMode !== 'fit') toggleZoom();
                }}
              >
                适应窗口
              </button>
              <button
                type="button"
                className="zoom-mode-button"
                aria-pressed={zoomMode === 'actual'}
                title="按预览图的实际像素显示；照片较大时可滚动查看"
                onClick={() => {
                  if (zoomMode !== 'actual') toggleZoom();
                }}
              >
                100%
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
