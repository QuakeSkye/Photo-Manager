import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import type { PhotoSummary } from '../../shared/commands/library';
import { VirtualFilmstrip } from './VirtualFilmstrip';

afterEach(cleanup);

function photos(count: number): PhotoSummary[] {
  return Array.from({ length: count }, (_, index) => ({
    photoId: `photo-${index}`,
    photoVersion: 1,
    fileName: `DSCF${String(index + 1).padStart(5, '0')}.JPG`,
    captureLocalKey: `2026-07-30T12:${String(index % 60).padStart(2, '0')}:00`,
    preview: null,
    deletionState: null,
  }));
}

describe('VirtualFilmstrip', () => {
  test('renders a bounded window and materializes a far-away selection', async () => {
    const items = photos(1_000);
    const onSelectIndex = vi.fn();
    const { container, rerender } = render(
      <VirtualFilmstrip
        photos={items}
        selectedPhotoId={items[0].photoId}
        selectedIndex={0}
        onSelectIndex={onSelectIndex}
        onWheel={vi.fn()}
      />,
    );

    expect(screen.getAllByRole('button').length).toBeLessThan(20);
    expect(
      screen.getByRole('complementary', { name: '照片缩略图' }),
    ).toHaveStyle({
      '--filmstrip-item-height': '106px',
      '--filmstrip-row-height': '112px',
    });
    expect(container.querySelector('.filmstrip-track')).toHaveStyle({
      height: '112000px',
    });

    const viewport = container.querySelector('.filmstrip-viewport');
    expect(viewport).not.toBeNull();
    if (viewport === null) return;
    Object.defineProperty(viewport, 'scrollTop', {
      configurable: true,
      value: 11_200,
      writable: true,
    });
    fireEvent.scroll(viewport);
    expect(
      await screen.findByRole('button', { name: '第 101 张：DSCF00101.JPG' }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole('button').length).toBeLessThan(20);

    rerender(
      <VirtualFilmstrip
        photos={items}
        selectedPhotoId={items[999].photoId}
        selectedIndex={999}
        onSelectIndex={onSelectIndex}
        onWheel={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: '第 1000 张：DSCF01000.JPG' }),
      ).toHaveAttribute('aria-current', 'true'),
    );
    expect(screen.getAllByRole('button').length).toBeLessThan(20);
  });

  test('renders filter controls, distinct D / ! badges, and zero-match empty state', () => {
    const onDateFilterChange = vi.fn();
    const onClearFilters = vi.fn();
    const items: PhotoSummary[] = [
      {
        photoId: 'marked-1',
        photoVersion: 1,
        fileName: 'DSCF00001.JPG',
        captureLocalKey: '2026-07-30T12:00:00',
        preview: null,
        deletionState: 'Marked',
      },
      {
        photoId: 'problem-1',
        photoVersion: 1,
        fileName: 'DSCF00002.RAF',
        captureLocalKey: null,
        preview: null,
        deletionState: 'Problem',
      },
    ];

    const { rerender } = render(
      <VirtualFilmstrip
        photos={items}
        selectedPhotoId="marked-1"
        selectedIndex={0}
        onSelectIndex={vi.fn()}
        onWheel={vi.fn()}
        filterControls={{
          dateFilter: 'all',
          dateOptions: [
            { value: 'all', label: '全部日期', count: 12 },
            { value: '2026-07-30', label: '2026-07-30', count: 11 },
            { value: 'unknown', label: '日期未知', count: 1 },
          ],
          totalPhotoCount: 12,
          hasActiveFilter: true,
          onDateFilterChange,
          onClearFilters,
        }}
      />,
    );

    expect(screen.getByLabelText('已标记待删除')).toHaveTextContent('D 待删');
    expect(screen.getByLabelText('待删除标记需要处理')).toHaveTextContent('!');
    expect(screen.getByText('结果 2 / 12')).toBeInTheDocument();

    expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '清除筛选' }));
    expect(onClearFilters).toHaveBeenCalledTimes(1);

    rerender(
      <VirtualFilmstrip
        photos={[]}
        selectedPhotoId={null}
        selectedIndex={-1}
        onSelectIndex={vi.fn()}
        onWheel={vi.fn()}
        filterControls={{
          dateFilter: 'all',
          dateOptions: [{ value: 'all', label: '全部日期', count: 12 }],
          totalPhotoCount: 12,
          hasActiveFilter: true,
          onDateFilterChange,
          onClearFilters,
        }}
      />,
    );

    expect(screen.getByText('没有匹配当前筛选的照片')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '清除全部筛选' }));
    expect(onClearFilters).toHaveBeenCalledTimes(2);
  });
});
