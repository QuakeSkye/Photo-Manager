use std::sync::Arc;

use rusqlite::OptionalExtension;

use crate::{
    db::Database,
    domain::{AssetId, DomainError, ErrorCode, LibraryId, PhotoId, RootBindingId, RootId},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Jpg,
    Raf,
}

impl MediaKind {
    const fn as_sql(self) -> &'static str {
        match self {
            Self::Jpg => "Jpg",
            Self::Raf => "Raf",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetObservation {
    pub library_id: LibraryId,
    pub root_id: RootId,
    pub root_binding_id: RootBindingId,
    pub media_kind: MediaKind,
    pub relative_path: String,
    pub path_key: Vec<u8>,
    pub file_id: Option<Vec<u8>>,
    pub size_bytes: i64,
    pub mtime_filetime: i64,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserveAssetResult {
    pub asset_id: AssetId,
    pub asset_version: i64,
    pub photo_id: PhotoId,
    pub photo_version: i64,
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct CatalogService {
    database: Arc<Database>,
}

impl CatalogService {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub fn observe_asset(
        &self,
        observation: AssetObservation,
    ) -> Result<ObserveAssetResult, DomainError> {
        let writer = self.database.writer()?;
        writer.transaction(move |transaction| {
            let by_path = transaction
                .query_row(
                    "SELECT asset_id FROM asset
                     WHERE root_id=?1 AND current_path_key=?2 AND presence_state!='Tombstoned'",
                    (
                        observation.root_id.as_bytes().as_slice(),
                        &observation.path_key,
                    ),
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(crate::db::migration::map_sqlite_error)?;
            let by_file = match &observation.file_id {
                Some(file_id) => transaction
                    .query_row(
                        "SELECT asset_id FROM asset
                         WHERE root_id=?1 AND current_file_id=?2 AND presence_state!='Tombstoned'",
                        (observation.root_id.as_bytes().as_slice(), file_id),
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(crate::db::migration::map_sqlite_error)?,
                None => None,
            };

            if by_path.is_some() && by_file.is_some() && by_path != by_file {
                return Err(DomainError::new(
                    ErrorCode::ReservationConflict,
                    "同一路径与文件身份指向不同对象，已停止建档。",
                ));
            }
            let candidate = by_file.or(by_path);

            match candidate {
                Some(id_bytes) => update_existing(transaction, &observation, id_bytes),
                None => insert_new(transaction, &observation),
            }
        })
    }
}

fn update_existing(
    transaction: &rusqlite::Transaction<'_>,
    observation: &AssetObservation,
    id_bytes: Vec<u8>,
) -> Result<ObserveAssetResult, DomainError> {
    let (current_version, row_version, path_key, file_id, size, mtime): (
        i64,
        i64,
        Vec<u8>,
        Option<Vec<u8>>,
        i64,
        i64,
    ) = transaction
        .query_row(
            "SELECT a.current_version, a.row_version, r.path_key, r.file_id,
                    r.size_bytes, r.mtime_filetime
             FROM asset a
             JOIN asset_revision r
               ON r.asset_id=a.asset_id AND r.asset_version=a.current_version
             WHERE a.asset_id=?1",
            [&id_bytes],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    let asset_id = AssetId::from_uuid(
        uuid::Uuid::from_slice(&id_bytes)
            .map_err(|_| DomainError::new(ErrorCode::Internal, "数据库对象标识损坏。"))?,
    );
    let (photo_bytes, photo_version, photo_row_version): (Vec<u8>, i64, i64) = transaction
        .query_row(
            "SELECT p.photo_id, p.photo_version, p.row_version
             FROM photo p
             JOIN photo_asset pa ON pa.photo_id=p.photo_id
             WHERE pa.asset_id=?1 AND pa.to_photo_version IS NULL",
            [&id_bytes],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    let photo_id = PhotoId::from_uuid(
        uuid::Uuid::from_slice(&photo_bytes)
            .map_err(|_| DomainError::new(ErrorCode::Internal, "数据库对象标识损坏。"))?,
    );
    let unchanged = path_key == observation.path_key
        && file_id == observation.file_id
        && size == observation.size_bytes
        && mtime == observation.mtime_filetime;
    if unchanged {
        return Ok(ObserveAssetResult {
            asset_id,
            asset_version: current_version,
            photo_id,
            photo_version,
            changed: false,
        });
    }

    let next_asset_version = current_version
        .checked_add(1)
        .ok_or_else(|| DomainError::new(ErrorCode::Internal, "Asset 版本已耗尽。"))?;
    let next_photo_version = photo_version
        .checked_add(1)
        .ok_or_else(|| DomainError::new(ErrorCode::Internal, "Photo 版本已耗尽。"))?;
    transaction
        .execute(
            "INSERT INTO asset_revision(
                asset_id, asset_version, root_id, root_binding_id, relative_path,
                path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                presence_state, hard_link_count, observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, 'Present', 1, ?10)",
            rusqlite::params![
                &id_bytes,
                next_asset_version,
                observation.root_id.as_bytes().as_slice(),
                observation.root_binding_id.as_bytes().as_slice(),
                &observation.relative_path,
                &observation.path_key,
                &observation.file_id,
                observation.size_bytes,
                observation.mtime_filetime,
                observation.observed_at,
            ],
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    let updated_asset = transaction
        .execute(
            "UPDATE asset
             SET current_version=?1, current_path_key=?2, current_file_id=?3,
                 row_version=row_version+1
             WHERE asset_id=?4 AND current_version=?5 AND row_version=?6",
            rusqlite::params![
                next_asset_version,
                &observation.path_key,
                &observation.file_id,
                &id_bytes,
                current_version,
                row_version,
            ],
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    if updated_asset != 1 {
        return Err(DomainError::stale_version("Asset"));
    }
    let updated_photo = transaction
        .execute(
            "UPDATE photo
             SET photo_version=?1, row_version=row_version+1
             WHERE photo_id=?2 AND photo_version=?3 AND row_version=?4",
            (
                next_photo_version,
                &photo_bytes,
                photo_version,
                photo_row_version,
            ),
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    if updated_photo != 1 {
        return Err(DomainError::stale_version("Photo"));
    }

    Ok(ObserveAssetResult {
        asset_id,
        asset_version: next_asset_version,
        photo_id,
        photo_version: next_photo_version,
        changed: true,
    })
}

fn insert_new(
    transaction: &rusqlite::Transaction<'_>,
    observation: &AssetObservation,
) -> Result<ObserveAssetResult, DomainError> {
    let asset_id = AssetId::new();
    let photo_id = PhotoId::new();
    transaction
        .execute(
            "INSERT INTO asset(
                asset_id, library_id, root_id, media_kind, current_version,
                current_path_key, current_file_id, presence_state,
                deletion_supported, created_at
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, 'Present', 1, ?7)",
            rusqlite::params![
                asset_id.as_bytes().as_slice(),
                observation.library_id.as_bytes().as_slice(),
                observation.root_id.as_bytes().as_slice(),
                observation.media_kind.as_sql(),
                &observation.path_key,
                &observation.file_id,
                observation.observed_at,
            ],
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO asset_revision(
                asset_id, asset_version, root_id, root_binding_id, relative_path,
                path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                presence_state, hard_link_count, observed_at
             ) VALUES (?1, 1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, 'Present', 1, ?9)",
            rusqlite::params![
                asset_id.as_bytes().as_slice(),
                observation.root_id.as_bytes().as_slice(),
                observation.root_binding_id.as_bytes().as_slice(),
                &observation.relative_path,
                &observation.path_key,
                &observation.file_id,
                observation.size_bytes,
                observation.mtime_filetime,
                observation.observed_at,
            ],
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO photo(
                photo_id, library_id, photo_version, lifecycle, date_class,
                capture_local_key, natural_name_display, natural_sort_key,
                sort_key_version, created_at
             ) VALUES (?1, ?2, 1, 'Active', 'Unknown', NULL, ?3, ?4, 1, ?5)",
            (
                photo_id.as_bytes().as_slice(),
                observation.library_id.as_bytes().as_slice(),
                &observation.relative_path,
                &observation.path_key,
                observation.observed_at,
            ),
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO photo_asset(
                photo_asset_id, photo_id, asset_id, role, from_photo_version
             ) VALUES (?1, ?2, ?3, ?4, 1)",
            (
                uuid::Uuid::new_v4().as_bytes().as_slice(),
                photo_id.as_bytes().as_slice(),
                asset_id.as_bytes().as_slice(),
                observation.media_kind.as_sql(),
            ),
        )
        .map_err(crate::db::migration::map_sqlite_error)?;

    Ok(ObserveAssetResult {
        asset_id,
        asset_version: 1,
        photo_id,
        photo_version: 1,
        changed: true,
    })
}
