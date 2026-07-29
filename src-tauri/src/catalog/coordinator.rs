use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

use rusqlite::OptionalExtension;

use crate::{
    db::migration::map_sqlite_error,
    domain::{DomainError, ErrorCode, LibraryId, RootBindingId, RootId},
};

use super::{CatalogService, LibraryScanner, ScanReport, ScanRequest};

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

#[derive(Clone)]
pub struct ScanCoordinator {
    catalog: Arc<CatalogService>,
    active: Arc<AtomicBool>,
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
        Self {
            catalog,
            active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start_background_if_bound(&self) -> Result<Option<BackgroundScan>, DomainError> {
        let Some(request) = self.current_request()? else {
            return Ok(None);
        };
        self.start(request).map(Some)
    }

    pub fn rescan_current(&self) -> Result<BackgroundScan, DomainError> {
        let request = self
            .current_request()?
            .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "尚未绑定 Fuji 图库。"))?;
        self.start(request)
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn start(&self, request: ScanRequest) -> Result<BackgroundScan, DomainError> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| DomainError::new(ErrorCode::LeaseUnavailable, "图库扫描已在进行中。"))?;
        let catalog = self.catalog.clone();
        let active = self.active.clone();
        let root_id = request.root_id;
        let (sender, receiver) = mpsc::sync_channel(1);
        let spawn = thread::Builder::new()
            .name("fuji-library-readonly-scan".into())
            .spawn(move || {
                let result = LibraryScanner::new(catalog.clone()).scan(request);
                if let Err(error) = &result {
                    let mismatch = error.code == ErrorCode::IdentityMismatch;
                    if mismatch || error.code == ErrorCode::RootOffline {
                        let _ = catalog.mark_root_unavailable(root_id, mismatch);
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
}

fn corrupt_id() -> DomainError {
    DomainError::new(ErrorCode::DatabaseCorrupt, "Root 绑定标识损坏。")
}
