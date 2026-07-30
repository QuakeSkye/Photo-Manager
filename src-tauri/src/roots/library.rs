use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::OptionalExtension;

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode, LibraryId, RootBindingId, RootId, VolumeId},
};

use super::{DirectoryProbe, probe_directory};

const PHASE_ONE_READ_CAPABILITIES: i64 = 0b1111;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FujiLibraryBinding {
    pub library_id: LibraryId,
    pub root_id: RootId,
    pub root_binding_id: RootBindingId,
    pub reconnected: bool,
    pub created_new_library: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FujiLibraryCandidate {
    pub display_path: String,
    pub reconnects_existing_library: bool,
}

#[derive(Debug, Clone)]
pub struct FujiLibraryService {
    database: Arc<Database>,
    app_data_directory: PathBuf,
}

impl FujiLibraryService {
    pub fn new(database: Arc<Database>, app_data_directory: impl Into<PathBuf>) -> Self {
        Self {
            database,
            app_data_directory: app_data_directory.into(),
        }
    }

    pub fn bind(&self, selected_directory: &Path) -> Result<FujiLibraryBinding, DomainError> {
        let probe = probe_directory(selected_directory)?;
        if paths_overlap(&probe.absolute_path, &self.app_data_directory) {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "图库与应用数据目录不能重叠。",
            ));
        }
        self.bind_probed(probe)
    }

    pub fn inspect_candidate(
        &self,
        selected_directory: &Path,
    ) -> Result<FujiLibraryCandidate, DomainError> {
        let probe = probe_directory(selected_directory)?;
        if paths_overlap(&probe.absolute_path, &self.app_data_directory) {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "图库与应用数据目录不能重叠。",
            ));
        }
        let reconnects_existing_library = if probe.strong_identity {
            match &probe.directory_file_id {
                Some(directory_id) => self
                    .database
                    .open_read_connection()?
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM root_binding binding
                            JOIN root ON root.root_id=binding.root_id
                            JOIN library ON library.library_id=root.library_id
                            JOIN volume_identity volume ON volume.volume_id=binding.volume_id
                            WHERE root.root_kind='FujiLibrary'
                              AND root.lifecycle!='Tombstoned'
                              AND library.lifecycle!='Tombstoned'
                              AND volume.strong_identity_key=?1
                              AND binding.root_directory_file_id=?2
                         )",
                        (&probe.volume_identity_key, directory_id),
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)?,
                None => false,
            }
        } else {
            false
        };
        Ok(FujiLibraryCandidate {
            display_path: probe.absolute_path.to_string_lossy().into_owned(),
            reconnects_existing_library,
        })
    }

    pub fn verify_current(&self) -> Result<FujiLibraryBinding, DomainError> {
        let connection = self.database.open_read_connection()?;
        let row = connection
            .query_row(
                "SELECT library.library_id, root.root_id, binding.binding_id,
                        binding.absolute_path_display
                 FROM current_library
                 JOIN library ON library.library_id=current_library.library_id
                 JOIN root ON root.library_id=library.library_id
                    AND root.root_kind='FujiLibrary' AND root.lifecycle!='Tombstoned'
                 JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
                 WHERE current_library.slot=1",
                (),
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "尚未绑定 Fuji 图库。"))?;
        drop(connection);

        let rebound = self.bind(Path::new(&row.3))?;
        if rebound.library_id.as_bytes().as_slice() != row.0
            || rebound.root_id.as_bytes().as_slice() != row.1
            || (rebound.root_binding_id.as_bytes().as_slice() != row.2 && !rebound.reconnected)
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "图库目录身份已变化，已停止使用旧绑定。",
            ));
        }
        Ok(rebound)
    }

    fn bind_probed(&self, probe: DirectoryProbe) -> Result<FujiLibraryBinding, DomainError> {
        let writer = self.database.writer()?;
        writer.transaction(move |transaction| {
            let matched = if probe.strong_identity {
                match &probe.directory_file_id {
                    Some(directory_id) => transaction
                        .query_row(
                            "SELECT library.library_id, root.root_id, binding.binding_id,
                                    binding.absolute_path_key, binding.binding_generation
                             FROM root_binding AS binding
                             JOIN root ON root.root_id=binding.root_id
                             JOIN library ON library.library_id=root.library_id
                             JOIN volume_identity AS volume ON volume.volume_id=binding.volume_id
                             WHERE root.root_kind='FujiLibrary'
                               AND root.lifecycle!='Tombstoned'
                               AND library.lifecycle!='Tombstoned'
                               AND volume.strong_identity_key=?1
                               AND binding.root_directory_file_id=?2
                             ORDER BY binding.binding_generation DESC LIMIT 1",
                            (&probe.volume_identity_key, directory_id),
                            |row| {
                                Ok((
                                    row.get::<_, Vec<u8>>(0)?,
                                    row.get::<_, Vec<u8>>(1)?,
                                    row.get::<_, Vec<u8>>(2)?,
                                    row.get::<_, Vec<u8>>(3)?,
                                    row.get::<_, i64>(4)?,
                                ))
                            },
                        )
                        .optional()
                        .map_err(map_sqlite_error)?,
                    None => None,
                }
            } else {
                None
            };

            reject_conflicting_root(transaction, &probe, matched.as_ref().map(|value| value.1.as_slice()))?;

            if let Some((library_bytes, root_bytes, binding_bytes, old_path_key, generation)) = matched {
                let library_id = library_id(&library_bytes)?;
                let root_id = root_id(&root_bytes)?;
                transaction
                    .execute("UPDATE library SET lifecycle='Inactive', row_version=row_version+1 WHERE lifecycle='Active' AND library_id!=?1", [library_id.as_bytes().as_slice()])
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute("UPDATE library SET lifecycle='Active', row_version=row_version+1 WHERE library_id=?1 AND lifecycle='Inactive'", [library_id.as_bytes().as_slice()])
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "INSERT INTO current_library(slot, library_id) VALUES (1, ?1)
                         ON CONFLICT(slot) DO UPDATE SET library_id=excluded.library_id",
                        [library_id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;

                let same_path = old_path_key == probe.absolute_path_key;
                let binding_id = if same_path {
                    let binding_id = binding_id(&binding_bytes)?;
                    transaction
                        .execute(
                            "UPDATE root SET lifecycle='Online', root_version=root_version+1
                             WHERE root_id=?1 AND lifecycle IN ('Offline','IdentityMismatch')",
                            [root_id.as_bytes().as_slice()],
                        )
                        .map_err(map_sqlite_error)?;
                    binding_id
                } else {
                    let next_binding = RootBindingId::new();
                    let previous_binding = binding_id(&binding_bytes)?;
                    transaction
                        .execute(
                            "UPDATE root_binding SET binding_state='Superseded'
                             WHERE binding_id=?1 AND binding_state='Active'",
                            [previous_binding.as_bytes().as_slice()],
                        )
                        .map_err(map_sqlite_error)?;
                    let volume_id = ensure_volume(transaction, &probe)?;
                    transaction
                        .execute(
                            "INSERT INTO root_binding(
                                binding_id, root_id, binding_generation, volume_id,
                                absolute_path_display, absolute_path_key, root_directory_file_id,
                                filesystem_kind, evidence_level, reparse_checked,
                                binding_state, supersedes_binding_id, verified_at
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'Strong', 1,
                                       'Active', ?9, ?10)",
                            rusqlite::params![
                                next_binding.as_bytes().as_slice(),
                                root_id.as_bytes().as_slice(),
                                generation + 1,
                                volume_id.as_bytes().as_slice(),
                                probe.absolute_path.to_string_lossy(),
                                &probe.absolute_path_key,
                                &probe.directory_file_id,
                                probe.filesystem_kind.as_sql(),
                                previous_binding.as_bytes().as_slice(),
                                now_micros(),
                            ],
                        )
                        .map_err(map_sqlite_error)?;
                    transaction
                        .execute(
                            "UPDATE root SET current_binding_id=?1, lifecycle='Online',
                                    root_version=root_version+1, capability_mask=?2
                             WHERE root_id=?3",
                            (
                                next_binding.as_bytes().as_slice(),
                                PHASE_ONE_READ_CAPABILITIES,
                                root_id.as_bytes().as_slice(),
                            ),
                        )
                        .map_err(map_sqlite_error)?;
                    next_binding
                };
                return Ok(FujiLibraryBinding {
                    library_id,
                    root_id,
                    root_binding_id: binding_id,
                    reconnected: true,
                    created_new_library: false,
                });
            }

            create_new_library(transaction, &probe)
        })
    }
}

