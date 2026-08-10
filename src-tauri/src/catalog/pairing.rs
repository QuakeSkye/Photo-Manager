use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, Transaction};

use crate::{
    db::migration::{map_sqlite_error, now_micros},
    domain::{AssetId, DomainError, ErrorCode, LibraryId, PhotoId},
};

use super::{file_name_display, natural_sort_key, photo_id_from, sync_photo_sorting, uuid_from};

#[derive(Debug, Clone)]
struct PresentAsset {
    asset_id: AssetId,
    asset_version: i64,
    photo_id: PhotoId,
    photo_version: i64,
    role: String,
    stem_key: String,
    capture: Option<String>,
    model: Option<String>,
    serial: Option<String>,
}

#[derive(Debug, Clone)]
struct Relation {
    asset_id: AssetId,
    role: String,
    present: bool,
}

pub fn reconcile_library(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
) -> Result<(), DomainError> {
    backfill_complete_import_resolutions(transaction, library_id)?;
    let mut present = load_present_assets(transaction, library_id)?;
    let imported = load_same_import_pairs(transaction, library_id)?;
    let desired = combined_desired_pairs(&present, &imported);
    let current = load_current_relations(transaction, library_id)?;

    for (photo_id, (version, relations)) in current {
        let jpg = relations.iter().find(|relation| relation.role == "Jpg");
        let raf = relations.iter().find(|relation| relation.role == "Raf");
        if let (Some(jpg), Some(raf)) = (jpg, raf) {
            let pair = (jpg.asset_id, raf.asset_id);
            if !jpg.present || !raf.present || !desired.contains(&pair) {
                split_or_detach(transaction, photo_id, version, &relations)?;
            }
        } else if relations.iter().any(|relation| !relation.present) {
            split_or_detach(transaction, photo_id, version, &relations)?;
        }
    }

    present = load_present_assets(transaction, library_id)?;
    let desired = combined_desired_pairs(&present, &imported);
    merge_desired_pairs(transaction, library_id, &present, desired, &imported)
}

pub fn reconcile_import_groups(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
) -> Result<(), DomainError> {
    backfill_complete_import_resolutions(transaction, library_id)?;
    let present = load_present_assets(transaction, library_id)?;
    let imported = load_same_import_pairs(transaction, library_id)?;
    merge_desired_pairs(
        transaction,
        library_id,
        &present,
        imported.clone(),
        &imported,
    )
}

fn combined_desired_pairs(
    present: &[PresentAsset],
    imported: &HashSet<(AssetId, AssetId)>,
) -> HashSet<(AssetId, AssetId)> {
    let mut desired = desired_pairs(present);
    let explicitly_resolved_assets = imported
        .iter()
        .flat_map(|(jpg, raf)| [*jpg, *raf])
        .collect::<HashSet<_>>();
    desired.retain(|pair| {
        imported.contains(pair)
            || (!explicitly_resolved_assets.contains(&pair.0)
                && !explicitly_resolved_assets.contains(&pair.1))
    });
    desired.extend(imported.iter().copied());
    desired
}

