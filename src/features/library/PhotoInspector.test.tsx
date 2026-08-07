import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import type { ComponentProps } from 'react';

import type { PhotoDetail } from '../../shared/commands/library';
import { PhotoInspector } from './PhotoInspector';

const detail: PhotoDetail = {
  photoId: 'photo-1',
  photoVersion: 1,
  fileName: 'DSCF00001.JPG',
  captureLocalKey: '2026-07-30T14:25:10',
  fileTimeMs: Date.UTC(2026, 6, 30, 14, 26, 0),
  cameraModel: 'FUJIFILM X-T5',
  cameraSerial: '3A001234',
  jpeg: {
    assetId: 'jpeg-1',
    assetVersion: 1,
    sizeBytes: 18_420_000,
    fileTimeMs: Date.UTC(2026, 6, 30, 14, 26, 0),
    previewState: 'JpegSource',
  },
  raf: {
    assetId: 'raf-1',
    assetVersion: 1,
    sizeBytes: 82_600_000,
    fileTimeMs: Date.UTC(2026, 6, 30, 14, 26, 0),
    previewState: 'RafEmbeddedJpeg',
  },
  totalSizeBytes: 101_020_000,
  preview: null,
};

const inspectorProps: ComponentProps<typeof PhotoInspector> = {
  detail,
  loading: false,
  error: null,
  marked: false,
  mutationPending: false,
  toolStatus: null,
  externalActionPending: false,
  externalError: null,
  onOpenPhotoshop: vi.fn(),
  onOpenFileLocation: vi.fn(),
  onOpenSettings: vi.fn(),
  onToggleDeletion: vi.fn(),
};

function inspector(
  overrides: Partial<ComponentProps<typeof PhotoInspector>> = {},
) {
  return <PhotoInspector {...inspectorProps} {...overrides} />;
}

function renderInspector() {
  return render(inspector());
}

describe('PhotoInspector', () => {
  test('keeps both metadata groups expanded by default and collapses them independently', () => {
    const { rerender } = renderInspector();

    const timeToggle = screen.getByRole('button', { name: /时间与文件/ });
    const parameterToggle = screen.getByRole('button', {
      name: /照片参数.*FUJIFILM X-T5/,
    });

    expect(timeToggle).toHaveAttribute('aria-expanded', 'true');
    expect(parameterToggle).toHaveAttribute('aria-expanded', 'true');
    expect(
      document.getElementById(timeToggle.getAttribute('aria-controls') ?? ''),
    ).not.toBeNull();
    expect(
      document.getElementById(
        parameterToggle.getAttribute('aria-controls') ?? '',
      ),
    ).not.toBeNull();

    fireEvent.click(timeToggle);
    expect(timeToggle).toHaveAttribute('aria-expanded', 'false');
    expect(parameterToggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('拍摄日期')).not.toBeVisible();
    expect(screen.getByText('相机')).toBeInTheDocument();

    fireEvent.click(parameterToggle);
    expect(parameterToggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.getByText('相机')).not.toBeVisible();

    rerender(inspector({ detail: null, loading: true }));
    rerender(inspector());
    expect(screen.getByRole('button', { name: /时间与文件/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    expect(
      screen.getByRole('button', { name: /照片参数.*FUJIFILM X-T5/ }),
    ).toHaveAttribute('aria-expanded', 'false');

    expect(
      screen.getByRole('button', { name: '用 Photoshop 打开 RAF' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: '打开当前文件位置' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /标记待删除/ }),
    ).toBeInTheDocument();
  });
});
