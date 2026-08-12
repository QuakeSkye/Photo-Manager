use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    io_scheduler::{IoPriority, IoScheduler},
    roots::{FileProbe, open_read_only, path_is_reparse, probe_file, resolve_contained_path},
};

#[derive(Debug, Clone)]
pub(crate) struct StableHashObservation {
    pub before: FileProbe,
    pub after: FileProbe,
    pub hash: [u8; 32],
}

pub(crate) fn stable_hash(
    scheduler: &Arc<IoScheduler>,
    cancelled: &AtomicBool,
    root: &Path,
    relative: &str,
    expected: &FileProbe,
) -> Result<StableHashObservation, DomainError> {
    let _permit = scheduler.acquire(IoPriority::BackgroundHashAndCache, || {
        cancelled.load(Ordering::Acquire)
    })?;
    let path = safe_file(root, relative)?;
    let mut handle = open_read_only(&path)?;
    let before = probe_file(&handle)?;
    require_identity(&before, expected)?;
    let hash = hash_file(&mut handle, cancelled)?;
    let after = probe_file(&handle)?;
    require_identity(&after, &before)?;
    let reopened = open_read_only(&path)?;
    require_identity(&probe_file(&reopened)?, &before)?;
    Ok(StableHashObservation {
        before,
        after,
        hash,
    })
}

pub(crate) fn stable_byte_equal(
    scheduler: &Arc<IoScheduler>,
    cancelled: &AtomicBool,
    root: &Path,
    left_relative: &str,
    left_expected: &FileProbe,
    right_relative: &str,
    right_expected: &FileProbe,
) -> Result<bool, DomainError> {
    let _permit = scheduler.acquire(IoPriority::BackgroundHashAndCache, || {
        cancelled.load(Ordering::Acquire)
    })?;
    let left_path = safe_file(root, left_relative)?;
    let right_path = safe_file(root, right_relative)?;
    let mut left = open_read_only(&left_path)?;
    let mut right = open_read_only(&right_path)?;
    let left_before = probe_file(&left)?;
    let right_before = probe_file(&right)?;
    require_identity(&left_before, left_expected)?;
    require_identity(&right_before, right_expected)?;
    if left_before.size_bytes != right_before.size_bytes {
        return Ok(false);
    }
    let left_hash = hash_file(&mut left, cancelled)?;
    let right_hash = hash_file(&mut right, cancelled)?;
    if left_hash != right_hash {
        return Ok(false);
    }
    left.seek(SeekFrom::Start(0)).map_err(read_error)?;
    right.seek(SeekFrom::Start(0)).map_err(read_error)?;
    let mut left_buffer = vec![0_u8; 1024 * 1024];
    let mut right_buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(cancelled_error());
        }
        let left_count = left.read(&mut left_buffer).map_err(read_error)?;
        let right_count = right.read(&mut right_buffer).map_err(read_error)?;
        if left_count != right_count || left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
        if left_count == 0 {
            break;
        }
    }
    require_identity(&probe_file(&left)?, &left_before)?;
    require_identity(&probe_file(&right)?, &right_before)?;
    let left_reopened = open_read_only(&left_path)?;
    let right_reopened = open_read_only(&right_path)?;
    require_identity(&probe_file(&left_reopened)?, &left_before)?;
    require_identity(&probe_file(&right_reopened)?, &right_before)?;
    Ok(true)
}

#[derive(Debug, Clone)]
pub(crate) struct StablePairObservation {
    pub deleted: StableHashObservation,
    pub retained: StableHashObservation,
}

pub(crate) struct StablePairProofInput<'a> {
    pub root: &'a Path,
    pub deleted_relative: &'a str,
    pub deleted_expected: &'a FileProbe,
    pub retained_relative: &'a str,
    pub retained_expected: &'a FileProbe,
}

pub(crate) fn stable_pair_proof(
    scheduler: &Arc<IoScheduler>,
    cancelled: &AtomicBool,
    root: &Path,
    deleted_relative: &str,
    deleted_expected: &FileProbe,
    retained_relative: &str,
    retained_expected: &FileProbe,
) -> Result<StablePairObservation, DomainError> {
    let deleted = stable_hash(
        scheduler,
        cancelled,
        root,
        deleted_relative,
        deleted_expected,
    )?;
    let retained = stable_hash(
        scheduler,
        cancelled,
        root,
        retained_relative,
        retained_expected,
    )?;
    if deleted.hash != retained.hash
        || !stable_byte_equal(
            scheduler,
            cancelled,
            root,
            deleted_relative,
            deleted_expected,
            retained_relative,
            retained_expected,
        )?
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "待删文件与保留副本已不再逐字节相同。",
        ));
    }
    Ok(StablePairObservation { deleted, retained })
}

