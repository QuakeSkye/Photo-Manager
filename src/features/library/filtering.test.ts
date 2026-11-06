import { describe, expect, test } from 'vitest';

import type { PhotoSummary } from '../../shared/commands/library';
import {
  DEFAULT_LIBRARY_FILTER,
  buildDateFilterOptions,
  filterLibraryPhotos,
  hasActiveLibraryFilter,
  resolveSelectedPhotoId,
} from './filtering';

const samplePhotos: PhotoSummary[] = [
  {
    photoId: 'p1',
    photoVersion: 1,
    fileName: 'DSCF1042.JPG',
    captureLocalKey: '2026-07-30T09:24:18',
    preview: null,
    deletionState: null,
  },
  {
    photoId: 'p2',
    photoVersion: 1,
    fileName: 'DSCF1039.RAF',
    captureLocalKey: '2026-07-30T10:24:18',
    preview: null,
    deletionState: 'Marked',
  },
  {
    photoId: 'p3',
    photoVersion: 1,
    fileName: 'PORTRAIT_001.JPG',
    captureLocalKey: '2026-07-29T11:00:00',
    preview: null,
    deletionState: null,
  },
  {
    photoId: 'p4',
    photoVersion: 1,
    fileName: 'UNKNOWN_DATE.JPG',
    captureLocalKey: null,
    preview: null,
    deletionState: null,
  },
];

describe('library filtering helpers', () => {
  test('builds date options including actual dates and unknown date bucket', () => {
    const options = buildDateFilterOptions(samplePhotos);
    expect(options).toEqual([
      { value: 'all', label: '全部日期', count: 4 },
      { value: '2026-07-30', label: '2026-07-30', count: 2 },
      { value: '2026-07-29', label: '2026-07-29', count: 1 },
      { value: 'unknown', label: '日期未知', count: 1 },
    ]);
  });

  test('filters by case-insensitive filename keyword and capture date', () => {
    expect(filterLibraryPhotos(samplePhotos, DEFAULT_LIBRARY_FILTER)).toBe(
      samplePhotos,
    );
    expect(hasActiveLibraryFilter(DEFAULT_LIBRARY_FILTER)).toBe(false);

    const byKeyword = filterLibraryPhotos(samplePhotos, {
      searchQuery: 'dscf10',
      dateFilter: 'all',
    });
    expect(byKeyword.map((photo) => photo.photoId)).toEqual(['p1', 'p2']);

    const byDate = filterLibraryPhotos(samplePhotos, {
      searchQuery: '',
      dateFilter: '2026-07-29',
    });
    expect(byDate.map((photo) => photo.photoId)).toEqual(['p3']);

    const byUnknownDate = filterLibraryPhotos(samplePhotos, {
      searchQuery: '',
      dateFilter: 'unknown',
    });
    expect(byUnknownDate.map((photo) => photo.photoId)).toEqual(['p4']);

    const combinedZero = filterLibraryPhotos(samplePhotos, {
      searchQuery: 'portrait',
      dateFilter: '2026-07-30',
    });
    expect(combinedZero).toHaveLength(0);
  });

  test('resolves preferred photo or nearest surviving neighbor in previous order', () => {
    const remaining = [samplePhotos[0], samplePhotos[2], samplePhotos[3]];
    const previousOrder = ['p1', 'p2', 'p3', 'p4'];

    expect(resolveSelectedPhotoId(remaining, 'p3', previousOrder)).toBe('p3');
    expect(resolveSelectedPhotoId(remaining, 'p2', previousOrder)).toBe('p3');
    expect(resolveSelectedPhotoId([samplePhotos[0]], 'p4', previousOrder)).toBe(
      'p1',
    );
    expect(resolveSelectedPhotoId([], 'p1', previousOrder)).toBeNull();
  });
});
