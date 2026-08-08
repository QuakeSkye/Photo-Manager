use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    recycle::shell::{
        REQUIRED_RECYCLE_FLAGS, RecycleCallbacks, ShellAsset, ShellPhoto, StaRecycleWorker,
    },
    safety::PhaseEightRunRoot,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetySuiteReport {
    report_version: u32,
    run_id: uuid::Uuid,
    import: crate::import_execution::PhaseEightImportFileOpsReport,
    recycle: RecycleReport,
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

pub fn run(root: &PhaseEightRunRoot) -> Result<SafetySuiteReport, DomainError> {
    root.verify_current_identity()?;
    let import_root = root.resolve_owned(Path::new("candidate-import-suite"))?;
    let import = crate::import_execution::phase_eight_file_ops_suite(&import_root)?;
    let recycle_root = root.resolve_owned(Path::new("candidate-recycle-suite"))?;
    let recycle = run_recycle(&recycle_root)?;
    root.verify_current_identity()?;
    root.ensure_allocation_within_limit(0)?;
    Ok(SafetySuiteReport {
        report_version: 1,
        run_id: root.run_id(),
        import,
        recycle,
    })
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
