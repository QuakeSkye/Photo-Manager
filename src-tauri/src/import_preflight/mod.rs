mod planner;
pub(crate) mod proof;

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::OptionalExtension;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    catalog::MediaKind,
    db::{Database, migration::now_micros},
    domain::{DomainError, ErrorCode, RootBindingId, RootId, VolumeId},
    io_scheduler::{IoPriority, IoScheduler},
    media,
    roots::{DirectoryProbe, FileProbe, path_is_reparse, path_key, probe_directory},
    safety::{FeatureGate, RuntimeSafetyGate, SafetyPermit},
};

use proof::{AuditSide, ProcessIoAudit, ProcessIoAuditReport, StableReadHandle};

const READ_CAPABILITIES: i64 = 0b1111;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSetupSnapshot {
    pub source: Option<ImportSourceView>,
    pub source_needs_reselection: bool,
    pub target: Option<ImportTargetView>,
    pub can_start_preflight: bool,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSourceView {
    pub volume_name: String,
    pub display_path: String,
    pub recursively_scans_selected_subtree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTargetView {
    pub display_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreflightStatus {
    pub phase: ImportPreflightPhase,
    pub scanned_files: u64,
    pub supported_files: u64,
    pub current_relative_path: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportPreflightPhase {
    Idle,
    Running,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
}

impl Default for ImportPreflightStatus {
    fn default() -> Self {
        Self {
            phase: ImportPreflightPhase::Idle,
            scanned_files: 0,
            supported_files: 0,
            current_relative_path: None,
            error_message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreflightDraft {
    pub source_path: String,
    pub target_path: String,
    pub summary: ImportPreflightSummary,
    pub items: Vec<ImportDraftItem>,
    pub ignored_suffixes: BTreeMap<String, u64>,
    pub draft_digest: String,
    pub io_audit: ProcessIoAuditReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreflightSummary {
    pub planned_groups: u64,
    pub new_groups: u64,
    pub new_files: u64,
    pub new_bytes: u64,
    pub skipped_files: u64,
    pub conflict_groups: u64,
    pub unknown_date_files: u64,
    pub unreadable_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDraftItem {
    pub logical_group: String,
    pub file_name: String,
    pub source_relative_path: String,
    pub media_kind: DraftMediaKind,
    pub disposition: DraftDisposition,
    pub target_relative_path: Option<String>,
    pub size_bytes: u64,
    pub date_unknown: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DraftMediaKind {
    Jpg,
    Raf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DraftDisposition {
    New,
    Skip,
    Conflict,
    Unreadable,
}

#[derive(Debug, Clone)]
struct SelectedSource {
    profile_id: Vec<u8>,
    display_path: PathBuf,
}

#[derive(Debug, Default)]
struct RuntimeState {
    selected_source: Option<SelectedSource>,
    status: ImportPreflightStatus,
    last_draft: Option<ExecutionDraftSeed>,
}

#[derive(Debug, Clone)]
pub struct ImportPreflightService {
    database: Arc<Database>,
    safety_gate: Arc<RuntimeSafetyGate>,
    app_data_directory: PathBuf,
    state: Arc<Mutex<RuntimeState>>,
    cancel_requested: Arc<AtomicBool>,
    scheduler: Arc<IoScheduler>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceContext {
    pub library_id: Vec<u8>,
    pub volume_id: Vec<u8>,
    pub root_id: Vec<u8>,
    pub binding_id: Vec<u8>,
    pub root_path: PathBuf,
    pub root_probe: DirectoryProbe,
}

#[derive(Debug, Clone)]
pub(crate) struct TargetContext {
    pub library_id: Vec<u8>,
    pub root_id: Vec<u8>,
    pub binding_id: Vec<u8>,
    pub root_path: PathBuf,
    pub root_probe: DirectoryProbe,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    pub relative_path: PathBuf,
    pub file_name: String,
    pub stem: String,
    pub extension: String,
    pub kind: DraftMediaKind,
    pub size_bytes: u64,
    pub capture_local_key: Option<String>,
    pub inspection: media::MediaInspection,
    pub full_hash: [u8; 32],
    pub identity: FileProbe,
}

#[derive(Debug, Clone)]
pub(crate) struct UnreadableSource {
    pub relative_path: PathBuf,
    pub file_name: String,
    pub kind: DraftMediaKind,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EnumeratedSource {
    pub files: Vec<SourceFile>,
    pub unreadable: Vec<UnreadableSource>,
    pub ignored_suffixes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecutionDraftSeed {
    pub profile_id: Vec<u8>,
    pub source: SourceContext,
    pub target: TargetContext,
    pub enumerated: EnumeratedSource,
    pub draft: ImportPreflightDraft,
}

#[derive(Debug)]
struct ProfileRecord {
    profile_id: Vec<u8>,
    library_id: Vec<u8>,
    source_root_id: Vec<u8>,
    binding_id: Vec<u8>,
    volume_id: Vec<u8>,
    source_relative_path: String,
    display_path: String,
    volume_key: Vec<u8>,
    directory_file_id: Option<Vec<u8>>,
    binding_path_key: Vec<u8>,
    binding_generation: i64,
}

impl ImportPreflightService {
    #[cfg(test)]
    pub fn new(database: Arc<Database>, app_data_directory: impl Into<PathBuf>) -> Self {
        let safety_gate = Arc::new(RuntimeSafetyGate::new(database.clone()));
        safety_gate.mark_ready().expect("test safety gate");
        Self::with_scheduler(
            database,
            app_data_directory,
            Arc::new(IoScheduler::for_rotational()),
            safety_gate,
        )
    }

    pub fn with_scheduler(
        database: Arc<Database>,
        app_data_directory: impl Into<PathBuf>,
        scheduler: Arc<IoScheduler>,
        safety_gate: Arc<RuntimeSafetyGate>,
    ) -> Self {
        Self {
            database,
            safety_gate,
            app_data_directory: app_data_directory.into(),
            state: Arc::new(Mutex::new(RuntimeState::default())),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            scheduler,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_permit(&self) -> SafetyPermit {
        self.safety_gate
            .require_feature(FeatureGate::Import)
            .expect("healthy test import gate")
    }

    pub fn setup_snapshot(&self) -> Result<ImportSetupSnapshot, DomainError> {
        let target = self.current_target().ok();
        let blocked_reason = self.active_task_reason()?;
        let mut source_needs_reselection = false;
        let selected = self.state()?.selected_source.clone();
        let source = if let Some(selected) = selected {
            self.validate_selected_source(&selected)
                .ok()
                .map(|profile| source_view(&selected.display_path, &profile.volume_key))
        } else if let Some(prefilled) = self.find_prefill()? {
            let view = source_view(Path::new(&prefilled.display_path), &prefilled.volume_key);
            self.state()?.selected_source = Some(SelectedSource {
                profile_id: prefilled.profile_id,
                display_path: PathBuf::from(&prefilled.display_path),
            });
            Some(view)
        } else {
            source_needs_reselection = self.has_source_profiles()?;
            None
        };
        let target_view = target.as_ref().map(|target| ImportTargetView {
            display_path: target.root_path.to_string_lossy().into_owned(),
        });
        Ok(ImportSetupSnapshot {
            can_start_preflight: source.is_some()
                && target_view.is_some()
                && blocked_reason.is_none()
                && self.state()?.status.phase != ImportPreflightPhase::Running,
            source,
            source_needs_reselection,
            target: target_view,
            blocked_reason,
        })
    }

    pub fn select_source(
        &self,
        permit: &SafetyPermit,
        selected_directory: &Path,
    ) -> Result<ImportSetupSnapshot, DomainError> {
        self.safety_gate.revalidate(permit, FeatureGate::Import)?;
        if let Some(reason) = self.active_task_reason()? {
            return Err(DomainError::new(ErrorCode::LeaseUnavailable, reason));
        }
        let audit = ProcessIoAudit::default();
        let source_probe = probe_directory(selected_directory)?;
        let target = self.current_target()?;
        self.reject_source_conflicts(&source_probe, &target)?;
        let selected = self.register_source_profile(source_probe)?;
        self.state()?.selected_source = Some(selected);
        let report = audit.snapshot();
        debug_assert_eq!(report.source_write_requests, 0);
        self.setup_snapshot()
    }

    pub fn status(&self) -> Result<ImportPreflightStatus, DomainError> {
        Ok(self.state()?.status.clone())
    }

    pub fn cancel(&self) -> Result<(), DomainError> {
        let mut state = self.state()?;
        if state.status.phase == ImportPreflightPhase::Running {
            state.status.phase = ImportPreflightPhase::Cancelling;
            self.cancel_requested.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub fn run_preflight(
        &self,
        permit: &SafetyPermit,
    ) -> Result<ImportPreflightDraft, DomainError> {
        self.safety_gate.revalidate(permit, FeatureGate::Import)?;
        {
            let mut state = self.state()?;
            if matches!(
                state.status.phase,
                ImportPreflightPhase::Running | ImportPreflightPhase::Cancelling
            ) {
                return Err(DomainError::new(
                    ErrorCode::LeaseUnavailable,
                    "只读预检已经在进行中。",
                ));
            }
            state.status = ImportPreflightStatus {
                phase: ImportPreflightPhase::Running,
                ..ImportPreflightStatus::default()
            };
        }
        self.cancel_requested.store(false, Ordering::Release);
        let result = self.run_preflight_inner();
        let mut state = self.state()?;
        match &result {
            Ok(_) => state.status.phase = ImportPreflightPhase::Completed,
            Err(error) if self.cancel_requested.load(Ordering::Acquire) => {
                state.status.phase = ImportPreflightPhase::Cancelled;
                state.status.error_message = None;
                return Err(DomainError::new(
                    ErrorCode::InvalidTransition,
                    "只读预检已取消，没有生成导入计划。",
                ));
            }
            Err(error) => {
                state.status.phase = ImportPreflightPhase::Failed;
                state.status.error_message = Some(error.user_message.clone());
            }
        }
        result
    }

    fn run_preflight_inner(&self) -> Result<ImportPreflightDraft, DomainError> {
        if let Some(reason) = self.active_task_reason()? {
            return Err(DomainError::new(ErrorCode::LeaseUnavailable, reason));
        }
        let selected = self.state()?.selected_source.clone().ok_or_else(|| {
            DomainError::new(ErrorCode::RootOffline, "请先选择内存卡来源文件夹。")
        })?;
        let profile = self.validate_selected_source(&selected)?;
        let profile = self.refresh_source_binding_if_needed(profile, &selected.display_path)?;
        let source = self.source_context(&profile, &selected.display_path)?;
        let target = self.current_target()?;
        self.reject_source_conflicts(&source.root_probe, &target)?;
        if source.library_id != target.library_id {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "来源配置不属于当前 Fuji 图库，请重新选择来源。",
            ));
        }

        let audit = ProcessIoAudit::default();
        let enumerated = self.enumerate_source(&source, &audit)?;
        self.check_cancelled()?;
        let draft = planner::build_draft(
            &self.database,
            &source,
            &target,
            enumerated.clone(),
            &audit,
            || self.check_cancelled(),
        )?;
        let report = audit.snapshot();
        if report.source_write_requests != 0 || report.target_write_requests != 0 {
            return Err(DomainError::new(
                ErrorCode::VolumeWriteDenied,
                "只读预检审计发现写请求，结果已拒绝。",
            ));
        }
        let draft = ImportPreflightDraft {
            io_audit: report,
            ..draft
        };
        self.state()?.last_draft = Some(ExecutionDraftSeed {
            profile_id: profile.profile_id,
            source,
            target,
            enumerated,
            draft: draft.clone(),
        });
        Ok(draft)
    }

    pub(crate) fn confirmed_seed(
        &self,
        draft_digest: &str,
    ) -> Result<ExecutionDraftSeed, DomainError> {
        let seed = self.state()?.last_draft.clone().ok_or_else(|| {
            DomainError::new(
                ErrorCode::InvalidTransition,
                "请先完成一次只读预检，再确认导入。",
            )
        })?;
        if seed.draft.draft_digest != draft_digest {
            return Err(DomainError::new(
                ErrorCode::StaleVersion,
                "只读计划已经变化，请重新预检并确认。",
            ));
        }
        Ok(seed)
    }

    pub(crate) fn refresh_active_import(
        &self,
        job_id: &[u8],
    ) -> Result<ExecutionDraftSeed, DomainError> {
        let active_matches: bool = self
            .database
            .open_read_connection()?
            .query_row(
                "SELECT EXISTS(
                SELECT 1 FROM task_intent
                WHERE task_id=?1 AND task_kind='Import'
                  AND lifecycle NOT IN ('Completed','Cancelled')
             )",
                [job_id],
                |row| row.get(0),
            )
            .map_err(crate::db::migration::map_sqlite_error)?;
        if !active_matches {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "当前没有可刷新的导入任务。",
            ));
        }
        let selected = self.state()?.selected_source.clone().ok_or_else(|| {
            DomainError::new(ErrorCode::RootOffline, "请重新选择原始内存卡来源文件夹。")
        })?;
        let profile = self.validate_selected_source(&selected)?;
        let profile = self.refresh_source_binding_if_needed(profile, &selected.display_path)?;
        let source = self.source_context(&profile, &selected.display_path)?;
        let target = self.current_target()?;
        self.reject_source_conflicts(&source.root_probe, &target)?;
        let audit = ProcessIoAudit::default();
        let enumerated = self.enumerate_source(&source, &audit)?;
        let draft = planner::build_draft(
            &self.database,
            &source,
            &target,
            enumerated.clone(),
            &audit,
            || self.check_cancelled(),
        )?;
        let report = audit.snapshot();
        if report.source_write_requests != 0 || report.target_write_requests != 0 {
            return Err(DomainError::new(
                ErrorCode::VolumeWriteDenied,
                "导入执行前验证发现写请求，任务已停止。",
            ));
        }
        Ok(ExecutionDraftSeed {
            profile_id: profile.profile_id,
            source,
            target,
            enumerated,
            draft: ImportPreflightDraft {
                io_audit: report,
                ..draft
            },
        })
    }

    fn enumerate_source(
        &self,
        source: &SourceContext,
        audit: &ProcessIoAudit,
    ) -> Result<EnumeratedSource, DomainError> {
        let mut result = EnumeratedSource::default();
        let mut stack = vec![source.root_path.clone()];
        while let Some(directory) = stack.pop() {
            self.check_cancelled()?;
            audit.enumerate(AuditSide::Source);
            let mut entries = {
                let _permit = self.scheduler.acquire(IoPriority::ScanAndExif, || {
                    self.cancel_requested.load(Ordering::Acquire)
                })?;
                fs::read_dir(&directory)
                    .map_err(source_io_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(source_io_error)?
            };
            entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
            for entry in entries.into_iter().rev() {
                self.check_cancelled()?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(&source.root_path)
                    .map_err(|_| {
                        DomainError::new(ErrorCode::UnsafePath, "来源枚举越过了所选子树。")
                    })?
                    .to_path_buf();
                let file_type = entry.file_type().map_err(source_io_error)?;
                if file_type.is_symlink() || path_is_reparse(&path)? {
                    self.update_progress(&relative, file_type.is_file(), false)?;
                    *result
                        .ignored_suffixes
                        .entry("[reparse]".into())
                        .or_default() += 1;
                    continue;
                }
                if file_type.is_dir() {
                    self.update_current_path(&relative)?;
                    stack.push(path);
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let Some((kind, media_kind, extension)) = supported_kind(&path) else {
                    self.update_progress(&relative, true, false)?;
                    let suffix = path
                        .extension()
                        .and_then(|value| value.to_str())
                        .map(|value| format!(".{}", value.to_ascii_lowercase()))
                        .unwrap_or_else(|| "[无扩展名]".into());
                    *result.ignored_suffixes.entry(suffix).or_default() += 1;
                    continue;
                };
                self.update_progress(&relative, true, true)?;
                let file_name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_string();
                let stem = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_string();
                if let Err(error) = validate_target_file_name(&file_name) {
                    result.unreadable.push(UnreadableSource {
                        relative_path: relative,
                        file_name,
                        kind,
                        reason: error.user_message,
                    });
                    continue;
                }
                let _permit = self
                    .scheduler
                    .acquire(IoPriority::BackgroundHashAndCache, || {
                        self.cancel_requested.load(Ordering::Acquire)
                    })?;
                match inspect_source_file(&path, media_kind, audit) {
                    Ok((identity, full_hash, inspection)) => {
                        let capture_local_key = inspection.exif.capture_local_key.clone();
                        result.files.push(SourceFile {
                            relative_path: relative,
                            file_name,
                            stem,
                            extension,
                            kind,
                            size_bytes: identity.size_bytes,
                            capture_local_key,
                            inspection,
                            full_hash,
                            identity,
                        });
                    }
                    Err(error) => result.unreadable.push(UnreadableSource {
                        relative_path: relative,
                        file_name,
                        kind,
                        reason: error.user_message,
                    }),
                }
            }
        }
        let after = probe_directory(&source.root_path)?;
        if after.volume_identity_key != source.root_probe.volume_identity_key
            || after.directory_file_id != source.root_probe.directory_file_id
            || after.absolute_path_key != source.root_probe.absolute_path_key
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "来源目录在只读预检期间发生变化，请重新选择来源。",
            ));
        }
        Ok(result)
    }

    fn update_progress(
        &self,
        relative: &Path,
        is_file: bool,
        supported: bool,
    ) -> Result<(), DomainError> {
        let mut state = self.state()?;
        if is_file {
            state.status.scanned_files += 1;
        }
        if supported {
            state.status.supported_files += 1;
        }
        state.status.current_relative_path = Some(display_relative(relative));
        Ok(())
    }

    fn update_current_path(&self, relative: &Path) -> Result<(), DomainError> {
        self.state()?.status.current_relative_path = Some(display_relative(relative));
        Ok(())
    }

    fn check_cancelled(&self) -> Result<(), DomainError> {
        if self.cancel_requested.load(Ordering::Acquire) {
            Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "只读预检已取消。",
            ))
        } else {
            Ok(())
        }
    }

    fn current_target(&self) -> Result<TargetContext, DomainError> {
        let connection = self.database.open_read_connection()?;
        let row = connection
            .query_row(
                "SELECT library.library_id, root.root_id, binding.binding_id,
                        binding.absolute_path_display, binding.root_directory_file_id,
                        volume.strong_identity_key
                 FROM current_library
                 JOIN library ON library.library_id=current_library.library_id
                 JOIN root ON root.library_id=library.library_id
                    AND root.root_kind='FujiLibrary' AND root.lifecycle='Online'
                 JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
                    AND binding.binding_state='Active'
                 JOIN volume_identity AS volume ON volume.volume_id=binding.volume_id
                 WHERE current_library.slot=1",
                (),
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(crate::db::migration::map_sqlite_error)?
            .ok_or_else(|| {
                DomainError::new(ErrorCode::RootOffline, "尚未绑定可用的 Fuji 图库。")
            })?;
        let root_path = PathBuf::from(&row.3);
        let root_probe = probe_directory(&root_path)?;
        if root_probe.filesystem_kind.as_sql() != "NTFS"
            || !root_probe.strong_identity
            || root_probe.directory_file_id != row.4
            || row.5.as_ref() != Some(&root_probe.volume_identity_key)
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "Fuji 图库身份或本机 NTFS 能力验证失败。",
            ));
        }
        Ok(TargetContext {
            library_id: row.0,
            root_id: row.1,
            binding_id: row.2,
            root_path,
            root_probe,
        })
    }

    fn validate_selected_source(
        &self,
        selected: &SelectedSource,
    ) -> Result<ProfileRecord, DomainError> {
        let profile = self.load_profile(&selected.profile_id)?;
        let probe = probe_directory(&selected.display_path)?;
        if probe.volume_identity_key != profile.volume_key
            || probe.directory_file_id != profile.directory_file_id
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "内存卡或所选来源文件夹身份已变化，请重新选择。",
            ));
        }
        Ok(profile)
    }

    fn source_context(
        &self,
        profile: &ProfileRecord,
        path: &Path,
    ) -> Result<SourceContext, DomainError> {
        let probe = probe_directory(path)?;
        Ok(SourceContext {
            library_id: profile.library_id.clone(),
            volume_id: profile.volume_id.clone(),
            root_id: profile.source_root_id.clone(),
            binding_id: profile.binding_id.clone(),
            root_path: probe.absolute_path.clone(),
            root_probe: probe,
        })
    }

    fn reject_source_conflicts(
        &self,
        source: &DirectoryProbe,
        target: &TargetContext,
    ) -> Result<(), DomainError> {
        if source.volume_identity_key == target.root_probe.volume_identity_key {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "内存卡来源与 Fuji 图库不能位于同一卷。",
            ));
        }
        let app_data = probe_directory(&self.app_data_directory)?;
        if source.volume_identity_key == app_data.volume_identity_key {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "该卷承载应用数据，不能作为内存卡来源。",
            ));
        }
        let connection = self.database.open_read_connection()?;
        let conflicts = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM root
                    JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
                    JOIN volume_identity AS volume ON volume.volume_id=binding.volume_id
                    WHERE root.lifecycle!='Tombstoned'
                      AND root.root_kind IN ('FujiLibrary','AppDataRoot','CacheRoot')
                      AND volume.strong_identity_key=?1
                 )",
                [&source.volume_identity_key],
                |row| row.get::<_, bool>(0),
            )
            .map_err(crate::db::migration::map_sqlite_error)?;
        if conflicts {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "该卷承载应用必须写入的位置，不能作为内存卡来源。",
            ));
        }
        Ok(())
    }

    fn register_source_profile(
        &self,
        probe: DirectoryProbe,
    ) -> Result<SelectedSource, DomainError> {
        let target = self.current_target()?;
        let source_relative = volume_relative_path(&probe.absolute_path);
        let source_path_key = path_key(Path::new(&source_relative));
        let display_path = probe.absolute_path.clone();
        let writer = self.database.writer()?;
        let profile_id = writer.transaction(move |transaction| {
            let volume_id = if let Some(bytes) = transaction
                .query_row(
                    "SELECT volume_id FROM volume_identity
                     WHERE identity_format_version=1 AND strong_identity_key=?1",
                    [&probe.volume_identity_key],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(crate::db::migration::map_sqlite_error)?
            {
                bytes
            } else {
                let id = VolumeId::new();
                transaction
                    .execute(
                        "INSERT INTO volume_identity(
                            volume_id, filesystem_kind, strong_identity_key,
                            identity_format_version, identity_strength, created_at
                         ) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                        (
                            id.as_bytes().as_slice(),
                            probe.filesystem_kind.as_sql(),
                            &probe.volume_identity_key,
                            if probe.strong_identity {
                                "Strong"
                            } else {
                                "UserConfirmedLimited"
                            },
                            now_micros(),
                        ),
                    )
                    .map_err(crate::db::migration::map_sqlite_error)?;
                id.as_bytes().to_vec()
            };
            transaction
                .execute(
                    "INSERT INTO volume_policy(volume_id, deny_write, reason, created_at)
                     VALUES (?1, 1, 'ImportSource', ?2)
                     ON CONFLICT(volume_id) DO NOTHING",
                    (&volume_id, now_micros()),
                )
                .map_err(crate::db::migration::map_sqlite_error)?;

            if let Some(existing) = transaction
                .query_row(
                    "SELECT profile.profile_id
                     FROM import_source_profile AS profile
                     WHERE profile.library_id=?1 AND profile.volume_id=?2
                       AND profile.source_path_key=?3 AND profile.retired_at IS NULL",
                    (&target.library_id, &volume_id, &source_path_key),
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(crate::db::migration::map_sqlite_error)?
            {
                transaction
                    .execute(
                        "UPDATE import_source_profile SET last_confirmed_at=?1 WHERE profile_id=?2",
                        (now_micros(), &existing),
                    )
                    .map_err(crate::db::migration::map_sqlite_error)?;
                return Ok(existing);
            }

            let root_id = RootId::new();
            let binding_id = RootBindingId::new();
            let profile_id = Uuid::new_v4();
            let created_at = now_micros();
            transaction
                .execute(
                    "INSERT INTO root(root_id, library_id, root_kind, capability_mask,
                                      root_version, lifecycle, created_at)
                     VALUES (?1, NULL, 'ImportSource', ?2, 1, 'Online', ?3)",
                    (root_id.as_bytes().as_slice(), READ_CAPABILITIES, created_at),
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO root_binding(
                        binding_id, root_id, binding_generation, volume_id,
                        absolute_path_display, absolute_path_key, root_directory_file_id,
                        filesystem_kind, evidence_level, reparse_checked,
                        binding_state, verified_at
                     ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, 1, 'Active', ?9)",
                    rusqlite::params![
                        binding_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                        &volume_id,
                        probe.absolute_path.to_string_lossy(),
                        &probe.absolute_path_key,
                        &probe.directory_file_id,
                        probe.filesystem_kind.as_sql(),
                        if probe.strong_identity {
                            "Strong"
                        } else {
                            "UserConfirmedLimited"
                        },
                        created_at,
                    ],
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE root SET current_binding_id=?1 WHERE root_id=?2",
                    (
                        binding_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                    ),
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO import_source_profile(
                        profile_id, library_id, source_root_id, volume_id,
                        source_relative_path, source_path_key, source_directory_file_id,
                        identity_strength, last_confirmed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        profile_id.as_bytes().as_slice(),
                        &target.library_id,
                        root_id.as_bytes().as_slice(),
                        &volume_id,
                        source_relative,
                        source_path_key,
                        &probe.directory_file_id,
                        if probe.strong_identity {
                            "Strong"
                        } else {
                            "UserConfirmedLimited"
                        },
                        created_at,
                    ],
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            Ok(profile_id.as_bytes().to_vec())
        })?;
        Ok(SelectedSource {
            profile_id,
            display_path,
        })
    }

    fn load_profile(&self, profile_id: &[u8]) -> Result<ProfileRecord, DomainError> {
        self.database
            .open_read_connection()?
            .query_row(
                "SELECT profile.profile_id, profile.library_id, profile.source_root_id,
                        binding.binding_id, profile.volume_id, profile.source_relative_path,
                        binding.absolute_path_display, volume.strong_identity_key,
                        profile.source_directory_file_id, binding.absolute_path_key,
                        binding.binding_generation
                 FROM import_source_profile AS profile
                 JOIN root ON root.root_id=profile.source_root_id
                 JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
                 JOIN volume_identity AS volume ON volume.volume_id=profile.volume_id
                 WHERE profile.profile_id=?1 AND profile.retired_at IS NULL
                   AND root.lifecycle='Online' AND binding.binding_state='Active'",
                [profile_id],
                |row| {
                    Ok(ProfileRecord {
                        profile_id: row.get(0)?,
                        library_id: row.get(1)?,
                        source_root_id: row.get(2)?,
                        binding_id: row.get(3)?,
                        volume_id: row.get(4)?,
                        source_relative_path: row.get(5)?,
                        display_path: row.get(6)?,
                        volume_key: row.get::<_, Option<Vec<u8>>>(7)?.unwrap_or_default(),
                        directory_file_id: row.get(8)?,
                        binding_path_key: row.get(9)?,
                        binding_generation: row.get(10)?,
                    })
                },
            )
            .map_err(crate::db::migration::map_sqlite_error)
    }

    fn refresh_source_binding_if_needed(
        &self,
        profile: ProfileRecord,
        selected_path: &Path,
    ) -> Result<ProfileRecord, DomainError> {
        let probe = probe_directory(selected_path)?;
        if probe.absolute_path_key == profile.binding_path_key {
            return Ok(profile);
        }
        if probe.volume_identity_key != profile.volume_key
            || probe.directory_file_id != profile.directory_file_id
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "重新连接的来源目录不是原内存卡文件夹。",
            ));
        }
        let old_binding = profile.binding_id.clone();
        let root_id = profile.source_root_id.clone();
        let volume_id = profile.volume_id.clone();
        let next_binding = RootBindingId::new();
        let next_generation = profile.binding_generation + 1;
        let evidence_level = if probe.strong_identity {
            "Strong"
        } else {
            "UserConfirmedLimited"
        };
        let writer = self.database.writer()?;
        writer.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE root_binding SET binding_state='Superseded'
                     WHERE binding_id=?1 AND binding_state='Active'",
                    [&old_binding],
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO root_binding(
                        binding_id, root_id, binding_generation, volume_id,
                        absolute_path_display, absolute_path_key, root_directory_file_id,
                        filesystem_kind, evidence_level, reparse_checked,
                        binding_state, supersedes_binding_id, verified_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1,
                               'Active', ?10, ?11)",
                    rusqlite::params![
                        next_binding.as_bytes().as_slice(),
                        &root_id,
                        next_generation,
                        &volume_id,
                        probe.absolute_path.to_string_lossy(),
                        &probe.absolute_path_key,
                        &probe.directory_file_id,
                        probe.filesystem_kind.as_sql(),
                        evidence_level,
                        &old_binding,
                        now_micros(),
                    ],
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE root SET current_binding_id=?1, root_version=root_version+1,
                                    lifecycle='Online' WHERE root_id=?2",
                    (next_binding.as_bytes().as_slice(), &root_id),
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            Ok(())
        })?;
        self.load_profile(&profile.profile_id)
    }

    fn find_prefill(&self) -> Result<Option<ProfileRecord>, DomainError> {
        let target = match self.current_target() {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let connection = self.database.open_read_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT profile.profile_id
                 FROM import_source_profile AS profile
                 WHERE profile.library_id=?1 AND profile.retired_at IS NULL
                   AND profile.identity_strength='Strong'
                 ORDER BY profile.last_confirmed_at DESC",
            )
            .map_err(crate::db::migration::map_sqlite_error)?;
        let profile_ids = statement
            .query_map([&target.library_id], |row| row.get::<_, Vec<u8>>(0))
            .map_err(crate::db::migration::map_sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(crate::db::migration::map_sqlite_error)?;
        drop(statement);
        drop(connection);
        for id in profile_ids {
            let mut profile = self.load_profile(&id)?;
            for candidate in prefill_candidates(&profile) {
                let Ok(probe) = probe_directory(&candidate) else {
                    continue;
                };
                if probe.strong_identity
                    && probe.volume_identity_key == profile.volume_key
                    && probe.directory_file_id == profile.directory_file_id
                {
                    profile.display_path = probe.absolute_path.to_string_lossy().into_owned();
                    return Ok(Some(profile));
                }
            }
        }
        Ok(None)
    }

    fn has_source_profiles(&self) -> Result<bool, DomainError> {
        let target = match self.current_target() {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };
        self.database
            .open_read_connection()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM import_source_profile
                               WHERE library_id=?1 AND retired_at IS NULL)",
                [&target.library_id],
                |row| row.get(0),
            )
            .map_err(crate::db::migration::map_sqlite_error)
    }

    fn active_task_reason(&self) -> Result<Option<String>, DomainError> {
        let active = self
            .database
            .open_read_connection()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM task_intent
                           WHERE lifecycle NOT IN ('Completed','Cancelled'))",
                (),
                |row| row.get::<_, bool>(0),
            )
            .map_err(crate::db::migration::map_sqlite_error)?;
        Ok(active.then(|| "请先完成或取消当前照片文件任务。".to_string()))
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, RuntimeState>, DomainError> {
        self.state.lock().map_err(|_| {
            DomainError::new(ErrorCode::Internal, "只读预检状态不可用，请重新启动应用。")
        })
    }
}

