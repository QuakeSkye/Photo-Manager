import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

export interface ImportSourceView {
  readonly volumeName: string;
  readonly displayPath: string;
  readonly recursivelyScansSelectedSubtree: boolean;
}

export interface ImportTargetView {
  readonly displayPath: string;
}

export interface ImportSetupSnapshot {
  readonly source: ImportSourceView | null;
  readonly sourceNeedsReselection: boolean;
  readonly target: ImportTargetView | null;
  readonly canStartPreflight: boolean;
  readonly blockedReason: string | null;
}

export type ImportPreflightPhase =
  'idle' | 'running' | 'cancelling' | 'cancelled' | 'completed' | 'failed';

export interface ImportPreflightStatus {
  readonly phase: ImportPreflightPhase;
  readonly scannedFiles: number;
  readonly supportedFiles: number;
  readonly currentRelativePath: string | null;
  readonly errorMessage: string | null;
}

export interface ImportPreflightSummary {
  readonly plannedGroups: number;
  readonly newGroups: number;
  readonly newFiles: number;
  readonly newBytes: number;
  readonly skippedFiles: number;
  readonly conflictGroups: number;
  readonly unknownDateFiles: number;
  readonly unreadableFiles: number;
}

export interface ImportDraftItem {
  readonly logicalGroup: string;
  readonly fileName: string;
  readonly sourceRelativePath: string;
  readonly mediaKind: 'Jpg' | 'Raf';
  readonly disposition: 'New' | 'Skip' | 'Conflict' | 'Unreadable';
  readonly targetRelativePath: string | null;
  readonly sizeBytes: number;
  readonly dateUnknown: boolean;
  readonly warning: string | null;
}

export interface ProcessIoAuditReport {
  readonly scope: string;
  readonly sourceReadOpens: number;
  readonly sourceDirectoryEnumerations: number;
  readonly sourceMetadataReads: number;
  readonly sourceFullHashes: number;
  readonly sourceByteComparisons: number;
  readonly sourcePathReopens: number;
  readonly targetReadOpens: number;
  readonly targetDirectoryEnumerations: number;
  readonly targetMetadataReads: number;
  readonly targetFullHashes: number;
  readonly targetByteComparisons: number;
  readonly targetPathReopens: number;
  readonly sourceWriteRequests: number;
  readonly targetWriteRequests: number;
}

export interface ImportPreflightDraft {
  readonly sourcePath: string;
  readonly targetPath: string;
  readonly summary: ImportPreflightSummary;
  readonly items: readonly ImportDraftItem[];
  readonly ignoredSuffixes: Readonly<Record<string, number>>;
  readonly draftDigest: string;
  readonly ioAudit: ProcessIoAuditReport;
}

export type ImportExecutionPhase =
  | 'idle'
  | 'validatingPlan'
  | 'awaitingPlanConfirmation'
  | 'awaitingSpace'
  | 'running'
  | 'awaitingSource'
  | 'needsAttention'
  | 'completed'
  | 'completedWithErrors'
  | 'cancelled';

export interface ImportSpaceView {
  readonly plannedNewBytes: number;
  readonly availableBytes: number;
  readonly reserveBytes: number;
  readonly missingBytes: number;
}

export interface ImportExecutionStatus {
  readonly phase: ImportExecutionPhase;
  readonly jobId: string | null;
  readonly attemptOrdinal: number | null;
  readonly completedFiles: number;
  readonly publishedFiles: number;
  readonly skippedFiles: number;
  readonly failedFiles: number;
  readonly notStartedFiles: number;
  readonly currentFile: string | null;
  readonly currentAction: string | null;
  readonly message: string | null;
  readonly updatedPlan: ImportPreflightDraft | null;
  readonly space: ImportSpaceView | null;
  readonly sourceWriteRequests: number;
}

export interface ImportRecoverySummary {
  readonly hasUnfinishedImport: boolean;
  readonly jobId: string | null;
  readonly phase: ImportExecutionPhase;
  readonly completedFiles: number;
  readonly publishedFiles: number;
  readonly unresolvedFiles: number;
  readonly message: string | null;
}

