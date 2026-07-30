import { useCallback, useState, type WheelEvent } from 'react';

import type { PhotoSummary } from '../../shared/commands/library';
import { PreviewImage } from './preview';

export function PhotoCanvas({
  photo,
  onWheel,
}: {
  photo: PhotoSummary | null;
  onWheel: (event: WheelEvent) => void;
}) {
  const [previewState, setPreviewState] = useState<
    'loading' | 'ready' | 'unavailable'
  >('loading');
  const onStateChange = useCallback(
    (state: 'loading' | 'ready' | 'unavailable') => setPreviewState(state),
    [],
  );
  return (
    <section className="photo-canvas" aria-label="当前照片" onWheel={onWheel}>
      {photo === null ? (
        <div className="empty-gallery">
          <div className="empty-icon" aria-hidden="true">
            ▧
          </div>
          <h2>图库中还没有照片</h2>
          <p>本次扫描可靠完成，但没有发现 JPG、JPEG 或 RAF 照片。</p>
        </div>
      ) : (
        <div className="photo-stage" data-preview-state={previewState}>
          <PreviewImage
            reference={photo.preview}
            alt={photo.fileName}
            onStateChange={onStateChange}
          />
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
    </section>
  );
}