fn inspect_source_file(
    path: &Path,
    kind: MediaKind,
    audit: &ProcessIoAudit,
) -> Result<(FileProbe, [u8; 32], media::MediaInspection), DomainError> {
    let mut handle = StableReadHandle::open(path, AuditSide::Source, audit)?;
    let before = handle.probe(audit)?;
    if before.reparse_point {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "照片文件是 reparse point。",
        ));
    }
    let inspection = media::inspect(handle.file_mut(), kind).unwrap_or(media::MediaInspection {
        exif: media::ExifMetadata::default(),
        preview: media::PreviewState::Unavailable,
    });
    let full_hash = handle.full_hash(audit)?;
    let after = handle.probe(audit)?;
    if before != after {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "完整读取期间来源文件发生变化。",
        ));
    }
    Ok((before, full_hash, inspection))
}

fn supported_kind(path: &Path) -> Option<(DraftMediaKind, MediaKind, String)> {
    let extension = path.extension()?.to_str()?.to_string();
    match extension.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some((DraftMediaKind::Jpg, MediaKind::Jpg, extension)),
        "raf" => Some((DraftMediaKind::Raf, MediaKind::Raf, extension)),
        _ => None,
    }
}

fn validate_target_file_name(file_name: &str) -> Result<(), DomainError> {
    if file_name.is_empty()
        || file_name.len() > 240
        || file_name.ends_with([' ', '.'])
        || file_name
            .chars()
            .any(|value| value < ' ' || "<>:\"/\\|?*".contains(value))
    {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "文件名不符合 Windows 安全名称规则。",
        ));
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if reserved {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "文件名使用了 Windows 保留名称。",
        ));
    }
    Ok(())
}

