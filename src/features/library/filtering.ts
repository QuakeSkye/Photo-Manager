import type { PhotoSummary } from '../../shared/commands/library';

// ISO capture date, or the special values 'all' and 'unknown'.
export type DateFilterValue = string;

export interface LibraryFilterState {
  readonly dateFilter: DateFilterValue;
}

export interface DateFilterOption {
  readonly value: DateFilterValue;
  readonly label: string;
  readonly count: number;
}

export const DEFAULT_LIBRARY_FILTER: LibraryFilterState = {
  dateFilter: 'all',
};

export function extractCaptureDateKey(
  captureLocalKey: string | null,
): string | null {
  if (captureLocalKey === null || captureLocalKey.length < 10) return null;
  return captureLocalKey.slice(0, 10);
}

export function hasActiveLibraryFilter(filter: LibraryFilterState): boolean {
  return filter.dateFilter !== 'all';
}

export function buildDateFilterOptions(
  photos: readonly PhotoSummary[],
): readonly DateFilterOption[] {
  const dateCounts = new Map<string, number>();
  let unknownCount = 0;

  for (const photo of photos) {
    const dateKey = extractCaptureDateKey(photo.captureLocalKey);
    if (dateKey === null) {
      unknownCount += 1;
    } else {
      dateCounts.set(dateKey, (dateCounts.get(dateKey) ?? 0) + 1);
    }
  }

  const sortedDates = [...dateCounts.keys()].sort((left, right) =>
    right.localeCompare(left),
  );

  const options: DateFilterOption[] = [
    {
      value: 'all',
      label: '全部日期',
      count: photos.length,
    },
    ...sortedDates.map((dateKey) => ({
      value: dateKey,
      label: dateKey,
      count: dateCounts.get(dateKey) ?? 0,
    })),
  ];

  if (unknownCount > 0) {
    options.push({
      value: 'unknown',
      label: '日期未知',
      count: unknownCount,
    });
  }

  return options;
}

export function filterLibraryPhotos(
  photos: readonly PhotoSummary[],
  filter: LibraryFilterState,
): readonly PhotoSummary[] {
  const dateFilter = filter.dateFilter;

  if (dateFilter === 'all') {
    return photos;
  }

  return photos.filter((photo) => {
    if (dateFilter === 'unknown') {
      if (extractCaptureDateKey(photo.captureLocalKey) !== null) return false;
    } else if (dateFilter !== 'all') {
      if (extractCaptureDateKey(photo.captureLocalKey) !== dateFilter) {
        return false;
      }
    }

    return true;
  });
}

export function resolveSelectedPhotoId(
  photos: readonly PhotoSummary[],
  preferredPhotoId: string | null,
  previousOrderedIds?: readonly string[],
  fallbackPhotoId?: string | null,
): string | null {
  if (photos.length === 0) return null;

  const availableIds = new Set(photos.map((photo) => photo.photoId));

  if (preferredPhotoId !== null && availableIds.has(preferredPhotoId)) {
    return preferredPhotoId;
  }

  if (
    preferredPhotoId !== null &&
    previousOrderedIds !== undefined &&
    previousOrderedIds.length > 0
  ) {
    const anchorIndex = previousOrderedIds.indexOf(preferredPhotoId);
    if (anchorIndex >= 0) {
      for (
        let index = anchorIndex + 1;
        index < previousOrderedIds.length;
        index += 1
      ) {
        const candidateId = previousOrderedIds[index];
        if (candidateId !== undefined && availableIds.has(candidateId)) {
          return candidateId;
        }
      }
      for (let index = anchorIndex - 1; index >= 0; index -= 1) {
        const candidateId = previousOrderedIds[index];
        if (candidateId !== undefined && availableIds.has(candidateId)) {
          return candidateId;
        }
      }
    }
  }

  if (
    fallbackPhotoId !== undefined &&
    fallbackPhotoId !== null &&
    availableIds.has(fallbackPhotoId)
  ) {
    return fallbackPhotoId;
  }

  return photos[0]?.photoId ?? null;
}
