use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    roots::{FileProbe, probe_file},
    safety::{FaultInjector, FaultPoint},
};

pub(crate) const COPY_BUFFER_BYTES: usize = 1024 * 1024;

pub(crate) struct PartialWriteHandle {
    file: File,
}

impl PartialWriteHandle {
    pub(crate) fn create_new(path: &Path, faults: &FaultInjector) -> Result<Self, DomainError> {
        faults.checkpoint(FaultPoint::Win32BeforeCall)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        configure_exclusive_mutating_handle(&mut options);
        let file = options.open(path).map_err(write_error)?;
        faults.checkpoint(FaultPoint::Win32AfterCall)?;
        Ok(Self { file })
    }

    pub(crate) fn probe(&self) -> Result<FileProbe, DomainError> {
        probe_file(&self.file)
    }

    pub(crate) fn copy_from<F>(
        &mut self,
        source: &mut File,
        expected_bytes: u64,
        mut cancelled: F,
    ) -> Result<[u8; 32], DomainError>
    where
        F: FnMut() -> bool,
    {
        source.seek(SeekFrom::Start(0)).map_err(read_error)?;
        self.file.seek(SeekFrom::Start(0)).map_err(write_error)?;
        let mut digest = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            if cancelled() {
                return Err(DomainError::new(
                    ErrorCode::InvalidTransition,
                    "已请求取消剩余导入。",
                ));
            }
            let count = source.read(&mut buffer).map_err(read_error)?;
            if count == 0 {
                break;
            }
            self.file.write_all(&buffer[..count]).map_err(write_error)?;
            digest.update(&buffer[..count]);
            copied = copied.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            if copied > expected_bytes {
                return Err(DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "来源文件在复制期间增长，已停止当前导入。",
                ));
            }
        }
        if copied != expected_bytes {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "来源文件大小与已确认计划不一致。",
            ));
        }
        Ok(digest.finalize().into())
    }

    pub(crate) fn flush(self, faults: &FaultInjector) -> Result<(), DomainError> {
        faults.checkpoint(FaultPoint::Win32BeforeCall)?;
        self.file.sync_all().map_err(write_error)?;
        faults.checkpoint(FaultPoint::Win32AfterCall)?;
        Ok(())
    }
}

pub(crate) struct VerifiedPartialHandle {
    file: File,
}

impl VerifiedPartialHandle {
    pub(crate) fn open(path: &Path) -> Result<Self, DomainError> {
        let mut options = OpenOptions::new();
        options.read(true);
        configure_verify_publish_handle(&mut options);
        Ok(Self {
            file: options.open(path).map_err(write_error)?,
        })
    }

    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn probe(&self) -> Result<FileProbe, DomainError> {
        probe_file(&self.file)
    }

    pub(crate) fn set_last_write_time(&self, filetime: i64) -> Result<(), DomainError> {
        set_last_write_time(&self.file, filetime)
    }

    pub(crate) fn publish_no_replace(
        &self,
        final_path: &Path,
        faults: &FaultInjector,
    ) -> Result<(), DomainError> {
        publish_no_replace(&self.file, final_path, faults)
    }

    pub(crate) fn delete_owned(self, faults: &FaultInjector) -> Result<(), DomainError> {
        delete_by_handle(&self.file, faults)
    }
}

pub(crate) fn hash_and_compare(
    source: &mut File,
    partial: &mut File,
) -> Result<([u8; 32], [u8; 32], bool), DomainError> {
    let source_hash = hash(source)?;
    let partial_hash = hash(partial)?;
    source.seek(SeekFrom::Start(0)).map_err(read_error)?;
    partial.seek(SeekFrom::Start(0)).map_err(read_error)?;
    let mut source_buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut partial_buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let source_count = source.read(&mut source_buffer).map_err(read_error)?;
        let partial_count = partial.read(&mut partial_buffer).map_err(read_error)?;
        if source_count != partial_count
            || source_buffer[..source_count] != partial_buffer[..partial_count]
        {
            return Ok((source_hash, partial_hash, false));
        }
        if source_count == 0 {
            return Ok((source_hash, partial_hash, true));
        }
    }
}

fn hash(file: &mut File) -> Result<[u8; 32], DomainError> {
    file.seek(SeekFrom::Start(0)).map_err(read_error)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = file.read(&mut buffer).map_err(read_error)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}

#[cfg(windows)]
fn configure_exclusive_mutating_handle(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::DELETE;
    options
        .share_mode(0)
        .access_mode(0x8000_0000 | 0x4000_0000 | DELETE);
}

#[cfg(not(windows))]
fn configure_exclusive_mutating_handle(_options: &mut OpenOptions) {}

#[cfg(windows)]
fn configure_verify_publish_handle(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_WRITE_ATTRIBUTES};
    // FILE_SHARE_READ only: the verified object cannot be written, renamed, or deleted by a
    // newly opened handle until its no-replace publish fact commits.
    options
        .share_mode(0x0000_0001)
        .access_mode(0x8000_0000 | FILE_WRITE_ATTRIBUTES | DELETE);
}

