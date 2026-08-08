use std::cmp::Ordering;

use rusqlite::OptionalExtension;

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode, LibraryId, PhotoId},
};

use super::photo_id_from;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    CaptureDesc,
    CaptureAsc,
}

impl SortMode {
    const fn as_sql(self) -> &'static str {
        match self {
            Self::CaptureDesc => "CaptureDesc",
            Self::CaptureAsc => "CaptureAsc",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelinePhoto {
    pub photo_id: PhotoId,
    pub photo_version: i64,
    pub capture_local_key: Option<String>,
    pub natural_name_display: String,
    pub natural_sort_key: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewingCursor {
    pub library_id: LibraryId,
    pub sort_mode: SortMode,
    pub anchor: TimelinePhoto,
    pub updated_at: i64,
}

pub fn load_timeline(
    database: &Database,
    library_id: LibraryId,
    sort_mode: SortMode,
) -> Result<Vec<TimelinePhoto>, DomainError> {
    let connection = database.open_read_connection()?;
    let direction = match sort_mode {
        SortMode::CaptureDesc => "DESC",
        SortMode::CaptureAsc => "ASC",
    };
    let sql = format!(
        "SELECT photo.photo_id, photo.photo_version, photo.capture_local_key,
                photo.natural_name_display, photo.natural_sort_key
         FROM photo
         JOIN photo_asset relation
           ON relation.photo_id=photo.photo_id AND relation.to_photo_version IS NULL
         JOIN asset ON asset.asset_id=relation.asset_id AND asset.presence_state='Present'
         JOIN root ON root.root_id=asset.root_id AND root.lifecycle='Online'
         WHERE photo.library_id=?1 AND photo.lifecycle='Active'
         GROUP BY photo.photo_id
         ORDER BY photo.date_class ASC,
                  photo.capture_local_key {direction}, photo.natural_sort_key ASC,
                  photo.photo_id ASC"
    );
    let mut statement = connection.prepare(&sql).map_err(map_sqlite_error)?;
    statement
        .query_map([library_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(map_sqlite_error)?
        .map(|row| {
            let (id, version, capture, display, key) = row.map_err(map_sqlite_error)?;
            Ok(TimelinePhoto {
                photo_id: photo_id_from(&id)?,
                photo_version: version,
                capture_local_key: capture,
                natural_name_display: display,
                natural_sort_key: key,
            })
        })
        .collect()
}

pub fn save_cursor(
    database: &Database,
    library_id: LibraryId,
    sort_mode: SortMode,
    photo_id: PhotoId,
) -> Result<ViewingCursor, DomainError> {
    let writer = database.writer()?;
    writer.transaction(move |transaction| {
        let anchor = transaction
            .query_row(
                "SELECT photo_version, capture_local_key, natural_name_display, natural_sort_key
                 FROM photo WHERE photo_id=?1 AND library_id=?2 AND lifecycle='Active'",
                (
                    photo_id.as_bytes().as_slice(),
                    library_id.as_bytes().as_slice(),
                ),
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or_else(|| {
                DomainError::new(ErrorCode::StaleVersion, "当前照片不再可用于保存浏览位置。")
            })?;
        let now = now_micros();
        transaction
            .execute(
                "INSERT INTO viewing_cursor(
                    library_id, sort_mode, photo_id, photo_version, date_class,
                    capture_local_key, natural_name_display, natural_sort_key,
                    anchor_photo_id, sort_key_version, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?3, 1, ?9)
                 ON CONFLICT(library_id, sort_mode) DO UPDATE SET
                    photo_id=excluded.photo_id, photo_version=excluded.photo_version,
                    date_class=excluded.date_class, capture_local_key=excluded.capture_local_key,
                    natural_name_display=excluded.natural_name_display,
                    natural_sort_key=excluded.natural_sort_key,
                    anchor_photo_id=excluded.anchor_photo_id,
                    sort_key_version=excluded.sort_key_version, updated_at=excluded.updated_at",
                rusqlite::params![
                    library_id.as_bytes().as_slice(),
                    sort_mode.as_sql(),
                    photo_id.as_bytes().as_slice(),
                    anchor.0,
                    if anchor.1.is_some() {
                        "Known"
                    } else {
                        "Unknown"
                    },
                    anchor.1,
                    anchor.2,
                    anchor.3,
                    now,
                ],
            )
            .map_err(map_sqlite_error)?;
        Ok(ViewingCursor {
            library_id,
            sort_mode,
            anchor: TimelinePhoto {
                photo_id,
                photo_version: anchor.0,
                capture_local_key: anchor.1,
                natural_name_display: anchor.2,
                natural_sort_key: anchor.3,
            },
            updated_at: now,
        })
    })
}

pub fn restore_cursor(
    database: &Database,
    library_id: LibraryId,
    sort_mode: SortMode,
) -> Result<Option<PhotoId>, DomainError> {
    let connection = database.open_read_connection()?;
    let cursor = connection
        .query_row(
            "SELECT cursor.photo_id, cursor.photo_version, cursor.capture_local_key,
                    cursor.natural_name_display, cursor.natural_sort_key, cursor.updated_at
             FROM viewing_cursor cursor WHERE cursor.library_id=?1 AND cursor.sort_mode=?2",
            (library_id.as_bytes().as_slice(), sort_mode.as_sql()),
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?;
    drop(connection);
    let Some((id, version, capture, display, key, _updated)) = cursor else {
        return Ok(None);
    };
    let original = photo_id_from(&id)?;
    let timeline = load_timeline(database, library_id, sort_mode)?;
    if timeline.iter().any(|photo| photo.photo_id == original) {
        return Ok(Some(original));
    }
    let connection = database.open_read_connection()?;
    let alias = connection
        .query_row(
            "SELECT canonical_photo_id FROM photo_alias WHERE old_photo_id=?1",
            [original.as_bytes().as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map(|bytes| photo_id_from(&bytes))
        .transpose()?;
    if let Some(alias) = alias.filter(|alias| timeline.iter().any(|photo| photo.photo_id == *alias))
    {
        return Ok(Some(alias));
    }
    let anchor = TimelinePhoto {
        photo_id: original,
        photo_version: version,
        capture_local_key: capture,
        natural_name_display: display,
        natural_sort_key: key,
    };
    let next = timeline
        .iter()
        .find(|photo| compare(photo, &anchor, sort_mode) == Ordering::Greater);
    Ok(next.or_else(|| timeline.last()).map(|photo| photo.photo_id))
}

fn compare(left: &TimelinePhoto, right: &TimelinePhoto, mode: SortMode) -> Ordering {
    match (&left.capture_local_key, &right.capture_local_key) {
        (Some(left_time), Some(right_time)) => {
            let time = match mode {
                SortMode::CaptureAsc => left_time.cmp(right_time),
                SortMode::CaptureDesc => right_time.cmp(left_time),
            };
            time.then_with(|| left.natural_sort_key.cmp(&right.natural_sort_key))
                .then_with(|| left.photo_id.cmp(&right.photo_id))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left
            .natural_sort_key
            .cmp(&right.natural_sort_key)
            .then_with(|| left.photo_id.cmp(&right.photo_id)),
    }
}
