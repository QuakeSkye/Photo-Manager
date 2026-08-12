use rusqlite::{OptionalExtension, Transaction, params};

use crate::{
    db::migration::{map_sqlite_error, now_micros},
    domain::{DomainError, ErrorCode},
};

use super::{
    CatalogService, bump_photo, candidate_lookup, checked_next, current_photo_for_asset,
    library_id_from, pairing, require_one,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecycledBatchCatalogResult {
    pub event_id: Option<Vec<u8>>,
    pub from_revision: i64,
    pub to_revision: i64,
    pub changed_assets: usize,
}

#[derive(Debug, Clone)]
struct RecycledAsset {
    asset_id: Vec<u8>,
    frozen_version: i64,
    current_version: i64,
    row_version: i64,
    presence_state: String,
    library_id: Vec<u8>,
    root_id: Vec<u8>,
    root_binding_id: Vec<u8>,
    relative_path: String,
    path_key: Vec<u8>,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
    hard_link_count: i64,
}

impl CatalogService {
    pub(crate) fn reconcile_recycled_batch(
        &self,
        batch_id: &[u8],
    ) -> Result<RecycledBatchCatalogResult, DomainError> {
        let batch_id = batch_id.to_vec();
        self.database.writer()?.transaction(move |transaction| {
            if let Some(existing) = existing_result(transaction, &batch_id)? {
                return Ok(existing);
            }
            let assets = load_recycled_assets(transaction, &batch_id)?;
            if assets.is_empty() {
                return Ok(RecycledBatchCatalogResult {
                    event_id: None,
                    from_revision: current_revision(transaction)?,
                    to_revision: current_revision(transaction)?,
                    changed_assets: 0,
                });
            }
            let library_id = library_id_from(&assets[0].library_id)?;
            let binding_id = super::binding_id_from(&assets[0].root_binding_id)?;
            if assets.iter().any(|asset| {
                asset.library_id != assets[0].library_id
                    || asset.root_binding_id != assets[0].root_binding_id
            }) {
                return Err(DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "回收批次跨越了不同的图库或 RootBinding，不能定点更新索引。",
                ));
            }

            let mut changes = Vec::with_capacity(assets.len());
            for asset in &assets {
                if asset.current_version != asset.frozen_version
                    || asset.presence_state != "Present"
                {
                    return Err(DomainError::new(
                        ErrorCode::IdentityMismatch,
                        "回收完成后 Asset 身份已经发生其他变化，不能安全沿用重复结果。",
                    ));
                }
                let next = checked_next(asset.current_version, "Asset")?;
                transaction
                    .execute(
                        "INSERT INTO asset_revision(
                            asset_id,asset_version,root_id,root_binding_id,relative_path,
                            path_key,path_key_version,file_id,size_bytes,mtime_filetime,
                            presence_state,hard_link_count,observed_at,scan_run_id
                         ) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8,?9,'Missing',?10,?11,NULL)",
                        params![
                            &asset.asset_id,
                            next,
                            &asset.root_id,
                            &asset.root_binding_id,
                            &asset.relative_path,
                            &asset.path_key,
                            &asset.file_id,
                            asset.size_bytes,
                            asset.mtime_filetime,
                            asset.hard_link_count,
                            now_micros(),
                        ],
                    )
                    .map_err(map_sqlite_error)?;
                require_one(
                    transaction
                        .execute(
                            "UPDATE asset SET current_version=?1,presence_state='Missing',
                                row_version=row_version+1
                             WHERE asset_id=?2 AND current_version=?3 AND row_version=?4
                               AND presence_state='Present'",
                            params![
                                next,
                                &asset.asset_id,
                                asset.current_version,
                                asset.row_version,
                            ],
                        )
                        .map_err(map_sqlite_error)?,
                    "Asset",
                )?;
                if let Some((photo_id, photo_version)) =
                    current_photo_for_asset(transaction, &asset.asset_id)?
                {
                    bump_photo(transaction, photo_id, photo_version)?;
                }
                changes.push(candidate_lookup::CatalogRevisionAssetChange {
                    asset_id: asset.asset_id.clone(),
                    before_asset_version: Some(asset.current_version),
                    after_asset_version: next,
                    before_presence: Some("Present".into()),
                    after_presence: "Missing".into(),
                    change_kind: "Missing",
                });
            }
            let event_id = candidate_lookup::bump_catalog_revision(
                transaction,
                library_id,
                binding_id,
                "RecycleTargeted",
                None,
                Some(&batch_id),
                &changes,
            )?;
            pairing::reconcile_library(transaction, library_id)?;
            let (from_revision, to_revision) = transaction
                .query_row(
                    "SELECT from_revision,to_revision FROM catalog_revision_event WHERE event_id=?1",
                    [&event_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(map_sqlite_error)?;
            Ok(RecycledBatchCatalogResult {
                event_id: Some(event_id),
                from_revision,
                to_revision,
                changed_assets: changes.len(),
            })
        })
    }
}

