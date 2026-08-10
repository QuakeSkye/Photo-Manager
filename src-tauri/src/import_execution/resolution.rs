use rusqlite::{OptionalExtension, Transaction};

use crate::{
    db::migration::{map_sqlite_error, now_micros},
    domain::{DomainError, ErrorCode, ResolutionFactId},
};

pub(super) fn insert_published(
    transaction: &Transaction<'_>,
    step_id: &[u8],
    indexed_fact_id: &[u8],
    asset_id: &[u8],
    asset_version: i64,
) -> Result<(), DomainError> {
    let evidence_snapshot_id: Vec<u8> = transaction
        .query_row(
            "SELECT published.target_snapshot_id
             FROM import_indexed_fact AS indexed
             JOIN import_published_fact AS published
               ON published.published_fact_id=indexed.published_fact_id
             WHERE indexed.indexed_fact_id=?1 AND published.step_id=?2",
            (indexed_fact_id, step_id),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    insert_resolution(
        transaction,
        step_id,
        asset_id,
        asset_version,
        "PublishedIndexed",
        Some(indexed_fact_id),
        None,
        &evidence_snapshot_id,
    )
}

pub(super) fn insert_skipped(
    transaction: &Transaction<'_>,
    step_id: &[u8],
    duplicate_proof_id: &[u8],
) -> Result<(), DomainError> {
    let (asset_id, asset_version, snapshot): (Vec<u8>, i64, Vec<u8>) = transaction
        .query_row(
            "SELECT asset.asset_id, asset.current_version, proof.target_after_snapshot_id
             FROM import_duplicate_proof AS proof
             JOIN asset ON asset.asset_id=proof.target_asset_id
             JOIN asset_revision AS revision
               ON revision.asset_id=asset.asset_id
              AND revision.asset_version=asset.current_version
             JOIN file_identity_snapshot AS snapshot
               ON snapshot.snapshot_id=proof.target_after_snapshot_id
             WHERE proof.duplicate_proof_id=?1 AND proof.step_id=?2
               AND proof.proof_scope='Execution'
               AND asset.presence_state='Present'
               AND revision.presence_state='Present'
               AND revision.root_id=snapshot.root_id
               AND revision.root_binding_id=snapshot.root_binding_id
               AND revision.path_key=snapshot.path_key
               AND revision.file_id IS snapshot.file_id
               AND revision.size_bytes=snapshot.size_bytes
               AND revision.mtime_filetime=snapshot.mtime_filetime",
            (duplicate_proof_id, step_id),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or_else(|| {
            DomainError::new(
                ErrorCode::IdentityMismatch,
                "重复目标已无法解析到执行期证明的当前 Asset。",
            )
        })?;
    insert_resolution(
        transaction,
        step_id,
        &asset_id,
        asset_version,
        "SkippedDuplicate",
        None,
        Some(duplicate_proof_id),
        &snapshot,
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_resolution(
    transaction: &Transaction<'_>,
    step_id: &[u8],
    asset_id: &[u8],
    asset_version: i64,
    kind: &str,
    indexed_fact_id: Option<&[u8]>,
    duplicate_proof_id: Option<&[u8]>,
    evidence_snapshot_id: &[u8],
) -> Result<(), DomainError> {
    let fact_id = ResolutionFactId::new();
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO import_asset_resolution_fact(
                resolution_fact_id, step_id, asset_id, asset_version,
                resolution_kind, indexed_fact_id, duplicate_proof_id,
                evidence_snapshot_id, resolved_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                fact_id.as_bytes().as_slice(),
                step_id,
                asset_id,
                asset_version,
                kind,
                indexed_fact_id,
                duplicate_proof_id,
                evidence_snapshot_id,
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    if inserted == 1 {
        return Ok(());
    }
    let matches: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM import_asset_resolution_fact
                WHERE step_id=?1 AND asset_id=?2 AND asset_version=?3
                  AND resolution_kind=?4
                  AND indexed_fact_id IS ?5
                  AND duplicate_proof_id IS ?6
                  AND evidence_snapshot_id=?7
             )",
            rusqlite::params![
                step_id,
                asset_id,
                asset_version,
                kind,
                indexed_fact_id,
                duplicate_proof_id,
                evidence_snapshot_id,
            ],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if matches {
        Ok(())
    } else {
        Err(DomainError::new(
            ErrorCode::DatabaseCorrupt,
            "导入 Step 已存在冲突的 Asset 解析事实，已停止写入。",
        ))
    }
}
