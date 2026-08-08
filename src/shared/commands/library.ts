import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

export type SortMode = 'captureDesc' | 'captureAsc';
export type ScanKind = 'full' | 'incremental';
export type ScanPhase =
  'running' | 'cancelling' | 'cancelled' | 'failed' | 'completed';

export interface PreviewReference {
  readonly assetId: string;
  readonly assetVersion: number;
  readonly mediaKind: 'Jpg' | 'Raf';
  readonly state: 'JpegSource' | 'RafEmbeddedJpeg' | 'Unavailable';
}

export interface PhotoSummary {
  readonly photoId: string;
  readonly photoVersion: number;
  readonly fileName: string;
  readonly captureLocalKey: string | null;
  readonly preview: PreviewReference | null;
  readonly deletionState: 'Marked' | 'Problem' | null;
}

export interface PendingDeleteSummary {
  readonly totalCount: number;
  readonly executableCount: number;
  readonly problemCount: number;
  readonly needsAttentionCount: number;
  readonly executableFileCount: number;
  readonly executableBytes: number;
}

export type DeletionProblemKind =
  | 'RootOffline'
  | 'PhotoVersionChanged'
  | 'MembersChanged'
  | 'FileMissing'
  | 'FileIdentityChanged'
  | 'UnsupportedHardLink'
  | 'RetryableFailed'
  | 'NeedsAttention'
  | 'Partial';

export interface DeletionProblem {
  readonly lifecycle: 'Open' | 'Resolved' | 'Superseded';
  readonly kind: DeletionProblemKind;
  readonly message: string;
  readonly canRescan: boolean;
  readonly canRebind: boolean;
}

export interface PendingDeleteItem extends PhotoSummary {
  readonly jpegPresent: boolean;
  readonly rafPresent: boolean;
  readonly relativeDirectory: string;
  readonly physicalFileCount: number;
  readonly totalSizeBytes: number;
  readonly problem: DeletionProblem | null;
}

export interface PendingDeleteSnapshot {
  readonly summary: PendingDeleteSummary;
  readonly items: readonly PendingDeleteItem[];
}

export interface DeletionMutationResult {
  readonly photoId: string;
  readonly marked: boolean;
  readonly summary: PendingDeleteSummary;
  readonly undoneOperationId: string | null;
}

export interface ScanStatus {
  readonly kind: ScanKind;
  readonly phase: ScanPhase;
  readonly rootPath: string;
  readonly directoriesChecked: number;
  readonly filesChecked: number;
  readonly discoveredAssets: number;
  readonly ignoredFiles: number;
  readonly warningCount: number;
  readonly currentRelativePath: string | null;
  readonly errorMessage: string | null;
}

export interface LibraryBinding {
  readonly displayName: string;
  readonly displayPath: string;
  readonly status: 'Online' | 'Offline' | 'IdentityMismatch';
}

export interface ScanReportSummary {
  readonly startedAt: number;
  readonly completedAt: number | null;
  readonly result: string | null;
  readonly itemCount: number | null;
}

export interface LibrarySnapshot {
  readonly binding: LibraryBinding | null;
  readonly photos: readonly PhotoSummary[];
  readonly restoredPhotoId: string | null;
  readonly scan: ScanStatus | null;
  readonly pendingReviewCount: number;
  readonly pendingDeleteSummary: PendingDeleteSummary;
  readonly reports: readonly ScanReportSummary[];
}

export interface AssetView {
  readonly assetId: string;
  readonly assetVersion: number;
  readonly sizeBytes: number;
  readonly fileTimeMs: number;
  readonly previewState: PreviewReference['state'];
}

export interface PhotoDetail {
  readonly photoId: string;
  readonly photoVersion: number;
  readonly fileName: string;
  readonly captureLocalKey: string | null;
  readonly fileTimeMs: number | null;
  readonly cameraModel: string | null;
  readonly cameraSerial: string | null;
  readonly jpeg: AssetView | null;
  readonly raf: AssetView | null;
  readonly totalSizeBytes: number;
  readonly preview: PreviewReference | null;
}

export interface LibraryCandidate {
  readonly displayPath: string;
  readonly reconnectsExistingLibrary: boolean;
}

