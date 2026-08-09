import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test } from 'vitest';

import {
  resetDemoDeletionState,
  simulateDemoRestart,
} from '../shared/commands/library';
import { resetDemoSettings } from '../shared/commands/settings';
import { App } from './App';

beforeEach(() => {
  resetDemoDeletionState();
  resetDemoSettings();
  window.history.replaceState(null, '', '/');
});

afterEach(() => cleanup());

describe('阶段 3 图库与既有只读能力', () => {
  test('renders marking controls while keeping the conditional entry hidden at zero', async () => {
    render(<App />);

    expect(
      await screen.findByRole('navigation', { name: '照片视图' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '图库' })).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /^待删除 \d+$/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /用 Photoshop 打开 RAF/ }),
    ).toBeDisabled();
    expect(
      screen.getByRole('button', { name: /打开当前文件位置/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /标记待删除/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: '照片排序' })).toHaveValue(
      'captureDesc',
    );
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('uses one selection state for arrow keys', async () => {
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(
      await screen.findByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
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

  test('routes wheel input to image scrolling at 100% and keeps filmstrip navigation', async () => {
    render(<App />);
    const canvas = await screen.findByLabelText('当前照片');
    const image = await screen.findByRole('img', { name: 'DSCF1031.JPG' });
    Object.defineProperties(image, {
      naturalWidth: { configurable: true, value: 4000 },
      naturalHeight: { configurable: true, value: 6000 },
    });
    fireEvent.load(image);
    fireEvent.doubleClick(image);

    expect(canvas).toHaveAttribute('data-zoom-mode', 'actual');
    fireEvent.wheel(canvas, { deltaY: 40 });
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    fireEvent.wheel(screen.getByLabelText('照片详情'), { deltaY: 40 });
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    const filmstrip = screen
      .getByLabelText('照片缩略图')
      .querySelector('.filmstrip-viewport');
    expect(filmstrip).not.toBeNull();
    fireEvent.wheel(filmstrip as Element, { deltaY: 40 });
    expect(
      await screen.findByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('switches oldest/newest order while keeping unknown dates last', async () => {
    render(<App />);
    const sort = await screen.findByRole('combobox', { name: '照片排序' });

    fireEvent.change(sort, { target: { value: 'captureAsc' } });

    await waitFor(() => expect(sort).toHaveValue('captureAsc'));
    for (let index = 0; index < 20; index += 1)
      fireEvent.keyDown(window, { key: 'ArrowLeft' });
    expect(
      await screen.findByRole('button', { name: '第 1 张：DSCF0942.RAF' }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('does not steal navigation or D/R from focused controls', async () => {
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    const settings = screen.getByRole('button', { name: /设置/ });
    settings.focus();

    fireEvent.keyDown(settings, { key: 'ArrowRight' });
    fireEvent.keyDown(settings, { key: 'D' });
    fireEvent.keyDown(settings, { key: 'R' });

    expect(settings).toHaveFocus();
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    expect(
      screen.queryByRole('button', { name: /^待删除 \d+$/ }),
    ).not.toBeInTheDocument();
  });

  test('keeps D/R and arrow navigation active after a filmstrip click', async () => {
    render(<App />);
    const first = await screen.findByRole('button', {
      name: '第 1 张：DSCF1042.JPG',
    });
    fireEvent.click(first);
    first.focus();
    expect(first).toHaveFocus();

    fireEvent.keyDown(first, { key: 'ArrowRight' });
    expect(
      screen.getByRole('button', { name: '第 2 张：DSCF1039.RAF' }),
    ).toHaveAttribute('aria-current', 'true');
    fireEvent.keyDown(first, { key: 'ArrowLeft' });
    expect(first).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(first, { key: 'D' });
    await within(
      await screen.findByRole('navigation', { name: '照片视图' }),
    ).findByRole('button', { name: /^待删除 1$/ });
    expect(
      screen.getByRole('button', { name: '第 2 张：DSCF1039.RAF' }),
    ).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(first, { key: 'R' });
    await waitFor(() =>
      expect(
        within(
          screen.getByRole('navigation', { name: '照片视图' }),
        ).queryByRole('button', { name: /^待删除 \d+$/ }),
      ).not.toBeInTheDocument(),
    );
    expect(first).toHaveAttribute('aria-current', 'true');
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
    fireEvent.keyDown(dialog, { key: 'D' });
    fireEvent.keyDown(dialog, { key: 'R' });
    expect(dialog).toBeInTheDocument();
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

describe('阶段 3 待删除复核闭环', () => {
  test('D creates the 0→1 entry, advances once, and ignores repeat input', async () => {
    render(<App />);
    expect(
      await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(window, { key: 'd' });
    fireEvent.keyDown(window, { key: 'd', repeat: true });
    fireEvent.click(
      screen.getByRole('button', { name: '第 6 张：DSCF1009.JPG' }),
    );

    expect(
      await within(
        await screen.findByRole('navigation', { name: '照片视图' }),
      ).findByRole('button', { name: /^待删除 1$/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/library');
  });

  test('R undoes multiple current-session marks in reverse order', async () => {
    render(<App />);
    await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' });

    fireEvent.keyDown(window, { key: 'D' });
    await within(
      screen.getByRole('navigation', { name: '照片视图' }),
    ).findByRole('button', { name: /^待删除 1$/ });
    fireEvent.keyDown(window, { key: 'D' });
    await within(
      screen.getByRole('navigation', { name: '照片视图' }),
    ).findByRole('button', { name: /^待删除 2$/ });

    fireEvent.keyDown(window, { key: 'R' });
    fireEvent.keyDown(window, { key: 'R', repeat: true });
    await within(
      screen.getByRole('navigation', { name: '照片视图' }),
    ).findByRole('button', { name: /^待删除 1$/ });
    expect(
      screen.getByRole('button', { name: '第 4 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');

    fireEvent.keyDown(window, { key: 'R' });
    await waitFor(() =>
      expect(
        within(
          screen.getByRole('navigation', { name: '照片视图' }),
        ).queryByRole('button', { name: /^待删除 \d+$/ }),
      ).not.toBeInTheDocument(),
    );
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
  });

  test('cancelling a non-final review item selects the next item at the same index', async () => {
    render(<App />);
    await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' });
    fireEvent.keyDown(window, { key: 'D' });
    await waitFor(() =>
      expect(
        within(screen.getByRole('navigation', { name: '照片视图' })).getByRole(
          'button',
          { name: /^待删除 1$/ },
        ),
      ).toBeInTheDocument(),
    );
    fireEvent.keyDown(window, { key: 'D' });
    const pending = await within(
      screen.getByRole('navigation', { name: '照片视图' }),
    ).findByRole('button', { name: /^待删除 2$/ });
    fireEvent.click(pending);
    await screen.findByText('待删除 · 第 1 / 2 张');

    fireEvent.click(screen.getByRole('button', { name: /取消当前待删除/ }));

    expect(await screen.findByText('待删除 · 第 1 / 1 张')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: '第 1 张：DSCF1028.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/pending-delete');
  });

  test('failed and offline additions keep the photo and zero summary unchanged', async () => {
    window.history.replaceState(null, '', '/?scenario=mark-failure');
    const failed = render(<App />);
    await screen.findByRole('button', { name: '第 3 张：DSCF1031.JPG' });
    fireEvent.keyDown(window, { key: 'D' });
    expect(await screen.findByRole('alert')).toHaveTextContent(
      '待删除标记未能可靠保存',
    );
    expect(
      screen.getByRole('button', { name: '第 3 张：DSCF1031.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    expect(
      within(screen.getByRole('navigation', { name: '照片视图' })).queryByRole(
        'button',
        { name: /^待删除 \d+$/ },
      ),
    ).not.toBeInTheDocument();

    failed.unmount();
    resetDemoDeletionState();
    window.history.replaceState(null, '', '/?scenario=offline');
    render(<App />);
    await screen.findByText('图库当前离线');
    fireEvent.keyDown(window, { key: 'D' });
    expect(await screen.findByRole('alert')).toHaveTextContent(
      '照片文件夹离线时不能新增或恢复待删除标记',
    );
    expect(
      within(screen.getByRole('navigation', { name: '照片视图' })).queryByRole(
        'button',
        { name: /^待删除 \d+$/ },
      ),
    ).not.toBeInTheDocument();
  });

  test('last cancellation applies the 1→0 route and returns to the cancelled photo', async () => {
    window.history.replaceState(null, '', '/?scenario=pending');
    render(<App />);
    fireEvent.click(
      await within(
        await screen.findByRole('navigation', { name: '照片视图' }),
      ).findByRole('button', { name: /^待删除 1$/ }),
    );

    expect(await screen.findByText('待删除 · 第 1 / 1 张')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /取消当前待删除/ }));

    await waitFor(() => expect(window.location.hash).toBe('#/library'));
    expect(
      screen.getByRole('button', { name: '第 1 张：DSCF1042.JPG' }),
    ).toHaveAttribute('aria-current', 'true');
    expect(
      within(screen.getByRole('navigation', { name: '照片视图' })).queryByRole(
        'button',
        { name: /^待删除 \d+$/ },
      ),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/待删除 · 第/)).not.toBeInTheDocument();
  });

  test('derives problem review actions while execute-delete stays gated', async () => {
    window.history.replaceState(null, '', '/?scenario=problem');
    render(<App />);
    fireEvent.click(
      await within(
        await screen.findByRole('navigation', { name: '照片视图' }),
      ).findByRole('button', { name: /待删除 1/ }),
    );

    expect(await screen.findByText('需要重新确认')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /取消失效标记/ })).toBeEnabled();
    expect(screen.getByRole('button', { name: '重新扫描确认' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '执行删除…' })).toBeDisabled();
    expect(screen.getByText(/身份已验证的 NTFS Fuji 图库/)).toBeInTheDocument();
    expect(screen.queryByText(/Photoshop|文件位置/)).not.toBeInTheDocument();
  });

  test('restart restores the entry but intentionally clears R history', async () => {
    window.history.replaceState(null, '', '/?scenario=recovery');
    const first = render(<App />);
    expect(
      await within(
        await screen.findByRole('navigation', { name: '照片视图' }),
      ).findByRole('button', { name: /^待删除 1$/ }),
    ).toBeInTheDocument();

    first.unmount();
    simulateDemoRestart();
    render(<App />);
    await within(
      await screen.findByRole('navigation', { name: '照片视图' }),
    ).findByRole('button', { name: /^待删除 1$/ });
    fireEvent.keyDown(window, { key: 'R' });

    expect(await screen.findByRole('alert')).toHaveTextContent(
      '当前会话没有可撤销的待删除标记操作',
    );
    expect(
      within(screen.getByRole('navigation', { name: '照片视图' })).getByRole(
        'button',
        { name: /^待删除 1$/ },
      ),
    ).toBeInTheDocument();
    expect(window.location.hash).toBe('#/library');
  });
});

describe('阶段 7 Windows 回收站确认与归零', () => {
  test('shows logical, physical and capacity totals before recycle and returns to library after success', async () => {
    window.history.replaceState(null, '', '/?scenario=pending');
    render(<App />);
    fireEvent.click(
      await within(
        await screen.findByRole('navigation', { name: '照片视图' }),
      ).findByRole('button', { name: /^待删除 1$/ }),
    );
    await screen.findByText('待删除 · 第 1 / 1 张');

    fireEvent.click(screen.getByRole('button', { name: '执行删除…' }));
    const dialog = await screen.findByRole('dialog', {
      name: '确认移入 Windows 回收站',
    });
    expect(within(dialog).getByText('逻辑照片')).toBeInTheDocument();
    expect(within(dialog).getByText('物理 JPG / RAF 文件')).toBeInTheDocument();
    expect(within(dialog).getByText('合计容量')).toBeInTheDocument();
    expect(
      within(dialog).getByText(/只能通过 Windows 回收站恢复/),
    ).toBeInTheDocument();
    expect(within(dialog).getByText(/不会永久删除/)).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole('button', { name: '移入回收站' }));
    await waitFor(() => expect(window.location.hash).toBe('#/library'));
    expect(
      within(screen.getByRole('navigation', { name: '照片视图' })).queryByRole(
        'button',
        { name: /^待删除 \d+$/ },
      ),
    ).not.toBeInTheDocument();
  });
});

describe('阶段 4 Photoshop、文件位置与设置完善', () => {
  test('applies paired, RAF-only and JPG-only external action rules', async () => {
    render(<App />);
    await screen.findByText('DSCF1031.JPG');

    const photoshop = screen.getByRole('button', {
      name: '用 Photoshop 打开 RAF',
    });
    const location = screen.getByRole('button', {
      name: '打开当前文件位置',
    });
    expect(photoshop).toBeDisabled();
    expect(location).toBeEnabled();

    fireEvent.click(
      screen.getByRole('button', { name: '第 2 张：DSCF1039.RAF' }),
    );
    await waitFor(() => expect(photoshop).toBeEnabled());
    expect(location).toBeEnabled();

    fireEvent.click(
      screen.getByRole('button', { name: '第 1 张：DSCF1042.JPG' }),
    );
    await waitFor(() => expect(screen.getByText('JPG ✓')).toBeInTheDocument());
    expect(screen.getByText('RAF ✓')).toBeInTheDocument();
    expect(photoshop).toBeEnabled();
    fireEvent.click(photoshop);
    expect(
      await screen.findByRole('dialog', { name: '把 RAF 交给 Photoshop？' }),
    ).toHaveTextContent('不受本应用控制');
    fireEvent.click(screen.getByRole('button', { name: '取消' }));
  });

  test('first RAF click configures through the picker while invalid tools require settings', async () => {
    window.history.replaceState(null, '', '/?scenario=photoshop-unconfigured');
    const first = render(<App />);
    fireEvent.click(
      await screen.findByRole('button', { name: '第 2 张：DSCF1039.RAF' }),
    );
    const photoshop = screen.getByRole('button', {
      name: '用 Photoshop 打开 RAF',
    });
    await waitFor(() => expect(photoshop).toBeEnabled());
    fireEvent.click(photoshop);
    expect(
      await screen.findByRole('dialog', { name: '把 RAF 交给 Photoshop？' }),
    ).toBeInTheDocument();

    first.unmount();
    resetDemoSettings();
    window.history.replaceState(null, '', '/?scenario=photoshop-invalid');
    render(<App />);
    fireEvent.click(
      await screen.findByRole('button', { name: '第 2 张：DSCF1039.RAF' }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: '用 Photoshop 打开 RAF' }),
      ).toBeDisabled(),
    );
    expect(screen.getByText(/Photoshop 需要重新选择/)).toBeInTheDocument();
  });

  test('renders all four settings groups without exposing ToolId or manual cleanup', async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: /设置/ }));

    expect(await screen.findByLabelText('写入安全状态')).toHaveTextContent(
      'Import 与 Recycle 已开放',
    );

    const sidebar = screen.getByRole('navigation', { name: '设置分组' });
    expect(
      within(sidebar).getByRole('button', { name: /照片文件夹/ }),
    ).toBeInTheDocument();
    expect(
      within(sidebar).getByRole('button', { name: /预览缓存/ }),
    ).toBeInTheDocument();
    expect(
      within(sidebar).getByRole('button', { name: /Photoshop/ }),
    ).toBeInTheDocument();
    expect(
      within(sidebar).getByRole('button', { name: /操作记录/ }),
    ).toBeInTheDocument();

    fireEvent.click(within(sidebar).getByRole('button', { name: /预览缓存/ }));
    expect(await screen.findByText('实际占用')).toBeInTheDocument();
    expect(screen.getByText('磁盘可用')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /清理缓存/ }),
    ).not.toBeInTheDocument();

    const capacity = screen.getByRole('combobox', {
      name: '预览缓存容量上限',
    });
    expect(capacity).toHaveValue(String(32 * 1_073_741_824));
    fireEvent.change(capacity, {
      target: { value: String(16 * 1_073_741_824) },
    });
    expect(
      screen.getByRole('dialog', { name: '更改预览缓存上限？' }),
    ).toHaveTextContent('后台渐进淘汰');
    fireEvent.click(screen.getByRole('button', { name: '取消' }));
    expect(capacity).toHaveValue(String(32 * 1_073_741_824));

    fireEvent.click(within(sidebar).getByRole('button', { name: /Photoshop/ }));
    expect(
      await screen.findByText('Photoshop.exe · 已验证'),
    ).toBeInTheDocument();
    expect(screen.queryByText(/20000000-0000/)).not.toBeInTheDocument();
    expect(screen.getByText(/不控制 Photoshop 后续写入/)).toBeInTheDocument();
  });

  test('pending-delete DOM contains neither external action nor placeholder', async () => {
    window.history.replaceState(null, '', '/?scenario=pending');
    render(<App />);
    fireEvent.click(
      await within(
        await screen.findByRole('navigation', { name: '照片视图' }),
      ).findByRole('button', { name: /^待删除 1$/ }),
    );
    await screen.findByText('待删除 · 第 1 / 1 张');
    expect(
      screen.queryByRole('button', { name: /Photoshop|文件位置/ }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/Photoshop|文件位置/)).not.toBeInTheDocument();
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
