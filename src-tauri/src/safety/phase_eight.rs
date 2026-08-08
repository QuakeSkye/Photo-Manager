use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use uuid::Uuid;

use crate::{
    domain::{DomainError, ErrorCode},
    roots::{DirectoryProbe, FilesystemKind, path_is_reparse, probe_directory},
};

pub const PHASE_EIGHT_PARENT: &str = r"G:\test";
pub const PHASE_EIGHT_MAX_ALLOCATED_BYTES: u64 = 300_000_000_000;
pub const PHASE_EIGHT_MIN_FREE_RESERVE_BYTES: u64 = 100_000_000_000;

const RUNS_DIRECTORY: &str = "local-photo-manager-phase8";
const RUN_PREFIX: &str = "fpm-phase8-run-";
const MARKER_FILE: &str = ".fuji-photo-manager-phase8-run-v1";
const MARKER_FORMAT: &str = "phase-eight-run-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseEightMediaProfile {
    pub incurs_seek_penalty: bool,
    pub device_query_succeeded: bool,
}

#[derive(Debug)]
pub struct PhaseEightRunRoot {
    path: PathBuf,
    run_id: Uuid,
    parent_path: PathBuf,
    parent_volume_identity: Vec<u8>,
    parent_directory_file_id: Vec<u8>,
    run_volume_identity: Vec<u8>,
    run_directory_file_id: Vec<u8>,
}

impl PhaseEightRunRoot {
    pub fn probe_allowed_parent_media() -> Result<PhaseEightMediaProfile, DomainError> {
        let parent = validate_parent(Path::new(PHASE_EIGHT_PARENT), true)?;
        query_seek_penalty(&parent.absolute_path)
    }

    pub fn create() -> Result<Self, DomainError> {
        Self::create_under(Path::new(PHASE_EIGHT_PARENT), true)
    }

    pub fn verify(path: &Path) -> Result<Self, DomainError> {
        Self::verify_under(path, Path::new(PHASE_EIGHT_PARENT), true)
    }

    fn create_under(parent: &Path, require_production_parent: bool) -> Result<Self, DomainError> {
        let parent_probe = validate_parent(parent, require_production_parent)?;
        let runs = parent_probe.absolute_path.join(RUNS_DIRECTORY);
        if !runs.exists() {
            fs::create_dir(&runs).map_err(phase_eight_io_error)?;
        }
        validate_directory(&runs)?;

        let run_id = Uuid::new_v4();
        let path = runs.join(format!("{RUN_PREFIX}{run_id}"));
        fs::create_dir(&path).map_err(phase_eight_io_error)?;
        let run_probe = validate_directory(&path)?;
        let marker = encode_marker(run_id, &parent_probe, &run_probe)?;
        fs::write(path.join(MARKER_FILE), marker).map_err(phase_eight_io_error)?;

        Self::verify_under(
            &path,
            &parent_probe.absolute_path,
            require_production_parent,
        )
    }