fn backfill_complete_import_resolutions(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO import_asset_resolution_fact(
                resolution_fact_id, step_id, asset_id, asset_version,
                resolution_kind, indexed_fact_id, duplicate_proof_id,
                evidence_snapshot_id, resolved_at
             )
             SELECT randomblob(16), published.step_id, asset.asset_id,
                    asset.current_version, 'PublishedIndexed', indexed.indexed_fact_id,
                    NULL, published.target_snapshot_id, ?2
             FROM import_indexed_fact AS indexed
             JOIN import_published_fact AS published
               ON published.published_fact_id=indexed.published_fact_id
             JOIN asset ON asset.asset_id=indexed.asset_id
             JOIN asset_revision AS revision
               ON revision.asset_id=asset.asset_id
              AND revision.asset_version=asset.current_version
             JOIN file_identity_snapshot AS snapshot
               ON snapshot.snapshot_id=published.target_snapshot_id
             WHERE asset.library_id=?1 AND asset.presence_state='Present'
               AND revision.presence_state='Present'
               AND revision.root_id=snapshot.root_id
               AND revision.root_binding_id=snapshot.root_binding_id
               AND revision.path_key=snapshot.path_key
               AND revision.file_id IS snapshot.file_id
               AND revision.size_bytes=snapshot.size_bytes
               AND revision.mtime_filetime=snapshot.mtime_filetime",
            (library_id.as_bytes().as_slice(), now_micros()),
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO import_asset_resolution_fact(
                resolution_fact_id, step_id, asset_id, asset_version,
                resolution_kind, indexed_fact_id, duplicate_proof_id,
                evidence_snapshot_id, resolved_at
             )
             SELECT randomblob(16), proof.step_id, asset.asset_id,
                    asset.current_version, 'SkippedDuplicate', NULL,
                    proof.duplicate_proof_id, proof.target_after_snapshot_id, ?2
             FROM import_duplicate_proof AS proof
             JOIN asset ON asset.asset_id=proof.target_asset_id
             JOIN asset_revision AS revision
               ON revision.asset_id=asset.asset_id
              AND revision.asset_version=asset.current_version
             JOIN file_identity_snapshot AS snapshot
               ON snapshot.snapshot_id=proof.target_after_snapshot_id
             WHERE proof.proof_scope='Execution'
               AND asset.library_id=?1 AND asset.presence_state='Present'
               AND revision.presence_state='Present'
               AND revision.root_id=snapshot.root_id
               AND revision.root_binding_id=snapshot.root_binding_id
               AND revision.path_key=snapshot.path_key
               AND revision.file_id IS snapshot.file_id
               AND revision.size_bytes=snapshot.size_bytes
               AND revision.mtime_filetime=snapshot.mtime_filetime",
            (library_id.as_bytes().as_slice(), now_micros()),
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn merge_desired_pairs(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
    present: &[PresentAsset],
    desired: HashSet<(AssetId, AssetId)>,
    imported: &HashSet<(AssetId, AssetId)>,
) -> Result<(), DomainError> {
    let by_id = present
        .iter()
        .map(|asset| (asset.asset_id, asset))
        .collect::<HashMap<_, _>>();
    for (jpg_id, raf_id) in desired {
        let jpg = by_id.get(&jpg_id).ok_or_else(catalog_invariant)?;
        let raf = by_id.get(&raf_id).ok_or_else(catalog_invariant)?;
        let evidence_kind = if imported.contains(&(jpg_id, raf_id)) {
            "SameImportGroup"
        } else {
            "HistoricalExact"
        };
        let evidence_id = ensure_pair_evidence(transaction, library_id, jpg, raf, evidence_kind)?;
        if jpg.photo_id != raf.photo_id {
            merge_pair(transaction, jpg, raf, &evidence_id)?;
        } else if !current_evidence_matches(transaction, jpg.photo_id, &evidence_id)? {
            refresh_pair_evidence(transaction, jpg, raf, &evidence_id)?;
        }
    }
    Ok(())
}

fn load_present_assets(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
) -> Result<Vec<PresentAsset>, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT asset.asset_id, asset.current_version, photo.photo_id, photo.photo_version,
                    relation.role, revision.relative_path, metadata.capture_local_key,
                    metadata.camera_model, metadata.camera_serial
             FROM asset
             JOIN asset_revision revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             JOIN asset_media_metadata metadata
               ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
             JOIN photo_asset relation
               ON relation.asset_id=asset.asset_id AND relation.to_photo_version IS NULL
             JOIN photo ON photo.photo_id=relation.photo_id AND photo.lifecycle='Active'
             WHERE asset.library_id=?1 AND asset.presence_state='Present'",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([library_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut assets = Vec::new();
    for row in rows {
        let (asset, asset_version, photo, photo_version, role, relative, capture, model, serial) =
            row.map_err(map_sqlite_error)?;
        let display = file_name_display(&relative);
        let stem = display
            .rsplit_once('.')
            .map_or(display.as_str(), |(stem, _)| stem);
        assets.push(PresentAsset {
            asset_id: AssetId::from_uuid(uuid_from(&asset)?),
            asset_version,
            photo_id: photo_id_from(&photo)?,
            photo_version,
            role,
            stem_key: stem.to_lowercase(),
            capture,
            model,
            serial,
        });
    }
    Ok(assets)
}

