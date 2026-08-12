import { useCallback, useEffect, useRef, useState } from 'react';

import { LibraryView } from '../features/library/LibraryView';
import { ImportView } from '../features/import/ImportView';
import { PendingDeleteView } from '../features/pending-delete/PendingDeleteView';
import { DuplicateReviewView } from '../features/duplicate-review/DuplicateReviewView';
import { ScanView } from '../features/scanning/ScanView';
import { SettingsView } from '../features/settings/SettingsView';
import { Modal } from '../shared/ui/Modal';
import {
  bindLibraryFolder,
  getLibrarySnapshot,
  getPendingDeleteSnapshot,
  getScanStatus,
  retryLibraryScan,
  saveViewingCursor,
  toggleDeletionMark,
  undoDeletionMark,
  type BindResult,
  type LibrarySnapshot,
  type PendingDeleteSnapshot,
  type SortMode,
} from '../shared/commands/library';
import {
  cancelRemainingRecycle,
  getRecycleConfirmation,
  retryRecycle,
  startRecycle,
  type RecycleConfirmation,
  type RecycleStatus,
} from '../shared/commands/recycle';
import { getSafetyStatus, type SafetyStatus } from '../shared/commands/safety';
import { APP_VERSION } from '../shared/version';
import {
  getDuplicateReviewStatus,
  type DuplicateReviewStatus,
} from '../shared/commands/duplicate-review';

type Route =
  | 'library'
  | 'pending-delete'
  | 'duplicate-review'
  | 'settings'
  | 'scan'
  | 'import';

const idleDuplicateStatus: DuplicateReviewStatus = {
  state: 'idle',
  runId: null,
  phase: null,
  checkedFiles: 0,
  candidateFiles: 0,
  hashedFiles: 0,
  currentRelativePath: null,
  familyCount: 0,
  expiredReason: null,
  resultVersionId: null,
  lastReceipt: null,
};

export function App() {
  return (
    <>
      <AppContent />
      <div className="app-version" aria-label={`应用版本 v${APP_VERSION}`}>
        v{APP_VERSION}
      </div>
    </>
  );
}

