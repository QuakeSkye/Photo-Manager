use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use crate::{
    db::migration::{map_sqlite_error, now_micros},
    domain::{AssetId, DomainError, ErrorCode, LibraryId, RootBindingId, RootId},
    io_scheduler::{IoPriority, IoScheduler},
    media::{ExifMetadata, PreviewState, inspect},
    roots::{
        FilesystemKind, open_read_only, path_is_reparse, path_key, probe_directory, probe_file,
    },
};

use super::{
    AssetObservation, CatalogObservation, CatalogService, MediaKind, ObservationOutcome,
    ScanCompletion,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequest {
    pub library_id: LibraryId,
    pub root_id: RootId,
    pub root_binding_id: RootBindingId,
    pub root_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    pub relative_path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub completion: ScanCompletion,
    pub ignored_by_extension: BTreeMap<String, usize>,
    pub issues: Vec<ScanIssue>,
    pub cancelled: bool,
    pub files_checked: usize,
    pub metadata_records_added: usize,
    pub elapsed_ms: u64,
    pub files_per_second: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub directories_checked: usize,
    pub files_checked: usize,
    pub observed_assets: usize,
    pub ignored_files: usize,
    pub warning_count: usize,
    pub current_relative_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LibraryScanner {
    catalog: Arc<CatalogService>,
    scheduler: Arc<IoScheduler>,
    scheduler_cancel: Option<Arc<AtomicBool>>,
}

impl LibraryScanner {
    pub fn new(catalog: Arc<CatalogService>) -> Self {
        Self::with_scheduler(catalog, Arc::new(IoScheduler::for_rotational()), None)
    }

    pub fn with_scheduler(
        catalog: Arc<CatalogService>,
        scheduler: Arc<IoScheduler>,
        scheduler_cancel: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            catalog,
            scheduler,
            scheduler_cancel,
        }
    }

    pub fn scan(&self, request: ScanRequest) -> Result<ScanReport, DomainError> {
        self.scan_with_observer(request, || false, |_| {})
    }

    pub fn scan_with_observer<C, O>(
        &self,
        request: ScanRequest,
        mut cancelled: C,
        mut observe: O,
    ) -> Result<ScanReport, DomainError>
    where
        C: FnMut() -> bool,
        O: FnMut(ScanProgress),
    {
        let _bulk_io = self.scheduler.acquire_bulk_io(&mut cancelled)?;
        let started = Instant::now();
        let root_probe = probe_directory(&request.root_path)?;
        self.verify_binding(&request, &root_probe)?;
        let scan_run_id = self.catalog.begin_scan(
            request.library_id,
            request.root_id,
            request.root_binding_id,
        )?;
        let mut state = ScanState::default();
        let scan_result = self.walk_directory(
            &request,
            &root_probe,
            scan_run_id,
            &mut state,
            &mut cancelled,
            &mut observe,
        );
        if let Err(error) = scan_result {
            let _ = self.catalog.fail_scan(scan_run_id);
            return Err(error);
        }
        let ignored_files = state.ignored_by_extension.values().sum();
        if state.cancelled {
            let completion = self.catalog.interrupt_scan(
                scan_run_id,
                state.observed.len(),
                ignored_files,
                state.skipped_reparse,
                state.issues.len(),
            )?;
            return Ok(state.report(completion, true, started.elapsed()));
        }
        let completion = self.catalog.complete_scan_with_deferred(
            scan_run_id,
            &state.observed,
            &state.deferred_path_keys,
            ignored_files,
            state.skipped_reparse,
            state.issues.len(),
        )?;
        Ok(state.report(completion, false, started.elapsed()))
    }

    fn verify_binding(
        &self,
        request: &ScanRequest,
        probe: &crate::roots::DirectoryProbe,
    ) -> Result<(), DomainError> {
        let connection = self.catalog.database.open_read_connection()?;
        let valid: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM root
                    JOIN root_binding binding ON binding.binding_id=root.current_binding_id
                    JOIN volume_identity volume ON volume.volume_id=binding.volume_id
                    WHERE root.root_id=?1 AND root.library_id=?2 AND root.lifecycle='Online'
                      AND binding.binding_id=?3 AND binding.binding_state='Active'
                      AND binding.absolute_path_key=?4
                      AND binding.root_directory_file_id IS ?5
                      AND volume.strong_identity_key=?6
                 )",
                rusqlite::params![
                    request.root_id.as_bytes().as_slice(),
                    request.library_id.as_bytes().as_slice(),
                    request.root_binding_id.as_bytes().as_slice(),
                    &probe.absolute_path_key,
                    &probe.directory_file_id,
                    &probe.volume_identity_key,
                ],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        if !valid {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "扫描前 Root、卷或目录身份复核失败。",
            ));
        }
        Ok(())
    }

    fn walk_directory<C, O>(
        &self,
        request: &ScanRequest,
        root_probe: &crate::roots::DirectoryProbe,
        scan_run_id: crate::domain::ScanRunId,
        state: &mut ScanState,
        cancelled: &mut C,
        observe: &mut O,
    ) -> Result<(), DomainError>
    where
        C: FnMut() -> bool,
        O: FnMut(ScanProgress),
    {
        let mut stack = vec![request.root_path.clone()];
        while let Some(directory) = stack.pop() {
            if cancelled() {
                state.cancelled = true;
                break;
            }
            state.directories_checked += 1;
            state.current_relative_path = directory
                .strip_prefix(&request.root_path)
                .ok()
                .map(|value| value.to_string_lossy().replace('/', "\\"))
                .filter(|value| !value.is_empty());
            observe(state.progress());
            let mut entries = {
                let _directory_permit = self.scheduler.acquire(IoPriority::ScanAndExif, || {
                    self.scheduler_cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Acquire))
                })?;
                fs::read_dir(&directory)
                    .map_err(scan_io_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(scan_io_error)?
            };
            entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
            for entry in entries.into_iter().rev() {
                if cancelled() {
                    state.cancelled = true;
                    break;
                }
                state.files_checked += 1;
                let _file_permit = self.scheduler.acquire(IoPriority::ScanAndExif, || {
                    self.scheduler_cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Acquire))
                })?;
                let path = entry.path();
                let relative = path.strip_prefix(&request.root_path).map_err(|_| {
                    DomainError::new(ErrorCode::UnsafePath, "扫描路径离开了图库 Root。")
                })?;
                let display = relative.to_string_lossy().replace('/', "\\");
                state.current_relative_path = Some(display.clone());
                match path_is_reparse(&path) {
                    Ok(true) => {
                        state.skipped_reparse += 1;
                        state.issues.push(ScanIssue {
                            relative_path: display,
                            message: "已跳过 reparse point。".into(),
                        });
                        continue;
                    }
                    Err(error) => {
                        state.deferred_path_keys.push(path_key(relative));
                        state.issues.push(ScanIssue {
                            relative_path: display,
                            message: error.user_message,
                        });
                        continue;
                    }
                    Ok(false) => {}
                }
                let file_type = match entry.file_type() {
                    Ok(value) => value,
                    Err(error) => {
                        state.deferred_path_keys.push(path_key(relative));
                        state.issues.push(issue(display, error));
                        continue;
                    }
                };
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !file_type.is_file() {
                    state
                        .ignored_by_extension
                        .entry("<non-file>".into())
                        .and_modify(|value| *value += 1)
                        .or_insert(1);
                    continue;
                }
                let extension = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let kind = match extension.as_str() {
                    "jpg" | "jpeg" => MediaKind::Jpg,
                    "raf" => MediaKind::Raf,
                    _ => {
                        let key = if extension.is_empty() {
                            "<none>".into()
                        } else {
                            format!(".{extension}")
                        };
                        state
                            .ignored_by_extension
                            .entry(key)
                            .and_modify(|value| *value += 1)
                            .or_insert(1);
                        continue;
                    }
                };
                if is_application_partial(&path) {
                    continue;
                }
                let mut file = match open_read_only(&path) {
                    Ok(value) => value,
                    Err(error) => {
                        state.deferred_path_keys.push(path_key(relative));
                        state.issues.push(ScanIssue {
                            relative_path: display,
                            message: error.user_message,
                        });
                        continue;
                    }
                };
                let identity = match probe_file(&file) {
                    Ok(value) => value,
                    Err(error) => {
                        state.deferred_path_keys.push(path_key(relative));
                        state.issues.push(ScanIssue {
                            relative_path: display,
                            message: error.user_message,
                        });
                        continue;
                    }
                };
                if identity.reparse_point {
                    state.skipped_reparse += 1;
                    continue;
                }
                let (exif, preview) = match inspect(&mut file, kind) {
                    Ok(value) => (value.exif, value.preview),
                    Err(error) => {
                        state.issues.push(ScanIssue {
                            relative_path: display.clone(),
                            message: error.user_message,
                        });
                        (ExifMetadata::default(), PreviewState::Unavailable)
                    }
                };
                let size_bytes = i64::try_from(identity.size_bytes).map_err(|_| {
                    DomainError::new(ErrorCode::MediaUnreadable, "照片大小超过索引范围。")
                })?;
                let hard_link_count = i64::try_from(identity.hard_link_count).unwrap_or(i64::MAX);
                let observation = CatalogObservation {
                    asset: AssetObservation {
                        library_id: request.library_id,
                        root_id: request.root_id,
                        root_binding_id: request.root_binding_id,
                        media_kind: kind,
                        relative_path: display.clone(),
                        path_key: path_key(relative),
                        file_id: identity.file_id,
                        size_bytes,
                        mtime_filetime: identity.mtime_filetime,
                        observed_at: now_micros(),
                    },
                    hard_link_count,
                    deletion_supported: root_probe.filesystem_kind == FilesystemKind::Ntfs
                        && hard_link_count == 1,
                    scan_run_id: Some(scan_run_id),
                    capture_local_key: exif.capture_local_key,
                    camera_model: exif.camera_model,
                    camera_serial: exif.camera_serial,
                    capture_metadata: exif.capture,
                    preview,
                };
                match self.catalog.observe_media_asset_outcome(observation) {
                    Ok(ObservationOutcome::Observed(result)) => {
                        state.observed.push(result.asset_id);
                        state.metadata_records_added +=
                            usize::from(result.capture_metadata_inserted);
                    }
                    Ok(ObservationOutcome::DeferredByReservation) => {
                        state.deferred_path_keys.push(path_key(relative));
                    }
                    Err(error) => state.issues.push(ScanIssue {
                        relative_path: display,
                        message: error.user_message,
                    }),
                }
                observe(state.progress());
            }
            if state.cancelled {
                break;
            }
        }
        state.observed.sort_unstable();
        state.observed.dedup();
        state.deferred_path_keys.sort_unstable();
        state.deferred_path_keys.dedup();
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ScanState {
    observed: Vec<AssetId>,
    ignored_by_extension: BTreeMap<String, usize>,
    skipped_reparse: usize,
    deferred_path_keys: Vec<Vec<u8>>,
    issues: Vec<ScanIssue>,
    directories_checked: usize,
    files_checked: usize,
    current_relative_path: Option<String>,
    cancelled: bool,
    metadata_records_added: usize,
}

