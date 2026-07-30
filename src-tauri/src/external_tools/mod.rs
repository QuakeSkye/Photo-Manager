use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::Arc,
};

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode, PhotoId, ToolId},
    roots::{open_read_only, path_key, probe_directory, probe_file, resolve_contained_path},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoshopStatus {
    pub configured: bool,
    pub available: bool,
    pub tool_id: Option<String>,
    pub display_name: String,
    pub executable_label: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ExternalToolService {
    database: Arc<Database>,
}

impl ExternalToolService {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub fn photoshop_status(&self) -> Result<PhotoshopStatus, DomainError> {
        let connection = self.database.open_read_connection()?;
        let row = connection
            .query_row(
                "SELECT tool.tool_id, tool.executable_path_display, tool.validation_state
                 FROM external_tool_policy policy
                 LEFT JOIN external_tool tool ON tool.tool_id=policy.photoshop_tool_id
                 WHERE policy.slot=1",
                (),
                |row| {
                    Ok((
                        row.get::<_, Option<Vec<u8>>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(map_sqlite_error)?;
        let Some(tool_bytes) = row.0 else {
            return Ok(unconfigured_status());
        };
        let tool_id = id_from_bytes::<ToolId>(&tool_bytes)?;
        if row.2.as_deref() != Some("Valid") {
            return Ok(invalid_status(tool_id));
        }
        let executable = PathBuf::from(row.1.ok_or_else(|| {
            DomainError::new(ErrorCode::DatabaseCorrupt, "Photoshop 配置缺少程序位置。")
        })?);
        if self.verify_registered_tool(tool_id, &executable).is_err() {
            self.invalidate(tool_id)?;
            return Ok(invalid_status(tool_id));
        }
        Ok(PhotoshopStatus {
            configured: true,
            available: true,
            tool_id: Some(tool_id.to_string()),
            display_name: "Adobe Photoshop".into(),
            executable_label: Some("Photoshop.exe · 已验证".into()),
            message: "可以把当前 RAF 交给 Photoshop。".into(),
        })
    }

    pub fn register_photoshop(&self, executable: &Path) -> Result<PhotoshopStatus, DomainError> {
        let verified = verify_photoshop_executable(executable)?;
        let tool_id = ToolId::new();
        let display_path = executable.to_string_lossy().to_string();
        let executable_key = path_key(executable);
        let file_id = verified.file_id.ok_or_else(|| {
            DomainError::new(ErrorCode::IdentityMismatch, "无法可靠识别 Photoshop.exe。")
        })?;
        let size_bytes = i64::try_from(verified.size_bytes).map_err(|_| {
            DomainError::new(ErrorCode::UnsupportedMedia, "Photoshop.exe 大小无效。")
        })?;
        let mtime = verified.mtime_filetime;
        self.database.writer()?.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE external_tool SET validation_state='Invalid', invalidated_at=?1
                     WHERE tool_kind='Photoshop' AND validation_state='Valid'",
                    [now_micros()],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO external_tool(
                        tool_id, tool_kind, executable_path_display, executable_path_key,
                        file_id, size_bytes, mtime_filetime, validation_state,
                        created_at, last_validated_at, invalidated_at
                     ) VALUES (?1, 'Photoshop', ?2, ?3, ?4, ?5, ?6, 'Valid', ?7, ?7, NULL)",
                    rusqlite::params![
                        tool_id.as_bytes().as_slice(),
                        display_path,
                        executable_key,
                        file_id,
                        size_bytes,
                        mtime,
                        now_micros(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE external_tool_policy SET photoshop_tool_id=?1,
                        row_version=row_version+1 WHERE slot=1",
                    [tool_id.as_bytes().as_slice()],
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })?;
        self.photoshop_status()
    }

    pub fn open_raf_in_photoshop(
        &self,
        tool_id: ToolId,
        photo_id: PhotoId,
        photo_version: i64,
    ) -> Result<(), DomainError> {
        let executable = self.load_current_tool(tool_id)?;
        let executable_handle = match self.verify_registered_tool(tool_id, &executable) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = self.invalidate(tool_id);
                return Err(error);
            }
        };
        let photo = self.resolve_photo_asset(photo_id, photo_version, TargetRole::Raf)?;
        let photo_handle = verify_photo_target(&photo)?;
        launch_direct(&executable, &photo.absolute_path)?;
        drop(photo_handle);
        drop(executable_handle);
        Ok(())
    }

    pub fn open_file_location(
        &self,
        photo_id: PhotoId,
        photo_version: i64,
    ) -> Result<(), DomainError> {
        let photo = self.resolve_photo_asset(photo_id, photo_version, TargetRole::Location)?;
        let photo_handle = verify_photo_target(&photo)?;
        reveal_in_explorer(&photo.absolute_path)?;
        drop(photo_handle);
        Ok(())
    }

    fn load_current_tool(&self, tool_id: ToolId) -> Result<PathBuf, DomainError> {
        self.database
            .open_read_connection()?
            .query_row(
                "SELECT tool.executable_path_display
                 FROM external_tool_policy policy
                 JOIN external_tool tool ON tool.tool_id=policy.photoshop_tool_id
                 WHERE policy.slot=1 AND tool.tool_id=?1
                   AND tool.tool_kind='Photoshop' AND tool.validation_state='Valid'",
                [tool_id.as_bytes().as_slice()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_error)?
            .map(PathBuf::from)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::CapabilityDenied,
                    "Photoshop 配置无效或已被重新选择，请刷新设置。",
                )
            })
    }

    fn verify_registered_tool(&self, tool_id: ToolId, path: &Path) -> Result<File, DomainError> {
        let (stored_id, stored_size, stored_mtime): (Vec<u8>, i64, i64) = self
            .database
            .open_read_connection()?
            .query_row(
                "SELECT file_id, size_bytes, mtime_filetime FROM external_tool
                 WHERE tool_id=?1 AND validation_state='Valid'",
                [tool_id.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(map_sqlite_error)?;
        let handle = open_read_only(path).map_err(|error| {
            DomainError::new(
                ErrorCode::RootOffline,
                "Photoshop.exe 已不可用，请重新选择。",
            )
            .with_diagnostic(error.to_string())
        })?;
        let actual = probe_file(&handle)?;
        if actual.file_id.as_deref() != Some(stored_id.as_slice())
            || i64::try_from(actual.size_bytes).unwrap_or(i64::MAX) != stored_size
            || actual.mtime_filetime != stored_mtime
            || actual.reparse_point
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "Photoshop.exe 已发生变化，请重新选择。",
            ));
        }
        Ok(handle)
    }

    fn invalidate(&self, tool_id: ToolId) -> Result<(), DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE external_tool SET validation_state='Invalid', invalidated_at=?1
                     WHERE tool_id=?2 AND validation_state='Valid'",
                    (now_micros(), tool_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn resolve_photo_asset(
        &self,
        photo_id: PhotoId,
        photo_version: i64,
        target: TargetRole,
    ) -> Result<ResolvedPhotoTarget, DomainError> {
        let connection = self.database.open_read_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT relation.role, binding.absolute_path_display,
                        binding.root_directory_file_id, volume.strong_identity_key,
                        revision.relative_path, revision.file_id, revision.size_bytes,
                        revision.mtime_filetime
                 FROM current_library current
                 JOIN photo ON photo.library_id=current.library_id
                 JOIN photo_asset relation ON relation.photo_id=photo.photo_id
                 JOIN asset ON asset.asset_id=relation.asset_id
                 JOIN asset_revision revision
                   ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
                 JOIN root ON root.root_id=asset.root_id
                 JOIN root_binding binding ON binding.binding_id=root.current_binding_id
                 JOIN volume_identity volume ON volume.volume_id=binding.volume_id
                 WHERE current.slot=1 AND photo.photo_id=?1 AND photo.photo_version=?2
                   AND photo.lifecycle='Active' AND relation.to_photo_version IS NULL
                   AND asset.library_id=current.library_id AND asset.presence_state='Present'
                   AND root.lifecycle='Online' AND binding.binding_state='Active'",
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map((photo_id.as_bytes().as_slice(), photo_version), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<Vec<u8>>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(map_sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_error)?;
        let role = select_target_role(
            target,
            rows.iter().any(|row| row.0 == "Jpg"),
            rows.iter().any(|row| row.0 == "Raf"),
        )?;
        let row = rows.into_iter().find(|row| row.0 == role).ok_or_else(|| {
            let message = if target == TargetRole::Raf {
                "当前照片没有 RAF，不能交给 Photoshop。"
            } else {
                "当前图库找不到可定位的照片文件。"
            };
            DomainError::new(ErrorCode::CapabilityDenied, message)
        })?;
        let root_path = PathBuf::from(row.1);
        let absolute_path = resolve_contained_path(&root_path, Path::new(&row.4))?;
        Ok(ResolvedPhotoTarget {
            root_path,
            root_file_id: row.2,
            volume_key: row.3,
            absolute_path,
            file_id: row.5,
            size_bytes: row.6,
            mtime_filetime: row.7,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetRole {
    Raf,
    Location,
}

fn select_target_role(
    target: TargetRole,
    has_jpeg: bool,
    has_raf: bool,
) -> Result<&'static str, DomainError> {
    match (target, has_jpeg, has_raf) {
        (TargetRole::Raf, _, true) => Ok("Raf"),
        (TargetRole::Raf, _, false) => Err(DomainError::new(
            ErrorCode::CapabilityDenied,
            "当前照片没有 RAF，不能交给 Photoshop。",
        )),
        (TargetRole::Location, true, _) => Ok("Jpg"),
        (TargetRole::Location, false, true) => Ok("Raf"),
        (TargetRole::Location, false, false) => Err(DomainError::new(
            ErrorCode::CapabilityDenied,
            "当前图库找不到可定位的照片文件。",
        )),
    }
}

struct ResolvedPhotoTarget {
    root_path: PathBuf,
    root_file_id: Option<Vec<u8>>,
    volume_key: Vec<u8>,
    absolute_path: PathBuf,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
}

fn verify_photo_target(target: &ResolvedPhotoTarget) -> Result<File, DomainError> {
    let root = probe_directory(&target.root_path)?;
    if root.directory_file_id != target.root_file_id
        || root.volume_identity_key != target.volume_key
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "打开外部程序前照片文件夹身份已变化。",
        ));
    }
    let file = open_read_only(&target.absolute_path)?;
    let identity = probe_file(&file)?;
    if identity.file_id != target.file_id
        || i64::try_from(identity.size_bytes).unwrap_or(i64::MAX) != target.size_bytes
        || identity.mtime_filetime != target.mtime_filetime
        || identity.reparse_point
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "照片文件已发生变化，请重新扫描后再试。",
        ));
    }
    Ok(file)
}

