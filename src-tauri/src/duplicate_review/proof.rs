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