fn create_new_library(
    transaction: &rusqlite::Transaction<'_>,
    probe: &DirectoryProbe,
) -> Result<FujiLibraryBinding, DomainError> {
    let library_id = LibraryId::new();
    let root_id = RootId::new();
    let binding_id = RootBindingId::new();
    let volume_id = ensure_volume(transaction, probe)?;
    let created_at = now_micros();
    transaction
        .execute(
            "UPDATE library SET lifecycle='Inactive', row_version=row_version+1
             WHERE lifecycle='Active'",
            (),
        )
        .map_err(map_sqlite_error)?;
    let display_name = probe
        .absolute_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Fuji Library");
    transaction
        .execute(
            "INSERT INTO library(library_id, library_kind, display_name, lifecycle, created_at)
             VALUES (?1, 'Fuji', ?2, 'Active', ?3)",
            (library_id.as_bytes().as_slice(), display_name, created_at),
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO root(root_id, library_id, root_kind, capability_mask,
                              root_version, lifecycle, created_at)
             VALUES (?1, ?2, 'FujiLibrary', ?3, 1, 'Online', ?4)",
            (
                root_id.as_bytes().as_slice(),
                library_id.as_bytes().as_slice(),
                PHASE_ONE_READ_CAPABILITIES,
                created_at,
            ),
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO root_binding(
                binding_id, root_id, binding_generation, volume_id,
                absolute_path_display, absolute_path_key, root_directory_file_id,
                filesystem_kind, evidence_level, reparse_checked, binding_state, verified_at
             ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, 1, 'Active', ?9)",
            rusqlite::params![
                binding_id.as_bytes().as_slice(),
                root_id.as_bytes().as_slice(),
                volume_id.as_bytes().as_slice(),
                probe.absolute_path.to_string_lossy(),
                &probe.absolute_path_key,
                &probe.directory_file_id,
                probe.filesystem_kind.as_sql(),
                if probe.strong_identity {
                    "Strong"
                } else {
                    "UserConfirmedLimited"
                },
                created_at,
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "UPDATE root SET current_binding_id=?1 WHERE root_id=?2",
            (
                binding_id.as_bytes().as_slice(),
                root_id.as_bytes().as_slice(),
            ),
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO current_library(slot, library_id) VALUES (1, ?1)
             ON CONFLICT(slot) DO UPDATE SET library_id=excluded.library_id",
            [library_id.as_bytes().as_slice()],
        )
        .map_err(map_sqlite_error)?;
    Ok(FujiLibraryBinding {
        library_id,
        root_id,
        root_binding_id: binding_id,
        reconnected: false,
        created_new_library: true,
    })
}

fn ensure_volume(
    transaction: &rusqlite::Transaction<'_>,
    probe: &DirectoryProbe,
) -> Result<VolumeId, DomainError> {
    if let Some(bytes) = transaction
        .query_row(
            "SELECT volume_id FROM volume_identity
             WHERE identity_format_version=1 AND strong_identity_key=?1",
            [&probe.volume_identity_key],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
    {
        return volume_id(&bytes);
    }
    let volume_id = VolumeId::new();
    transaction
        .execute(
            "INSERT INTO volume_identity(
                volume_id, filesystem_kind, strong_identity_key,
                identity_format_version, identity_strength, created_at
             ) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            (
                volume_id.as_bytes().as_slice(),
                probe.filesystem_kind.as_sql(),
                &probe.volume_identity_key,
                if probe.strong_identity {
                    "Strong"
                } else {
                    "UserConfirmedLimited"
                },
                now_micros(),
            ),
        )
        .map_err(map_sqlite_error)?;
    Ok(volume_id)
}

fn reject_conflicting_root(
    transaction: &rusqlite::Transaction<'_>,
    candidate: &DirectoryProbe,
    matched_root_id: Option<&[u8]>,
) -> Result<(), DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT root.root_id, binding.absolute_path_display
             FROM root
             JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
             WHERE root.lifecycle!='Tombstoned'",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map((), |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(map_sqlite_error)?;
    for row in rows {
        let (root_id, path) = row.map_err(map_sqlite_error)?;
        if matched_root_id == Some(root_id.as_slice()) {
            continue;
        }
        if paths_overlap(&candidate.absolute_path, Path::new(&path)) {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "所选图库与现有 Root 重叠。",
            ));
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = left
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    let right = right
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    left == right
        || left
            .strip_prefix(&right)
            .is_some_and(|rest| rest.starts_with('\\'))
        || right
            .strip_prefix(&left)
            .is_some_and(|rest| rest.starts_with('\\'))
}

