use std::{
    fs,
    path::Path,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    db::Database,
    domain::{DomainError, ErrorCode},
    recycle::shell::{
        REQUIRED_RECYCLE_FLAGS, RecycleCallbacks, ShellAsset, ShellPhoto, StaRecycleWorker,
    },
    safety::{
        FaultInjector, FaultPoint, FeatureGate, GateCloseReason, GateState, PhaseEightRunRoot,
        RuntimeSafetyGate, SharedHealth,
    },
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetySuiteReport {
    report_version: u32,
    run_id: uuid::Uuid,
    import: crate::import_execution::PhaseEightImportFileOpsReport,
    recycle: RecycleReport,
    runtime_gate: RuntimeGateReport,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecycleReport {
    required_flags: u32,
    permanent_delete_flag_present: bool,
    callback_count: usize,
    recycle_objects_nonempty: bool,
    restored_identical: bool,
    operations_aborted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeGateReport {
    healthy_default_open: bool,
    startup_closed_until_ready: bool,
    unannounced_writer_poison_require_rejected: bool,
    unannounced_writer_poison_revalidate_rejected: bool,
    wrong_feature_rejected: bool,
    wrong_instance_rejected: bool,
    shared_fault_closes_both: bool,
    fault_latch_monotonic: bool,
    post_physical_latch_checkpoint_completed: bool,
    stale_permit_rejected_after_latch: bool,
    concurrent_fault_invalidates_issued_permits: bool,
}

pub fn run(root: &PhaseEightRunRoot) -> Result<SafetySuiteReport, DomainError> {
    root.verify_current_identity()?;
    let import_root = root.resolve_owned(Path::new("candidate-import-suite"))?;
    let import = crate::import_execution::phase_eight_file_ops_suite(&import_root)?;
    let recycle_root = root.resolve_owned(Path::new("candidate-recycle-suite"))?;
    let recycle = run_recycle(&recycle_root)?;
    let gate_root = root.resolve_owned(Path::new("candidate-runtime-gate-suite"))?;
    let runtime_gate = run_runtime_gate(&gate_root)?;
    root.verify_current_identity()?;
    root.ensure_allocation_within_limit(0)?;
    Ok(SafetySuiteReport {
        report_version: 2,
        run_id: root.run_id(),
        import,
        recycle,
        runtime_gate,
    })
}

fn run_runtime_gate(directory: &Path) -> Result<RuntimeGateReport, DomainError> {
    fs::create_dir(directory).map_err(io_error)?;
    let startup_directory = directory.join("startup");
    fs::create_dir(&startup_directory).map_err(io_error)?;
    let startup_database = Arc::new(Database::open(&startup_directory)?);
    let startup_gate = RuntimeSafetyGate::new(startup_database);
    let startup_closed_until_ready = startup_gate.status().global_write_gate == GateState::Closed
        && startup_gate.require_feature(FeatureGate::Import).is_err();
    startup_gate.mark_ready()?;

    let database = Arc::new(Database::open(directory)?);
    let gate = Arc::new(RuntimeSafetyGate::new(database.clone()));
    gate.mark_ready()?;
    let permit = gate.require_feature(FeatureGate::Import)?;
    let healthy_default_open = gate.status().global_write_gate == GateState::Open;
    let recycle_permit = gate.require_feature(FeatureGate::Recycle)?;
    let wrong_feature_rejected = gate
        .revalidate(&recycle_permit, FeatureGate::Import)
        .is_err();
    let foreign_gate = RuntimeSafetyGate::new(database.clone());
    foreign_gate.mark_ready()?;
    let foreign_permit = foreign_gate.require_feature(FeatureGate::Import)?;
    let wrong_instance_rejected = gate
        .revalidate(&foreign_permit, FeatureGate::Import)
        .is_err();
    let faults = FaultInjector::default();
    faults.latch_gate_on_hit(
        FaultPoint::ImportAfterPhysicalBeforeFact,
        1,
        gate.clone(),
        GateCloseReason::CapabilityBoundary,
    )?;
    let post_physical_latch_checkpoint_completed = faults
        .checkpoint(FaultPoint::ImportAfterPhysicalBeforeFact)
        .is_ok();
    let stale_permit_rejected_after_latch = gate.revalidate(&permit, FeatureGate::Import).is_err();
    let fault_latch_monotonic =
        gate.mark_ready().is_err() && gate.status().global_write_gate == GateState::Closed;

    let poison_directory = directory.join("writer-poison");
    fs::create_dir(&poison_directory).map_err(io_error)?;
    let poison_database = Arc::new(Database::open(&poison_directory)?);
    let poison_gate = RuntimeSafetyGate::new(poison_database.clone());
    poison_gate.mark_ready()?;
    let poison_permit = poison_gate.require_feature(FeatureGate::Recycle)?;
    poison_database
        .write_health_handle()
        .store(false, Ordering::Release);
    let unannounced_writer_poison_require_rejected =
        poison_gate.require_feature(FeatureGate::Recycle).is_err();
    let unannounced_writer_poison_revalidate_rejected = poison_gate
        .revalidate(&poison_permit, FeatureGate::Recycle)
        .is_err();

    let shared_directory = directory.join("shared-fault");
    fs::create_dir(&shared_directory).map_err(io_error)?;
    let shared_database = Arc::new(Database::open(&shared_directory)?);
    let shared_gate = RuntimeSafetyGate::new(shared_database);
    shared_gate.mark_ready()?;
    let mut health = SharedHealth::healthy();
    health.reconciler = false;
    shared_gate.set_shared_health(health);
    let shared_status = shared_gate.status();
    let shared_fault_closes_both = shared_status.import_gate == GateState::Closed
        && shared_status.recycle_gate == GateState::Closed;

    let concurrent_directory = directory.join("concurrent-fault");
    fs::create_dir(&concurrent_directory).map_err(io_error)?;
    let concurrent_database = Arc::new(Database::open(&concurrent_directory)?);
    let concurrent_gate = Arc::new(RuntimeSafetyGate::new(concurrent_database));
    concurrent_gate.mark_ready()?;
    let barrier = Arc::new(Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let worker_gate = concurrent_gate.clone();
        let worker_barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            worker_barrier.wait();
            worker_gate.require_feature(FeatureGate::Import).ok()
        }));
    }
    barrier.wait();
    concurrent_gate.latch_fault(GateCloseReason::CapabilityBoundary);
    let concurrent_fault_invalidates_issued_permits = workers.into_iter().all(|worker| {
        worker.join().is_ok_and(|permit| {
            permit.is_none_or(|permit| {
                concurrent_gate
                    .revalidate(&permit, FeatureGate::Import)
                    .is_err()
            })
        })
    });

    let report = RuntimeGateReport {
        healthy_default_open,
        startup_closed_until_ready,
        unannounced_writer_poison_require_rejected,
        unannounced_writer_poison_revalidate_rejected,
        wrong_feature_rejected,
        wrong_instance_rejected,
        shared_fault_closes_both,
        fault_latch_monotonic,
        post_physical_latch_checkpoint_completed,
        stale_permit_rejected_after_latch,
        concurrent_fault_invalidates_issued_permits,
    };
    if !report.healthy_default_open
        || !report.startup_closed_until_ready
        || !report.unannounced_writer_poison_require_rejected
        || !report.unannounced_writer_poison_revalidate_rejected
        || !report.wrong_feature_rejected
        || !report.wrong_instance_rejected
        || !report.shared_fault_closes_both
        || !report.fault_latch_monotonic
        || !report.post_physical_latch_checkpoint_completed
        || !report.stale_permit_rejected_after_latch
        || !report.concurrent_fault_invalidates_issued_permits
    {
        return Err(DomainError::new(
            ErrorCode::CapabilityDenied,
            "阶段 8 运行时安全门候选套件未通过。",
        ));
    }
    Ok(report)
}

