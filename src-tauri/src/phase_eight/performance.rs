use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rusqlite::OptionalExtension;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    cache::{CacheKind, CacheService},
    catalog::{CatalogService, SortMode},
    db::{Database, migration::map_sqlite_error},
    domain::{AssetId, DomainError, ErrorCode, LibraryId, OperationId, PhotoId},
    io_scheduler::{IoPriority, IoScheduler},
    pending_delete::PendingDeleteService,
    safety::{FaultInjector, PhaseEightRunRoot},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceReport {
    report_version: u32,
    run_id: Uuid,
    database_open_ms: u128,
    initial_timeline_ms: u128,
    indexed_startup_to_operable_ms: u128,
    cache_hit_p95_ms: u128,
    cache_hit_samples_ms: Vec<u128>,
    uncached_feedback_p95_ms: u128,
    uncached_preview_p95_ms: u128,
    uncached_preview_samples_ms: Vec<u128>,
    mark_or_undo_p95_ms: u128,
    mark_or_undo_samples_ms: Vec<u128>,
    working_set_bytes: u64,
    background_cache_hit_p95_ms: u128,
    background_degradation_ratio: f64,
    cancel_response_ms: u128,
    timeline_photo_count: usize,
    all_thresholds_passed: bool,
}

pub fn run(root: &PhaseEightRunRoot) -> Result<PerformanceReport, DomainError> {
    root.verify_current_identity()?;
    let scale_app_data = root.resolve_owned(Path::new("scale-app-data"))?;
    let startup = Instant::now();
    let scale_database = Arc::new(Database::open(&scale_app_data)?);
    let database_open_ms = startup.elapsed().as_millis();
    let library_id = current_library(&scale_database)?;
    let catalog = CatalogService::new(scale_database.clone());
    let timeline_started = Instant::now();
    let timeline = catalog.timeline(library_id, SortMode::CaptureDesc)?;
    let initial_timeline_ms = timeline_started.elapsed().as_millis();
    let indexed_startup_to_operable_ms = startup.elapsed().as_millis();
    let working_set_bytes = process_working_set_bytes()?;

    let cache_suite = root.resolve_owned(Path::new("candidate-cache-suite-v2"))?;
    let cache_app_data = cache_suite.join("app-data");
    let cache_database = Arc::new(Database::open(&cache_app_data)?);
    let (asset_id, asset_version, photo_id, photo_version, cached_path) =
        cache_fixture_state(&cache_database)?;
    let mut cache_hit_samples_ms = Vec::new();
    for _ in 0..101 {
        let started = Instant::now();
        let bytes = fs::read(&cached_path).map_err(io_error)?;
        if bytes.is_empty() {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存命中对象为空。",
            ));
        }
        cache_hit_samples_ms.push(started.elapsed().as_millis());
    }
    let cache_hit_p95_ms = p95(&cache_hit_samples_ms);

    let scheduler = Arc::new(IoScheduler::for_rotational());
    let mut feedback_samples = Vec::new();
    for _ in 0..101 {
        scheduler.note_browse_activity()?;
        let started = Instant::now();
        drop(scheduler.acquire(IoPriority::CurrentPreview, || false)?);
        feedback_samples.push(started.elapsed().as_millis());
    }
    let uncached_feedback_p95_ms = p95(&feedback_samples);

    let cache_service = CacheService::new(
        cache_database.clone(),
        &cache_app_data,
        FaultInjector::default(),
    );
    let preview_bytes = vec![0x5a; 512 * 1024];
    let preview_run = Uuid::new_v4();
    let mut uncached_preview_samples_ms = Vec::new();
    for ordinal in 0..20_u32 {
        let started = Instant::now();
        let entry = cache_service.get_or_create(
            asset_id,
            asset_version,
            CacheKind::Preview,
            format!("phase8-perf-preview-{preview_run}-{ordinal}").as_bytes(),
            &preview_bytes,
        )?;
        if !entry.absolute_path.is_file() {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "未缓存预览未落地。",
            ));
        }
        uncached_preview_samples_ms.push(started.elapsed().as_millis());
    }
    let uncached_preview_p95_ms = p95(&uncached_preview_samples_ms);

    let pending = PendingDeleteService::new(cache_database.clone());
    let mut mark_or_undo_samples_ms = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        pending.toggle(photo_id, photo_version, OperationId::new())?;
        mark_or_undo_samples_ms.push(started.elapsed().as_millis());
        let started = Instant::now();
        pending.undo(OperationId::new())?;
        mark_or_undo_samples_ms.push(started.elapsed().as_millis());
    }
    let mark_or_undo_p95_ms = p95(&mark_or_undo_samples_ms);

    let background_path = root.resolve_owned(Path::new("io-sample-unbuffered-v2.bin"))?;
    let background_scheduler = scheduler.clone();
    let background = std::thread::spawn(move || -> Result<(), DomainError> {
        use std::io::Read;
        let _permit = background_scheduler.acquire(IoPriority::BackgroundHashAndCache, || false)?;
        let mut file = fs::File::open(background_path).map_err(io_error)?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        for _ in 0..256 {
            file.read_exact(&mut buffer).map_err(io_error)?;
        }
        Ok(())
    });
    let mut background_samples = Vec::new();
    for _ in 0..101 {
        scheduler.note_browse_activity()?;
        let _permit = scheduler.acquire(IoPriority::CurrentPreview, || false)?;
        let started = Instant::now();
        let _ = fs::read(&cached_path).map_err(io_error)?;
        background_samples.push(started.elapsed().as_millis());
    }
    background
        .join()
        .map_err(|_| DomainError::new(ErrorCode::Internal, "后台 I/O 性能线程崩溃。"))??;
    let background_cache_hit_p95_ms = p95(&background_samples);
    let background_degradation_ratio =
        background_cache_hit_p95_ms as f64 / cache_hit_p95_ms.max(1) as f64;

    scheduler.note_browse_activity()?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let cancel_scheduler = scheduler.clone();
    let cancel_started = Instant::now();
    let waiter = std::thread::spawn(move || {
        cancel_scheduler
            .acquire(IoPriority::BackgroundHashAndCache, || {
                worker_cancelled.load(Ordering::Acquire)
            })
            .map(drop)
    });
    std::thread::sleep(Duration::from_millis(50));
    cancelled.store(true, Ordering::Release);
    let cancelled_result = waiter
        .join()
        .map_err(|_| DomainError::new(ErrorCode::Internal, "取消响应线程崩溃。"))?;
    if !cancelled_result.is_err_and(|error| error.code == ErrorCode::InvalidTransition) {
        return Err(DomainError::new(
            ErrorCode::Internal,
            "后台 I/O 未按取消请求停止。",
        ));
    }
    let cancel_response_ms = cancel_started.elapsed().as_millis();

    let all_thresholds_passed = indexed_startup_to_operable_ms <= 3_000
        && cache_hit_p95_ms <= 150
        && uncached_feedback_p95_ms <= 100
        && uncached_preview_p95_ms <= 2_000
        && mark_or_undo_p95_ms <= 250
        && working_set_bytes <= 1_610_612_736
        && background_degradation_ratio <= 2.0
        && cancel_response_ms <= 2_000
        && timeline.len() == 100_000;
    let report = PerformanceReport {
        report_version: 1,
        run_id: root.run_id(),
        database_open_ms,
        initial_timeline_ms,
        indexed_startup_to_operable_ms,
        cache_hit_p95_ms,
        cache_hit_samples_ms,
        uncached_feedback_p95_ms,
        uncached_preview_p95_ms,
        uncached_preview_samples_ms,
        mark_or_undo_p95_ms,
        mark_or_undo_samples_ms,
        working_set_bytes,
        background_cache_hit_p95_ms,
        background_degradation_ratio,
        cancel_response_ms,
        timeline_photo_count: timeline.len(),
        all_thresholds_passed,
    };
    if !report.all_thresholds_passed {
        return Err(
            DomainError::new(ErrorCode::CapabilityDenied, "阶段 8 性能阈值未全部通过。")
                .with_diagnostic(serde_json::to_string(&report).unwrap_or_default()),
        );
    }
    Ok(report)
}