export interface BindResult {
  readonly displayPath: string;
  readonly scanKind: ScanKind;
}

const previewCache = new Map<string, Promise<string>>();

export async function getLibrarySnapshot(
  sortMode: SortMode,
): Promise<LibrarySnapshot> {
  if (useDemoBackend()) return demoSnapshot(sortMode);
  return invoke<LibrarySnapshot>('get_library_snapshot', { sortMode });
}

export async function getPhotoDetail(
  photoId: string,
  photoVersion: number,
): Promise<PhotoDetail> {
  if (useDemoBackend()) return demoDetail(photoId, photoVersion);
  return invoke<PhotoDetail>('get_photo_detail', { photoId, photoVersion });
}

export async function getPendingDeleteSnapshot(
  sortMode: SortMode,
): Promise<PendingDeleteSnapshot> {
  if (useDemoBackend()) return demoPendingSnapshot(sortMode);
  return invoke<PendingDeleteSnapshot>('get_pending_delete_snapshot', {
    sortMode,
  });
}

export async function toggleDeletionMark(
  photoId: string,
  photoVersion: number,
  operationId: string,
): Promise<DeletionMutationResult> {
  if (useDemoBackend())
    return demoToggleDeletionMark(photoId, photoVersion, operationId);
  return invoke<DeletionMutationResult>('toggle_deletion_mark', {
    photoId,
    photoVersion,
    operationId,
  });
}

export async function undoDeletionMark(
  operationId: string,
): Promise<DeletionMutationResult> {
  if (useDemoBackend()) return demoUndoDeletionMark(operationId);
  return invoke<DeletionMutationResult>('undo_deletion_mark', { operationId });
}

export function getPhotoPreview(reference: PreviewReference): Promise<string> {
  if (reference.state === 'Unavailable') {
    return Promise.reject(new Error('RAF 内嵌预览不可用'));
  }
  const key = `${reference.assetId}:${reference.assetVersion}`;
  const cached = previewCache.get(key);
  if (cached !== undefined) return cached;
  const request = useDemoBackend()
    ? Promise.resolve(demoImage(reference.assetId))
    : invoke<{ dataUrl: string }>('get_photo_preview', {
        assetId: reference.assetId,
        assetVersion: reference.assetVersion,
      }).then((payload) => payload.dataUrl);
  previewCache.set(key, request);
  request.catch(() => previewCache.delete(key));
  return request;
}

export async function saveViewingCursor(
  sortMode: SortMode,
  photoId: string,
): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('save_viewing_cursor', { sortMode, photoId });
}

export async function chooseLibraryFolder(): Promise<string | null> {
  if (useDemoBackend()) return 'G:\\照片\\Fuji Library';
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === 'string' ? selected : null;
}

export async function validateLibraryFolder(
  path: string,
): Promise<LibraryCandidate> {
  if (useDemoBackend()) {
    return {
      displayPath: path,
      reconnectsExistingLibrary:
        path === demoSnapshot('captureDesc').binding?.displayPath,
    };
  }
  return invoke<LibraryCandidate>('validate_library_folder', { path });
}

export async function bindLibraryFolder(path: string): Promise<BindResult> {
  if (useDemoBackend()) return { displayPath: path, scanKind: 'full' };
  return invoke<BindResult>('bind_library_folder', { path });
}

export async function getScanStatus(): Promise<ScanStatus | null> {
  if (useDemoBackend()) return demoSnapshot('captureDesc').scan;
  return invoke<ScanStatus | null>('get_scan_status');
}

export async function cancelLibraryScan(): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('cancel_library_scan');
}

export async function retryLibraryScan(): Promise<void> {
  if (useDemoBackend()) return;
  await invoke('retry_library_scan');
}

function useDemoBackend(): boolean {
  return typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);
}

function scenario(): string {
  return (
    new URLSearchParams(window.location.search).get('scenario') ?? 'library'
  );
}

