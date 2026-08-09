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

export function PhotoCanvas({
  photo,
  onWheel,
  onZoomModeChange,
}: {
  photo: PhotoSummary | null;
  onWheel: (event: WheelEvent) => void;
  onZoomModeChange?: (mode: CanvasZoomMode) => void;
}) {
  const [previewState, setPreviewState] = useState<
    'loading' | 'ready' | 'unavailable'
  >('loading');
  const [zoomMode, setZoomMode] = useState<CanvasZoomMode>('fit');
  const [naturalSize, setNaturalSize] = useState<Size | null>(null);
  const [viewportSize, setViewportSize] = useState<Size>({
    width: 0,
    height: 0,
  });
  const viewportRef = useRef<HTMLDivElement>(null);
  const centerFrameRef = useRef<number | null>(null);

  const resetToFit = useCallback(() => {
    setZoomMode('fit');
    setNaturalSize(null);
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

  const statusText =
    previewState !== 'ready' || displayScale === null
      ? null
      : zoomMode === 'actual'
        ? '100%'
        : `适合窗口 ${Math.round(displayScale * 100)}%`;

  return (
    <section
      className="photo-canvas"
      aria-label="当前照片"
      onWheel={zoomMode === 'fit' ? onWheel : undefined}
      data-zoom-mode={zoomMode}
    >
      {photo === null ? (
        <div className="empty-gallery">
          <div className="empty-icon" aria-hidden="true">
            ◇
          </div>
          <h2>图库中还没有照片</h2>
          <p>本次扫描可靠完成，但没有发现 JPG、JPEG 或 RAF 照片。</p>
        </div>
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
            <div className="canvas-unavailable">
              <strong>RAF 预览不可用</strong>
              <span>未能读取此文件的内嵌 JPEG；原始文件没有被修改。</span>
            </div>
          )}
        </div>
      )}
      {statusText !== null && (
        <span className="zoom-status" aria-label="缩放状态">
          {statusText}
        </span>
      )}
    </section>
  );
}
