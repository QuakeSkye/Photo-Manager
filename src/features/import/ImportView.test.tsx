import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { createRef } from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import * as commands from '../../shared/commands/import';

import { ImportView } from './ImportView';

const pendingDeleteSummary = {
  totalCount: 0,
  executableCount: 0,
  problemCount: 0,
  needsAttentionCount: 0,
  executableFileCount: 0,
  executableBytes: 0,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('phase six import execution UI', () => {
  test('shows a closed safety reason and disables every write entry', async () => {
    render(
      <ImportView
        pendingDeleteSummary={pendingDeleteSummary}
        settingsButtonRef={createRef<HTMLButtonElement>()}
        onOpenLibrary={vi.fn()}
        onOpenPendingDelete={vi.fn()}
        onOpenSettings={vi.fn()}
        onBindTarget={vi.fn()}
        writeEnabled={false}
        safetyMessage="数据库 writer 已失效"
      />,
    );

    expect(
      await screen.findByText(/Import 写入安全门已关闭/),
    ).toHaveTextContent('数据库 writer 已失效');
    expect(screen.getByRole('button', { name: '开始只读预检' })).toBeDisabled();
    expect(
      screen.getAllByRole('button', { name: /选择文件夹|更改/ })[0],
    ).toBeDisabled();
  });

  test('keeps the draft non-executing until the final Start Import confirmation', async () => {
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
    fireEvent.click(screen.getByRole('button', { name: '确认并导入…' }));
    expect(screen.getByRole('dialog')).toHaveTextContent('最终确认导入');
    expect(screen.getByRole('dialog')).toHaveTextContent(
      '点击“开始导入”后才会创建 TaskIntent',
    );
    expect(screen.queryByText('导入完成')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '开始导入' }));
    expect(
      await screen.findByRole('heading', { name: '导入完成' }),
    ).toBeInTheDocument();
    expect(screen.getByText('来源写请求：0（必须为 0）')).toBeInTheDocument();
    expect(
      screen.queryByText(/demo-only-not-rendered/),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByText('正在只读预检')).not.toBeInTheDocument(),
    );
  });
});

const executionStatus = (
  phase: commands.ImportExecutionPhase,
): commands.ImportExecutionStatus => ({
  phase,
  jobId: 'test-job',
  attemptOrdinal: null,
  completedFiles: 0,
  publishedFiles: 0,
  skippedFiles: 0,
  failedFiles: 0,
  notStartedFiles: 1,
  currentFile: null,
  currentAction: null,
  message: null,
  updatedPlan: null,
  space: null,
  sourceWriteRequests: 0,
});
function renderImport() {
  return render(
    <ImportView
      pendingDeleteSummary={pendingDeleteSummary}
      settingsButtonRef={createRef<HTMLButtonElement>()}
      onOpenLibrary={vi.fn()}
      onOpenPendingDelete={vi.fn()}
      onOpenSettings={vi.fn()}
      onBindTarget={vi.fn()}
    />,
  );
}
async function confirmInitialImport() {
  await screen.findByRole('button', { name: '开始只读预检' });
  fireEvent.click(screen.getByRole('button', { name: '开始只读预检' }));
  fireEvent.click(await screen.findByRole('button', { name: '确认并导入…' }));
  fireEvent.click(screen.getByRole('button', { name: '开始导入' }));
}

test('async updated plan is the exact draft submitted on reconfirmation', async () => {
  const draft = await commands.startImportPreflight();
  const updated = { ...draft, draftDigest: 'new-authoritative-digest' };
  const start = vi
    .spyOn(commands, 'startImport')
    .mockResolvedValueOnce(executionStatus('validatingPlan'))
    .mockResolvedValue(executionStatus('completed'));
  vi.spyOn(commands, 'getImportExecutionStatus').mockResolvedValue({
    ...executionStatus('awaitingPlanConfirmation'),
    updatedPlan: updated,
  });
  renderImport();
  await confirmInitialImport();
  fireEvent.click(
    await screen.findByRole('button', { name: '重新确认并导入…' }),
  );
  fireEvent.click(screen.getByRole('button', { name: '开始导入' }));
  await waitFor(() =>
    expect(start).toHaveBeenLastCalledWith('new-authoritative-digest'),
  );
});

test('waiting for space can be cancelled while preserving files', async () => {
  vi.spyOn(commands, 'startImport').mockResolvedValue(
    executionStatus('awaitingSpace'),
  );
  const cancel = vi
    .spyOn(commands, 'cancelRemainingImport')
    .mockResolvedValue();
  vi.spyOn(commands, 'getImportExecutionStatus').mockResolvedValue(
    executionStatus('cancelled'),
  );
  renderImport();
  await confirmInitialImport();
  fireEvent.click(await screen.findByRole('button', { name: '取消剩余导入' }));
  expect(screen.getByRole('dialog')).toHaveTextContent(
    '已导入照片和临时文件均保留',
  );
  expect(cancel).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '保留文件并取消' }));
  await waitFor(() => expect(cancel).toHaveBeenCalledTimes(1));
  expect(
    await screen.findByRole('heading', { name: '导入已取消' }),
  ).toBeInTheDocument();
});

test('recovery lists uncertain paths and requires explicit isolation before retry', async () => {
  vi.spyOn(commands, 'getImportRecoverySummary').mockResolvedValue({
    hasUnfinishedImport: true,
    jobId: 'test-job',
    phase: 'needsAttention',
    completedFiles: 0,
    publishedFiles: 0,
    unresolvedFiles: 2,
    message: '等待核对',
  });
  vi.spyOn(commands, 'getImportRecoveryItems').mockResolvedValue([
    {
      ownershipId: 'abc',
      sourceItemId: 'source',
      sourcePath: 'DSCF0001.JPG',
      retainedPath: 'D:/test/retained.partial',
      observedBytes: null,
      state: 'NeedsAttention',
      excluded: false,
      canExclude: true,
    },
  ]);
  const exclude = vi
    .spyOn(commands, 'excludeImportRecoveryItems')
    .mockResolvedValue();
  const retry = vi
    .spyOn(commands, 'retryImport')
    .mockResolvedValue(executionStatus('validatingPlan'));
  renderImport();
  expect(
    await screen.findByText('D:/test/retained.partial'),
  ).toBeInTheDocument();
  fireEvent.click(
    await screen.findByRole('button', { name: '隔离异常项并继续…' }),
  );
  expect(retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '隔离并继续安全项' }));
  await waitFor(() => expect(exclude).toHaveBeenCalledWith(['abc']));
  await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
});
