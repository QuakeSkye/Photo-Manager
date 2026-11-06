import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
  act,
} from '@testing-library/react';
import { createRef } from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import * as commands from '../../shared/commands/import';
import * as library from '../../shared/commands/library';

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

    expect(await screen.findByText(/暂时无法导入/)).toHaveTextContent(
      '数据库 writer 已失效',
    );
    expect(screen.getByRole('button', { name: '检查导入内容' })).toBeDisabled();
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
    expect(screen.queryByText('检查导入计划')).toBeInTheDocument();
    expect(screen.queryByText('DSCF2041.JPG')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '检查导入内容' }));

    expect(
      await screen.findByRole('region', { name: '本次导入计划' }),
    ).toBeInTheDocument();
    expect(screen.getByText('已有相同文件')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /同名另存/ }),
    ).toBeInTheDocument();
    expect(screen.getAllByText('日期未知').length).toBeGreaterThan(0);
    expect(screen.getAllByText('无法读取').length).toBeGreaterThan(0);
    expect(screen.getAllByText('DSCF2041.JPG').length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('checkbox')).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: '确认导入…' }));
    expect(screen.getByRole('dialog')).toHaveTextContent('确认导入');
    expect(screen.getByRole('dialog')).toHaveTextContent('将按完整计划新增');
    expect(screen.queryByText('导入完成')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '开始导入' }));
    expect(
      await screen.findByRole('heading', { name: '导入完成' }),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/来源写请求|TaskIntent|ReplaceIfExists/),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/检查时有 1 个文件无法读取/)).toBeInTheDocument();
    expect(
      screen.queryByText(/demo-only-not-rendered/),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByText('正在检查导入内容')).not.toBeInTheDocument(),
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
  await waitFor(() =>
    expect(screen.getByRole('button', { name: '检查导入内容' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: '检查导入内容' }));
  fireEvent.click(await screen.findByRole('button', { name: '确认导入…' }));
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
    await screen.findByText(/D:\/test\/retained.partial/),
  ).toBeInTheDocument();
  fireEvent.click(
    await screen.findByRole('button', { name: '保留异常项并继续…' }),
  );
  expect(retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '保留异常项并继续' }));
  await waitFor(() => expect(exclude).toHaveBeenCalledWith(['abc']));
  await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
});

test('plan filters only change the visible list and submit the complete digest', async () => {
  const draft = await commands.startImportPreflight();
  const start = vi
    .spyOn(commands, 'startImport')
    .mockResolvedValue(executionStatus('completed'));
  renderImport();
  await waitFor(() =>
    expect(screen.getByRole('button', { name: '检查导入内容' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: '检查导入内容' }));
  fireEvent.click(
    await screen.findByRole('button', { name: /无法读取.*个文件/ }),
  );
  expect(screen.queryByText('DSCF2041.JPG')).not.toBeInTheDocument();
  expect(screen.getByText('BROKEN.RAF')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '确认导入…' }));
  const dialog = screen.getByRole('dialog');
  expect(dialog).toHaveTextContent('3 个文件');
  expect(dialog).toHaveTextContent(draft.sourcePath);
  expect(dialog).toHaveTextContent(draft.targetPath);
  expect(start).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole('button', { name: '开始导入' }));
  await waitFor(() =>
    expect(start).toHaveBeenCalledExactlyOnceWith(draft.draftDigest),
  );
});

