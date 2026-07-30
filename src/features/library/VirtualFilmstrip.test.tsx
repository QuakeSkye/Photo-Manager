import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import type { PhotoSummary } from '../../shared/commands/library';
import { VirtualFilmstrip } from './VirtualFilmstrip';

function photos(count: number): PhotoSummary[] {
  return Array.from({ length: count }, (_, index) => ({
    photoId: `photo-${index}`,
    photoVersion: 1,
    fileName: `DSCF${String(index + 1).padStart(5, '0')}.JPG`,
    captureLocalKey: `2026-07-30T12:${String(index % 60).padStart(2, '0')}:00`,
    preview: null,
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
    expect(container.querySelector('.filmstrip-track')).toHaveStyle({
      height: '82000px',
    });

    const viewport = container.querySelector('.filmstrip-viewport');
    expect(viewport).not.toBeNull();
    if (viewport === null) return;
    Object.defineProperty(viewport, 'scrollTop', {
      configurable: true,
      value: 8_200,
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
});
