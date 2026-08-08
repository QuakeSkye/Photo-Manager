use std::{
    fs::{self, OpenOptions},
    path::Path,
    time::Instant,
};

use rusqlite::{Connection, params};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    catalog::{CatalogService, SortMode},
    db::{Database, migration::map_sqlite_error},
    domain::{DomainError, ErrorCode, LibraryId},
    roots::{path_key, probe_directory},
    safety::PhaseEightRunRoot,
};

const PHOTO_COUNT: u64 = 100_000;
const ASSET_COUNT: u64 = 200_000;
const LOGICAL_BYTES_PER_ASSET: u64 = 6 * 1024 * 1024;
const ONE_TIB: u64 = 1024 * 1024 * 1024 * 1024;
const BATCH_SIZE: u64 = 1_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleReport {
    report_version: u32,
    run_id: Uuid,
    run_root: String,
    media_incurs_seek_penalty: bool,
    photo_count: u64,
    asset_count: u64,
    logical_asset_bytes: u64,
    allocated_bytes: u64,
    fixture_create_ms: u128,
    database_populate_ms: u128,
    timeline_desc_ms: u128,
    timeline_asc_ms: u128,
    checkpoint_ms: u128,
    checkpoint_busy: bool,
    checkpoint_log_frames: i64,
    checkpointed_frames: i64,
    schema_version: u32,
    schema_checksum: String,
    migration_chain_checksum: String,
    timeline_desc_query_plan: Vec<String>,
    timeline_asc_query_plan: Vec<String>,
}

pub fn run(root: &PhaseEightRunRoot) -> Result<ScaleReport, DomainError> {
    root.verify_current_identity()?;
    let media = root.media_profile()?;
    if !media.device_query_succeeded || !media.incurs_seek_penalty {
        return Err(DomainError::new(
            ErrorCode::UnsupportedMedia,
            "阶段 8 HDD 规模测试要求可证明为机械硬盘的 G:\\test。",
        ));
    }

    let fixture_started = Instant::now();
    let library_path = root.resolve_owned(Path::new("scale-library"))?;
    fs::create_dir(&library_path).map_err(io_error)?;
    create_sparse_assets(root, &library_path)?;
    let fixture_create_ms = fixture_started.elapsed().as_millis();

    let app_data = root.resolve_owned(Path::new("scale-app-data"))?;
    fs::create_dir(&app_data).map_err(io_error)?;
    let database = Database::open(&app_data)?;
    if !database.is_write_healthy() {
        return Err(DomainError::new(
            ErrorCode::ReadOnlySafeMode,
            "规模数据库未以健康读写模式打开。",
        ));
    }
    let library_id = LibraryId::from_uuid(id(1, 1));
    let populate_started = Instant::now();
    populate_database(&database, &library_path, library_id)?;
    let database_populate_ms = populate_started.elapsed().as_millis();

    let counts = exact_counts(&database)?;
    if counts != (PHOTO_COUNT, ASSET_COUNT) {
        return Err(
            DomainError::new(ErrorCode::DatabaseCorrupt, "规模数据库计数不精确。")
                .with_diagnostic(format!("photos={}, assets={}", counts.0, counts.1)),
        );
    }
    let logical_asset_bytes = ASSET_COUNT * LOGICAL_BYTES_PER_ASSET;
    if logical_asset_bytes < ONE_TIB {
        return Err(DomainError::new(
            ErrorCode::Internal,
            "逻辑规模未达到 1 TiB。",
        ));
    }

    let database = std::sync::Arc::new(database);
    let catalog = CatalogService::new(database.clone());
    let desc_started = Instant::now();
    let desc = catalog.timeline(library_id, SortMode::CaptureDesc)?;
    let timeline_desc_ms = desc_started.elapsed().as_millis();
    let asc_started = Instant::now();
    let asc = catalog.timeline(library_id, SortMode::CaptureAsc)?;
    let timeline_asc_ms = asc_started.elapsed().as_millis();
    if desc.len() as u64 != PHOTO_COUNT || asc.len() as u64 != PHOTO_COUNT {
        return Err(DomainError::new(
            ErrorCode::DatabaseCorrupt,
            "规模时间线结果数量错误。",
        ));
    }

    let checkpoint_started = Instant::now();
    let checkpoint = database.controlled_checkpoint()?;
    let checkpoint_ms = checkpoint_started.elapsed().as_millis();
    let connection = database.open_read_connection()?;
    let schema_checksum = crate::db::schema_checksum(&connection)?;
    let desc_plan = timeline_query_plan(&connection, "DESC", library_id)?;
    let asc_plan = timeline_query_plan(&connection, "ASC", library_id)?;
    let allocated_bytes = root.allocated_bytes()?;

    Ok(ScaleReport {
        report_version: 1,
        run_id: root.run_id(),
        run_root: root.path().display().to_string(),
        media_incurs_seek_penalty: true,
        photo_count: counts.0,
        asset_count: counts.1,
        logical_asset_bytes,
        allocated_bytes,
        fixture_create_ms,
        database_populate_ms,
        timeline_desc_ms,
        timeline_asc_ms,
        checkpoint_ms,
        checkpoint_busy: checkpoint.busy,
        checkpoint_log_frames: checkpoint.log_frames,
        checkpointed_frames: checkpoint.checkpointed_frames,
        schema_version: crate::db::LATEST_SCHEMA_VERSION,
        schema_checksum,
        migration_chain_checksum: crate::db::migration_chain_checksum(),
        timeline_desc_query_plan: desc_plan,
        timeline_asc_query_plan: asc_plan,
    })
}

