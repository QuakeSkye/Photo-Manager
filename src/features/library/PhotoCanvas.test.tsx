import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type { PhotoSummary } from '../../shared/commands/library';
import { PhotoCanvas } from './PhotoCanvas';

const photo: PhotoSummary = {
  photoId: 'photo-1',
  photoVersion: 1,
  fileName: 'PORTRAIT.JPG',
  captureLocalKey: '2026-08-09T12:00:00',
  preview: {
    assetId: 'asset-1',
    assetVersion: 1,
    mediaKind: 'Jpg',
    state: 'JpegSource',
  },
  deletionState: null,
};

let resizeCallback: ResizeObserverCallback;

class ControlledResizeObserver implements ResizeObserver {
  constructor(callback: ResizeObserverCallback) {
    resizeCallback = callback;
  }
  observe() {}
  unobserve() {}
  disconnect() {}
}

function resize(width: number, height: number) {
  resizeCallback(
    [
      {
        contentRect: { width, height },
      } as ResizeObserverEntry,
    ],
    {} as ResizeObserver,
  );
}

function loadImage(width: number, height: number) {
  const image = screen.getByRole('img');
  Object.defineProperty(image, 'naturalWidth', {
    configurable: true,
    value: width,
  });
  Object.defineProperty(image, 'naturalHeight', {
    configurable: true,
    value: height,
  });
  fireEvent.load(image);
  return image;
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('PhotoCanvas', () => {
  test('fits a portrait, shows the exact mode, and gives actual-size wheel events to scrolling', async () => {
    const onWheel = vi.fn();
    const onZoomModeChange = vi.fn();
    render(
      <PhotoCanvas
        photo={photo}
        onWheel={onWheel}
        onZoomModeChange={onZoomModeChange}
      />,
    );
    const image = await screen.findByRole('img', { name: 'PORTRAIT.JPG' });
    resize(600, 400);
    loadImage(1000, 2000);

    expect(screen.getByLabelText('缩放状态')).toHaveTextContent('适合窗口 20%');
    expect(image).toHaveStyle({ width: '200px', height: '400px' });
    fireEvent.wheel(screen.getByLabelText('当前照片'), { deltaY: 40 });
    expect(onWheel).toHaveBeenCalledTimes(1);

    const viewport = document.querySelector('.photo-stage');
    expect(viewport).not.toBeNull();
    for (const [property, value] of [
      ['clientWidth', 600],
      ['clientHeight', 400],
      ['scrollWidth', 1000],
      ['scrollHeight', 2000],
    ] as const) {
      Object.defineProperty(viewport, property, {
        configurable: true,
        value,
      });
    }
    onWheel.mockClear();
    fireEvent.doubleClick(image);
    expect(screen.getByLabelText('缩放状态')).toHaveTextContent(
      '100% 预览尺寸',
    );
    expect(image).toHaveStyle({ width: '1000px', height: '2000px' });
    fireEvent.wheel(screen.getByLabelText('当前照片'), { deltaY: 40 });
    expect(onWheel).not.toHaveBeenCalled();
    expect(onZoomModeChange).toHaveBeenLastCalledWith('actual');
    await waitFor(() => {
      expect(viewport?.scrollLeft).toBe(200);
      expect(viewport?.scrollTop).toBe(800);
    });

    fireEvent.click(screen.getByLabelText('缩放状态'));
    expect(screen.getByLabelText('缩放状态')).toHaveTextContent('适合窗口 20%');
    expect(onZoomModeChange).toHaveBeenLastCalledWith('fit');
  });

  test('never enlarges small images and recomputes after a resize', async () => {
    render(<PhotoCanvas photo={photo} onWheel={vi.fn()} />);
    const image = await screen.findByRole('img');
    resize(1200, 800);
    loadImage(400, 300);
    expect(screen.getByLabelText('缩放状态')).toHaveTextContent(
      '适合窗口 100%',
    );
    expect(image).toHaveStyle({ width: '400px', height: '300px' });

    resize(200, 150);
    await waitFor(() => {
      expect(screen.getByLabelText('缩放状态')).toHaveTextContent(
        '适合窗口 50%',
      );
      expect(image).toHaveStyle({ width: '200px', height: '150px' });
    });
  });

  test('switching photos resets fit and scroll while empty, filtered-empty and unavailable states behave accurately', async () => {
    const onClearFilters = vi.fn();
    const onOpenFileLocation = vi.fn();
    const { rerender } = render(
      <PhotoCanvas photo={photo} onWheel={vi.fn()} />,
    );
    const image = await screen.findByRole('img');
    resize(600, 400);
    loadImage(1000, 2000);
    fireEvent.doubleClick(image);
    const viewport = document.querySelector('.photo-stage') as HTMLDivElement;
    viewport.scrollLeft = 120;
    viewport.scrollTop = 240;

    rerender(
      <PhotoCanvas
        photo={{
          ...photo,
          photoId: 'photo-2',
          photoVersion: 2,
          fileName: 'NEXT.JPG',
          preview: { ...photo.preview!, assetId: 'asset-2' },
        }}
        onWheel={vi.fn()}
      />,
    );
    expect(screen.getByLabelText('当前照片')).toHaveAttribute(
      'data-zoom-mode',
      'fit',
    );
    expect(viewport.scrollLeft).toBe(0);
    expect(viewport.scrollTop).toBe(0);
    expect(screen.queryByLabelText('缩放状态')).not.toBeInTheDocument();

    rerender(
      <PhotoCanvas
        photo={{
          ...photo,
          photoId: 'photo-3',
          preview: {
            assetId: 'asset-3',
            assetVersion: 1,
            mediaKind: 'Raf',
            state: 'Unavailable',
          },
        }}
        onWheel={vi.fn()}
        canOpenFileLocation
        onOpenFileLocation={onOpenFileLocation}
      />,
    );
    expect(await screen.findByLabelText('预览不可用')).toBeInTheDocument();
    expect(
      screen.getByText('暂时无法生成这张照片的预览。'),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: '重试预览' }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '打开当前文件位置' }));
    expect(onOpenFileLocation).toHaveBeenCalledTimes(1);
    expect(screen.queryByLabelText('缩放状态')).not.toBeInTheDocument();

    rerender(
      <PhotoCanvas
        photo={null}
        onWheel={vi.fn()}
        isFilteredEmpty
        onClearFilters={onClearFilters}
      />,
    );
    expect(screen.getByText('当前筛选没有结果')).toBeInTheDocument();
    expect(screen.queryByText('图库中还没有照片')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '清除全部筛选条件' }));
    expect(onClearFilters).toHaveBeenCalledTimes(1);

    rerender(<PhotoCanvas photo={null} onWheel={vi.fn()} />);
    expect(screen.getByText('图库中还没有照片')).toBeInTheDocument();
    expect(screen.queryByLabelText('缩放状态')).not.toBeInTheDocument();
  });
});
