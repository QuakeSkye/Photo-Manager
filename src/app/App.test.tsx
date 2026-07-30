import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test } from 'vitest';

import { App } from './App';

beforeEach(() => {
  window.history.replaceState(null, '', '/');
});

afterEach(() => cleanup());

describe('阶段 2 只读图库', () => {
  test('renders the white immersive library without future-stage controls', async () => {
    render(<App />);

    expect(
      await screen.findByRole('navigation', { name: '照片视图' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '图库' })).toBeInTheDocument();
    expect(screen.queryByText(/待删除/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /导入|Photoshop|文件位置|标记/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByText('只读浏览')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: '照片排序' })).toHaveValue(
      'captureDesc',
    );
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('uses one selection state for arrow keys and ignores D in read-only phase', async () => {
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(
      await screen.findByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(window, { key: 'd' });
    expect(
      screen.getByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    expect(screen.queryByText(/待删除/)).not.toBeInTheDocument();
  });

  test('uses the same bounded selection for wheel and keyboard navigation', async () => {
    render(<App />);
    const canvas = await screen.findByLabelText('当前照片');

    fireEvent.wheel(canvas, { deltaY: 40 });
    expect(
      await screen.findByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    for (let index = 0; index < 20; index += 1)
      fireEvent.keyDown(window, { key: 'ArrowLeft' });
    expect(
      screen.getByRole('button', { name: '第 1 张：DSCF1042.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    fireEvent.keyDown(window, { key: 'ArrowLeft' });
    expect(
      screen.getByRole('button', { name: '第 1 张：DSCF1042.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    for (let index = 0; index < 20; index += 1)
      fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(
      screen.getByRole('button', {
        name: /第 12 张：一段很长的文件名/,
      }),
    ).toHaveAttribute('aria-current', 'true');
    fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(
      screen.getByRole('button', {
        name: /第 12 张：一段很长的文件名/,
      }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('switches oldest/newest order while keeping unknown dates last', async () => {
    render(<App />);
    const sort = await screen.findByRole('combobox', { name: '照片排序' });

    fireEvent.change(sort, { target: { value: 'captureAsc' } });

    await waitFor(() => expect(sort).toHaveValue('captureAsc'));
    expect(
      await screen.findByRole('button', { name: '第 1 张：DSCF0942.RAF' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: '第 11 张：DSCF1042.JPG' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', {
        name: /第 12 张：一段很长的文件名/,
      }),
    ).toBeInTheDocument();
  });

  test('does not steal arrow keys from focused controls', async () => {
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    const settings = screen.getByRole('button', { name: /设置/ });
    settings.focus();

    fireEvent.keyDown(settings, { key: 'ArrowRight' });

    expect(settings).toHaveFocus();
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('traps modal focus, supports Escape, and restores the trigger', async () => {
    window.history.replaceState(null, '', '/?scenario=scan-running');
    render(<App />);
    const trigger = await screen.findByRole('button', { name: '取消扫描…' });
    fireEvent.click(trigger);

    const dialog = screen.getByRole('dialog', { name: '取消扫描？' });
    const confirm = screen.getByRole('button', { name: '确认取消扫描' });
    const cancel = screen.getByRole('button', { name: '取消' });
    expect(confirm).toHaveFocus();
    fireEvent.keyDown(dialog, { key: 'Tab' });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(dialog, { key: 'Tab', shiftKey: true });
    expect(confirm).toHaveFocus();

    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  test('keeps unknown capture dates last and labels file time separately', async () => {
    window.history.replaceState(null, '', '/?scenario=unknown');
    render(<App />);

    expect(await screen.findByText('拍摄时间未知')).toBeInTheDocument();
    expect(screen.getByText('文件时间')).toBeInTheDocument();
    expect(
      screen.getByText('文件时间仅供参考，不参与照片排序。'),
    ).toBeInTheDocument();
    expect(screen.getByLabelText('当前照片日期')).toHaveTextContent('日期未知');
  });

  test('distinguishes JPG-only, RAF-only and paired Photos', async () => {
    render(<App />);

    expect(await screen.findByText('JPG ✓')).toBeInTheDocument();
    expect(screen.getByText('RAF ×')).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole('button', { name: '第 2 张：DSCF1039.RAF' }),
    );
    await waitFor(() => expect(screen.getByText('JPG ×')).toBeInTheDocument());
    expect(screen.getByText('RAF ✓')).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole('button', { name: '第 1 张：DSCF1042.JPG' }),
    );
    await waitFor(() => expect(screen.getByText('JPG ✓')).toBeInTheDocument());
    expect(screen.getByText('RAF ✓')).toBeInTheDocument();
  });

  test('shows explicit empty, offline and preview-unavailable states', async () => {
    window.history.replaceState(null, '', '/?scenario=empty');
    const rendered = render(<App />);
    expect(await screen.findByText('图库中还没有照片')).toBeInTheDocument();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();

    rendered.unmount();
    window.history.replaceState(null, '', '/?scenario=offline');
    render(<App />);
    expect(await screen.findByText('图库当前离线')).toBeInTheDocument();

    cleanup();
    window.history.replaceState(null, '', '/');
    render(<App />);
    fireEvent.click(
      await screen.findByRole('button', { name: '第 5 张：DSCF1017.RAF' }),
    );
    expect(
      (await screen.findAllByLabelText('预览不可用')).length,
    ).toBeGreaterThan(0);
    expect(screen.getByLabelText('当前照片')).toBeInTheDocument();
  });

  test('guards a stale pending-delete route before rendering an empty review page', async () => {
    window.history.replaceState(null, '', '/#/pending-delete');
    render(<App />);

    expect(
      await screen.findByRole('button', { name: '图库' }),
    ).toBeInTheDocument();
    expect(window.location.hash).toBe('#/library');
    expect(screen.queryByText(/待删除 0/)).not.toBeInTheDocument();
  });

  test('settings is a tool page and restores focus and the current photo', async () => {
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    const settings = screen.getByRole('button', { name: /设置/ });
    fireEvent.click(settings);

    expect(
      await screen.findByRole('heading', { name: 'Fuji 图库根目录' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /照片文件夹/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /操作记录/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('navigation', { name: '照片视图' }).children,
    ).toHaveLength(1);

    fireEvent.click(screen.getAllByRole('button', { name: '返回图库' })[0]);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /设置/ })).toHaveFocus(),
    );
  });
});

describe('Root 与扫描视觉状态', () => {
  test('uses the photo-folder page as the first-launch gate', async () => {
    window.history.replaceState(null, '', '/?scenario=unbound');
    render(<App />);

    expect(
      await screen.findByRole('heading', { name: '选择照片文件夹' }),
    ).toBeInTheDocument();
    expect(screen.getByText('尚未选择')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: '返回图库' }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText('图库中还没有照片')).not.toBeInTheDocument();
  });

  test.each([
    ['running', '正在扫描照片', '取消扫描…'],
    ['cancelling', '正在安全停止扫描', '正在停止…'],
    ['cancelled', '扫描已取消', '继续扫描'],
    ['failed', '扫描未完成', '继续扫描'],
    ['completed', '扫描完成', '进入图库'],
  ] as const)(
    'shows full-scan %s state without a fake gallery',
    async (scenario, heading, action) => {
      window.history.replaceState(null, '', `/?scenario=scan-${scenario}`);
      render(<App />);

      expect(
        await screen.findByRole('heading', { name: heading }),
      ).toBeInTheDocument();
      expect(screen.getByRole('button', { name: action })).toBeInTheDocument();
      expect(screen.getByText('照片文件保持不变')).toBeInTheDocument();
      expect(screen.queryByLabelText('当前照片')).not.toBeInTheDocument();
    },
  );

  test('keeps incremental scan visible while allowing an immediate return to the library', async () => {
    window.history.replaceState(null, '', '/?scenario=scan-incremental');
    render(<App />);

    expect(await screen.findByText('正在更新图库')).toBeInTheDocument();
    expect(screen.getByLabelText('当前照片')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /查看进度/ }),
    ).toBeInTheDocument();
  });

  test.each([
    ['cancelling', '正在停止图库更新'],
    ['cancelled', '图库更新已取消'],
    ['failed', '图库更新未完成'],
  ] as const)(
    'keeps incremental %s status explicit without blocking the gallery',
    async (phase, message) => {
      window.history.replaceState(
        null,
        '',
        `/?scenario=scan-${phase === 'cancelling' ? 'incremental-cancelling' : `incremental-${phase}`}`,
      );
      render(<App />);

      expect(await screen.findByText(message)).toBeInTheDocument();
      expect(screen.getByLabelText('当前照片')).toBeInTheDocument();
    },
  );

  test('wraps long paths and filenames without exposing internal identifiers', async () => {
    window.history.replaceState(null, '', '/?scenario=long');
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    fireEvent.click(screen.getByRole('button', { name: /设置/ }));

    const path = await screen.findByTitle(/FUJIFILM-X-T5-ARCHIVE/);
    expect(path).toHaveClass('long-path');
    expect(document.body.textContent).not.toMatch(
      /RootId|LibraryId|PhotoId|AssetId|volume GUID|文件 ID|hash/i,
    );
  });
});
