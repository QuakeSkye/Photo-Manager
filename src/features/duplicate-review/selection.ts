import type {
  DuplicateMemberResult,
  DuplicateSetResult,
  DuplicateSubsetSelection,
} from '../../shared/commands/duplicate-review';

export interface SelectionToggleResult {
  readonly selected: ReadonlySet<string>;
  readonly error: string | null;
}

export interface BulkSelectionState {
  readonly mode: 'select' | 'clear' | 'disabled';
  readonly label: string;
  readonly pressed: boolean;
  readonly changeCount: number;
  readonly eligibleAssetIds: readonly string[];
  readonly protectedAssetIds: readonly string[];
}

export function toggleMemberSelection(
  sets: readonly DuplicateSetResult[],
  current: ReadonlySet<string>,
  assetId: string,
): SelectionToggleResult {
  const set = sets.find((candidate) =>
    candidate.members.some((member) => member.assetId === assetId),
  );
  const member = set?.members.find(
    (candidate) => candidate.assetId === assetId,
  );
  if (set === undefined || member === undefined || !member.selectable) {
    return { selected: current, error: '此文件当前不可选择。' };
  }

  const next = new Set(current);
  if (next.has(assetId)) {
    next.delete(assetId);
    return { selected: next, error: null };
  }

  next.add(assetId);
  if (findRetainedMember(set, next) === null) {
    return {
      selected: current,
      error: '每组重复文件至少需要保留一份。',
    };
  }
  return { selected: next, error: null };
}

export function getBulkSelectionState(
  sets: readonly DuplicateSetResult[],
  selected: ReadonlySet<string>,
): BulkSelectionState {
  const { eligibleAssetIds, protectedAssetIds } = getBulkEligibleMembers(sets);
  if (eligibleAssetIds.length === 0) {
    return {
      mode: 'disabled',
      label: '没有可批量勾选的副本',
      pressed: false,
      changeCount: 0,
      eligibleAssetIds,
      protectedAssetIds,
    };
  }

  const selectedEligibleCount = eligibleAssetIds.filter((assetId) =>
    selected.has(assetId),
  ).length;
  if (selectedEligibleCount === eligibleAssetIds.length) {
    return {
      mode: 'clear',
      label: `取消全部勾选（${selected.size}）`,
      pressed: true,
      changeCount: selected.size,
      eligibleAssetIds,
      protectedAssetIds,
    };
  }

  const remainingCount = eligibleAssetIds.length - selectedEligibleCount;
  return {
    mode: 'select',
    label:
      selectedEligibleCount === 0
        ? `勾选全部非建议保留（${remainingCount}）`
        : `勾选剩余非建议保留（${remainingCount}）`,
    pressed: false,
    changeCount: remainingCount,
    eligibleAssetIds,
    protectedAssetIds,
  };
}

export function applyBulkSelection(
  state: BulkSelectionState,
  selected: ReadonlySet<string>,
): ReadonlySet<string> {
  if (state.mode === 'clear') return new Set();
  if (state.mode === 'disabled') return selected;
  const next = new Set(selected);
  for (const assetId of state.protectedAssetIds) next.delete(assetId);
  for (const assetId of state.eligibleAssetIds) next.add(assetId);
  return next;
}

export function buildDuplicateSelections(
  sets: readonly DuplicateSetResult[],
  selected: ReadonlySet<string>,
): DuplicateSubsetSelection[] {
  return sets.flatMap((set) => {
    const retained = findRetainedMember(set, selected);
    if (retained === null) return [];
    return set.members
      .filter((member) => member.selectable && selected.has(member.assetId))
      .map((member) => ({
        setId: set.setId,
        deletedAssetId: member.assetId,
        deletedAssetVersion: member.assetVersion,
        retainedAssetId: retained.assetId,
        retainedAssetVersion: retained.assetVersion,
      }));
  });
}

export function selectionBytes(
  selections: readonly DuplicateSubsetSelection[],
  members: ReadonlyMap<string, DuplicateMemberResult>,
): number {
  return selections.reduce(
    (sum, item) => sum + (members.get(item.deletedAssetId)?.sizeBytes ?? 0),
    0,
  );
}

function getBulkEligibleMembers(sets: readonly DuplicateSetResult[]): {
  readonly eligibleAssetIds: string[];
  readonly protectedAssetIds: string[];
} {
  const eligibleAssetIds: string[] = [];
  const protectedAssetIds: string[] = [];
  for (const set of sets) {
    const suggested = set.members.find(
      (member) => member.assetId === set.suggestedRetainedAssetId,
    );
    if (suggested === undefined || !suggested.selectable) continue;
    protectedAssetIds.push(suggested.assetId);
    eligibleAssetIds.push(
      ...set.members
        .filter(
          (member) => member.selectable && member.assetId !== suggested.assetId,
        )
        .map((member) => member.assetId),
    );
  }
  return { eligibleAssetIds, protectedAssetIds };
}

function findRetainedMember(
  set: DuplicateSetResult,
  selected: ReadonlySet<string>,
): DuplicateMemberResult | null {
  const suggested = set.members.find(
    (member) =>
      member.assetId === set.suggestedRetainedAssetId &&
      member.selectable &&
      !selected.has(member.assetId),
  );
  if (suggested !== undefined) return suggested;
  return (
    set.members.find(
      (member) => member.selectable && !selected.has(member.assetId),
    ) ?? null
  );
}
