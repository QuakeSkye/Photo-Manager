use std::{collections::HashSet, sync::Arc};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DuplicateReconciliationState {
    NotApplicable,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuplicateReconciliationOutcome {
    pub state: DuplicateReconciliationState,
    pub run_id: Option<String>,
    pub recycled_files: usize,
    pub recycled_bytes: i64,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DuplicateResultReconciler {
    database: Arc<Database>,
}

#[derive(Debug, Clone)]
struct CandidateMember {
    set_id: Vec<u8>,
    asset_id: Vec<u8>,
    asset_version: i64,
    batch_asset_id: Vec<u8>,
    recycled_fact_id: Vec<u8>,
    event_id: Vec<u8>,
    size_bytes: i64,
    retained_asset_id: Vec<u8>,
    retained_asset_version: i64,
    retained_root_binding_id: Vec<u8>,
    retained_path_key: Vec<u8>,
    retained_file_id: Option<Vec<u8>>,
    retained_size_bytes: i64,
    retained_mtime_filetime: i64,
}

impl DuplicateResultReconciler {
    pub(crate) fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub(crate) fn reconcile_batch(
        &self,
        batch_id: &[u8],
    ) -> Result<DuplicateReconciliationOutcome, DomainError> {
        let connection = self.database.open_read_connection()?;
        let runs = candidate_runs(&connection, batch_id)?;
        drop(connection);
        if runs.is_empty() {
            return Ok(DuplicateReconciliationOutcome {
                state: DuplicateReconciliationState::NotApplicable,
                run_id: None,
                recycled_files: 0,
                recycled_bytes: 0,
                message: None,
            });
        }
        let mut aggregate = DuplicateReconciliationOutcome {
            state: DuplicateReconciliationState::Applied,
            run_id: None,
            recycled_files: 0,
            recycled_bytes: 0,
            message: None,
        };
        for run_id in runs {
            let batch = batch_id.to_vec();
            let run = run_id.clone();
            let outcome = self
                .database
                .writer()?
                .transaction(move |transaction| reconcile_run(transaction, &batch, &run))?;
            aggregate.run_id = outcome.run_id.clone();
            aggregate.recycled_files += outcome.recycled_files;
            aggregate.recycled_bytes += outcome.recycled_bytes;
            if outcome.state == DuplicateReconciliationState::Rejected {
                aggregate.state = DuplicateReconciliationState::Rejected;
                aggregate.message = outcome.message;
            }
        }
        Ok(aggregate)
    }

    pub(crate) fn reject_batch(
        &self,
        batch_id: &[u8],
        reason: &str,
    ) -> Result<DuplicateReconciliationOutcome, DomainError> {
        let connection = self.database.open_read_connection()?;
        let runs = candidate_runs(&connection, batch_id)?;
        drop(connection);
        if runs.is_empty() {
            return Ok(DuplicateReconciliationOutcome {
                state: DuplicateReconciliationState::NotApplicable,
                run_id: None,
                recycled_files: 0,
                recycled_bytes: 0,
                message: Some(reason.into()),
            });
        }
        let mut aggregate = DuplicateReconciliationOutcome {
            state: DuplicateReconciliationState::Rejected,
            run_id: None,
            recycled_files: 0,
            recycled_bytes: 0,
            message: Some(reason.into()),
        };
        for run_id in runs {
            let batch = batch_id.to_vec();
            let run = run_id.clone();
            let reason = reason.to_string();
            let outcome = self
                .database
                .writer()?
                .transaction(move |transaction| reject_run(transaction, &batch, &run, &reason))?;
            aggregate.run_id = outcome.run_id;
            aggregate.recycled_files += outcome.recycled_files;
            aggregate.recycled_bytes += outcome.recycled_bytes;
        }
        Ok(aggregate)
    }
}

fn reject_run(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
    run_id: &[u8],
    reason: &str,
) -> Result<DuplicateReconciliationOutcome, DomainError> {
    if let Some(outcome) = existing_outcome(transaction, batch_id, run_id)? {
        return Ok(outcome);
    }
    let (run_revision, current_revision): (i64, i64) = transaction
        .query_row(
            "SELECT run.catalog_revision,revision.catalog_revision
             FROM duplicate_scan_run run
             JOIN library_catalog_revision revision ON revision.library_id=run.library_id
             WHERE run.run_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let parent: Option<(Vec<u8>, i64)> = transaction
        .query_row(
            "SELECT reconciliation_id,to_catalog_revision
             FROM duplicate_result_reconciliation
             WHERE source_run_id=?1 AND reconciliation_state='Applied'
             ORDER BY completed_at DESC LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let (files, bytes): (i64, i64) = transaction
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(identity.size_bytes),0)
             FROM deletion_batch_photo photo
             JOIN deletion_batch_asset asset ON asset.batch_photo_id=photo.batch_photo_id
             JOIN deletion_mark_asset_snapshot marked
               ON marked.mark_asset_snapshot_id=asset.mark_asset_snapshot_id
             JOIN file_identity_snapshot identity
               ON identity.snapshot_id=marked.file_identity_snapshot_id
             JOIN deletion_batch_retention_guard guard
               ON guard.batch_asset_id=asset.batch_asset_id
             JOIN duplicate_equivalence_proof proof
               ON proof.proof_id=guard.equivalence_proof_id
             JOIN deletion_recycled_fact fact ON fact.batch_asset_id=asset.batch_asset_id
             WHERE photo.batch_id=?1 AND proof.run_id=?2
               AND asset.resolution_state='Recycled'",
            params![batch_id, run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let id = Uuid::new_v4().as_bytes().to_vec();
    let from_revision = parent.as_ref().map_or(run_revision, |value| value.1);
    let now = now_micros();
    transaction
        .execute(
            "INSERT INTO duplicate_result_reconciliation(
                reconciliation_id,source_run_id,parent_reconciliation_id,recycle_batch_id,
                from_catalog_revision,to_catalog_revision,reconciliation_state,
                removed_asset_count,removed_bytes,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,'Pending',0,0,?7)",
            params![
                &id,
                run_id,
                parent.as_ref().map(|value| &value.0),
                batch_id,
                from_revision,
                current_revision,
                now,
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "UPDATE duplicate_result_reconciliation
             SET reconciliation_state='Rejected',removed_asset_count=?2,removed_bytes=?3,
                 rejection_reason=?4,completed_at=?5
             WHERE reconciliation_id=?1 AND reconciliation_state='Pending'",
            params![&id, files, bytes, reason, now],
        )
        .map_err(map_sqlite_error)?;
    Ok(DuplicateReconciliationOutcome {
        state: DuplicateReconciliationState::Rejected,
        run_id: Some(uuid_string(run_id)?),
        recycled_files: files as usize,
        recycled_bytes: bytes,
        message: Some(reason.into()),
    })
}

fn candidate_runs(connection: &Connection, batch_id: &[u8]) -> Result<Vec<Vec<u8>>, DomainError> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT proof.run_id
             FROM deletion_batch_photo batch_photo
             JOIN deletion_mark mark ON mark.mark_id=batch_photo.mark_id
             JOIN deletion_batch_asset batch_asset
               ON batch_asset.batch_photo_id=batch_photo.batch_photo_id
             JOIN deletion_batch_retention_guard guard
               ON guard.batch_asset_id=batch_asset.batch_asset_id
             JOIN duplicate_equivalence_proof proof
               ON proof.proof_id=guard.equivalence_proof_id
             JOIN deletion_recycled_fact recycled
               ON recycled.batch_asset_id=batch_asset.batch_asset_id
             WHERE batch_photo.batch_id=?1
               AND mark.mark_scope='DuplicateSubset'
               AND batch_asset.resolution_state='Recycled'
             ORDER BY proof.run_id",
        )
        .map_err(map_sqlite_error)?;
    statement
        .query_map([batch_id], |row| row.get(0))
        .map_err(map_sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)
}

fn reconcile_run(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
    run_id: &[u8],
) -> Result<DuplicateReconciliationOutcome, DomainError> {
    if let Some(outcome) = existing_outcome(transaction, batch_id, run_id)? {
        return Ok(outcome);
    }
    let members = load_candidates(transaction, batch_id, run_id)?;
    if members.is_empty() {
        return Ok(DuplicateReconciliationOutcome {
            state: DuplicateReconciliationState::NotApplicable,
            run_id: Some(uuid_string(run_id)?),
            recycled_files: 0,
            recycled_bytes: 0,
            message: None,
        });
    }
    let (library_id, run_binding, run_revision, run_state): (Vec<u8>, Vec<u8>, i64, String) =
        transaction
            .query_row(
                "SELECT library_id,root_binding_id,catalog_revision,run_state
                 FROM duplicate_scan_run WHERE run_id=?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(map_sqlite_error)?;
    let parent: Option<(Vec<u8>, i64)> = transaction
        .query_row(
            "SELECT reconciliation_id,to_catalog_revision
             FROM duplicate_result_reconciliation
             WHERE source_run_id=?1 AND reconciliation_state='Applied'
             ORDER BY completed_at DESC LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let from_revision = parent.as_ref().map_or(run_revision, |value| value.1);
    let current: Option<(Vec<u8>, i64, String, String)> = transaction
        .query_row(
            "SELECT revision.root_binding_id,revision.catalog_revision,root.lifecycle,binding.binding_state
             FROM library_catalog_revision revision
             JOIN root ON root.library_id=revision.library_id
             JOIN root_binding binding ON binding.binding_id=revision.root_binding_id
             WHERE revision.library_id=?1",
            [&library_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let rejection = match &current {
        None => Some("当前图库目录修订不可用。".to_string()),
        Some((binding, _, _, _)) if binding != &run_binding => {
            Some("回收期间图库 RootBinding 已改变。".into())
        }
        Some((_, _, lifecycle, binding_state))
            if lifecycle != "Online" || binding_state != "Active" =>
        {
            Some("回收后图库 Root 不在线，无法证明保留副本。".into())
        }
        _ if run_state != "Completed" => Some("来源重复检测没有完整完成。".into()),
        _ => validate_reconciliation(
            transaction,
            batch_id,
            run_id,
            &library_id,
            from_revision,
            current.as_ref().expect("checked current").1,
            &members,
        )?,
    };
    let to_revision = current.map_or(from_revision, |value| value.1);
    let reconciliation_id = Uuid::new_v4().as_bytes().to_vec();
    transaction
        .execute(
            "INSERT INTO duplicate_result_reconciliation(
                reconciliation_id,source_run_id,parent_reconciliation_id,recycle_batch_id,
                from_catalog_revision,to_catalog_revision,reconciliation_state,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,'Pending',?7)",
            params![
                &reconciliation_id,
                run_id,
                parent.as_ref().map(|value| &value.0),
                batch_id,
                from_revision,
                to_revision,
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    let recycled_bytes = members.iter().map(|member| member.size_bytes).sum::<i64>();
    if let Some(reason) = rejection {
        transaction
            .execute(
                "UPDATE duplicate_result_reconciliation
                 SET reconciliation_state='Rejected',removed_asset_count=?2,removed_bytes=?3,
                     rejection_reason=?4,completed_at=?5
                 WHERE reconciliation_id=?1 AND reconciliation_state='Pending'",
                params![
                    &reconciliation_id,
                    members.len() as i64,
                    recycled_bytes,
                    &reason,
                    now_micros(),
                ],
            )
            .map_err(map_sqlite_error)?;
        return Ok(DuplicateReconciliationOutcome {
            state: DuplicateReconciliationState::Rejected,
            run_id: Some(uuid_string(run_id)?),
            recycled_files: members.len(),
            recycled_bytes,
            message: Some(reason),
        });
    }
    for member in &members {
        transaction
            .execute(
                "INSERT INTO duplicate_result_reconciliation_member(
                    reconciliation_id,set_id,asset_id,asset_version,batch_asset_id,
                    recycled_fact_id,catalog_revision_event_id,removed_bytes
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    &reconciliation_id,
                    &member.set_id,
                    &member.asset_id,
                    member.asset_version,
                    &member.batch_asset_id,
                    &member.recycled_fact_id,
                    &member.event_id,
                    member.size_bytes,
                ],
            )
            .map_err(map_sqlite_error)?;
    }
    transaction
        .execute(
            "UPDATE duplicate_result_reconciliation
             SET reconciliation_state='Applied',removed_asset_count=?2,removed_bytes=?3,
                 completed_at=?4
             WHERE reconciliation_id=?1 AND reconciliation_state='Pending'",
            params![
                &reconciliation_id,
                members.len() as i64,
                recycled_bytes,
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(DuplicateReconciliationOutcome {
        state: DuplicateReconciliationState::Applied,
        run_id: Some(uuid_string(run_id)?),
        recycled_files: members.len(),
        recycled_bytes,
        message: Some("图库索引和重复结果已更新，无须重新检测。".into()),
    })
}

fn validate_reconciliation(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
    run_id: &[u8],
    library_id: &[u8],
    from_revision: i64,
    current_revision: i64,
    members: &[CandidateMember],
) -> Result<Option<String>, DomainError> {
    let event: Option<(Vec<u8>, i64, i64)> = transaction
        .query_row(
            "SELECT event_id,from_revision,to_revision FROM catalog_revision_event
             WHERE library_id=?1 AND recycle_batch_id=?2",
            params![library_id, batch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let Some((event_id, event_from, event_to)) = event else {
        return Ok(Some("没有找到本批次的定点目录更新事实。".into()));
    };
    if event_from != from_revision || event_to != current_revision {
        return Ok(Some("重复检测后还存在本批次之外的目录变化。".into()));
    }
    if members.iter().any(|member| member.event_id != event_id) {
        return Ok(Some("回收成员与目录变化事实不能一一对应。".into()));
    }
    let unexplained: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM catalog_revision_asset_change change
                WHERE change.event_id=?1 AND NOT EXISTS(
                    SELECT 1 FROM deletion_batch_photo photo
                    JOIN deletion_batch_asset asset ON asset.batch_photo_id=photo.batch_photo_id
                    JOIN deletion_recycled_fact fact ON fact.batch_asset_id=asset.batch_asset_id
                    WHERE photo.batch_id=?2 AND asset.asset_id=change.asset_id
                      AND asset.resolution_state='Recycled'
                )
             )",
            params![&event_id, batch_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if unexplained {
        return Ok(Some(
            "目录修订中存在无法由当前回收批次解释的资产变化。".into(),
        ));
    }
    for member in members {
        let deleted_valid: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM asset
                    JOIN catalog_revision_asset_change change ON change.asset_id=asset.asset_id
                    WHERE asset.asset_id=?1 AND asset.presence_state='Missing'
                      AND asset.current_version=change.after_asset_version
                      AND change.event_id=?2 AND change.after_presence='Missing'
                      AND change.before_asset_version=?3
                 )",
                params![&member.asset_id, &event_id, member.asset_version],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        if !deleted_valid {
            return Ok(Some("已回收成员没有对应的 Missing Asset 版本。".into()));
        }
        let retained_valid: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM asset
                    JOIN asset_revision revision
                      ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
                    WHERE asset.asset_id=?1 AND asset.current_version=?2
                      AND asset.presence_state='Present' AND revision.presence_state='Present'
                      AND revision.root_binding_id=?3 AND revision.path_key=?4
                      AND revision.file_id IS ?5 AND revision.size_bytes=?6
                      AND revision.mtime_filetime=?7
                 )",
                params![
                    &member.retained_asset_id,
                    member.retained_asset_version,
                    &member.retained_root_binding_id,
                    &member.retained_path_key,
                    &member.retained_file_id,
                    member.retained_size_bytes,
                    member.retained_mtime_filetime,
                ],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        if !retained_valid {
            return Ok(Some("算法建议保留的副本在回收期间发生了变化。".into()));
        }
    }
    let current_removed = members
        .iter()
        .map(|member| member.asset_id.clone())
        .collect::<HashSet<_>>();
    let mut statement = transaction
        .prepare(
            "SELECT DISTINCT observation.asset_id,observation.asset_version,
                    asset.current_version,asset.presence_state
             FROM duplicate_equivalence_proof proof
             JOIN duplicate_file_observation observation
               ON observation.observation_id IN (
                    proof.anchor_observation_id,proof.member_observation_id
               )
             JOIN asset ON asset.asset_id=observation.asset_id
             WHERE proof.run_id=?1 AND NOT EXISTS(
                SELECT 1 FROM duplicate_result_reconciliation_member removed
                JOIN duplicate_result_reconciliation reconciliation
                  ON reconciliation.reconciliation_id=removed.reconciliation_id
                WHERE reconciliation.source_run_id=?1
                  AND reconciliation.reconciliation_state='Applied'
                  AND removed.asset_id=observation.asset_id
             )",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(map_sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)?;
    if rows
        .into_iter()
        .any(|(asset_id, observed, current, presence)| {
            !current_removed.contains(&asset_id) && (observed != current || presence != "Present")
        })
    {
        return Ok(Some("重复结果中未删除的成员在回收期间发生了变化。".into()));
    }
    Ok(None)
}

fn load_candidates(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
    run_id: &[u8],
) -> Result<Vec<CandidateMember>, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT proof.set_id,batch_asset.asset_id,marked.asset_version,
                    batch_asset.batch_asset_id,recycled.recycled_fact_id,event.event_id,
                    deleted_identity.size_bytes,guard.retained_asset_id,
                    guard.retained_asset_version,retained_identity.root_binding_id,
                    retained_identity.path_key,retained_identity.file_id,
                    retained_identity.size_bytes,retained_identity.mtime_filetime
             FROM deletion_batch_photo batch_photo
             JOIN deletion_mark mark ON mark.mark_id=batch_photo.mark_id
             JOIN deletion_batch_asset batch_asset
               ON batch_asset.batch_photo_id=batch_photo.batch_photo_id
             JOIN deletion_mark_asset_snapshot marked
               ON marked.mark_asset_snapshot_id=batch_asset.mark_asset_snapshot_id
             JOIN file_identity_snapshot deleted_identity
               ON deleted_identity.snapshot_id=marked.file_identity_snapshot_id
             JOIN deletion_batch_retention_guard guard
               ON guard.batch_asset_id=batch_asset.batch_asset_id
             JOIN file_identity_snapshot retained_identity
               ON retained_identity.snapshot_id=guard.retained_snapshot_id
             JOIN duplicate_equivalence_proof proof
               ON proof.proof_id=guard.equivalence_proof_id
             JOIN deletion_recycled_fact recycled
               ON recycled.batch_asset_id=batch_asset.batch_asset_id
             JOIN catalog_revision_event event ON event.recycle_batch_id=batch_photo.batch_id
             JOIN catalog_revision_asset_change change
               ON change.event_id=event.event_id AND change.asset_id=batch_asset.asset_id
             WHERE batch_photo.batch_id=?1 AND proof.run_id=?2
               AND mark.mark_scope='DuplicateSubset'
               AND batch_asset.resolution_state='Recycled'
               AND change.after_presence='Missing'
             ORDER BY batch_asset.batch_asset_id",
        )
        .map_err(map_sqlite_error)?;
    statement
        .query_map(params![batch_id, run_id], |row| {
            Ok(CandidateMember {
                set_id: row.get(0)?,
                asset_id: row.get(1)?,
                asset_version: row.get(2)?,
                batch_asset_id: row.get(3)?,
                recycled_fact_id: row.get(4)?,
                event_id: row.get(5)?,
                size_bytes: row.get(6)?,
                retained_asset_id: row.get(7)?,
                retained_asset_version: row.get(8)?,
                retained_root_binding_id: row.get(9)?,
                retained_path_key: row.get(10)?,
                retained_file_id: row.get(11)?,
                retained_size_bytes: row.get(12)?,
                retained_mtime_filetime: row.get(13)?,
            })
        })
        .map_err(map_sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)
}

fn existing_outcome(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
    run_id: &[u8],
) -> Result<Option<DuplicateReconciliationOutcome>, DomainError> {
    transaction
        .query_row(
            "SELECT reconciliation_state,removed_asset_count,removed_bytes,rejection_reason
             FROM duplicate_result_reconciliation
             WHERE source_run_id=?1 AND recycle_batch_id=?2
               AND reconciliation_state!='Pending'",
            params![run_id, batch_id],
            |row| {
                let state: String = row.get(0)?;
                Ok(DuplicateReconciliationOutcome {
                    state: if state == "Applied" {
                        DuplicateReconciliationState::Applied
                    } else {
                        DuplicateReconciliationState::Rejected
                    },
                    run_id: Some(uuid_string(run_id).map_err(|_| rusqlite::Error::InvalidQuery)?),
                    recycled_files: row.get::<_, i64>(1)? as usize,
                    recycled_bytes: row.get(2)?,
                    message: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite_error)
}

fn uuid_string(bytes: &[u8]) -> Result<String, DomainError> {
    Uuid::from_slice(bytes)
        .map(|value| value.to_string())
        .map_err(|_| DomainError::new(ErrorCode::InvalidId, "协调事实中的 ID 无效。"))
}