fn run_recycle(directory: &Path) -> Result<RecycleReport, DomainError> {
    fs::create_dir(directory).map_err(io_error)?;
    let jpeg = directory.join("PAIR.JPG");
    let raf = directory.join("PAIR.RAF");
    let jpeg_bytes = b"phase-eight-generated-recycle-jpeg";
    let raf_bytes = b"phase-eight-generated-recycle-raf";
    create_new(&jpeg, jpeg_bytes)?;
    create_new(&raf, raf_bytes)?;
    let expected = [Sha256::digest(jpeg_bytes), Sha256::digest(raf_bytes)];

    struct Callbacks {
        callbacks: AtomicUsize,
    }
    impl RecycleCallbacks for Callbacks {
        fn pre_delete(&self, asset: &ShellAsset) -> Result<(), DomainError> {
            if !asset.absolute_path.is_file() {
                return Err(DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "回收前生成夹具缺失。",
                ));
            }
            Ok(())
        }

        fn post_delete(
            &self,
            asset: &ShellAsset,
            hr_delete: i32,
            evidence: Option<Vec<u8>>,
        ) -> Result<(), DomainError> {
            if hr_delete < 0
                || evidence.as_ref().is_none_or(Vec::is_empty)
                || asset.absolute_path.exists()
            {
                return Err(DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "回收结果缺少非空对象证据。",
                ));
            }
            self.callbacks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let callbacks = Arc::new(Callbacks {
        callbacks: AtomicUsize::new(0),
    });
    let worker = StaRecycleWorker::spawn()?;
    let result = worker.recycle_and_restore_for_phase_eight(
        ShellPhoto {
            assets: vec![
                ShellAsset {
                    attempt_asset_id: vec![1; 16],
                    absolute_path: jpeg.clone(),
                },
                ShellAsset {
                    attempt_asset_id: vec![2; 16],
                    absolute_path: raf.clone(),
                },
            ],
        },
        callbacks.clone(),
    )?;
    let restored = [
        Sha256::digest(fs::read(&jpeg).map_err(io_error)?),
        Sha256::digest(fs::read(&raf).map_err(io_error)?),
    ];
    let callback_count = callbacks.callbacks.load(Ordering::SeqCst);
    let report = RecycleReport {
        required_flags: REQUIRED_RECYCLE_FLAGS,
        permanent_delete_flag_present: REQUIRED_RECYCLE_FLAGS & 0x0000_4000 != 0,
        callback_count,
        recycle_objects_nonempty: callback_count == 2,
        restored_identical: restored == expected,
        operations_aborted: result.operations_aborted,
    };
    if result.perform_operations_result != 0
        || report.operations_aborted
        || report.permanent_delete_flag_present
        || !report.recycle_objects_nonempty
        || !report.restored_identical
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "阶段 8 回收站恢复套件未通过。",
        ));
    }
    Ok(report)
}

fn create_new(path: &Path, bytes: &[u8]) -> Result<(), DomainError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::Internal, "阶段 8 Recycle 生成夹具 I/O 失败。")
        .with_diagnostic(error.to_string())
}