fn create_sparse_assets(root: &PhaseEightRunRoot, library_path: &Path) -> Result<(), DomainError> {
    for bucket in 0..(ASSET_COUNT / BATCH_SIZE) {
        let directory = library_path.join(format!("b{bucket:03}"));
        fs::create_dir(&directory).map_err(io_error)?;
        for offset in 0..BATCH_SIZE {
            let index = bucket * BATCH_SIZE + offset;
            let extension = if index % 2 == 0 { "JPG" } else { "RAF" };
            let path = directory.join(format!("DSCF{:06}.{extension}", index / 2));
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(io_error)?;
            mark_sparse(&file)?;
            file.set_len(LOGICAL_BYTES_PER_ASSET).map_err(io_error)?;
        }
        if (bucket + 1) % 10 == 0 {
            let allocated_bytes = root.ensure_allocation_within_limit(0)?;
            eprintln!(
                "phase8-scale asset_files={} allocated_bytes={allocated_bytes}",
                (bucket + 1) * BATCH_SIZE
            );
        }
    }
    root.ensure_allocation_within_limit(0)?;
    Ok(())
}

fn populate_database(
    database: &Database,
    library_path: &Path,
    library_id: LibraryId,
) -> Result<(), DomainError> {
    let directory = probe_directory(library_path)?;
    let volume_id = id(2, 1);
    let root_id = id(3, 1);
    let binding_id = id(4, 1);
    let library_path = library_path.to_path_buf();
    database.writer()?.transaction(move |transaction| {
        let now = crate::db::migration::now_micros();
        transaction.execute(
            "INSERT INTO library(library_id,library_kind,display_name,lifecycle,created_at) VALUES(?1,'Fuji','Phase 8 scale','Active',?2)",
            params![library_id.as_bytes().as_slice(), now],
        ).map_err(map_sqlite_error)?;
        transaction.execute("INSERT INTO current_library(slot,library_id) VALUES(1,?1)", [library_id.as_bytes().as_slice()]).map_err(map_sqlite_error)?;
        transaction.execute(
            "INSERT INTO volume_identity(volume_id,filesystem_kind,strong_identity_key,identity_format_version,identity_strength,created_at) VALUES(?1,'NTFS',?2,1,'Strong',?3)",
            params![volume_id.as_bytes().as_slice(), directory.volume_identity_key, now],
        ).map_err(map_sqlite_error)?;
        transaction.execute(
            "INSERT INTO root(root_id,library_id,root_kind,capability_mask,current_binding_id,lifecycle,created_at) VALUES(?1,?2,'FujiLibrary',1,?3,'Online',?4)",
            params![root_id.as_bytes().as_slice(), library_id.as_bytes().as_slice(), binding_id.as_bytes().as_slice(), now],
        ).map_err(map_sqlite_error)?;
        transaction.execute(
            "INSERT INTO root_binding(binding_id,root_id,binding_generation,volume_id,absolute_path_display,absolute_path_key,root_directory_file_id,filesystem_kind,evidence_level,reparse_checked,binding_state,verified_at) VALUES(?1,?2,1,?3,?4,?5,?6,'NTFS','Strong',1,'Active',?7)",
            params![binding_id.as_bytes().as_slice(), root_id.as_bytes().as_slice(), volume_id.as_bytes().as_slice(), library_path.display().to_string(), path_key(&library_path), directory.directory_file_id, now],
        ).map_err(map_sqlite_error)?;
        Ok(())
    })?;

    for start_photo in (0..PHOTO_COUNT).step_by(BATCH_SIZE as usize) {
        database.writer()?.transaction(move |transaction| {
            let now = crate::db::migration::now_micros();
            for photo_index in start_photo..start_photo + BATCH_SIZE {
                let photo_id = id(10, photo_index + 1);
                let capture = format!("2026-01-{:02}T{:02}:{:02}:{:02}.{:06}", (photo_index % 28) + 1, photo_index % 24, photo_index % 60, (photo_index / 60) % 60, photo_index);
                let name = format!("DSCF{photo_index:06}");
                transaction.execute(
                    "INSERT INTO photo(photo_id,library_id,photo_version,lifecycle,date_class,capture_local_key,natural_name_display,natural_sort_key,sort_key_version,created_at) VALUES(?1,?2,1,'Active','Known',?3,?4,?5,1,?6)",
                    params![photo_id.as_bytes().as_slice(), library_id.as_bytes().as_slice(), capture, name, format!("{photo_index:020}").into_bytes(), now],
                ).map_err(map_sqlite_error)?;
                for member in 0..2_u64 {
                    let asset_index = photo_index * 2 + member;
                    let asset_id = id(20, asset_index + 1);
                    let relation_id = id(30, asset_index + 1);
                    let kind = if member == 0 { "Jpg" } else { "Raf" };
                    let extension = if member == 0 { "JPG" } else { "RAF" };
                    let relative = format!("b{:03}/DSCF{photo_index:06}.{extension}", asset_index / BATCH_SIZE);
                    let key = relative.to_lowercase().into_bytes();
                    transaction.execute(
                        "INSERT INTO asset(asset_id,library_id,root_id,media_kind,current_version,current_path_key,presence_state,deletion_supported,created_at) VALUES(?1,?2,?3,?4,1,?5,'Present',1,?6)",
                        params![asset_id.as_bytes().as_slice(), library_id.as_bytes().as_slice(), root_id.as_bytes().as_slice(), kind, key, now],
                    ).map_err(map_sqlite_error)?;
                    transaction.execute(
                        "INSERT INTO asset_revision(asset_id,asset_version,root_id,root_binding_id,relative_path,path_key,path_key_version,size_bytes,mtime_filetime,presence_state,hard_link_count,observed_at) VALUES(?1,1,?2,?3,?4,?5,1,?6,0,'Present',1,?7)",
                        params![asset_id.as_bytes().as_slice(), root_id.as_bytes().as_slice(), binding_id.as_bytes().as_slice(), relative, key, LOGICAL_BYTES_PER_ASSET as i64, now],
                    ).map_err(map_sqlite_error)?;
                    transaction.execute(
                        "INSERT INTO asset_media_metadata(asset_id,asset_version,capture_local_key,camera_model,camera_serial,preview_state,parser_version,observed_at) VALUES(?1,1,?2,'Generated-XM5','PHASE8','JpegSource',1,?3)",
                        params![asset_id.as_bytes().as_slice(), capture, now],
                    ).map_err(map_sqlite_error)?;
                    transaction.execute(
                        "INSERT INTO photo_asset(photo_asset_id,photo_id,asset_id,role,from_photo_version) VALUES(?1,?2,?3,?4,1)",
                        params![relation_id.as_bytes().as_slice(), photo_id.as_bytes().as_slice(), asset_id.as_bytes().as_slice(), kind],
                    ).map_err(map_sqlite_error)?;
                }
            }
            Ok(())
        })?;
        if (start_photo + BATCH_SIZE) % 10_000 == 0 {
            eprintln!(
                "phase8-scale photos={} assets={}",
                start_photo + BATCH_SIZE,
                (start_photo + BATCH_SIZE) * 2
            );
        }
    }
    Ok(())
}

