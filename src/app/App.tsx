import { useCallback, useEffect, useRef, useState } from 'react';

import { LibraryView } from '../features/library/LibraryView';
import { ImportView } from '../features/import/ImportView';
import { PendingDeleteView } from '../features/pending-delete/PendingDeleteView';
import { ScanView } from '../features/scanning/ScanView';
import { SettingsView } from '../features/settings/SettingsView';
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

type Route = 'library' | 'pending-delete' | 'settings' | 'scan' | 'import';

export function App() {
  const [sortMode, setSortMode] = useState<SortMode>('captureDesc');
  const [snapshot, setSnapshot] = useState<LibrarySnapshot | null>(null);
  const [pendingSnapshot, setPendingSnapshot] =
    useState<PendingDeleteSnapshot | null>(null);
  const [selectedPhotoId, setSelectedPhotoId] = useState<string | null>(null);
  const [route, setRoute] = useState<Route>('library');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [mutationPending, setMutationPending] = useState(false);
  const mutationInFlight = useRef(false);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const restoreAfterSettings = useRef<{
    photoId: string | null;
    route: Route;
  } | null>(null);
  const scanReturnRoute = useRef<'library' | 'import'>('library');

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
    if (mutationInFlight.current || mode === sortMode) return;
    void changeSortMode(mode);
  };

  const changeSortMode = async (mode: SortMode) => {
    setLoading(true);
    setError(null);
    try {
      const [nextLibrary, nextPending] = await Promise.all([
        getLibrarySnapshot(mode),
        getPendingDeleteSnapshot(mode),
      ]);
      setSortMode(mode);
      setSnapshot(nextLibrary);
      setPendingSnapshot(nextPending);
      const list =
        route === 'pending-delete' ? nextPending.items : nextLibrary.photos;
      setSelectedPhotoId((current) =>
        current !== null && list.some((photo) => photo.photoId === current)
          ? current
          : (list[0]?.photoId ?? null),
      );
    } catch (reason) {
      setError(readError(reason));
    } finally {
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
          onCancelCurrent={() => void performToggle()}
          onUndoDeletion={() => void performUndo()}
          onRescan={() => void startRescan()}
        />
      )}
      {route === 'settings' && (
        <SettingsView
          snapshot={snapshot}
          isBindingGate={snapshot.binding === null}
          settingsButtonRef={settingsButtonRef}
          onBack={closeSettings}
          onBindingStarted={(result) => void bindingStarted(result)}
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
        />
      )}
      {route === 'scan' && snapshot.scan !== null && (
        <ScanView
          scan={snapshot.scan}
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
    </div>
  );
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