fn current_library(database: &Database) -> Result<LibraryId, DomainError> {
    let bytes: Vec<u8> = database
        .open_read_connection()?
        .query_row(
            "SELECT library_id FROM current_library WHERE slot=1",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    Ok(LibraryId::from_uuid(
        Uuid::from_slice(&bytes).map_err(invalid_id)?,
    ))
}

fn cache_fixture_state(
    database: &Database,
) -> Result<(AssetId, i64, PhotoId, i64, std::path::PathBuf), DomainError> {
    let connection = database.open_read_connection()?;
    let row = connection
        .query_row(
            "SELECT a.asset_id,a.current_version,p.photo_id,p.photo_version,
                    b.absolute_path_display,e.relative_path
             FROM asset a JOIN photo_asset pa ON pa.asset_id=a.asset_id AND pa.to_photo_version IS NULL
             JOIN photo p ON p.photo_id=pa.photo_id
             JOIN cache_entry e ON e.asset_id=a.asset_id AND e.entry_state='Live'
             JOIN root r ON r.root_id=e.cache_root_id
             JOIN root_binding b ON b.binding_id=r.current_binding_id
             ORDER BY e.entry_ordinal DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or_else(|| DomainError::new(ErrorCode::CacheOwnership, "性能套件缺少活动缓存夹具。"))?;
    Ok((
        AssetId::from_uuid(Uuid::from_slice(&row.0).map_err(invalid_id)?),
        row.1,
        PhotoId::from_uuid(Uuid::from_slice(&row.2).map_err(invalid_id)?),
        row.3,
        Path::new(&row.4).join(row.5),
    ))
}

fn p95(samples: &[u128]) -> u128 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[((sorted.len() * 95).div_ceil(100)).saturating_sub(1)]
}

#[cfg(windows)]
fn process_working_set_bytes() -> Result<u64, DomainError> {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok == 0 {
        Err(io_error(std::io::Error::last_os_error()))
    } else {
        Ok(counters.PeakWorkingSetSize as u64)
    }
}

#[cfg(not(windows))]
fn process_working_set_bytes() -> Result<u64, DomainError> {
    Err(DomainError::new(
        ErrorCode::UnsupportedMedia,
        "working set 验证仅支持 Windows。",
    ))
}

fn invalid_id(error: uuid::Error) -> DomainError {
    DomainError::new(ErrorCode::DatabaseCorrupt, "性能夹具标识损坏。")
        .with_diagnostic(error.to_string())
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "阶段 8 性能样本 I/O 失败。")
        .with_diagnostic(error.to_string())
}