export async function chooseImportSourceFolder(): Promise<string | null> {
  if (useDemoBackend()) return 'E:\\DCIM\\100_FUJI';
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === 'string' ? selected : null;
}

export async function getImportSetup(): Promise<ImportSetupSnapshot> {
  if (useDemoBackend()) return demoSetup();
  return invoke<ImportSetupSnapshot>('get_import_setup');
}

export async function selectImportSource(
  path: string,
): Promise<ImportSetupSnapshot> {
  if (useDemoBackend()) return { ...demoSetup(), source: demoSetup().source };
  return invoke<ImportSetupSnapshot>('select_import_source', { path });
}

export async function startImportPreflight(): Promise<ImportPreflightDraft> {
  if (useDemoBackend()) return demoDraft();
  return invoke<ImportPreflightDraft>('start_import_preflight');
}

export async function getImportPreflightStatus(): Promise<ImportPreflightStatus> {
  if (useDemoBackend()) {
    return {
      phase: 'completed',
      scannedFiles: 7,
      supportedFiles: 6,
      currentRelativePath: 'DCIM\\100_FUJI',
      errorMessage: null,
    };
  }
  return invoke<ImportPreflightStatus>('get_import_preflight_status');
}

export async function cancelImportPreflight(): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('cancel_import_preflight');
}

export async function startImport(
  draftDigest: string,
): Promise<ImportExecutionStatus> {
  if (useDemoBackend()) {
    demoExecution = {
      ...emptyExecution(),
      phase: 'completed',
      jobId: 'demo-import-job',
      attemptOrdinal: 1,
      completedFiles: 5,
      publishedFiles: 3,
      skippedFiles: 1,
      message: '导入完成。',
    };
    return demoExecution;
  }
  return invoke<ImportExecutionStatus>('start_import', { draftDigest });
}

export async function retryImport(): Promise<ImportExecutionStatus> {
  if (useDemoBackend()) return demoExecution;
  return invoke<ImportExecutionStatus>('retry_import');
}

export async function getImportExecutionStatus(): Promise<ImportExecutionStatus> {
  if (useDemoBackend()) return demoExecution;
  return invoke<ImportExecutionStatus>('get_import_execution_status');
}

export async function cancelRemainingImport(): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('cancel_remaining_import');
}

export async function cancelImportBeforeStart(): Promise<ImportExecutionStatus> {
  if (useDemoBackend()) {
    demoExecution = { ...emptyExecution(), phase: 'cancelled' };
    return demoExecution;
  }
  return invoke<ImportExecutionStatus>('cancel_import_before_start');
}

export async function getImportRecoverySummary(): Promise<ImportRecoverySummary> {
  if (useDemoBackend()) {
    return {
      hasUnfinishedImport: false,
      jobId: null,
      phase: 'idle',
      completedFiles: 0,
      publishedFiles: 0,
      unresolvedFiles: 0,
      message: null,
    };
  }
  return invoke<ImportRecoverySummary>('get_import_recovery_summary');
}

function useDemoBackend(): boolean {
  return typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);
}

let demoExecution: ImportExecutionStatus = emptyExecution();

function emptyExecution(): ImportExecutionStatus {
  return {
    phase: 'idle',
    jobId: null,
    attemptOrdinal: null,
    completedFiles: 0,
    publishedFiles: 0,
    skippedFiles: 0,
    failedFiles: 0,
    notStartedFiles: 0,
    currentFile: null,
    currentAction: null,
    message: null,
    updatedPlan: null,
    space: null,
    sourceWriteRequests: 0,
  };
}

function demoSetup(): ImportSetupSnapshot {
  return {
    source: {
      volumeName: 'FUJIFILM_SD (E:)',
      displayPath: 'E:\\DCIM\\100_FUJI',
      recursivelyScansSelectedSubtree: true,
    },
    sourceNeedsReselection: false,
    target: { displayPath: 'G:\\照片' },
    canStartPreflight: true,
    blockedReason: null,
  };
}