fn exact_counts(database: &Database) -> Result<(u64, u64), DomainError> {
    let connection = database.open_read_connection()?;
    let (photos, assets) = connection
        .query_row(
            "SELECT (SELECT count(*) FROM photo), (SELECT count(*) FROM asset)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(map_sqlite_error)?;
    Ok((photos as u64, assets as u64))
}

fn timeline_query_plan(
    connection: &Connection,
    direction: &str,
    library_id: LibraryId,
) -> Result<Vec<String>, DomainError> {
    let sql = format!(
        "EXPLAIN QUERY PLAN SELECT photo.photo_id FROM photo JOIN photo_asset relation ON relation.photo_id=photo.photo_id AND relation.to_photo_version IS NULL JOIN asset ON asset.asset_id=relation.asset_id AND asset.presence_state='Present' JOIN root ON root.root_id=asset.root_id AND root.lifecycle='Online' WHERE photo.library_id=?1 AND photo.lifecycle='Active' GROUP BY photo.photo_id ORDER BY photo.date_class ASC, photo.capture_local_key {direction}, photo.natural_sort_key ASC, photo.photo_id ASC"
    );
    let mut statement = connection.prepare(&sql).map_err(map_sqlite_error)?;
    statement
        .query_map([library_id.as_bytes().as_slice()], |row| {
            row.get::<_, String>(3)
        })
        .map_err(map_sqlite_error)?
        .map(|row| row.map_err(map_sqlite_error))
        .collect()
}

const fn id(namespace: u64, value: u64) -> Uuid {
    Uuid::from_u128(((namespace as u128) << 64) | value as u128)
}

#[cfg(windows)]
fn mark_sparse(file: &std::fs::File) -> Result<(), DomainError> {
    use std::{os::windows::io::AsRawHandle, ptr};
    let mut returned = 0_u32;
    let ok = unsafe {
        windows_sys::Win32::System::IO::DeviceIoControl(
            file.as_raw_handle() as *mut _,
            windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE,
            ptr::null(),
            0,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io_error(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn mark_sparse(_file: &std::fs::File) -> Result<(), DomainError> {
    Err(DomainError::new(
        ErrorCode::UnsupportedMedia,
        "阶段 8 sparse fixture 仅支持 Windows NTFS。",
    ))
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::CapabilityDenied, "阶段 8 规模夹具 I/O 失败。")
        .with_diagnostic(error.to_string())
}