fn verify_photoshop_executable(path: &Path) -> Result<crate::roots::FileProbe, DomainError> {
    let is_photoshop = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("Photoshop.exe"));
    if !is_photoshop {
        return Err(DomainError::new(
            ErrorCode::CapabilityDenied,
            "请选择名为 Photoshop.exe 的程序文件。",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetBinaryTypeW;
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut binary_type = 0_u32;
        if unsafe { GetBinaryTypeW(wide.as_ptr(), &mut binary_type) } == 0 {
            return Err(DomainError::new(
                ErrorCode::CapabilityDenied,
                "所选 Photoshop.exe 不是可验证的 Windows 可执行文件。",
            ));
        }
    }
    let file = open_read_only(path).map_err(|error| {
        DomainError::new(ErrorCode::RootOffline, "无法读取所选 Photoshop.exe。")
            .with_diagnostic(error.to_string())
    })?;
    let probe = probe_file(&file)?;
    if probe.reparse_point || probe.size_bytes == 0 {
        return Err(DomainError::new(
            ErrorCode::CapabilityDenied,
            "所选 Photoshop.exe 不是可用的普通程序文件。",
        ));
    }
    Ok(probe)
}

fn launch_direct(executable: &Path, photo: &Path) -> Result<(), DomainError> {
    Command::new(executable)
        .arg(photo)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            DomainError::new(ErrorCode::RootOffline, "无法启动 Photoshop，请重新选择。")
                .with_diagnostic(error.to_string())
        })
}

