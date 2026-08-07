import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createRef } from 'react';
import { describe, expect, test, vi } from 'vitest';

import { ImportView } from './ImportView';

const pendingDeleteSummary = {
  totalCount: 0,
  executableCount: 0,
  problemCount: 0,
  needsAttentionCount: 0,
  executableFileCount: 0,
  executableBytes: 0,
};

describe('phase five import preflight UI', () => {
  test('requires an explicit click and renders the complete read-only draft categories', async () => {
    render(
      <ImportView
        pendingDeleteSummary={pendingDeleteSummary}
        settingsButtonRef={createRef<HTMLButtonElement>()}
        onOpenLibrary={vi.fn()}
        onOpenPendingDelete={vi.fn()}
        onOpenSettings={vi.fn()}
        onBindTarget={vi.fn()}
      />,
    );

    expect(await screen.findByText('选择导入位置')).toBeInTheDocument();
    expect(screen.getByText('E:\\DCIM\\100_FUJI')).toBeInTheDocument();
    expect(screen.getByText('G:\\照片')).toBeInTheDocument();
    expect(screen.queryByText('预检与确认')).toBeInTheDocument();
    expect(screen.queryByText('DSCF2041.JPG')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '开始只读预检' }));

    expect(await screen.findByText('本次导入计划')).toBeInTheDocument();
    expect(screen.getByText('已存在，跳过')).toBeInTheDocument();
    expect(screen.getByText('同名但内容不同')).toBeInTheDocument();
    expect(screen.getAllByText('日期未知').length).toBeGreaterThan(0);
    expect(screen.getAllByText('无法读取').length).toBeGreaterThan(0);
    expect(screen.getAllByText('DSCF2041.JPG').length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('checkbox')).toHaveLength(0);
    expect(screen.getByRole('button', { name: '确认并导入…' })).toBeDisabled();
    expect(
      screen.getByText('当前阶段仅开放只读预检；导入执行保持关闭。'),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/demo-only-not-rendered/),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByText('正在只读预检')).not.toBeInTheDocument(),
    );
  });
});
