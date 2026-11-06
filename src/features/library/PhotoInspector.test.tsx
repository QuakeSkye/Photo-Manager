import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import type { ComponentProps } from 'react';

import type { PhotoDetail } from '../../shared/commands/library';
import { PhotoInspector } from './PhotoInspector';

afterEach(cleanup);

const detail: PhotoDetail = {
  photoId: 'photo-1',
  photoVersion: 1,
  fileName: 'DSCF00001.JPG',
  captureLocalKey: '2026-07-30T14:25:10',
  fileTimeMs: Date.UTC(2026, 6, 30, 14, 26, 0),
  cameraModel: 'FUJIFILM X-T5',
  cameraSerial: '3A001234',
  lensModel: 'XF35mmF1.4 R',
  fNumber: { numerator: 14, denominator: 5 },
  exposureTime: { numerator: 1, denominator: 500 },
  iso: 400,
  focalLength: { numerator: 35, denominator: 1 },
  pixelWidth: 6240,
  pixelHeight: 4160,
  filmSimulation: { key: 'classic-chrome', code: 0x600 },
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
  onUndoDeletion: vi.fn(),
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
  test('renders 3-column exposure strip and grouped capture parameters including film simulation', () => {
    const { container } = renderInspector();

    const exposureStrip = screen.getByLabelText('核心曝光参数');
    expect(within(exposureStrip).getByText('光圈')).toBeInTheDocument();
    expect(within(exposureStrip).getByText('F2.8')).toBeInTheDocument();
    expect(within(exposureStrip).getByText('快门')).toBeInTheDocument();
    expect(within(exposureStrip).getByText('1/500 秒')).toBeInTheDocument();
    expect(within(exposureStrip).getByText('ISO')).toBeInTheDocument();
    expect(within(exposureStrip).getByText('ISO 400')).toBeInTheDocument();

    const section = screen.getByText('拍摄参数').closest('section');
    expect(section).not.toBeNull();
    expect(
      [...(section?.querySelectorAll('dt') ?? [])].map(
        (element) => element.textContent,
      ),
    ).toEqual(['相机', '镜头', '焦距', '尺寸', '胶片模拟']);
    for (const value of [
      'FUJIFILM X-T5',
      'XF35mmF1.4 R',
      'F2.8',
      '1/500 秒',
      'ISO 400',
      '35 mm',
      '6240 × 4160',
      'Classic Chrome',
    ]) {
      expect(container).toHaveTextContent(value);
    }
  });

  test('renders explicit format copy, RAF embedded preview note, and unknown capture time state', () => {
    render(
      inspector({
        marked: true,
        deletionState: 'Marked',
        detail: {
          ...detail,
          captureLocalKey: null,
          jpeg: null,
          preview: {
            assetId: 'raf-1',
            assetVersion: 1,
            mediaKind: 'Raf',
            state: 'RafEmbeddedJpeg',
          },
        },
      }),
    );

    expect(screen.getByText('未找到 JPG')).toBeInTheDocument();
    expect(screen.getByText('RAF ✓')).toBeInTheDocument();
    expect(screen.getByText('当前为 RAF 内嵌预览')).toBeInTheDocument();
    expect(screen.getByLabelText('当前待删除状态')).toHaveTextContent(
      'D 已标记待删除',
    );
    expect(screen.getAllByText('未知')).toHaveLength(2);
    expect(screen.getByText('2026-07-30 14:26:00')).toBeInTheDocument();
    expect(
      screen.getByText(/参考文件时间来自文件系统记录/),
    ).toBeInTheDocument();
  });

  test('formats long exposure and unknown film codes while keeping missing fields local', () => {
    render(
      inspector({
        detail: {
          ...detail,
          lensModel: null,
          exposureTime: { numerator: 3, denominator: 2 },
          focalLength: { numerator: 105, denominator: 2 },
          filmSimulation: { key: 'unknown', code: 0xdead },
        },
      }),
    );

    expect(screen.getAllByText('未记录')).toHaveLength(1);
    expect(screen.getByText('1.5 秒')).toBeInTheDocument();
    expect(screen.getByText('52.5 mm')).toBeInTheDocument();
    expect(screen.getByText('未知模式（0xDEAD）')).toBeInTheDocument();
    expect(screen.getByText('ISO 400')).toBeInTheDocument();
  });

  test('keeps both metadata groups expanded by default and collapses them independently', () => {
    const { rerender } = renderInspector();

    const timeToggle = screen.getByRole('button', { name: /时间与文件/ });
    const parameterToggle = screen.getByRole('button', {
      name: /拍摄参数.*FUJIFILM X-T5/,
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
      screen.getByRole('button', { name: /拍摄参数.*FUJIFILM X-T5/ }),
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
    expect(screen.getByRole('button', { name: /撤销/ })).toBeInTheDocument();
  });
});
