use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags, backup::Backup};
use serde::Serialize;

use crate::{
    db::migration::map_sqlite_error,
    domain::{DomainError, ErrorCode},
    safety::{PhaseEightRunRoot, TestRoot},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiFixtureReport {
    report_version: u32,
    run_id: uuid::Uuid,
    test_root: String,
    photo_count: u64,
}

pub fn prepare(root: &PhaseEightRunRoot) -> Result<UiFixtureReport, DomainError> {
    root.verify_current_identity()?;
    let test_root = TestRoot::create_in(root.path())?;
    let app_data = test_root.path().join("app-data");
    std::fs::create_dir(&app_data).map_err(io_error)?;
    let source_path = root.resolve_owned(Path::new("scale-app-data/catalog.sqlite3"))?;
    let source = Connection::open_with_flags(
        source_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(map_sqlite_error)?;
    let target_path = app_data.join("catalog.sqlite3");
    if target_path.exists() {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "UI 夹具目标不得覆盖。",
        ));
    }
    let mut target = Connection::open_with_flags(
        &target_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(map_sqlite_error)?;
    let backup = Backup::new(&source, &mut target).map_err(map_sqlite_error)?;
    backup
        .run_to_completion(64, Duration::from_millis(10), None)
        .map_err(map_sqlite_error)?;
    drop(backup);
    let photo_count: u64 = target
        .query_row("SELECT count(*) FROM photo", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)? as u64;
    if photo_count != 100_000 {
        return Err(DomainError::new(
            ErrorCode::DatabaseCorrupt,
            "UI 夹具不是 10万 Photo 候选库。",
        ));
    }
    target
        .execute(
            "INSERT INTO scan_run(
                scan_run_id,library_id,root_id,root_binding_id,run_state,
                started_at,completed_at,observed_assets
             ) SELECT ?1,current.library_id,root.root_id,root.current_binding_id,
                      'Completed',1,2,200000
               FROM current_library current
               JOIN root ON root.library_id=current.library_id
              WHERE current.slot=1 AND root.root_kind='FujiLibrary'",
            [uuid::Uuid::new_v4().as_bytes().as_slice()],
        )
        .map_err(map_sqlite_error)?;
    target
        .close()
        .map_err(|(_, error)| map_sqlite_error(error))?;
    root.verify_current_identity()?;
    Ok(UiFixtureReport {
        report_version: 1,
        run_id: root.run_id(),
        test_root: test_root.path().display().to_string(),
        photo_count,
    })
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::DatabaseDurability, "阶段 8 UI 夹具创建失败。")
        .with_diagnostic(error.to_string())
}
