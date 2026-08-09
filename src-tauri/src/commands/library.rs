use std::{collections::HashMap, path::Path, str::FromStr, sync::Arc};

use base64::Engine;
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::{
    cache::PreviewService,
    catalog::{ScanCoordinator, ScanKind, ScanStatus, SortMode},
    db::{Database, migration::map_sqlite_error},
    domain::{AssetId, DomainError, ErrorCode, LibraryId, PhotoId},
    media::MEDIA_PARSER_VERSION,
    pending_delete::{MarkReviewState, PendingDeleteService, PendingDeleteSummary},
    roots::{FujiLibraryCandidate, FujiLibraryService},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySnapshot {
    binding: Option<LibraryBindingView>,
    photos: Vec<PhotoSummary>,
    restored_photo_id: Option<String>,
    scan: Option<ScanStatus>,
    pending_review_count: usize,
    pending_delete_summary: PendingDeleteSummary,
    reports: Vec<ScanReportSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryBindingView {
    display_name: String,
    display_path: String,
    status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoSummary {
    photo_id: String,
    photo_version: i64,
    file_name: String,
    capture_local_key: Option<String>,
    preview: Option<PreviewReference>,
    deletion_state: Option<MarkReviewState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewReference {
    asset_id: String,
    asset_version: i64,
    media_kind: String,
    state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanReportSummary {
    started_at: i64,
    completed_at: Option<i64>,
    result: Option<String>,
    item_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDetail {
    photo_id: String,
    photo_version: i64,
    file_name: String,
    capture_local_key: Option<String>,
    file_time_ms: Option<i64>,
    camera_model: Option<String>,
    camera_serial: Option<String>,
    lens_model: Option<String>,
    f_number: Option<RationalView>,
    exposure_time: Option<RationalView>,
    iso: Option<i64>,
    focal_length: Option<RationalView>,
    pixel_width: Option<i64>,
    pixel_height: Option<i64>,
    film_simulation: Option<FilmSimulationView>,
    jpeg: Option<AssetView>,
    raf: Option<AssetView>,
    total_size_bytes: i64,
    preview: Option<PreviewReference>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RationalView {
    numerator: i64,
    denominator: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct FilmSimulationView {
    key: String,
    code: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetView {
    asset_id: String,
    asset_version: i64,
    size_bytes: i64,
    file_time_ms: i64,
    preview_state: String,
}

#[derive(Debug)]
struct DetailAssetRow {
    asset_id: Vec<u8>,
    version: i64,
    kind: String,
    size: i64,
    mtime: i64,
    preview_state: String,
    camera_model: Option<String>,
    camera_serial: Option<String>,
    lens_model: Option<String>,
    f_number_numerator: Option<i64>,
    f_number_denominator: Option<i64>,
    exposure_time_numerator: Option<i64>,
    exposure_time_denominator: Option<i64>,
    iso: Option<i64>,
    focal_length_numerator: Option<i64>,
    focal_length_denominator: Option<i64>,
    pixel_width: Option<i64>,
    pixel_height: Option<i64>,
    film_simulation_key: Option<String>,
    film_simulation_code: Option<i64>,
}

#[derive(Debug, Default)]
struct MergedCaptureFields {
    lens_model: Option<String>,
    f_number: Option<RationalView>,
    exposure_time: Option<RationalView>,
    iso: Option<i64>,
    focal_length: Option<RationalView>,
    dimensions: Option<(i64, i64)>,
    film_simulation: Option<FilmSimulationView>,
}

impl MergedCaptureFields {
    fn merge(&mut self, row: &DetailAssetRow) {
        self.lens_model = self.lens_model.take().or_else(|| row.lens_model.clone());
        self.f_number = self
            .f_number
            .or_else(|| rational_view(row.f_number_numerator, row.f_number_denominator));
        self.exposure_time = self
            .exposure_time
            .or_else(|| rational_view(row.exposure_time_numerator, row.exposure_time_denominator));
        self.iso = self.iso.or(row.iso.filter(|value| *value > 0));
        self.focal_length = self
            .focal_length
            .or_else(|| rational_view(row.focal_length_numerator, row.focal_length_denominator));
        self.dimensions = self.dimensions.or_else(|| {
            let width = row.pixel_width.filter(|value| *value > 0)?;
            let height = row.pixel_height.filter(|value| *value > 0)?;
            Some((width, height))
        });
        self.film_simulation = self.film_simulation.take().or_else(|| {
            row.film_simulation_key
                .as_ref()
                .map(|key| FilmSimulationView {
                    key: key.clone(),
                    code: row.film_simulation_code,
                })
        });
    }
}

fn rational_view(numerator: Option<i64>, denominator: Option<i64>) -> Option<RationalView> {
    let numerator = numerator?;
    let denominator = denominator.filter(|value| *value != 0)?;
    Some(RationalView {
        numerator,
        denominator,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPayload {
    asset_id: String,
    asset_version: i64,
    data_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindResult {
    display_path: String,
    scan_kind: ScanKind,
}

#[tauri::command]
pub fn get_library_snapshot(
    sort_mode: String,
    database: tauri::State<'_, Arc<Database>>,
    catalog: tauri::State<'_, Arc<crate::catalog::CatalogService>>,
    scans: tauri::State<'_, ScanCoordinator>,
    pending_delete: tauri::State<'_, Arc<PendingDeleteService>>,
) -> Result<LibrarySnapshot, DomainError> {
    let sort_mode = parse_sort_mode(&sort_mode)?;
    let connection = database.open_read_connection()?;
    let binding = connection
        .query_row(
            "SELECT library.library_id, library.display_name, binding.absolute_path_display,
                    root.lifecycle
             FROM current_library
             JOIN library ON library.library_id=current_library.library_id
             JOIN root ON root.library_id=library.library_id AND root.root_kind='FujiLibrary'
             JOIN root_binding binding ON binding.binding_id=root.current_binding_id
             WHERE current_library.slot=1 AND library.lifecycle='Active'",
            (),
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?;

    let Some((library_bytes, display_name, display_path, root_status)) = binding else {
        let pending_delete_summary = PendingDeleteSummary::default();
        return Ok(LibrarySnapshot {
            binding: None,
            photos: Vec::new(),
            restored_photo_id: None,
            scan: scans.status()?,
            pending_review_count: 0,
            pending_delete_summary,
            reports: load_scan_reports(&connection)?,
        });
    };
    let library_id = id_from_bytes::<LibraryId>(&library_bytes)?;
    let pending_snapshot = pending_delete.snapshot(SortMode::CaptureDesc)?;
    let review_states = pending_snapshot
        .items
        .iter()
        .map(|item| {
            Ok((
                PhotoId::from_str(&item.photo_id)?,
                if item.problem.is_some() {
                    MarkReviewState::Problem
                } else {
                    MarkReviewState::Marked
                },
            ))
        })
        .collect::<Result<HashMap<_, _>, DomainError>>()?;
    let pending_delete_summary = pending_snapshot.summary;
    let timeline = catalog.timeline(library_id, sort_mode)?;
    let restored_photo_id = catalog
        .restore_viewing_cursor(library_id, sort_mode)?
        .map(|value| value.to_string());

    let mut preview_by_photo = HashMap::<Vec<u8>, PreviewReference>::new();
    let mut statement = connection
        .prepare(
            "SELECT relation.photo_id, asset.asset_id, asset.current_version,
                    asset.media_kind, metadata.preview_state
             FROM photo_asset relation
             JOIN asset ON asset.asset_id=relation.asset_id
             JOIN asset_media_metadata metadata
               ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
             WHERE relation.to_photo_version IS NULL AND asset.library_id=?1
               AND asset.presence_state='Present'
             ORDER BY CASE metadata.preview_state WHEN 'Unavailable' THEN 1 ELSE 0 END,
                      CASE asset.media_kind WHEN 'Jpg' THEN 0 ELSE 1 END",
        )
        .map_err(map_sqlite_error)?;
    for row in statement
        .query_map([library_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (photo, asset, version, media_kind, state) = row.map_err(map_sqlite_error)?;
        preview_by_photo.entry(photo).or_insert(PreviewReference {
            asset_id: id_from_bytes::<AssetId>(&asset)?.to_string(),
            asset_version: version,
            media_kind,
            state,
        });
    }

    let photos = timeline
        .into_iter()
        .map(|photo| PhotoSummary {
            photo_id: photo.photo_id.to_string(),
            photo_version: photo.photo_version,
            file_name: photo.natural_name_display,
            capture_local_key: photo.capture_local_key,
            preview: preview_by_photo.remove(photo.photo_id.as_bytes().as_slice()),
            deletion_state: review_states.get(&photo.photo_id).copied(),
        })
        .collect();
    Ok(LibrarySnapshot {
        binding: Some(LibraryBindingView {
            display_name,
            display_path,
            status: root_status,
        }),
        photos,
        restored_photo_id,
        scan: scans.status()?,
        pending_review_count: pending_delete_summary.total_count,
        pending_delete_summary,
        reports: load_scan_reports(&connection)?,
    })
}

#[tauri::command]
pub fn get_photo_detail(
    photo_id: String,
    photo_version: i64,
    database: tauri::State<'_, Arc<Database>>,
) -> Result<PhotoDetail, DomainError> {
    let photo_id = PhotoId::from_str(&photo_id)?;
    let connection = database.open_read_connection()?;
    let (file_name, capture_local_key): (String, Option<String>) = connection
        .query_row(
            "SELECT natural_name_display, capture_local_key FROM photo
             WHERE photo_id=?1 AND photo_version=?2 AND lifecycle='Active'",
            (photo_id.as_bytes().as_slice(), photo_version),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or_else(|| DomainError::stale_version("Photo"))?;

    let mut statement = connection
        .prepare(
            "SELECT asset.asset_id, asset.current_version, asset.media_kind,
                    revision.size_bytes, revision.mtime_filetime,
                    metadata.preview_state, metadata.camera_model, metadata.camera_serial,
                    capture.lens_model,
                    capture.f_number_numerator, capture.f_number_denominator,
                    capture.exposure_time_numerator, capture.exposure_time_denominator,
                    capture.iso_value,
                    capture.focal_length_numerator, capture.focal_length_denominator,
                    capture.pixel_width, capture.pixel_height,
                    capture.film_simulation_key, capture.film_simulation_code
             FROM photo_asset relation
             JOIN asset ON asset.asset_id=relation.asset_id
             JOIN asset_revision revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             JOIN asset_media_metadata metadata
               ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
             LEFT JOIN asset_capture_metadata capture
               ON capture.asset_id=asset.asset_id
              AND capture.asset_version=asset.current_version
              AND capture.parser_version=(
                  SELECT MAX(latest.parser_version)
                  FROM asset_capture_metadata latest
                  WHERE latest.asset_id=asset.asset_id
                    AND latest.asset_version=asset.current_version
                    AND latest.parser_version<=?2
              )
             WHERE relation.photo_id=?1 AND relation.to_photo_version IS NULL
               AND asset.presence_state='Present'
             ORDER BY CASE asset.media_kind WHEN 'Jpg' THEN 0 ELSE 1 END,
                      asset.asset_id",
        )
        .map_err(map_sqlite_error)?;
    let mut jpeg = None;
    let mut raf = None;
    let mut preview = None;
    let mut total_size_bytes = 0_i64;
    let mut file_time_ms = None;
    let mut camera_model = None;
    let mut camera_serial = None;
    let mut capture_fields = MergedCaptureFields::default();
    for row in statement
        .query_map(
            rusqlite::params![photo_id.as_bytes().as_slice(), MEDIA_PARSER_VERSION,],
            |row| {
                Ok(DetailAssetRow {
                    asset_id: row.get(0)?,
                    version: row.get(1)?,
                    kind: row.get(2)?,
                    size: row.get(3)?,
                    mtime: row.get(4)?,
                    preview_state: row.get(5)?,
                    camera_model: row.get(6)?,
                    camera_serial: row.get(7)?,
                    lens_model: row.get(8)?,
                    f_number_numerator: row.get(9)?,
                    f_number_denominator: row.get(10)?,
                    exposure_time_numerator: row.get(11)?,
                    exposure_time_denominator: row.get(12)?,
                    iso: row.get(13)?,
                    focal_length_numerator: row.get(14)?,
                    focal_length_denominator: row.get(15)?,
                    pixel_width: row.get(16)?,
                    pixel_height: row.get(17)?,
                    film_simulation_key: row.get(18)?,
                    film_simulation_code: row.get(19)?,
                })
            },
        )
        .map_err(map_sqlite_error)?
    {
        let row = row.map_err(map_sqlite_error)?;
        let asset_id = id_from_bytes::<AssetId>(&row.asset_id)?;
        let asset = AssetView {
            asset_id: asset_id.to_string(),
            asset_version: row.version,
            size_bytes: row.size,
            file_time_ms: file_time_to_unix_ms(row.mtime),
            preview_state: row.preview_state.clone(),
        };
        let asset_file_time_ms = asset.file_time_ms;
        let candidate_available = row.preview_state != "Unavailable";
        let candidate = PreviewReference {
            asset_id: asset_id.to_string(),
            asset_version: row.version,
            media_kind: row.kind.clone(),
            state: row.preview_state.clone(),
        };
        if row.kind == "Jpg" {
            if candidate_available || preview.is_none() {
                preview = Some(candidate);
            }
            jpeg = Some(asset);
        } else {
            if preview
                .as_ref()
                .is_none_or(|current| current.state == "Unavailable" && candidate_available)
            {
                preview = Some(candidate);
            }
            raf = Some(asset);
        }
        total_size_bytes = total_size_bytes.saturating_add(row.size);
        file_time_ms =
            Some(file_time_ms.map_or(asset_file_time_ms, |old: i64| old.max(asset_file_time_ms)));
        camera_model = camera_model.or(row.camera_model.clone());
        camera_serial = camera_serial.or(row.camera_serial.clone());
        capture_fields.merge(&row);
    }
    let (pixel_width, pixel_height) = capture_fields
        .dimensions
        .map_or((None, None), |(width, height)| (Some(width), Some(height)));
    Ok(PhotoDetail {
        photo_id: photo_id.to_string(),
        photo_version,
        file_name,
        capture_local_key,
        file_time_ms,
        camera_model,
        camera_serial,
        lens_model: capture_fields.lens_model,
        f_number: capture_fields.f_number,
        exposure_time: capture_fields.exposure_time,
        iso: capture_fields.iso,
        focal_length: capture_fields.focal_length,
        pixel_width,
        pixel_height,
        film_simulation: capture_fields.film_simulation,
        jpeg,
        raf,
        total_size_bytes,
        preview,
    })
}

#[tauri::command]
pub fn get_photo_preview(
    asset_id: String,
    asset_version: i64,
    previews: tauri::State<'_, Arc<PreviewService>>,
) -> Result<PreviewPayload, DomainError> {
    let asset_id = AssetId::from_str(&asset_id)?;
    let bytes = previews.read_controlled(asset_id, asset_version)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(PreviewPayload {
        asset_id: asset_id.to_string(),
        asset_version,
        data_url: format!("data:image/jpeg;base64,{encoded}"),
    })
}

#[tauri::command]
pub fn save_viewing_cursor(
    sort_mode: String,
    photo_id: String,
    catalog: tauri::State<'_, Arc<crate::catalog::CatalogService>>,
    database: tauri::State<'_, Arc<Database>>,
) -> Result<(), DomainError> {
    let sort_mode = parse_sort_mode(&sort_mode)?;
    let photo_id = PhotoId::from_str(&photo_id)?;
    let connection = database.open_read_connection()?;
    let library = connection
        .query_row(
            "SELECT library_id FROM current_library WHERE slot=1",
            (),
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "尚未绑定 Fuji 图库。"))?;
    catalog.save_viewing_cursor(id_from_bytes::<LibraryId>(&library)?, sort_mode, photo_id)?;
    Ok(())
}

#[tauri::command]
pub fn validate_library_folder(
    path: String,
    libraries: tauri::State<'_, Arc<FujiLibraryService>>,
) -> Result<FujiLibraryCandidate, DomainError> {
    libraries.inspect_candidate(Path::new(&path))
}

#[tauri::command]
pub fn bind_library_folder(
    path: String,
    libraries: tauri::State<'_, Arc<FujiLibraryService>>,
    scans: tauri::State<'_, ScanCoordinator>,
) -> Result<BindResult, DomainError> {
    if scans.is_active() {
        return Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "请先完成或取消当前图库扫描。",
        ));
    }
    let binding = libraries.bind(Path::new(&path))?;
    let scan_kind = if binding.created_new_library {
        ScanKind::Full
    } else {
        ScanKind::Incremental
    };
    let _background = scans.start_current(scan_kind)?;
    Ok(BindResult {
        display_path: path,
        scan_kind,
    })
}

#[tauri::command]
pub fn get_scan_status(
    scans: tauri::State<'_, ScanCoordinator>,
) -> Result<Option<ScanStatus>, DomainError> {
    scans.status()
}

#[tauri::command]
pub fn cancel_library_scan(scans: tauri::State<'_, ScanCoordinator>) -> Result<(), DomainError> {
    scans.cancel()
}

#[tauri::command]
pub fn retry_library_scan(scans: tauri::State<'_, ScanCoordinator>) -> Result<(), DomainError> {
    let kind = scans
        .status()?
        .map(|status| status.kind)
        .unwrap_or(ScanKind::Full);
    let _background = scans.start_current(kind)?;
    Ok(())
}

fn parse_sort_mode(value: &str) -> Result<SortMode, DomainError> {
    match value {
        "captureDesc" => Ok(SortMode::CaptureDesc),
        "captureAsc" => Ok(SortMode::CaptureAsc),
        _ => Err(DomainError::new(ErrorCode::InvalidId, "排序方式无效。")),
    }
}

fn load_scan_reports(
    connection: &rusqlite::Connection,
) -> Result<Vec<ScanReportSummary>, DomainError> {
    let mut statement = connection
        .prepare(
            "SELECT started_at, completed_at, result, item_count
             FROM operation_report WHERE report_kind='Scan'
             ORDER BY started_at DESC LIMIT 50",
        )
        .map_err(map_sqlite_error)?;
    statement
        .query_map((), |row| {
            Ok(ScanReportSummary {
                started_at: row.get(0)?,
                completed_at: row.get(1)?,
                result: row.get(2)?,
                item_count: row.get(3)?,
            })
        })
        .map_err(map_sqlite_error)?
        .map(|row| row.map_err(map_sqlite_error))
        .collect()
}

fn id_from_bytes<T: FromStr<Err = DomainError>>(bytes: &[u8]) -> Result<T, DomainError> {
    let uuid = uuid::Uuid::from_slice(bytes)
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "数据库对象标识损坏。"))?;
    T::from_str(&uuid.to_string())
}

#[cfg(windows)]
fn file_time_to_unix_ms(value: i64) -> i64 {
    value
        .saturating_div(10_000)
        .saturating_sub(11_644_473_600_000)
}

#[cfg(not(windows))]
fn file_time_to_unix_ms(value: i64) -> i64 {
    value.saturating_div(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail_row(kind: &str) -> DetailAssetRow {
        DetailAssetRow {
            asset_id: uuid::Uuid::new_v4().as_bytes().to_vec(),
            version: 1,
            kind: kind.to_string(),
            size: 1,
            mtime: 1,
            preview_state: "JpegSource".to_string(),
            camera_model: None,
            camera_serial: None,
            lens_model: None,
            f_number_numerator: None,
            f_number_denominator: None,
            exposure_time_numerator: None,
            exposure_time_denominator: None,
            iso: None,
            focal_length_numerator: None,
            focal_length_denominator: None,
            pixel_width: None,
            pixel_height: None,
            film_simulation_key: None,
            film_simulation_code: None,
        }
    }

    #[test]
    fn jpg_first_merge_keeps_jpg_values_and_fills_only_missing_fields_from_raf() {
        let mut jpg = detail_row("Jpg");
        jpg.lens_model = Some("XF35mmF1.4 R".to_string());
        jpg.iso = Some(400);
        jpg.pixel_width = Some(6240);
        jpg.pixel_height = Some(4160);
        jpg.film_simulation_key = Some("classic-chrome".to_string());
        jpg.film_simulation_code = Some(0x600);
        let mut raf = detail_row("Raf");
        raf.lens_model = Some("RAF lens should not win".to_string());
        raf.f_number_numerator = Some(14);
        raf.f_number_denominator = Some(5);
        raf.iso = Some(800);
        raf.pixel_width = Some(3000);
        raf.pixel_height = None;

        let mut merged = MergedCaptureFields::default();
        merged.merge(&jpg);
        merged.merge(&raf);

        assert_eq!(merged.lens_model.as_deref(), Some("XF35mmF1.4 R"));
        assert_eq!(merged.iso, Some(400));
        assert_eq!(merged.f_number, rational_view(Some(14), Some(5)));
        assert_eq!(merged.dimensions, Some((6240, 4160)));
        assert_eq!(
            merged.film_simulation,
            Some(FilmSimulationView {
                key: "classic-chrome".to_string(),
                code: Some(0x600),
            })
        );
    }

    #[test]
    fn invalid_pairs_are_not_returned_as_complete_values() {
        assert_eq!(rational_view(Some(1), Some(0)), None);
        let mut row = detail_row("Jpg");
        row.pixel_width = Some(6240);
        let mut merged = MergedCaptureFields::default();
        merged.merge(&row);
        assert_eq!(merged.dimensions, None);
    }
}
