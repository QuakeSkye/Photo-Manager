import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { createRef } from 'react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import * as commands from '../../shared/commands/duplicate-review';
import { DuplicateReviewView } from './DuplicateReviewView';

vi.mock('../../shared/commands/duplicate-review', async () => {
  const actual = await vi.importActual<typeof commands>(
    '../../shared/commands/duplicate-review',
  );
  return {
    ...actual,
    getDuplicateReviewResults: vi.fn(),
    startDuplicateReviewScan: vi.fn(),
    cancelDuplicateReviewScan: vi.fn(),
    openDuplicateAssetLocation: vi.fn(),
    addDuplicateSubsetToPendingDelete: vi.fn(),
    startDuplicatePendingBatch: vi.fn(),
    getDuplicatePendingBatchStatus: vi.fn(),
    cancelDuplicatePendingBatch: vi.fn(),
  };
});

const idle: commands.DuplicateReviewStatus = {
  state: 'idle',
  runId: null,
  phase: null,
  checkedFiles: 0,
  candidateFiles: 0,
  hashedFiles: 0,
  currentRelativePath: null,
  familyCount: 0,
  expiredReason: null,
  resultVersionId: null,
  lastReceipt: null,
};

const results: commands.DuplicateReviewResults = {
  runId: '00000000-0000-0000-0000-000000000001',
  resultVersionId: '00000000-0000-0000-0000-000000000001',
  expired: false,
  expiredReason: null,
  lastReceipt: null,
  summary: {
    equivalenceSets: 1,
    jpgOnlyFamilies: 1,
    rafOnlyFamilies: 0,
    jpgAndRafFamilies: 0,
    redundantFiles: 1,
    reclaimableBytes: 10,
  },
  families: [
    {
      familyId: 'family',
      raf: null,
      jpg: {
        setId: '00000000-0000-0000-0000-000000000002',
        role: 'Jpg',
        fileSizeBytes: 10,
        redundantFiles: 1,
        reclaimableBytes: 10,
        suggestedRetainedAssetId: 'keep',
        members: [
          {
            assetId: 'keep',
            assetVersion: 1,
            role: 'Jpg',
            fullPath: 'G:\\2026\\keep.jpg',
            directory: 'G:\\2026',
            relativePath: '2026\\keep.jpg',
            sizeBytes: 10,
            captureLocalKey: null,
            photoId: 'photo-keep',
            online: true,
            selectable: true,
            suggestedRetain: true,
          },
          {
            assetId: 'delete',
            assetVersion: 1,
            role: 'Jpg',
            fullPath: 'G:\\old\\delete.jpg',
            directory: 'G:\\old',
            relativePath: 'old\\delete.jpg',
            sizeBytes: 10,
            captureLocalKey: null,
            photoId: 'photo-delete',
            online: true,
            selectable: true,
            suggestedRetain: false,
          },
        ],
      },
    },
  ],
};

function view(
  status: commands.DuplicateReviewStatus = idle,
  onStatusChange = vi.fn(),
  onPendingAdded = vi.fn().mockResolvedValue(undefined),
) {
  return (
    <DuplicateReviewView
      status={status}
      pendingDeleteSummary={{
        totalCount: 0,
        executableCount: 0,
        problemCount: 0,
        needsAttentionCount: 0,
        executableFileCount: 0,
        executableBytes: 0,
      }}
      settingsButtonRef={createRef()}
      onStatusChange={onStatusChange}
      onOpenLibrary={vi.fn()}
      onOpenPendingDelete={vi.fn()}
      onOpenSettings={vi.fn()}
      onPendingAdded={onPendingAdded}
    />
  );
}