    fn verify_under(
        path: &Path,
        expected_parent: &Path,
        require_production_parent: bool,
    ) -> Result<Self, DomainError> {
        let parent_probe = validate_parent(expected_parent, require_production_parent)?;
        let run_probe = validate_directory(path)?;
        let expected_runs = parent_probe.absolute_path.join(RUNS_DIRECTORY);
        let actual_parent = run_probe
            .absolute_path
            .parent()
            .ok_or_else(unsafe_run_root)?;
        if !path_keys_equal(actual_parent, &expected_runs) {
            return Err(unsafe_run_root());
        }

        let directory_name = run_probe
            .absolute_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(unsafe_run_root)?;
        let run_id = directory_name
            .strip_prefix(RUN_PREFIX)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(unsafe_run_root)?;
        let marker = fs::read_to_string(run_probe.absolute_path.join(MARKER_FILE))
            .map_err(|_| unsafe_run_root())?;
        let fields = decode_marker(&marker)?;

        require_marker(&fields, "format", MARKER_FORMAT)?;
        require_marker(&fields, "run_id", &run_id.to_string())?;
        require_marker(
            &fields,
            "max_allocated_bytes",
            &PHASE_EIGHT_MAX_ALLOCATED_BYTES.to_string(),
        )?;
        require_marker(
            &fields,
            "parent_path_key",
            &hex::encode(&parent_probe.absolute_path_key),
        )?;
        require_marker(
            &fields,
            "parent_volume_identity",
            &hex::encode(&parent_probe.volume_identity_key),
        )?;
        require_marker(
            &fields,
            "parent_directory_file_id",
            &hex::encode(required_file_id(&parent_probe)?),
        )?;
        require_marker(
            &fields,
            "run_volume_identity",
            &hex::encode(&run_probe.volume_identity_key),
        )?;
        require_marker(
            &fields,
            "run_directory_file_id",
            &hex::encode(required_file_id(&run_probe)?),
        )?;

        let parent_directory_file_id = required_file_id(&parent_probe)?.to_vec();
        let run_directory_file_id = required_file_id(&run_probe)?.to_vec();
        Ok(Self {
            path: run_probe.absolute_path,
            run_id,
            parent_path: parent_probe.absolute_path,
            parent_volume_identity: parent_probe.volume_identity_key,
            parent_directory_file_id,
            run_volume_identity: run_probe.volume_identity_key,
            run_directory_file_id,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn resolve_owned(&self, relative: &Path) -> Result<PathBuf, DomainError> {
        self.verify_current_identity()?;
        crate::roots::resolve_contained_path(&self.path, relative)
    }

    pub const fn run_id(&self) -> Uuid {
        self.run_id
    }

    pub fn verify_current_identity(&self) -> Result<(), DomainError> {
        let parent = validate_directory(&self.parent_path)?;
        let run = validate_directory(&self.path)?;
        if parent.volume_identity_key != self.parent_volume_identity
            || required_file_id(&parent)? != self.parent_directory_file_id
            || run.volume_identity_key != self.run_volume_identity
            || required_file_id(&run)? != self.run_directory_file_id
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "阶段 8 测试 Root 身份已经变化，已停止测试。",
            ));
        }
        Ok(())
    }

    pub fn allocated_bytes(&self) -> Result<u64, DomainError> {
        self.verify_current_identity()?;
        allocated_tree_size(&self.path)
    }

    pub fn ensure_allocation_within_limit(
        &self,
        additional_bytes: u64,
    ) -> Result<u64, DomainError> {
        let allocated = self.allocated_bytes()?;
        if allocated
            .checked_add(additional_bytes)
            .is_none_or(|total| total > PHASE_EIGHT_MAX_ALLOCATED_BYTES)
        {
            return Err(DomainError::new(
                ErrorCode::CapabilityDenied,
                "阶段 8 测试将超过 300 GB 实际占用上限，已停止。",
            ));
        }
        Ok(allocated)
    }

    pub fn ensure_disk_reserve(&self, incoming_bytes: u64) -> Result<u64, DomainError> {
        self.verify_current_identity()?;
        let available = available_free_bytes(&self.path)?;
        let required = incoming_bytes
            .checked_add(PHASE_EIGHT_MIN_FREE_RESERVE_BYTES)
            .ok_or_else(|| {
                DomainError::new(ErrorCode::CapabilityDenied, "阶段 8 磁盘余量计算溢出。")
            })?;
        if available < required {
            return Err(DomainError::new(
                ErrorCode::CapabilityDenied,
                "G 盘无法同时容纳本次阶段 8 写入和 100 GB 安全余量。",
            )
            .with_diagnostic(format!("available={available}, required={required}")));
        }
        Ok(available)
    }

    pub fn media_profile(&self) -> Result<PhaseEightMediaProfile, DomainError> {
        self.verify_current_identity()?;
        query_seek_penalty(&self.path)
    }
}

fn validate_parent(
    parent: &Path,
    require_production_parent: bool,
) -> Result<DirectoryProbe, DomainError> {
    let probe = validate_directory(parent)?;
    if require_production_parent
        && !path_keys_equal(&probe.absolute_path, Path::new(PHASE_EIGHT_PARENT))
    {
        return Err(DomainError::new(
            ErrorCode::TestRootRequired,
            "阶段 8 测试只允许在 G:\\test 下创建专属 Root。",
        ));
    }
    if require_production_parent {
        let available = available_free_bytes(&probe.absolute_path)?;
        let required = PHASE_EIGHT_MAX_ALLOCATED_BYTES + PHASE_EIGHT_MIN_FREE_RESERVE_BYTES;
        if available < required {
            return Err(DomainError::new(
                ErrorCode::CapabilityDenied,
                "G 盘不足以保留 300 GB 测试额度和 100 GB 安全余量。",
            )
            .with_diagnostic(format!("available={available}, required={required}")));
        }
    }
    Ok(probe)
}

#[cfg(windows)]
fn available_free_bytes(path: &Path) -> Result<u64, DomainError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut available = 0_u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(phase_eight_io_error(std::io::Error::last_os_error()))
    } else {
        Ok(available)
    }
}

