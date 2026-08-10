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
    fireEvent.click(screen.getByRole('button', { name: '加入待删除…' }));
    expect(screen.getByText(/将回收：G:\\old\\delete.jpg/)).toBeInTheDocument();
    expect(screen.getByText(/保留：G:\\2026\\keep.jpg/)).toBeInTheDocument();
  });
});