fn reveal_in_explorer(photo: &Path) -> Result<(), DomainError> {
    Command::new("explorer.exe")
        .arg("/select,")
        .arg(photo)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            DomainError::new(ErrorCode::RootOffline, "无法在资源管理器中定位照片。")
                .with_diagnostic(error.to_string())
        })
}

fn unconfigured_status() -> PhotoshopStatus {
    PhotoshopStatus {
        configured: false,
        available: false,
        tool_id: None,
        display_name: "Adobe Photoshop".into(),
        executable_label: None,
        message: "尚未选择 Photoshop。".into(),
    }
}

fn invalid_status(tool_id: ToolId) -> PhotoshopStatus {
    PhotoshopStatus {
        configured: true,
        available: false,
        tool_id: Some(tool_id.to_string()),
        display_name: "Adobe Photoshop".into(),
        executable_label: Some("Photoshop.exe · 需要重新选择".into()),
        message: "已配置程序缺失或验证失败，请重新选择。".into(),
    }
}

fn id_from_bytes<T: FromStr<Err = DomainError>>(bytes: &[u8]) -> Result<T, DomainError> {
    let uuid = uuid::Uuid::from_slice(bytes)
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "工具标识损坏。"))?;
    T::from_str(&uuid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::{CatalogService, LibraryScanner, ScanRequest},
        roots::FujiLibraryService,
        safety::TestRoot,
    };
    use std::fs;

    #[test]
    fn pairing_matrix_always_prefers_raf_for_photoshop_and_jpeg_for_location() {
        assert_eq!(
            select_target_role(TargetRole::Raf, true, true).unwrap(),
            "Raf"
        );
        assert_eq!(
            select_target_role(TargetRole::Raf, false, true).unwrap(),
            "Raf"
        );
        assert!(select_target_role(TargetRole::Raf, true, false).is_err());
        assert_eq!(
            select_target_role(TargetRole::Location, true, true).unwrap(),
            "Jpg"
        );
        assert_eq!(
            select_target_role(TargetRole::Location, true, false).unwrap(),
            "Jpg"
        );
        assert_eq!(
            select_target_role(TargetRole::Location, false, true).unwrap(),
            "Raf"
        );
    }

    #[test]
    fn stale_and_cross_library_photos_are_rejected_without_launching_any_program() {
        let parent = tempfile::tempdir().unwrap();
        let app_root = TestRoot::create_in(parent.path()).unwrap();
        let first = parent.path().join("generated-library-one");
        let second = parent.path().join("generated-library-two");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("DISPOSABLE.JPG"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        let database = Arc::new(Database::open(app_root.path()).unwrap());
        let libraries = FujiLibraryService::new(database.clone(), app_root.path());
        let first_binding = libraries.bind(&first).unwrap();
        let catalog = Arc::new(CatalogService::new(database.clone()));
        LibraryScanner::new(catalog.clone())
            .scan(ScanRequest {
                library_id: first_binding.library_id,
                root_id: first_binding.root_id,
                root_binding_id: first_binding.root_binding_id,
                root_path: first,
            })
            .unwrap();
        let (photo_bytes, version): (Vec<u8>, i64) = database
            .open_read_connection()
            .unwrap()
            .query_row("SELECT photo_id, photo_version FROM photo", (), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let photo_id = id_from_bytes::<PhotoId>(&photo_bytes).unwrap();
        let tools = ExternalToolService::new(database);
        assert!(
            tools
                .resolve_photo_asset(photo_id, version + 1, TargetRole::Location)
                .is_err()
        );
        assert!(
            tools
                .resolve_photo_asset(photo_id, version, TargetRole::Raf)
                .is_err()
        );

        libraries.bind(&second).unwrap();
        assert!(
            tools
                .resolve_photo_asset(photo_id, version, TargetRole::Location)
                .is_err()
        );
    }

    #[test]
    fn tool_id_is_the_only_reusable_capability_and_changed_executable_becomes_invalid() {
        let parent = tempfile::tempdir().unwrap();
        let app_root = TestRoot::create_in(parent.path()).unwrap();
        let database = Arc::new(Database::open(app_root.path()).unwrap());
        let tools = ExternalToolService::new(database);
        let photoshop = parent.path().join("Photoshop.exe");
        fs::copy(std::env::current_exe().unwrap(), &photoshop).unwrap();

        let status = tools.register_photoshop(&photoshop).unwrap();
        let configured = ToolId::from_str(status.tool_id.as_deref().unwrap()).unwrap();
        assert!(tools.load_current_tool(ToolId::new()).is_err());
        assert_eq!(tools.load_current_tool(configured).unwrap(), photoshop);

        fs::write(&photoshop, b"changed disposable executable copy").unwrap();
        let invalid = tools.photoshop_status().unwrap();
        assert!(invalid.configured);
        assert!(!invalid.available);
        assert_eq!(
            invalid.executable_label.as_deref(),
            Some("Photoshop.exe · 需要重新选择")
        );

        fs::copy(std::env::current_exe().unwrap(), &photoshop).unwrap();
        let reselected = tools.register_photoshop(&photoshop).unwrap();
        assert!(reselected.available);
        assert_ne!(reselected.tool_id, status.tool_id);
        assert!(tools.load_current_tool(configured).is_err());
    }
}