#[cfg(not(windows))]
fn available_free_bytes(_path: &Path) -> Result<u64, DomainError> {
    Err(DomainError::new(
        ErrorCode::UnsupportedMedia,
        "阶段 8 磁盘余量验证仅支持 Windows。",
    ))
}

fn validate_directory(path: &Path) -> Result<DirectoryProbe, DomainError> {
    let probe = probe_directory(path)?;
    if probe.filesystem_kind != FilesystemKind::Ntfs
        || !probe.strong_identity
        || probe.directory_file_id.is_none()
        || path_is_reparse(&probe.absolute_path)?
    {
        return Err(DomainError::new(
            ErrorCode::TestRootRequired,
            "阶段 8 测试 Root 必须是身份可证明的 NTFS 普通目录。",
        ));
    }
    Ok(probe)
}

fn required_file_id(probe: &DirectoryProbe) -> Result<&[u8], DomainError> {
    probe
        .directory_file_id
        .as_deref()
        .ok_or_else(unsafe_run_root)
}

fn encode_marker(
    run_id: Uuid,
    parent: &DirectoryProbe,
    run: &DirectoryProbe,
) -> Result<String, DomainError> {
    Ok(format!(
        "format={MARKER_FORMAT}\nrun_id={run_id}\nmax_allocated_bytes={PHASE_EIGHT_MAX_ALLOCATED_BYTES}\nparent_path_key={}\nparent_volume_identity={}\nparent_directory_file_id={}\nrun_volume_identity={}\nrun_directory_file_id={}\n",
        hex::encode(&parent.absolute_path_key),
        hex::encode(&parent.volume_identity_key),
        hex::encode(required_file_id(parent)?),
        hex::encode(&run.volume_identity_key),
        hex::encode(required_file_id(run)?),
    ))
}

fn decode_marker(marker: &str) -> Result<BTreeMap<&str, &str>, DomainError> {
    let mut fields = BTreeMap::new();
    for line in marker.lines() {
        let (key, value) = line.split_once('=').ok_or_else(unsafe_run_root)?;
        if key.is_empty() || value.is_empty() || fields.insert(key, value).is_some() {
            return Err(unsafe_run_root());
        }
    }
    if fields.len() != 8 {
        return Err(unsafe_run_root());
    }
    Ok(fields)
}

fn require_marker(
    fields: &BTreeMap<&str, &str>,
    key: &str,
    expected: &str,
) -> Result<(), DomainError> {
    if fields.get(key).copied() != Some(expected) {
        return Err(unsafe_run_root());
    }
    Ok(())
}

fn path_keys_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

fn allocated_tree_size(root: &Path) -> Result<u64, DomainError> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(phase_eight_io_error)? {
            let entry = entry.map_err(phase_eight_io_error)?;
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(phase_eight_io_error(error)),
            };
            if metadata_is_reparse(&metadata) {
                return Err(DomainError::new(
                    ErrorCode::UnsafePath,
                    "阶段 8 测试 Root 中出现 reparse point，已停止。",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                total = total
                    .checked_add(allocated_file_size(&path, metadata.len())?)
                    .ok_or_else(|| {
                        DomainError::new(
                            ErrorCode::CapabilityDenied,
                            "阶段 8 实际占用统计溢出，已停止。",
                        )
                    })?;
                if total > PHASE_EIGHT_MAX_ALLOCATED_BYTES {
                    return Err(DomainError::new(
                        ErrorCode::CapabilityDenied,
                        "阶段 8 测试已超过 300 GB 实际占用上限，已停止。",
                    ));
                }
            } else {
                return Err(DomainError::new(
                    ErrorCode::UnsafePath,
                    "阶段 8 测试 Root 中出现不受支持的文件系统对象。",
                ));
            }
        }
    }
    Ok(total)
}

#[cfg(windows)]
fn allocated_file_size(path: &Path, _logical_size: u64) -> Result<u64, DomainError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{GetLastError, NO_ERROR},
        Storage::FileSystem::GetCompressedFileSizeW,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut high = 0_u32;
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
    if low == u32::MAX {
        let error = unsafe { GetLastError() };
        if error == 2 || error == 3 {
            return Ok(0);
        }
        if error != NO_ERROR {
            return Err(phase_eight_io_error(std::io::Error::from_raw_os_error(
                error as i32,
            )));
        }
    }
    Ok((u64::from(high) << 32) | u64::from(low))
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0000_0400 != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(not(windows))]
fn allocated_file_size(_path: &Path, logical_size: u64) -> Result<u64, DomainError> {
    Ok(logical_size)
}

