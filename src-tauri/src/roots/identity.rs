use std::{
    fs::{self, File, Metadata, OpenOptions},
    path::{Component, Path, PathBuf},
};

#[cfg(not(windows))]
use std::time::UNIX_EPOCH;

use crate::domain::{DomainError, ErrorCode};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemKind {
    Ntfs,
    ExFat,
    ReFs,
    Other,
}

impl FilesystemKind {
    pub const fn as_sql(self) -> &'static str {
        match self {
            Self::Ntfs => "NTFS",
            Self::ExFat => "exFAT",
            Self::ReFs => "ReFS",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryProbe {
    pub absolute_path: PathBuf,
    pub absolute_path_key: Vec<u8>,
    pub volume_identity_key: Vec<u8>,
    pub directory_file_id: Option<Vec<u8>>,
    pub filesystem_kind: FilesystemKind,
    pub strong_identity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProbe {
    pub file_id: Option<Vec<u8>>,
    pub size_bytes: u64,
    pub mtime_filetime: i64,
    pub hard_link_count: u64,
    pub reparse_point: bool,
}

pub fn path_key(path: &Path) -> Vec<u8> {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
        .into_bytes()
}

pub fn path_is_reparse(path: &Path) -> Result<bool, DomainError> {
    fs::symlink_metadata(path)
        .map(|metadata| is_reparse(&metadata))
        .map_err(root_io_error)
}

pub fn resolve_contained_path(root: &Path, relative: &Path) -> Result<PathBuf, DomainError> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "相对路径试图离开已验证 Root。",
        ));
    }
    let root = absolute_lexical(root)?;
    ensure_no_reparse_components(&root)?;
    let candidate = root.join(relative);
    let mut current = root;
    for component in relative.components() {
        if matches!(component, Component::CurDir) {
            continue;
        }
        current.push(component.as_os_str());
        if current.exists() && path_is_reparse(&current)? {
            return Err(DomainError::new(
                ErrorCode::UnsafePath,
                "文件路径包含 reparse point。",
            ));
        }
    }
    Ok(candidate)
}

pub fn probe_directory(path: &Path) -> Result<DirectoryProbe, DomainError> {
    let absolute = absolute_lexical(path)?;
    ensure_no_reparse_components(&absolute)?;
    let file = open_directory_read_only(&absolute)?;
    let metadata = file.metadata().map_err(root_io_error)?;
    if !metadata.is_dir() {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "选择的位置不是目录。",
        ));
    }
    let identity = file_handle_identity(&file, &metadata)?;
    let (filesystem_kind, volume_key, strong_identity) =
        volume_identity(&absolute, &file, &metadata)?;
    Ok(DirectoryProbe {
        absolute_path_key: path_key(&absolute),
        absolute_path: absolute,
        volume_identity_key: volume_key,
        directory_file_id: identity.file_id,
        filesystem_kind,
        strong_identity,
    })
}

pub fn probe_file(file: &File) -> Result<FileProbe, DomainError> {
    let metadata = file.metadata().map_err(root_io_error)?;
    if !metadata.is_file() {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "目标不是普通文件。",
        ));
    }
    file_handle_identity(file, &metadata)
}

pub fn open_read_only(path: &Path) -> Result<File, DomainError> {
    let mut options = OpenOptions::new();
    options.read(true);
    configure_read_sharing(&mut options);
    options.open(path).map_err(root_io_error)
}

fn open_directory_read_only(path: &Path) -> Result<File, DomainError> {
    let mut options = OpenOptions::new();
    options.read(true);
    configure_directory_open(&mut options);
    options.open(path).map_err(root_io_error)
}

#[cfg(windows)]
fn configure_read_sharing(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    // Keep the identity stable while parsing: other processes may read, but cannot replace,
    // write, or delete the file through a newly opened sharing handle.
    options.share_mode(0x0000_0001);
}

#[cfg(not(windows))]
fn configure_read_sharing(_options: &mut OpenOptions) {}

#[cfg(windows)]
fn configure_directory_open(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    options
        .share_mode(0x0000_0001)
        .custom_flags(0x0200_0000 | 0x0020_0000);
}

#[cfg(not(windows))]
fn configure_directory_open(_options: &mut OpenOptions) {}