function demoSnapshot(sortMode: SortMode): LibrarySnapshot {
  const selectedScenario = scenario();
  if (selectedScenario === 'unbound') {
    return {
      binding: null,
      photos: [],
      restoredPhotoId: null,
      scan: null,
      pendingReviewCount: 0,
      pendingDeleteSummary: emptyPendingSummary(),
      reports: [],
    };
  }
  const photos = demoPhotos();
  const sorted =
    sortMode === 'captureAsc'
      ? [...photos.slice(0, -1)].reverse().concat(photos.at(-1) ?? [])
      : photos;
  let scan: ScanStatus | null = null;
  if (selectedScenario.startsWith('scan-')) {
    const incremental = selectedScenario.startsWith('scan-incremental');
    const phase = (
      incremental
        ? selectedScenario.replace(/^scan-incremental-?/, '') || 'running'
        : selectedScenario.replace('scan-', '')
    ) as ScanPhase;
    scan = {
      kind: incremental ? 'incremental' : 'full',
      phase,
      rootPath: 'G:\\照片\\2026 Fuji 图库',
      directoriesChecked: 128,
      filesChecked: 2486,
      discoveredAssets: 1824,
      ignoredFiles: 18,
      warningCount: phase === 'failed' ? 3 : 1,
      currentRelativePath: '2026\\07\\30\\家庭旅行与一个非常长的目录名称',
      errorMessage:
        phase === 'failed' ? '图库连接中断，剩余目录未能可靠枚举。' : null,
    };
  }
  const deletion = demoDeletionState();
  const summary = demoPendingSummary(deletion);
  return {
    binding: {
      displayName: 'Fuji Library',
      displayPath:
        selectedScenario === 'long'
          ? 'G:\\照片\\这是一个用于验证 Windows 长路径换行和可访问性的非常非常长的图库目录\\2026\\家庭与旅行\\FUJIFILM-X-T5-ARCHIVE'
          : 'G:\\照片\\2026 Fuji 图库',
      status: selectedScenario === 'offline' ? 'Offline' : 'Online',
    },
    photos:
      selectedScenario === 'empty'
        ? []
        : sorted.map((photo) => ({
            ...photo,
            deletionState: deletion.problems.has(photo.photoId)
              ? 'Problem'
              : deletion.marks.has(photo.photoId)
                ? 'Marked'
                : null,
          })),
    restoredPhotoId:
      selectedScenario === 'unknown'
        ? (sorted.at(-1)?.photoId ?? null)
        : (sorted[2]?.photoId ?? sorted[0]?.photoId ?? null),
    scan,
    pendingReviewCount: summary.totalCount,
    pendingDeleteSummary: summary,
    reports: [
      {
        startedAt: 1785373200000000,
        completedAt: 1785373265000000,
        result: 'Success',
        itemCount: 2486,
      },
      {
        startedAt: 1785286800000000,
        completedAt: 1785286812000000,
        result: 'Partial',
        itemCount: 211,
      },
    ],
  };
}

function demoPhotos(): PhotoSummary[] {
  const names = [
    'DSCF1042.JPG',
    'DSCF1039.RAF',
    'DSCF1031.JPG',
    'DSCF1028.JPG',
    'DSCF1017.RAF',
    'DSCF1009.JPG',
    'DSCF0998.JPG',
    'DSCF0984.RAF',
    'DSCF0971.JPG',
    'DSCF0955.JPG',
    'DSCF0942.RAF',
    '一段很长的文件名_用于验证详情栏换行_家庭旅行_FUJIFILM_XT5_0001.JPG',
  ];
  return names.map((fileName, index) => ({
    photoId: `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
    photoVersion: 1,
    fileName,
    captureLocalKey:
      index === names.length - 1
        ? null
        : `2026-07-${String(30 - index).padStart(2, '0')}T${String(9 + (index % 8)).padStart(2, '0')}:24:18`,
    preview: {
      assetId: `10000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
      assetVersion: 1,
      mediaKind: fileName.endsWith('.RAF') ? 'Raf' : 'Jpg',
      state:
        index === 4
          ? 'Unavailable'
          : fileName.endsWith('.RAF')
            ? 'RafEmbeddedJpeg'
            : 'JpegSource',
    },
    deletionState: null,
  }));
}