#[cfg(windows)]
fn query_seek_penalty(path: &Path) -> Result<PhaseEightMediaProfile, DomainError> {
    use std::{ffi::OsString, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            IO::DeviceIoControl,
            Ioctl::{
                DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY,
                PropertyStandardQuery, STORAGE_PROPERTY_QUERY, StorageDeviceSeekPenaltyProperty,
            },
        },
    };

    let drive = path
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .filter(|component| component.len() == 2 && component.ends_with(':'))
        .ok_or_else(|| {
            DomainError::new(
                ErrorCode::UnsupportedMedia,
                "无法确定阶段 8 测试卷的设备路径。",
            )
        })?;
    let device = OsString::from(format!(r"\\.\{drive}"))
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            device.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(DomainError::new(
            ErrorCode::UnsupportedMedia,
            "无法读取 G 盘机械介质属性，机械硬盘校准门保持关闭。",
        )
        .with_diagnostic(std::io::Error::last_os_error().to_string()));
    }

    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut descriptor = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
    let mut returned = 0_u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast(),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            (&raw mut descriptor).cast(),
            std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 || returned < std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32 {
        return Err(DomainError::new(
            ErrorCode::UnsupportedMedia,
            "无法证明 G 盘是否为机械介质，机械硬盘校准门保持关闭。",
        )
        .with_diagnostic(std::io::Error::last_os_error().to_string()));
    }

    Ok(PhaseEightMediaProfile {
        incurs_seek_penalty: descriptor.IncursSeekPenalty,
        device_query_succeeded: true,
    })
}

#[cfg(not(windows))]
fn query_seek_penalty(_path: &Path) -> Result<PhaseEightMediaProfile, DomainError> {
    Err(DomainError::new(
        ErrorCode::UnsupportedMedia,
        "机械介质验证只支持 Windows。",
    ))
}

fn phase_eight_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::TestRootRequired,
        "无法安全读取或创建阶段 8 测试 Root。",
    )
    .with_diagnostic(error.to_string())
}

fn unsafe_run_root() -> DomainError {
    DomainError::new(
        ErrorCode::TestRootRequired,
        "阶段 8 测试只允许使用应用创建且身份匹配的专属 G 盘 Root。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PhaseEightRunRoot) {
        let parent = tempfile::tempdir().unwrap();
        let root = PhaseEightRunRoot::create_under(parent.path(), false).unwrap();
        (parent, root)
    }

    #[test]
    fn phase_eight_root_binds_marker_parent_volume_and_directory_identity() {
        let (_parent, root) = fixture();
        let verified = PhaseEightRunRoot::verify_under(
            root.path(),
            root.path().parent().unwrap().parent().unwrap(),
            false,
        )
        .unwrap();

        assert_eq!(verified.run_id(), root.run_id());
        assert_eq!(verified.path(), root.path());
        assert!(verified.allocated_bytes().unwrap() > 0);
    }

    #[test]
    fn copied_or_modified_marker_is_rejected() {
        let (_parent, root) = fixture();
        fs::write(
            root.path().join(MARKER_FILE),
            b"format=phase-eight-run-v1\n",
        )
        .unwrap();

        assert!(
            PhaseEightRunRoot::verify_under(
                root.path(),
                root.path().parent().unwrap().parent().unwrap(),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn arbitrary_parent_and_path_escape_are_rejected() {
        let parent = tempfile::tempdir().unwrap();
        assert!(PhaseEightRunRoot::create_under(parent.path(), true).is_err());

        let (_fixture_parent, root) = fixture();
        let unrelated = tempfile::tempdir().unwrap();
        assert!(PhaseEightRunRoot::verify_under(root.path(), unrelated.path(), false).is_err());
    }

    #[test]
    fn allocation_limit_is_checked_with_overflow_protection() {
        let (_parent, root) = fixture();
        assert!(
            root.ensure_allocation_within_limit(PHASE_EIGHT_MAX_ALLOCATED_BYTES)
                .is_err()
        );
        assert!(root.ensure_allocation_within_limit(u64::MAX).is_err());
    }

    #[test]
    #[ignore = "reads the user-designated G: device property without writing"]
    fn allowed_g_parent_proves_seek_penalty_property() {
        let profile = PhaseEightRunRoot::probe_allowed_parent_media().unwrap();
        println!("{profile:?}");
        assert!(profile.device_query_succeeded);
    }
}