fn absolute_lexical(path: &Path) -> Result<PathBuf, DomainError> {
    let raw = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(root_io_error)?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(DomainError::new(
                        ErrorCode::UnsafePath,
                        "目录路径越过了文件系统根目录。",
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

pub(crate) fn ensure_no_reparse_components(path: &Path) -> Result<(), DomainError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let metadata = fs::symlink_metadata(&current).map_err(root_io_error)?;
        if is_reparse(&metadata) {
            return Err(DomainError::new(
                ErrorCode::UnsafePath,
                "目录路径包含 reparse point，已停止。",
            )
            .with_diagnostic(current.display().to_string()));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn file_handle_identity(file: &File, metadata: &Metadata) -> Result<FileProbe, DomainError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return Err(root_io_error(std::io::Error::last_os_error()));
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let mut value = Vec::with_capacity(12);
    value.extend_from_slice(&info.dwVolumeSerialNumber.to_le_bytes());
    value.extend_from_slice(&index.to_le_bytes());
    Ok(FileProbe {
        file_id: Some(value),
        size_bytes: metadata.len(),
        mtime_filetime: mtime_value(metadata),
        hard_link_count: u64::from(info.nNumberOfLinks).max(1),
        reparse_point: is_reparse(metadata),
    })
}

#[cfg(unix)]
fn file_handle_identity(_file: &File, metadata: &Metadata) -> Result<FileProbe, DomainError> {
    use std::os::unix::fs::MetadataExt;
    let mut value = Vec::with_capacity(16);
    value.extend_from_slice(&metadata.dev().to_le_bytes());
    value.extend_from_slice(&metadata.ino().to_le_bytes());
    Ok(FileProbe {
        file_id: Some(value),
        size_bytes: metadata.len(),
        mtime_filetime: mtime_value(metadata),
        hard_link_count: metadata.nlink().max(1),
        reparse_point: is_reparse(metadata),
    })
}

#[cfg(not(any(windows, unix)))]
fn file_handle_identity(_file: &File, metadata: &Metadata) -> Result<FileProbe, DomainError> {
    Ok(FileProbe {
        file_id: None,
        size_bytes: metadata.len(),
        mtime_filetime: mtime_value(metadata),
        hard_link_count: 1,
        reparse_point: is_reparse(metadata),
    })
}

#[cfg(windows)]
fn is_reparse(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn mtime_value(metadata: &Metadata) -> i64 {
    use std::os::windows::fs::MetadataExt;
    i64::try_from(metadata.last_write_time()).unwrap_or(i64::MAX)
}

#[cfg(not(windows))]
fn mtime_value(metadata: &Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_nanos()).ok())
        .unwrap_or_default()
}

#[cfg(windows)]
fn volume_identity(
    path: &Path,
    _file: &File,
    _metadata: &Metadata,
) -> Result<(FilesystemKind, Vec<u8>, bool), DomainError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut mount = vec![0_u16; 32_768];
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) };
    if ok == 0 {
        return Err(root_io_error(std::io::Error::last_os_error()));
    }
    let mount_len = mount
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(mount.len());
    mount.truncate(mount_len + 1);
    let mut volume_name = vec![0_u16; 128];
    let named = unsafe {
        GetVolumeNameForVolumeMountPointW(
            mount.as_ptr(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
        )
    };
    let mut fs_name = vec![0_u16; 64];
    let mut serial = 0_u32;
    let info = unsafe {
        GetVolumeInformationW(
            mount.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    };
    if info == 0 {
        return Err(root_io_error(std::io::Error::last_os_error()));
    }
    let fs_len = fs_name
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(fs_name.len());
    let fs = String::from_utf16_lossy(&fs_name[..fs_len]);
    let kind = match fs.to_ascii_uppercase().as_str() {
        "NTFS" => FilesystemKind::Ntfs,
        "EXFAT" => FilesystemKind::ExFat,
        "REFS" => FilesystemKind::ReFs,
        _ => FilesystemKind::Other,
    };
    let mut key = b"volume-v1:".to_vec();
    if named != 0 {
        let name_len = volume_name
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(volume_name.len());
        key.extend_from_slice(String::from_utf16_lossy(&volume_name[..name_len]).as_bytes());
    } else {
        key.extend_from_slice(format!("serial:{serial:08x}").as_bytes());
    }
    Ok((kind, key, kind == FilesystemKind::Ntfs))
}

#[cfg(unix)]
fn volume_identity(
    _path: &Path,
    _file: &File,
    metadata: &Metadata,
) -> Result<(FilesystemKind, Vec<u8>, bool), DomainError> {
    use std::os::unix::fs::MetadataExt;
    Ok((
        FilesystemKind::Other,
        format!("test-volume-v1:{}", metadata.dev()).into_bytes(),
        true,
    ))
}

#[cfg(not(any(windows, unix)))]
fn volume_identity(
    path: &Path,
    _file: &File,
    _metadata: &Metadata,
) -> Result<(FilesystemKind, Vec<u8>, bool), DomainError> {
    Ok((FilesystemKind::Other, path_key(path), false))
}

fn root_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::RootOffline, "无法以只读方式访问图库。")
        .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_probe_reports_hard_links_and_never_opens_for_write() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("DSCF0001.JPG");
        let linked = directory.path().join("DSCF0001-copy.JPG");
        fs::write(&original, b"generated fixture").unwrap();
        fs::hard_link(&original, &linked).unwrap();

        let mut file = open_read_only(&original).unwrap();
        let probe = probe_file(&file).unwrap();
        assert!(probe.hard_link_count >= 2);
        assert!(file.write_all(b"must fail").is_err());
    }

    #[test]
    fn directory_probe_rejects_symlink_component() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = directory.path().join("link");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(probe_directory(&link).is_err());
    }
}