fn uuid_from_bytes(bytes: &[u8]) -> Result<uuid::Uuid, DomainError> {
    uuid::Uuid::from_slice(bytes)
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "数据库对象标识损坏。"))
}

fn library_id(bytes: &[u8]) -> Result<LibraryId, DomainError> {
    Ok(LibraryId::from_uuid(uuid_from_bytes(bytes)?))
}

fn root_id(bytes: &[u8]) -> Result<RootId, DomainError> {
    Ok(RootId::from_uuid(uuid_from_bytes(bytes)?))
}

fn binding_id(bytes: &[u8]) -> Result<RootBindingId, DomainError> {
    Ok(RootBindingId::from_uuid(uuid_from_bytes(bytes)?))
}

fn volume_id(bytes: &[u8]) -> Result<VolumeId, DomainError> {
    Ok(VolumeId::from_uuid(uuid_from_bytes(bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::TestRoot;

    #[test]
    fn same_directory_reconnects_and_new_directory_creates_new_library() {
        let parent = tempfile::tempdir().unwrap();
        let app_root = TestRoot::create_in(parent.path()).unwrap();
        let library_one = parent.path().join("library-one");
        let library_two = parent.path().join("library-two");
        std::fs::create_dir(&library_one).unwrap();
        std::fs::create_dir(&library_two).unwrap();
        let database = Arc::new(Database::open(app_root.path()).unwrap());
        let service = FujiLibraryService::new(database, app_root.path());

        let first = service.bind(&library_one).unwrap();
        assert!(first.created_new_library);
        let reconnect = service.bind(&library_one).unwrap();
        assert_eq!(reconnect.library_id, first.library_id);
        assert_eq!(reconnect.root_id, first.root_id);
        assert!(reconnect.reconnected);

        let second = service.bind(&library_two).unwrap();
        assert_ne!(second.library_id, first.library_id);
        assert!(second.created_new_library);
    }

    #[test]
    fn app_data_overlap_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let app_root = TestRoot::create_in(parent.path()).unwrap();
        let database = Arc::new(Database::open(app_root.path()).unwrap());
        let service = FujiLibraryService::new(database, app_root.path());

        assert!(service.bind(app_root.path()).is_err());
    }
}