fn existing_result(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
) -> Result<Option<RecycledBatchCatalogResult>, DomainError> {
    transaction
        .query_row(
            "SELECT event.event_id,event.from_revision,event.to_revision,COUNT(change.asset_id)
             FROM catalog_revision_event event
             LEFT JOIN catalog_revision_asset_change change ON change.event_id=event.event_id
             WHERE event.recycle_batch_id=?1
             GROUP BY event.event_id",
            [batch_id],
            |row| {
                Ok(RecycledBatchCatalogResult {
                    event_id: Some(row.get(0)?),
                    from_revision: row.get(1)?,
                    to_revision: row.get(2)?,
                    changed_assets: row.get::<_, i64>(3)? as usize,
                })
            },
        )
        .optional()
        .map_err(map_sqlite_error)
}

fn current_revision(transaction: &Transaction<'_>) -> Result<i64, DomainError> {
    transaction
        .query_row(
            "SELECT COALESCE(MAX(catalog_revision),0) FROM library_catalog_revision",
            (),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)
}

fn load_recycled_assets(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
) -> Result<Vec<RecycledAsset>, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT batch_asset.asset_id,marked.asset_version,asset.current_version,
                    asset.row_version,asset.presence_state,asset.library_id,
                    revision.root_id,revision.root_binding_id,revision.relative_path,
                    revision.path_key,revision.file_id,revision.size_bytes,
                    revision.mtime_filetime,revision.hard_link_count
             FROM deletion_batch_photo batch_photo
             JOIN deletion_batch_asset batch_asset
               ON batch_asset.batch_photo_id=batch_photo.batch_photo_id
             JOIN deletion_mark_asset_snapshot marked
               ON marked.mark_asset_snapshot_id=batch_asset.mark_asset_snapshot_id
             JOIN deletion_recycled_fact recycled
               ON recycled.batch_asset_id=batch_asset.batch_asset_id
             JOIN deletion_attempt_asset attempt
               ON attempt.attempt_asset_id=recycled.attempt_asset_id
             JOIN asset ON asset.asset_id=batch_asset.asset_id
             JOIN asset_revision revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             WHERE batch_photo.batch_id=?1
               AND batch_asset.resolution_state='Recycled'
               AND attempt.source_path_missing=1
             ORDER BY batch_asset.batch_asset_id",
        )
        .map_err(map_sqlite_error)?;
    statement
        .query_map([batch_id], |row| {
            Ok(RecycledAsset {
                asset_id: row.get(0)?,
                frozen_version: row.get(1)?,
                current_version: row.get(2)?,
                row_version: row.get(3)?,
                presence_state: row.get(4)?,
                library_id: row.get(5)?,
                root_id: row.get(6)?,
                root_binding_id: row.get(7)?,
                relative_path: row.get(8)?,
                path_key: row.get(9)?,
                file_id: row.get(10)?,
                size_bytes: row.get(11)?,
                mtime_filetime: row.get(12)?,
                hard_link_count: row.get(13)?,
            })
        })
        .map_err(map_sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)
}
