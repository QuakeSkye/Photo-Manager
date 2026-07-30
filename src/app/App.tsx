import { useCallback, useEffect, useRef, useState } from 'react';

import { LibraryView } from '../features/library/LibraryView';
import { ScanView } from '../features/scanning/ScanView';
import { SettingsView } from '../features/settings/SettingsView';
import {
  getLibrarySnapshot,
  getScanStatus,
  saveViewingCursor,
  type BindResult,
  type LibrarySnapshot,
  type SortMode,
} from '../shared/commands/library';

type Route = 'library' | 'settings' | 'scan';

export function App() {
  const [sortMode, setSortMode] = useState<SortMode>('captureDesc');
  const [snapshot, setSnapshot] = useState<LibrarySnapshot | null>(null);
  const [selectedPhotoId, setSelectedPhotoId] = useState<string | null>(null);
  const [route, setRoute] = useState<Route>('library');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const restoreAfterSettings = useRef<{
    photoId: string | null;
    route: Route;
  } | null>(null);

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
    void load(sortMode).then((next) => {
      if (next === null) return;
      const stalePendingRoute = window.location.hash.includes('pending-delete');
      if (stalePendingRoute) window.history.replaceState(null, '', '#/library');
      if (next.binding === null) {
        setRoute('settings');
      } else if (next.scan?.kind === 'full') {
        setRoute('scan');
      } else {
        setRoute('library');
      }
    });
  }, [load, sortMode]);

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

  const selectedIndex =
    snapshot?.photos.findIndex((photo) => photo.photoId === selectedPhotoId) ??
    -1;
  const selectIndex = useCallback((index: number) => {
    setSnapshot((current) => {
      if (current === null || current.photos.length === 0) return current;
      const bounded = Math.max(0, Math.min(current.photos.length - 1, index));
      setSelectedPhotoId(current.photos[bounded]?.photoId ?? null);
      return current;
    });
  }, []);

  const navigate = useCallback(
    (delta: number) => {
      if (selectedIndex < 0) return;
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
        route !== 'library' ||
        event.repeat ||
        event.ctrlKey ||
        event.altKey ||
        event.metaKey
      )
        return;
      const target = event.target as HTMLElement | null;
      if (
        target instanceof Element &&
        target.closest('input, select, textarea, button, [role="dialog"]') !==
          null
      )
        return;
      if (event.key === 'ArrowLeft') {
        event.preventDefault();
        navigate(-1);
      } else if (event.key === 'ArrowRight') {
        event.preventDefault();
        navigate(1);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [navigate, route]);

  const openSettings = () => {
    restoreAfterSettings.current = { photoId: selectedPhotoId, route };
    setRoute('settings');
  };

  const closeSettings = () => {
    const restore = restoreAfterSettings.current;
    const target =
      restore?.route === 'scan' && snapshot?.scan !== null ? 'scan' : 'library';
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
    setSortMode(mode);
    setLoading(true);
  };

  const bindingStarted = async (result: BindResult) => {
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
      setRoute(result.scanKind === 'full' ? 'scan' : 'library');
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
          onOpenScan={() => setRoute('scan')}
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
          onEnterLibrary={() => void enterLibrary()}
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