function AppContent() {
  const [sortMode, setSortMode] = useState<SortMode>('captureDesc');
  const [snapshot, setSnapshot] = useState<LibrarySnapshot | null>(null);
  const [pendingSnapshot, setPendingSnapshot] =
    useState<PendingDeleteSnapshot | null>(null);
  const [selectedPhotoId, setSelectedPhotoId] = useState<string | null>(null);
  const [route, setRoute] = useState<Route>('library');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [mutationPending, setMutationPending] = useState(false);
  const [recycleConfirmation, setRecycleConfirmation] =
    useState<RecycleConfirmation | null>(null);
  const [recycleStatus, setRecycleStatus] = useState<RecycleStatus | null>(
    null,
  );
  const [recycleExecuting, setRecycleExecuting] = useState(false);
  const [safetyStatus, setSafetyStatus] = useState<SafetyStatus | null>(null);
  const [duplicateReviewStatus, setDuplicateReviewStatus] =
    useState<DuplicateReviewStatus>(idleDuplicateStatus);
  const mutationInFlight = useRef(false);
  const sortInFlight = useRef(false);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const restoreAfterSettings = useRef<{
    photoId: string | null;
    route: Route;
  } | null>(null);
  const scanReturnRoute = useRef<'library' | 'import'>('library');

  const refreshSafetyStatus = useCallback(async () => {
    try {
      setSafetyStatus(await getSafetyStatus());
    } catch (reason) {
      setSafetyStatus({
        phase: `v${APP_VERSION}`,
        globalWriteGate: 'closed',
        importGate: 'closed',
        recycleGate: 'closed',
        databaseMode: 'unavailable',
        reason: 'fault_latched',
        message: readError(reason),
      });
    }
  }, []);

  useEffect(() => {
    void refreshSafetyStatus();
    const timer = window.setInterval(() => void refreshSafetyStatus(), 2000);
    return () => window.clearInterval(timer);
  }, [refreshSafetyStatus]);

  useEffect(() => {
    const refresh = () =>
      void getDuplicateReviewStatus()
        .then(setDuplicateReviewStatus)
        .catch(() => undefined);
    refresh();
    const timer = window.setInterval(
      refresh,
      duplicateReviewStatus.state === 'running' ? 500 : 4000,
    );
    return () => window.clearInterval(timer);
  }, [duplicateReviewStatus.state]);

  const load = useCallback(
    async (mode: SortMode, preserveSelection = false) => {
      setError(null);
      try {
        const next = await getLibrarySnapshot(mode);
        setSnapshot(next);
        setSelectedPhotoId((current) => {
          if (
            preserveSelection &&
            current !== null &&
            next.photos.some((photo) => photo.photoId === current)
          ) {
            return current;
          }
          return next.restoredPhotoId ?? next.photos[0]?.photoId ?? null;
        });
        return next;
      } catch (reason) {
        setError(readError(reason));
        return null;
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    void load('captureDesc').then((next) => {
      if (next === null) return;
      window.history.replaceState(null, '', '#/library');
      if (next.binding === null) {
        setRoute('settings');
      } else if (next.scan?.kind === 'full') {
        setRoute('scan');
      } else {
        setRoute('library');
      }
    });
  }, [load]);

  useEffect(() => {
    if (
      snapshot?.scan?.phase !== 'running' &&
      snapshot?.scan?.phase !== 'cancelling'
    )
      return;
    const timer = window.setInterval(() => {
      void getScanStatus()
        .then((scan) =>
          setSnapshot((current) =>
            current === null ? current : { ...current, scan },
          ),
        )
        .catch((reason) => setError(readError(reason)));
    }, 350);
    return () => window.clearInterval(timer);
  }, [snapshot?.scan?.phase]);

  const activePhotos =
    route === 'pending-delete'
      ? (pendingSnapshot?.items ?? [])
      : (snapshot?.photos ?? []);
  const selectedIndex = activePhotos.findIndex(
    (photo) => photo.photoId === selectedPhotoId,
  );
  const selectIndex = useCallback(
    (index: number) => {
      if (mutationInFlight.current || activePhotos.length === 0) return;
      const bounded = Math.max(0, Math.min(activePhotos.length - 1, index));
      setSelectedPhotoId(activePhotos[bounded]?.photoId ?? null);
    },
    [activePhotos],
  );

  const navigate = useCallback(
    (delta: number) => {
      if (mutationInFlight.current || selectedIndex < 0) return;
      selectIndex(selectedIndex + delta);
    },
    [selectIndex, selectedIndex],
  );

  useEffect(() => {
    if (loading || route !== 'library' || selectedPhotoId === null) return;
    void saveViewingCursor(sortMode, selectedPhotoId).catch((reason) =>
      setError(readError(reason)),
    );
  }, [loading, route, selectedPhotoId, sortMode]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        (route !== 'library' && route !== 'pending-delete') ||
        mutationInFlight.current ||
        event.repeat ||
        event.ctrlKey ||
        event.altKey ||
        event.metaKey
      )
        return;
      const target = event.target as HTMLElement | null;
      if (
        target instanceof Element &&
        target.closest(
          'input, select, textarea, button:not(.filmstrip-item), [role="dialog"]',
        ) !== null
      )
        return;
      if (event.key === 'ArrowLeft') {
        event.preventDefault();
        navigate(-1);
      } else if (event.key === 'ArrowRight') {
        event.preventDefault();
        navigate(1);
      } else if (event.key.toLowerCase() === 'd') {
        event.preventDefault();
        void performToggle();
      } else if (event.key.toLowerCase() === 'r') {
        event.preventDefault();
        void performUndo();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  });

  const openSettings = () => {
    if (mutationInFlight.current) return;
    restoreAfterSettings.current = { photoId: selectedPhotoId, route };
    setRoute('settings');
  };

  const closeSettings = () => {
    const restore = restoreAfterSettings.current;
    const target =
      restore?.route === 'scan' && snapshot?.scan !== null
        ? 'scan'
        : restore?.route === 'import'
          ? 'import'
          : restore?.route === 'pending-delete' &&
              (snapshot?.pendingDeleteSummary.totalCount ?? 0) > 0
            ? 'pending-delete'
            : 'library';
    if (
      restore !== null &&
      restore.photoId !== null &&
      snapshot?.photos.some((photo) => photo.photoId === restore.photoId)
    ) {
      setSelectedPhotoId(restore.photoId);
    }
    setRoute(target);
    window.requestAnimationFrame(() => settingsButtonRef.current?.focus());
  };

  const changeSort = (mode: SortMode) => {
    if (mutationInFlight.current || sortInFlight.current || mode === sortMode)
      return;
    void changeSortMode(mode);
  };

  const changeSortMode = async (mode: SortMode) => {
    sortInFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      if (route === 'pending-delete') {
        const [nextLibrary, nextPending] = await Promise.all([
          getLibrarySnapshot(mode),
          getPendingDeleteSnapshot(mode),
        ]);
        setSortMode(mode);
        setSnapshot(nextLibrary);
        setPendingSnapshot(nextPending);
        setSelectedPhotoId(nextPending.items[0]?.photoId ?? null);
      } else {
        const nextLibrary = await getLibrarySnapshot(mode);
        setSortMode(mode);
        setSnapshot(nextLibrary);
        setSelectedPhotoId(nextLibrary.photos[0]?.photoId ?? null);
      }
    } catch (reason) {
      setError(readError(reason));
    } finally {
      sortInFlight.current = false;
      setLoading(false);
    }
  };

  const openLibrary = () => {
    if (mutationInFlight.current || snapshot === null) return;
    setRoute('library');
    window.history.replaceState(null, '', '#/library');
    if (
      selectedPhotoId === null ||
      !snapshot.photos.some((photo) => photo.photoId === selectedPhotoId)
    ) {
      setSelectedPhotoId(
        snapshot.restoredPhotoId ?? snapshot.photos[0]?.photoId ?? null,
      );
    }
  };

  const openImport = () => {
    if (mutationInFlight.current || snapshot === null) return;
    setRoute('import');
    window.history.replaceState(null, '', '#/import');
  };

  const openDuplicateReview = () => {
    if (mutationInFlight.current || snapshot === null) return;
    setRoute('duplicate-review');
    window.history.replaceState(null, '', '#/duplicate-review');
  };

  const openPendingDelete = async () => {
    if (mutationInFlight.current || snapshot === null) return;
    if (snapshot.pendingDeleteSummary.totalCount === 0) {
      openLibrary();
      return;
    }
    try {
      const next = await getPendingDeleteSnapshot(sortMode);
      if (next.summary.totalCount === 0) {
        setSnapshot((current) =>
          current === null
            ? current
            : {
                ...current,
                pendingDeleteSummary: next.summary,
                pendingReviewCount: 0,
              },
        );
        openLibrary();
        return;
      }
      setPendingSnapshot(next);
      setSelectedPhotoId((current) =>
        current !== null &&
        next.items.some((photo) => photo.photoId === current)
          ? current
          : (next.items[0]?.photoId ?? null),
      );
      setRoute('pending-delete');
      window.history.replaceState(null, '', '#/pending-delete');
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const refreshDeletionState = async () => {
    const [nextLibrary, nextPending] = await Promise.all([
      getLibrarySnapshot(sortMode),
      getPendingDeleteSnapshot(sortMode),
    ]);
    setSnapshot(nextLibrary);
    setPendingSnapshot(nextPending);
    return { nextLibrary, nextPending };
  };

  const refreshAfterRecycle = async (result: RecycleStatus) => {
    const { nextLibrary, nextPending } = await refreshDeletionState();
    if (result.duplicateReconciliation !== 'notApplicable') {
      setDuplicateReviewStatus(await getDuplicateReviewStatus());
    }
    const mustReviewProblems =
      result.remainingProblemCount > 0 ||
      result.phase === 'awaitingUser' ||
      result.phase === 'needsAttention';
    if (mustReviewProblems || nextPending.summary.totalCount > 0) {
      setRoute('pending-delete');
      window.history.replaceState(null, '', '#/pending-delete');
      setPendingSnapshot(nextPending);
      setSelectedPhotoId(nextPending.items[0]?.photoId ?? null);
      return;
    }
    if (
      result.duplicateReconciliation === 'applied' ||
      result.duplicateReconciliation === 'rejected'
    ) {
      setRoute('duplicate-review');
      window.history.replaceState(null, '', '#/duplicate-review');
      setSelectedPhotoId(null);
      return;
    }
    setRoute('library');
    window.history.replaceState(null, '', '#/library');
    setSelectedPhotoId(
      nextLibrary.restoredPhotoId ?? nextLibrary.photos[0]?.photoId ?? null,
    );
  };

  const performToggle = async () => {
    if (mutationInFlight.current || selectedIndex < 0) return;
    const current = activePhotos[selectedIndex];
    if (current === undefined) return;
    const sourceRoute = route;
    const sourceIndex = selectedIndex;
    mutationInFlight.current = true;
    setMutationPending(true);
    setError(null);
    try {
      const result = await toggleDeletionMark(
        current.photoId,
        current.photoVersion,
        crypto.randomUUID(),
      );
      const { nextLibrary, nextPending } = await refreshDeletionState();
      if (sourceRoute === 'pending-delete') {
        if (result.summary.totalCount === 0) {
          setRoute('library');
          window.history.replaceState(null, '', '#/library');
          setSelectedPhotoId(
            nextLibrary.photos.some((photo) => photo.photoId === result.photoId)
              ? result.photoId
              : (nextLibrary.restoredPhotoId ??
                  nextLibrary.photos[0]?.photoId ??
                  null),
          );
        } else {
          const nextIndex = Math.min(sourceIndex, nextPending.items.length - 1);
          setSelectedPhotoId(nextPending.items[nextIndex]?.photoId ?? null);
        }
      } else {
        const nextIndex = Math.min(
          sourceIndex + 1,
          nextLibrary.photos.length - 1,
        );
        setSelectedPhotoId(
          nextLibrary.photos[nextIndex]?.photoId ?? result.photoId,
        );
      }
    } catch (reason) {
      setError(readError(reason));
    } finally {
      mutationInFlight.current = false;
      setMutationPending(false);
    }
  };

  const performUndo = async () => {
    if (mutationInFlight.current) return;
    mutationInFlight.current = true;
    setMutationPending(true);
    setError(null);
    try {
      const result = await undoDeletionMark(crypto.randomUUID());
      const { nextLibrary, nextPending } = await refreshDeletionState();
      if (
        route === 'pending-delete' &&
        result.marked &&
        nextPending.items.some((photo) => photo.photoId === result.photoId)
      ) {
        setSelectedPhotoId(result.photoId);
      } else {
        setRoute('library');
        window.history.replaceState(null, '', '#/library');
        setSelectedPhotoId(
          nextLibrary.photos.some((photo) => photo.photoId === result.photoId)
            ? result.photoId
            : (nextLibrary.restoredPhotoId ??
                nextLibrary.photos[0]?.photoId ??
                null),
        );
      }
    } catch (reason) {
      setError(readError(reason));
    } finally {
      mutationInFlight.current = false;
      setMutationPending(false);
    }
  };

  const startRescan = async () => {
    try {
      await retryLibraryScan();
      const next = await load(sortMode, true);
      if (next !== null && next.scan !== null) setRoute('scan');
    } catch (reason) {
      setError(readError(reason));
    }
  };

  const openRecycleConfirmation = async () => {
    if (mutationInFlight.current) return;
    if (safetyStatus?.recycleGate !== 'open') {
      setError(
        safetyStatus?.message ?? 'Recycle 写入安全状态尚未就绪，请稍后重试。',
      );
      return;
    }
    mutationInFlight.current = true;
    setMutationPending(true);
    setError(null);
    try {
      const confirmation = await getRecycleConfirmation();
      if (!confirmation.canExecute) {
        throw new Error('没有可执行的有效待删除照片。');
      }
      setRecycleConfirmation(confirmation);
    } catch (reason) {
      mutationInFlight.current = false;
      setMutationPending(false);
      setError(readError(reason));
    }
  };

  const closeRecycleConfirmation = () => {
    if (recycleExecuting) return;
    setRecycleConfirmation(null);
    mutationInFlight.current = false;
    setMutationPending(false);
  };

  const confirmRecycle = async () => {
    if (recycleConfirmation === null || recycleExecuting) return;
    setRecycleExecuting(true);
    setError(null);
    try {
      const result = recycleConfirmation.retry
        ? await retryRecycle(recycleConfirmation)
        : await startRecycle(recycleConfirmation);
      setRecycleStatus(result);
      setRecycleConfirmation(null);
      await refreshAfterRecycle(result);
    } catch (reason) {
      setError(readError(reason));
      void refreshSafetyStatus();
    } finally {
      setRecycleExecuting(false);
      mutationInFlight.current = false;
      setMutationPending(false);
    }
  };

  const cancelRecycleRemaining = async () => {
    if (mutationInFlight.current) return;
    mutationInFlight.current = true;
    setMutationPending(true);
    setError(null);
    try {
      const result = await cancelRemainingRecycle();
      setRecycleStatus(result);
      await refreshAfterRecycle(result);
    } catch (reason) {
      setError(readError(reason));
    } finally {
      mutationInFlight.current = false;
      setMutationPending(false);
    }
  };

  const bindingStarted = async (
    result: BindResult,
    returnRoute: 'library' | 'import' = 'library',
  ) => {
    scanReturnRoute.current = returnRoute;
    const next = await load(sortMode);
    if (next !== null) {
      setSnapshot({
        ...next,
        scan: next.scan ?? {
          kind: result.scanKind,
          phase: 'running',
          rootPath: result.displayPath,
          directoriesChecked: 0,
          filesChecked: 0,
          discoveredAssets: 0,
          ignoredFiles: 0,
          warningCount: 0,
          currentRelativePath: null,
          errorMessage: null,
        },
      });
      setRoute(result.scanKind === 'full' ? 'scan' : returnRoute);
    }
  };

  const enterLibrary = async () => {
    const next = await load(sortMode);
    if (next !== null) setRoute('library');
  };

  if (loading && snapshot === null)
    return (
      <BootState title="正在打开图库" message="正在恢复只读索引与浏览位置…" />
    );
  if (snapshot === null) {
    return (
      <BootState
        title="无法打开图库"
        message={error ?? '本地只读核心暂时不可用。'}
        error
      />
    );
  }

  return (
    <div className="app-shell" data-route={route}>
      {error !== null && (
        <div className="global-error" role="alert">
          <span>{error}</span>
          <button
            type="button"
            onClick={() => setError(null)}
            aria-label="关闭错误消息"
          >
            ×
          </button>
        </div>
      )}
      {route === 'library' && (
        <LibraryView
          snapshot={snapshot}
          selectedPhotoId={selectedPhotoId}
          selectedIndex={selectedIndex}
          sortMode={sortMode}
          settingsButtonRef={settingsButtonRef}
          onSelectIndex={selectIndex}
          onNavigate={navigate}
          onSortChange={changeSort}
          onOpenSettings={openSettings}
          onOpenImport={openImport}
          duplicateReviewStatus={duplicateReviewStatus}
          onOpenDuplicateReview={openDuplicateReview}
          importEnabled={safetyStatus?.importGate === 'open'}
          onOpenScan={() => setRoute('scan')}
          mutationPending={mutationPending}
          onToggleDeletion={() => void performToggle()}
          onUndoDeletion={() => void performUndo()}
          onOpenPendingDelete={() => void openPendingDelete()}
        />
      )}
      {route === 'pending-delete' && pendingSnapshot !== null && (
        <PendingDeleteView
          snapshot={pendingSnapshot}
          selectedPhotoId={selectedPhotoId}
          selectedIndex={selectedIndex}
          sortMode={sortMode}
          settingsButtonRef={settingsButtonRef}
          mutationPending={mutationPending}
          onSelectIndex={selectIndex}
          onNavigate={navigate}
          onSortChange={changeSort}
          onOpenLibrary={openLibrary}
          onOpenSettings={openSettings}
          onOpenImport={openImport}
          duplicateReviewStatus={duplicateReviewStatus}
          onOpenDuplicateReview={openDuplicateReview}
          onCancelCurrent={() => void performToggle()}
          onUndoDeletion={() => void performUndo()}
          onRescan={() => void startRescan()}
          onExecute={() => void openRecycleConfirmation()}
          onRetryRemaining={() => void openRecycleConfirmation()}
          onCancelRemaining={() => void cancelRecycleRemaining()}
          recycleMessage={
            recycleStatus?.reconciliationMessage ??
            recycleStatus?.message ??
            null
          }
          importEnabled={safetyStatus?.importGate === 'open'}
          recycleEnabled={safetyStatus?.recycleGate === 'open'}
          safetyMessage={safetyStatus?.message}
        />
      )}
      {route === 'duplicate-review' && (
        <DuplicateReviewView
          status={duplicateReviewStatus}
          pendingDeleteSummary={snapshot.pendingDeleteSummary}
          settingsButtonRef={settingsButtonRef}
          onStatusChange={setDuplicateReviewStatus}
          onOpenLibrary={openLibrary}
          onOpenPendingDelete={() => void openPendingDelete()}
          onOpenSettings={openSettings}
          onPendingAdded={async () => {
            const { nextLibrary, nextPending } = await refreshDeletionState();
            setSnapshot(nextLibrary);
            setPendingSnapshot(nextPending);
            setSelectedPhotoId(nextPending.items[0]?.photoId ?? null);
            setRoute('pending-delete');
            window.history.replaceState(null, '', '#/pending-delete');
          }}
        />
      )}
      {route === 'settings' && (
        <SettingsView
          snapshot={snapshot}
          isBindingGate={snapshot.binding === null}
          settingsButtonRef={settingsButtonRef}
          onBack={closeSettings}
          onBindingStarted={(result) => void bindingStarted(result)}
          safetyStatus={safetyStatus}
        />
      )}
      {route === 'import' && (
        <ImportView
          pendingDeleteSummary={snapshot.pendingDeleteSummary}
          settingsButtonRef={settingsButtonRef}
          onOpenLibrary={openLibrary}
          onOpenPendingDelete={() => void openPendingDelete()}
          onOpenSettings={openSettings}
          onBindTarget={async (path) => {
            const result = await bindLibraryFolder(path);
            await bindingStarted(result, 'import');
          }}
          writeEnabled={safetyStatus?.importGate === 'open'}
          safetyMessage={safetyStatus?.message}
        />
      )}
      {route === 'scan' && snapshot.scan !== null && (
        <ScanView
          scan={snapshot.scan}
          safetyStatus={safetyStatus}
          settingsButtonRef={settingsButtonRef}
          onBack={() =>
            setRoute(
              snapshot.scan?.kind === 'full' &&
                snapshot.scan.phase !== 'completed'
                ? 'scan'
                : 'library',
            )
          }
          onEnterLibrary={() => {
            if (scanReturnRoute.current === 'import') {
              void load(sortMode).then((next) => {
                if (next !== null) setRoute('import');
              });
            } else {
              void enterLibrary();
            }
          }}
          onOpenSettings={openSettings}
          onStatusChange={(scan) =>
            setSnapshot((current) =>
              current === null ? current : { ...current, scan },
            )
          }
        />
      )}
      {recycleConfirmation !== null && (
        <Modal
          title={
            recycleConfirmation.retry
              ? '确认重试剩余文件'
              : '确认移入 Windows 回收站'
          }
          confirmLabel={
            recycleConfirmation.retry ? '重试剩余文件' : '移入回收站'
          }
          busy={recycleExecuting}
          confirmDisabled={safetyStatus?.recycleGate !== 'open'}
          onConfirm={() => void confirmRecycle()}
          onCancel={closeRecycleConfirmation}
        >
          <dl>
            <div className="metadata-row">
              <dt>逻辑照片</dt>
              <dd>{recycleConfirmation.logicalPhotoCount}</dd>
            </div>
            <div className="metadata-row">
              <dt>物理 JPG / RAF 文件</dt>
              <dd>{recycleConfirmation.physicalFileCount}</dd>
            </div>
            <div className="metadata-row">
              <dt>合计容量</dt>
              <dd>{formatBytes(recycleConfirmation.totalBytes)}</dd>
            </div>
          </dl>
          <p>
            执行后应用内 R 无效，只能通过 Windows 回收站恢复。应用不会永久删除，
            也不会在回收站不可用时降级。
          </p>
        </Modal>
      )}
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit < 2 ? 0 : 1)} ${units[unit]}`;
}

function BootState({
  title,
  message,
  error = false,
}: {
  title: string;
  message: string;
  error?: boolean;
}) {
  return (
    <main className="boot-state" aria-live="polite">
      <div
        className={error ? 'boot-mark boot-mark-error' : 'boot-mark'}
        aria-hidden="true"
      >
        F
      </div>
      <h1>{title}</h1>
      <p>{message}</p>
    </main>
  );
}

function readError(reason: unknown): string {
  if (
    typeof reason === 'object' &&
    reason !== null &&
    'userMessage' in reason
  ) {
    return String(reason.userMessage);
  }
  return reason instanceof Error ? reason.message : '操作失败，请重试。';
}
