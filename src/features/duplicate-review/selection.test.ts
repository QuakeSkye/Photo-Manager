import { describe, expect, test } from 'vitest';

import type {
  DuplicateMemberResult,
  DuplicateSetResult,
} from '../../shared/commands/duplicate-review';
import {
  applyBulkSelection,
  buildDuplicateSelections,
  getBulkSelectionState,
  selectionBytes,
  toggleMemberSelection,
} from './selection';

function member(
  assetId: string,
  options: Partial<DuplicateMemberResult> = {},
): DuplicateMemberResult {
  return {
    assetId,
    assetVersion: 1,
    role: 'Jpg',
    fullPath: `G:\\${assetId}.jpg`,
    directory: 'G:\\',
    relativePath: `${assetId}.jpg`,
    sizeBytes: 10,
    captureLocalKey: null,
    photoId: null,
    online: true,
    selectable: true,
    suggestedRetain: false,
    ...options,
  };
}

function set(
  setId: string,
  members: readonly DuplicateMemberResult[],
  suggestedRetainedAssetId: string | null,
): DuplicateSetResult {
  return {
    setId,
    role: 'Jpg',
    fileSizeBytes: 10,
    redundantFiles: Math.max(0, members.length - 1),
    reclaimableBytes: Math.max(0, members.length - 1) * 10,
    suggestedRetainedAssetId,
    members,
  };
}

describe('duplicate review selection', () => {
  test('selects every eligible non-suggested member across all sets', () => {
    const sets = [
      set(
        'first',
        [member('keep-a', { suggestedRetain: true }), member('delete-a')],
        'keep-a',
      ),
      set(
        'second',
        [member('keep-b', { suggestedRetain: true }), member('delete-b')],
        'keep-b',
      ),
    ];
    const state = getBulkSelectionState(sets, new Set());
    expect(state.label).toBe('勾选全部非建议保留（2）');
    expect([...applyBulkSelection(state, new Set())]).toEqual([
      'delete-a',
      'delete-b',
    ]);
  });

  test('skips disabled members and sets without a reliable suggestion', () => {
    const sets = [
      set(
        'safe',
        [
          member('keep', { suggestedRetain: true }),
          member('delete'),
          member('offline', { selectable: false }),
        ],
        'keep',
      ),
      set('unsafe', [member('one'), member('two')], null),
    ];
    const state = getBulkSelectionState(sets, new Set());
    expect(state.eligibleAssetIds).toEqual(['delete']);
  });

  test('reports remaining items and then clears every manual selection', () => {
    const sets = [
      set(
        'set',
        [
          member('keep', { suggestedRetain: true }),
          member('delete-a'),
          member('delete-b'),
        ],
        'keep',
      ),
    ];
    const partial = getBulkSelectionState(sets, new Set(['delete-a']));
    expect(partial.label).toBe('勾选剩余非建议保留（1）');
    const selected = applyBulkSelection(partial, new Set(['delete-a']));
    const clear = getBulkSelectionState(sets, selected);
    expect(clear.label).toBe('取消全部勾选（2）');
    expect(applyBulkSelection(clear, new Set([...selected, 'manual']))).toEqual(
      new Set(),
    );
  });

  test('restores the suggestion as retained before applying a bulk selection', () => {
    const sets = [
      set(
        'set',
        [
          member('keep', { suggestedRetain: true }),
          member('delete-a'),
          member('delete-b'),
        ],
        'keep',
      ),
    ];
    const current = new Set(['keep', 'delete-a']);
    const state = getBulkSelectionState(sets, current);
    expect(applyBulkSelection(state, current)).toEqual(
      new Set(['delete-a', 'delete-b']),
    );
  });

  test('refuses to select the last reliable retained member', () => {
    const sets = [
      set(
        'set',
        [member('keep', { suggestedRetain: true }), member('delete')],
        'keep',
      ),
    ];
    const result = toggleMemberSelection(sets, new Set(['delete']), 'keep');
    expect(result.selected).toEqual(new Set(['delete']));
    expect(result.error).toBe('每组重复文件至少需要保留一份。');
  });

  test('uses another unselected member when the suggestion is manually selected', () => {
    const currentSet = set(
      'set',
      [
        member('suggested', { suggestedRetain: true }),
        member('alternate'),
        member('delete'),
      ],
      'suggested',
    );
    const selected = new Set(['suggested', 'delete']);
    expect(buildDuplicateSelections([currentSet], selected)).toEqual([
      expect.objectContaining({
        deletedAssetId: 'suggested',
        retainedAssetId: 'alternate',
      }),
      expect.objectContaining({
        deletedAssetId: 'delete',
        retainedAssetId: 'alternate',
      }),
    ]);
  });

  test('sums selected physical file bytes', () => {
    const members = new Map([
      ['one', member('one', { sizeBytes: 12 })],
      ['two', member('two', { sizeBytes: 30 })],
    ]);
    expect(
      selectionBytes(
        [
          {
            setId: 'set',
            deletedAssetId: 'one',
            deletedAssetVersion: 1,
            retainedAssetId: 'two',
            retainedAssetVersion: 1,
          },
        ],
        members,
      ),
    ).toBe(12);
  });
});