fn source_view(path: &Path, volume_key: &[u8]) -> ImportSourceView {
    ImportSourceView {
        volume_name: volume_display_name(path, volume_key),
        display_path: path.to_string_lossy().into_owned(),
        recursively_scans_selected_subtree: true,
    }
}

fn volume_display_name(path: &Path, volume_key: &[u8]) -> String {
    path.components()
        .next()
        .map(|value| value.as_os_str().to_string_lossy().into_owned())
        .filter(|value| !value.is_empty() && value != "\\")
        .unwrap_or_else(|| {
            format!(
                "内存卡 {:02X}{:02X}",
                volume_key.first().copied().unwrap_or(0),
                volume_key.get(1).copied().unwrap_or(0)
            )
        })
}

fn volume_relative_path(path: &Path) -> String {
    let mut relative = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        relative.push(component.as_os_str());
    }
    relative.to_string_lossy().replace('/', "\\")
}

fn prefill_candidates(profile: &ProfileRecord) -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from(&profile.display_path)];
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", char::from(letter)));
        let candidate = root.join(&profile.source_relative_path);
        if !candidates
            .iter()
            .any(|value| path_key(value) == path_key(&candidate))
        {
            candidates.push(candidate);
        }
    }
    candidates
}

pub(super) fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

fn source_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::RootOffline, "无法继续只读枚举所选内存卡子树。")
        .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests;