fn load_same_import_pairs(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
) -> Result<HashSet<(AssetId, AssetId)>, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT jpg_resolution.asset_id, raf_resolution.asset_id
             FROM import_group AS import_pair
             JOIN import_asset_step AS jpg_step
               ON jpg_step.group_id=import_pair.group_id AND jpg_step.role='Jpg'
             JOIN import_asset_step AS raf_step
               ON raf_step.group_id=import_pair.group_id AND raf_step.role='Raf'
             JOIN import_asset_resolution_fact AS jpg_resolution
               ON jpg_resolution.step_id=jpg_step.step_id
             JOIN import_asset_resolution_fact AS raf_resolution
               ON raf_resolution.step_id=raf_step.step_id
             JOIN asset AS jpg_asset ON jpg_asset.asset_id=jpg_resolution.asset_id
             JOIN asset AS raf_asset ON raf_asset.asset_id=raf_resolution.asset_id
             JOIN asset_revision AS jpg_current
               ON jpg_current.asset_id=jpg_asset.asset_id
              AND jpg_current.asset_version=jpg_asset.current_version
             JOIN asset_revision AS raf_current
               ON raf_current.asset_id=raf_asset.asset_id
              AND raf_current.asset_version=raf_asset.current_version
             JOIN file_identity_snapshot AS jpg_snapshot
               ON jpg_snapshot.snapshot_id=jpg_resolution.evidence_snapshot_id
             JOIN file_identity_snapshot AS raf_snapshot
               ON raf_snapshot.snapshot_id=raf_resolution.evidence_snapshot_id
             LEFT JOIN asset_media_metadata AS jpg_metadata
               ON jpg_metadata.asset_id=jpg_asset.asset_id
              AND jpg_metadata.asset_version=jpg_asset.current_version
             LEFT JOIN asset_media_metadata AS raf_metadata
               ON raf_metadata.asset_id=raf_asset.asset_id
              AND raf_metadata.asset_version=raf_asset.current_version
             WHERE import_pair.pair_evidence='SameImportGroup'
               AND jpg_asset.library_id=?1 AND raf_asset.library_id=?1
               AND jpg_asset.media_kind='Jpg' AND raf_asset.media_kind='Raf'
               AND jpg_asset.presence_state='Present'
               AND raf_asset.presence_state='Present'
               AND jpg_asset.current_version=jpg_resolution.asset_version
               AND raf_asset.current_version=raf_resolution.asset_version
               AND jpg_current.root_id=jpg_snapshot.root_id
               AND jpg_current.root_binding_id=jpg_snapshot.root_binding_id
               AND jpg_current.path_key=jpg_snapshot.path_key
               AND jpg_current.file_id IS jpg_snapshot.file_id
               AND jpg_current.size_bytes=jpg_snapshot.size_bytes
               AND jpg_current.mtime_filetime=jpg_snapshot.mtime_filetime
               AND raf_current.root_id=raf_snapshot.root_id
               AND raf_current.root_binding_id=raf_snapshot.root_binding_id
               AND raf_current.path_key=raf_snapshot.path_key
               AND raf_current.file_id IS raf_snapshot.file_id
               AND raf_current.size_bytes=raf_snapshot.size_bytes
               AND raf_current.mtime_filetime=raf_snapshot.mtime_filetime
               AND (jpg_metadata.capture_local_key IS NULL
                    OR raf_metadata.capture_local_key IS NULL
                    OR jpg_metadata.capture_local_key=raf_metadata.capture_local_key)",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([library_id.as_bytes().as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(map_sqlite_error)?;
    let mut pairs = HashSet::new();
    for row in rows {
        let (jpg, raf) = row.map_err(map_sqlite_error)?;
        let jpg = AssetId::from_uuid(uuid_from(&jpg)?);
        let raf = AssetId::from_uuid(uuid_from(&raf)?);
        if jpg != raf {
            pairs.insert((jpg, raf));
        }
    }
    Ok(pairs)
}

fn desired_pairs(assets: &[PresentAsset]) -> HashSet<(AssetId, AssetId)> {
    let mut groups: HashMap<&str, Vec<&PresentAsset>> = HashMap::new();
    for asset in assets {
        groups.entry(&asset.stem_key).or_default().push(asset);
    }
    let mut desired = HashSet::new();
    for group in groups.values() {
        let jpg = group
            .iter()
            .filter(|asset| asset.role == "Jpg")
            .copied()
            .collect::<Vec<_>>();
        let raf = group
            .iter()
            .filter(|asset| asset.role == "Raf")
            .copied()
            .collect::<Vec<_>>();
        if jpg.len() != 1 || raf.len() != 1 {
            continue;
        }
        let (jpg, raf) = (jpg[0], raf[0]);
        let reliable_time = jpg.capture.is_some() && jpg.capture == raf.capture;
        let model_conflict = conflicts(&jpg.model, &raf.model);
        let serial_conflict = conflicts(&jpg.serial, &raf.serial);
        if reliable_time && !model_conflict && !serial_conflict {
            desired.insert((jpg.asset_id, raf.asset_id));
        }
    }
    desired
}