impl ScanState {
    fn report(
        self,
        completion: ScanCompletion,
        cancelled: bool,
        elapsed: std::time::Duration,
    ) -> ScanReport {
        let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let files_per_second = if elapsed.is_zero() {
            u64::try_from(self.files_checked).unwrap_or(u64::MAX)
        } else {
            let rate = self.files_checked as f64 / elapsed.as_secs_f64();
            rate.round().clamp(0.0, u64::MAX as f64) as u64
        };
        ScanReport {
            completion,
            ignored_by_extension: self.ignored_by_extension,
            issues: self.issues,
            cancelled,
            files_checked: self.files_checked,
            metadata_records_added: self.metadata_records_added,
            elapsed_ms,
            files_per_second,
        }
    }

    fn progress(&self) -> ScanProgress {
        ScanProgress {
            directories_checked: self.directories_checked,
            files_checked: self.files_checked,
            observed_assets: self.observed.len(),
            ignored_files: self.ignored_by_extension.values().sum(),
            warning_count: self.issues.len(),
            current_relative_path: self.current_relative_path.clone(),
        }
    }
}

fn is_application_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.starts_with(".fpm-partial-") || value.ends_with(".fpm-partial"))
}

fn issue(relative_path: String, error: std::io::Error) -> ScanIssue {
    ScanIssue {
        relative_path,
        message: format!("无法读取目录项：{error}"),
    }
}

fn scan_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::RootOffline, "无法继续只读扫描图库。")
        .with_diagnostic(error.to_string())
}