test('returning to selection invalidates the old plan before choosing another source', async () => {
  const draft = await commands.startImportPreflight();
  const preflight = vi
    .spyOn(commands, 'startImportPreflight')
    .mockResolvedValueOnce(draft)
    .mockResolvedValue({ ...draft, draftDigest: 'replacement-plan' });
  const choose = vi
    .spyOn(commands, 'chooseImportSourceFolder')
    .mockResolvedValue('F:/DCIM');
  const setup = await commands.getImportSetup();
  vi.spyOn(commands, 'selectImportSource').mockResolvedValue({
    ...setup,
    source: { ...setup.source!, displayPath: 'F:/DCIM' },
  });
  const start = vi
    .spyOn(commands, 'startImport')
    .mockResolvedValue(executionStatus('completed'));
  renderImport();
  await waitFor(() =>
    expect(screen.getByRole('button', { name: '检查导入内容' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: '检查导入内容' }));
  fireEvent.click(
    await screen.findByRole('button', { name: '更改来源或目标' }),
  );
  await waitFor(() =>
    expect(
      screen.getByRole('button', { name: '更改来源文件夹' }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: '更改来源文件夹' }));
  await screen.findByText('F:/DCIM');
  expect(choose).toHaveBeenCalledOnce();
  expect(
    screen.queryByRole('button', { name: '确认导入…' }),
  ).not.toBeInTheDocument();
  await confirmInitialImport();
  await waitFor(() =>
    expect(start).toHaveBeenCalledExactlyOnceWith('replacement-plan'),
  );
  expect(preflight).toHaveBeenCalledTimes(2);
});

test('does not start twice while the start command is pending', async () => {
  let finish!: (value: commands.ImportExecutionStatus) => void;
  const start = vi.spyOn(commands, 'startImport').mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  renderImport();
  await confirmInitialImport();
  const confirm = within(screen.getByRole('dialog')).getByRole('button', {
    name: '正在确认…',
  });
  expect(confirm).toBeDisabled();
  fireEvent.click(confirm);
  expect(start).toHaveBeenCalledOnce();
  await act(async () => {
    finish(executionStatus('completed'));
    await Promise.resolve();
  });
  expect(
    await screen.findByRole('heading', { name: '导入完成' }),
  ).toBeInTheDocument();
});

test('stopping remains pending until the backend reports cancellation', async () => {
  vi.spyOn(commands, 'startImport').mockResolvedValue(
    executionStatus('running'),
  );
  const status = vi
    .spyOn(commands, 'getImportExecutionStatus')
    .mockResolvedValue(executionStatus('running'));
  const cancel = vi
    .spyOn(commands, 'cancelRemainingImport')
    .mockResolvedValue();
  renderImport();
  await confirmInitialImport();
  fireEvent.click(await screen.findByRole('button', { name: '停止导入…' }));
  expect(cancel).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '保留文件并取消' }));
  expect(
    await screen.findByRole('heading', { name: '正在停止导入…' }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole('heading', { name: '导入完成' }),
  ).not.toBeInTheDocument();
  status.mockResolvedValue({
    ...executionStatus('cancelled'),
    publishedFiles: 2,
    notStartedFiles: 1,
  });
  expect(
    await screen.findByRole('heading', { name: '导入已取消' }),
  ).toBeInTheDocument();
  expect(cancel).toHaveBeenCalledOnce();
  expect(screen.getByText(/已完成的照片保留在图库中/)).toBeInTheDocument();
});

test('partial completion keeps recovery controls and does not offer a new task', async () => {
  vi.spyOn(commands, 'startImport').mockResolvedValue({
    ...executionStatus('completedWithErrors'),
    publishedFiles: 2,
    failedFiles: 1,
  });
  renderImport();
  await confirmInitialImport();
  expect(
    await screen.findByRole('heading', { name: '导入部分完成' }),
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '检查并继续' })).toBeEnabled();
  expect(screen.getByRole('button', { name: '取消剩余导入' })).toBeEnabled();
  expect(
    screen.queryByRole('button', { name: '导入其他照片' }),
  ).not.toBeInTheDocument();
});

test('source reconnection requiring a fresh plan cannot reuse the old confirmation', async () => {
  vi.spyOn(commands, 'startImport').mockResolvedValue(
    executionStatus('awaitingSource'),
  );
  const setup = await commands.getImportSetup();
  vi.spyOn(commands, 'reselectImportSource').mockResolvedValue(setup);
  vi.spyOn(commands, 'getImportExecutionStatus').mockResolvedValue(
    executionStatus('idle'),
  );
  renderImport();
  await confirmInitialImport();
  fireEvent.click(
    await screen.findByRole('button', { name: '重新选择原来源' }),
  );
  expect(
    await screen.findByRole('heading', { name: '选择导入位置' }),
  ).toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent(
    '请重新检查导入内容并确认',
  );
  expect(
    screen.queryByRole('button', { name: '确认导入…' }),
  ).not.toBeInTheDocument();
});

test('changing the target discards the plan and requires a new preflight', async () => {
  const initialSetup = await commands.getImportSetup();
  const setup = vi
    .spyOn(commands, 'getImportSetup')
    .mockResolvedValue(initialSetup);
  vi.spyOn(library, 'chooseLibraryFolder').mockResolvedValue('H:/Photos');
  vi.spyOn(library, 'validateLibraryFolder').mockResolvedValue({
    displayPath: 'H:/Photos',
    reconnectsExistingLibrary: false,
  });
  const bind = vi.fn(() => {
    setup.mockResolvedValue({
      ...initialSetup,
      target: { displayPath: 'H:/Photos' },
    });
    return Promise.resolve();
  });
  render(
    <ImportView
      pendingDeleteSummary={pendingDeleteSummary}
      settingsButtonRef={createRef<HTMLButtonElement>()}
      onOpenLibrary={vi.fn()}
      onOpenPendingDelete={vi.fn()}
      onOpenSettings={vi.fn()}
      onBindTarget={bind}
    />,
  );
  await waitFor(() =>
    expect(screen.getByRole('button', { name: '检查导入内容' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: '检查导入内容' }));
  fireEvent.click(
    await screen.findByRole('button', { name: '更改来源或目标' }),
  );
  await waitFor(() =>
    expect(screen.getByRole('button', { name: '更改目标图库' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: '更改目标图库' }));
  fireEvent.click(await screen.findByRole('button', { name: '确认切换' }));
  await waitFor(() =>
    expect(bind).toHaveBeenCalledExactlyOnceWith('H:/Photos'),
  );
  await waitFor(() =>
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
  );
  expect(screen.getByText('H:/Photos')).toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: '确认导入…' }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: '检查导入内容' })).toBeEnabled();
});