describe('DuplicateReviewView', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(cleanup);

  test('does not start a scan when opened and starts only from the explicit button', async () => {
    vi.mocked(commands.startDuplicateReviewScan).mockResolvedValue({
      ...idle,
      state: 'running',
    });
    render(view());
    expect(commands.startDuplicateReviewScan).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '开始检测' }));
    await waitFor(() =>
      expect(commands.startDuplicateReviewScan).toHaveBeenCalledOnce(),
    );
  });

  test('shows cancellation progress and applies the returned terminal status', async () => {
    const onStatusChange = vi.fn();
    const cancelled = { ...idle, state: 'cancelled' as const };
    vi.mocked(commands.cancelDuplicateReviewScan).mockResolvedValue(cancelled);
    render(
      view({ ...idle, state: 'running', phase: 'Comparing' }, onStatusChange),
    );

    fireEvent.click(screen.getByRole('button', { name: '取消检测' }));

    expect(screen.getByRole('button', { name: '正在取消…' })).toBeDisabled();
    await waitFor(() => expect(onStatusChange).toHaveBeenCalledWith(cancelled));
  });

  test('labels hashing progress without implying byte comparison is complete', () => {
    render(
      view({
        ...idle,
        state: 'running',
        phase: 'Comparing',
        checkedFiles: 4,
        candidateFiles: 4,
        hashedFiles: 4,
      }),
    );
    expect(screen.getByText('正在逐字节比较')).toBeInTheDocument();
    expect(screen.getByText(/已完成哈希 4 个/)).toBeInTheDocument();
    expect(screen.queryByText(/已完整验证/)).not.toBeInTheDocument();
  });

  test('shows the six-part summary and does not preselect suggested files', async () => {
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    expect(await screen.findAllByText('JPG：2 份（重复 1 份）')).toHaveLength(
      2,
    );
    expect(screen.getByText('理论可释放')).toBeInTheDocument();
    expect(
      screen.getByText(/区分大小写的完整文件名与大小筛选/),
    ).toBeInTheDocument();
    expect(screen.getAllByRole('checkbox')).toHaveLength(2);
    expect(
      screen
        .getAllByRole('checkbox')
        .every((box) => !(box as HTMLInputElement).checked),
    ).toBe(true);
    expect(screen.queryAllByRole('radio')).toHaveLength(0);
    expect(
      screen.queryByRole('button', { name: /选择这一整套副本/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByText('建议保留')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: '勾选全部非建议保留（1）' }),
    ).toHaveAttribute('aria-pressed', 'false');
  });

  test('marks checked rows as pending delete and keeps the full path readable', async () => {
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    const checkbox = await screen.findByRole('checkbox', {
      name: /将 G:\\old\\delete.jpg 标记为待删除/,
    });
    fireEvent.click(checkbox);
    const row = screen.getByText('delete.jpg').closest('.duplicate-member');
    expect(row).toHaveClass('is-selected');
    expect(row).toHaveTextContent('G:\\old\\delete.jpg');
    expect(row).toHaveTextContent('待删除');
    expect(
      screen.getByRole('checkbox', {
        name: /取消 G:\\old\\delete.jpg 的待删除标记/,
      }),
    ).toBeChecked();
  });

  test('bulk selects every non-suggested file and toggles back to no selection', async () => {
    const secondFamily = {
      ...results.families[0],
      familyId: 'family-two',
      jpg: {
        ...results.families[0].jpg!,
        setId: '00000000-0000-0000-0000-000000000003',
        suggestedRetainedAssetId: 'keep-two',
        members: [
          {
            ...results.families[0].jpg!.members[0],
            assetId: 'keep-two',
            fullPath: 'G:\\2026\\keep-two.jpg',
            relativePath: '2026\\keep-two.jpg',
          },
          {
            ...results.families[0].jpg!.members[1],
            assetId: 'delete-two',
            fullPath: 'G:\\old\\delete-two.jpg',
            relativePath: 'old\\delete-two.jpg',
          },
        ],
      },
    };
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue({
      ...results,
      families: [...results.families, secondFamily],
    });
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 2,
      }),
    );
    fireEvent.click(
      await screen.findByRole('button', {
        name: '勾选全部非建议保留（2）',
      }),
    );
    expect(
      screen.getByRole('checkbox', { name: /取消 G:\\old\\delete.jpg/ }),
    ).toBeChecked();
    expect(
      screen.getByRole('checkbox', {
        name: /取消 G:\\old\\delete-two.jpg/,
      }),
    ).toBeChecked();
    const clear = screen.getByRole('button', { name: '取消全部勾选（2）' });
    expect(clear).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(clear);
    expect(
      screen
        .getAllByRole('checkbox')
        .every((box) => !(box as HTMLInputElement).checked),
    ).toBe(true);
    expect(screen.getByText('尚未选择文件')).toBeInTheDocument();
  });

  test('allows overriding the suggestion but refuses selecting the last copy', async () => {
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    fireEvent.click(
      await screen.findByRole('checkbox', {
        name: /将 G:\\2026\\keep.jpg 标记为待删除/,
      }),
    );
    fireEvent.click(
      screen.getByRole('checkbox', {
        name: /将 G:\\old\\delete.jpg 标记为待删除/,
      }),
    );
    expect(
      screen.getByText('每组重复文件至少需要保留一份。'),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('checkbox', {
        name: /将 G:\\old\\delete.jpg 标记为待删除/,
      }),
    ).not.toBeChecked();
  });

  test('keeps the action bar outside the scrolling main content', async () => {
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    const { container } = render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    await screen.findByLabelText('待删除选择摘要');
    const main = container.querySelector('.duplicate-review-main');
    const bar = container.querySelector('.duplicate-selection-bar');
    expect(main?.parentElement).toBe(bar?.parentElement);
    expect(main?.contains(bar)).toBe(false);
  });

  test('confirms explicit delete and retained paths before creating a mark', async () => {
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    vi.mocked(commands.addDuplicateSubsetToPendingDelete).mockResolvedValue(
      undefined,
    );
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    const checkbox = await screen.findByRole('checkbox', {
      name: /delete.jpg/,
    });
    fireEvent.click(checkbox);
    fireEvent.click(screen.getByRole('button', { name: '加入待删除（1）…' }));
    const dialog = screen.getByRole('dialog', { name: '确认加入待删除' });
    const list = dialog.querySelector('.duplicate-confirm-list');
    expect(dialog).toHaveClass('duplicate-confirm-modal');
    expect(list).toBeInstanceOf(HTMLUListElement);
    expect(list?.parentElement).toHaveClass('modal-body');
    expect(dialog.querySelector('.modal-actions')).toBeInTheDocument();
    expect(screen.getByText(/将回收：G:\\old\\delete.jpg/)).toBeInTheDocument();
    expect(screen.getByText(/保留：G:\\2026\\keep.jpg/)).toBeInTheDocument();
  });

  test('keeps the dialog open for validation and routes only after the whole batch completes', async () => {
    const onPendingAdded = vi.fn().mockResolvedValue(undefined);
    const validating = batchStatus('validating');
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    vi.mocked(commands.startDuplicatePendingBatch).mockResolvedValue(
      validating,
    );
    vi.mocked(commands.getDuplicatePendingBatchStatus).mockResolvedValue({
      ...validating,
      phase: 'completed',
      validatedFiles: 1,
      validatedBytes: 20,
      markedPhotos: 1,
      markedFiles: 1,
      markedBytes: 10,
    });
    render(
      view(
        {
          ...idle,
          state: 'completedWithDuplicates',
          runId: results.runId,
          familyCount: 1,
        },
        vi.fn(),
        onPendingAdded,
      ),
    );
    fireEvent.click(
      await screen.findByRole('checkbox', { name: /delete.jpg/ }),
    );
    fireEvent.click(screen.getByRole('button', { name: '加入待删除（1）…' }));
    fireEvent.click(screen.getByRole('button', { name: '加入待删除' }));

    expect(await screen.findByText('正在验证 0 / 1')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '取消验证' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '取消验证' })).toHaveFocus();
    expect(onPendingAdded).not.toHaveBeenCalled();
    await waitFor(() => expect(onPendingAdded).toHaveBeenCalledOnce(), {
      timeout: 2_000,
    });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  test('shows the first failed pair inside the dialog and preserves the selection', async () => {
    const validating = batchStatus('validating');
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    vi.mocked(commands.startDuplicatePendingBatch).mockResolvedValue(
      validating,
    );
    vi.mocked(commands.getDuplicatePendingBatchStatus).mockResolvedValue({
      ...validating,
      phase: 'failed',
      failure: {
        code: 'identity_mismatch',
        message: '待删文件与保留副本已不再逐字节相同。',
        deletedPath: 'G:\\old\\delete.jpg',
        retainedPath: 'G:\\2026\\keep.jpg',
        diagnostic: null,
      },
    });
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    fireEvent.click(
      await screen.findByRole('checkbox', { name: /delete.jpg/ }),
    );
    fireEvent.click(screen.getByRole('button', { name: '加入待删除（1）…' }));
    fireEvent.click(screen.getByRole('button', { name: '加入待删除' }));

    expect(await screen.findByText('整批未提交')).toBeInTheDocument();
    expect(
      screen.getByText('待删文件与保留副本已不再逐字节相同。'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重新验证整批' })).toBeEnabled();
    expect(
      screen.getByRole('checkbox', { name: /取消 G:\\old\\delete.jpg/ }),
    ).toBeChecked();
  });

  test('cancels validation without clearing the selected files', async () => {
    const validating = batchStatus('validating');
    vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue(results);
    vi.mocked(commands.startDuplicatePendingBatch).mockResolvedValue(
      validating,
    );
    vi.mocked(commands.cancelDuplicatePendingBatch).mockResolvedValue({
      ...validating,
      cancellationRequested: true,
    });
    vi.mocked(commands.getDuplicatePendingBatchStatus).mockResolvedValue({
      ...validating,
      phase: 'cancelled',
      cancellationRequested: true,
    });
    render(
      view({
        ...idle,
        state: 'completedWithDuplicates',
        runId: results.runId,
        familyCount: 1,
      }),
    );
    fireEvent.click(
      await screen.findByRole('checkbox', { name: /delete.jpg/ }),
    );
    fireEvent.click(screen.getByRole('button', { name: '加入待删除（1）…' }));
    fireEvent.click(screen.getByRole('button', { name: '加入待删除' }));
    fireEvent.click(await screen.findByRole('button', { name: '取消验证' }));

    expect(await screen.findByText(/验证已安全取消/)).toBeInTheDocument();
    expect(
      screen.getByRole('checkbox', { name: /取消 G:\\old\\delete.jpg/ }),
    ).toBeChecked();
  });
});

