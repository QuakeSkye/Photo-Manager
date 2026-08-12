use std::sync::Arc;

use rusqlite::{OptionalExtension, Transaction};

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{AssetId, DomainError, LibraryId, RootBindingId, RootId},
};

use super::{CatalogObservation, asset_id_from, binding_id_from};

pub const CANDIDATE_KEY_FORMAT_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CatalogRevisionAssetChange {
    pub asset_id: Vec<u8>,
    pub before_asset_version: Option<i64>,
    pub after_asset_version: i64,
    pub before_presence: Option<String>,
    pub after_presence: String,
    pub change_kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateLookupInput {
    pub library_id: LibraryId,
    pub root_id: RootId,
    pub original_filename: String,
    pub size_bytes: i64,
    pub capture_local_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCandidate {
    pub asset_id: AssetId,
    pub asset_version: i64,
    pub root_id: RootId,
    pub root_binding_id: RootBindingId,
    pub relative_path: String,
    pub path_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CandidateLookup {
    database: Arc<Database>,
}

impl CandidateLookup {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub fn find(&self, input: CandidateLookupInput) -> Result<Vec<ImportCandidate>, DomainError> {
        find_candidates(&self.database, input)
    }

    pub fn catalog_revision(&self, library_id: LibraryId) -> Result<i64, DomainError> {
        self.database
            .open_read_connection()?
            .query_row(
                "SELECT catalog_revision FROM library_catalog_revision WHERE library_id=?1",
                [library_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)
    }
}

pub(crate) fn find_candidates(
    database: &Database,
    input: CandidateLookupInput,
) -> Result<Vec<ImportCandidate>, DomainError> {
    let (basename, extension) = candidate_name_parts(&input.original_filename);
    let connection = database.open_read_connection()?;
    let mut statement = connection
        .prepare(
            "SELECT key.asset_id, key.asset_version, key.root_id,
                        key.root_binding_id, key.relative_path, key.path_key
                 FROM asset_import_candidate_key AS key
                 JOIN asset ON asset.asset_id=key.asset_id
                 JOIN asset_revision AS revision
                   ON revision.asset_id=asset.asset_id
                  AND revision.asset_version=asset.current_version
                 WHERE key.library_id=?1 AND key.root_id=?2
                   AND key.basename_folded=?3 AND key.extension_folded=?4
                   AND key.size_bytes=?5 AND key.key_format_version=?6
                   AND key.asset_version=asset.current_version
                   AND asset.presence_state='Present'
                   AND revision.presence_state='Present'
                   AND (key.capture_local_key IS NULL OR ?7 IS NULL OR key.capture_local_key=?7)
                 ORDER BY key.path_key, key.asset_id",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(
            rusqlite::params![
                input.library_id.as_bytes().as_slice(),
                input.root_id.as_bytes().as_slice(),
                basename,
                extension,
                input.size_bytes,
                CANDIDATE_KEY_FORMAT_VERSION,
                input.capture_local_key,
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                ))
            },
        )
        .map_err(map_sqlite_error)?;
    rows.map(|row| {
        let (asset, version, root, binding, relative_path, path_key) =
            row.map_err(map_sqlite_error)?;
        Ok(ImportCandidate {
            asset_id: asset_id_from(&asset)?,
            asset_version: version,
            root_id: RootId::from_uuid(super::uuid_from(&root)?),
            root_binding_id: binding_id_from(&binding)?,
            relative_path,
            path_key,
        })
    })
    .collect()
}

pub(super) fn ensure_candidate_key(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
    asset_version: i64,
    observation: &CatalogObservation,
) -> Result<(), DomainError> {
    let (basename, extension) = candidate_name_parts(&observation.asset.relative_path);
    transaction
        .execute(
            "INSERT OR IGNORE INTO asset_import_candidate_key(
                asset_id, asset_version, key_format_version, library_id, root_id,
                root_binding_id, basename_folded, extension_folded, size_bytes,
                capture_local_key, relative_path, path_key, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                asset_id,
                asset_version,
                CANDIDATE_KEY_FORMAT_VERSION,
                observation.asset.library_id.as_bytes().as_slice(),
                observation.asset.root_id.as_bytes().as_slice(),
                observation.asset.root_binding_id.as_bytes().as_slice(),
                basename,
                extension,
                observation.asset.size_bytes,
                &observation.capture_local_key,
                &observation.asset.relative_path,
                &observation.asset.path_key,
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

pub(super) fn bump_catalog_revision(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
    binding_id: RootBindingId,
    cause_kind: &'static str,
    scan_run_id: Option<&[u8]>,
    recycle_batch_id: Option<&[u8]>,
    changes: &[CatalogRevisionAssetChange],
) -> Result<Vec<u8>, DomainError> {
    let current: Option<i64> = transaction
        .query_row(
            "SELECT catalog_revision FROM library_catalog_revision WHERE library_id=?1",
            [library_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let from_revision = current.unwrap_or(0);
    let to_revision = from_revision.checked_add(1).ok_or_else(|| {
        DomainError::new(
            crate::domain::ErrorCode::InvalidTransition,
            "图库目录修订号已达到上限。",
        )
    })?;
    transaction
        .execute(
            "INSERT INTO library_catalog_revision(
                library_id, catalog_revision, root_binding_id, updated_at
             ) VALUES (?1, 1, ?2, ?3)
             ON CONFLICT(library_id) DO UPDATE SET
                catalog_revision=?4,
                root_binding_id=excluded.root_binding_id,
                updated_at=excluded.updated_at",
            rusqlite::params![
                library_id.as_bytes().as_slice(),
                binding_id.as_bytes().as_slice(),
                now_micros(),
                to_revision,
            ],
        )
        .map_err(map_sqlite_error)?;
    let event_id = uuid::Uuid::new_v4().as_bytes().to_vec();
    transaction
        .execute(
            "INSERT INTO catalog_revision_event(
                event_id,library_id,root_binding_id,from_revision,to_revision,cause_kind,
                scan_run_id,recycle_batch_id,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                &event_id,
                library_id.as_bytes().as_slice(),
                binding_id.as_bytes().as_slice(),
                from_revision,
                to_revision,
                cause_kind,
                scan_run_id,
                recycle_batch_id,
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    for change in changes {
        transaction
            .execute(
                "INSERT INTO catalog_revision_asset_change(
                    event_id,asset_id,before_asset_version,after_asset_version,
                    before_presence,after_presence,change_kind
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![
                    &event_id,
                    &change.asset_id,
                    change.before_asset_version,
                    change.after_asset_version,
                    &change.before_presence,
                    &change.after_presence,
                    change.change_kind,
                ],
            )
            .map_err(map_sqlite_error)?;
    }
    Ok(event_id)
}

pub(super) fn ensure_catalog_revision(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
    binding_id: RootBindingId,
) -> Result<(), DomainError> {
    let current: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT root_binding_id FROM library_catalog_revision WHERE library_id=?1",
            [library_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    match current {
        None => {
            transaction
                .execute(
                    "INSERT INTO library_catalog_revision(
                        library_id,catalog_revision,root_binding_id,updated_at
                     ) VALUES(?1,0,?2,?3)",
                    (
                        library_id.as_bytes().as_slice(),
                        binding_id.as_bytes().as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
        }
        Some(current_binding) if current_binding != binding_id.as_bytes().as_slice() => {
            bump_catalog_revision(
                transaction,
                library_id,
                binding_id,
                "RootBindingChanged",
                None,
                None,
                &[],
            )?;
        }
        Some(_) => {}
    }
    Ok(())
}

fn candidate_name_parts(relative_path: &str) -> (String, String) {
    let filename = relative_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(relative_path);
    match filename.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem.to_lowercase(), extension.to_lowercase())
        }
        _ => (filename.to_lowercase(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::candidate_name_parts;

    #[test]
    fn candidate_names_fold_only_the_final_path_component() {
        assert_eq!(
            candidate_name_parts("2026\\20260810\\DSCF1234.JpG"),
            ("dscf1234".into(), "jpg".into())
        );
    }
}
