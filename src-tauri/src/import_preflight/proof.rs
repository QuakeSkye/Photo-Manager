use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    roots::{
        DirectoryProbe, FileProbe, open_read_only, probe_directory, probe_file,
        resolve_contained_path,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuditSide {
    Source,
    Target,
}

#[derive(Debug, Default)]
pub(crate) struct ProcessIoAudit {
    source_read_opens: AtomicU64,
    source_directory_enumerations: AtomicU64,
    source_metadata_reads: AtomicU64,
    source_full_hashes: AtomicU64,
    source_byte_comparisons: AtomicU64,
    source_path_reopens: AtomicU64,
    target_read_opens: AtomicU64,
    target_directory_enumerations: AtomicU64,
    target_metadata_reads: AtomicU64,
    target_full_hashes: AtomicU64,
    target_byte_comparisons: AtomicU64,
    target_path_reopens: AtomicU64,
    source_write_requests: AtomicU64,
    target_write_requests: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIoAuditReport {
    pub scope: &'static str,
    pub source_read_opens: u64,
    pub source_directory_enumerations: u64,
    pub source_metadata_reads: u64,
    pub source_full_hashes: u64,
    pub source_byte_comparisons: u64,
    pub source_path_reopens: u64,
    pub target_read_opens: u64,
    pub target_directory_enumerations: u64,
    pub target_metadata_reads: u64,
    pub target_full_hashes: u64,
    pub target_byte_comparisons: u64,
    pub target_path_reopens: u64,
    pub source_write_requests: u64,
    pub target_write_requests: u64,
}

impl ProcessIoAudit {
    pub(crate) fn snapshot(&self) -> ProcessIoAuditReport {
        let load = |value: &AtomicU64| value.load(Ordering::Relaxed);
        ProcessIoAuditReport {
            scope: "application-process/import-preflight",
            source_read_opens: load(&self.source_read_opens),
            source_directory_enumerations: load(&self.source_directory_enumerations),
            source_metadata_reads: load(&self.source_metadata_reads),
            source_full_hashes: load(&self.source_full_hashes),
            source_byte_comparisons: load(&self.source_byte_comparisons),
            source_path_reopens: load(&self.source_path_reopens),
            target_read_opens: load(&self.target_read_opens),
            target_directory_enumerations: load(&self.target_directory_enumerations),
            target_metadata_reads: load(&self.target_metadata_reads),
            target_full_hashes: load(&self.target_full_hashes),
            target_byte_comparisons: load(&self.target_byte_comparisons),
            target_path_reopens: load(&self.target_path_reopens),
            source_write_requests: load(&self.source_write_requests),
            target_write_requests: load(&self.target_write_requests),
        }
    }

    pub(crate) fn enumerate(&self, side: AuditSide) {
        match side {
            AuditSide::Source => &self.source_directory_enumerations,
            AuditSide::Target => &self.target_directory_enumerations,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn read_open(&self, side: AuditSide, path_reopen: bool) {
        let counter = match side {
            AuditSide::Source => &self.source_read_opens,
            AuditSide::Target => &self.target_read_opens,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        if path_reopen {
            match side {
                AuditSide::Source => &self.source_path_reopens,
                AuditSide::Target => &self.target_path_reopens,
            }
            .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn metadata(&self, side: AuditSide) {
        match side {
            AuditSide::Source => &self.source_metadata_reads,
            AuditSide::Target => &self.target_metadata_reads,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn full_hash(&self, side: AuditSide) {
        match side {
            AuditSide::Source => &self.source_full_hashes,
            AuditSide::Target => &self.target_full_hashes,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn byte_compare(&self, side: AuditSide) {
        match side {
            AuditSide::Source => &self.source_byte_comparisons,
            AuditSide::Target => &self.target_byte_comparisons,
        }
        .fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct StableReadHandle {
    file: File,
    side: AuditSide,
}

impl StableReadHandle {
    pub(crate) fn open(
        path: &Path,
        side: AuditSide,
        audit: &ProcessIoAudit,
    ) -> Result<Self, DomainError> {
        audit.read_open(side, false);
        Ok(Self {
            file: open_read_only(path)?,
            side,
        })
    }

    fn reopen(path: &Path, side: AuditSide, audit: &ProcessIoAudit) -> Result<Self, DomainError> {
        audit.read_open(side, true);
        Ok(Self {
            file: open_read_only(path)?,
            side,
        })
    }

    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn probe(&self, audit: &ProcessIoAudit) -> Result<FileProbe, DomainError> {
        audit.metadata(self.side);
        probe_file(&self.file)
    }

    pub(crate) fn full_hash(&mut self, audit: &ProcessIoAudit) -> Result<[u8; 32], DomainError> {
        let side = self.side;
        hash_file(self.file_mut(), side, audit)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BoundPath {
    pub root_id: Vec<u8>,
    pub binding_id: Vec<u8>,
    pub root_path: PathBuf,
    pub relative_path: PathBuf,
    pub expected_root: DirectoryProbe,
}

impl BoundPath {
    pub(super) fn resolve(&self) -> Result<PathBuf, DomainError> {
        if self.root_id.len() != 16 || self.binding_id.len() != 16 {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "重复证明缺少稳定 RootBinding 身份。",
            ));
        }
        resolve_contained_path(&self.root_path, &self.relative_path)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DuplicateProof {
    pub source_hash: [u8; 32],
    pub target_hash: [u8; 32],
    pub source_before: FileProbe,
    pub source_after: FileProbe,
    pub target_before: FileProbe,
    pub target_after: FileProbe,
}

/// Keeps every handle used by a duplicate proof alive until the caller has
/// committed the rows that consume the proof.
pub(crate) struct DuplicateProofGuard {
    pub proof: DuplicateProof,
    _source_handle: StableReadHandle,
    _target_handle: StableReadHandle,
    _source_reopened: StableReadHandle,
    _target_reopened: StableReadHandle,
}

pub(crate) fn prove_duplicate(
    source: &BoundPath,
    target: &BoundPath,
    audit: &ProcessIoAudit,
) -> Result<Option<DuplicateProof>, DomainError> {
    prove_duplicate_with_hasher_and_commit(source, target, audit, hash_file, |proof| {
        Ok(proof.clone())
    })
}

pub(crate) fn prove_duplicate_guard(
    source: &BoundPath,
    target: &BoundPath,
    audit: &ProcessIoAudit,
) -> Result<Option<DuplicateProofGuard>, DomainError> {
    prove_duplicate_guard_with_hasher(source, target, audit, hash_file)
}

pub(crate) fn prove_duplicate_and_commit<T, F>(
    source: &BoundPath,
    target: &BoundPath,
    audit: &ProcessIoAudit,
    commit: F,
) -> Result<Option<T>, DomainError>
where
    F: FnOnce(&DuplicateProof) -> Result<T, DomainError>,
{
    prove_duplicate_with_hasher_and_commit(source, target, audit, hash_file, commit)
}

#[cfg(test)]
fn prove_duplicate_with_hasher<F>(
    source: &BoundPath,
    target: &BoundPath,
    audit: &ProcessIoAudit,
    hasher: F,
) -> Result<Option<DuplicateProof>, DomainError>
where
    F: FnMut(&mut File, AuditSide, &ProcessIoAudit) -> Result<[u8; 32], DomainError>,
{
    prove_duplicate_with_hasher_and_commit(source, target, audit, hasher, |proof| Ok(proof.clone()))
}

fn prove_duplicate_with_hasher_and_commit<T, H, C>(
    source: &BoundPath,
    target: &BoundPath,
    audit: &ProcessIoAudit,
    hasher: H,
    commit: C,
) -> Result<Option<T>, DomainError>
where
    H: FnMut(&mut File, AuditSide, &ProcessIoAudit) -> Result<[u8; 32], DomainError>,
    C: FnOnce(&DuplicateProof) -> Result<T, DomainError>,
{
    let guard = match prove_duplicate_guard_with_hasher(source, target, audit, hasher)? {
        Some(guard) => guard,
        None => return Ok(None),
    };
    commit(&guard.proof).map(Some)
}

fn prove_duplicate_guard_with_hasher<H>(
    source: &BoundPath,
    target: &BoundPath,
    audit: &ProcessIoAudit,
    mut hasher: H,
) -> Result<Option<DuplicateProofGuard>, DomainError>
where
    H: FnMut(&mut File, AuditSide, &ProcessIoAudit) -> Result<[u8; 32], DomainError>,
{
    verify_root_binding(&source.expected_root, &source.root_path)?;
    verify_root_binding(&target.expected_root, &target.root_path)?;
    let source_path = source.resolve()?;
    let target_path = target.resolve()?;
    let mut source_handle = StableReadHandle::open(&source_path, AuditSide::Source, audit)?;
    let mut target_handle = StableReadHandle::open(&target_path, AuditSide::Target, audit)?;
    let source_before = source_handle.probe(audit)?;
    let target_before = target_handle.probe(audit)?;
    if source_before.reparse_point
        || target_before.reparse_point
        || source_before.size_bytes != target_before.size_bytes
    {
        return Ok(None);
    }

    let source_hash = hasher(source_handle.file_mut(), AuditSide::Source, audit)?;
    let target_hash = hasher(target_handle.file_mut(), AuditSide::Target, audit)?;
    if source_hash != target_hash {
        return Ok(None);
    }
    if !byte_equal(source_handle.file_mut(), target_handle.file_mut(), audit)? {
        return Ok(None);
    }

    let source_after = source_handle.probe(audit)?;
    let target_after = target_handle.probe(audit)?;
    if source_before != source_after || target_before != target_after {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "重复比较期间文件发生变化，不能判定为已导入。",
        ));
    }

    // Keep both original comparison handles alive while proving that each planned path still
    // resolves to the same object. A readable old handle is not a path-binding proof.
    let source_reopened = StableReadHandle::reopen(&source_path, AuditSide::Source, audit)?;
    let target_reopened = StableReadHandle::reopen(&target_path, AuditSide::Target, audit)?;
    let source_reopened_probe = source_reopened.probe(audit)?;
    let target_reopened_probe = target_reopened.probe(audit)?;
    if source_reopened_probe.file_id.is_none()
        || target_reopened_probe.file_id.is_none()
        || source_reopened_probe.file_id != source_before.file_id
        || target_reopened_probe.file_id != target_before.file_id
        || source_reopened_probe.size_bytes != source_before.size_bytes
        || target_reopened_probe.size_bytes != target_before.size_bytes
        || source_reopened_probe.mtime_filetime != source_before.mtime_filetime
        || target_reopened_probe.mtime_filetime != target_before.mtime_filetime
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "计划路径不再指向重复比较开始时的文件。",
        ));
    }
    verify_root_binding(&source.expected_root, &source.root_path)?;
    verify_root_binding(&target.expected_root, &target.root_path)?;

    let proof = DuplicateProof {
        source_hash,
        target_hash,
        source_before,
        source_after,
        target_before,
        target_after,
    };
    Ok(Some(DuplicateProofGuard {
        proof,
        _source_handle: source_handle,
        _target_handle: target_handle,
        _source_reopened: source_reopened,
        _target_reopened: target_reopened,
    }))
}

fn verify_root_binding(expected: &DirectoryProbe, path: &Path) -> Result<(), DomainError> {
    let current = probe_directory(path)?;
    if current.volume_identity_key != expected.volume_identity_key
        || current.directory_file_id != expected.directory_file_id
        || current.absolute_path_key != expected.absolute_path_key
        || current.filesystem_kind != expected.filesystem_kind
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "Root 身份在只读预检期间发生变化。",
        ));
    }
    Ok(())
}

fn hash_file(
    file: &mut File,
    side: AuditSide,
    audit: &ProcessIoAudit,
) -> Result<[u8; 32], DomainError> {
    audit.full_hash(side);
    file.seek(SeekFrom::Start(0)).map_err(read_error)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(read_error)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}

fn byte_equal(
    source: &mut File,
    target: &mut File,
    audit: &ProcessIoAudit,
) -> Result<bool, DomainError> {
    audit.byte_compare(AuditSide::Source);
    audit.byte_compare(AuditSide::Target);
    source.seek(SeekFrom::Start(0)).map_err(read_error)?;
    target.seek(SeekFrom::Start(0)).map_err(read_error)?;
    let mut source_buffer = vec![0_u8; 1024 * 1024];
    let mut target_buffer = vec![0_u8; 1024 * 1024];
    loop {
        let source_count = source.read(&mut source_buffer).map_err(read_error)?;
        let target_count = target.read(&mut target_buffer).map_err(read_error)?;
        if source_count != target_count
            || source_buffer[..source_count] != target_buffer[..target_count]
        {
            return Ok(false);
        }
        if source_count == 0 {
            return Ok(true);
        }
    }
}

fn read_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::MediaUnreadable,
        "无法完整读取照片内容，已从可导入项目中排除。",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn bound(root: &Path, name: &str) -> BoundPath {
        BoundPath {
            root_id: vec![1; 16],
            binding_id: vec![2; 16],
            root_path: root.to_path_buf(),
            relative_path: PathBuf::from(name),
            expected_root: probe_directory(root).unwrap(),
        }
    }

    #[test]
    fn simulated_hash_collision_still_requires_byte_equality() {
        let source_root = tempfile::tempdir().unwrap();
        let target_root = tempfile::tempdir().unwrap();
        fs::write(source_root.path().join("same.JPG"), b"AAAA").unwrap();
        fs::write(target_root.path().join("same.JPG"), b"BBBB").unwrap();
        let audit = ProcessIoAudit::default();
        let forced_collision = |_file: &mut File, side: AuditSide, audit: &ProcessIoAudit| {
            audit.full_hash(side);
            Ok([7_u8; 32])
        };

        let result = prove_duplicate_with_hasher(
            &bound(source_root.path(), "same.JPG"),
            &bound(target_root.path(), "same.JPG"),
            &audit,
            forced_collision,
        )
        .unwrap();

        assert!(result.is_none());
        let report = audit.snapshot();
        assert_eq!(report.source_byte_comparisons, 1);
        assert_eq!(report.target_byte_comparisons, 1);
        assert_eq!(report.source_write_requests, 0);
        assert_eq!(report.target_write_requests, 0);
    }
}
