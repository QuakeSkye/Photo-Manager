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
    let mut present = load_present_assets(transaction, library_id)?;
    let desired = desired_pairs(&present);
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
    let desired = desired_pairs(&present);
    let by_id = present
        .iter()
        .map(|asset| (asset.asset_id, asset))
        .collect::<HashMap<_, _>>();
    for (jpg_id, raf_id) in desired {
        let jpg = by_id.get(&jpg_id).ok_or_else(catalog_invariant)?;
        let raf = by_id.get(&raf_id).ok_or_else(catalog_invariant)?;
        let evidence_id = ensure_pair_evidence(transaction, library_id, jpg, raf)?;
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
) -> Result<Vec<u8>, DomainError> {
    if let Some(id) = transaction
        .query_row(
            "SELECT pair_evidence_id FROM pair_evidence
             WHERE evidence_kind='HistoricalExact' AND jpg_asset_id=?1 AND jpg_asset_version=?2
               AND raf_asset_id=?3 AND raf_asset_version=?4",
            (
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
             ) VALUES (?1, 'HistoricalExact', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                &id,
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