interface DemoDeletionAction {
  readonly photoId: string;
  readonly effect: 'Marked' | 'Unmarked';
}

interface DemoDeletionState {
  readonly scenario: string;
  readonly marks: Set<string>;
  readonly problems: Map<string, DeletionProblem>;
  readonly operations: Map<string, DeletionMutationResult>;
  readonly undo: DemoDeletionAction[];
}

let activeDemoDeletionState: DemoDeletionState | null = null;

function demoDeletionState(): DemoDeletionState {
  const selectedScenario = scenario();
  if (activeDemoDeletionState?.scenario === selectedScenario)
    return activeDemoDeletionState;
  const photos = demoPhotos();
  const marks = new Set<string>();
  const problems = new Map<string, DeletionProblem>();
  if (selectedScenario.includes('pending') || selectedScenario === 'recovery')
    marks.add(photos[0].photoId);
  if (selectedScenario.includes('problem')) {
    marks.add(photos[1].photoId);
    problems.set(photos[1].photoId, {
      lifecycle: 'Open',
      kind: selectedScenario.includes('offline')
        ? 'RootOffline'
        : 'PhotoVersionChanged',
      message: selectedScenario.includes('offline')
        ? '照片文件夹离线；标记已保留，但不能重新标记或执行删除。'
        : '照片版本在标记后发生变化，需要取消失效标记并重新确认。',
      canRescan: !selectedScenario.includes('offline'),
      canRebind: selectedScenario.includes('offline'),
    });
  }
  activeDemoDeletionState = {
    scenario: selectedScenario,
    marks,
    problems,
    operations: new Map(),
    undo: [],
  };
  return activeDemoDeletionState;
}

function demoPendingSummary(state: DemoDeletionState): PendingDeleteSummary {
  const problemCount = [...state.marks].filter((id) =>
    state.problems.has(id),
  ).length;
  const executableCount = state.marks.size - problemCount;
  return {
    totalCount: state.marks.size,
    executableCount,
    problemCount,
    needsAttentionCount: 0,
    executableFileCount: executableCount,
    executableBytes: executableCount * 18_420_000,
  };
}

function emptyPendingSummary(): PendingDeleteSummary {
  return {
    totalCount: 0,
    executableCount: 0,
    problemCount: 0,
    needsAttentionCount: 0,
    executableFileCount: 0,
    executableBytes: 0,
  };
}

function demoPendingSnapshot(sortMode: SortMode): PendingDeleteSnapshot {
  const state = demoDeletionState();
  const photos = demoSnapshot(sortMode).photos;
  return {
    summary: demoPendingSummary(state),
    items: photos
      .filter((photo) => state.marks.has(photo.photoId))
      .map((photo) => {
        const detail = demoDetail(photo.photoId, photo.photoVersion);
        return {
          ...photo,
          jpegPresent: detail.jpeg !== null,
          rafPresent: detail.raf !== null,
          relativeDirectory: '2026\\07\\30',
          physicalFileCount:
            Number(detail.jpeg !== null) + Number(detail.raf !== null),
          totalSizeBytes: detail.totalSizeBytes,
          problem: state.problems.get(photo.photoId) ?? null,
        };
      }),
  };
}

function demoToggleDeletionMark(
  photoId: string,
  _photoVersion: number,
  operationId: string,
): DeletionMutationResult {
  const state = demoDeletionState();
  const prior = state.operations.get(operationId);
  if (prior !== undefined) return prior;
  if (scenario().includes('mark-failure')) {
    throw demoError('待删除标记未能可靠保存，请重试。');
  }
  const wasMarked = state.marks.has(photoId);
  if (!wasMarked && scenario().includes('offline')) {
    throw demoError('照片文件夹离线时不能新增或恢复待删除标记。');
  }
  if (wasMarked) {
    state.marks.delete(photoId);
    state.problems.delete(photoId);
  } else {
    state.marks.add(photoId);
  }
  state.undo.push({ photoId, effect: wasMarked ? 'Unmarked' : 'Marked' });
  const result: DeletionMutationResult = {
    photoId,
    marked: !wasMarked,
    summary: demoPendingSummary(state),
    undoneOperationId: null,
  };
  state.operations.set(operationId, result);
  return result;
}

