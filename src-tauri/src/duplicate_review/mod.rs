mod proof;
mod reconciliation;
mod scanner;

pub(crate) use proof::{
    StableHashObservation, StablePairObservation, StablePairProofInput, stable_pair_proof,
    stable_pair_stream_proof,
};
pub(crate) use reconciliation::{DuplicateReconciliationState, DuplicateResultReconciler};

#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{AssetId, DomainError, ErrorCode, LibraryId},
    io_scheduler::IoScheduler,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateReviewState {
    Idle,
    Running,
    CompletedEmpty,
    CompletedWithDuplicates,
    Expired,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReviewStatus {
    pub state: DuplicateReviewState,
    pub run_id: Option<String>,
    pub phase: Option<String>,
    pub checked_files: u64,
    pub candidate_files: u64,
    pub hashed_files: u64,
    pub current_relative_path: Option<String>,
    pub family_count: usize,
    pub expired_reason: Option<String>,
    pub result_version_id: Option<String>,
    pub last_receipt: Option<DuplicateReviewReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReviewReceipt {
    pub reconciliation_id: String,
    pub batch_id: String,
    pub state: String,
    pub recycled_files: usize,
    pub recycled_bytes: i64,
    pub completed_at: i64,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReviewSummary {
    pub equivalence_sets: usize,
    pub jpg_only_families: usize,
    pub raf_only_families: usize,
    pub jpg_and_raf_families: usize,
    pub redundant_files: usize,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMemberResult {
    pub asset_id: String,
    pub asset_version: i64,
    pub role: String,
    pub full_path: String,
    pub directory: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub capture_local_key: Option<String>,
    pub photo_id: Option<String>,
    pub online: bool,
    pub selectable: bool,
    pub suggested_retain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSetResult {
    pub set_id: String,
    pub role: String,
    pub file_size_bytes: u64,
    pub redundant_files: usize,
    pub reclaimable_bytes: u64,
    pub suggested_retained_asset_id: Option<String>,
    pub members: Vec<DuplicateMemberResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFamilyResult {
    pub family_id: String,
    pub jpg: Option<DuplicateSetResult>,
    pub raf: Option<DuplicateSetResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReviewResults {
    pub run_id: Option<String>,
    pub result_version_id: Option<String>,
    pub expired: bool,
    pub expired_reason: Option<String>,
    pub last_receipt: Option<DuplicateReviewReceipt>,
    pub summary: DuplicateReviewSummary,
    pub families: Vec<DuplicateFamilyResult>,
}

type LatestRunRow = (
    Vec<u8>,
    Vec<u8>,
    i64,
    String,
    String,
    i64,
    i64,
    i64,
    Option<String>,
);

#[derive(Debug)]
pub struct DuplicateReviewService {
    database: Arc<Database>,
    scheduler: Arc<IoScheduler>,
    cancel_requested: Arc<AtomicBool>,
    worker_active: Arc<AtomicBool>,
}

impl DuplicateReviewService {
    pub fn new(database: Arc<Database>, scheduler: Arc<IoScheduler>) -> Self {
        Self {
            database,
            scheduler,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            worker_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(self: &Arc<Self>) -> Result<DuplicateReviewStatus, DomainError> {
        if self
            .worker_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "已有重复检测正在运行。",
            ));
        }
        self.cancel_requested.store(false, Ordering::Release);
        let service = Arc::clone(self);
        std::thread::Builder::new()
            .name("duplicate-review-scan".into())
            .spawn(move || {
                let _guard = WorkerGuard(Arc::clone(&service.worker_active));
                let _ = scanner::execute(
                    &service.database,
                    &service.scheduler,
                    &service.cancel_requested,
                );
            })
            .map_err(|error| {
                self.worker_active.store(false, Ordering::Release);
                DomainError::new(ErrorCode::Internal, "无法启动重复检测后台任务。")
                    .with_diagnostic(error.to_string())
            })?;
        Ok(DuplicateReviewStatus {
            state: DuplicateReviewState::Running,
            run_id: None,
            phase: Some("Enumerating".into()),
            checked_files: 0,
            candidate_files: 0,
            hashed_files: 0,
            current_relative_path: None,
            family_count: 0,
            expired_reason: None,
            result_version_id: None,
            last_receipt: None,
        })
    }

    pub fn cancel(&self) -> Result<DuplicateReviewStatus, DomainError> {
        self.cancel_requested.store(true, Ordering::Release);
        if !self.worker_active.load(Ordering::Acquire) {
            self.finish_orphaned_runs("CancelledWithoutWorker")?;
        }
        self.status()
    }

    pub fn reconcile_startup(&self) -> Result<usize, DomainError> {
        self.finish_orphaned_runs("ProcessInterrupted")
    }

    fn finish_orphaned_runs(&self, error_code: &'static str) -> Result<usize, DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE duplicate_scan_run
                     SET run_state='Cancelled', phase='Finished', error_code=?1,
                         completed_at=?2, current_relative_path=NULL
                     WHERE run_state='Running'",
                    (error_code, now_micros()),
                )
                .map_err(map_sqlite_error)
        })
    }

    pub fn status(&self) -> Result<DuplicateReviewStatus, DomainError> {
        let connection = self.database.open_read_connection()?;
        let row: Option<LatestRunRow> = connection
            .query_row(
                "SELECT run_id, root_binding_id, catalog_revision, run_state, phase,
                            checked_files, candidate_files, hashed_files, current_relative_path
                     FROM duplicate_scan_run ORDER BY started_at DESC LIMIT 1",
                (),
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?;
        let Some((
            run_id,
            binding,
            revision,
            run_state,
            phase,
            checked,
            candidates,
            hashed,
            current,
        )) = row
        else {
            return Ok(idle_status());
        };
        let reconciliation = latest_reconciliation(&connection, &run_id)?;
        let effective_revision = reconciliation
            .as_ref()
            .filter(|value| value.state == "Applied")
            .map_or(revision, |value| value.to_revision);
        let (expired, expired_reason) = if reconciliation
            .as_ref()
            .is_some_and(|value| value.state == "Rejected")
        {
            (
                true,
                reconciliation
                    .as_ref()
                    .and_then(|value| value.rejection_reason.clone()),
            )
        } else if run_state == "Completed" {
            completion_expiry(&connection, &binding, effective_revision)?
        } else {
            (false, None)
        };
        let set_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM duplicate_equivalence_set WHERE run_id=?1",
                [&run_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        let family_count = if set_count == 0 {
            0
        } else {
            load_families(&connection, &run_id, false)?.len()
        };
        let state = if expired {
            DuplicateReviewState::Expired
        } else {
            match run_state.as_str() {
                "Running" => DuplicateReviewState::Running,
                "Completed" if family_count == 0 => DuplicateReviewState::CompletedEmpty,
                "Completed" => DuplicateReviewState::CompletedWithDuplicates,
                "Cancelled" => DuplicateReviewState::Cancelled,
                _ => DuplicateReviewState::Failed,
            }
        };
        Ok(DuplicateReviewStatus {
            state,
            run_id: Some(uuid_string(&run_id)?),
            phase: Some(phase),
            checked_files: u64::try_from(checked).unwrap_or(0),
            candidate_files: u64::try_from(candidates).unwrap_or(0),
            hashed_files: u64::try_from(hashed).unwrap_or(0),
            current_relative_path: current,
            family_count,
            expired_reason,
            result_version_id: Some(
                reconciliation
                    .as_ref()
                    .map_or_else(|| uuid_string(&run_id), |value| uuid_string(&value.id))?,
            ),
            last_receipt: reconciliation.map(|value| value.receipt()).transpose()?,
        })
    }

    pub fn results(&self) -> Result<DuplicateReviewResults, DomainError> {
        let status = self.status()?;
        let Some(run_id) = status.run_id.clone() else {
            return Ok(DuplicateReviewResults {
                run_id: None,
                result_version_id: None,
                expired: false,
                expired_reason: None,
                last_receipt: None,
                summary: DuplicateReviewSummary::default(),
                families: Vec::new(),
            });
        };
        let run_bytes = uuid::Uuid::parse_str(&run_id)
            .map_err(|_| DomainError::new(ErrorCode::InvalidId, "重复检测 Run ID 无效。"))?
            .as_bytes()
            .to_vec();
        let connection = self.database.open_read_connection()?;
        let mut families = load_families(&connection, &run_bytes, true)?;
        let summary = summarize(&families);
        let expired = status.state == DuplicateReviewState::Expired;
        if expired {
            families.clear();
        }
        Ok(DuplicateReviewResults {
            run_id: Some(run_id),
            result_version_id: status.result_version_id,
            expired,
            expired_reason: status.expired_reason,
            last_receipt: status.last_receipt,
            summary,
            families,
        })
    }

    #[cfg(test)]
    pub(crate) fn scan_blocking_for_test(&self) -> Result<(), DomainError> {
        scanner::execute(&self.database, &self.scheduler, &self.cancel_requested)
    }

    pub fn database(&self) -> &Arc<Database> {
        &self.database
    }
}

struct WorkerGuard(Arc<AtomicBool>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn idle_status() -> DuplicateReviewStatus {
    DuplicateReviewStatus {
        state: DuplicateReviewState::Idle,
        run_id: None,
        phase: None,
        checked_files: 0,
        candidate_files: 0,
        hashed_files: 0,
        current_relative_path: None,
        family_count: 0,
        expired_reason: None,
        result_version_id: None,
        last_receipt: None,
    }
}

fn completion_expiry(
    connection: &rusqlite::Connection,
    binding: &[u8],
    revision: i64,
) -> Result<(bool, Option<String>), DomainError> {
    let current: Option<(Vec<u8>, i64, String, String)> = connection
        .query_row(
            "SELECT revision.root_binding_id, revision.catalog_revision,
                    root.lifecycle, binding.binding_state
             FROM current_library AS current
             JOIN library_catalog_revision AS revision
               ON revision.library_id=current.library_id
             JOIN root ON root.library_id=current.library_id
             JOIN root_binding AS binding ON binding.binding_id=revision.root_binding_id
             WHERE current.slot=1",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    match current {
        None => Ok((true, Some("当前图库 RootBinding 不可用。".into()))),
        Some((current_binding, _, _, _)) if current_binding != binding => {
            Ok((true, Some("图库 RootBinding 已改变。".into())))
        }
        Some((_, _, root_state, binding_state))
            if root_state != "Online" || binding_state != "Active" =>
        {
            Ok((true, Some("图库 Root 当前离线或身份不可用。".into())))
        }
        Some((_, current_revision, _, _)) if current_revision != revision => {
            Ok((true, Some("图库内容或文件身份已改变。".into())))
        }
        _ => Ok((false, None)),
    }
}

#[derive(Debug)]
struct SetBuild {
    result: DuplicateSetResult,
    photo_ids: HashSet<String>,
    pairing_keys: HashSet<String>,
}

#[derive(Debug)]
struct ReconciliationSnapshot {
    id: Vec<u8>,
    batch_id: Vec<u8>,
    state: String,
    to_revision: i64,
    removed_assets: i64,
    removed_bytes: i64,
    rejection_reason: Option<String>,
    completed_at: i64,
}

impl ReconciliationSnapshot {
    fn receipt(self) -> Result<DuplicateReviewReceipt, DomainError> {
        let applied = self.state == "Applied";
        Ok(DuplicateReviewReceipt {
            reconciliation_id: uuid_string(&self.id)?,
            batch_id: uuid_string(&self.batch_id)?,
            state: self.state.to_lowercase(),
            recycled_files: usize::try_from(self.removed_assets).unwrap_or(0),
            recycled_bytes: self.removed_bytes,
            completed_at: self.completed_at,
            message: if applied {
                "图库索引和重复结果已更新，无须重新检测。".into()
            } else {
                self.rejection_reason
                    .unwrap_or_else(|| "现有重复结果无法安全沿用。".into())
            },
        })
    }
}

fn latest_reconciliation(
    connection: &rusqlite::Connection,
    run_id: &[u8],
) -> Result<Option<ReconciliationSnapshot>, DomainError> {
    connection
        .query_row(
            "SELECT reconciliation_id,recycle_batch_id,reconciliation_state,
                    to_catalog_revision,removed_asset_count,removed_bytes,
                    rejection_reason,completed_at
             FROM duplicate_result_reconciliation
             WHERE source_run_id=?1 AND reconciliation_state!='Pending'
             ORDER BY completed_at DESC LIMIT 1",
            [run_id],
            |row| {
                Ok(ReconciliationSnapshot {
                    id: row.get(0)?,
                    batch_id: row.get(1)?,
                    state: row.get(2)?,
                    to_revision: row.get(3)?,
                    removed_assets: row.get(4)?,
                    removed_bytes: row.get(5)?,
                    rejection_reason: row.get(6)?,
                    completed_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite_error)
}

fn load_families(
    connection: &rusqlite::Connection,
    run_id: &[u8],
    include_paths: bool,
) -> Result<Vec<DuplicateFamilyResult>, DomainError> {
    let root_path: String = connection
        .query_row(
            "SELECT binding.absolute_path_display
             FROM duplicate_scan_run AS run
             JOIN root_binding AS binding ON binding.binding_id=run.root_binding_id
             WHERE run.run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let mut statement = connection
        .prepare(
            "SELECT equivalent_set.set_id, equivalent_set.role,
                    equivalent_set.file_size_bytes, observation.asset_id,
                    observation.asset_version, revision.relative_path,
                    metadata.capture_local_key, photo.photo_id
             FROM duplicate_equivalence_set AS equivalent_set
             JOIN (
                 SELECT set_id, anchor_observation_id AS observation_id
                 FROM duplicate_equivalence_proof
                 UNION
                 SELECT set_id, member_observation_id AS observation_id
                 FROM duplicate_equivalence_proof
             ) AS membership ON membership.set_id=equivalent_set.set_id
             JOIN duplicate_file_observation AS observation
               ON observation.observation_id=membership.observation_id
             JOIN asset_revision AS revision
               ON revision.asset_id=observation.asset_id
              AND revision.asset_version=observation.asset_version
             LEFT JOIN asset_media_metadata AS metadata
               ON metadata.asset_id=observation.asset_id
              AND metadata.asset_version=observation.asset_version
             LEFT JOIN photo_asset AS relation
               ON relation.asset_id=observation.asset_id AND relation.to_photo_version IS NULL
             LEFT JOIN photo ON photo.photo_id=relation.photo_id AND photo.lifecycle='Active'
             WHERE equivalent_set.run_id=?1
               AND NOT EXISTS(
                   SELECT 1 FROM duplicate_result_reconciliation_member removed
                   JOIN duplicate_result_reconciliation reconciliation
                     ON reconciliation.reconciliation_id=removed.reconciliation_id
                   WHERE reconciliation.source_run_id=?1
                     AND reconciliation.reconciliation_state='Applied'
                     AND removed.asset_id=observation.asset_id
               )
             ORDER BY equivalent_set.role, equivalent_set.set_id, revision.path_key",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<Vec<u8>>>(7)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut sets: HashMap<Vec<u8>, SetBuild> = HashMap::new();
    for row in rows {
        let (set_id, role, size, asset, version, relative, capture, photo) =
            row.map_err(map_sqlite_error)?;
        let asset_id = uuid_string(&asset)?;
        let full_path = Path::new(&root_path)
            .join(&relative)
            .to_string_lossy()
            .into_owned();
        let directory = Path::new(&full_path)
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let set = sets.entry(set_id.clone()).or_insert_with(|| SetBuild {
            result: DuplicateSetResult {
                set_id: uuid_string(&set_id).unwrap_or_default(),
                role: role.clone(),
                file_size_bytes: u64::try_from(size).unwrap_or(0),
                redundant_files: 0,
                reclaimable_bytes: 0,
                suggested_retained_asset_id: None,
                members: Vec::new(),
            },
            photo_ids: HashSet::new(),
            pairing_keys: HashSet::new(),
        });
        let photo_id = photo.as_deref().map(uuid_string).transpose()?;
        if let Some(photo_id) = &photo_id {
            set.photo_ids.insert(photo_id.clone());
        }
        let stem = Path::new(&relative)
            .file_stem()
            .map(|value| value.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if let Some(capture) = capture.as_deref() {
            set.pairing_keys.insert(format!("{stem}\0{capture}"));
        }
        set.result.members.push(DuplicateMemberResult {
            asset_id,
            asset_version: version,
            role,
            full_path: if include_paths {
                full_path
            } else {
                String::new()
            },
            directory: if include_paths {
                directory
            } else {
                String::new()
            },
            relative_path: relative,
            size_bytes: u64::try_from(size).unwrap_or(0),
            capture_local_key: capture,
            photo_id,
            online: true,
            selectable: true,
            suggested_retain: false,
        });
    }
    let mut sets = sets.into_values().collect::<Vec<_>>();
    sets.retain(|set| set.result.members.len() >= 2);
    sets.sort_by(|left, right| left.result.set_id.cmp(&right.result.set_id));
    for set in &mut sets {
        let redundant = set.result.members.len().saturating_sub(1);
        set.result.redundant_files = redundant;
        set.result.reclaimable_bytes = set
            .result
            .file_size_bytes
            .saturating_mul(u64::try_from(redundant).unwrap_or(u64::MAX));
        if let Some(index) = suggested_retain_index(&set.result.members) {
            set.result.members[index].suggested_retain = true;
            set.result.suggested_retained_asset_id =
                Some(set.result.members[index].asset_id.clone());
        }
    }
    Ok(aggregate_families(sets))
}

fn aggregate_families(mut sets: Vec<SetBuild>) -> Vec<DuplicateFamilyResult> {
    let mut families = Vec::new();
    while let Some(set) = sets.pop() {
        let counterpart_role = if set.result.role == "Jpg" {
            "Raf"
        } else {
            "Jpg"
        };
        let matches = sets
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                candidate.result.role == counterpart_role
                    && (!candidate.photo_ids.is_disjoint(&set.photo_ids)
                        || !candidate.pairing_keys.is_disjoint(&set.pairing_keys))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let counterpart = (matches.len() == 1).then(|| sets.remove(matches[0]));
        let (jpg, raf) = if set.result.role == "Jpg" {
            (Some(set.result), counterpart.map(|value| value.result))
        } else {
            (counterpart.map(|value| value.result), Some(set.result))
        };
        let family_id = format!(
            "{}:{}",
            jpg.as_ref().map(|set| set.set_id.as_str()).unwrap_or(""),
            raf.as_ref().map(|set| set.set_id.as_str()).unwrap_or("")
        );
        families.push(DuplicateFamilyResult {
            family_id,
            jpg,
            raf,
        });
    }
    families.sort_by(|left, right| left.family_id.cmp(&right.family_id));
    families
}

fn suggested_retain_index(members: &[DuplicateMemberResult]) -> Option<usize> {
    members
        .iter()
        .enumerate()
        .filter(|(_, member)| member.online)
        .min_by_key(|(_, member)| {
            (
                !is_canonical_capture_path(member),
                member.relative_path.chars().count(),
                member.relative_path.to_lowercase(),
            )
        })
        .map(|(index, _)| index)
}

fn is_canonical_capture_path(member: &DuplicateMemberResult) -> bool {
    let Some(capture) = member.capture_local_key.as_deref() else {
        return false;
    };
    let Some(day) = capture.get(..10) else {
        return false;
    };
    let compact = day.replace('-', "");
    if compact.len() != 8 {
        return false;
    }
    let year = &compact[..4];
    let normalized = member.relative_path.replace('/', "\\").to_lowercase();
    let parent = Path::new(&normalized)
        .parent()
        .map(|path| path.to_string_lossy().replace('/', "\\"))
        .unwrap_or_default();
    if member.role == "Raf" {
        parent == format!("{year}\\{compact}\\raw")
    } else {
        parent == format!("{year}\\{compact}")
    }
}

fn summarize(families: &[DuplicateFamilyResult]) -> DuplicateReviewSummary {
    let mut summary = DuplicateReviewSummary::default();
    for family in families {
        match (family.jpg.is_some(), family.raf.is_some()) {
            (true, true) => summary.jpg_and_raf_families += 1,
            (true, false) => summary.jpg_only_families += 1,
            (false, true) => summary.raf_only_families += 1,
            _ => {}
        }
        for set in [family.jpg.as_ref(), family.raf.as_ref()]
            .into_iter()
            .flatten()
        {
            summary.equivalence_sets += 1;
            summary.redundant_files += set.redundant_files;
            summary.reclaimable_bytes = summary
                .reclaimable_bytes
                .saturating_add(set.reclaimable_bytes);
        }
    }
    summary
}

fn uuid_string(bytes: &[u8]) -> Result<String, DomainError> {
    uuid::Uuid::from_slice(bytes)
        .map(|value| value.to_string())
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "重复检测对象标识损坏。"))
}

pub fn validate_selection(
    set: &DuplicateSetResult,
    selected_asset_ids: &[String],
) -> Result<(), DomainError> {
    let members = set
        .members
        .iter()
        .map(|member| member.asset_id.as_str())
        .collect::<HashSet<_>>();
    let selected = selected_asset_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if selected.is_empty() || !selected.is_subset(&members) || selected.len() >= members.len() {
        return Err(DomainError::new(
            ErrorCode::InvalidTransition,
            "每个重复集合必须至少保留一份未选择的文件。",
        ));
    }
    Ok(())
}

pub fn current_library_id(database: &Database) -> Result<LibraryId, DomainError> {
    let bytes: Vec<u8> = database
        .open_read_connection()?
        .query_row(
            "SELECT library_id FROM current_library WHERE slot=1",
            (),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    Ok(LibraryId::from_uuid(
        uuid::Uuid::from_slice(&bytes)
            .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "当前 Library ID 损坏。"))?,
    ))
}

pub fn parse_asset_id(value: &str) -> Result<AssetId, DomainError> {
    value.parse()
}
