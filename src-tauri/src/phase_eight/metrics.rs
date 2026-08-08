use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::Instant,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    io_scheduler::{IoPriority, IoScheduler, IoSchedulerMetrics},
    safety::PhaseEightRunRoot,
};

const IO_SAMPLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUFFER_BYTES: usize = 8 * 1024 * 1024;
const RANDOM_BLOCK_BYTES: usize = 4096;
const RANDOM_READ_COUNT: u64 = 16_384;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IoReport {
    report_version: u32,
    run_id: uuid::Uuid,
    sample_bytes_allocated: u64,
    sequential_write_bytes: u64,
    sequential_write_ms: u128,
    sequential_read_bytes: u64,
    sequential_read_ms: u128,
    random_read_bytes: u64,
    random_read_ms: u128,
    random_read_unbuffered: bool,
    content_sha256: String,
    random_read_checksum: u64,
    allocated_bytes_before: u64,
    allocated_bytes_after: u64,
    free_bytes_before: u64,
    scheduler: IoSchedulerMetrics,
}

pub fn run(root: &PhaseEightRunRoot) -> Result<IoReport, DomainError> {
    root.verify_current_identity()?;
    root.ensure_allocation_within_limit(IO_SAMPLE_BYTES)?;
    let free_bytes_before = root.ensure_disk_reserve(IO_SAMPLE_BYTES)?;
    let allocated_bytes_before = root.allocated_bytes()?;
    let path = root.resolve_owned(Path::new("io-sample-unbuffered-v2.bin"))?;
    let scheduler = IoScheduler::for_rotational();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    fill_pattern(&mut buffer);

    let write_started = Instant::now();
    {
        let _permit = scheduler.acquire(IoPriority::ImportCurrentFile, || false)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error)?;
        let mut remaining = IO_SAMPLE_BYTES;
        while remaining > 0 {
            let count = remaining.min(BUFFER_BYTES as u64) as usize;
            file.write_all(&buffer[..count]).map_err(io_error)?;
            remaining -= count as u64;
        }
        file.sync_all().map_err(io_error)?;
    }
    let sequential_write_ms = write_started.elapsed().as_millis();

    let read_started = Instant::now();
    let mut digest = Sha256::new();
    {
        let _permit = scheduler.acquire(IoPriority::BackgroundHashAndCache, || false)?;
        let mut file = OpenOptions::new()
            .read(true)
            .open(&path)
            .map_err(io_error)?;
        let mut remaining = IO_SAMPLE_BYTES;
        while remaining > 0 {
            let count = remaining.min(BUFFER_BYTES as u64) as usize;
            file.read_exact(&mut buffer[..count]).map_err(io_error)?;
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
    }
    let sequential_read_ms = read_started.elapsed().as_millis();

    let random_started = Instant::now();
    let random_read_checksum;
    {
        let _permit = scheduler.acquire(IoPriority::CurrentPreview, || false)?;
        random_read_checksum = random_read_unbuffered(&path)?;
    }
    let random_read_ms = random_started.elapsed().as_millis();
    let allocated_bytes_after = root.allocated_bytes()?;
    root.ensure_allocation_within_limit(0)?;

    Ok(IoReport {
        report_version: 1,
        run_id: root.run_id(),
        sample_bytes_allocated: IO_SAMPLE_BYTES,
        sequential_write_bytes: IO_SAMPLE_BYTES,
        sequential_write_ms,
        sequential_read_bytes: IO_SAMPLE_BYTES,
        sequential_read_ms,
        random_read_bytes: RANDOM_READ_COUNT * RANDOM_BLOCK_BYTES as u64,
        random_read_ms,
        random_read_unbuffered: cfg!(windows),
        content_sha256: hex::encode(digest.finalize()),
        random_read_checksum,
        allocated_bytes_before,
        allocated_bytes_after,
        free_bytes_before,
        scheduler: scheduler.metrics()?,
    })
}

#[cfg(windows)]
fn random_read_unbuffered(path: &Path) -> Result<u64, DomainError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_NO_BUFFERING, FILE_FLAG_RANDOM_ACCESS,
    };

    #[repr(C, align(4096))]
    struct AlignedBlock([u8; RANDOM_BLOCK_BYTES]);

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_RANDOM_ACCESS);
    let mut file = options.open(path).map_err(io_error)?;
    let mut block = AlignedBlock([0; RANDOM_BLOCK_BYTES]);
    let block_count = IO_SAMPLE_BYTES / RANDOM_BLOCK_BYTES as u64;
    let mut checksum = 0_u64;
    for index in 0..RANDOM_READ_COUNT {
        let block_index = index.wrapping_mul(1_103_515_245).wrapping_add(12_345) % block_count;
        file.seek(SeekFrom::Start(block_index * RANDOM_BLOCK_BYTES as u64))
            .map_err(io_error)?;
        file.read_exact(&mut block.0).map_err(io_error)?;
        checksum = checksum.wrapping_add(u64::from(block.0[index as usize % block.0.len()]));
    }
    Ok(checksum)
}

#[cfg(not(windows))]
fn random_read_unbuffered(path: &Path) -> Result<u64, DomainError> {
    let mut file = OpenOptions::new().read(true).open(path).map_err(io_error)?;
    let mut block = [0_u8; RANDOM_BLOCK_BYTES];
    let block_count = IO_SAMPLE_BYTES / RANDOM_BLOCK_BYTES as u64;
    let mut checksum = 0_u64;
    for index in 0..RANDOM_READ_COUNT {
        let block_index = index.wrapping_mul(1_103_515_245).wrapping_add(12_345) % block_count;
        file.seek(SeekFrom::Start(block_index * RANDOM_BLOCK_BYTES as u64))
            .map_err(io_error)?;
        file.read_exact(&mut block).map_err(io_error)?;
        checksum = checksum.wrapping_add(u64::from(block[index as usize % block.len()]));
    }
    Ok(checksum)
}

fn fill_pattern(buffer: &mut [u8]) {
    let mut value = 0x9e37_79b9_u32;
    for byte in buffer {
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        *byte = value as u8;
    }
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::CapabilityDenied, "阶段 8 实际 I/O 样本失败。")
        .with_diagnostic(error.to_string())
}
