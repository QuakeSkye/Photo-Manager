import { invoke } from '@tauri-apps/api/core';

import { completeDemoRecycle, getPendingDeleteSnapshot } from './library';

export interface RecycleConfirmation {
  readonly logicalPhotoCount: number;
  readonly physicalFileCount: number;
  readonly totalBytes: number;
  readonly retry: boolean;
  readonly canExecute: boolean;
}

export type RecyclePhase =
  | 'idle'
  | 'running'
  | 'awaitingUser'
  | 'needsAttention'
  | 'completed'
  | 'cancelled';

export interface RecycleStatus {
  readonly phase: RecyclePhase;
  readonly batchId: string | null;
  readonly recycledFiles: number;
  readonly retryableFiles: number;
  readonly needsAttentionFiles: number;
  readonly cancelledFiles: number;
  readonly message: string | null;
}

export async function getRecycleConfirmation(): Promise<RecycleConfirmation> {
  if (useDemoBackend()) {
    const pending = await getPendingDeleteSnapshot('captureDesc');
    return {
      logicalPhotoCount: pending.summary.executableCount,
      physicalFileCount: pending.summary.executableFileCount,
      totalBytes: pending.summary.executableBytes,
      retry: false,
      canExecute: pending.summary.executableCount > 0,
    };
  }
  return invoke<RecycleConfirmation>('get_recycle_confirmation');
}

export async function startRecycle(
  confirmation: RecycleConfirmation,
): Promise<RecycleStatus> {
  if (useDemoBackend()) {
    completeDemoRecycle();
    return completedDemoStatus(confirmation.physicalFileCount);
  }
  return invoke<RecycleStatus>('start_recycle', {
    operationId: crypto.randomUUID(),
    logicalPhotoCount: confirmation.logicalPhotoCount,
    physicalFileCount: confirmation.physicalFileCount,
    totalBytes: confirmation.totalBytes,
  });
}

export async function retryRecycle(
  confirmation: RecycleConfirmation,
): Promise<RecycleStatus> {
  if (useDemoBackend()) {
    completeDemoRecycle();
    return completedDemoStatus(confirmation.physicalFileCount);
  }
  return invoke<RecycleStatus>('retry_recycle', {
    operationId: crypto.randomUUID(),
    logicalPhotoCount: confirmation.logicalPhotoCount,
    physicalFileCount: confirmation.physicalFileCount,
    totalBytes: confirmation.totalBytes,
  });
}

export async function cancelRemainingRecycle(): Promise<RecycleStatus> {
  if (useDemoBackend()) {
    completeDemoRecycle();
    return { ...completedDemoStatus(0), phase: 'cancelled' };
  }
  return invoke<RecycleStatus>('cancel_remaining_recycle', {
    operationId: crypto.randomUUID(),
  });
}

function completedDemoStatus(recycledFiles: number): RecycleStatus {
  return {
    phase: 'completed',
    batchId: 'demo-batch',
    recycledFiles,
    retryableFiles: 0,
    needsAttentionFiles: 0,
    cancelledFiles: 0,
    message: '全部文件已移入 Windows 回收站。',
  };
}

function useDemoBackend(): boolean {
  return !('__TAURI_INTERNALS__' in window);
}