function demoUndoDeletionMark(operationId: string): DeletionMutationResult {
  const state = demoDeletionState();
  const prior = state.operations.get(operationId);
  if (prior !== undefined) return prior;
  const action = state.undo.at(-1);
  if (action === undefined)
    throw demoError('当前会话没有可撤销的待删除标记操作。');
  if (action.effect === 'Unmarked' && scenario().includes('offline'))
    throw demoError('照片文件夹离线时不能新增或恢复待删除标记。');
  state.undo.pop();
  const marked = action.effect === 'Unmarked';
  if (marked) state.marks.add(action.photoId);
  else state.marks.delete(action.photoId);
  const result = {
    photoId: action.photoId,
    marked,
    summary: demoPendingSummary(state),
    undoneOperationId: 'demo-operation',
  };
  state.operations.set(operationId, result);
  return result;
}

export function resetDemoDeletionState(): void {
  activeDemoDeletionState = null;
}

export function simulateDemoRestart(): void {
  demoDeletionState().undo.length = 0;
}

export function completeDemoRecycle(): void {
  const state = demoDeletionState();
  for (const photoId of [...state.marks]) {
    if (!state.problems.has(photoId)) state.marks.delete(photoId);
  }
  state.undo.length = 0;
}

function demoError(userMessage: string): Error & { userMessage: string } {
  return Object.assign(new Error(userMessage), { userMessage });
}

function demoDetail(photoId: string, photoVersion: number): PhotoDetail {
  const photo =
    demoPhotos().find((item) => item.photoId === photoId) ?? demoPhotos()[0];
  const index = demoPhotos().findIndex(
    (item) => item.photoId === photo.photoId,
  );
  const jpeg = photo.fileName.endsWith('.RAF')
    ? null
    : demoAsset(photo, 18_420_000, 'JpegSource');
  const raf =
    index % 3 === 0 || photo.fileName.endsWith('.RAF')
      ? demoAsset(photo, 82_600_000, photo.preview?.state ?? 'RafEmbeddedJpeg')
      : null;
  return {
    photoId,
    photoVersion,
    fileName:
      scenario() === 'long' && index === 2
        ? 'FUJIFILM_X-T5_2026_07_30_家庭旅行_一段非常非常长且必须完整换行显示的照片文件名_000001.JPG'
        : photo.fileName,
    captureLocalKey: scenario() === 'unknown' ? null : photo.captureLocalKey,
    fileTimeMs: Date.UTC(2026, 6, 30 - Math.max(index, 0), 16, 4, 12),
    cameraModel: index % 4 === 1 ? null : 'FUJIFILM X-T5',
    cameraSerial: '3A001234',
    jpeg,
    raf,
    totalSizeBytes: (jpeg?.sizeBytes ?? 0) + (raf?.sizeBytes ?? 0),
    preview: photo.preview,
  };
}

function demoAsset(
  photo: PhotoSummary,
  sizeBytes: number,
  previewState: PreviewReference['state'],
): AssetView {
  return {
    assetId: photo.preview?.assetId ?? photo.photoId,
    assetVersion: 1,
    sizeBytes,
    fileTimeMs: Date.UTC(2026, 6, 30, 16, 4, 12),
    previewState,
  };
}

function demoImage(seed: string): string {
  const number = Number(seed.slice(-2)) || 1;
  const hue = (number * 31) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1600 1067"><defs><linearGradient id="s" x1="0" y1="0" x2="1" y2="1"><stop stop-color="hsl(${hue} 38% 72%)"/><stop offset=".52" stop-color="hsl(${(hue + 38) % 360} 34% 42%)"/><stop offset="1" stop-color="#17202a"/></linearGradient></defs><rect width="1600" height="1067" fill="url(#s)"/><circle cx="1240" cy="260" r="110" fill="#fff" opacity=".72"/><path d="M0 820L370 470l240 220 250-310 430 440z" fill="#15232d" opacity=".62"/><path d="M0 900l450-300 300 250 300-180 550 280v117H0z" fill="#f4d9b0" opacity=".72"/></svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}
