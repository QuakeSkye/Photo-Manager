use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::{
    db::migration::map_sqlite_error,
    domain::{DomainError, ErrorCode, LibraryId, RootBindingId, RootId},
    io_scheduler::IoScheduler,
};

use super::{CatalogService, LibraryScanner, ScanProgress, ScanReport, ScanRequest};

pub struct BackgroundScan {
    receiver: Receiver<Result<ScanReport, DomainError>>,
}

impl BackgroundScan {
    pub fn wait(self) -> Result<ScanReport, DomainError> {
        self.receiver
            .recv()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "后台扫描结果通道已关闭。"))?
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanKind {
    Full,
    Incremental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanPhase {
    Running,
    Cancelling,
    Cancelled,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub kind: ScanKind,
    pub phase: ScanPhase,
    pub root_path: String,
    pub directories_checked: usize,
    pub files_checked: usize,
    pub discovered_assets: usize,
    pub ignored_files: usize,
    pub warning_count: usize,
    pub current_relative_path: Option<String>,
    pub error_message: Option<String>,
}

impl ScanStatus {
    fn running(kind: ScanKind, request: &ScanRequest) -> Self {
        Self {
            kind,
            phase: ScanPhase::Running,
            root_path: request.root_path.to_string_lossy().into_owned(),
            directories_checked: 0,
            files_checked: 0,
            discovered_assets: 0,
            ignored_files: 0,
            warning_count: 0,
            current_relative_path: None,
            error_message: None,
        }
    }

    fn apply_progress(&mut self, progress: ScanProgress) {
        self.directories_checked = progress.directories_checked;
        self.files_checked = progress.files_checked;
        self.discovered_assets = progress.observed_assets;
        self.ignored_files = progress.ignored_files;
        self.warning_count = progress.warning_count;
        self.current_relative_path = progress.current_relative_path;
    }
}

#[derive(Clone)]
pub struct ScanCoordinator {
    catalog: Arc<CatalogService>,
    active: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    status: Arc<Mutex<Option<ScanStatus>>>,
    scheduler: Arc<IoScheduler>,
}

impl std::fmt::Debug for ScanCoordinator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScanCoordinator")
            .field("active", &self.active.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ScanCoordinator {
    pub fn new(catalog: Arc<CatalogService>) -> Self {
        Self::with_scheduler(catalog, Arc::new(IoScheduler::for_rotational()))
    }

    pub fn with_scheduler(catalog: Arc<CatalogService>, scheduler: Arc<IoScheduler>) -> Self {
        Self {
            catalog,
            active: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            status: Arc::new(Mutex::new(None)),
            scheduler,
        }
    }

    pub fn start_background_if_bound(&self) -> Result<Option<BackgroundScan>, DomainError> {
        let Some(request) = self.current_request()? else {
            return Ok(None);
        };
        let history = self.scan_history(request.library_id)?;
        if !history.has_completed {
            if let Some(previous) = history.latest_terminal {
                *self.status.lock().map_err(lock_error)? = Some(ScanStatus {
                    kind: ScanKind::Full,
                    phase: previous.phase,
                    root_path: request.root_path.to_string_lossy().into_owned(),
                    directories_checked: 0,
                    files_checked: 0,
                    discovered_assets: previous.observed_assets,
                    ignored_files: previous.ignored_files,
                    warning_count: previous.warning_count,
                    current_relative_path: None,
                    error_message: previous.error_message,
                });
                return Ok(None);
            }
        }
        let kind = if history.has_completed {
            ScanKind::Incremental
        } else {
            ScanKind::Full
        };
        self.start(request, kind).map(Some)
    }

    pub fn start_full_current(&self) -> Result<BackgroundScan, DomainError> {
        self.start_current(ScanKind::Full)
    }

    pub fn rescan_current(&self) -> Result<BackgroundScan, DomainError> {
        self.start_current(ScanKind::Incremental)
    }

    pub fn start_current(&self, kind: ScanKind) -> Result<BackgroundScan, DomainError> {
        let request = self
            .current_request()?
            .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "尚未绑定 Fuji 图库。"))?;
        self.start(request, kind)
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn status(&self) -> Result<Option<ScanStatus>, DomainError> {
        Ok(self.status.lock().map_err(lock_error)?.clone())
    }

    pub fn cancel(&self) -> Result<(), DomainError> {
        if !self.is_active() {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "当前没有正在进行的图库扫描。",
            ));
        }
        self.cancel_requested.store(true, Ordering::Release);
        if let Some(status) = self.status.lock().map_err(lock_error)?.as_mut() {
            status.phase = ScanPhase::Cancelling;
        }
        Ok(())
    }

    fn start(&self, request: ScanRequest, kind: ScanKind) -> Result<BackgroundScan, DomainError> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| DomainError::new(ErrorCode::LeaseUnavailable, "图库扫描已在进行中。"))?;
        self.cancel_requested.store(false, Ordering::Release);
        *self.status.lock().map_err(lock_error)? = Some(ScanStatus::running(kind, &request));

        let catalog = self.catalog.clone();
        let active = self.active.clone();
        let cancel_requested = self.cancel_requested.clone();
        let shared_status = self.status.clone();
        let scheduler = self.scheduler.clone();
        let root_id = request.root_id;
        let (sender, receiver) = mpsc::sync_channel(1);
        let spawn = thread::Builder::new()
            .name("fuji-library-readonly-scan".into())
            .spawn(move || {
                let progress_status = shared_status.clone();
                let cancellation = cancel_requested.clone();
                let result = LibraryScanner::with_scheduler(
                    catalog.clone(),
                    scheduler,
                    Some(cancel_requested.clone()),
                )
                .scan_with_observer(
                    request,
                    move || cancellation.load(Ordering::Acquire),
                    move |progress| {
                        if let Ok(mut guard) = progress_status.lock()
                            && let Some(status) = guard.as_mut()
                        {
                            status.apply_progress(progress);
                        }
                    },
                );
                if let Err(error) = &result {
                    let mismatch = error.code == ErrorCode::IdentityMismatch;
                    if mismatch || error.code == ErrorCode::RootOffline {
                        let _ = catalog.mark_root_unavailable(root_id, mismatch);
                    }
                }
                if let Ok(mut guard) = shared_status.lock()
                    && let Some(status) = guard.as_mut()
                {
                    match &result {
                        Ok(report) if report.cancelled => status.phase = ScanPhase::Cancelled,
                        Ok(report) => {
                            status.phase = ScanPhase::Completed;
                            status.discovered_assets = report.completion.observed_assets;
                            status.ignored_files = report.completion.ignored_files;
                            status.warning_count = report.completion.errors;
                            status.current_relative_path = None;
                        }
                        Err(error) => {
                            status.phase = ScanPhase::Failed;
                            status.error_message = Some(error.user_message.clone());
                        }
                    }
                }
                active.store(false, Ordering::Release);
                let _ = sender.send(result);
            });
        if let Err(error) = spawn {
            self.active.store(false, Ordering::Release);
            return Err(
                DomainError::new(ErrorCode::Internal, "无法启动后台图库扫描。")
                    .with_diagnostic(error.to_string()),
            );
        }
        Ok(BackgroundScan { receiver })
    }

    fn current_request(&self) -> Result<Option<ScanRequest>, DomainError> {
        let connection = self.catalog.database.open_read_connection()?;
        connection
            .query_row(
                "SELECT library.library_id, root.root_id, binding.binding_id,
                        binding.absolute_path_display
                 FROM current_library
                 JOIN library ON library.library_id=current_library.library_id
                 JOIN root ON root.library_id=library.library_id
                    AND root.root_kind='FujiLibrary' AND root.lifecycle='Online'
                 JOIN root_binding binding ON binding.binding_id=root.current_binding_id
                    AND binding.binding_state='Active'
                 WHERE current_library.slot=1 AND library.lifecycle='Active'",
                (),
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
            .map(|row| {
                Ok(ScanRequest {
                    library_id: LibraryId::from_uuid(
                        uuid::Uuid::from_slice(&row.0).map_err(|_| corrupt_id())?,
                    ),
                    root_id: RootId::from_uuid(
                        uuid::Uuid::from_slice(&row.1).map_err(|_| corrupt_id())?,
                    ),
                    root_binding_id: RootBindingId::from_uuid(
                        uuid::Uuid::from_slice(&row.2).map_err(|_| corrupt_id())?,
                    ),
                    root_path: row.3.into(),
                })
            })
            .transpose()
    }

    fn scan_history(&self, library_id: LibraryId) -> Result<ScanHistory, DomainError> {
        let connection = self.catalog.database.open_read_connection()?;
        let has_completed = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scan_run WHERE library_id=?1 AND run_state='Completed')",
                [library_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        let latest_terminal = connection
            .query_row(
                "SELECT run_state, observed_assets, ignored_files, error_count
                 FROM scan_run WHERE library_id=?1 AND run_state!='Running'
                 ORDER BY started_at DESC LIMIT 1",
                [library_id.as_bytes().as_slice()],
                |row| {
                    Ok(PersistedScan {
                        phase: match row.get::<_, String>(0)?.as_str() {
                            "Completed" => ScanPhase::Completed,
                            "Failed" => ScanPhase::Failed,
                            _ => ScanPhase::Cancelled,
                        },
                        observed_assets: usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        ignored_files: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                        warning_count: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                        error_message: None,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite_error)?;
        Ok(ScanHistory {
            has_completed,
            latest_terminal,
        })
    }
}

struct ScanHistory {
    has_completed: bool,
    latest_terminal: Option<PersistedScan>,
}

struct PersistedScan {
    phase: ScanPhase,
    observed_assets: usize,
    ignored_files: usize,
    warning_count: usize,
    error_message: Option<String>,
}

fn corrupt_id() -> DomainError {
    DomainError::new(ErrorCode::DatabaseCorrupt, "Root 绑定标识损坏。")
}

fn lock_error<T>(_error: std::sync::PoisonError<T>) -> DomainError {
    DomainError::new(ErrorCode::Internal, "扫描状态暂时不可用。")
}
