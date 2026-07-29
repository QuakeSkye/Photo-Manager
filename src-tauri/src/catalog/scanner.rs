use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    db::migration::{map_sqlite_error, now_micros},
    domain::{AssetId, DomainError, ErrorCode, LibraryId, RootBindingId, RootId},
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
}

#[derive(Debug, Clone)]
pub struct LibraryScanner {
    catalog: Arc<CatalogService>,
}

impl LibraryScanner {
    pub fn new(catalog: Arc<CatalogService>) -> Self {
        Self { catalog }
    }

    pub fn scan(&self, request: ScanRequest) -> Result<ScanReport, DomainError> {
        let root_probe = probe_directory(&request.root_path)?;
        self.verify_binding(&request, &root_probe)?;
        let scan_run_id = self.catalog.begin_scan(
            request.library_id,
            request.root_id,
            request.root_binding_id,
        )?;
        let mut state = ScanState::default();
        let scan_result = self.walk_directory(&request, &root_probe, scan_run_id, &mut state);
        if let Err(error) = scan_result {
            let _ = self.catalog.fail_scan(scan_run_id);
            return Err(error);
        }
        let ignored_files = state.ignored_by_extension.values().sum();
        let completion = self.catalog.complete_scan_with_deferred(
            scan_run_id,
            &state.observed,
            &state.deferred_path_keys,
            ignored_files,
            state.skipped_reparse,
            state.issues.len(),
        )?;
        Ok(ScanReport {
            completion,
            ignored_by_extension: state.ignored_by_extension,
            issues: state.issues,
        })
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

    fn walk_directory(
        &self,
        request: &ScanRequest,
        root_probe: &crate::roots::DirectoryProbe,
        scan_run_id: crate::domain::ScanRunId,
        state: &mut ScanState,
    ) -> Result<(), DomainError> {
        let mut stack = vec![request.root_path.clone()];
        while let Some(directory) = stack.pop() {
            let mut entries = fs::read_dir(&directory)
                .map_err(scan_io_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(scan_io_error)?;
            entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
            for entry in entries.into_iter().rev() {
                let path = entry.path();
                let relative = path.strip_prefix(&request.root_path).map_err(|_| {
                    DomainError::new(ErrorCode::UnsafePath, "扫描路径离开了图库 Root。")
                })?;
                let display = relative.to_string_lossy().replace('/', "\\");
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
                    preview,
                };
                match self.catalog.observe_media_asset_outcome(observation) {
                    Ok(ObservationOutcome::Observed(result)) => {
                        state.observed.push(result.asset_id)
                    }
                    Ok(ObservationOutcome::DeferredByReservation) => {
                        state.deferred_path_keys.push(path_key(relative));
                    }
                    Err(error) => state.issues.push(ScanIssue {
                        relative_path: display,
                        message: error.user_message,
                    }),
                }
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
