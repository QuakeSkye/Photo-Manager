import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { LibraryView } from '../features/library/LibraryView';
import {
  DEFAULT_LIBRARY_FILTER,
  buildDateFilterOptions,
  filterLibraryPhotos,
  hasActiveLibraryFilter,
  resolveSelectedPhotoId,
  type DateFilterValue,
  type LibraryFilterState,
} from '../features/library/filtering';
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

interface SavedLibraryContext {
  readonly photoId: string | null;
  readonly orderedPhotoIds: readonly string[];
  readonly sortMode: SortMode;
  readonly filter: LibraryFilterState;
}

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
  const [libraryFilter, setLibraryFilter] = useState<LibraryFilterState>(
    DEFAULT_LIBRARY_FILTER,
  );
  const [snapshot, setSnapshot] = useState<LibrarySnapshot | null>(null);
  const [pendingSnapshot, setPendingSnapshot] =
    useState<PendingDeleteSnapshot | null>(null);
  const [selectedPhotoId, setSelectedPhotoId] = useState<string | null>(null);
  const [route, setRoute] = useState<Route>('library');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [feedbackMessage, setFeedbackMessage] = useState<string | null>(null);
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
  const duplicateStatusRequestId = useRef(0);
  const mutationInFlight = useRef(false);
  const sortInFlight = useRef(false);
  const feedbackTimerRef = useRef<number | null>(null);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const lastSelectedLibraryPhotoIdRef = useRef<string | null>(null);
  const savedLibraryContextRef = useRef<SavedLibraryContext>({
    photoId: null,
    orderedPhotoIds: [],
    sortMode: 'captureDesc',
    filter: DEFAULT_LIBRARY_FILTER,
  });
  const restoreAfterSettings = useRef<{
    photoId: string | null;
    route: Route;
  } | null>(null);
  const scanReturnRoute = useRef<'library' | 'import'>('library');

  const showFeedback = useCallback((message: string) => {
    if (feedbackTimerRef.current !== null) {
      window.clearTimeout(feedbackTimerRef.current);
    }
    setFeedbackMessage(message);
    feedbackTimerRef.current = window.setTimeout(() => {
      setFeedbackMessage(null);
      feedbackTimerRef.current = null;
    }, 1800);
  }, []);

  useEffect(() => {
    return () => {
      if (feedbackTimerRef.current !== null) {
        window.clearTimeout(feedbackTimerRef.current);
      }
    };
  }, []);

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

  const applyDuplicateReviewStatus = useCallback(
    (status: DuplicateReviewStatus) => {
      duplicateStatusRequestId.current += 1;
      setDuplicateReviewStatus(status);
    },
    [],
  );

  const refreshDuplicateReviewStatus = useCallback(async () => {
    const requestId = ++duplicateStatusRequestId.current;
    try {
      const status = await getDuplicateReviewStatus();
      if (requestId === duplicateStatusRequestId.current) {
        setDuplicateReviewStatus(status);
      }
    } catch {
      // A transient status read must not replace the last confirmed state.
    }
  }, []);

  useEffect(() => {
    void refreshSafetyStatus();
    const timer = window.setInterval(() => void refreshSafetyStatus(), 2000);
    return () => window.clearInterval(timer);
  }, [refreshSafetyStatus]);

  useEffect(() => {
    void refreshDuplicateReviewStatus();
    const timer = window.setInterval(
      () => void refreshDuplicateReviewStatus(),
      duplicateReviewStatus.state === 'running' ? 500 : 4000,
    );
    return () => window.clearInterval(timer);
  }, [duplicateReviewStatus.state, refreshDuplicateReviewStatus]);

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
          const initial =
            next.restoredPhotoId ?? next.photos[0]?.photoId ?? null;
          lastSelectedLibraryPhotoIdRef.current = initial;
          return initial;
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

  const dateOptions = useMemo(() => {
    const options = buildDateFilterOptions(snapshot?.photos ?? []);
    const selectedDate = libraryFilter.dateFilter;
    if (!options.some((option) => option.value === selectedDate)) {
      return [
        ...options,
        {
          value: selectedDate,
          label: selectedDate === 'unknown' ? '日期未知' : selectedDate,
          count: 0,
        },
      ];
    }
    return options;
  }, [snapshot?.photos, libraryFilter.dateFilter]);

  const effectiveFilter = libraryFilter;

  const filteredLibraryPhotos = useMemo(
    () => filterLibraryPhotos(snapshot?.photos ?? [], effectiveFilter),
    [effectiveFilter, snapshot?.photos],
  );

  const hasActiveFilter = hasActiveLibraryFilter(effectiveFilter);

  const activePhotos =
    route === 'pending-delete'
      ? (pendingSnapshot?.items ?? [])
      : filteredLibraryPhotos;

  const selectedIndex = activePhotos.findIndex(
    (photo) => photo.photoId === selectedPhotoId,
  );

  const selectIndex = useCallback(
    (index: number) => {
      if (mutationInFlight.current || activePhotos.length === 0) return;
      const bounded = Math.max(0, Math.min(activePhotos.length - 1, index));
      const nextId = activePhotos[bounded]?.photoId ?? null;
      if (route === 'library' && nextId !== null) {
        lastSelectedLibraryPhotoIdRef.current = nextId;
      }
      setSelectedPhotoId(nextId);
    },
    [activePhotos, route],
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
    lastSelectedLibraryPhotoIdRef.current = selectedPhotoId;
    void saveViewingCursor(sortMode, selectedPhotoId).catch((reason) =>
      setError(readError(reason)),
    );
  }, [loading, route, selectedPhotoId, sortMode]);

  useEffect(() => {
    if (route !== 'library' || snapshot === null) return;
    const filteredOrder = filteredLibraryPhotos.map((photo) => photo.photoId);
    savedLibraryContextRef.current = {
      photoId: selectedPhotoId ?? lastSelectedLibraryPhotoIdRef.current,
      orderedPhotoIds:
        filteredOrder.length > 0
          ? filteredOrder
          : snapshot.photos.map((photo) => photo.photoId),
      sortMode,
      filter: effectiveFilter,
    };
  }, [
    effectiveFilter,
    filteredLibraryPhotos,
    route,
    selectedPhotoId,
    snapshot,
    sortMode,
  ]);

  const applyLibraryFilter = useCallback(
    (nextFilter: LibraryFilterState) => {
      if (snapshot === null) {
        setLibraryFilter(nextFilter);
        return;
      }
      if (selectedPhotoId !== null) {
        lastSelectedLibraryPhotoIdRef.current = selectedPhotoId;
      }
      const nextFiltered = filterLibraryPhotos(snapshot.photos, nextFilter);
      const preferred =
        selectedPhotoId ?? lastSelectedLibraryPhotoIdRef.current;
      const nextSelected = resolveSelectedPhotoId(
        nextFiltered,
        preferred,
        savedLibraryContextRef.current.orderedPhotoIds,
        snapshot.restoredPhotoId,
      );
      setLibraryFilter(nextFilter);
      setSelectedPhotoId(nextSelected);
    },
    [selectedPhotoId, snapshot],
  );

  const handleSearchChange = useCallback(
    (searchQuery: string) => {
      applyLibraryFilter({
        ...effectiveFilter,
        searchQuery,
      });
    },
    [applyLibraryFilter, effectiveFilter],
  );

  const handleDateFilterChange = useCallback(
    (dateFilter: DateFilterValue) => {
      applyLibraryFilter({
        ...effectiveFilter,
        dateFilter,
      });
    },
    [applyLibraryFilter, effectiveFilter],
  );

  const handleClearFilters = useCallback(() => {
    if (snapshot === null) {
      setLibraryFilter(DEFAULT_LIBRARY_FILTER);
      return;
    }
    const preferred =
      selectedPhotoId ??
      lastSelectedLibraryPhotoIdRef.current ??
      snapshot.restoredPhotoId;
    const nextSelected = resolveSelectedPhotoId(
      snapshot.photos,
      preferred,
      savedLibraryContextRef.current.orderedPhotoIds,
      snapshot.restoredPhotoId,
    );
    setLibraryFilter(DEFAULT_LIBRARY_FILTER);
    setSelectedPhotoId(nextSelected);
  }, [selectedPhotoId, snapshot]);

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
      if (document.querySelector('[role="dialog"]') !== null) return;
      const target = event.target as HTMLElement | null;
      if (
        target instanceof Element &&
        (target.closest(
          'input, select, textarea, [contenteditable=""], [contenteditable="true"], [contenteditable="plaintext-only"], button:not(.filmstrip-item):not(.zoom-status), [role="dialog"]',
        ) !== null ||
          (target instanceof HTMLElement && target.isContentEditable))
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

  const snapshotLibraryContextIfNeeded = () => {
    if (route === 'library' && snapshot !== null) {
      const filteredOrder = filteredLibraryPhotos.map((photo) => photo.photoId);
      savedLibraryContextRef.current = {
        photoId: selectedPhotoId ?? lastSelectedLibraryPhotoIdRef.current,
        orderedPhotoIds:
          filteredOrder.length > 0
            ? filteredOrder
            : snapshot.photos.map((photo) => photo.photoId),
        sortMode,
        filter: effectiveFilter,
      };
    }
  };

  const restoreLibraryContext = async (
    providedSnapshot?: LibrarySnapshot,
    preferredOverridePhotoId?: string | null,
  ) => {
    const saved = savedLibraryContextRef.current;
    const targetSortMode = saved.sortMode;
    const targetFilter = saved.filter;
    let targetSnapshot = providedSnapshot ?? snapshot;

    if (targetSnapshot === null) return;

    if (sortMode !== targetSortMode && providedSnapshot === undefined) {
      try {
        targetSnapshot = await getLibrarySnapshot(targetSortMode);
        setSnapshot(targetSnapshot);
      } catch (reason) {
        setError(readError(reason));
        return;
      }
    } else if (providedSnapshot !== undefined) {
      setSnapshot(providedSnapshot);
    }

    setSortMode(targetSortMode);
    setLibraryFilter(targetFilter);

    const nextFiltered = filterLibraryPhotos(
      targetSnapshot.photos,
      targetFilter,
    );
    const preferred =
      preferredOverridePhotoId !== undefined
        ? preferredOverridePhotoId
        : (saved.photoId ?? lastSelectedLibraryPhotoIdRef.current);

    const resolvedPhotoId = resolveSelectedPhotoId(
      nextFiltered,
      preferred,
      saved.orderedPhotoIds,
      targetSnapshot.restoredPhotoId,
    );

    if (resolvedPhotoId !== null) {
      lastSelectedLibraryPhotoIdRef.current = resolvedPhotoId;
    }
    setSelectedPhotoId(resolvedPhotoId);
    setRoute('library');
    window.history.replaceState(null, '', '#/library');
  };

  const openSettings = () => {
    if (mutationInFlight.current) return;
    snapshotLibraryContextIfNeeded();
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

    if (target === 'library') {
      void restoreLibraryContext(
        undefined,
        restore?.route === 'library' ? restore.photoId : undefined,
      );
    } else {
      if (
        restore !== null &&
        restore.photoId !== null &&
        target === 'pending-delete' &&
        pendingSnapshot?.items.some(
          (photo) => photo.photoId === restore.photoId,
        )
      ) {
        setSelectedPhotoId(restore.photoId);
      }
      setRoute(target);
    }
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
        setSelectedPhotoId((current) =>
          resolveSelectedPhotoId(
            nextPending.items,
            current,
            pendingSnapshot?.items.map((item) => item.photoId),
          ),
        );
      } else {
        const previousOrder = filteredLibraryPhotos.map(
          (photo) => photo.photoId,
        );
        const nextLibrary = await getLibrarySnapshot(mode);
        const nextFiltered = filterLibraryPhotos(
          nextLibrary.photos,
          effectiveFilter,
        );
        setSortMode(mode);
        setSnapshot(nextLibrary);
        const nextSelected = resolveSelectedPhotoId(
          nextFiltered,
          selectedPhotoId ?? lastSelectedLibraryPhotoIdRef.current,
          previousOrder,
          nextLibrary.restoredPhotoId,
        );
        if (nextSelected !== null) {
          lastSelectedLibraryPhotoIdRef.current = nextSelected;
        }
        setSelectedPhotoId(nextSelected);
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
    void restoreLibraryContext();
  };

  const openImport = () => {
    if (mutationInFlight.current || snapshot === null) return;
    snapshotLibraryContextIfNeeded();
    setRoute('import');
    window.history.replaceState(null, '', '#/import');
  };

  const openDuplicateReview = () => {
    if (mutationInFlight.current || snapshot === null) return;
    snapshotLibraryContextIfNeeded();
    setRoute('duplicate-review');
    window.history.replaceState(null, '', '#/duplicate-review');
  };

  const openPendingDelete = async () => {
    if (mutationInFlight.current || snapshot === null) return;
    snapshotLibraryContextIfNeeded();
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

  const refreshDeletionState = async (librarySortOverride?: SortMode) => {
    const targetLibrarySort = librarySortOverride ?? sortMode;
    const [nextLibrary, nextPending] = await Promise.all([
      getLibrarySnapshot(targetLibrarySort),
      getPendingDeleteSnapshot(sortMode),
    ]);
    setSnapshot(nextLibrary);
    setPendingSnapshot(nextPending);
    return { nextLibrary, nextPending };
  };

  const refreshAfterRecycle = async (result: RecycleStatus) => {
    const saved = savedLibraryContextRef.current;
    const { nextLibrary, nextPending } = await refreshDeletionState(
      saved.sortMode,
    );
    if (result.duplicateReconciliation !== 'notApplicable') {
      applyDuplicateReviewStatus(await getDuplicateReviewStatus());
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
    await restoreLibraryContext(nextLibrary);
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
      const saved = savedLibraryContextRef.current;
      const targetLibrarySort =
        sourceRoute === 'pending-delete' && result.summary.totalCount === 0
          ? saved.sortMode
          : sortMode;
      const { nextLibrary, nextPending } =
        await refreshDeletionState(targetLibrarySort);

      if (sourceRoute === 'pending-delete') {
        if (result.summary.totalCount === 0) {
          const nextFiltered = filterLibraryPhotos(
            nextLibrary.photos,
            saved.filter,
          );
          const preferred = nextFiltered.some(
            (photo) => photo.photoId === result.photoId,
          )
            ? result.photoId
            : saved.photoId;
          await restoreLibraryContext(nextLibrary, preferred);
        } else {
          const nextIndex = Math.min(sourceIndex, nextPending.items.length - 1);
          setSelectedPhotoId(nextPending.items[nextIndex]?.photoId ?? null);
        }
      } else {
        const nextFiltered = filterLibraryPhotos(
          nextLibrary.photos,
          effectiveFilter,
        );
        const nextIndex = Math.min(sourceIndex + 1, nextFiltered.length - 1);
        const nextPhotoId =
          nextFiltered[nextIndex]?.photoId ??
          (nextFiltered.some((photo) => photo.photoId === result.photoId)
            ? result.photoId
            : null);
        if (nextPhotoId !== null) {
          lastSelectedLibraryPhotoIdRef.current = nextPhotoId;
        }
        setSelectedPhotoId(nextPhotoId);
        showFeedback(result.marked ? '已标记待删除' : '已取消待删除');
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
    const sourceRoute = route;
    const sourceIndex = selectedIndex;
    mutationInFlight.current = true;
    setMutationPending(true);
    setError(null);
    try {
      const result = await undoDeletionMark(crypto.randomUUID());
      const saved = savedLibraryContextRef.current;
      const targetLibrarySort =
        sourceRoute === 'pending-delete' && result.summary.totalCount === 0
          ? saved.sortMode
          : sortMode;
      const { nextLibrary, nextPending } =
        await refreshDeletionState(targetLibrarySort);

      if (
        sourceRoute === 'pending-delete' &&
        nextPending.summary.totalCount > 0
      ) {
        if (
          result.marked &&
          nextPending.items.some((photo) => photo.photoId === result.photoId)
        ) {
          setSelectedPhotoId(result.photoId);
        } else {
          const nextIndex = Math.min(
            Math.max(0, sourceIndex),
            nextPending.items.length - 1,
          );
          setSelectedPhotoId(nextPending.items[nextIndex]?.photoId ?? null);
        }
      } else if (sourceRoute === 'pending-delete') {
        const nextFiltered = filterLibraryPhotos(
          nextLibrary.photos,
          saved.filter,
        );
        const preferred = nextFiltered.some(
          (photo) => photo.photoId === result.photoId,
        )
          ? result.photoId
          : saved.photoId;
        await restoreLibraryContext(nextLibrary, preferred);
      } else {
        const nextFiltered = filterLibraryPhotos(
          nextLibrary.photos,
          effectiveFilter,
        );
        const nextPhotoId = resolveSelectedPhotoId(
          nextFiltered,
          result.photoId,
          saved.orderedPhotoIds,
          nextLibrary.restoredPhotoId,
        );
        if (nextPhotoId !== null) {
          lastSelectedLibraryPhotoIdRef.current = nextPhotoId;
        }
        setSelectedPhotoId(nextPhotoId);
        showFeedback(
          result.marked ? '已撤销：恢复待删除标记' : '已撤销待删除标记',
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
    setLibraryFilter(DEFAULT_LIBRARY_FILTER);
    const next = await load(sortMode);
    if (next !== null) {
      savedLibraryContextRef.current = {
        photoId: next.restoredPhotoId ?? next.photos[0]?.photoId ?? null,
        orderedPhotoIds: next.photos.map((photo) => photo.photoId),
        sortMode,
        filter: DEFAULT_LIBRARY_FILTER,
      };
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
    const next = await load(savedLibraryContextRef.current.sortMode);
    if (next !== null) await restoreLibraryContext(next);
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
          photos={filteredLibraryPhotos}
          selectedPhotoId={selectedPhotoId}
          selectedIndex={selectedIndex}
          sortMode={sortMode}
          filterControls={{
            searchQuery: effectiveFilter.searchQuery,
            dateFilter: effectiveFilter.dateFilter,
            dateOptions,
            totalPhotoCount: snapshot.photos.length,
            hasActiveFilter,
            onSearchChange: handleSearchChange,
            onDateFilterChange: handleDateFilterChange,
            onClearFilters: handleClearFilters,
          }}
          feedbackMessage={feedbackMessage}
          settingsButtonRef={settingsButtonRef}
          onSelectIndex={selectIndex}
          onNavigate={navigate}
          onSortChange={changeSort}
          onOpenSettings={openSettings}
          onOpenImport={openImport}
          duplicateReviewStatus={duplicateReviewStatus}
          onOpenDuplicateReview={openDuplicateReview}
          importEnabled={safetyStatus?.importGate === 'open'}
          onOpenScan={() => {
            snapshotLibraryContextIfNeeded();
            setRoute('scan');
          }}
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
          onStatusChange={applyDuplicateReviewStatus}
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
          onBack={() => {
            if (
              snapshot.scan?.kind === 'full' &&
              snapshot.scan.phase !== 'completed'
            ) {
              setRoute('scan');
            } else {
              void restoreLibraryContext();
            }
          }}
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
