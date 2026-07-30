import { useEffect, useState } from 'react';

import {
  getPhotoPreview,
  type PreviewReference,
} from '../../shared/commands/library';

interface PreviewImageProps {
  readonly reference: PreviewReference | null;
  readonly alt: string;
  readonly thumbnail?: boolean;
  readonly onStateChange?: (state: 'loading' | 'ready' | 'unavailable') => void;
}

export function PreviewImage({
  reference,
  alt,
  thumbnail = false,
  onStateChange,
}: PreviewImageProps) {
  const [source, setSource] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(
    reference?.state === 'Unavailable',
  );

  useEffect(() => {
    let active = true;
    setSource(null);
    if (reference === null || reference.state === 'Unavailable') {
      setUnavailable(true);
      onStateChange?.('unavailable');
      return;
    }
    setUnavailable(false);
    onStateChange?.('loading');
    void getPhotoPreview(reference)
      .then((value) => {
        if (active) {
          setSource(value);
          onStateChange?.('ready');
        }
      })
      .catch(() => {
        if (active) {
          setUnavailable(true);
          onStateChange?.('unavailable');
        }
      });
    return () => {
      active = false;
    };
  }, [onStateChange, reference]);

  if (unavailable)
    return (
      <span
        className={
          thumbnail ? 'preview-unavailable thumbnail' : 'preview-unavailable'
        }
        aria-label="预览不可用"
      >
        RAF
      </span>
    );
  if (source === null)
    return (
      <span
        className={thumbnail ? 'preview-loading thumbnail' : 'preview-loading'}
        aria-label="正在加载预览"
      >
        <i />
      </span>
    );
  return (
    <img
      className={thumbnail ? 'preview-image thumbnail' : 'preview-image'}
      src={source}
      alt={alt}
      draggable={false}
    />
  );
}
