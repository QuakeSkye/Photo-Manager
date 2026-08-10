import { invoke } from '@tauri-apps/api/core';

export type DuplicateReviewState =
  | 'idle'
  | 'running'
  | 'completedEmpty'
  | 'completedWithDuplicates'
  | 'expired'
  | 'cancelled'
  | 'failed';

export interface DuplicateReviewStatus {
  readonly state: DuplicateReviewState;
  readonly runId: string | null;
  readonly phase: string | null;
  readonly checkedFiles: number;
  readonly candidateFiles: number;
  readonly hashedFiles: number;
  readonly currentRelativePath: string | null;
  readonly familyCount: number;
  readonly expiredReason: string | null;
}

export interface DuplicateReviewSummary {
  readonly equivalenceSets: number;
  readonly jpgOnlyFamilies: number;
  readonly rafOnlyFamilies: number;
  readonly jpgAndRafFamilies: number;
  readonly redundantFiles: number;
  readonly reclaimableBytes: number;
}

export interface DuplicateMemberResult {
  readonly assetId: string;
  readonly assetVersion: number;
  readonly role: 'Jpg' | 'Raf';
  readonly fullPath: string;
  readonly directory: string;
  readonly relativePath: string;
  readonly sizeBytes: number;
  readonly captureLocalKey: string | null;
  readonly photoId: string | null;
  readonly online: boolean;
  readonly selectable: boolean;
  readonly suggestedRetain: boolean;
}

export interface DuplicateSetResult {
  readonly setId: string;
  readonly role: 'Jpg' | 'Raf';
  readonly fileSizeBytes: number;
  readonly redundantFiles: number;
  readonly reclaimableBytes: number;
  readonly suggestedRetainedAssetId: string | null;
  readonly members: readonly DuplicateMemberResult[];
}

export interface DuplicateFamilyResult {
  readonly familyId: string;
  readonly jpg: DuplicateSetResult | null;
  readonly raf: DuplicateSetResult | null;
}

export interface DuplicateReviewResults {
  readonly runId: string | null;
  readonly expired: boolean;
  readonly expiredReason: string | null;
  readonly summary: DuplicateReviewSummary;
  readonly families: readonly DuplicateFamilyResult[];
}

export interface DuplicateSubsetSelection {
  readonly setId: string;
  readonly deletedAssetId: string;
  readonly deletedAssetVersion: number;
  readonly retainedAssetId: string;
  readonly retainedAssetVersion: number;
}

export function getDuplicateReviewStatus(): Promise<DuplicateReviewStatus> {
  return invoke('get_duplicate_review_status');
}

export function startDuplicateReviewScan(): Promise<DuplicateReviewStatus> {
  return invoke('start_duplicate_review_scan');
}

export function cancelDuplicateReviewScan(): Promise<DuplicateReviewStatus> {
  return invoke('cancel_duplicate_review_scan');
}

export function getDuplicateReviewResults(): Promise<DuplicateReviewResults> {
  return invoke('get_duplicate_review_results');
}

export function openDuplicateAssetLocation(
  assetId: string,
  assetVersion: number,
): Promise<void> {
  return invoke('open_duplicate_asset_location', { assetId, assetVersion });
}

export function addDuplicateSubsetToPendingDelete(
  runId: string,
  selections: readonly DuplicateSubsetSelection[],
): Promise<void> {
  return invoke('add_duplicate_subset_to_pending_delete', {
    request: { runId, operationId: crypto.randomUUID(), selections },
  });
}