pub(crate) fn stable_pair_stream_proof(
    scheduler: &Arc<IoScheduler>,
    cancelled: &AtomicBool,
    input: StablePairProofInput<'_>,
    mut on_bytes_read: impl FnMut(u64),
) -> Result<StablePairObservation, DomainError> {
    let _permit = scheduler.acquire(IoPriority::BackgroundHashAndCache, || {
        cancelled.load(Ordering::Acquire)
    })?;
    let deleted_path = safe_file(input.root, input.deleted_relative)?;
    let retained_path = safe_file(input.root, input.retained_relative)?;
    let mut deleted_handle = open_read_only(&deleted_path)?;
    let mut retained_handle = open_read_only(&retained_path)?;
    let deleted_before = probe_file(&deleted_handle)?;
    let retained_before = probe_file(&retained_handle)?;
    require_identity(&deleted_before, input.deleted_expected)?;
    require_identity(&retained_before, input.retained_expected)?;
    if deleted_before.size_bytes != retained_before.size_bytes {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "待删文件与保留副本大小已不一致。",
        ));
    }

    let mut deleted_digest = Sha256::new();
    let mut retained_digest = Sha256::new();
    let mut deleted_buffer = vec![0_u8; 1024 * 1024];
    let mut retained_buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(cancelled_error());
        }
        let deleted_count = deleted_handle
            .read(&mut deleted_buffer)
            .map_err(read_error)?;
        let retained_count = retained_handle
            .read(&mut retained_buffer)
            .map_err(read_error)?;
        on_bytes_read(u64::try_from(deleted_count.saturating_add(retained_count)).unwrap_or(0));
        if deleted_count != retained_count
            || deleted_buffer[..deleted_count] != retained_buffer[..retained_count]
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "待删文件与保留副本已不再逐字节相同。",
            ));
        }
        if deleted_count == 0 {
            break;
        }
        deleted_digest.update(&deleted_buffer[..deleted_count]);
        retained_digest.update(&retained_buffer[..retained_count]);
    }
    let deleted_hash: [u8; 32] = deleted_digest.finalize().into();
    let retained_hash: [u8; 32] = retained_digest.finalize().into();
    if deleted_hash != retained_hash {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "待删文件与保留副本的内容摘要不一致。",
        ));
    }
    let deleted_after = probe_file(&deleted_handle)?;
    let retained_after = probe_file(&retained_handle)?;
    require_identity(&deleted_after, &deleted_before)?;
    require_identity(&retained_after, &retained_before)?;
    let deleted_reopened = open_read_only(&deleted_path)?;
    let retained_reopened = open_read_only(&retained_path)?;
    require_identity(&probe_file(&deleted_reopened)?, &deleted_before)?;
    require_identity(&probe_file(&retained_reopened)?, &retained_before)?;
    Ok(StablePairObservation {
        deleted: StableHashObservation {
            before: deleted_before,
            after: deleted_after,
            hash: deleted_hash,
        },
        retained: StableHashObservation {
            before: retained_before,
            after: retained_after,
            hash: retained_hash,
        },
    })
}

fn safe_file(root: &Path, relative: &str) -> Result<PathBuf, DomainError> {
    let path = resolve_contained_path(root, Path::new(relative))?;
    if path_is_reparse(&path)? || !path.is_file() {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "重复检测候选不是受约束 Root 内的普通文件。",
        ));
    }
    Ok(path)
}

fn hash_file(file: &mut std::fs::File, cancelled: &AtomicBool) -> Result<[u8; 32], DomainError> {
    file.seek(SeekFrom::Start(0)).map_err(read_error)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(cancelled_error());
        }
        let count = file.read(&mut buffer).map_err(read_error)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}

fn require_identity(actual: &FileProbe, expected: &FileProbe) -> Result<(), DomainError> {
    if actual == expected {
        Ok(())
    } else {
        Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "重复检测期间文件身份发生变化，未发布该证据。",
        ))
    }
}

fn cancelled_error() -> DomainError {
    DomainError::new(ErrorCode::InvalidTransition, "重复检测已安全取消。")
}

fn read_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "无法稳定读取重复检测候选。")
        .with_diagnostic(error.to_string())
}