#[cfg(not(windows))]
fn configure_verify_publish_handle(_options: &mut OpenOptions) {}

#[cfg(windows)]
fn set_last_write_time(file: &File, value: i64) -> Result<(), DomainError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::FILETIME, Storage::FileSystem::SetFileTime};
    let raw = u64::try_from(value)
        .map_err(|_| DomainError::new(ErrorCode::IdentityMismatch, "来源文件时间无效。"))?;
    let filetime = FILETIME {
        dwLowDateTime: raw as u32,
        dwHighDateTime: (raw >> 32) as u32,
    };
    let ok = unsafe {
        SetFileTime(
            file.as_raw_handle() as _,
            std::ptr::null(),
            std::ptr::null(),
            &filetime,
        )
    };
    if ok == 0 {
        return Err(write_error(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_last_write_time(_file: &File, _value: i64) -> Result<(), DomainError> {
    Err(DomainError::new(
        ErrorCode::CapabilityDenied,
        "阶段 6 的真实文件动作只在 Windows NTFS 测试 Root 上开放。",
    ))
}

#[cfg(windows)]
fn publish_no_replace(
    file: &File,
    final_path: &Path,
    faults: &FaultInjector,
) -> Result<(), DomainError> {
    use std::{os::windows::ffi::OsStrExt, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FILE_RENAME_INFO_0, FileRenameInfo, SetFileInformationByHandle,
    };

    let name = final_path.as_os_str().encode_wide().collect::<Vec<_>>();
    let header = std::mem::size_of::<FILE_RENAME_INFO>() - std::mem::size_of::<u16>();
    let bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| header.checked_add(length))
        .ok_or_else(|| DomainError::new(ErrorCode::UnsafePath, "发布路径过长。"))?;
    let mut buffer = vec![0_u8; bytes];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous = FILE_RENAME_INFO_0 {
            ReplaceIfExists: false,
        };
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = u32::try_from(name.len() * 2)
            .map_err(|_| DomainError::new(ErrorCode::UnsafePath, "发布路径过长。"))?;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
    }
    faults.checkpoint(FaultPoint::Win32BeforeCall)?;
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileRenameInfo,
            buffer.as_ptr().cast(),
            u32::try_from(buffer.len()).unwrap_or(u32::MAX),
        )
    };
    if ok == 0 {
        return Err(write_error(std::io::Error::last_os_error()));
    }
    faults.checkpoint(FaultPoint::Win32AfterCall)?;
    Ok(())
}

#[cfg(not(windows))]
fn publish_no_replace(
    _file: &File,
    _final_path: &Path,
    _faults: &FaultInjector,
) -> Result<(), DomainError> {
    Err(DomainError::new(
        ErrorCode::CapabilityDenied,
        "阶段 6 的句柄级发布只在 Windows NTFS 测试 Root 上开放。",
    ))
}

#[cfg(windows)]
fn delete_by_handle(file: &File, faults: &FaultInjector) -> Result<(), DomainError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    faults.checkpoint(FaultPoint::Win32BeforeCall)?;
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>()).unwrap(),
        )
    };
    if ok == 0 {
        return Err(write_error(std::io::Error::last_os_error()));
    }
    faults.checkpoint(FaultPoint::Win32AfterCall)?;
    Ok(())
}

#[cfg(not(windows))]
fn delete_by_handle(_file: &File, _faults: &FaultInjector) -> Result<(), DomainError> {
    Err(DomainError::new(
        ErrorCode::CapabilityDenied,
        "阶段 6 的句柄级 partial 清理只在 Windows NTFS 测试 Root 上开放。",
    ))
}

#[cfg(windows)]
pub(crate) fn disk_space(path: &Path) -> Result<(u64, u64), DomainError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        return Err(write_error(std::io::Error::last_os_error()));
    }
    Ok((total, available))
}

#[cfg(not(windows))]
pub(crate) fn disk_space(_path: &Path) -> Result<(u64, u64), DomainError> {
    Err(DomainError::new(
        ErrorCode::CapabilityDenied,
        "阶段 6 空间门只在 Windows NTFS 测试 Root 上执行。",
    ))
}

fn read_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::MediaUnreadable, "无法完整读取来源照片。")
        .with_diagnostic(error.to_string())
}

fn write_error(error: std::io::Error) -> DomainError {
    let code = if error.raw_os_error() == Some(112) {
        ErrorCode::LeaseUnavailable
    } else if error.kind() == std::io::ErrorKind::AlreadyExists {
        ErrorCode::ReservationConflict
    } else {
        ErrorCode::Internal
    };
    DomainError::new(code, "测试 Fuji Root 文件动作未能安全完成。")
        .with_diagnostic(error.to_string())
}