function demoDraft(): ImportPreflightDraft {
  return {
    sourcePath: 'E:\\DCIM\\100_FUJI',
    targetPath: 'G:\\照片',
    summary: {
      plannedGroups: 3,
      newGroups: 2,
      newFiles: 3,
      newBytes: 67_108_864,
      skippedFiles: 1,
      conflictGroups: 1,
      unknownDateFiles: 1,
      unreadableFiles: 1,
    },
    items: [
      {
        logicalGroup: 'DSCF2041',
        fileName: 'DSCF2041.JPG',
        sourceRelativePath: 'DSCF2041.JPG',
        mediaKind: 'Jpg',
        disposition: 'New',
        targetRelativePath: '2026\\20260729\\DSCF2041.JPG',
        sizeBytes: 8_388_608,
        dateUnknown: false,
        warning: null,
      },
      {
        logicalGroup: 'DSCF2041',
        fileName: 'DSCF2041.RAF',
        sourceRelativePath: 'raw\\DSCF2041.RAF',
        mediaKind: 'Raf',
        disposition: 'New',
        targetRelativePath: '2026\\20260729\\raw\\DSCF2041.RAF',
        sizeBytes: 50_331_648,
        dateUnknown: false,
        warning: null,
      },
      {
        logicalGroup: 'DSCF2039',
        fileName: 'DSCF2039.JPG',
        sourceRelativePath: 'DSCF2039.JPG',
        mediaKind: 'Jpg',
        disposition: 'Skip',
        targetRelativePath: '2026\\20260728\\DSCF2039.JPG',
        sizeBytes: 8_388_608,
        dateUnknown: false,
        warning: null,
      },
      {
        logicalGroup: 'DSCF2040',
        fileName: 'DSCF2040.JPG',
        sourceRelativePath: 'DSCF2040.JPG',
        mediaKind: 'Jpg',
        disposition: 'Conflict',
        targetRelativePath: 'Unknown-Date\\DSCF2040_2.JPG',
        sizeBytes: 8_388_608,
        dateUnknown: true,
        warning: null,
      },
      {
        logicalGroup: '来源根目录',
        fileName: 'BROKEN.RAF',
        sourceRelativePath: 'BROKEN.RAF',
        mediaKind: 'Raf',
        disposition: 'Unreadable',
        targetRelativePath: null,
        sizeBytes: 0,
        dateUnknown: false,
        warning: '无法完整读取照片内容。',
      },
    ],
    ignoredSuffixes: { '.png': 1 },
    draftDigest: 'demo-only-not-rendered',
    ioAudit: {
      scope: 'application-process/import-preflight',
      sourceReadOpens: 10,
      sourceDirectoryEnumerations: 2,
      sourceMetadataReads: 20,
      sourceFullHashes: 6,
      sourceByteComparisons: 1,
      sourcePathReopens: 1,
      targetReadOpens: 2,
      targetDirectoryEnumerations: 3,
      targetMetadataReads: 4,
      targetFullHashes: 1,
      targetByteComparisons: 1,
      targetPathReopens: 1,
      sourceWriteRequests: 0,
      targetWriteRequests: 0,
    },
  };
}

export interface ImportRecoveryItem {
  readonly ownershipId: string;
  readonly sourceItemId: string;
  readonly sourcePath: string;
  readonly retainedPath: string;
  readonly observedBytes: number | null;
  readonly state: string;
  readonly excluded: boolean;
  readonly canExclude: boolean;
  readonly targetPath?: string | null;
}
export async function getImportRecoveryItems(): Promise<ImportRecoveryItem[]> {
  if (useDemoBackend()) return [];
  return invoke('get_import_recovery_items');
}
export async function excludeImportRecoveryItems(
  ownershipIds: string[],
): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('exclude_import_recovery_items', { ownershipIds });
}
export async function reselectImportSource(
  path: string,
): Promise<ImportSetupSnapshot> {
  if (useDemoBackend()) return demoSetup();
  return invoke('reselect_import_source', { path });
}
