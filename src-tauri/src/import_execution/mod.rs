mod file_ops;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    catalog::{AssetObservation, CatalogService, MediaKind},
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode, LibraryId, RootBindingId, RootId},
    import_preflight::{
        DraftDisposition, DraftMediaKind, ExecutionDraftSeed, ImportDraftItem,
        ImportPreflightDraft, ImportPreflightService, SourceFile,
        proof::{
            AuditSide, BoundPath, DuplicateProof, ProcessIoAudit, StableReadHandle,
            prove_duplicate_and_commit, prove_duplicate_guard,
        },
    },
    roots::{
        DirectoryProbe, FileProbe, FilesystemKind, path_key, probe_directory,
        resolve_contained_path,
    },
    safety::{
        FaultInjector, FaultPoint, FeatureGate, GateCloseReason, RuntimeSafetyGate, SafetyPermit,
    },
};

use file_ops::{PartialWriteHandle, VerifiedPartialHandle, disk_space, hash_and_compare};

pub const IMPORT_MIN_RESERVE_BYTES: u64 = 10_737_418_240;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhaseEightImportFileOpsReport {
    pub source_write_requests: u64,
    pub source_unchanged: bool,
    pub existing_target_unchanged: bool,
    pub successful_publish_identical: bool,
    pub no_overwrite_rejected: bool,
    pub fault_before_publish_rejected: bool,
    pub permanent_delete_used: bool,
}

pub(crate) fn phase_eight_file_ops_suite(
    suite_root: &Path,
) -> Result<PhaseEightImportFileOpsReport, DomainError> {
    fs::create_dir(suite_root).map_err(phase_eight_suite_io)?;
    let source_root = suite_root.join("generated-source-volume");
    let target_root = suite_root.join("generated-target-library");
    fs::create_dir(&source_root).map_err(phase_eight_suite_io)?;
    fs::create_dir(&target_root).map_err(phase_eight_suite_io)?;
    let source = source_root.join("GENERATED.JPG");
    let source_bytes = b"phase-eight-generated-source-jpeg-v1";
    create_new_bytes(&source, source_bytes)?;
    let source_before = file_snapshot(&source)?;

    let partial = target_root.join("success.partial");
    let formal = target_root.join("GENERATED.JPG");
    copy_publish(&source, &partial, &formal, &FaultInjector::default())?;
    let successful_publish_identical =
        fs::read(&formal).map_err(phase_eight_suite_io)? == source_bytes;

    let existing = target_root.join("EXISTING.JPG");
    let existing_bytes = b"must-remain-byte-identical";
    create_new_bytes(&existing, existing_bytes)?;
    let conflict_partial = target_root.join("conflict.partial");
    let conflict = copy_publish(
        &source,
        &conflict_partial,
        &existing,
        &FaultInjector::default(),
    );
    let no_overwrite_rejected = conflict.is_err();
    let existing_target_unchanged =
        fs::read(&existing).map_err(phase_eight_suite_io)? == existing_bytes;

    let fault_partial = target_root.join("fault.partial");
    let fault_final = target_root.join("FAULT.JPG");
    let faults = FaultInjector::default();
    faults.fail_on_hit(FaultPoint::Win32BeforeCall, 1)?;
    let fault = copy_publish(&source, &fault_partial, &fault_final, &faults);
    let fault_before_publish_rejected = fault
        .as_ref()
        .is_err_and(|error| error.code == ErrorCode::FaultInjected)
        && !fault_final.exists();
    let source_unchanged = file_snapshot(&source)? == source_before;

    let report = PhaseEightImportFileOpsReport {
        source_write_requests: 0,
        source_unchanged,
        existing_target_unchanged,
        successful_publish_identical,
        no_overwrite_rejected,
        fault_before_publish_rejected,
        permanent_delete_used: false,
    };
    if !report.source_unchanged
        || !report.existing_target_unchanged
        || !report.successful_publish_identical
        || !report.no_overwrite_rejected
        || !report.fault_before_publish_rejected
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "阶段 8 Import 文件动作安全套件未通过。",
        ));
    }
    Ok(report)
}

fn copy_publish(
    source_path: &Path,
    partial_path: &Path,
    final_path: &Path,
    faults: &FaultInjector,
) -> Result<(), DomainError> {
    let mut source = crate::roots::open_read_only(source_path)?;
    let expected = source.metadata().map_err(phase_eight_suite_io)?.len();
    let mut partial = PartialWriteHandle::create_new(partial_path, faults)?;
    let copied_hash = partial.copy_from(&mut source, expected, || false)?;
    partial.flush(faults)?;
    let mut verified = VerifiedPartialHandle::open(partial_path)?;
    let (source_hash, partial_hash, identical) =
        hash_and_compare(&mut source, verified.file_mut())?;
    if !identical || source_hash != partial_hash || source_hash != copied_hash {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "阶段 8 Import partial 完整验证失败。",
        ));
    }
    verified.publish_no_replace(final_path, faults)
}

fn create_new_bytes(path: &Path, bytes: &[u8]) -> Result<(), DomainError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(phase_eight_suite_io)?;
    file.write_all(bytes).map_err(phase_eight_suite_io)?;
    file.sync_all().map_err(phase_eight_suite_io)
}

fn file_snapshot(path: &Path) -> Result<(u64, i64, Vec<u8>), DomainError> {
    let file = crate::roots::open_read_only(path)?;
    let probe = crate::roots::probe_file(&file)?;
    let bytes = fs::read(path).map_err(phase_eight_suite_io)?;
    Ok((
        probe.size_bytes,
        probe.mtime_filetime,
        Sha256::digest(bytes).to_vec(),
    ))
}