fn conflicts(left: &Option<String>, right: &Option<String>) -> bool {
    matches!((left, right), (Some(left), Some(right)) if !left.eq_ignore_ascii_case(right))
}

fn load_current_relations(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
) -> Result<HashMap<PhotoId, (i64, Vec<Relation>)>, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT photo.photo_id, photo.photo_version, relation.asset_id, relation.role,
                    asset.presence_state
             FROM photo
             JOIN photo_asset relation ON relation.photo_id=photo.photo_id
                AND relation.to_photo_version IS NULL
             JOIN asset ON asset.asset_id=relation.asset_id
             WHERE photo.library_id=?1 AND photo.lifecycle='Active'",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([library_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut photos = HashMap::new();
    for row in rows {
        let (photo, version, asset, role, presence) = row.map_err(map_sqlite_error)?;
        photos
            .entry(photo_id_from(&photo)?)
            .or_insert_with(|| (version, Vec::new()))
            .1
            .push(Relation {
                asset_id: AssetId::from_uuid(uuid_from(&asset)?),
                role,
                present: presence == "Present",
            });
    }
    Ok(photos)
}

fn ensure_pair_evidence(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
    jpg: &PresentAsset,
    raf: &PresentAsset,
    evidence_kind: &str,
) -> Result<Vec<u8>, DomainError> {
    if let Some(id) = transaction
        .query_row(
            "SELECT pair_evidence_id FROM pair_evidence
             WHERE evidence_kind=?1 AND jpg_asset_id=?2 AND jpg_asset_version=?3
               AND raf_asset_id=?4 AND raf_asset_version=?5",
            (
                evidence_kind,
                jpg.asset_id.as_bytes().as_slice(),
                jpg.asset_version,
                raf.asset_id.as_bytes().as_slice(),
                raf.asset_version,
            ),
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
    {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().as_bytes().to_vec();
    transaction
        .execute(
            "INSERT INTO pair_evidence(
                pair_evidence_id, evidence_kind, library_id, jpg_asset_id, raf_asset_id,
                jpg_asset_version, raf_asset_version, capture_local_key, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                &id,
                evidence_kind,
                library_id.as_bytes().as_slice(),
                jpg.asset_id.as_bytes().as_slice(),
                raf.asset_id.as_bytes().as_slice(),
                jpg.asset_version,
                raf.asset_version,
                &jpg.capture,
                now_micros(),
            ),
        )
        .map_err(map_sqlite_error)?;
    Ok(id)
}

fn merge_pair(
    transaction: &Transaction<'_>,
    jpg: &PresentAsset,
    raf: &PresentAsset,
    evidence_id: &[u8],
) -> Result<(), DomainError> {
    let jpg_next = next(jpg.photo_version)?;
    let raf_next = next(raf.photo_version)?;
    close_relations(transaction, jpg.photo_id, jpg_next)?;
    close_relations(transaction, raf.photo_id, raf_next)?;
    insert_relation(
        transaction,
        jpg.photo_id,
        jpg.asset_id,
        "Jpg",
        jpg_next,
        Some(evidence_id),
    )?;
    insert_relation(
        transaction,
        jpg.photo_id,
        raf.asset_id,
        "Raf",
        jpg_next,
        Some(evidence_id),
    )?;
    update_photo_version(
        transaction,
        jpg.photo_id,
        jpg.photo_version,
        jpg_next,
        false,
    )?;
    update_photo_version(transaction, raf.photo_id, raf.photo_version, raf_next, true)?;
    sync_photo_sorting(transaction, jpg.photo_id)?;
    transaction
        .execute(
            "INSERT INTO photo_alias(old_photo_id, canonical_photo_id, reason, created_at)
             VALUES (?1, ?2, 'Merge', ?3)",
            (
                raf.photo_id.as_bytes().as_slice(),
                jpg.photo_id.as_bytes().as_slice(),
                now_micros(),
            ),
        )
        .map_err(map_sqlite_error)?;
    relation_event(
        transaction,
        "Merge",
        jpg.photo_id,
        Some(raf.photo_id),
        Some(raf.asset_id),
        jpg.photo_version,
        jpg_next,
    )?;
    Ok(())
}

fn refresh_pair_evidence(
    transaction: &Transaction<'_>,
    jpg: &PresentAsset,
    raf: &PresentAsset,
    evidence_id: &[u8],
) -> Result<(), DomainError> {
    let next_version = next(jpg.photo_version)?;
    close_relations(transaction, jpg.photo_id, next_version)?;
    insert_relation(
        transaction,
        jpg.photo_id,
        jpg.asset_id,
        "Jpg",
        next_version,
        Some(evidence_id),
    )?;
    insert_relation(
        transaction,
        jpg.photo_id,
        raf.asset_id,
        "Raf",
        next_version,
        Some(evidence_id),
    )?;
    update_photo_version(
        transaction,
        jpg.photo_id,
        jpg.photo_version,
        next_version,
        false,
    )?;
    sync_photo_sorting(transaction, jpg.photo_id)?;
    relation_event(
        transaction,
        "Merge",
        jpg.photo_id,
        None,
        None,
        jpg.photo_version,
        next_version,
    )
}

fn current_evidence_matches(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    evidence_id: &[u8],
) -> Result<bool, DomainError> {
    transaction
        .query_row(
            "SELECT COUNT(*)=2 FROM photo_asset
             WHERE photo_id=?1 AND to_photo_version IS NULL AND pair_evidence_id=?2",
            (photo_id.as_bytes().as_slice(), evidence_id),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)
}

fn split_or_detach(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    version: i64,
    relations: &[Relation],
) -> Result<(), DomainError> {
    let next_version = next(version)?;
    let keep = relations
        .iter()
        .find(|relation| relation.present && relation.role == "Jpg")
        .or_else(|| relations.iter().find(|relation| relation.present));
    let split = relations.iter().find(|relation| {
        relation.present && keep.is_some_and(|keep| keep.asset_id != relation.asset_id)
    });
    close_relations(transaction, photo_id, next_version)?;
    if let Some(keep) = keep {
        insert_relation(
            transaction,
            photo_id,
            keep.asset_id,
            &keep.role,
            next_version,
            None,
        )?;
        update_photo_version(transaction, photo_id, version, next_version, false)?;
        sync_photo_sorting(transaction, photo_id)?;
    } else {
        update_photo_version(transaction, photo_id, version, next_version, true)?;
    }
    if let Some(split) = split {
        let new_photo = create_single_photo(transaction, photo_id, split)?;
        relation_event(
            transaction,
            "Split",
            photo_id,
            Some(new_photo),
            Some(split.asset_id),
            version,
            next_version,
        )?;
    } else if let Some(removed) = relations.iter().find(|relation| !relation.present) {
        relation_event(
            transaction,
            "MemberRemoved",
            photo_id,
            None,
            Some(removed.asset_id),
            version,
            next_version,
        )?;
    }
    Ok(())
}

fn create_single_photo(
    transaction: &Transaction<'_>,
    source_photo_id: PhotoId,
    relation: &Relation,
) -> Result<PhotoId, DomainError> {
    let (library, capture, relative): (Vec<u8>, Option<String>, String) = transaction
        .query_row(
            "SELECT source.library_id, metadata.capture_local_key, revision.relative_path
             FROM photo source
             JOIN asset ON asset.asset_id=?2
             JOIN asset_revision revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             LEFT JOIN asset_media_metadata metadata
               ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
             WHERE source.photo_id=?1",
            (
                source_photo_id.as_bytes().as_slice(),
                relation.asset_id.as_bytes().as_slice(),
            ),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(map_sqlite_error)?;
    let id = PhotoId::new();
    let display = file_name_display(&relative);
    transaction
        .execute(
            "INSERT INTO photo(
                photo_id, library_id, photo_version, lifecycle, date_class,
                capture_local_key, natural_name_display, natural_sort_key,
                sort_key_version, created_at
             ) VALUES (?1, ?2, 1, 'Active', ?3, ?4, ?5, ?6, 1, ?7)",
            (
                id.as_bytes().as_slice(),
                library,
                if capture.is_some() {
                    "Known"
                } else {
                    "Unknown"
                },
                capture,
                &display,
                natural_sort_key(&display),
                now_micros(),
            ),
        )
        .map_err(map_sqlite_error)?;
    insert_relation(transaction, id, relation.asset_id, &relation.role, 1, None)?;
    Ok(id)
}

fn close_relations(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    to_version: i64,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "UPDATE photo_asset SET to_photo_version=?1
             WHERE photo_id=?2 AND to_photo_version IS NULL",
            (to_version, photo_id.as_bytes().as_slice()),
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn insert_relation(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    asset_id: AssetId,
    role: &str,
    from_version: i64,
    evidence_id: Option<&[u8]>,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "INSERT INTO photo_asset(
                photo_asset_id, photo_id, asset_id, role, pair_evidence_id, from_photo_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                uuid::Uuid::new_v4().as_bytes().as_slice(),
                photo_id.as_bytes().as_slice(),
                asset_id.as_bytes().as_slice(),
                role,
                evidence_id,
                from_version,
            ),
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn update_photo_version(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    old_version: i64,
    new_version: i64,
    tombstone: bool,
) -> Result<(), DomainError> {
    let changed = if tombstone {
        transaction.execute(
            "UPDATE photo SET photo_version=?1, lifecycle='Tombstoned', tombstoned_at=?2,
                    row_version=row_version+1
             WHERE photo_id=?3 AND photo_version=?4 AND lifecycle='Active'",
            (
                new_version,
                now_micros(),
                photo_id.as_bytes().as_slice(),
                old_version,
            ),
        )
    } else {
        transaction.execute(
            "UPDATE photo SET photo_version=?1, row_version=row_version+1
             WHERE photo_id=?2 AND photo_version=?3 AND lifecycle='Active'",
            (new_version, photo_id.as_bytes().as_slice(), old_version),
        )
    }
    .map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(DomainError::stale_version("Photo"));
    }
    Ok(())
}

fn relation_event(
    transaction: &Transaction<'_>,
    kind: &str,
    photo_id: PhotoId,
    related_photo_id: Option<PhotoId>,
    asset_id: Option<AssetId>,
    before: i64,
    after: i64,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "INSERT INTO photo_relation_event(
                relation_event_id, event_kind, photo_id, related_photo_id,
                asset_id, before_version, after_version, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                uuid::Uuid::new_v4().as_bytes().as_slice(),
                kind,
                photo_id.as_bytes().as_slice(),
                related_photo_id.map(|id| id.as_bytes().to_vec()),
                asset_id.map(|id| id.as_bytes().to_vec()),
                before,
                after,
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn next(value: i64) -> Result<i64, DomainError> {
    value
        .checked_add(1)
        .ok_or_else(|| DomainError::new(ErrorCode::Internal, "Photo 版本已耗尽。"))
}

fn catalog_invariant() -> DomainError {
    DomainError::new(ErrorCode::DatabaseCorrupt, "Catalog 配对状态不一致。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_conflicting_metadata_never_pairs() {
        let base = |kind: &str, capture: Option<&str>| PresentAsset {
            asset_id: AssetId::new(),
            asset_version: 1,
            photo_id: PhotoId::new(),
            photo_version: 1,
            role: kind.into(),
            stem_key: "dscf1".into(),
            capture: capture.map(str::to_string),
            model: None,
            serial: None,
        };
        assert!(desired_pairs(&[base("Jpg", None), base("Raf", None)]).is_empty());
        assert!(
            desired_pairs(&[
                base("Jpg", Some("2026-01-01T00:00:00")),
                base("Raf", Some("2026-01-02T00:00:00"))
            ])
            .is_empty()
        );
    }

    #[test]
    fn unique_exact_time_pair_is_conservative_positive_case() {
        let capture = Some("2026-01-01T00:00:00".into());
        let jpg = PresentAsset {
            asset_id: AssetId::new(),
            asset_version: 1,
            photo_id: PhotoId::new(),
            photo_version: 1,
            role: "Jpg".into(),
            stem_key: "dscf1".into(),
            capture: capture.clone(),
            model: Some("X-T5".into()),
            serial: None,
        };
        let raf = PresentAsset {
            asset_id: AssetId::new(),
            asset_version: 1,
            photo_id: PhotoId::new(),
            photo_version: 1,
            role: "Raf".into(),
            stem_key: "dscf1".into(),
            capture,
            model: Some("X-T5".into()),
            serial: None,
        };
        assert!(desired_pairs(&[jpg, raf]).len() == 1);
    }
}
