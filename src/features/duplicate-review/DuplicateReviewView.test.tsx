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
};

const results: commands.DuplicateReviewResults = {
  runId: '00000000-0000-0000-0000-000000000001',
  expired: false,
  expiredReason: null,
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

function view(status: commands.DuplicateReviewStatus = idle) {
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
      onStatusChange={vi.fn()}
      onOpenLibrary={vi.fn()}
      onOpenSettings={vi.fn()}
      onPendingAdded={vi.fn().mockResolvedValue(undefined)}
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
    expect(screen.getByText(/将回收：G:\\old\\delete.jpg/)).toBeInTheDocument();
    expect(screen.getByText(/保留：G:\\2026\\keep.jpg/)).toBeInTheDocument();
  });
});
