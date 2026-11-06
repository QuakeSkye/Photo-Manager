import {
  useEffect,
  useState,
  type CSSProperties,
  type MouseEvent,
} from 'react';

import {
  getPhotoPreview,
  type PreviewReference,
} from '../../shared/commands/library';

interface PreviewImageProps {
  readonly reference: PreviewReference | null;
  readonly alt: string;
  readonly thumbnail?: boolean;
  readonly retryToken?: number;
  readonly onStateChange?: (state: 'loading' | 'ready' | 'unavailable') => void;
  readonly onImageLoad?: (size: {
    readonly width: number;
    readonly height: number;
  }) => void;
  readonly onImageDoubleClick?: (event: MouseEvent<HTMLImageElement>) => void;
  readonly imageStyle?: CSSProperties;
}

export function PreviewImage({
  reference,
  alt,
  thumbnail = false,
  retryToken = 0,
  onStateChange,
  onImageLoad,
  onImageDoubleClick,
  imageStyle,
}: PreviewImageProps) {
  const [source, setSource] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(
    reference === null || reference.state === 'Unavailable',
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
  }, [onStateChange, reference, retryToken]);

  if (unavailable)
    return (
      <span
        className={
          thumbnail ? 'preview-unavailable thumbnail' : 'preview-unavailable'
        }
        aria-label="预览不可用"
      >
        {thumbnail ? '无预览' : ''}
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
      style={imageStyle}
      onLoad={(event) => {
        onImageLoad?.({
          width: event.currentTarget.naturalWidth,
          height: event.currentTarget.naturalHeight,
        });
      }}
      onDoubleClick={onImageDoubleClick}
    />
  );
}