fn phase_eight_suite_io(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::Internal, "阶段 8 Import 生成夹具 I/O 失败。")
        .with_diagnostic(error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportExecutionPhase {
    Idle,
    ValidatingPlan,
    AwaitingPlanConfirmation,
    AwaitingSpace,
    Running,
    AwaitingSource,
    NeedsAttention,
    Completed,
    CompletedWithErrors,
    Cancelled,
}

fn insert_source_items(
    transaction: &Transaction<'_>,
    job_id: &[u8],
    seed: &ExecutionDraftSeed,
    now: i64,
) -> Result<HashMap<String, Vec<u8>>, DomainError> {
    let readable = seed
        .enumerated
        .files
        .iter()
        .map(|file| {
            (
                crate::import_preflight::display_relative(&file.relative_path).to_lowercase(),
                file,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut result = HashMap::new();
    for item in &seed.draft.items {
        let key = item.source_relative_path.to_lowercase();
        let source_item = id();
        let relative = Path::new(&item.source_relative_path);
        let source_key = path_key(relative);
        let (probe, hash, capture, parse) = if let Some(file) = readable.get(&key) {
            (
                file.identity.clone(),
                Some(file.full_hash),
                file.capture_local_key.clone(),
                "Readable",
            )
        } else {
            (
                FileProbe {
                    file_id: None,
                    size_bytes: item.size_bytes,
                    mtime_filetime: 0,
                    hard_link_count: 1,
                    reparse_point: false,
                },
                None,
                None,
                "Unreadable",
            )
        };
        let snapshot = insert_snapshot(
            transaction,
            &seed.source.root_id,
            &seed.source.binding_id,
            &seed.source.volume_id,
            &item.source_relative_path,
            &source_key,
            &probe,
            hash.as_ref(),
            now,
        )?;
        let extension = relative
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        transaction.execute("INSERT INTO import_source_item(source_item_id,job_id,source_root_id,source_path_key,source_file_id,source_snapshot_id,original_filename,role,created_at,source_relative_path,original_extension,capture_local_key,parse_result) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![&source_item,job_id,&seed.source.root_id,&source_key,&probe.file_id,&snapshot,&item.file_name,role_sql(item.media_kind),now,&item.source_relative_path,extension,&capture,parse]).map_err(map_sqlite_error)?;
        result.insert(key, source_item);
    }
    Ok(result)
}
fn load_source_item_map(
    transaction: &Transaction<'_>,
    job_id: &[u8],
) -> Result<HashMap<String, Vec<u8>>, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT source_relative_path,source_item_id FROM import_source_item WHERE job_id=?1",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([job_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(map_sqlite_error)?;
    let mut result = HashMap::new();
    for row in rows {
        let (path, value) = row.map_err(map_sqlite_error)?;
        result.insert(path.to_lowercase(), value);
    }
    Ok(result)
}
#[allow(clippy::too_many_arguments)] // The arguments mirror one immutable generation record.
fn insert_generation(
    transaction: &Transaction<'_>,
    job_id: &[u8],
    generation_no: i64,
    state: &str,
    reason: &str,
    supersedes: Option<&[u8]>,
    seed: &ExecutionDraftSeed,
    source_items: &HashMap<String, Vec<u8>>,
    now: i64,
) -> Result<Vec<u8>, DomainError> {
    let generation = id();
    transaction.execute("INSERT INTO import_plan_generation(generation_id,job_id,generation_no,generation_state,target_root_id,target_binding_id,planned_source_files,planned_copy_files,planned_new_bytes,generation_reason,created_at,confirmed_at,supersedes_generation_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![&generation,job_id,generation_no,state,&seed.target.root_id,&seed.target.binding_id,i64::try_from(seed.draft.items.len()).unwrap_or(i64::MAX),i64::try_from(seed.draft.summary.new_files).unwrap_or(i64::MAX),i64::try_from(seed.draft.summary.new_bytes).unwrap_or(i64::MAX),reason,now,(state=="Confirmed").then_some(now),supersedes]).map_err(map_sqlite_error)?;
    let digest = hex::decode(&seed.draft.draft_digest).map_err(|error| {
        DomainError::new(ErrorCode::Internal, "计划摘要无效。").with_diagnostic(error.to_string())
    })?;
    transaction
        .execute(
            "INSERT INTO generation_plan_digest(generation_id,digest) VALUES(?1,?2)",
            params![&generation, &digest],
        )
        .map_err(map_sqlite_error)?;
    let mut groups: BTreeMap<String, Vec<&ImportDraftItem>> = BTreeMap::new();
    for item in &seed.draft.items {
        groups
            .entry(item.logical_group.clone())
            .or_default()
            .push(item)
    }
    for (logical, items) in groups {
        let group = id();
        let suffix = items
            .iter()
            .filter_map(|item| item.target_relative_path.as_deref())
            .filter_map(suffix_ordinal)
            .max()
            .unwrap_or(1);
        let pair = (items.len() > 1).then_some("SameImportGroup");
        let date_class = if items.iter().any(|item| item.date_unknown) {
            "Unknown"
        } else {
            "Known"
        };
        transaction.execute("INSERT INTO import_group(group_id,generation_id,group_key,pair_evidence,target_date_class,name_family_key,suffix_ordinal) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![&group,&generation,Sha256::digest(logical.as_bytes()).as_slice(),pair,date_class,Sha256::digest(logical.to_lowercase().as_bytes()).as_slice(),i64::from(suffix)]).map_err(map_sqlite_error)?;
        for item in items {
            let source_item = source_items
                .get(&item.source_relative_path.to_lowercase())
                .ok_or_else(|| DomainError::new(ErrorCode::Internal, "计划源项目映射缺失。"))?;
            let step = id();
            let plan_kind = match item.disposition {
                DraftDisposition::Skip => "SkipDuplicate",
                DraftDisposition::Unreadable => "Unreadable",
                DraftDisposition::New | DraftDisposition::Conflict => "Copy",
            };
            let final_key = item
                .target_relative_path
                .as_ref()
                .map(|path| path_key(Path::new(path)));
            let expected = matches!(
                item.disposition,
                DraftDisposition::New | DraftDisposition::Conflict
            )
            .then_some(i64::try_from(item.size_bytes).unwrap_or(i64::MAX));
            let namespace = expected.map(|_| {
                Sha256::digest([job_id, generation.as_slice(), source_item.as_slice()].concat())
                    .to_vec()
            });
            let seed_value = expected.map(|_| Uuid::new_v4().as_bytes().to_vec());
            transaction.execute("INSERT INTO import_asset_step(step_id,generation_id,source_item_id,group_id,role,plan_kind,final_root_id,final_binding_id,final_relative_path,final_path_key,partial_namespace,partial_seed,expected_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![&step,&generation,source_item,&group,role_sql(item.media_kind),plan_kind,(plan_kind!="Unreadable").then_some(seed.target.root_id.as_slice()),(plan_kind!="Unreadable").then_some(seed.target.binding_id.as_slice()),&item.target_relative_path,&final_key,&namespace,&seed_value,expected]).map_err(map_sqlite_error)?;
        }
    }
    Ok(generation)
}
#[allow(clippy::too_many_arguments)] // Proof endpoints stay explicit to prevent cross-chain mixing.
fn insert_duplicate_proof(
    transaction: &Transaction<'_>,
    scope: &str,
    step_id: &[u8],
    validation: Option<&[u8]>,
    attempt_step: Option<&[u8]>,
    lease_generation: Option<i64>,
    source_reservation: Option<&[u8]>,
    target_reservation: Option<&[u8]>,
    proof: &DuplicateProof,
    seed: &ExecutionDraftSeed,
    target_asset: Option<&[u8]>,
) -> Result<(), DomainError> {
    let target_relative: String = transaction
        .query_row(
            "SELECT final_relative_path FROM import_asset_step WHERE step_id=?1",
            [step_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let source_relative:String=transaction.query_row("SELECT item.source_relative_path FROM import_asset_step AS step JOIN import_source_item AS item ON item.source_item_id=step.source_item_id WHERE step.step_id=?1",[step_id],|row|row.get(0)).map_err(map_sqlite_error)?;
    let target_asset=match target_asset{Some(value)=>Some(value.to_vec()),None=>transaction.query_row("SELECT asset_id FROM asset WHERE root_id=?1 AND current_path_key=?2 AND presence_state='Present'",params![&seed.target.root_id,path_key(Path::new(&target_relative))],|row|row.get(0)).optional().map_err(map_sqlite_error)?}.ok_or_else(||DomainError::new(ErrorCode::IdentityMismatch,"重复目标尚未由 CatalogService 建档。"))?;
    let source_before = insert_snapshot(
        transaction,
        &seed.source.root_id,
        &seed.source.binding_id,
        &seed.source.volume_id,
        &source_relative,
        &path_key(Path::new(&source_relative)),
        &proof.source_before,
        Some(&proof.source_hash),
        now_micros(),
    )?;
    let source_after = insert_snapshot(
        transaction,
        &seed.source.root_id,
        &seed.source.binding_id,
        &seed.source.volume_id,
        &source_relative,
        &path_key(Path::new(&source_relative)),
        &proof.source_after,
        Some(&proof.source_hash),
        now_micros(),
    )?;
    let target_volume: Vec<u8> = transaction
        .query_row(
            "SELECT volume_id FROM root_binding WHERE binding_id=?1",
            [&seed.target.binding_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let target_before = insert_snapshot(
        transaction,
        &seed.target.root_id,
        &seed.target.binding_id,
        &target_volume,
        &target_relative,
        &path_key(Path::new(&target_relative)),
        &proof.target_before,
        Some(&proof.target_hash),
        now_micros(),
    )?;
    let target_after = insert_snapshot(
        transaction,
        &seed.target.root_id,
        &seed.target.binding_id,
        &target_volume,
        &target_relative,
        &path_key(Path::new(&target_relative)),
        &proof.target_after,
        Some(&proof.target_hash),
        now_micros(),
    )?;
    let proof_id = id();
    transaction.execute("INSERT INTO import_duplicate_proof(duplicate_proof_id,proof_scope,step_id,validation_id,execution_attempt_step_id,lease_generation,source_read_reservation_id,target_existing_reservation_id,source_before_snapshot_id,source_after_snapshot_id,target_before_snapshot_id,target_after_snapshot_id,full_hash_equal,byte_equal,created_at,target_asset_id,source_full_hash,target_full_hash,length_equal) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,1,1,?13,?14,?15,?16,1)",params![&proof_id,scope,step_id,validation,attempt_step,lease_generation,source_reservation,target_reservation,&source_before,&source_after,&target_before,&target_after,now_micros(),&target_asset,proof.source_hash.as_slice(),proof.target_hash.as_slice()]).map_err(map_sqlite_error)?;
    transaction.execute("INSERT INTO import_duplicate_path_binding_proof(path_binding_proof_id,duplicate_proof_id,source_root_id,source_binding_id,source_path_key,source_file_id,target_root_id,target_binding_id,target_path_key,target_file_id,source_contained,target_contained,source_reparse_free,target_reparse_free,source_relative_path,source_path_key_version,source_volume_id,source_binding_generation_before,source_binding_generation_after,target_relative_path,target_path_key_version,target_volume_id,target_binding_generation_before,target_binding_generation_after) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,1,1,1,?11,1,?12,1,1,?13,1,?14,1,1)",params![id(),&proof_id,&seed.source.root_id,&seed.source.binding_id,path_key(Path::new(&source_relative)),proof.source_before.file_id.as_ref().unwrap(),&seed.target.root_id,&seed.target.binding_id,path_key(Path::new(&target_relative)),proof.target_before.file_id.as_ref().unwrap(),&source_relative,&seed.source.volume_id,&target_relative,&target_volume]).map_err(map_sqlite_error)?;
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Snapshot identity fields are intentionally not defaultable.
fn insert_snapshot(
    transaction: &Transaction<'_>,
    root_id: &[u8],
    binding_id: &[u8],
    volume_id: &[u8],
    relative: &str,
    key: &[u8],
    probe: &FileProbe,
    hash: Option<&[u8; 32]>,
    captured: i64,
) -> Result<Vec<u8>, DomainError> {
    let snapshot = id();
    insert_snapshot_with_id(
        transaction,
        &snapshot,
        root_id,
        binding_id,
        volume_id,
        relative,
        key,
        probe,
        hash,
        captured,
    )?;
    Ok(snapshot)
}
#[allow(clippy::too_many_arguments)] // Snapshot identity fields are intentionally not defaultable.
fn insert_snapshot_with_id(
    transaction: &Transaction<'_>,
    snapshot: &[u8],
    root_id: &[u8],
    binding_id: &[u8],
    volume_id: &[u8],
    relative: &str,
    key: &[u8],
    probe: &FileProbe,
    hash: Option<&[u8; 32]>,
    captured: i64,
) -> Result<(), DomainError> {
    transaction.execute("INSERT INTO file_identity_snapshot(snapshot_id,root_id,root_binding_id,volume_id,relative_path,path_key,path_key_version,file_id,size_bytes,mtime_filetime,content_hash_algorithm,content_hash,captured_at,evidence_level) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8,?9,?10,?11,?12,?13)",params![snapshot,root_id,binding_id,volume_id,relative,key,&probe.file_id,i64::try_from(probe.size_bytes).unwrap_or(i64::MAX),probe.mtime_filetime,hash.map(|_|"SHA-256"),hash.map(|value|value.as_slice()),captured,if probe.file_id.is_some(){"Strong"}else{"UserConfirmedLimited"}]).map_err(map_sqlite_error)?;
    Ok(())
}
fn insert_operation(
    transaction: &Transaction<'_>,
    operation_id: &[u8],
    kind: &str,
    target: &[u8],
) -> Result<(), DomainError> {
    transaction.execute("INSERT INTO operation_event(operation_id,operation_kind,target_id,request_hash,first_received_at,related_entity_id) VALUES(?1,?2,?3,?4,?5,?3)",params![operation_id,kind,target,Sha256::digest([kind.as_bytes(),target].concat()).as_slice(),now_micros()]).map_err(map_sqlite_error)?;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn insert_reservation(
    transaction: &Transaction<'_>,
    reservation: &[u8],
    task: &[u8],
    attempt: &[u8],
    lease: i64,
    kind: &str,
    root: &[u8],
    binding: &[u8],
    asset: Option<&[u8]>,
    file: Option<&[u8]>,
    path: Option<&[u8]>,
    now: i64,
) -> Result<(), DomainError> {
    transaction.execute("INSERT INTO operation_reservation(reservation_id,owner_kind,task_id,attempt_id,lease_generation,reservation_kind,root_id,root_binding_id,asset_id,file_id,path_key,created_at) VALUES(?1,'TaskAttempt',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![reservation,task,attempt,lease,kind,root,binding,asset,file,path,now]).map_err(map_sqlite_error)?;
    Ok(())
}
fn require_fence(
    transaction: &Transaction<'_>,
    attempt: &AttemptContext,
    step: &AttemptStep,
) -> Result<(), DomainError> {
    let valid:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM import_attempt AS attempt JOIN file_write_lease AS lease ON lease.slot=1 WHERE attempt.attempt_id=?1 AND attempt.attempt_state='Running' AND lease.task_id=?2 AND lease.lease_generation=?3) AND EXISTS(SELECT 1 FROM import_attempt_step WHERE attempt_step_id=?4 AND attempt_id=?1 AND finished_at IS NULL)",params![&attempt.attempt_id,&attempt.job.job_id,attempt.lease_generation,&step.attempt_step_id],|row|row.get(0)).map_err(map_sqlite_error)?;
    if valid {
        Ok(())
    } else {
        Err(DomainError::new(
            ErrorCode::LeaseGenerationMismatch,
            "Attempt 文件动作授权已失效。",
        ))
    }
}
fn attempt_fence(
    database: &Database,
    attempt: &AttemptContext,
    step: &AttemptStep,
) -> Result<(), DomainError> {
    let connection = database.open_read_connection()?;
    let valid:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM import_attempt AS attempt JOIN file_write_lease AS lease ON lease.slot=1 WHERE attempt.attempt_id=?1 AND attempt.attempt_state='Running' AND lease.task_id=?2 AND lease.lease_generation=?3) AND EXISTS(SELECT 1 FROM import_attempt_step WHERE attempt_step_id=?4 AND attempt_id=?1 AND finished_at IS NULL)",params![&attempt.attempt_id,&attempt.job.job_id,attempt.lease_generation,&step.attempt_step_id],|row|row.get(0)).map_err(map_sqlite_error)?;
    if valid {
        Ok(())
    } else {
        Err(DomainError::new(
            ErrorCode::LeaseGenerationMismatch,
            "Attempt 文件动作授权已失效。",
        ))
    }
}
fn finish_step_row(
    transaction: &Transaction<'_>,
    step: &[u8],
    from: &str,
    to: &str,
    error: Option<&str>,
    warning: Option<&str>,
) -> Result<(), DomainError> {
    let changed=transaction.execute("UPDATE import_attempt_step SET step_state=?3,error_code=?4,warning_code=COALESCE(?5,warning_code),finished_at=CASE WHEN ?3 IN ('PartialVerified','Published') THEN NULL ELSE ?6 END,row_version=row_version+1 WHERE attempt_step_id=?1 AND step_state=?2",params![step,from,to,error,warning,now_micros()]).map_err(map_sqlite_error)?;
    require_one(changed, "attempt step")
}
fn append_report_item(
    transaction: &Transaction<'_>,
    report: &[u8],
    step: &AttemptStep,
    result: &str,
    warning: Option<&str>,
    bytes: u64,
) -> Result<(), DomainError> {
    transaction.execute("INSERT OR IGNORE INTO operation_report_item(report_item_id,report_id,item_kind,stable_item_id,result_code,warning_code,display_path_snapshot,size_bytes) VALUES(?1,?2,'ImportAssetStep',?3,?4,?5,?6,?7)",params![id(),report,&step.step_id,result,warning,&step.final_relative_path,i64::try_from(bytes).unwrap_or(i64::MAX)]).map_err(map_sqlite_error)?;
    Ok(())
}
fn require_one(changed: usize, entity: &'static str) -> Result<(), DomainError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DomainError::stale_version(entity))
    }
}
fn bound_source(seed: &ExecutionDraftSeed, file: &SourceFile) -> BoundPath {
    BoundPath {
        root_id: seed.source.root_id.clone(),
        binding_id: seed.source.binding_id.clone(),
        root_path: seed.source.root_path.clone(),
        relative_path: file.relative_path.clone(),
        expected_root: seed.source.root_probe.clone(),
    }
}
fn bound_target(seed: &ExecutionDraftSeed, relative: &str) -> BoundPath {
    BoundPath {
        root_id: seed.target.root_id.clone(),
        binding_id: seed.target.binding_id.clone(),
        root_path: seed.target.root_path.clone(),
        relative_path: PathBuf::from(relative),
        expected_root: seed.target.root_probe.clone(),
    }
}
fn ensure_parent_directories(
    root: &Path,
    expected_root: &DirectoryProbe,
    final_path: &Path,
    faults: FaultInjector,
) -> Result<(), DomainError> {
    let current_root = probe_directory(root)?;
    if current_root != *expected_root
        || current_root.filesystem_kind != FilesystemKind::Ntfs
        || crate::roots::path_is_reparse(root)?
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "Fuji 图库 Root 身份或 NTFS 能力已变化。",
        ));
    }
    let parent = final_path
        .parent()
        .ok_or_else(|| DomainError::new(ErrorCode::UnsafePath, "目标路径缺少父目录。"))?;
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| DomainError::new(ErrorCode::UnsafePath, "目标目录越过测试 Root。"))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if current.exists() {
            if !current.is_dir() || crate::roots::path_is_reparse(&current)? {
                return Err(DomainError::new(
                    ErrorCode::UnsafePath,
                    "目标目录身份不安全。",
                ));
            }
        } else {
            faults.checkpoint(FaultPoint::Win32BeforeCall)?;
            fs::create_dir(&current).map_err(|error| {
                DomainError::new(ErrorCode::Internal, "无法创建 Fuji 图库目标目录。")
                    .with_diagnostic(error.to_string())
            })?;
            faults.checkpoint(FaultPoint::Win32AfterCall)?;
        }
    }
    Ok(())
}
fn partial_name(final_relative: &str, job: &[u8], ordinal: i64, nonce: Uuid) -> String {
    let path = Path::new(final_relative);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("photo");
    let job_hex = hex::encode(job);
    crate::import_preflight::display_relative(&parent.join(format!(
        ".{name}.fpm-{}-{ordinal}-{nonce}.partial",
        &job_hex[..8.min(job_hex.len())]
    )))
}
fn suffix_ordinal(relative: &str) -> Option<u32> {
    let stem = Path::new(relative).file_stem()?.to_str()?;
    let Some((_, suffix)) = stem.rsplit_once('_') else {
        return Some(1);
    };
    suffix.parse().ok().filter(|value| *value >= 2).or(Some(1))
}
fn source_mtime(transaction: &Transaction<'_>, snapshot: &[u8]) -> Result<i64, DomainError> {
    transaction
        .query_row(
            "SELECT mtime_filetime FROM file_identity_snapshot WHERE snapshot_id=?1",
            [snapshot],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)
}
fn hash_read_handle(file: &mut fs::File) -> Result<[u8; 32], DomainError> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        DomainError::new(ErrorCode::MediaUnreadable, "无法重读恢复候选文件。")
            .with_diagnostic(error.to_string())
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            DomainError::new(ErrorCode::MediaUnreadable, "无法完整读取恢复候选文件。")
                .with_diagnostic(error.to_string())
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}
fn role_sql(kind: DraftMediaKind) -> &'static str {
    match kind {
        DraftMediaKind::Jpg => "Jpg",
        DraftMediaKind::Raf => "Raf",
    }
}
fn id() -> Vec<u8> {
    Uuid::new_v4().as_bytes().to_vec()
}
fn uuid_string(value: &[u8]) -> Result<String, DomainError> {
    Uuid::from_slice(value)
        .map(|id| id.to_string())
        .map_err(|error| {
            DomainError::new(ErrorCode::InvalidId, "数据库中的任务标识无效。")
                .with_diagnostic(error.to_string())
        })
}
fn library_id(value: &[u8]) -> Result<LibraryId, DomainError> {
    Ok(LibraryId::from_uuid(Uuid::from_slice(value).map_err(
        |_| DomainError::new(ErrorCode::InvalidId, "Library ID 无效。"),
    )?))
}
fn root_id(value: &[u8]) -> Result<RootId, DomainError> {
    Ok(RootId::from_uuid(Uuid::from_slice(value).map_err(
        |_| DomainError::new(ErrorCode::InvalidId, "Root ID 无效。"),
    )?))
}
fn binding_id(value: &[u8]) -> Result<RootBindingId, DomainError> {
    Ok(RootBindingId::from_uuid(Uuid::from_slice(value).map_err(
        |_| DomainError::new(ErrorCode::InvalidId, "Binding ID 无效。"),
    )?))
}
fn lock_error<T>(_error: std::sync::PoisonError<T>) -> DomainError {
    DomainError::new(ErrorCode::Internal, "导入执行状态不可用。")
}
fn phase_from_job_state(state: &str) -> ImportExecutionPhase {
    match state {
        "ValidatingPlan" => ImportExecutionPhase::ValidatingPlan,
        "AwaitingPlanConfirmation" => ImportExecutionPhase::AwaitingPlanConfirmation,
        "AwaitingSpace" => ImportExecutionPhase::AwaitingSpace,
        "Running" | "CancelRequested" => ImportExecutionPhase::Running,
        "AwaitingSource" => ImportExecutionPhase::AwaitingSource,
        "NeedsAttention" => ImportExecutionPhase::NeedsAttention,
        "Completed" => ImportExecutionPhase::Completed,
        "CompletedWithErrors" => ImportExecutionPhase::CompletedWithErrors,
        "Cancelled" => ImportExecutionPhase::Cancelled,
        _ => ImportExecutionPhase::Idle,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportExecutionStatus {
    pub phase: ImportExecutionPhase,
    pub job_id: Option<String>,
    pub attempt_ordinal: Option<u64>,
    pub completed_files: u64,
    pub published_files: u64,
    pub skipped_files: u64,
    pub failed_files: u64,
    pub not_started_files: u64,
    pub current_file: Option<String>,
    pub current_action: Option<String>,
    pub message: Option<String>,
    pub updated_plan: Option<ImportPreflightDraft>,
    pub space: Option<ImportSpaceView>,
    pub source_write_requests: u64,
}

impl Default for ImportExecutionStatus {
    fn default() -> Self {
        Self {
            phase: ImportExecutionPhase::Idle,
            job_id: None,
            attempt_ordinal: None,
            completed_files: 0,
            published_files: 0,
            skipped_files: 0,
            failed_files: 0,
            not_started_files: 0,
            current_file: None,
            current_action: None,
            message: None,
            updated_plan: None,
            space: None,
            source_write_requests: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSpaceView {
    pub planned_new_bytes: u64,
    pub available_bytes: u64,
    pub reserve_bytes: u64,
    pub missing_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecoverySummary {
    pub has_unfinished_import: bool,
    pub job_id: Option<String>,
    pub phase: ImportExecutionPhase,
    pub completed_files: u64,
    pub published_files: u64,
    pub unresolved_files: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportExecutionService {
    database: Arc<Database>,
    safety_gate: Arc<RuntimeSafetyGate>,
    preflight: Arc<ImportPreflightService>,
    catalog: Arc<CatalogService>,
    status: Arc<Mutex<ImportExecutionStatus>>,
    latest_seed: Arc<Mutex<Option<ExecutionDraftSeed>>>,
    cancel_requested: Arc<AtomicBool>,
    faults: FaultInjector,
}

#[derive(Debug, Clone)]
struct JobContext {
    job_id: Vec<u8>,
    generation_id: Vec<u8>,
    validation_id: Vec<u8>,
    target_root_id: Vec<u8>,
    target_binding_id: Vec<u8>,
    target_volume_id: Vec<u8>,
    target_path: PathBuf,
    library_id: Vec<u8>,
}

#[derive(Debug, Clone)]
struct AttemptContext {
    job: JobContext,
    attempt_id: Vec<u8>,
    lease_generation: i64,
    report_id: Vec<u8>,
    steps: Vec<AttemptStep>,
}

#[derive(Debug, Clone)]
struct AttemptStep {
    attempt_step_id: Vec<u8>,
    step_id: Vec<u8>,
    source_item_id: Vec<u8>,
    source_relative_path: String,
    source_path_key: Vec<u8>,
    source_file_id: Option<Vec<u8>>,
    source_snapshot_id: Vec<u8>,
    source_root_id: Vec<u8>,
    source_binding_id: Vec<u8>,
    source_volume_id: Vec<u8>,
    role: DraftMediaKind,
    disposition: String,
    final_relative_path: Option<String>,
    final_path_key: Option<Vec<u8>>,
    target_file_id: Option<Vec<u8>>,
    expected_bytes: u64,
    source_reservation_id: Option<Vec<u8>>,
    target_reservation_id: Option<Vec<u8>>,
    partial_reservation_id: Option<Vec<u8>>,
    ownership_id: Option<Vec<u8>>,
    partial_relative_path: Option<String>,
}

#[derive(Debug)]
struct PendingPublishRecovery {
    intent_id: Vec<u8>,
    attempt_step_id: Vec<u8>,
    step_id: Vec<u8>,
    source_item_id: Vec<u8>,
    ownership_id: Vec<u8>,
    claim_id: Vec<u8>,
    partial_relative_path: String,
    final_relative_path: String,
    final_path_key: Vec<u8>,
    verified_snapshot_id: Vec<u8>,
    verified_file_id: Vec<u8>,
    verified_size: u64,
    verified_mtime: i64,
    verified_hash: [u8; 32],
    intent_state: String,
    target_root_id: Vec<u8>,
    target_binding_id: Vec<u8>,
    target_volume_id: Vec<u8>,
}

#[derive(Debug)]
struct PendingCleanupRecovery {
    ownership_id: Vec<u8>,
    claim_id: Vec<u8>,
    attempt_step_id: Vec<u8>,
    relative_path: String,
    file_id: Vec<u8>,
    deleted_snapshot_id: Vec<u8>,
}

struct BoundTargetRecoveryRoot {
    display_path: String,
    path_key: Vec<u8>,
    directory_file_id: Option<Vec<u8>>,
    filesystem: String,
    volume_key: Option<Vec<u8>>,
}

impl ImportExecutionService {
    pub fn new(
        database: Arc<Database>,
        preflight: Arc<ImportPreflightService>,
        catalog: Arc<CatalogService>,
        safety_gate: Arc<RuntimeSafetyGate>,
    ) -> Self {
        Self::with_faults(
            database,
            preflight,
            catalog,
            safety_gate,
            FaultInjector::default(),
        )
    }

    pub fn with_faults(
        database: Arc<Database>,
        preflight: Arc<ImportPreflightService>,
        catalog: Arc<CatalogService>,
        safety_gate: Arc<RuntimeSafetyGate>,
        faults: FaultInjector,
    ) -> Self {
        Self {
            database,
            safety_gate,
            preflight,
            catalog,
            status: Arc::new(Mutex::new(ImportExecutionStatus::default())),
            latest_seed: Arc::new(Mutex::new(None)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            faults,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_permit(&self) -> SafetyPermit {
        self.safety_gate
            .require_feature(FeatureGate::Import)
            .expect("healthy test import gate")
    }

    pub fn status(&self) -> Result<ImportExecutionStatus, DomainError> {
        Ok(self.lock_status()?.clone())
    }

    pub fn request_cancel(&self) -> Result<(), DomainError> {
        self.cancel_requested.store(true, Ordering::Release);
        let job_id = self.active_job_id()?;
        if let Some(job_id) = job_id {
            self.database.writer()?.transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE import_job
                         SET job_state='CancelRequested', row_version=row_version+1
                         WHERE job_id=?1 AND job_state='Running'",
                        [&job_id],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn start(
        &self,
        permit: SafetyPermit,
        draft_digest: &str,
    ) -> Result<ImportExecutionStatus, DomainError> {
        self.safety_gate.revalidate(&permit, FeatureGate::Import)?;
        self.cancel_requested.store(false, Ordering::Release);
        let existing = self.active_job_id()?;
        let (job_id, seed) = match existing {
            None => {
                let seed = self.preflight.confirmed_seed(draft_digest)?;
                self.require_safe_target(&seed)?;
                self.safety_gate.revalidate(&permit, FeatureGate::Import)?;
                let job_id = self.create_initial_intent(&seed)?;
                (job_id, seed)
            }
            Some(job_id) => {
                let seed = self
                    .latest_seed
                    .lock()
                    .map_err(lock_error)?
                    .clone()
                    .ok_or_else(|| {
                        DomainError::new(
                            ErrorCode::StaleVersion,
                            "计划已更新，请重新打开导入页确认。",
                        )
                    })?;
                if seed.draft.draft_digest != draft_digest {
                    return Err(DomainError::new(
                        ErrorCode::StaleVersion,
                        "更新后的计划尚未由用户确认。",
                    ));
                }
                self.confirm_proposed_if_needed(&job_id)?;
                (job_id, seed)
            }
        };
        *self.latest_seed.lock().map_err(lock_error)? = Some(seed);
        self.set_phase(
            ImportExecutionPhase::ValidatingPlan,
            Some(&job_id),
            Some("正在执行全量只读验证。"),
        )?;
        let refreshed = match self.preflight.refresh_active_import(&job_id) {
            Ok(seed) => seed,
            Err(error) if error.code == ErrorCode::RootOffline => {
                self.record_validation_wait(&job_id, "AwaitingSource")?;
                self.set_phase(
                    ImportExecutionPhase::AwaitingSource,
                    Some(&job_id),
                    Some("等待原始内存卡；不会自动继续。"),
                )?;
                return self.status();
            }
            Err(error) => return Err(error),
        };
        self.require_safe_target(&refreshed)?;
        *self.latest_seed.lock().map_err(lock_error)? = Some(refreshed.clone());
        let validation = self.validate_generation(&job_id, &refreshed)?;
        let Some(job) = validation else {
            return self.status();
        };
        let Some(space_check_id) = self.check_space(&job)? else {
            return self.status();
        };
        self.safety_gate.revalidate(&permit, FeatureGate::Import)?;
        let attempt = self.create_attempt(&job, &space_check_id, "Initial")?;
        self.execute_attempt(attempt, &refreshed)?;
        self.status()
    }

    pub fn retry(&self, permit: SafetyPermit) -> Result<ImportExecutionStatus, DomainError> {
        self.safety_gate.revalidate(&permit, FeatureGate::Import)?;
        let job_id = self.active_job_id()?.ok_or_else(|| {
            DomainError::new(ErrorCode::InvalidTransition, "没有可重试的导入任务。")
        })?;
        self.cancel_requested.store(false, Ordering::Release);
        self.close_interrupted_attempt(&job_id)?;
        let refreshed = self.preflight.refresh_active_import(&job_id)?;
        self.require_safe_target(&refreshed)?;
        *self.latest_seed.lock().map_err(lock_error)? = Some(refreshed.clone());
        self.set_phase(
            ImportExecutionPhase::ValidatingPlan,
            Some(&job_id),
            Some("正在重新验证未完成项。"),
        )?;
        let Some(job) = self.validate_generation(&job_id, &refreshed)? else {
            return self.status();
        };
        let Some(space_check_id) = self.check_space(&job)? else {
            return self.status();
        };
        let reconcile: bool = self.database.open_read_connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM import_plan_validation_item WHERE validation_id=?1 AND disposition IN ('ReverifyOwnedPartial','IndexPublished'))",
            [&job.validation_id],|row|row.get(0),
        ).map_err(map_sqlite_error)?;
        self.safety_gate.revalidate(&permit, FeatureGate::Import)?;
        let attempt = self.create_attempt(
            &job,
            &space_check_id,
            if reconcile { "Reconcile" } else { "Retry" },
        )?;
        self.execute_attempt(attempt, &refreshed)?;
        self.status()
    }

    pub fn cancel_before_start(&self) -> Result<ImportExecutionStatus, DomainError> {
        let job_id = self.active_job_id()?.ok_or_else(|| {
            DomainError::new(ErrorCode::InvalidTransition, "没有可取消的导入任务。")
        })?;
        let job_id_for_tx = job_id.clone();
        self.database.writer()?.transaction(move |transaction| {
            let state: String = transaction
                .query_row(
                    "SELECT job_state FROM import_job WHERE job_id=?1",
                    [&job_id_for_tx],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if state == "Running" || state == "CancelRequested" {
                return Err(DomainError::new(
                    ErrorCode::InvalidTransition,
                    "导入已开始，请使用“取消剩余导入”。",
                ));
            }
            let ordinal: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(attempt_ordinal),0)+1 FROM task_attempt WHERE task_id=?1",
                    [&job_id_for_tx],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            let attempt_id = id();
            let operation_id = id();
            let report_id = id();
            let now = now_micros();
            insert_operation(transaction, &operation_id, "ImportCancelBeforeStart", &job_id_for_tx)?;
            transaction.execute(
                "INSERT INTO task_attempt(attempt_id,task_id,attempt_kind,attempt_ordinal,operation_id,created_at)
                 VALUES(?1,?2,'Import',?3,?4,?5)",
                params![&attempt_id,&job_id_for_tx,ordinal,&operation_id,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO import_attempt(attempt_id,job_id,attempt_reason,attempt_state,finished_at,row_version)
                 VALUES(?1,?2,'CancelBeforeStart','Cancelled',?3,1)",
                params![&attempt_id,&job_id_for_tx,now],
            ).map_err(map_sqlite_error)?;
            let (library_id,target_root_id):(Vec<u8>,Vec<u8>)=transaction.query_row(
                "SELECT library_id,target_root_id FROM import_job WHERE job_id=?1",[&job_id_for_tx],
                |row|Ok((row.get(0)?,row.get(1)?)),
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO operation_report(report_id,report_kind,library_id,root_id,task_attempt_id,report_state,started_at,completed_at,result,item_count,total_bytes)
                 VALUES(?1,'Import',?2,?3,?4,'Finalized',?5,?5,'Cancelled',0,0)",
                params![&report_id,&library_id,&target_root_id,&attempt_id,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE import_job SET job_state='Cancelled',completed_at=?2,row_version=row_version+1 WHERE job_id=?1",
                params![&job_id_for_tx,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE task_intent SET lifecycle='Cancelled',resolved_at=?2,row_version=row_version+1 WHERE task_id=?1",
                params![&job_id_for_tx,now],
            ).map_err(map_sqlite_error)?;
            Ok(())
        })?;
        self.set_phase(
            ImportExecutionPhase::Cancelled,
            Some(&job_id),
            Some("导入在任何文件动作前取消。"),
        )?;
        self.status()
    }

    pub fn recovery_summary(&self) -> Result<ImportRecoverySummary, DomainError> {
        let Some(job_id) = self.active_job_id()? else {
            return Ok(ImportRecoverySummary {
                has_unfinished_import: false,
                job_id: None,
                phase: ImportExecutionPhase::Idle,
                completed_files: 0,
                published_files: 0,
                unresolved_files: 0,
                message: None,
            });
        };
        let connection = self.database.open_read_connection()?;
        let (state, published, indexed, total): (String, i64, i64, i64) = connection
            .query_row(
                "SELECT job_state,
                        (SELECT COUNT(*) FROM import_published_fact AS fact
                         JOIN import_asset_step AS step ON step.step_id=fact.step_id
                         JOIN import_plan_generation AS generation ON generation.generation_id=step.generation_id
                         WHERE generation.job_id=import_job.job_id),
                        (SELECT COUNT(*) FROM import_indexed_fact AS indexed
                         JOIN import_published_fact AS fact ON fact.published_fact_id=indexed.published_fact_id
                         JOIN import_asset_step AS step ON step.step_id=fact.step_id
                         JOIN import_plan_generation AS generation ON generation.generation_id=step.generation_id
                         WHERE generation.job_id=import_job.job_id),
                        (SELECT planned_source_files FROM import_plan_generation
                         WHERE generation_id=import_job.current_confirmed_generation_id)
                 FROM import_job WHERE job_id=?1",
                [&job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(map_sqlite_error)?;
        let phase = phase_from_job_state(&state);
        Ok(ImportRecoverySummary {
            has_unfinished_import: true,
            job_id: Some(uuid_string(&job_id)?),
            phase,
            completed_files: u64::try_from(indexed).unwrap_or(0),
            published_files: u64::try_from(published).unwrap_or(0),
            unresolved_files: u64::try_from(total.saturating_sub(indexed)).unwrap_or(0),
            message: Some("检测到未终结导入；写操作不会自动恢复。".into()),
        })
    }

    pub fn reconcile_startup(&self) -> Result<(), DomainError> {
        if let Some(job_id) = self.active_job_id()? {
            self.close_interrupted_attempt(&job_id)?;
        }
        Ok(())
    }

    fn require_safe_target(&self, seed: &ExecutionDraftSeed) -> Result<(), DomainError> {
        let result = (|| {
            let current = probe_directory(&seed.target.root_path)?;
            if current != seed.target.root_probe
                || current.filesystem_kind != FilesystemKind::Ntfs
                || crate::roots::path_is_reparse(&seed.target.root_path)?
            {
                return Err(DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "Fuji 图库 Root 身份或 NTFS 能力已变化。",
                ));
            }
            if seed.source.root_probe.volume_identity_key
                == seed.target.root_probe.volume_identity_key
            {
                return Err(DomainError::new(
                    ErrorCode::RootConflict,
                    "来源卷与 Fuji 图库必须是不同卷。",
                ));
            }
            Ok(())
        })();
        if result.as_ref().is_err_and(|error| {
            matches!(
                error.code,
                ErrorCode::IdentityMismatch | ErrorCode::RootOffline | ErrorCode::UnsafePath
            )
        }) {
            self.safety_gate.latch_fault(GateCloseReason::RootIdentity);
        }
        result
    }

    fn create_initial_intent(&self, seed: &ExecutionDraftSeed) -> Result<Vec<u8>, DomainError> {
        let seed = seed.clone();
        let job_id = id();
        let job_for_tx = job_id.clone();
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            let target_volume_id: Vec<u8> = transaction.query_row(
                "SELECT volume_id FROM root_binding WHERE binding_id=?1 AND root_id=?2 AND binding_state='Active'",
                params![&seed.target.binding_id,&seed.target.root_id], |row|row.get(0),
            ).map_err(map_sqlite_error)?;
            let profile_valid: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM import_source_profile
                 WHERE profile_id=?1 AND library_id=?2 AND source_root_id=?3 AND volume_id=?4 AND retired_at IS NULL)",
                params![&seed.profile_id,&seed.source.library_id,&seed.source.root_id,&seed.source.volume_id],
                |row|row.get(0),
            ).map_err(map_sqlite_error)?;
            if !profile_valid {
                return Err(DomainError::new(ErrorCode::IdentityMismatch,"来源 Profile 已变化。"));
            }
            let active: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_intent WHERE lifecycle NOT IN ('Completed','Cancelled'))",(),|row|row.get(0),
            ).map_err(map_sqlite_error)?;
            if active {
                return Err(DomainError::new(ErrorCode::LeaseUnavailable,"已有未完成文件任务。"));
            }
            transaction.execute(
                "INSERT INTO task_intent(task_id,task_kind,lifecycle,row_version,created_at)
                 VALUES(?1,'Import','Prepared',1,?2)",params![&job_for_tx,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO import_job(job_id,library_id,source_root_id,target_root_id,target_binding_id,job_state,row_version,created_at,source_profile_id,source_volume_id)
                 VALUES(?1,?2,?3,?4,?5,'Prepared',1,?6,?7,?8)",
                params![&job_for_tx,&seed.target.library_id,&seed.source.root_id,&seed.target.root_id,&seed.target.binding_id,now,&seed.profile_id,&seed.source.volume_id],
            ).map_err(map_sqlite_error)?;
            let source_items = insert_source_items(transaction,&job_for_tx,&seed,now)?;
            let generation_id = insert_generation(
                transaction,&job_for_tx,1,"Confirmed","InitialPreflight",None,&seed,&source_items,now,
            )?;
            transaction.execute(
                "UPDATE import_job SET current_confirmed_generation_id=?2 WHERE job_id=?1",
                params![&job_for_tx,&generation_id],
            ).map_err(map_sqlite_error)?;
            let _ = target_volume_id;
            Ok(())
        })?;
        Ok(job_id)
    }

    fn confirm_proposed_if_needed(&self, job_id: &[u8]) -> Result<(), DomainError> {
        let job_id = job_id.to_vec();
        self.database.writer()?.transaction(move |transaction| {
            let proposed: Option<Vec<u8>> = transaction.query_row(
                "SELECT proposed_generation_id FROM import_job WHERE job_id=?1",[&job_id],|row|row.get(0),
            ).optional().map_err(map_sqlite_error)?.flatten();
            let Some(proposed) = proposed else { return Ok(()); };
            let old: Vec<u8> = transaction.query_row(
                "SELECT current_confirmed_generation_id FROM import_job WHERE job_id=?1",[&job_id],|row|row.get(0),
            ).map_err(map_sqlite_error)?;
            let now=now_micros();
            transaction.execute(
                "UPDATE import_plan_generation SET generation_state='Superseded' WHERE generation_id=?1 AND generation_state='Confirmed'",[&old],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE import_plan_generation SET generation_state='Confirmed',confirmed_at=?2 WHERE generation_id=?1 AND generation_state='Proposed'",params![&proposed,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE import_job SET current_confirmed_generation_id=?2,proposed_generation_id=NULL,job_state='ValidatingPlan',row_version=row_version+1 WHERE job_id=?1",
                params![&job_id,&proposed],
            ).map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn validate_generation(
        &self,
        job_id: &[u8],
        refreshed: &ExecutionDraftSeed,
    ) -> Result<Option<JobContext>, DomainError> {
        let connection = self.database.open_read_connection()?;
        let (generation_id, generation_no): (Vec<u8>, i64) = connection.query_row(
            "SELECT generation.generation_id,generation.generation_no
             FROM import_job AS job
             JOIN import_plan_generation AS generation ON generation.generation_id=job.current_confirmed_generation_id
             JOIN generation_plan_digest ON generation_plan_digest.generation_id=generation.generation_id
             WHERE job.job_id=?1 AND generation.generation_state='Confirmed'",
            [job_id], |row|Ok((row.get(0)?,row.get(1)?)),
        ).map_err(map_sqlite_error)?;
        drop(connection);
        if !self.source_identity_matches(job_id, refreshed)? {
            self.persist_simple_validation(job_id, &generation_id, "NeedsAttention")?;
            self.set_phase(
                ImportExecutionPhase::NeedsAttention,
                Some(job_id),
                Some("来源文件身份或内容已变化；不会重绑或猜测。"),
            )?;
            return Ok(None);
        }
        if self.has_unreconcilable_ownership(&generation_id)? {
            self.persist_simple_validation(job_id, &generation_id, "NeedsAttention")?;
            self.set_phase(
                ImportExecutionPhase::NeedsAttention,
                Some(job_id),
                Some("发现 Planned、CleaningOwnedPartial 或 NeedsAttention partial；不会按路径认领、覆盖或删除。"),
            )?;
            return Ok(None);
        }
        if self.unresolved_plan_changed(&generation_id, refreshed)? {
            self.create_proposed_generation(job_id, &generation_id, generation_no + 1, refreshed)?;
            self.set_phase(
                ImportExecutionPhase::AwaitingPlanConfirmation,
                Some(job_id),
                Some("此计划已更新，请重新确认。"),
            )?;
            self.lock_status()?.updated_plan = Some(refreshed.draft.clone());
            return Ok(None);
        }
        self.persist_ready_validation(job_id, &generation_id, refreshed)
    }

    fn source_identity_matches(
        &self,
        job_id: &[u8],
        seed: &ExecutionDraftSeed,
    ) -> Result<bool, DomainError> {
        let current = seed
            .enumerated
            .files
            .iter()
            .map(|file| {
                (
                    crate::import_preflight::display_relative(&file.relative_path).to_lowercase(),
                    file,
                )
            })
            .collect::<HashMap<_, _>>();
        let connection = self.database.open_read_connection()?;
        let mut statement=connection.prepare(
            "SELECT item.source_relative_path,snapshot.file_id,snapshot.size_bytes,snapshot.mtime_filetime,snapshot.content_hash
             FROM import_source_item AS item JOIN file_identity_snapshot AS snapshot ON snapshot.snapshot_id=item.source_snapshot_id
             WHERE item.job_id=?1 AND item.parse_result='Readable'"
        ).map_err(map_sqlite_error)?;
        let rows = statement
            .query_map([job_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            })
            .map_err(map_sqlite_error)?;
        for row in rows {
            let (path, file_id, size, mtime, hash) = row.map_err(map_sqlite_error)?;
            let Some(file) = current.get(&path.to_lowercase()) else {
                return Ok(false);
            };
            if file.identity.file_id != file_id
                || i64::try_from(file.size_bytes).unwrap_or(i64::MAX) != size
                || file.identity.mtime_filetime != mtime
                || hash.as_deref() != Some(file.full_hash.as_slice())
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn has_unreconcilable_ownership(&self, generation_id: &[u8]) -> Result<bool, DomainError> {
        self.database
            .open_read_connection()?
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM import_partial_ownership AS ownership
                    JOIN import_asset_step AS step ON step.step_id=ownership.step_id
                    WHERE step.generation_id=?1
                      AND ownership.state IN ('Planned','CleaningOwnedPartial','NeedsAttention')
                )",
                [generation_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)
    }

    fn unresolved_plan_changed(
        &self,
        generation_id: &[u8],
        seed: &ExecutionDraftSeed,
    ) -> Result<bool, DomainError> {
        let refreshed = seed
            .draft
            .items
            .iter()
            .map(|item| (item.source_relative_path.to_lowercase(), item))
            .collect::<HashMap<_, _>>();
        let connection = self.database.open_read_connection()?;
        let mut statement = connection.prepare("SELECT item.source_relative_path,step.plan_kind,step.final_relative_path,step.expected_bytes FROM import_asset_step AS step JOIN import_source_item AS item ON item.source_item_id=step.source_item_id LEFT JOIN import_published_fact AS published ON published.source_item_id=step.source_item_id WHERE step.generation_id=?1 AND published.published_fact_id IS NULL").map_err(map_sqlite_error)?;
        let rows = statement
            .query_map([generation_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(map_sqlite_error)?;
        for row in rows {
            let (path, kind, target, bytes) = row.map_err(map_sqlite_error)?;
            let Some(item) = refreshed.get(&path.to_lowercase()) else {
                return Ok(true);
            };
            let refreshed_kind = match item.disposition {
                DraftDisposition::Skip => "SkipDuplicate",
                DraftDisposition::Unreadable => "Unreadable",
                DraftDisposition::New | DraftDisposition::Conflict => "Copy",
            };
            let refreshed_bytes = matches!(
                item.disposition,
                DraftDisposition::New | DraftDisposition::Conflict
            )
            .then_some(i64::try_from(item.size_bytes).unwrap_or(i64::MAX));
            if kind != refreshed_kind
                || target != item.target_relative_path
                || bytes != refreshed_bytes
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn create_proposed_generation(
        &self,
        job_id: &[u8],
        old_generation: &[u8],
        generation_no: i64,
        seed: &ExecutionDraftSeed,
    ) -> Result<(), DomainError> {
        let job_id = job_id.to_vec();
        let old = old_generation.to_vec();
        let seed = seed.clone();
        self.database.writer()?.transaction(move|transaction|{
            if let Some(existing)=transaction.query_row("SELECT proposed_generation_id FROM import_job WHERE job_id=?1",[&job_id],|row|row.get::<_,Option<Vec<u8>>>(0)).map_err(map_sqlite_error)?{
                transaction.execute("UPDATE import_plan_generation SET generation_state='Rejected' WHERE generation_id=?1 AND generation_state='Proposed'",[&existing]).map_err(map_sqlite_error)?;
            }
            let source_items=load_source_item_map(transaction,&job_id)?;
            let generation=insert_generation(transaction,&job_id,generation_no,"Proposed","RetryRefresh",Some(&old),&seed,&source_items,now_micros())?;
            transaction.execute("UPDATE import_job SET proposed_generation_id=?2,job_state='AwaitingPlanConfirmation',row_version=row_version+1 WHERE job_id=?1",params![&job_id,&generation]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE task_intent SET lifecycle='AwaitingUser',row_version=row_version+1 WHERE task_id=?1 AND lifecycle IN ('Prepared','Active','NeedsAttention','AwaitingSource')",[&job_id]).map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn persist_simple_validation(
        &self,
        job_id: &[u8],
        generation_id: &[u8],
        outcome: &str,
    ) -> Result<Vec<u8>, DomainError> {
        let job_id = job_id.to_vec();
        let generation_id = generation_id.to_vec();
        let outcome = outcome.to_string();
        let validation = id();
        let returned = validation.clone();
        self.database.writer()?.transaction(move|transaction|{
            let ordinal:i64=transaction.query_row("SELECT COALESCE(MAX(validation_ordinal),0)+1 FROM import_plan_validation WHERE job_id=?1",[&job_id],|row|row.get(0)).map_err(map_sqlite_error)?;
            let now=now_micros();transaction.execute("INSERT INTO import_plan_validation(validation_id,job_id,generation_id,validation_ordinal,outcome,started_at,completed_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",params![&validation,&job_id,&generation_id,ordinal,&outcome,now]).map_err(map_sqlite_error)?;
            let state=if outcome=="AwaitingSource"{"AwaitingSource"}else{"NeedsAttention"};
            transaction.execute("UPDATE import_job SET job_state=?2,row_version=row_version+1 WHERE job_id=?1 AND job_state<>?2",params![&job_id,state]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE task_intent SET lifecycle=?2,row_version=row_version+1 WHERE task_id=?1 AND lifecycle<>?2",params![&job_id,state]).map_err(map_sqlite_error)?;Ok(())
        })?;
        Ok(returned)
    }

    fn record_validation_wait(&self, job_id: &[u8], outcome: &str) -> Result<(), DomainError> {
        let generation: Vec<u8> = self
            .database
            .open_read_connection()?
            .query_row(
                "SELECT current_confirmed_generation_id FROM import_job WHERE job_id=?1",
                [job_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        self.persist_simple_validation(job_id, &generation, outcome)
            .map(|_| ())
    }

    fn persist_ready_validation(
        &self,
        job_id: &[u8],
        generation_id: &[u8],
        seed: &ExecutionDraftSeed,
    ) -> Result<Option<JobContext>, DomainError> {
        let validation = id();
        let job_id = job_id.to_vec();
        let generation_id = generation_id.to_vec();
        let seed = seed.clone();
        let validation_return = validation.clone();
        let context = self.load_job_context(
            &job_id,
            &generation_id,
            &validation_return,
            &seed.target.root_path,
        )?;
        let audit = ProcessIoAudit::default();
        let step_rows = self.load_generation_steps_for_validation(&generation_id)?;
        let source_by_path = seed
            .enumerated
            .files
            .iter()
            .map(|file| {
                (
                    crate::import_preflight::display_relative(&file.relative_path).to_lowercase(),
                    file.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut proofs = Vec::new();
        for step in &step_rows {
            if step.disposition != "SkipWithCurrentProof" {
                continue;
            }
            let source = source_by_path
                .get(&step.source_relative_path.to_lowercase())
                .ok_or_else(|| {
                    DomainError::new(ErrorCode::IdentityMismatch, "跳过项来源已缺失。")
                })?;
            let target_relative = step
                .final_relative_path
                .as_ref()
                .ok_or_else(|| DomainError::new(ErrorCode::Internal, "跳过项缺少目标路径。"))?;
            let source_bound = bound_source(&seed, source);
            let target_bound = bound_target(&seed, target_relative);
            let proof =
                prove_duplicate_guard(&source_bound, &target_bound, &audit)?.ok_or_else(|| {
                    DomainError::new(ErrorCode::StaleVersion, "跳过证明已失效，需要更新计划。")
                })?;
            proofs.push((step.clone(), proof));
        }
        self.database.writer()?.transaction(move|transaction|{
            let ordinal:i64=transaction.query_row("SELECT COALESCE(MAX(validation_ordinal),0)+1 FROM import_plan_validation WHERE job_id=?1",[&job_id],|row|row.get(0)).map_err(map_sqlite_error)?;
            let now=now_micros();transaction.execute("INSERT INTO import_plan_validation(validation_id,job_id,generation_id,validation_ordinal,outcome,started_at,completed_at) VALUES(?1,?2,?3,?4,'Ready',?5,?5)",params![&validation,&job_id,&generation_id,ordinal,now]).map_err(map_sqlite_error)?;
            for step in &step_rows{let remaining=if step.disposition=="CopyNewBytes"{i64::try_from(step.expected_bytes).unwrap_or(i64::MAX)}else{0};transaction.execute("INSERT INTO import_plan_validation_item(validation_item_id,validation_id,step_id,disposition,remaining_new_bytes) VALUES(?1,?2,?3,?4,?5)",params![id(),&validation,&step.step_id,&step.disposition,remaining]).map_err(map_sqlite_error)?;}
            for (step,proof) in &proofs{insert_duplicate_proof(transaction,"Validation",&step.step_id,Some(&validation),None,None,None,None,&proof.proof,&seed,None)?;}
            transaction.execute("UPDATE import_job SET job_state='ReadyToStart',row_version=row_version+1 WHERE job_id=?1",[&job_id]).map_err(map_sqlite_error)?;
            Ok(())
        })?;
        Ok(Some(context))
    }

    fn check_space(&self, job: &JobContext) -> Result<Option<Vec<u8>>, DomainError> {
        let planned:i64=self.database.open_read_connection()?.query_row("SELECT COALESCE(SUM(remaining_new_bytes),0) FROM import_plan_validation_item WHERE validation_id=?1",[&job.validation_id],|row|row.get(0)).map_err(map_sqlite_error)?;
        let (total, available) = disk_space(&job.target_path)?;
        let reserve =
            IMPORT_MIN_RESERVE_BYTES.max(total.saturating_mul(2).saturating_add(99) / 100);
        let planned_u = u64::try_from(planned).unwrap_or(u64::MAX);
        let passed = available >= planned_u.saturating_add(reserve);
        let check = id();
        let returned = check.clone();
        let job_clone = job.clone();
        self.database.writer()?.transaction(move|transaction|{transaction.execute("INSERT INTO import_space_check(space_check_id,job_id,generation_id,validation_id,planned_new_bytes,target_total_bytes,target_available_bytes,reserve_bytes,checked_at,outcome) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![&check,&job_clone.job_id,&job_clone.generation_id,&job_clone.validation_id,planned,i64::try_from(total).unwrap_or(i64::MAX),i64::try_from(available).unwrap_or(i64::MAX),i64::try_from(reserve).unwrap_or(i64::MAX),now_micros(),if passed{"Passed"}else{"Insufficient"}]).map_err(map_sqlite_error)?;
            if !passed{transaction.execute("UPDATE import_job SET job_state='AwaitingSpace',row_version=row_version+1 WHERE job_id=?1",[&job_clone.job_id]).map_err(map_sqlite_error)?;transaction.execute("UPDATE task_intent SET lifecycle='AwaitingUser',row_version=row_version+1 WHERE task_id=?1 AND lifecycle IN ('Prepared','Active','NeedsAttention','AwaitingSource')",[&job_clone.job_id]).map_err(map_sqlite_error)?;}Ok(())})?;
        if !passed {
            let missing = planned_u.saturating_add(reserve).saturating_sub(available);
            let mut status = self.lock_status()?;
            status.phase = ImportExecutionPhase::AwaitingSpace;
            status.space = Some(ImportSpaceView {
                planned_new_bytes: planned_u,
                available_bytes: available,
                reserve_bytes: reserve,
                missing_bytes: missing,
            });
            status.message = Some("空间不足，尚未开始导入。".into());
            return Ok(None);
        }
        Ok(Some(returned))
    }

    fn create_attempt(
        &self,
        job: &JobContext,
        space_check_id: &[u8],
        reason: &str,
    ) -> Result<AttemptContext, DomainError> {
        let mut plan_steps = self.load_plan_steps(&job.job_id, &job.generation_id)?;
        for step in &mut plan_steps {
            if step.disposition == "SkipWithCurrentProof" {
                let relative = step
                    .final_relative_path
                    .as_ref()
                    .ok_or_else(|| DomainError::new(ErrorCode::Internal, "跳过项缺少目标路径。"))?;
                let path = resolve_contained_path(&job.target_path, Path::new(relative))?;
                let handle = crate::roots::open_read_only(&path)?;
                step.target_file_id = crate::roots::probe_file(&handle)?.file_id;
            }
        }
        let attempt_id = id();
        let report_id = id();
        let job_return = job.clone();
        let job = job.clone();
        let space = space_check_id.to_vec();
        let reason = reason.to_string();
        let attempt_return = attempt_id.clone();
        let report_return = report_id.clone();
        let (ordinal,lease_generation,steps)=self.database.writer()?.transaction(move|transaction|{
            let (lease_task,old_generation,row_version):(Option<Vec<u8>>,i64,i64)=transaction.query_row("SELECT task_id,lease_generation,row_version FROM file_write_lease WHERE slot=1",(),|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(map_sqlite_error)?;
            if lease_task.as_deref().is_some_and(|task|task!=job.job_id.as_slice()){return Err(DomainError::new(ErrorCode::LeaseUnavailable,"照片文件写入栅栏正由另一任务持有。"));}
            let lease_generation=old_generation.checked_add(1).ok_or_else(||DomainError::new(ErrorCode::Internal,"Lease generation 溢出。"))?;
            let now=now_micros();
            transaction.execute("UPDATE file_write_lease SET task_id=?1,lease_generation=?2,acquired_at=?3,released_at=NULL,row_version=?4 WHERE slot=1",params![&job.job_id,lease_generation,now,row_version+1]).map_err(map_sqlite_error)?;
            let ordinal:i64=transaction.query_row("SELECT COALESCE(MAX(attempt_ordinal),0)+1 FROM task_attempt WHERE task_id=?1",[&job.job_id],|row|row.get(0)).map_err(map_sqlite_error)?;
            let operation=id();insert_operation(transaction,&operation,"ImportAttempt",&job.job_id)?;
            transaction.execute("INSERT INTO task_attempt(attempt_id,task_id,attempt_kind,attempt_ordinal,operation_id,created_at) VALUES(?1,?2,'Import',?3,?4,?5)",params![&attempt_id,&job.job_id,ordinal,&operation,now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO import_attempt(attempt_id,job_id,generation_id,validation_id,space_check_id,attempt_reason,attempt_state,started_at,row_version) VALUES(?1,?2,?3,?4,?5,?6,'Running',?7,1)",params![&attempt_id,&job.job_id,&job.generation_id,&job.validation_id,&space,&reason,now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO operation_report(report_id,report_kind,library_id,root_id,task_attempt_id,report_state,started_at,item_count,total_bytes) VALUES(?1,'Import',?2,?3,?4,'Open',?5,0,0)",params![&report_id,&job.library_id,&job.target_root_id,&attempt_id,now]).map_err(map_sqlite_error)?;
            let mut created=Vec::new();
            for mut step in plan_steps {
                step.attempt_step_id=id();
                transaction.execute("INSERT INTO import_attempt_step(attempt_step_id,attempt_id,step_id,step_state,row_version) VALUES(?1,?2,?3,'Planned',1)",params![&step.attempt_step_id,&attempt_id,&step.step_id]).map_err(map_sqlite_error)?;
                if matches!(step.disposition.as_str(),"AlreadyResolved"|"Unreadable"){created.push(step);continue}
                let source_reservation=id();
                insert_reservation(transaction,&source_reservation,&job.job_id,&attempt_id,lease_generation,"SourceRead",&step.source_root_id,&step.source_binding_id,None,step.source_file_id.as_deref(),Some(&step.source_path_key),now)?;
                step.source_reservation_id=Some(source_reservation);
                if step.disposition=="SkipWithCurrentProof"{
                    let target=id();insert_reservation(transaction,&target,&job.job_id,&attempt_id,lease_generation,"ExistingAsset",&job.target_root_id,&job.target_binding_id,None,step.target_file_id.as_deref(),step.final_path_key.as_deref(),now)?;step.target_reservation_id=Some(target);
                } else if step.disposition=="IndexPublished"{
                    let target=id();insert_reservation(transaction,&target,&job.job_id,&attempt_id,lease_generation,"FinalPath",&job.target_root_id,&job.target_binding_id,None,None,step.final_path_key.as_deref(),now)?;step.target_reservation_id=Some(target);
                } else {
                    let final_res=id();insert_reservation(transaction,&final_res,&job.job_id,&attempt_id,lease_generation,"FinalPath",&job.target_root_id,&job.target_binding_id,None,None,step.final_path_key.as_deref(),now)?;step.target_reservation_id=Some(final_res);
                    if step.disposition=="CopyNewBytes"{
                        let ownership_ordinal:i64=transaction.query_row("SELECT COALESCE(MAX(ownership_ordinal),0)+1 FROM import_partial_ownership WHERE step_id=?1",[&step.step_id],|row|row.get(0)).map_err(map_sqlite_error)?;
                        let nonce=Uuid::new_v4();let final_relative=step.final_relative_path.as_ref().unwrap();let partial=partial_name(final_relative,&job.job_id,ownership_ordinal,nonce);let partial_key=path_key(Path::new(&partial));let partial_res=id();insert_reservation(transaction,&partial_res,&job.job_id,&attempt_id,lease_generation,"PartialPath",&job.target_root_id,&job.target_binding_id,None,None,Some(&partial_key),now)?;
                        let ownership=id();transaction.execute("INSERT INTO import_partial_ownership(ownership_id,step_id,ownership_ordinal,created_by_attempt_step_id,nonce,root_id,root_binding_id,relative_path,path_key,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'Planned')",params![&ownership,&step.step_id,ownership_ordinal,&step.attempt_step_id,nonce.as_bytes().as_slice(),&job.target_root_id,&job.target_binding_id,&partial,&partial_key]).map_err(map_sqlite_error)?;
                        step.partial_reservation_id=Some(partial_res);step.ownership_id=Some(ownership);step.partial_relative_path=Some(partial);
                    } else if step.disposition=="ReverifyOwnedPartial" {
                        let partial=step.partial_relative_path.clone().ok_or_else(||DomainError::new(ErrorCode::Internal,"Owned partial 缺少冻结路径。"))?;
                        let partial_key=path_key(Path::new(&partial));
                        let partial_res=id();
                        insert_reservation(transaction,&partial_res,&job.job_id,&attempt_id,lease_generation,"PartialPath",&job.target_root_id,&job.target_binding_id,None,step.target_file_id.as_deref(),Some(&partial_key),now)?;
                        let claim=id();
                        transaction.execute("INSERT INTO import_partial_claim(claim_id,ownership_id,attempt_step_id,claim_reason,lease_generation,source_read_reservation_id,partial_path_reservation_id,final_path_reservation_id,claimed_at) VALUES(?1,?2,?3,'Takeover',?4,?5,?6,?7,?8)",params![&claim,step.ownership_id.as_ref().unwrap(),&step.attempt_step_id,lease_generation,step.source_reservation_id.as_ref().unwrap(),&partial_res,step.target_reservation_id.as_ref().unwrap(),now]).map_err(map_sqlite_error)?;
                        step.partial_reservation_id=Some(partial_res);
                    }
                }
                created.push(step);
            }
            transaction.execute("UPDATE import_job SET job_state='Running',row_version=row_version+1 WHERE job_id=?1 AND job_state IN ('ReadyToStart','CompletedWithErrors','NeedsAttention','AwaitingSource')",[&job.job_id]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE task_intent SET lifecycle='Active',row_version=row_version+1 WHERE task_id=?1 AND lifecycle IN ('Prepared','AwaitingUser','AwaitingSource','NeedsAttention')",[&job.job_id]).map_err(map_sqlite_error)?;
            Ok((ordinal,lease_generation,created))
        })?;
        {
            let mut status = self.lock_status()?;
            status.phase = ImportExecutionPhase::Running;
            status.job_id = Some(uuid_string(&job_return.job_id)?);
            status.attempt_ordinal = Some(u64::try_from(ordinal).unwrap_or(0));
            status.not_started_files = u64::try_from(steps.len()).unwrap_or(u64::MAX);
            status.updated_plan = None;
            status.space = None;
            status.message = None;
        }
        Ok(AttemptContext {
            job: job_return,
            attempt_id: attempt_return,
            lease_generation,
            report_id: report_return,
            steps,
        })
    }

    fn execute_attempt(
        &self,
        attempt: AttemptContext,
        seed: &ExecutionDraftSeed,
    ) -> Result<(), DomainError> {
        let source_by_path = seed
            .enumerated
            .files
            .iter()
            .map(|file| {
                (
                    crate::import_preflight::display_relative(&file.relative_path).to_lowercase(),
                    file.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut had_error = false;
        let mut cancelled = false;
        for (index, step) in attempt.steps.iter().enumerate() {
            if self.cancel_requested.load(Ordering::Acquire) {
                cancelled = true;
                self.cancel_unstarted(&attempt, index)?;
                break;
            }
            let permit = self.safety_gate.require_feature(FeatureGate::Import)?;
            self.update_progress(step, "正在验证")?;
            let result = match step.disposition.as_str() {
                "Unreadable" => {
                    self.finish_simple_step(&attempt, step, "Unreadable", Some("MediaUnreadable"))
                }
                "AlreadyResolved" => {
                    self.finish_simple_step(&attempt, step, "AlreadyResolved", None)
                }
                "SkipWithCurrentProof" => self.execute_skip(&attempt, step, seed, &source_by_path),
                "CopyNewBytes" => self.execute_copy(&attempt, step, seed, &source_by_path, &permit),
                "IndexPublished" => self.index_existing_published(&attempt, step, seed),
                "ReverifyOwnedPartial" => {
                    self.reconcile_owned(&attempt, step, seed, &source_by_path, &permit)
                }
                _ => Err(DomainError::new(
                    ErrorCode::Internal,
                    "未知导入 Validation disposition。",
                )),
            };
            match result {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::DatabaseBusy
                            | ErrorCode::DatabaseDurability
                            | ErrorCode::DatabaseCorrupt
                            | ErrorCode::CapabilityDenied
                            | ErrorCode::FaultInjected
                    ) =>
                {
                    // A COMMIT or Win32 boundary may already have taken effect. Preserve the
                    // Running Attempt, claims, reservations and intents for explicit recovery.
                    return Err(error);
                }
                Err(error) if self.cancel_requested.load(Ordering::Acquire) => {
                    cancelled = true;
                    self.mark_step_error(&attempt, step, "Cancelled", Some(error.code))?;
                    self.cancel_unstarted(&attempt, index + 1)?;
                    break;
                }
                Err(error) => {
                    had_error = true;
                    self.settle_failed_publish_intent(step)?;
                    let state = if error.code == ErrorCode::RootOffline {
                        "AwaitingSource"
                    } else if error.code == ErrorCode::ReservationConflict {
                        "Conflict"
                    } else if error.code == ErrorCode::StaleVersion {
                        "PlanStale"
                    } else {
                        "NeedsAttention"
                    };
                    self.mark_step_error(&attempt, step, state, Some(error.code))?;
                    break;
                }
            }
        }
        self.finalize_attempt(&attempt, had_error, cancelled)
    }

    fn settle_failed_publish_intent(&self, step: &AttemptStep) -> Result<(), DomainError> {
        let attempt_step = step.attempt_step_id.clone();
        self.database.writer()?.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE import_publish_intent SET intent_state='Publishing'
                 WHERE attempt_step_id=?1 AND intent_state='Prepared'",
                    [&attempt_step],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE import_publish_intent SET intent_state='NeedsAttention',finished_at=?2
                 WHERE attempt_step_id=?1 AND intent_state='Publishing'",
                    params![&attempt_step, now_micros()],
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn execute_skip(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        seed: &ExecutionDraftSeed,
        sources: &HashMap<String, SourceFile>,
    ) -> Result<(), DomainError> {
        self.transition_step(
            &attempt.attempt_id,
            &step.attempt_step_id,
            "Planned",
            "Validating",
        )?;
        let source = sources
            .get(&step.source_relative_path.to_lowercase())
            .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "原始内存卡文件缺失。"))?;
        let target_relative = step.final_relative_path.as_ref().unwrap();
        let audit = ProcessIoAudit::default();
        let source_bound = bound_source(seed, source);
        let target_bound = bound_target(seed, target_relative);
        let source_res = step.source_reservation_id.as_ref().unwrap().clone();
        let target_res = step.target_reservation_id.as_ref().unwrap().clone();
        let attempt = attempt.clone();
        let step = step.clone();
        let seed = seed.clone();
        prove_duplicate_and_commit(&source_bound, &target_bound, &audit, move |proof| {
            attempt_fence(&self.database, &attempt, &step)?;
            self.database.writer()?.transaction({
                let proof = proof.clone();
                move |transaction| {
                    require_fence(transaction, &attempt, &step)?;
                    insert_duplicate_proof(
                        transaction,
                        "Execution",
                        &step.step_id,
                        None,
                        Some(&step.attempt_step_id),
                        Some(attempt.lease_generation),
                        Some(&source_res),
                        Some(&target_res),
                        &proof,
                        &seed,
                        None,
                    )?;
                    finish_step_row(
                        transaction,
                        &step.attempt_step_id,
                        "Validating",
                        "SkippedDuplicate",
                        None,
                        None,
                    )?;
                    append_report_item(
                        transaction,
                        &attempt.report_id,
                        &step,
                        "SkippedDuplicate",
                        None,
                        0,
                    )?;
                    Ok(())
                }
            })
        })?
        .ok_or_else(|| {
            DomainError::new(
                ErrorCode::StaleVersion,
                "执行期重复证明失效，不能沿用跳过结论。",
            )
        })?;
        let mut status = self.lock_status()?;
        status.skipped_files += 1;
        status.completed_files += 1;
        status.not_started_files = status.not_started_files.saturating_sub(1);
        status.source_write_requests = 0;
        Ok(())
    }

    fn execute_copy(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        seed: &ExecutionDraftSeed,
        sources: &HashMap<String, SourceFile>,
        permit: &SafetyPermit,
    ) -> Result<(), DomainError> {
        self.transition_step(
            &attempt.attempt_id,
            &step.attempt_step_id,
            "Planned",
            "Validating",
        )?;
        let source_record = sources
            .get(&step.source_relative_path.to_lowercase())
            .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "原始内存卡文件缺失。"))?;
        let source_path =
            resolve_contained_path(&seed.source.root_path, &source_record.relative_path)?;
        let partial_relative = step.partial_relative_path.as_ref().unwrap();
        let partial_path =
            resolve_contained_path(&attempt.job.target_path, Path::new(partial_relative))?;
        let final_relative = step.final_relative_path.as_ref().unwrap();
        let final_path =
            resolve_contained_path(&attempt.job.target_path, Path::new(final_relative))?;
        self.safety_gate.revalidate(permit, FeatureGate::Import)?;
        ensure_parent_directories(
            &attempt.job.target_path,
            &seed.target.root_probe,
            &final_path,
            self.faults.clone(),
        )?;
        let audit = ProcessIoAudit::default();
        let mut source = StableReadHandle::open(&source_path, AuditSide::Source, &audit)?;
        let source_before = source.probe(&audit)?;
        if source_before != source_record.identity {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "复制前来源身份变化。",
            ));
        }
        self.transition_step(
            &attempt.attempt_id,
            &step.attempt_step_id,
            "Validating",
            "Copying",
        )?;
        self.safety_gate.revalidate(permit, FeatureGate::Import)?;
        let mut writer = PartialWriteHandle::create_new(&partial_path, &self.faults)?;
        let created = writer.probe()?;
        let file_id = created.file_id.clone().ok_or_else(|| {
            DomainError::new(ErrorCode::IdentityMismatch, "partial 缺少稳定 file ID。")
        })?;
        let claim = id();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let source_res = step.source_reservation_id.as_ref().unwrap().clone();
        let partial_res = step.partial_reservation_id.as_ref().unwrap().clone();
        let final_res = step.target_reservation_id.as_ref().unwrap().clone();
        let attempt_clone = attempt.clone();
        let step_clone = step.clone();
        let claim_tx = claim.clone();
        let file_id_tx = file_id.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_clone,&step_clone)?;let changed=transaction.execute("UPDATE import_partial_ownership SET state='Owned',file_id=?2,created_size_observed=?3 WHERE ownership_id=?1 AND state='Planned'",params![&ownership,&file_id_tx,i64::try_from(created.size_bytes).unwrap_or(i64::MAX)]).map_err(map_sqlite_error)?;require_one(changed,"planned ownership")?;transaction.execute("INSERT INTO import_partial_claim(claim_id,ownership_id,attempt_step_id,claim_reason,lease_generation,source_read_reservation_id,partial_path_reservation_id,final_path_reservation_id,claimed_at) VALUES(?1,?2,?3,'Created',?4,?5,?6,?7,?8)",params![&claim_tx,&ownership,&step_clone.attempt_step_id,attempt_clone.lease_generation,&source_res,&partial_res,&final_res,now_micros()]).map_err(map_sqlite_error)?;Ok(())})?;
        let copied_hash = writer.copy_from(source.file_mut(), step.expected_bytes, || {
            self.cancel_requested.load(Ordering::Acquire)
        });
        if let Err(error) = copied_hash {
            drop(source);
            let _ = writer.flush(&self.faults);
            self.cleanup_owned_partial(attempt, step, &claim, &partial_path)?;
            return Err(error);
        }
        writer.flush(&self.faults)?;
        let mut partial = VerifiedPartialHandle::open(&partial_path)?;
        let partial_before = partial.probe()?;
        if partial_before.file_id.as_deref() != Some(file_id.as_slice())
            || partial_before.size_bytes != step.expected_bytes
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "partial 完整重读前身份或大小不匹配。",
            ));
        }
        let (source_hash, partial_hash, equal) =
            hash_and_compare(source.file_mut(), partial.file_mut())?;
        let source_after = source.probe(&audit)?;
        if !equal
            || source_hash != partial_hash
            || source_hash != source_record.full_hash
            || source_before != source_after
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "partial 与冻结来源的完整内容不一致。",
            ));
        }
        let time_preserved = partial
            .set_last_write_time(source_record.identity.mtime_filetime)
            .is_ok();
        let final_partial_probe = partial.probe()?;
        if final_partial_probe.file_id.as_deref() != Some(file_id.as_slice())
            || final_partial_probe.size_bytes != step.expected_bytes
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "设置文件时间后 partial 身份变化。",
            ));
        }
        let verified = id();
        let attempt_clone = attempt.clone();
        let step_clone = step.clone();
        let seed_clone = seed.clone();
        let claim_clone = claim.clone();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let verified_tx = verified.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_clone,&step_clone)?;let source_before_id=insert_snapshot(transaction,&step_clone.source_root_id,&step_clone.source_binding_id,&step_clone.source_volume_id,&step_clone.source_relative_path,&step_clone.source_path_key,&source_before,Some(&source_hash),now_micros())?;let source_after_id=insert_snapshot(transaction,&step_clone.source_root_id,&step_clone.source_binding_id,&step_clone.source_volume_id,&step_clone.source_relative_path,&step_clone.source_path_key,&source_after,Some(&source_hash),now_micros())?;let partial_snapshot=insert_snapshot(transaction,&attempt_clone.job.target_root_id,&attempt_clone.job.target_binding_id,&attempt_clone.job.target_volume_id,step_clone.partial_relative_path.as_ref().unwrap(),&path_key(Path::new(step_clone.partial_relative_path.as_ref().unwrap())),&final_partial_probe,Some(&partial_hash),now_micros())?;transaction.execute("INSERT INTO verified_partial_snapshot(verified_partial_snapshot_id,ownership_id,claim_id,attempt_step_id,final_partial_snapshot_id,source_before_snapshot_id,source_after_snapshot_id,content_hash,byte_equal,time_preserved,verified_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10)",params![&verified_tx,&ownership,&claim_clone,&step_clone.attempt_step_id,&partial_snapshot,&source_before_id,&source_after_id,partial_hash.as_slice(),i64::from(time_preserved),now_micros()]).map_err(map_sqlite_error)?;finish_step_row(transaction,&step_clone.attempt_step_id,"Copying","PartialVerified",None,(!time_preserved).then_some("LastWriteTimeNotPreserved"))?;let _=seed_clone;Ok(())})?;
        let intent = id();
        let attempt_clone = attempt.clone();
        let step_clone = step.clone();
        let claim_for_intent = claim.clone();
        let verified_for_intent = verified.clone();
        let final_res = step.target_reservation_id.as_ref().unwrap().clone();
        let intent_tx = intent.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_clone,&step_clone)?;transaction.execute("INSERT INTO import_publish_intent(publish_intent_id,attempt_step_id,partial_claim_id,verified_partial_snapshot_id,final_path_reservation_id,lease_generation,intent_state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'Prepared',?7)",params![&intent_tx,&step_clone.attempt_step_id,&claim_for_intent,&verified_for_intent,&final_res,attempt_clone.lease_generation,now_micros()]).map_err(map_sqlite_error)?;Ok(())})?;
        if final_path.exists() {
            return Err(DomainError::new(
                ErrorCode::ReservationConflict,
                "正式目标在发布前被并发占用。",
            ));
        }
        attempt_fence(&self.database, attempt, step)?;
        let intent_publish = intent.clone();
        self.database.writer()?.transaction(move|transaction|{let changed=transaction.execute("UPDATE import_publish_intent SET intent_state='Publishing' WHERE publish_intent_id=?1 AND intent_state='Prepared'",[&intent_publish]).map_err(map_sqlite_error)?;require_one(changed,"publish intent")?;Ok(())})?;
        self.safety_gate.revalidate(permit, FeatureGate::Import)?;
        partial.publish_no_replace(&final_path, &self.faults)?;
        self.faults
            .checkpoint(FaultPoint::ImportAfterPhysicalBeforeFact)?;
        let target_probe = partial.probe()?;
        let published = id();
        let attempt_clone = attempt.clone();
        let step_clone = step.clone();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let claim_for_fact = claim.clone();
        let intent_for_fact = intent.clone();
        let verified_for_fact = verified.clone();
        let final_key = step.final_path_key.as_ref().unwrap().clone();
        let published_tx = published.clone();
        let target_probe_tx = target_probe.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_clone,&step_clone)?;let target_snapshot=insert_snapshot(transaction,&attempt_clone.job.target_root_id,&attempt_clone.job.target_binding_id,&attempt_clone.job.target_volume_id,step_clone.final_relative_path.as_ref().unwrap(),&final_key,&target_probe_tx,Some(&partial_hash),now_micros())?;transaction.execute("INSERT INTO import_published_fact(published_fact_id,source_item_id,step_id,attempt_step_id,verified_partial_snapshot_id,publish_intent_id,target_snapshot_id,published_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![&published_tx,&step_clone.source_item_id,&step_clone.step_id,&step_clone.attempt_step_id,&verified_for_fact,&intent_for_fact,&target_snapshot,now_micros()]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_publish_intent SET intent_state='Published',finished_at=?2 WHERE publish_intent_id=?1 AND intent_state='Publishing'",params![&intent_for_fact,now_micros()]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_ownership SET state='ConsumedByPublish' WHERE ownership_id=?1 AND state='Owned'",[&ownership]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='Published' WHERE claim_id=?1 AND released_at IS NULL",params![&claim_for_fact,now_micros()]).map_err(map_sqlite_error)?;finish_step_row(transaction,&step_clone.attempt_step_id,"PartialVerified","Published",None,(!time_preserved).then_some("LastWriteTimeNotPreserved"))?;Ok(())})?;
        drop(partial);
        self.index_published(
            attempt,
            step,
            seed,
            &published,
            &source_hash,
            &target_probe,
            time_preserved,
            "Published",
        )?;
        let mut status = self.lock_status()?;
        status.published_files += 1;
        status.completed_files += 1;
        status.not_started_files = status.not_started_files.saturating_sub(1);
        status.source_write_requests = 0;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // The fact/catalog chain is passed without ambient mutable state.
    fn index_published(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        seed: &ExecutionDraftSeed,
        published_fact_id: &[u8],
        source_hash: &[u8; 32],
        target_probe: &FileProbe,
        time_preserved: bool,
        from_state: &str,
    ) -> Result<(), DomainError> {
        let role = match step.role {
            DraftMediaKind::Jpg => MediaKind::Jpg,
            DraftMediaKind::Raf => MediaKind::Raf,
        };
        let observation = AssetObservation {
            library_id: library_id(&attempt.job.library_id)?,
            root_id: root_id(&attempt.job.target_root_id)?,
            root_binding_id: binding_id(&attempt.job.target_binding_id)?,
            media_kind: role,
            relative_path: step.final_relative_path.clone().unwrap(),
            path_key: step.final_path_key.clone().unwrap(),
            file_id: target_probe.file_id.clone(),
            size_bytes: i64::try_from(target_probe.size_bytes).unwrap_or(i64::MAX),
            mtime_filetime: target_probe.mtime_filetime,
            observed_at: now_micros(),
        };
        let observed = self.catalog.observe_import_reserved_asset(
            observation.into(),
            &attempt.job.job_id,
            &attempt.attempt_id,
            attempt.lease_generation,
        )?;
        let indexed = id();
        let history = id();
        let published = published_fact_id.to_vec();
        let attempt = attempt.clone();
        let step = step.clone();
        let source_hash = *source_hash;
        let target_asset = observed.asset_id.as_bytes().to_vec();
        let source_volume = seed.source.volume_id.clone();
        let from_state = from_state.to_string();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt,&step)?;transaction.execute("INSERT INTO import_history(history_id,library_id,source_volume_id,source_file_id,source_size_bytes,source_mtime,source_full_hash,hash_algorithm,target_asset_id,published_fact_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'SHA-256',?8,?9,?10)",params![&history,&attempt.job.library_id,&source_volume,&step.source_file_id,i64::try_from(step.expected_bytes).unwrap_or(i64::MAX),source_mtime(transaction,&step.source_snapshot_id)?,source_hash.as_slice(),&target_asset,&published,now_micros()]).map_err(map_sqlite_error)?;transaction.execute("INSERT INTO import_indexed_fact(indexed_fact_id,published_fact_id,asset_id,indexed_at) VALUES(?1,?2,?3,?4)",params![&indexed,&published,&target_asset,now_micros()]).map_err(map_sqlite_error)?;finish_step_row(transaction,&step.attempt_step_id,&from_state,"Indexed",None,(!time_preserved).then_some("LastWriteTimeNotPreserved"))?;append_report_item(transaction,&attempt.report_id,&step,"Published",(!time_preserved).then_some("LastWriteTimeNotPreserved"),step.expected_bytes)?;Ok(())})
    }

    fn index_existing_published(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        seed: &ExecutionDraftSeed,
    ) -> Result<(), DomainError> {
        let connection = self.database.open_read_connection()?;
        let (published,hash,target_snapshot):(Vec<u8>,Vec<u8>,Vec<u8>)=connection.query_row("SELECT fact.published_fact_id,snapshot.content_hash,fact.target_snapshot_id FROM import_published_fact AS fact JOIN file_identity_snapshot AS snapshot ON snapshot.snapshot_id=fact.target_snapshot_id WHERE fact.step_id=?1",[&step.step_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(map_sqlite_error)?;
        drop(connection);
        let path = resolve_contained_path(
            &attempt.job.target_path,
            Path::new(step.final_relative_path.as_ref().unwrap()),
        )?;
        let handle = crate::roots::open_read_only(&path)?;
        let probe = crate::roots::probe_file(&handle)?;
        let expected: FileProbe = self.snapshot_probe(&target_snapshot)?;
        if probe != expected {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "已发布正式文件身份变化，不能补索引。",
            ));
        }
        let hash: [u8; 32] = hash
            .try_into()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "发布事实哈希长度无效。"))?;
        self.transition_step(
            &attempt.attempt_id,
            &step.attempt_step_id,
            "Planned",
            "ReconcilingPublished",
        )?;
        self.index_published(
            attempt,
            step,
            seed,
            &published,
            &hash,
            &probe,
            true,
            "ReconcilingPublished",
        )?;
        let mut status = self.lock_status()?;
        status.completed_files += 1;
        status.published_files += 1;
        status.not_started_files = status.not_started_files.saturating_sub(1);
        Ok(())
    }

    fn reconcile_owned(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        seed: &ExecutionDraftSeed,
        sources: &HashMap<String, SourceFile>,
        permit: &SafetyPermit,
    ) -> Result<(), DomainError> {
        self.transition_step(
            &attempt.attempt_id,
            &step.attempt_step_id,
            "Planned",
            "ValidatingOwnedPartial",
        )?;
        let source_record = sources
            .get(&step.source_relative_path.to_lowercase())
            .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "等待原始内存卡。"))?;
        let source_path =
            resolve_contained_path(&seed.source.root_path, &source_record.relative_path)?;
        let partial_relative = step
            .partial_relative_path
            .as_ref()
            .ok_or_else(|| DomainError::new(ErrorCode::Internal, "恢复步骤缺少 partial。"))?;
        let partial_path =
            resolve_contained_path(&attempt.job.target_path, Path::new(partial_relative))?;
        let final_path = resolve_contained_path(
            &attempt.job.target_path,
            Path::new(step.final_relative_path.as_ref().unwrap()),
        )?;
        let audit = ProcessIoAudit::default();
        let mut source = StableReadHandle::open(&source_path, AuditSide::Source, &audit)?;
        let source_before = source.probe(&audit)?;
        if source_before != source_record.identity {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "恢复前来源身份变化。",
            ));
        }
        let mut partial = VerifiedPartialHandle::open(&partial_path)?;
        let partial_before = partial.probe()?;
        let owned = self.load_ownership_probe(step.ownership_id.as_ref().unwrap())?;
        if partial_before.file_id != owned.file_id {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "Owned partial 身份不匹配。",
            ));
        }
        let (source_hash, partial_hash, equal) =
            hash_and_compare(source.file_mut(), partial.file_mut())?;
        let source_after = source.probe(&audit)?;
        if !equal
            || source_hash != partial_hash
            || source_hash != source_record.full_hash
            || source_before != source_after
            || partial_before.size_bytes != step.expected_bytes
        {
            drop(source);
            drop(partial);
            return self
                .clean_incomplete_owned(attempt, step, &partial_path)
                .and_then(|_| {
                    Err(DomainError::new(
                        ErrorCode::StaleVersion,
                        "不完整 partial 已安全清理；请重试以创建新 incarnation。",
                    ))
                });
        }
        let time_preserved = partial
            .set_last_write_time(source_record.identity.mtime_filetime)
            .is_ok();
        let final_partial = partial.probe()?;
        let claim = self.active_claim_for_step(&step.attempt_step_id)?;
        let verified = id();
        let attempt_c = attempt.clone();
        let step_c = step.clone();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let claim_c = claim.clone();
        let verified_tx = verified.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_c,&step_c)?;let source_before_id=insert_snapshot(transaction,&step_c.source_root_id,&step_c.source_binding_id,&step_c.source_volume_id,&step_c.source_relative_path,&step_c.source_path_key,&source_before,Some(&source_hash),now_micros())?;let source_after_id=insert_snapshot(transaction,&step_c.source_root_id,&step_c.source_binding_id,&step_c.source_volume_id,&step_c.source_relative_path,&step_c.source_path_key,&source_after,Some(&source_hash),now_micros())?;let partial_snapshot=insert_snapshot(transaction,&attempt_c.job.target_root_id,&attempt_c.job.target_binding_id,&attempt_c.job.target_volume_id,step_c.partial_relative_path.as_ref().unwrap(),&path_key(Path::new(step_c.partial_relative_path.as_ref().unwrap())),&final_partial,Some(&partial_hash),now_micros())?;transaction.execute("INSERT INTO verified_partial_snapshot(verified_partial_snapshot_id,ownership_id,claim_id,attempt_step_id,final_partial_snapshot_id,source_before_snapshot_id,source_after_snapshot_id,content_hash,byte_equal,time_preserved,verified_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10)",params![&verified_tx,&ownership,&claim_c,&step_c.attempt_step_id,&partial_snapshot,&source_before_id,&source_after_id,partial_hash.as_slice(),i64::from(time_preserved),now_micros()]).map_err(map_sqlite_error)?;finish_step_row(transaction,&step_c.attempt_step_id,"ValidatingOwnedPartial","PartialVerified",None,(!time_preserved).then_some("LastWriteTimeNotPreserved"))?;Ok(())})?;
        let intent = id();
        let attempt_c = attempt.clone();
        let step_c = step.clone();
        let final_res = step.target_reservation_id.as_ref().unwrap().clone();
        let claim_c = claim.clone();
        let verified_c = verified.clone();
        let intent_tx = intent.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_c,&step_c)?;transaction.execute("INSERT INTO import_publish_intent(publish_intent_id,attempt_step_id,partial_claim_id,verified_partial_snapshot_id,final_path_reservation_id,lease_generation,intent_state,created_at) VALUES(?1,?2,?3,?4,?5,?6,'Prepared',?7)",params![&intent_tx,&step_c.attempt_step_id,&claim_c,&verified_c,&final_res,attempt_c.lease_generation,now_micros()]).map_err(map_sqlite_error)?;Ok(())})?;
        if final_path.exists() {
            return Err(DomainError::new(
                ErrorCode::ReservationConflict,
                "正式目标已被占用。",
            ));
        }
        let intent_c = intent.clone();
        self.database.writer()?.transaction(move|transaction|{require_one(transaction.execute("UPDATE import_publish_intent SET intent_state='Publishing' WHERE publish_intent_id=?1 AND intent_state='Prepared'",[&intent_c]).map_err(map_sqlite_error)?,"publish intent")?;Ok(())})?;
        self.safety_gate.revalidate(permit, FeatureGate::Import)?;
        partial.publish_no_replace(&final_path, &self.faults)?;
        self.faults
            .checkpoint(FaultPoint::ImportAfterPhysicalBeforeFact)?;
        let target_probe = partial.probe()?;
        let published = id();
        let attempt_c = attempt.clone();
        let step_c = step.clone();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let claim_c = claim.clone();
        let intent_c = intent.clone();
        let verified_c = verified.clone();
        let final_key = step.final_path_key.as_ref().unwrap().clone();
        let published_tx = published.clone();
        let target_probe_tx = target_probe.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_c,&step_c)?;let snapshot=insert_snapshot(transaction,&attempt_c.job.target_root_id,&attempt_c.job.target_binding_id,&attempt_c.job.target_volume_id,step_c.final_relative_path.as_ref().unwrap(),&final_key,&target_probe_tx,Some(&partial_hash),now_micros())?;transaction.execute("INSERT INTO import_published_fact(published_fact_id,source_item_id,step_id,attempt_step_id,verified_partial_snapshot_id,publish_intent_id,target_snapshot_id,published_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![&published_tx,&step_c.source_item_id,&step_c.step_id,&step_c.attempt_step_id,&verified_c,&intent_c,&snapshot,now_micros()]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_publish_intent SET intent_state='Published',finished_at=?2 WHERE publish_intent_id=?1",params![&intent_c,now_micros()]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_ownership SET state='ConsumedByPublish' WHERE ownership_id=?1 AND state='Owned'",[&ownership]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='Published' WHERE claim_id=?1",params![&claim_c,now_micros()]).map_err(map_sqlite_error)?;finish_step_row(transaction,&step_c.attempt_step_id,"PartialVerified","Published",None,None)?;Ok(())})?;
        drop(partial);
        self.index_published(
            attempt,
            step,
            seed,
            &published,
            &source_hash,
            &target_probe,
            time_preserved,
            "Published",
        )
    }

    fn clean_incomplete_owned(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        partial_path: &Path,
    ) -> Result<(), DomainError> {
        let claim = self.active_claim_for_step(&step.attempt_step_id)?;
        let handle = VerifiedPartialHandle::open(partial_path)?;
        let observed = handle.probe()?;
        let expected = self.load_ownership_probe(step.ownership_id.as_ref().unwrap())?;
        if observed.file_id != expected.file_id {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "partial 身份异常，禁止清理。",
            ));
        }
        let deleted_snapshot = id();
        let attempt_c = attempt.clone();
        let step_c = step.clone();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let deleted_snapshot_tx = deleted_snapshot.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_c,&step_c)?;insert_snapshot_with_id(transaction,&deleted_snapshot_tx,&attempt_c.job.target_root_id,&attempt_c.job.target_binding_id,&attempt_c.job.target_volume_id,step_c.partial_relative_path.as_ref().unwrap(),&path_key(Path::new(step_c.partial_relative_path.as_ref().unwrap())),&observed,None,now_micros())?;require_one(transaction.execute("UPDATE import_partial_ownership SET state='CleaningOwnedPartial' WHERE ownership_id=?1 AND state='Owned'",[&ownership]).map_err(map_sqlite_error)?,"owned partial")?;require_one(transaction.execute("UPDATE import_attempt_step SET step_state='CleaningOwnedPartial',row_version=row_version+1 WHERE attempt_step_id=?1 AND step_state IN ('Copying','ValidatingOwnedPartial')",[&step_c.attempt_step_id]).map_err(map_sqlite_error)?,"cleaning attempt step")?;Ok(())})?;
        let cleanup_permit = self.safety_gate.require_feature(FeatureGate::Import)?;
        self.safety_gate
            .revalidate(&cleanup_permit, FeatureGate::Import)?;
        handle.delete_owned(&self.faults)?;
        let cleanup = id();
        let attempt_c = attempt.clone();
        let step_c = step.clone();
        let ownership = step.ownership_id.as_ref().unwrap().clone();
        let claim_c = claim.clone();
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt_c,&step_c)?;transaction.execute("INSERT INTO import_partial_cleanup_fact(cleanup_fact_id,ownership_id,claim_id,attempt_step_id,deleted_snapshot_id,outcome,cleaned_at) VALUES(?1,?2,?3,?4,?5,'DeleteSucceeded',?6)",params![&cleanup,&ownership,&claim_c,&step_c.attempt_step_id,&deleted_snapshot,now_micros()]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_ownership SET state='Cleaned' WHERE ownership_id=?1 AND state='CleaningOwnedPartial'",[&ownership]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='Cleaned' WHERE claim_id=?1 AND released_at IS NULL",params![&claim_c,now_micros()]).map_err(map_sqlite_error)?;finish_step_row(transaction,&step_c.attempt_step_id,"CleaningOwnedPartial","CleanedPartial",None,None)?;Ok(())})
    }

    fn cleanup_owned_partial(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        _claim: &[u8],
        partial_path: &Path,
    ) -> Result<(), DomainError> {
        self.clean_incomplete_owned(attempt, step, partial_path)
    }

    fn finish_simple_step(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), DomainError> {
        let attempt = attempt.clone();
        let step = step.clone();
        let state = state.to_string();
        let error = error.map(str::to_string);
        self.database.writer()?.transaction(move |transaction| {
            require_fence(transaction, &attempt, &step)?;
            finish_step_row(
                transaction,
                &step.attempt_step_id,
                "Planned",
                &state,
                error.as_deref(),
                None,
            )?;
            append_report_item(transaction, &attempt.report_id, &step, &state, None, 0)?;
            Ok(())
        })?;
        let mut status = self.lock_status()?;
        status.completed_files += 1;
        status.not_started_files = status.not_started_files.saturating_sub(1);
        Ok(())
    }

    fn cancel_unstarted(&self, attempt: &AttemptContext, from: usize) -> Result<(), DomainError> {
        for step in attempt.steps.iter().skip(from) {
            self.mark_step_error(attempt, step, "Cancelled", None)?;
        }
        Ok(())
    }

    fn mark_step_error(
        &self,
        attempt: &AttemptContext,
        step: &AttemptStep,
        state: &str,
        error: Option<ErrorCode>,
    ) -> Result<(), DomainError> {
        let attempt = attempt.clone();
        let step = step.clone();
        let state = state.to_string();
        let is_cancelled = state == "Cancelled";
        let error = error.map(|value| format!("{value:?}"));
        self.database.writer()?.transaction(move|transaction|{require_fence(transaction,&attempt,&step)?;transaction.execute("UPDATE import_attempt_step SET step_state=?2,error_code=?3,finished_at=?4,row_version=row_version+1 WHERE attempt_step_id=?1 AND finished_at IS NULL",params![&step.attempt_step_id,&state,&error,now_micros()]).map_err(map_sqlite_error)?;append_report_item(transaction,&attempt.report_id,&step,&state,None,0)?;if let Some(ownership)=&step.ownership_id{transaction.execute("UPDATE import_partial_ownership SET state='NeedsAttention' WHERE ownership_id=?1 AND state='Planned'",[ownership]).map_err(map_sqlite_error)?;}Ok(())})?;
        let mut status = self.lock_status()?;
        status.failed_files += u64::from(!is_cancelled);
        status.not_started_files = status.not_started_files.saturating_sub(1);
        Ok(())
    }

    fn finalize_attempt(
        &self,
        attempt: &AttemptContext,
        had_error: bool,
        cancelled: bool,
    ) -> Result<(), DomainError> {
        let attempt = attempt.clone();
        let job_id = attempt.job.job_id.clone();
        self.database.writer()?.transaction(move|transaction|{let now=now_micros();let (attempt_state,job_state,task_state,report_result,terminal)=if cancelled{("Cancelled","Cancelled","Cancelled","Cancelled",true)}else if had_error{("CompletedWithErrors","CompletedWithErrors","AwaitingUser","Partial",false)}else{("Completed","Completed","Completed","Success",true)};transaction.execute("UPDATE import_attempt SET attempt_state=?2,finished_at=?3,row_version=row_version+1 WHERE attempt_id=?1 AND attempt_state='Running'",params![&attempt.attempt_id,attempt_state,now]).map_err(map_sqlite_error)?;transaction.execute("UPDATE operation_report SET report_state='Finalized',completed_at=?2,result=?3,item_count=(SELECT COUNT(*) FROM operation_report_item WHERE report_id=?1),total_bytes=(SELECT COALESCE(SUM(size_bytes),0) FROM operation_report_item WHERE report_id=?1) WHERE report_id=?1 AND report_state='Open'",params![&attempt.report_id,now,report_result]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_job SET job_state=?2,completed_at=CASE WHEN ?3 THEN ?4 ELSE NULL END,row_version=row_version+1 WHERE job_id=?1",params![&attempt.job.job_id,job_state,terminal,now]).map_err(map_sqlite_error)?;transaction.execute("UPDATE task_intent SET lifecycle=?2,resolved_at=CASE WHEN ?3 THEN ?4 ELSE NULL END,row_version=row_version+1 WHERE task_id=?1",params![&attempt.job.job_id,task_state,terminal,now]).map_err(map_sqlite_error)?;transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='AttemptClosed' WHERE attempt_step_id IN (SELECT attempt_step_id FROM import_attempt_step WHERE attempt_id=?1) AND released_at IS NULL",params![&attempt.attempt_id,now]).map_err(map_sqlite_error)?;transaction.execute("UPDATE operation_reservation SET released_at=?2,release_reason='AttemptClosed' WHERE attempt_id=?1 AND released_at IS NULL",params![&attempt.attempt_id,now]).map_err(map_sqlite_error)?;if terminal{let (generation,version):(i64,i64)=transaction.query_row("SELECT lease_generation,row_version FROM file_write_lease WHERE slot=1 AND task_id=?1",[&attempt.job.job_id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(map_sqlite_error)?;transaction.execute("UPDATE file_write_lease SET task_id=NULL,lease_generation=?1,released_at=?2,row_version=?3 WHERE slot=1",params![generation+1,now,version+1]).map_err(map_sqlite_error)?;}Ok(())})?;
        let phase = if cancelled {
            ImportExecutionPhase::Cancelled
        } else if had_error {
            ImportExecutionPhase::CompletedWithErrors
        } else {
            ImportExecutionPhase::Completed
        };
        self.set_phase(phase, Some(&job_id), None)
    }

    fn transition_step(
        &self,
        attempt_id: &[u8],
        step_id: &[u8],
        from: &str,
        to: &str,
    ) -> Result<(), DomainError> {
        let attempt_id = attempt_id.to_vec();
        let step_id = step_id.to_vec();
        let from = from.to_string();
        let to = to.to_string();
        self.database.writer()?.transaction(move|transaction|{let changed=transaction.execute("UPDATE import_attempt_step SET step_state=?3,started_at=COALESCE(started_at,?4),row_version=row_version+1 WHERE attempt_step_id=?1 AND attempt_id=?2 AND step_state=?5",params![&step_id,&attempt_id,&to,now_micros(),&from]).map_err(map_sqlite_error)?;require_one(changed,"attempt step")?;Ok(())})
    }

    fn load_job_context(
        &self,
        job_id: &[u8],
        generation_id: &[u8],
        validation_id: &[u8],
        target_path: &Path,
    ) -> Result<JobContext, DomainError> {
        let connection = self.database.open_read_connection()?;
        let (library,target_root,target_binding,target_volume):(Vec<u8>,Vec<u8>,Vec<u8>,Vec<u8>)=connection.query_row("SELECT job.library_id,job.target_root_id,job.target_binding_id,binding.volume_id FROM import_job AS job JOIN root_binding AS binding ON binding.binding_id=job.target_binding_id WHERE job.job_id=?1",[job_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(map_sqlite_error)?;
        Ok(JobContext {
            job_id: job_id.to_vec(),
            generation_id: generation_id.to_vec(),
            validation_id: validation_id.to_vec(),
            target_root_id: target_root,
            target_binding_id: target_binding,
            target_volume_id: target_volume,
            target_path: target_path.to_path_buf(),
            library_id: library,
        })
    }

    fn load_plan_steps(
        &self,
        job_id: &[u8],
        generation_id: &[u8],
    ) -> Result<Vec<AttemptStep>, DomainError> {
        let connection = self.database.open_read_connection()?;
        let validation:Vec<u8>=connection.query_row("SELECT validation_id FROM import_plan_validation WHERE job_id=?1 AND generation_id=?2 AND outcome='Ready' ORDER BY validation_ordinal DESC LIMIT 1",params![job_id,generation_id],|row|row.get(0)).map_err(map_sqlite_error)?;
        let mut statement=connection.prepare("SELECT step.step_id,step.source_item_id,item.source_relative_path,item.source_path_key,item.source_file_id,item.source_snapshot_id,item.role,validation.disposition,step.final_relative_path,step.final_path_key,COALESCE(step.expected_bytes,0),snapshot.root_id,snapshot.root_binding_id,snapshot.volume_id,ownership.ownership_id,ownership.relative_path,ownership.file_id FROM import_plan_validation_item AS validation JOIN import_asset_step AS step ON step.step_id=validation.step_id JOIN import_source_item AS item ON item.source_item_id=step.source_item_id JOIN file_identity_snapshot AS snapshot ON snapshot.snapshot_id=item.source_snapshot_id LEFT JOIN import_partial_ownership AS ownership ON ownership.step_id=step.step_id AND ownership.state='Owned' WHERE validation.validation_id=?1 ORDER BY item.source_relative_path COLLATE NOCASE").map_err(map_sqlite_error)?;
        let rows = statement
            .query_map([&validation], |row| {
                let role: String = row.get(6)?;
                Ok(AttemptStep {
                    attempt_step_id: Vec::new(),
                    step_id: row.get(0)?,
                    source_item_id: row.get(1)?,
                    source_relative_path: row.get(2)?,
                    source_path_key: row.get(3)?,
                    source_file_id: row.get(4)?,
                    source_snapshot_id: row.get(5)?,
                    source_root_id: row.get(11)?,
                    source_binding_id: row.get(12)?,
                    source_volume_id: row.get(13)?,
                    role: if role == "Jpg" {
                        DraftMediaKind::Jpg
                    } else {
                        DraftMediaKind::Raf
                    },
                    disposition: row.get(7)?,
                    final_relative_path: row.get(8)?,
                    final_path_key: row.get(9)?,
                    target_file_id: row.get(16)?,
                    expected_bytes: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
                    source_reservation_id: None,
                    target_reservation_id: None,
                    partial_reservation_id: None,
                    ownership_id: row.get(14)?,
                    partial_relative_path: row.get(15)?,
                })
            })
            .map_err(map_sqlite_error)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(map_sqlite_error)?)
        }
        Ok(result)
    }

    fn load_generation_steps_for_validation(
        &self,
        generation_id: &[u8],
    ) -> Result<Vec<AttemptStep>, DomainError> {
        let connection = self.database.open_read_connection()?;
        let mut statement=connection.prepare("SELECT step.step_id,step.source_item_id,item.source_relative_path,item.source_path_key,item.source_file_id,item.source_snapshot_id,item.role,CASE WHEN indexed.indexed_fact_id IS NOT NULL THEN 'AlreadyResolved' WHEN published.published_fact_id IS NOT NULL THEN 'IndexPublished' WHEN ownership.state='Owned' THEN 'ReverifyOwnedPartial' WHEN step.plan_kind='SkipDuplicate' THEN 'SkipWithCurrentProof' WHEN step.plan_kind='Unreadable' THEN 'Unreadable' ELSE 'CopyNewBytes' END,step.final_relative_path,step.final_path_key,COALESCE(step.expected_bytes,0),snapshot.root_id,snapshot.root_binding_id,snapshot.volume_id,ownership.ownership_id,ownership.relative_path,ownership.file_id FROM import_asset_step AS step JOIN import_source_item AS item ON item.source_item_id=step.source_item_id JOIN file_identity_snapshot AS snapshot ON snapshot.snapshot_id=item.source_snapshot_id LEFT JOIN import_partial_ownership AS ownership ON ownership.step_id=step.step_id AND ownership.state='Owned' LEFT JOIN import_published_fact AS published ON published.source_item_id=step.source_item_id LEFT JOIN import_indexed_fact AS indexed ON indexed.published_fact_id=published.published_fact_id WHERE step.generation_id=?1 ORDER BY item.source_relative_path COLLATE NOCASE").map_err(map_sqlite_error)?;
        let rows = statement
            .query_map([generation_id], |row| {
                let role: String = row.get(6)?;
                Ok(AttemptStep {
                    attempt_step_id: Vec::new(),
                    step_id: row.get(0)?,
                    source_item_id: row.get(1)?,
                    source_relative_path: row.get(2)?,
                    source_path_key: row.get(3)?,
                    source_file_id: row.get(4)?,
                    source_snapshot_id: row.get(5)?,
                    source_root_id: row.get(11)?,
                    source_binding_id: row.get(12)?,
                    source_volume_id: row.get(13)?,
                    role: if role == "Jpg" {
                        DraftMediaKind::Jpg
                    } else {
                        DraftMediaKind::Raf
                    },
                    disposition: row.get(7)?,
                    final_relative_path: row.get(8)?,
                    final_path_key: row.get(9)?,
                    target_file_id: row.get(16)?,
                    expected_bytes: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
                    source_reservation_id: None,
                    target_reservation_id: None,
                    partial_reservation_id: None,
                    ownership_id: row.get(14)?,
                    partial_relative_path: row.get(15)?,
                })
            })
            .map_err(map_sqlite_error)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(map_sqlite_error)?)
        }
        Ok(result)
    }

    fn snapshot_probe(&self, snapshot_id: &[u8]) -> Result<FileProbe, DomainError> {
        self.database.open_read_connection()?.query_row("SELECT file_id,size_bytes,mtime_filetime FROM file_identity_snapshot WHERE snapshot_id=?1",[snapshot_id],|row|Ok(FileProbe{file_id:row.get(0)?,size_bytes:u64::try_from(row.get::<_,i64>(1)?).unwrap_or(0),mtime_filetime:row.get(2)?,hard_link_count:1,reparse_point:false})).map_err(map_sqlite_error)
    }
    fn load_ownership_probe(&self, ownership_id: &[u8]) -> Result<FileProbe, DomainError> {
        self.database.open_read_connection()?.query_row("SELECT file_id,created_size_observed FROM import_partial_ownership WHERE ownership_id=?1 AND state IN ('Owned','CleaningOwnedPartial')",[ownership_id],|row|Ok(FileProbe{file_id:row.get(0)?,size_bytes:u64::try_from(row.get::<_,i64>(1)?).unwrap_or(0),mtime_filetime:0,hard_link_count:1,reparse_point:false})).map_err(map_sqlite_error)
    }
    fn active_claim_for_step(&self, attempt_step_id: &[u8]) -> Result<Vec<u8>, DomainError> {
        self.database.open_read_connection()?.query_row("SELECT claim_id FROM import_partial_claim WHERE attempt_step_id=?1 AND released_at IS NULL",[attempt_step_id],|row|row.get(0)).map_err(map_sqlite_error)
    }
    fn active_job_id(&self) -> Result<Option<Vec<u8>>, DomainError> {
        self.database.open_read_connection()?.query_row("SELECT job_id FROM import_job WHERE job_state NOT IN ('Completed','Cancelled') ORDER BY created_at DESC LIMIT 1",(),|row|row.get(0)).optional().map_err(map_sqlite_error)
    }
    fn close_interrupted_attempt(&self, job_id: &[u8]) -> Result<(), DomainError> {
        let connection = self.database.open_read_connection()?;
        let active:Option<Vec<u8>>=connection.query_row("SELECT attempt_id FROM import_attempt WHERE job_id=?1 AND attempt_state IN ('Prepared','Running')",[job_id],|row|row.get(0)).optional().map_err(map_sqlite_error)?;
        let Some(attempt_id) = active else {
            return Ok(());
        };
        let target = connection.query_row(
            "SELECT binding.absolute_path_display,binding.absolute_path_key,binding.root_directory_file_id,binding.filesystem_kind,volume.strong_identity_key
             FROM import_job AS job
             JOIN root_binding AS binding ON binding.binding_id=job.target_binding_id
             JOIN volume_identity AS volume ON volume.volume_id=binding.volume_id
             WHERE job.job_id=?1 AND binding.binding_state='Active'",
            [job_id],
            |row| Ok(BoundTargetRecoveryRoot {
                display_path: row.get(0)?,
                path_key: row.get(1)?,
                directory_file_id: row.get(2)?,
                filesystem: row.get(3)?,
                volume_key: row.get(4)?,
            }),
        ).map_err(map_sqlite_error)?;
        drop(connection);
        let target_path = PathBuf::from(target.display_path);
        let current = probe_directory(&target_path)?;
        if target.filesystem != "NTFS"
            || current.filesystem_kind != FilesystemKind::Ntfs
            || current.absolute_path_key != target.path_key
            || current.directory_file_id != target.directory_file_id
            || target.volume_key.as_deref() != Some(current.volume_identity_key.as_slice())
            || crate::roots::path_is_reparse(&target_path)?
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "未终结 Import 的 Fuji 图库 Root 身份无法证明。",
            ));
        }
        self.reconcile_publish_intents(&attempt_id, &target_path)?;
        self.reconcile_cleaning_partials(&attempt_id, &target_path)?;
        let job = job_id.to_vec();
        self.database.writer()?.transaction(move|transaction|{
            let now=now_micros();
            transaction.execute("UPDATE import_partial_ownership SET state='NeedsAttention' WHERE created_by_attempt_step_id IN (SELECT attempt_step_id FROM import_attempt_step WHERE attempt_id=?1) AND state='Planned'",[&attempt_id]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE import_attempt_step SET step_state='Interrupted',finished_at=?2,row_version=row_version+1 WHERE attempt_id=?1 AND finished_at IS NULL",params![&attempt_id,now]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT OR IGNORE INTO operation_report_item(report_item_id,report_id,item_kind,stable_item_id,result_code,display_path_snapshot,size_bytes) SELECT randomblob(16),report.report_id,'ImportAssetStep',step.step_id,attempt_step.step_state,step.final_relative_path,COALESCE(step.expected_bytes,0) FROM operation_report AS report JOIN import_attempt_step AS attempt_step ON attempt_step.attempt_id=report.task_attempt_id JOIN import_asset_step AS step ON step.step_id=attempt_step.step_id WHERE report.task_attempt_id=?1",[&attempt_id]).map_err(map_sqlite_error)?;
            require_one(transaction.execute("UPDATE import_attempt SET attempt_state='Interrupted',finished_at=?2,row_version=row_version+1 WHERE attempt_id=?1 AND attempt_state IN ('Prepared','Running')",params![&attempt_id,now]).map_err(map_sqlite_error)?,"interrupted attempt")?;
            require_one(transaction.execute("UPDATE operation_report SET report_state='Finalized',completed_at=?2,result='NeedsAttention',item_count=(SELECT COUNT(*) FROM operation_report_item WHERE report_id=operation_report.report_id),total_bytes=(SELECT COALESCE(SUM(size_bytes),0) FROM operation_report_item WHERE report_id=operation_report.report_id) WHERE task_attempt_id=?1 AND report_state='Open'",params![&attempt_id,now]).map_err(map_sqlite_error)?,"interrupted report")?;
            transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='Interrupted' WHERE attempt_step_id IN (SELECT attempt_step_id FROM import_attempt_step WHERE attempt_id=?1) AND released_at IS NULL",params![&attempt_id,now]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE operation_reservation SET released_at=?2,release_reason='Interrupted' WHERE attempt_id=?1 AND released_at IS NULL",params![&attempt_id,now]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE import_job SET job_state='NeedsAttention',row_version=row_version+1 WHERE job_id=?1",[&job]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE task_intent SET lifecycle='NeedsAttention',row_version=row_version+1 WHERE task_id=?1 AND lifecycle='Active'",[&job]).map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn reconcile_publish_intents(
        &self,
        attempt_id: &[u8],
        target_root: &Path,
    ) -> Result<(), DomainError> {
        let connection = self.database.open_read_connection()?;
        let mut statement = connection.prepare(
            "SELECT intent.publish_intent_id,intent.attempt_step_id,step.step_id,step.source_item_id,
                    ownership.ownership_id,claim.claim_id,ownership.relative_path,step.final_relative_path,
                    step.final_path_key,verified.verified_partial_snapshot_id,snapshot.file_id,
                    snapshot.size_bytes,snapshot.mtime_filetime,snapshot.content_hash,intent.intent_state,
                    step.final_root_id,step.final_binding_id,binding.volume_id
             FROM import_publish_intent AS intent
             JOIN import_attempt_step AS attempt_step ON attempt_step.attempt_step_id=intent.attempt_step_id
             JOIN import_asset_step AS step ON step.step_id=attempt_step.step_id
             JOIN import_partial_claim AS claim ON claim.claim_id=intent.partial_claim_id
             JOIN import_partial_ownership AS ownership ON ownership.ownership_id=claim.ownership_id
             JOIN verified_partial_snapshot AS verified ON verified.verified_partial_snapshot_id=intent.verified_partial_snapshot_id
             JOIN file_identity_snapshot AS snapshot ON snapshot.snapshot_id=verified.final_partial_snapshot_id
             JOIN root_binding AS binding ON binding.binding_id=step.final_binding_id
             WHERE attempt_step.attempt_id=?1 AND intent.intent_state IN ('Prepared','Publishing')",
        ).map_err(map_sqlite_error)?;
        let rows = statement
            .query_map([attempt_id], |row| {
                let hash: Vec<u8> = row.get(13)?;
                let hash: [u8; 32] = hash.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok(PendingPublishRecovery {
                    intent_id: row.get(0)?,
                    attempt_step_id: row.get(1)?,
                    step_id: row.get(2)?,
                    source_item_id: row.get(3)?,
                    ownership_id: row.get(4)?,
                    claim_id: row.get(5)?,
                    partial_relative_path: row.get(6)?,
                    final_relative_path: row.get(7)?,
                    final_path_key: row.get(8)?,
                    verified_snapshot_id: row.get(9)?,
                    verified_file_id: row.get(10)?,
                    verified_size: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(u64::MAX),
                    verified_mtime: row.get(12)?,
                    verified_hash: hash,
                    intent_state: row.get(14)?,
                    target_root_id: row.get(15)?,
                    target_binding_id: row.get(16)?,
                    target_volume_id: row.get(17)?,
                })
            })
            .map_err(map_sqlite_error)?;
        let mut pending = Vec::new();
        for row in rows {
            pending.push(row.map_err(map_sqlite_error)?);
        }
        drop(statement);
        drop(connection);
        for item in pending {
            self.reconcile_one_publish(item, target_root)?;
        }
        Ok(())
    }

    fn reconcile_one_publish(
        &self,
        item: PendingPublishRecovery,
        target_root: &Path,
    ) -> Result<(), DomainError> {
        let partial_path =
            resolve_contained_path(target_root, Path::new(&item.partial_relative_path))?;
        let final_path = resolve_contained_path(target_root, Path::new(&item.final_relative_path))?;
        let partial_exists = partial_path.exists();
        let final_observation = if final_path.exists() {
            let mut file = crate::roots::open_read_only(&final_path)?;
            let probe = crate::roots::probe_file(&file)?;
            let hash = hash_read_handle(&mut file)?;
            Some((probe, hash))
        } else {
            None
        };
        let rename_proven = item.intent_state == "Publishing"
            && !partial_exists
            && final_observation.as_ref().is_some_and(|(probe, hash)| {
                probe.file_id.as_deref() == Some(item.verified_file_id.as_slice())
                    && probe.size_bytes == item.verified_size
                    && probe.mtime_filetime == item.verified_mtime
                    && *hash == item.verified_hash
            });
        if rename_proven {
            let (probe, _) = final_observation.expect("checked above");
            let published = id();
            let target_snapshot = id();
            let now = now_micros();
            let item_for_tx = item;
            return self.database.writer()?.transaction(move |transaction| {
                insert_snapshot_with_id(transaction,&target_snapshot,
                    &item_for_tx.target_root_id,&item_for_tx.target_binding_id,&item_for_tx.target_volume_id,
                    &item_for_tx.final_relative_path,&item_for_tx.final_path_key,&probe,Some(&item_for_tx.verified_hash),now)?;
                transaction.execute("INSERT INTO import_published_fact(published_fact_id,source_item_id,step_id,attempt_step_id,verified_partial_snapshot_id,publish_intent_id,target_snapshot_id,published_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![&published,&item_for_tx.source_item_id,&item_for_tx.step_id,&item_for_tx.attempt_step_id,&item_for_tx.verified_snapshot_id,&item_for_tx.intent_id,&target_snapshot,now]).map_err(map_sqlite_error)?;
                require_one(transaction.execute("UPDATE import_publish_intent SET intent_state='Published',finished_at=?2 WHERE publish_intent_id=?1 AND intent_state='Publishing'",params![&item_for_tx.intent_id,now]).map_err(map_sqlite_error)?,"publish recovery intent")?;
                require_one(transaction.execute("UPDATE import_partial_ownership SET state='ConsumedByPublish' WHERE ownership_id=?1 AND state='Owned'",[&item_for_tx.ownership_id]).map_err(map_sqlite_error)?,"publish recovery ownership")?;
                transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='Published' WHERE claim_id=?1 AND released_at IS NULL",params![&item_for_tx.claim_id,now]).map_err(map_sqlite_error)?;
                transaction.execute("UPDATE import_attempt_step SET step_state='PublishedNeedsIndex',finished_at=?2,row_version=row_version+1 WHERE attempt_step_id=?1 AND step_state='PartialVerified'",params![&item_for_tx.attempt_step_id,now]).map_err(map_sqlite_error)?;
                Ok(())
            });
        }

        let partial_still_owned = if partial_exists && final_observation.is_none() {
            let mut file = crate::roots::open_read_only(&partial_path)?;
            let probe = crate::roots::probe_file(&file)?;
            let hash = hash_read_handle(&mut file)?;
            probe.file_id.as_deref() == Some(item.verified_file_id.as_slice())
                && probe.size_bytes == item.verified_size
                && probe.mtime_filetime == item.verified_mtime
                && hash == item.verified_hash
        } else {
            false
        };
        self.database.writer()?.transaction(move |transaction| {
            if item.intent_state == "Prepared" {
                transaction.execute("UPDATE import_publish_intent SET intent_state='Publishing' WHERE publish_intent_id=?1 AND intent_state='Prepared'",[&item.intent_id]).map_err(map_sqlite_error)?;
            }
            require_one(transaction.execute("UPDATE import_publish_intent SET intent_state='NeedsAttention',finished_at=?2 WHERE publish_intent_id=?1 AND intent_state='Publishing'",params![&item.intent_id,now_micros()]).map_err(map_sqlite_error)?,"ambiguous publish intent")?;
            if !partial_still_owned {
                transaction.execute("UPDATE import_partial_ownership SET state='NeedsAttention' WHERE ownership_id=?1 AND state='Owned'",[&item.ownership_id]).map_err(map_sqlite_error)?;
            }
            transaction.execute("UPDATE import_attempt_step SET step_state='NeedsAttention',finished_at=?2,row_version=row_version+1 WHERE attempt_step_id=?1 AND finished_at IS NULL",params![&item.attempt_step_id,now_micros()]).map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn reconcile_cleaning_partials(
        &self,
        attempt_id: &[u8],
        target_root: &Path,
    ) -> Result<(), DomainError> {
        let connection = self.database.open_read_connection()?;
        let mut statement = connection.prepare(
            "SELECT ownership.ownership_id,claim.claim_id,attempt_step.attempt_step_id,
                    ownership.relative_path,ownership.file_id,snapshot.snapshot_id
             FROM import_partial_ownership AS ownership
             JOIN import_partial_claim AS claim ON claim.ownership_id=ownership.ownership_id AND claim.released_at IS NULL
             JOIN import_attempt_step AS attempt_step ON attempt_step.attempt_step_id=claim.attempt_step_id
             JOIN file_identity_snapshot AS snapshot ON snapshot.root_id=ownership.root_id
                AND snapshot.root_binding_id=ownership.root_binding_id AND snapshot.path_key=ownership.path_key
                AND snapshot.file_id=ownership.file_id
             WHERE attempt_step.attempt_id=?1 AND ownership.state='CleaningOwnedPartial'
               AND NOT EXISTS(SELECT 1 FROM import_partial_cleanup_fact AS fact WHERE fact.ownership_id=ownership.ownership_id)
             ORDER BY snapshot.captured_at DESC",
        ).map_err(map_sqlite_error)?;
        let rows = statement
            .query_map([attempt_id], |row| {
                Ok(PendingCleanupRecovery {
                    ownership_id: row.get(0)?,
                    claim_id: row.get(1)?,
                    attempt_step_id: row.get(2)?,
                    relative_path: row.get(3)?,
                    file_id: row.get(4)?,
                    deleted_snapshot_id: row.get(5)?,
                })
            })
            .map_err(map_sqlite_error)?;
        let mut pending = Vec::new();
        for row in rows {
            let row = row.map_err(map_sqlite_error)?;
            if !pending
                .iter()
                .any(|existing: &PendingCleanupRecovery| existing.ownership_id == row.ownership_id)
            {
                pending.push(row);
            }
        }
        drop(statement);
        drop(connection);
        for item in pending {
            let path = resolve_contained_path(target_root, Path::new(&item.relative_path))?;
            let outcome = if path.exists() {
                let handle = VerifiedPartialHandle::open(&path)?;
                if handle.probe()?.file_id.as_deref() != Some(item.file_id.as_slice()) {
                    self.mark_cleaning_needs_attention(&item)?;
                    continue;
                }
                handle.delete_owned(&self.faults)?;
                "DeleteSucceeded"
            } else {
                "ObservedMissingAfterCleaningIntent"
            };
            let cleanup_id = id();
            self.database.writer()?.transaction(move |transaction| {
                transaction.execute("INSERT INTO import_partial_cleanup_fact(cleanup_fact_id,ownership_id,claim_id,attempt_step_id,deleted_snapshot_id,outcome,cleaned_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![&cleanup_id,&item.ownership_id,&item.claim_id,&item.attempt_step_id,&item.deleted_snapshot_id,outcome,now_micros()]).map_err(map_sqlite_error)?;
                require_one(transaction.execute("UPDATE import_partial_ownership SET state='Cleaned' WHERE ownership_id=?1 AND state='CleaningOwnedPartial'",[&item.ownership_id]).map_err(map_sqlite_error)?,"cleaning ownership")?;
                transaction.execute("UPDATE import_partial_claim SET released_at=?2,release_reason='Cleaned' WHERE claim_id=?1 AND released_at IS NULL",params![&item.claim_id,now_micros()]).map_err(map_sqlite_error)?;
                transaction.execute("UPDATE import_attempt_step SET step_state='CleanedPartial',finished_at=?2,row_version=row_version+1 WHERE attempt_step_id=?1 AND step_state='CleaningOwnedPartial'",params![&item.attempt_step_id,now_micros()]).map_err(map_sqlite_error)?;
                Ok(())
            })?;
        }
        Ok(())
    }

    fn mark_cleaning_needs_attention(
        &self,
        item: &PendingCleanupRecovery,
    ) -> Result<(), DomainError> {
        let ownership = item.ownership_id.clone();
        let attempt_step = item.attempt_step_id.clone();
        self.database.writer()?.transaction(move |transaction| {
            transaction.execute("UPDATE import_partial_ownership SET state='NeedsAttention' WHERE ownership_id=?1 AND state='CleaningOwnedPartial'",[&ownership]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE import_attempt_step SET step_state='NeedsAttention',finished_at=?2,row_version=row_version+1 WHERE attempt_step_id=?1 AND step_state='CleaningOwnedPartial'",params![&attempt_step,now_micros()]).map_err(map_sqlite_error)?;
            Ok(())
        })
    }
    fn update_progress(&self, step: &AttemptStep, action: &str) -> Result<(), DomainError> {
        let mut status = self.lock_status()?;
        status.current_file = Some(step.source_relative_path.clone());
        status.current_action = Some(action.into());
        Ok(())
    }
    fn set_phase(
        &self,
        phase: ImportExecutionPhase,
        job_id: Option<&[u8]>,
        message: Option<&str>,
    ) -> Result<(), DomainError> {
        let mut status = self.lock_status()?;
        status.phase = phase;
        status.job_id = job_id.map(uuid_string).transpose()?;
        status.message = message.map(str::to_string);
        Ok(())
    }
    fn lock_status(&self) -> Result<std::sync::MutexGuard<'_, ImportExecutionStatus>, DomainError> {
        self.status.lock().map_err(lock_error)
    }
}