function batchStatus(
  phase: commands.DuplicatePendingBatchPhase,
): commands.DuplicatePendingBatchStatus {
  return {
    taskId: '00000000-0000-0000-0000-000000000020',
    operationId: '00000000-0000-0000-0000-000000000021',
    phase,
    cancellationRequested: false,
    totalFiles: 1,
    validatedFiles: 0,
    totalBytes: 20,
    validatedBytes: 0,
    currentDeletedPath: 'G:\\old\\delete.jpg',
    currentRetainedPath: 'G:\\2026\\keep.jpg',
    markedPhotos: 0,
    markedFiles: 0,
    markedBytes: 0,
    failure: null,
  };
}

test('shows the recycle receipt and completion actions for a reconciled empty result', async () => {
  const receipt: commands.DuplicateReviewReceipt = {
    reconciliationId: '00000000-0000-0000-0000-000000000010',
    batchId: '00000000-0000-0000-0000-000000000011',
    state: 'applied',
    recycledFiles: 1,
    recycledBytes: 10,
    completedAt: 1,
    message: '图库索引和重复结果已更新，无须重新检测。',
  };
  vi.mocked(commands.getDuplicateReviewResults).mockResolvedValue({
    ...results,
    resultVersionId: receipt.reconciliationId,
    lastReceipt: receipt,
    summary: {
      equivalenceSets: 0,
      jpgOnlyFamilies: 0,
      rafOnlyFamilies: 0,
      jpgAndRafFamilies: 0,
      redundantFiles: 0,
      reclaimableBytes: 0,
    },
    families: [],
  });
  render(
    view({
      ...idle,
      state: 'completedEmpty',
      runId: results.runId,
      resultVersionId: receipt.reconciliationId,
      lastReceipt: receipt,
    }),
  );

  expect(await screen.findByText('重复文件已处理')).toBeInTheDocument();
  expect(screen.getByText('本次发现的重复文件已处理完')).toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: '返回图库' })).toHaveLength(2);
  expect(
    screen.getByRole('button', { name: '再次检查重复' }),
  ).toBeInTheDocument();
});
