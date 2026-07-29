use std::sync::Arc;

use rusqlite::OptionalExtension;

use crate::{
    domain::{AttemptId, DomainError, ErrorCode, OperationId, VolumeId},
    roots::VolumeIdentityKey,
};

use super::{migration::now_micros, writer::DbWriter};

#[derive(Debug, Clone)]
pub struct OperationEventStore {
    writer: Arc<DbWriter>,
}

impl OperationEventStore {
    pub fn new(writer: Arc<DbWriter>) -> Self {
        Self { writer }
    }

    pub fn record(
        &self,
        operation_id: OperationId,
        operation_kind: String,
        request_hash: Vec<u8>,
    ) -> Result<bool, DomainError> {
        self.writer.transaction(move |transaction| {
            let existing = transaction
                .query_row(
                    "SELECT operation_kind, request_hash FROM operation_event WHERE operation_id=?1",
                    [operation_id.as_bytes().as_slice()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .optional()
                .map_err(super::migration::map_sqlite_error)?;

            if let Some((existing_kind, existing_hash)) = existing {
                if existing_kind == operation_kind && existing_hash == request_hash {
                    return Ok(false);
                }
                return Err(DomainError::new(
                    ErrorCode::DuplicateOperation,
                    "重复操作标识携带了不同请求，已拒绝执行。",
                ));
            }

            transaction
                .execute(
                    "INSERT INTO operation_event(
                        operation_id, operation_kind, request_hash, first_received_at
                     ) VALUES (?1, ?2, ?3, ?4)",
                    (
                        operation_id.as_bytes().as_slice(),
                        operation_kind,
                        request_hash,
                        now_micros(),
                    ),
                )
                .map_err(super::migration::map_sqlite_error)?;
            Ok(true)
        })
    }
}

#[derive(Debug, Clone)]
pub struct VolumePolicyStore {
    writer: Arc<DbWriter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicCloseOutcome {
    pub targeted_rescan_path_keys: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct AttemptCloser {
    writer: Arc<DbWriter>,
}

impl AttemptCloser {
    pub fn new(writer: Arc<DbWriter>) -> Self {
        Self { writer }
    }

    pub fn cancel_import_before_start(
        &self,
        attempt_id: AttemptId,
    ) -> Result<AtomicCloseOutcome, DomainError> {
        self.writer.transaction(move |transaction| {
            let state = transaction
                .query_row(
                    "SELECT task.task_id, attempt.attempt_reason, attempt.attempt_state,
                            job.job_state, task.lifecycle, report.report_state
                     FROM task_attempt AS identity
                     JOIN task_intent AS task ON task.task_id=identity.task_id
                     JOIN import_attempt AS attempt ON attempt.attempt_id=identity.attempt_id
                     JOIN import_job AS job ON job.job_id=attempt.job_id
                     JOIN operation_report AS report
                       ON report.task_attempt_id=identity.attempt_id
                     WHERE identity.attempt_id=?1 AND identity.attempt_kind='Import'",
                    [attempt_id.as_bytes().as_slice()],
                    |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .map_err(super::migration::map_sqlite_error)?;
            let (task_id, reason, attempt_state, job_state, task_state, report_state) = state;

            if reason != "CancelBeforeStart"
                || attempt_state != "Prepared"
                || job_state != "Prepared"
                || task_state != "Prepared"
                || report_state != "Open"
            {
                return Err(DomainError::new(
                    ErrorCode::InvalidTransition,
                    "CancelBeforeStart 原子关闭的前置状态不成立。",
                ));
            }

            let step_count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM import_attempt_step WHERE attempt_id=?1",
                    [attempt_id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .map_err(super::migration::map_sqlite_error)?;
            let reservation_count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM operation_reservation
                     WHERE attempt_id=?1 AND released_at IS NULL",
                    [attempt_id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .map_err(super::migration::map_sqlite_error)?;
            let owns_lease: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM file_write_lease WHERE slot=1 AND task_id=?1",
                    [&task_id],
                    |row| row.get(0),
                )
                .map_err(super::migration::map_sqlite_error)?;
            if step_count != 0 || reservation_count != 0 || owns_lease != 0 {
                return Err(DomainError::new(
                    ErrorCode::InvalidTransition,
                    "CancelBeforeStart 不得已有 Step、文件占用或写租约。",
                ));
            }

            let completed_at = now_micros();
            require_one(
                transaction
                    .execute(
                        "UPDATE operation_report
                         SET report_state='Finalized', completed_at=?1,
                             result='Cancelled', item_count=0, total_bytes=0
                         WHERE task_attempt_id=?2 AND report_state='Open'",
                        (completed_at, attempt_id.as_bytes().as_slice()),
                    )
                    .map_err(super::migration::map_sqlite_error)?,
                "OperationReport",
            )?;
            require_one(
                transaction
                    .execute(
                        "UPDATE import_attempt
                         SET attempt_state='Cancelled', finished_at=?1,
                             row_version=row_version+1
                         WHERE attempt_id=?2 AND attempt_state='Prepared'",
                        (completed_at, attempt_id.as_bytes().as_slice()),
                    )
                    .map_err(super::migration::map_sqlite_error)?,
                "ImportAttempt",
            )?;
            require_one(
                transaction
                    .execute(
                        "UPDATE import_job
                         SET job_state='Cancelled', completed_at=?1,
                             row_version=row_version+1
                         WHERE job_id=?2 AND job_state='Prepared'",
                        (completed_at, &task_id),
                    )
                    .map_err(super::migration::map_sqlite_error)?,
                "ImportJob",
            )?;
            require_one(
                transaction
                    .execute(
                        "UPDATE task_intent
                         SET lifecycle='Cancelled', resolved_at=?1,
                             row_version=row_version+1
                         WHERE task_id=?2 AND lifecycle='Prepared'",
                        (completed_at, &task_id),
                    )
                    .map_err(super::migration::map_sqlite_error)?,
                "TaskIntent",
            )?;

            Ok(AtomicCloseOutcome {
                targeted_rescan_path_keys: Vec::new(),
            })
        })
    }
}

fn require_one(changed: usize, entity: &'static str) -> Result<(), DomainError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DomainError::stale_version(entity))
    }
}

impl VolumePolicyStore {
    pub fn new(writer: Arc<DbWriter>) -> Self {
        Self { writer }
    }

    pub fn register_import_source_volume(
        &self,
        volume_id: VolumeId,
        strong_key: VolumeIdentityKey,
    ) -> Result<bool, DomainError> {
        let key = strong_key.as_str().as_bytes().to_vec();
        self.writer.transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO volume_identity(
                        volume_id, filesystem_kind, strong_identity_key,
                        identity_format_version, identity_strength, created_at
                     ) VALUES (?1, 'Other', ?2, 1, 'Strong', ?3)
                     ON CONFLICT(identity_format_version, strong_identity_key)
                     WHERE strong_identity_key IS NOT NULL DO NOTHING",
                    (volume_id.as_bytes().as_slice(), key, now_micros()),
                )
                .map_err(super::migration::map_sqlite_error)?;

            let canonical_id = transaction
                .query_row(
                    "SELECT volume_id FROM volume_identity
                     WHERE identity_format_version=1 AND strong_identity_key=?1",
                    [strong_key.as_str().as_bytes()],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .map_err(super::migration::map_sqlite_error)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO volume_policy(volume_id, deny_write, reason, created_at)
                     VALUES (?1, 1, 'ImportSource', ?2)
                     ON CONFLICT(volume_id) DO NOTHING",
                    (canonical_id, now_micros()),
                )
                .map_err(super::migration::map_sqlite_error)?;
            Ok(inserted == 1)
        })
    }
}
