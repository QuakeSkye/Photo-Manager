use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    catalog::MediaKind,
    db::{Database, migration::map_sqlite_error},
    domain::{AssetId, DomainError, ErrorCode},
    media::{PreviewState, read_preview_bytes},
    roots::{open_read_only, probe_directory, probe_file, resolve_contained_path},
};

use super::{CacheEntryHandle, CacheKind, CacheService};

#[derive(Debug, Clone)]
pub struct PreviewService {
    database: Arc<Database>,
    cache: Arc<CacheService>,
}

impl PreviewService {
    pub fn new(database: Arc<Database>, cache: Arc<CacheService>) -> Self {
        Self { database, cache }
    }

    pub fn materialize(
        &self,
        asset_id: AssetId,
        asset_version: i64,
        cache_kind: CacheKind,
        render_key: &[u8],
    ) -> Result<CacheEntryHandle, DomainError> {
        let source = self.load_source(asset_id, asset_version)?;
        let root = probe_directory(&source.root_path)?;
        if root.directory_file_id != source.root_file_id
            || root.volume_identity_key != source.volume_key
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "生成预览前图库 Root 身份已变化。",
            ));
        }
        let source_path =
            resolve_contained_path(&source.root_path, Path::new(&source.relative_path))?;
        let mut file = open_read_only(&source_path)?;
        let identity = probe_file(&file)?;
        if identity.file_id != source.file_id
            || i64::try_from(identity.size_bytes).unwrap_or(i64::MAX) != source.size_bytes
            || identity.mtime_filetime != source.mtime_filetime
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "生成预览前照片身份已变化，请重新扫描。",
            ));
        }
        let bytes = read_preview_bytes(&mut file, source.preview)?;
        drop(file);
        self.cache
            .get_or_create(asset_id, asset_version, cache_kind, render_key, &bytes)
    }

    fn load_source(
        &self,
        asset_id: AssetId,
        asset_version: i64,
    ) -> Result<PreviewSource, DomainError> {
        let connection = self.database.open_read_connection()?;
        connection
            .query_row(
                "SELECT asset.media_kind, binding.absolute_path_display,
                        binding.root_directory_file_id, volume.strong_identity_key,
                        revision.relative_path, revision.file_id, revision.size_bytes,
                        revision.mtime_filetime, metadata.preview_state,
                        metadata.preview_offset, metadata.preview_length
                 FROM asset
                 JOIN asset_revision revision
                   ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
                 JOIN root ON root.root_id=asset.root_id
                 JOIN root_binding binding ON binding.binding_id=root.current_binding_id
                 JOIN volume_identity volume ON volume.volume_id=binding.volume_id
                 JOIN asset_media_metadata metadata
                   ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
                 WHERE asset.asset_id=?1 AND asset.current_version=?2
                   AND asset.presence_state='Present' AND root.lifecycle='Online'
                   AND binding.binding_state='Active'",
                (asset_id.as_bytes().as_slice(), asset_version),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?, row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?, row.get::<_, Option<Vec<u8>>>(5)?,
                        row.get::<_, i64>(6)?, row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?, row.get::<_, Option<i64>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                    ))
                },
            )
            .map_err(map_sqlite_error)
            .and_then(|row| {
                let media_kind = match row.0.as_str() {
                    "Jpg" => MediaKind::Jpg,
                    "Raf" => MediaKind::Raf,
                    _ => return Err(DomainError::new(ErrorCode::DatabaseCorrupt, "照片媒体类型损坏。")),
                };
                let preview = match (row.8.as_str(), row.9, row.10) {
                    ("JpegSource", None, None) if media_kind == MediaKind::Jpg => PreviewState::JpegSource,
                    ("RafEmbeddedJpeg", Some(offset), Some(length))
                        if media_kind == MediaKind::Raf && offset >= 0 && length > 0 =>
                    {
                        PreviewState::RafEmbeddedJpeg { offset: offset as u64, length: length as u64 }
                    }
                    ("Unavailable", None, None) => PreviewState::Unavailable,
                    _ => return Err(DomainError::new(ErrorCode::DatabaseCorrupt, "预览元数据状态损坏。")),
                };
                Ok(PreviewSource {
                    root_path: PathBuf::from(row.1), root_file_id: row.2, volume_key: row.3,
                    relative_path: row.4, file_id: row.5, size_bytes: row.6,
                    mtime_filetime: row.7, preview,
                })
            })
    }
}

#[derive(Debug)]
struct PreviewSource {
    root_path: PathBuf,
    root_file_id: Option<Vec<u8>>,
    volume_key: Vec<u8>,
    relative_path: String,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
    preview: PreviewState,
}
