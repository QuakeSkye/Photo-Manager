mod preview;

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::{OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{
        AssetId, CacheEntryId, CacheEvictionIntentId, CacheRootCreateIntentId, DirectorySnapshotId,
        DomainError, ErrorCode, OperationId, ReservationId, RootBindingId, RootId, VolumeId,
    },
    roots::{DirectoryProbe, path_key, probe_directory, probe_file, resolve_contained_path},
    safety::{FaultInjector, FaultPoint},
};

pub use preview::PreviewService;

pub const CACHE_DEFAULT_BYTES: i64 = 34_359_738_368;
pub const CACHE_MAX_BYTES: i64 = 68_719_476_736;
pub const CACHE_MIN_RESERVE_BYTES: u64 = 21_474_836_480;

const CACHE_CAPABILITIES: i64 = 0b1000_1111;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    Thumbnail,
    Preview,
}

impl CacheKind {
    const fn as_sql(self) -> &'static str {
        match self {
            Self::Thumbnail => "Thumbnail",
            Self::Preview => "Preview",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRootPlan {
    pub intent_id: CacheRootCreateIntentId,
    pub cache_root_id: RootId,
    pub child_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntryHandle {
    pub cache_entry_id: CacheEntryId,
    pub cache_root_id: RootId,
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub entry_ordinal: i64,
}

#[derive(Debug, Clone)]
pub struct CacheService {
    database: Arc<Database>,
    app_data_directory: PathBuf,
    faults: FaultInjector,
}

impl CacheService {
    pub fn new(
        database: Arc<Database>,
        app_data_directory: impl Into<PathBuf>,
        faults: FaultInjector,
    ) -> Self {
        Self {
            database,
            app_data_directory: app_data_directory.into(),
            faults,
        }
    }

    pub fn prepare_root(
        &self,
        parent: &Path,
        operation_id: OperationId,
    ) -> Result<CacheRootPlan, DomainError> {
        let probe = probe_directory(parent)?;
        if !probe.strong_identity || probe.directory_file_id.is_none() {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存父目录缺少可靠目录身份。",
            ));
        }
        if paths_overlap(&probe.absolute_path, &self.app_data_directory) {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "自选缓存位置不能与应用数据目录重叠。",
            ));
        }
        let request_hash = Sha256::digest(&probe.absolute_path_key).to_vec();
        if let Some((kind, stored_hash, existing_intent)) = self
            .database
            .open_read_connection()?
            .query_row(
                "SELECT event.operation_kind, event.request_hash, intent.create_intent_id
                 FROM operation_event event
                 LEFT JOIN cache_root_create_intent intent ON intent.operation_id=event.operation_id
                 WHERE event.operation_id=?1",
                [operation_id.as_bytes().as_slice()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
        {
            if kind != "CacheRootCreate" || stored_hash != request_hash {
                return Err(DomainError::new(
                    ErrorCode::DuplicateOperation,
                    "OperationId 已用于不同缓存位置。",
                ));
            }
            let existing_intent = existing_intent.ok_or_else(|| {
                DomainError::new(ErrorCode::DatabaseCorrupt, "缓存创建操作缺少 Intent。")
            })?;
            let intent_id = CacheRootCreateIntentId::from_uuid(
                uuid::Uuid::from_slice(&existing_intent).map_err(|_| corrupt_id())?,
            );
            let stored = self.load_create_intent(intent_id)?;
            return Ok(CacheRootPlan {
                intent_id,
                cache_root_id: stored.root_id,
                child_path: stored.parent_path.join(stored.child_name),
            });
        }
        let intent_id = CacheRootCreateIntentId::new();
        let root_id = RootId::new();
        let snapshot_id = DirectorySnapshotId::new();
        let nonce = uuid::Uuid::new_v4().as_bytes().to_vec();
        let child_name = format!(
            "FujiPhotoManagerCache-{}-{}",
            root_id,
            hex::encode(&nonce[..6])
        );
        let child_path = probe.absolute_path.join(&child_name);
        let child_key = path_key(&child_path);
        let child_name_key = child_name.to_lowercase().into_bytes();
        let result_path = child_path.clone();
        self.database.writer()?.transaction(move |transaction| {
            reject_root_change_while_busy(transaction)?;
            reject_overlap(transaction, &probe.absolute_path)?;
            let volume_id = ensure_volume(transaction, &probe)?;
            reject_deny_write_volume(transaction, volume_id)?;
            record_operation(transaction, operation_id, "CacheRootCreate", &request_hash)?;
            transaction
                .execute(
                    "INSERT INTO directory_identity_snapshot(
                        directory_snapshot_id, volume_id, absolute_path_display, path_key,
                        directory_file_id, filesystem_kind, identity_strength,
                        contained, reparse_free, captured_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Strong', 1, 1, ?7)",
                    (
                        snapshot_id.as_bytes().as_slice(),
                        volume_id.as_bytes().as_slice(),
                        probe.absolute_path.to_string_lossy(),
                        &probe.absolute_path_key,
                        &probe.directory_file_id,
                        probe.filesystem_kind.as_sql(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO cache_root_create_intent(
                        create_intent_id, operation_id, parent_directory_snapshot_id,
                        planned_cache_root_id, planned_child_name_key, planned_child_path_key,
                        ownership_nonce, intent_state, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Prepared', ?8)",
                    (
                        intent_id.as_bytes().as_slice(),
                        operation_id.as_bytes().as_slice(),
                        snapshot_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                        child_name_key,
                        child_key,
                        &nonce,
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })?;
        Ok(CacheRootPlan {
            intent_id,
            cache_root_id: root_id,
            child_path: result_path,
        })
    }

    pub fn execute_root_create(
        &self,
        intent_id: CacheRootCreateIntentId,
    ) -> Result<RootId, DomainError> {
        let prepared = self.load_create_intent(intent_id)?;
        if prepared.state != "Prepared" {
            return if prepared.state == "Bound" {
                Ok(prepared.root_id)
            } else {
                Err(DomainError::new(
                    ErrorCode::CacheOwnership,
                    "缓存 Root 创建 Intent 已终结，不能自动重试。",
                ))
            };
        }
        let parent_probe = probe_directory(&prepared.parent_path)?;
        if parent_probe.absolute_path_key != prepared.parent_path_key
            || parent_probe.directory_file_id != prepared.parent_file_id
            || parent_probe.volume_identity_key != prepared.volume_key
        {
            self.finish_create_needs_attention(intent_id, None)?;
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "缓存父目录身份已变化。",
            ));
        }
        let child_path = parent_probe.absolute_path.join(&prepared.child_name);
        if path_key(&child_path) != prepared.child_path_key {
            self.finish_create_needs_attention(intent_id, None)?;
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存子目录计划不一致。",
            ));
        }
        if child_path.exists() {
            self.finish_create_needs_attention(intent_id, probe_directory(&child_path).ok())?;
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "计划中的缓存子目录已存在，已停止自动认领。",
            ));
        }
        self.faults.checkpoint(FaultPoint::Win32BeforeCall)?;
        fs::create_dir(&child_path).map_err(cache_io_error)?;
        self.faults.checkpoint(FaultPoint::Win32AfterCall)?;
        let child_probe = probe_directory(&child_path)?;
        let child_file_id = child_probe.directory_file_id.clone().ok_or_else(|| {
            DomainError::new(ErrorCode::CacheOwnership, "新缓存目录缺少文件身份。")
        })?;
        let binding_id = RootBindingId::new();
        let root_id = prepared.root_id;
        let parent_snapshot_id = prepared.parent_snapshot_id;
        let ownership_nonce = prepared.ownership_nonce;
        self.database.writer()?.transaction(move |transaction| {
            let volume_id = ensure_volume(transaction, &child_probe)?;
            reject_deny_write_volume(transaction, volume_id)?;
            transaction
                .execute(
                    "INSERT INTO root(
                        root_id, library_id, root_kind, capability_mask, root_version,
                        lifecycle, created_at
                     ) VALUES (?1, NULL, 'CacheRoot', ?2, 1, 'Online', ?3)",
                    (
                        root_id.as_bytes().as_slice(),
                        CACHE_CAPABILITIES,
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO root_binding(
                        binding_id, root_id, binding_generation, volume_id,
                        absolute_path_display, absolute_path_key, root_directory_file_id,
                        filesystem_kind, evidence_level, reparse_checked,
                        binding_state, verified_at
                     ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, 'Strong', 1, 'Active', ?8)",
                    (
                        binding_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                        volume_id.as_bytes().as_slice(),
                        child_probe.absolute_path.to_string_lossy(),
                        &child_probe.absolute_path_key,
                        &child_probe.directory_file_id,
                        child_probe.filesystem_kind.as_sql(),
                        now_micros(),
                    ),
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
                    "UPDATE cache_root_ownership SET ownership_state='Draining'
                     WHERE ownership_state='Active'",
                    (),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO cache_root_ownership(
                        cache_root_id, parent_directory_snapshot_id, child_root_binding_id,
                        ownership_nonce, directory_file_id, ownership_state, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'Active', ?6)",
                    (
                        root_id.as_bytes().as_slice(),
                        parent_snapshot_id.as_bytes().as_slice(),
                        binding_id.as_bytes().as_slice(),
                        ownership_nonce,
                        child_file_id,
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE cache_policy SET active_cache_root_id=?1, confirmed_at=?2,
                            row_version=row_version+1 WHERE slot=1",
                    (root_id.as_bytes().as_slice(), now_micros()),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE cache_root_create_intent
                     SET intent_state='Bound', finished_at=?1
                     WHERE create_intent_id=?2 AND intent_state='Prepared'",
                    (now_micros(), intent_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })?;
        Ok(root_id)
    }

    pub fn reconcile_root_create(
        &self,
        intent_id: CacheRootCreateIntentId,
    ) -> Result<RootId, DomainError> {
        let intent = self.load_create_intent(intent_id)?;
        if intent.state == "Bound" {
            return Ok(intent.root_id);
        }
        if intent.state != "Prepared" {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存 Root Intent 需要用户处理，不能自动重试。",
            ));
        }
        let child = intent.parent_path.join(&intent.child_name);
        if child.exists() {
            self.finish_create_needs_attention(intent_id, probe_directory(&child).ok())?;
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "崩溃后发现未登记缓存目录，已保留并等待用户处理。",
            ));
        }
        self.execute_root_create(intent_id)
    }

    pub fn get_or_create(
        &self,
        asset_id: AssetId,
        asset_version: i64,
        kind: CacheKind,
        render_key: &[u8],
        bytes: &[u8],
    ) -> Result<CacheEntryHandle, DomainError> {
        if asset_version < 1 || bytes.is_empty() {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存渲染请求无效。",
            ));
        }
        let root = self.active_root()?;
        if let Some(existing) =
            self.load_current_entry(root.root_id, asset_id, asset_version, kind, render_key)?
        {
            return self.validate_or_transition_entry(root, existing);
        }
        self.ensure_space(bytes.len() as u64)?;
        let ordinal = self.next_ordinal(root.root_id, asset_id, asset_version, kind, render_key)?;
        let nonce = uuid::Uuid::new_v4().as_bytes().to_vec();
        let render_hash = hex::encode(Sha256::digest(render_key));
        let relative = format!(
            "{}-v{}-{}-{}-{}-{}.jpg",
            asset_id,
            asset_version,
            kind.as_sql().to_ascii_lowercase(),
            &render_hash[..16],
            ordinal,
            hex::encode(&nonce[..6]),
        );
        let absolute = resolve_contained_path(&root.path, Path::new(&relative))?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        configure_cache_create(&mut options);
        self.faults.checkpoint(FaultPoint::Win32BeforeCall)?;
        let mut file = options.open(&absolute).map_err(cache_io_error)?;
        file.write_all(bytes).map_err(cache_io_error)?;
        file.flush().map_err(cache_io_error)?;
        file.sync_all().map_err(cache_io_error)?;
        let identity = probe_file(&file)?;
        self.faults.checkpoint(FaultPoint::Win32AfterCall)?;
        drop(file);
        let entry_id = CacheEntryId::new();
        let relative_key = path_key(Path::new(&relative));
        let stored_relative = relative.clone();
        let stored_render_key = render_key.to_vec();
        self.database.writer()?.transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO cache_entry(
                        cache_entry_id, cache_root_id, entry_ordinal, ownership_nonce,
                        asset_id, asset_version, cache_kind, render_key, relative_path,
                        path_key, file_id, size_bytes, mtime_filetime,
                        entry_state, created_at, last_accessed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                               ?13, 'Live', ?14, ?14)",
                    rusqlite::params![
                        entry_id.as_bytes().as_slice(),
                        root.root_id.as_bytes().as_slice(),
                        ordinal,
                        nonce,
                        asset_id.as_bytes().as_slice(),
                        asset_version,
                        kind.as_sql(),
                        stored_render_key,
                        stored_relative,
                        relative_key,
                        identity.file_id,
                        i64::try_from(identity.size_bytes).unwrap_or(i64::MAX),
                        identity.mtime_filetime,
                        now_micros(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })?;
        Ok(CacheEntryHandle {
            cache_entry_id: entry_id,
            cache_root_id: root.root_id,
            relative_path: relative,
            absolute_path: absolute,
            entry_ordinal: ordinal,
        })
    }

    pub fn evict_lru_until_within_limits(&self) -> Result<usize, DomainError> {
        let mut evicted = 0;
        loop {
            let root = self.active_root()?;
            let (capacity, used) = self.cache_usage(root.root_id)?;
            let (available, total) = disk_space(&root.path)?;
            let reserve = CACHE_MIN_RESERVE_BYTES.max(ceil_ten_percent(total)?);
            if used <= capacity && available >= reserve {
                return Ok(evicted);
            }
            let entry = self.oldest_live_entry(root.root_id)?.ok_or_else(|| {
                DomainError::new(
                    ErrorCode::CacheOwnership,
                    "缓存空间不足，且没有可安全淘汰的 Entry。",
                )
            })?;
            self.evict_entry(entry)?;
            evicted += 1;
        }
    }

    pub fn reconcile_evictions(&self) -> Result<usize, DomainError> {
        let connection = self.database.open_read_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT eviction_intent_id FROM cache_eviction_intent
             WHERE intent_state IN ('Prepared','Deleting','AwaitingRoot') ORDER BY created_at",
            )
            .map_err(map_sqlite_error)?;
        let ids = statement
            .query_map((), |row| row.get::<_, Vec<u8>>(0))
            .map_err(map_sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_error)?;
        drop(statement);
        drop(connection);
        let mut completed = 0;
        for bytes in ids {
            let id =
                CacheEvictionIntentId::from_uuid(uuid::Uuid::from_slice(&bytes).map_err(|_| {
                    DomainError::new(ErrorCode::DatabaseCorrupt, "缓存淘汰 Intent 标识损坏。")
                })?);
            if self.resume_eviction(id)? {
                completed += 1;
            }
        }
        Ok(completed)
    }

    fn ensure_space(&self, incoming: u64) -> Result<(), DomainError> {
        let root = self.active_root()?;
        let (capacity, used) = self.cache_usage(root.root_id)?;
        let (available, total) = disk_space(&root.path)?;
        let reserve = CACHE_MIN_RESERVE_BYTES.max(ceil_ten_percent(total)?);
        if used.saturating_add(incoming) <= capacity
            && available >= incoming.saturating_add(reserve)
        {
            return Ok(());
        }
        self.evict_lru_for(incoming)?;
        let root = self.active_root()?;
        let (capacity, used) = self.cache_usage(root.root_id)?;
        let (available, total) = disk_space(&root.path)?;
        let reserve = CACHE_MIN_RESERVE_BYTES.max(ceil_ten_percent(total)?);
        if used.saturating_add(incoming) > capacity || available < incoming.saturating_add(reserve)
        {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存空间不足，无法在保留安全余量后创建预览。",
            ));
        }
        Ok(())
    }

    fn evict_lru_for(&self, incoming: u64) -> Result<usize, DomainError> {
        let mut evicted = 0;
        loop {
            let root = self.active_root()?;
            let (capacity, used) = self.cache_usage(root.root_id)?;
            let (available, total) = disk_space(&root.path)?;
            let reserve = CACHE_MIN_RESERVE_BYTES.max(ceil_ten_percent(total)?);
            if used.saturating_add(incoming) <= capacity
                && available >= incoming.saturating_add(reserve)
            {
                return Ok(evicted);
            }
            let entry = self.oldest_live_entry(root.root_id)?.ok_or_else(|| {
                DomainError::new(
                    ErrorCode::CacheOwnership,
                    "缓存空间不足，且没有可安全淘汰的 Entry。",
                )
            })?;
            self.evict_entry(entry)?;
            evicted += 1;
        }
    }

    fn evict_entry(&self, entry: StoredEntry) -> Result<(), DomainError> {
        let intent_id = CacheEvictionIntentId::new();
        let reservation_id = ReservationId::new();
        let operation_id = OperationId::new();
        let snapshot_id = uuid::Uuid::new_v4();
        self.database.writer()?.transaction(move |transaction| {
            let volume_id: Vec<u8> = transaction
                .query_row(
                    "SELECT volume_id FROM root_binding WHERE binding_id=?1 AND root_id=?2",
                    (
                        entry.binding_id.as_bytes().as_slice(),
                        entry.root_id.as_bytes().as_slice(),
                    ),
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            record_operation(transaction, operation_id, "CacheEviction", &entry.path_key)?;
            transaction
                .execute(
                    "INSERT INTO file_identity_snapshot(
                    snapshot_id, root_id, root_binding_id, volume_id, relative_path,
                    path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                    captured_at, evidence_level
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, 'Strong')",
                    (
                        snapshot_id.as_bytes().as_slice(),
                        entry.root_id.as_bytes().as_slice(),
                        entry.binding_id.as_bytes().as_slice(),
                        volume_id,
                        &entry.relative_path,
                        &entry.path_key,
                        &entry.file_id,
                        entry.size_bytes,
                        entry.mtime_filetime,
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO cache_eviction_intent(
                    eviction_intent_id, operation_id, cache_entry_id, cache_root_id,
                    root_binding_id, deletion_snapshot_id, intent_state, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Prepared', ?7)",
                    (
                        intent_id.as_bytes().as_slice(),
                        operation_id.as_bytes().as_slice(),
                        entry.entry_id.as_bytes().as_slice(),
                        entry.root_id.as_bytes().as_slice(),
                        entry.binding_id.as_bytes().as_slice(),
                        snapshot_id.as_bytes().as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO operation_reservation(
                    reservation_id, owner_kind, cache_eviction_intent_id, cache_entry_id,
                    reservation_kind, root_id, root_binding_id, file_id, path_key, created_at
                 ) VALUES (?1, 'CacheEvictionIntent', ?2, ?3, 'CacheEntry', ?4, ?5, ?6, ?7, ?8)",
                    (
                        reservation_id.as_bytes().as_slice(),
                        intent_id.as_bytes().as_slice(),
                        entry.entry_id.as_bytes().as_slice(),
                        entry.root_id.as_bytes().as_slice(),
                        entry.binding_id.as_bytes().as_slice(),
                        &entry.file_id,
                        &entry.path_key,
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })?;
        self.resume_eviction(intent_id).map(|_| ())
    }

    fn resume_eviction(&self, intent_id: CacheEvictionIntentId) -> Result<bool, DomainError> {
        let intent = self.load_eviction(intent_id)?;
        if matches!(intent.state.as_str(), "Evicted" | "AlreadyMissing") {
            return Ok(true);
        }
        if intent.state == "NeedsAttention" {
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存淘汰需要用户处理，禁止自动重试。",
            ));
        }
        let root_probe = match probe_directory(&intent.root_path) {
            Ok(value) => value,
            Err(_) => {
                if intent.state != "AwaitingRoot" {
                    self.set_eviction_state(intent_id, &intent.state, "AwaitingRoot", false)?;
                }
                return Ok(false);
            }
        };
        if root_probe.directory_file_id != intent.root_directory_file_id {
            self.finish_eviction_attention(&intent)?;
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "缓存 Root 身份已变化。",
            ));
        }
        let path = resolve_contained_path(&intent.root_path, Path::new(&intent.relative_path))?;
        if !path.exists() {
            self.finish_eviction_missing(&intent)?;
            return Ok(true);
        }
        let file = open_cache_delete_handle(&path)?;
        let identity = probe_file(&file)?;
        if identity.file_id != intent.file_id
            || i64::try_from(identity.size_bytes).unwrap_or(i64::MAX) != intent.size_bytes
        {
            drop(file);
            self.finish_eviction_attention(&intent)?;
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "缓存 Entry 身份已变化。",
            ));
        }
        if intent.state != "Deleting" {
            self.set_eviction_state(intent_id, &intent.state, "Deleting", false)?;
        }
        self.faults.checkpoint(FaultPoint::Win32BeforeCall)?;
        delete_cache_file_by_handle(file)?;
        self.faults.checkpoint(FaultPoint::Win32AfterCall)?;
        if path.exists() {
            self.finish_eviction_attention(&intent)?;
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存删除结果不明确。",
            ));
        }
        self.finish_eviction_success(&intent)?;
        Ok(true)
    }

    fn finish_eviction_success(&self, intent: &StoredEviction) -> Result<(), DomainError> {
        finish_eviction(
            &self.database,
            intent,
            "Evicted",
            "Evicted",
            "DeleteSucceeded",
        )
    }

    fn finish_eviction_missing(&self, intent: &StoredEviction) -> Result<(), DomainError> {
        finish_eviction(
            &self.database,
            intent,
            "AlreadyMissing",
            "Missing",
            "ObservedMissingAfterIntent",
        )
    }

    fn finish_eviction_attention(&self, intent: &StoredEviction) -> Result<(), DomainError> {
        let id = intent.intent_id;
        let entry = intent.entry_id;
        self.database.writer()?.transaction(move |transaction| {
            transaction.execute(
                "UPDATE cache_eviction_intent SET intent_state='NeedsAttention', finished_at=?1,
                        row_version=row_version+1
                 WHERE eviction_intent_id=?2 AND intent_state IN ('Prepared','Deleting','AwaitingRoot')",
                (now_micros(), id.as_bytes().as_slice()),
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE cache_entry SET entry_state='NeedsAttention' WHERE cache_entry_id=?1 AND entry_state='Live'",
                [entry.as_bytes().as_slice()],
            ).map_err(map_sqlite_error)?;
            release_reservation(transaction, id, "NeedsAttention")
        })
    }

    fn set_eviction_state(
        &self,
        intent_id: CacheEvictionIntentId,
        from: &str,
        to: &str,
        terminal: bool,
    ) -> Result<(), DomainError> {
        let from = from.to_string();
        let to = to.to_string();
        self.database.writer()?.transaction(move |transaction| {
            let changed = transaction
                .execute(
                    "UPDATE cache_eviction_intent SET intent_state=?1,
                        finished_at=CASE WHEN ?2 THEN ?3 ELSE NULL END,
                        row_version=row_version+1
                 WHERE eviction_intent_id=?4 AND intent_state=?5",
                    (
                        to,
                        terminal,
                        now_micros(),
                        intent_id.as_bytes().as_slice(),
                        from,
                    ),
                )
                .map_err(map_sqlite_error)?;
            if changed != 1 {
                return Err(DomainError::stale_version("CacheEvictionIntent"));
            }
            Ok(())
        })
    }

    fn load_create_intent(
        &self,
        intent_id: CacheRootCreateIntentId,
    ) -> Result<StoredCreateIntent, DomainError> {
        let connection = self.database.open_read_connection()?;
        connection
            .query_row(
                "SELECT intent.planned_cache_root_id, intent.intent_state, intent.ownership_nonce,
                    intent.parent_directory_snapshot_id, snapshot.absolute_path_display,
                    snapshot.path_key, snapshot.directory_file_id, volume.strong_identity_key,
                    intent.planned_child_name_key, intent.planned_child_path_key
             FROM cache_root_create_intent intent
             JOIN directory_identity_snapshot snapshot
               ON snapshot.directory_snapshot_id=intent.parent_directory_snapshot_id
             JOIN volume_identity volume ON volume.volume_id=snapshot.volume_id
             WHERE intent.create_intent_id=?1",
                [intent_id.as_bytes().as_slice()],
                |row| {
                    let root: Vec<u8> = row.get(0)?;
                    let ownership_nonce: Vec<u8> = row.get(2)?;
                    let root_id = RootId::from_uuid(uuid::Uuid::from_slice(&root).unwrap());
                    Ok(StoredCreateIntent {
                        root_id,
                        state: row.get(1)?,
                        ownership_nonce: ownership_nonce.clone(),
                        parent_snapshot_id: DirectorySnapshotId::from_uuid(
                            uuid::Uuid::from_slice(&row.get::<_, Vec<u8>>(3)?).unwrap(),
                        ),
                        parent_path: PathBuf::from(row.get::<_, String>(4)?),
                        parent_path_key: row.get(5)?,
                        parent_file_id: row.get(6)?,
                        volume_key: row.get(7)?,
                        child_name: format!(
                            "FujiPhotoManagerCache-{}-{}",
                            root_id,
                            hex::encode(&ownership_nonce[..6]),
                        ),
                        child_path_key: row.get(9)?,
                    })
                },
            )
            .map_err(map_sqlite_error)
    }

    fn finish_create_needs_attention(
        &self,
        intent_id: CacheRootCreateIntentId,
        observed: Option<DirectoryProbe>,
    ) -> Result<(), DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            let observed_id = if let Some(probe) = observed {
                let volume = ensure_volume(transaction, &probe)?;
                let id = DirectorySnapshotId::new();
                transaction
                    .execute(
                        "INSERT INTO directory_identity_snapshot(
                        directory_snapshot_id, volume_id, absolute_path_display, path_key,
                        directory_file_id, filesystem_kind, identity_strength,
                        contained, reparse_free, captured_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 1, ?8)",
                        (
                            id.as_bytes().as_slice(),
                            volume.as_bytes().as_slice(),
                            probe.absolute_path.to_string_lossy(),
                            probe.absolute_path_key,
                            probe.directory_file_id,
                            probe.filesystem_kind.as_sql(),
                            if probe.strong_identity {
                                "Strong"
                            } else {
                                "UserConfirmedLimited"
                            },
                            now_micros(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
                Some(id.as_bytes().to_vec())
            } else {
                None
            };
            transaction
                .execute(
                    "UPDATE cache_root_create_intent SET intent_state='NeedsAttention',
                        observed_child_snapshot_id=?1, finished_at=?2
                 WHERE create_intent_id=?3 AND intent_state='Prepared'",
                    (observed_id, now_micros(), intent_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn active_root(&self) -> Result<StoredRoot, DomainError> {
        let connection = self.database.open_read_connection()?;
        let root = connection
            .query_row(
                "SELECT root.root_id, binding.binding_id, binding.absolute_path_display,
                    ownership.directory_file_id
             FROM cache_policy policy
             JOIN root ON root.root_id=policy.active_cache_root_id
             JOIN root_binding binding ON binding.binding_id=root.current_binding_id
             JOIN cache_root_ownership ownership ON ownership.cache_root_id=root.root_id
             WHERE policy.slot=1 AND root.lifecycle='Online'
               AND binding.binding_state='Active' AND ownership.ownership_state='Active'",
                (),
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or_else(|| {
                DomainError::new(ErrorCode::CacheOwnership, "没有可用的 Active CacheRoot。")
            })?;
        let stored = StoredRoot {
            root_id: RootId::from_uuid(uuid::Uuid::from_slice(&root.0).map_err(|_| corrupt_id())?),
            path: PathBuf::from(root.2),
            directory_file_id: Some(root.3),
        };
        let probe = probe_directory(&stored.path)?;
        if probe.directory_file_id != stored.directory_file_id {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "CacheRoot 身份复核失败。",
            ));
        }
        Ok(stored)
    }

    fn load_current_entry(
        &self,
        root_id: RootId,
        asset_id: AssetId,
        asset_version: i64,
        kind: CacheKind,
        render_key: &[u8],
    ) -> Result<Option<StoredEntry>, DomainError> {
        let connection = self.database.open_read_connection()?;
        connection
            .query_row(
                "SELECT entry.cache_entry_id, entry.relative_path, entry.path_key, entry.file_id,
                    entry.size_bytes, entry.mtime_filetime, entry.entry_ordinal, binding.binding_id
             FROM cache_entry entry
             JOIN root ON root.root_id=entry.cache_root_id
             JOIN root_binding binding ON binding.binding_id=root.current_binding_id
             WHERE entry.cache_root_id=?1 AND entry.asset_id=?2 AND entry.asset_version=?3
               AND entry.cache_kind=?4 AND entry.render_key=?5
               AND entry.entry_state IN ('Live','NeedsAttention')",
                (
                    root_id.as_bytes().as_slice(),
                    asset_id.as_bytes().as_slice(),
                    asset_version,
                    kind.as_sql(),
                    render_key,
                ),
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
            .map(|row| {
                Ok(StoredEntry {
                    entry_id: CacheEntryId::from_uuid(
                        uuid::Uuid::from_slice(&row.0).map_err(|_| corrupt_id())?,
                    ),
                    root_id,
                    binding_id: RootBindingId::from_uuid(
                        uuid::Uuid::from_slice(&row.7).map_err(|_| corrupt_id())?,
                    ),
                    relative_path: row.1,
                    path_key: row.2,
                    file_id: row.3,
                    size_bytes: row.4,
                    mtime_filetime: row.5,
                    ordinal: row.6,
                })
            })
            .transpose()
    }

    fn validate_or_transition_entry(
        &self,
        root: StoredRoot,
        entry: StoredEntry,
    ) -> Result<CacheEntryHandle, DomainError> {
        let absolute = resolve_contained_path(&root.path, Path::new(&entry.relative_path))?;
        if !absolute.exists() {
            let id = entry.entry_id;
            self.database.writer()?.transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE cache_entry SET entry_state='Missing'
                     WHERE cache_entry_id=?1 AND entry_state='Live'",
                        [id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })?;
            return Err(DomainError::new(
                ErrorCode::CacheOwnership,
                "缓存 Entry 已缺失，可安全重建。",
            ));
        }
        let file = File::open(&absolute).map_err(cache_io_error)?;
        let identity = probe_file(&file)?;
        if identity.file_id != entry.file_id
            || i64::try_from(identity.size_bytes).unwrap_or(i64::MAX) != entry.size_bytes
        {
            let id = entry.entry_id;
            self.database.writer()?.transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE cache_entry SET entry_state='NeedsAttention'
                     WHERE cache_entry_id=?1 AND entry_state='Live'",
                        [id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })?;
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "缓存 Entry 身份异常。",
            ));
        }
        let id = entry.entry_id;
        self.database.writer()?.transaction(move |transaction| {
            transaction.execute(
                "UPDATE cache_entry SET last_accessed_at=?1 WHERE cache_entry_id=?2 AND entry_state='Live'",
                (now_micros(), id.as_bytes().as_slice()),
            ).map_err(map_sqlite_error)?;
            Ok(())
        })?;
        Ok(CacheEntryHandle {
            cache_entry_id: entry.entry_id,
            cache_root_id: root.root_id,
            relative_path: entry.relative_path,
            absolute_path: absolute,
            entry_ordinal: entry.ordinal,
        })
    }

    fn next_ordinal(
        &self,
        root: RootId,
        asset: AssetId,
        version: i64,
        kind: CacheKind,
        render: &[u8],
    ) -> Result<i64, DomainError> {
        let connection = self.database.open_read_connection()?;
        let current: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(entry_ordinal), 0) FROM cache_entry
             WHERE cache_root_id=?1 AND asset_id=?2 AND asset_version=?3
               AND cache_kind=?4 AND render_key=?5",
                (
                    root.as_bytes().as_slice(),
                    asset.as_bytes().as_slice(),
                    version,
                    kind.as_sql(),
                    render,
                ),
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        current
            .checked_add(1)
            .ok_or_else(|| DomainError::new(ErrorCode::Internal, "缓存 incarnation 序号耗尽。"))
    }

    fn cache_usage(&self, root: RootId) -> Result<(u64, u64), DomainError> {
        let connection = self.database.open_read_connection()?;
        let (capacity, used): (i64, i64) = connection.query_row(
            "SELECT policy.capacity_bytes,
                    COALESCE(SUM(CASE WHEN entry.entry_state='Live' THEN entry.size_bytes ELSE 0 END), 0)
             FROM cache_policy policy LEFT JOIN cache_entry entry ON entry.cache_root_id=?1
             WHERE policy.slot=1",
            [root.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).map_err(map_sqlite_error)?;
        Ok((capacity as u64, used as u64))
    }

    fn oldest_live_entry(&self, root: RootId) -> Result<Option<StoredEntry>, DomainError> {
        let connection = self.database.open_read_connection()?;
        connection
            .query_row(
                "SELECT entry.cache_entry_id, entry.relative_path, entry.path_key, entry.file_id,
                    entry.size_bytes, entry.mtime_filetime, entry.entry_ordinal, binding.binding_id
             FROM cache_entry entry JOIN root ON root.root_id=entry.cache_root_id
             JOIN root_binding binding ON binding.binding_id=root.current_binding_id
             WHERE entry.cache_root_id=?1 AND entry.entry_state='Live'
             ORDER BY entry.last_accessed_at ASC, entry.cache_entry_id ASC LIMIT 1",
                [root.as_bytes().as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?
            .map(|row| {
                Ok(StoredEntry {
                    entry_id: CacheEntryId::from_uuid(
                        uuid::Uuid::from_slice(&row.0).map_err(|_| corrupt_id())?,
                    ),
                    root_id: root,
                    binding_id: RootBindingId::from_uuid(
                        uuid::Uuid::from_slice(&row.7).map_err(|_| corrupt_id())?,
                    ),
                    relative_path: row.1,
                    path_key: row.2,
                    file_id: row.3,
                    size_bytes: row.4,
                    mtime_filetime: row.5,
                    ordinal: row.6,
                })
            })
            .transpose()
    }

    fn load_eviction(&self, id: CacheEvictionIntentId) -> Result<StoredEviction, DomainError> {
        let connection = self.database.open_read_connection()?;
        connection.query_row(
            "SELECT intent.cache_entry_id, intent.intent_state, intent.cache_root_id,
                    intent.root_binding_id, binding.absolute_path_display,
                    ownership.directory_file_id, entry.relative_path, entry.path_key,
                    entry.file_id, entry.size_bytes
             FROM cache_eviction_intent intent JOIN cache_entry entry ON entry.cache_entry_id=intent.cache_entry_id
             JOIN root_binding binding ON binding.binding_id=intent.root_binding_id
             JOIN cache_root_ownership ownership ON ownership.cache_root_id=intent.cache_root_id
             WHERE intent.eviction_intent_id=?1",
            [id.as_bytes().as_slice()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?, row.get::<_, String>(4)?, row.get::<_, Vec<u8>>(5)?,
                row.get::<_, String>(6)?, row.get::<_, Vec<u8>>(7)?, row.get::<_, Option<Vec<u8>>>(8)?, row.get::<_, i64>(9)?)),
        ).map_err(map_sqlite_error).and_then(|row| Ok(StoredEviction {
            intent_id: id,
            entry_id: CacheEntryId::from_uuid(uuid::Uuid::from_slice(&row.0).map_err(|_| corrupt_id())?),
            state: row.1,
            root_path: PathBuf::from(row.4), root_directory_file_id: Some(row.5),
            relative_path: row.6, file_id: row.8, size_bytes: row.9,
        }))
    }
}

#[derive(Debug)]
struct StoredCreateIntent {
    root_id: RootId,
    state: String,
    ownership_nonce: Vec<u8>,
    parent_snapshot_id: DirectorySnapshotId,
    parent_path: PathBuf,
    parent_path_key: Vec<u8>,
    parent_file_id: Option<Vec<u8>>,
    volume_key: Vec<u8>,
    child_name: String,
    child_path_key: Vec<u8>,
}

#[derive(Debug, Clone)]
struct StoredRoot {
    root_id: RootId,
    path: PathBuf,
    directory_file_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct StoredEntry {
    entry_id: CacheEntryId,
    root_id: RootId,
    binding_id: RootBindingId,
    relative_path: String,
    path_key: Vec<u8>,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
    ordinal: i64,
}

#[derive(Debug)]
struct StoredEviction {
    intent_id: CacheEvictionIntentId,
    entry_id: CacheEntryId,
    state: String,
    root_path: PathBuf,
    root_directory_file_id: Option<Vec<u8>>,
    relative_path: String,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
}

fn finish_eviction(
    database: &Database,
    intent: &StoredEviction,
    intent_state: &str,
    entry_state: &str,
    outcome: &str,
) -> Result<(), DomainError> {
    let intent_id = intent.intent_id;
    let entry_id = intent.entry_id;
    let intent_state = intent_state.to_string();
    let entry_state = entry_state.to_string();
    let outcome = outcome.to_string();
    database.writer()?.transaction(move |transaction| {
        transaction.execute(
            "INSERT INTO cache_eviction_fact(
                eviction_fact_id, eviction_intent_id, cache_entry_id, outcome, observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            (uuid::Uuid::new_v4().as_bytes().as_slice(), intent_id.as_bytes().as_slice(),
                entry_id.as_bytes().as_slice(), outcome, now_micros()),
        ).map_err(map_sqlite_error)?;
        transaction.execute(
            "UPDATE cache_eviction_intent SET intent_state=?1, finished_at=?2, row_version=row_version+1
             WHERE eviction_intent_id=?3 AND intent_state IN ('Prepared','Deleting','AwaitingRoot')",
            (intent_state, now_micros(), intent_id.as_bytes().as_slice()),
        ).map_err(map_sqlite_error)?;
        transaction.execute(
            "UPDATE cache_entry SET entry_state=?1 WHERE cache_entry_id=?2 AND entry_state='Live'",
            (entry_state, entry_id.as_bytes().as_slice()),
        ).map_err(map_sqlite_error)?;
        release_reservation(transaction, intent_id, "EvictionFinished")
    })
}

fn release_reservation(
    transaction: &Transaction<'_>,
    intent_id: CacheEvictionIntentId,
    reason: &str,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "UPDATE operation_reservation SET released_at=?1, release_reason=?2
         WHERE cache_eviction_intent_id=?3 AND released_at IS NULL",
            (now_micros(), reason, intent_id.as_bytes().as_slice()),
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn reject_root_change_while_busy(transaction: &Transaction<'_>) -> Result<(), DomainError> {
    let busy: bool = transaction
        .query_row(
            "SELECT EXISTS(
            SELECT 1 FROM task_intent WHERE lifecycle NOT IN ('Completed','Cancelled')
            UNION ALL SELECT 1 FROM cache_eviction_intent
              WHERE intent_state IN ('Prepared','Deleting','AwaitingRoot')
         )",
            (),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if busy {
        Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "存在未终结任务，不能切换 CacheRoot。",
        ))
    } else {
        Ok(())
    }
}

fn reject_overlap(transaction: &Transaction<'_>, candidate: &Path) -> Result<(), DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT binding.absolute_path_display FROM root
         JOIN root_binding binding ON binding.binding_id=root.current_binding_id
         WHERE root.lifecycle!='Tombstoned'",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map((), |row| row.get::<_, String>(0))
        .map_err(map_sqlite_error)?;
    for row in rows {
        if paths_overlap(candidate, Path::new(&row.map_err(map_sqlite_error)?)) {
            return Err(DomainError::new(
                ErrorCode::RootConflict,
                "缓存父目录与现有 Root 重叠。",
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

fn ensure_volume(
    transaction: &Transaction<'_>,
    probe: &DirectoryProbe,
) -> Result<VolumeId, DomainError> {
    if let Some(bytes) = transaction.query_row(
        "SELECT volume_id FROM volume_identity WHERE identity_format_version=1 AND strong_identity_key=?1",
        [&probe.volume_identity_key], |row| row.get::<_, Vec<u8>>(0),
    ).optional().map_err(map_sqlite_error)? {
        return Ok(VolumeId::from_uuid(uuid::Uuid::from_slice(&bytes).map_err(|_| corrupt_id())?));
    }
    let id = VolumeId::new();
    transaction
        .execute(
            "INSERT INTO volume_identity(
            volume_id, filesystem_kind, strong_identity_key, identity_format_version,
            identity_strength, created_at
         ) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            (
                id.as_bytes().as_slice(),
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
    Ok(id)
}

fn reject_deny_write_volume(
    transaction: &Transaction<'_>,
    volume: VolumeId,
) -> Result<(), DomainError> {
    let denied: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM volume_policy WHERE volume_id=?1)",
            [volume.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if denied {
        Err(DomainError::new(
            ErrorCode::VolumeWriteDenied,
            "该卷曾作为 ImportSource，不能承载缓存。",
        ))
    } else {
        Ok(())
    }
}

fn record_operation(
    transaction: &Transaction<'_>,
    operation: OperationId,
    kind: &str,
    request_hash: &[u8],
) -> Result<(), DomainError> {
    let existing = transaction
        .query_row(
            "SELECT operation_kind, request_hash FROM operation_event WHERE operation_id=?1",
            [operation.as_bytes().as_slice()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    if let Some((existing_kind, existing_hash)) = existing {
        if existing_kind == kind && existing_hash == request_hash {
            return Ok(());
        }
        return Err(DomainError::new(
            ErrorCode::DuplicateOperation,
            "OperationId 已用于不同请求。",
        ));
    }
    transaction.execute(
        "INSERT INTO operation_event(operation_id, operation_kind, request_hash, first_received_at)
         VALUES (?1, ?2, ?3, ?4)",
        (operation.as_bytes().as_slice(), kind, request_hash, now_micros()),
    ).map_err(map_sqlite_error)?;
    Ok(())
}

#[cfg(windows)]
fn configure_cache_create(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    options.share_mode(0x0000_0001);
}

#[cfg(not(windows))]
fn configure_cache_create(_options: &mut OpenOptions) {}

#[cfg(windows)]
fn open_cache_delete_handle(path: &Path) -> Result<File, DomainError> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options
        .access_mode(0x0001_0000 | 0x0000_0080)
        .share_mode(0x0000_0001)
        .custom_flags(0x0020_0000);
    options.open(path).map_err(cache_io_error)
}

#[cfg(not(windows))]
fn open_cache_delete_handle(_path: &Path) -> Result<File, DomainError> {
    Err(DomainError::new(
        ErrorCode::CapabilityDenied,
        "缓存淘汰只支持 Windows 已验证句柄。",
    ))
}

#[cfg(windows)]
fn delete_cache_file_by_handle(file: File) -> Result<(), DomainError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        )
    };
    if ok == 0 {
        return Err(cache_io_error(std::io::Error::last_os_error()));
    }
    drop(file);
    Ok(())
}

#[cfg(not(windows))]
fn delete_cache_file_by_handle(_file: File) -> Result<(), DomainError> {
    Err(DomainError::new(
        ErrorCode::CapabilityDenied,
        "缓存淘汰只支持 Windows 已验证句柄。",
    ))
}

#[cfg(windows)]
fn disk_space(path: &Path) -> Result<(u64, u64), DomainError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut available = 0_u64;
    let mut total = 0_u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            &mut total,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(cache_io_error(std::io::Error::last_os_error()));
    }
    Ok((available, total))
}

#[cfg(not(windows))]
fn disk_space(_path: &Path) -> Result<(u64, u64), DomainError> {
    Ok((u64::MAX / 2, u64::MAX / 2))
}

fn ceil_ten_percent(total: u64) -> Result<u64, DomainError> {
    total
        .checked_add(9)
        .map(|value| value / 10)
        .ok_or_else(|| DomainError::new(ErrorCode::CacheOwnership, "缓存卷容量计算溢出。"))
}

fn cache_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::CacheOwnership, "缓存文件操作失败。")
        .with_diagnostic(error.to_string())
}

fn corrupt_id() -> DomainError {
    DomainError::new(ErrorCode::DatabaseCorrupt, "缓存数据库标识损坏。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::{CatalogService, LibraryScanner, ScanRequest},
        roots::FujiLibraryService,
        safety::TestRoot,
    };

    struct CacheFixture {
        _parent: tempfile::TempDir,
        app_root: PathBuf,
        library_path: PathBuf,
        cache_parent: PathBuf,
        second_cache_parent: PathBuf,
        database: Arc<Database>,
        asset_id: AssetId,
        asset_version: i64,
    }

    fn cache_fixture(faults: FaultInjector) -> CacheFixture {
        let parent = tempfile::tempdir().unwrap();
        let app_root = TestRoot::create_in(parent.path()).unwrap();
        let library_path = parent.path().join("generated-library");
        let cache_parent = parent.path().join("cache-parent");
        let second_cache_parent = parent.path().join("cache-parent-two");
        fs::create_dir(&library_path).unwrap();
        fs::create_dir(&cache_parent).unwrap();
        fs::create_dir(&second_cache_parent).unwrap();
        fs::write(library_path.join("GENERATED.JPG"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        let database = Arc::new(Database::open_with_injector(app_root.path(), faults).unwrap());
        let binding = FujiLibraryService::new(database.clone(), app_root.path())
            .bind(&library_path)
            .unwrap();
        let catalog = Arc::new(CatalogService::new(database.clone()));
        LibraryScanner::new(catalog)
            .scan(ScanRequest {
                library_id: binding.library_id,
                root_id: binding.root_id,
                root_binding_id: binding.root_binding_id,
                root_path: library_path.clone(),
            })
            .unwrap();
        let (asset, version): (Vec<u8>, i64) = database
            .open_read_connection()
            .unwrap()
            .query_row(
                "SELECT asset_id, current_version FROM asset WHERE presence_state='Present'",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        CacheFixture {
            _parent: parent,
            app_root: app_root.path().to_path_buf(),
            library_path,
            cache_parent,
            second_cache_parent,
            database,
            asset_id: AssetId::from_uuid(uuid::Uuid::from_slice(&asset).unwrap()),
            asset_version: version,
        }
    }

    fn activate_cache(
        fixture: &CacheFixture,
        faults: FaultInjector,
    ) -> (Arc<CacheService>, CacheRootPlan) {
        let service = Arc::new(CacheService::new(
            fixture.database.clone(),
            &fixture.app_root,
            faults,
        ));
        let plan = service
            .prepare_root(&fixture.cache_parent, OperationId::new())
            .unwrap();
        service.execute_root_create(plan.intent_id).unwrap();
        (service, plan)
    }

    #[test]
    fn capacity_constants_match_frozen_binary_gigabytes() {
        assert_eq!(CACHE_DEFAULT_BYTES, 32 * 1_073_741_824);
        assert_eq!(CACHE_MAX_BYTES, 64 * 1_073_741_824);
        assert_eq!(CACHE_MIN_RESERVE_BYTES, 20 * 1_073_741_824);
    }

    #[test]
    fn reserve_percentage_uses_checked_ceiling() {
        assert_eq!(ceil_ten_percent(11).unwrap(), 2);
        assert_eq!(ceil_ten_percent(20).unwrap(), 2);
    }

    #[test]
    fn cache_root_intent_is_committed_before_mkdir_and_operation_is_idempotent() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let service =
            CacheService::new(fixture.database.clone(), &fixture.app_root, faults.clone());
        let operation = OperationId::new();
        let first = service
            .prepare_root(&fixture.cache_parent, operation)
            .unwrap();
        let repeated = service
            .prepare_root(&fixture.cache_parent, operation)
            .unwrap();
        assert_eq!(repeated, first);
        assert!(!first.child_path.exists());
        service.execute_root_create(first.intent_id).unwrap();
        assert!(first.child_path.exists());
    }

    #[test]
    fn root_create_crash_after_mkdir_keeps_unknown_directory_and_blocks_retry() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let service =
            CacheService::new(fixture.database.clone(), &fixture.app_root, faults.clone());
        let plan = service
            .prepare_root(&fixture.cache_parent, OperationId::new())
            .unwrap();
        faults.fail_on_hit(FaultPoint::Win32AfterCall, 1).unwrap();
        assert!(service.execute_root_create(plan.intent_id).is_err());
        assert!(plan.child_path.exists());
        assert!(service.reconcile_root_create(plan.intent_id).is_err());
        let state: String = fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row(
                "SELECT intent_state FROM cache_root_create_intent WHERE create_intent_id=?1",
                [plan.intent_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "NeedsAttention");
        assert!(service.reconcile_root_create(plan.intent_id).is_err());
        assert_eq!(fs::read_dir(&fixture.cache_parent).unwrap().count(), 1);
    }

    #[test]
    fn create_intent_commit_failure_performs_no_mkdir() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let service =
            CacheService::new(fixture.database.clone(), &fixture.app_root, faults.clone());
        faults
            .fail_on_hit(FaultPoint::SqliteBeforeCommit, 1)
            .unwrap();
        assert!(
            service
                .prepare_root(&fixture.cache_parent, OperationId::new())
                .is_err()
        );
        assert_eq!(fs::read_dir(&fixture.cache_parent).unwrap().count(), 0);
        let intents: i64 = fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM cache_root_create_intent", (), |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(intents, 0);
    }

    #[test]
    fn cache_entry_eviction_preserves_unknown_files_and_rebuilds_new_incarnation() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let (service, plan) = activate_cache(&fixture, faults);
        let first = service
            .get_or_create(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Preview,
                b"preview-v1",
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        let unknown = plan.child_path.join("user-unknown-file.txt");
        fs::write(&unknown, b"must be preserved").unwrap();
        fixture
            .database
            .writer()
            .unwrap()
            .transaction(|transaction| {
                transaction
                    .execute("UPDATE cache_policy SET capacity_bytes=1 WHERE slot=1", ())
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(service.evict_lru_until_within_limits().unwrap(), 1);
        assert!(unknown.exists());
        let old_state: String = fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row(
                "SELECT entry_state FROM cache_entry WHERE cache_entry_id=?1",
                [first.cache_entry_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_state, "Evicted");
        fixture
            .database
            .writer()
            .unwrap()
            .transaction(|transaction| {
                transaction
                    .execute(
                        "UPDATE cache_policy SET capacity_bytes=?1 WHERE slot=1",
                        [CACHE_DEFAULT_BYTES],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .unwrap();
        let rebuilt = service
            .get_or_create(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Preview,
                b"preview-v1",
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        assert_eq!(rebuilt.entry_ordinal, 2);
        assert_ne!(rebuilt.relative_path, first.relative_path);
        assert!(unknown.exists());
        let facts: i64 = fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM cache_eviction_fact WHERE cache_entry_id=?1",
                [first.cache_entry_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(facts, 1);
    }

    #[test]
    fn deletion_commit_gap_recovers_same_intent_as_observed_missing() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let (service, _plan) = activate_cache(&fixture, faults.clone());
        let entry = service
            .get_or_create(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Thumbnail,
                b"thumb-v1",
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        fixture
            .database
            .writer()
            .unwrap()
            .transaction(|transaction| {
                transaction
                    .execute("UPDATE cache_policy SET capacity_bytes=1 WHERE slot=1", ())
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .unwrap();
        faults.fail_on_hit(FaultPoint::Win32AfterCall, 1).unwrap();
        assert!(service.evict_lru_until_within_limits().is_err());
        assert!(!entry.absolute_path.exists());
        assert_eq!(service.reconcile_evictions().unwrap(), 1);
        let connection = fixture.database.open_read_connection().unwrap();
        let result: (String, String, String, i64) = connection
            .query_row(
                "SELECT intent.intent_state, entry.entry_state, fact.outcome,
                    (SELECT COUNT(*) FROM operation_reservation reservation
                     WHERE reservation.cache_eviction_intent_id=intent.eviction_intent_id
                       AND reservation.released_at IS NULL)
             FROM cache_eviction_intent intent
             JOIN cache_entry entry ON entry.cache_entry_id=intent.cache_entry_id
             JOIN cache_eviction_fact fact ON fact.eviction_intent_id=intent.eviction_intent_id",
                (),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            result,
            (
                "AlreadyMissing".into(),
                "Missing".into(),
                "ObservedMissingAfterIntent".into(),
                0
            )
        );
    }

    #[test]
    fn eviction_identity_mismatch_enters_needs_attention_without_deleting() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let (service, _plan) = activate_cache(&fixture, faults);
        let entry = service
            .get_or_create(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Preview,
                b"identity-mismatch",
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        let mut changed = OpenOptions::new()
            .append(true)
            .open(&entry.absolute_path)
            .unwrap();
        changed.write_all(b"changed").unwrap();
        changed.sync_all().unwrap();
        drop(changed);
        fixture
            .database
            .writer()
            .unwrap()
            .transaction(|transaction| {
                transaction
                    .execute("UPDATE cache_policy SET capacity_bytes=1 WHERE slot=1", ())
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .unwrap();
        assert!(service.evict_lru_until_within_limits().is_err());
        assert!(entry.absolute_path.exists());
        let connection = fixture.database.open_read_connection().unwrap();
        let state: (String, String, i64) = connection
            .query_row(
                "SELECT intent.intent_state, entry.entry_state,
                    (SELECT COUNT(*) FROM cache_eviction_intent)
             FROM cache_eviction_intent intent
             JOIN cache_entry entry ON entry.cache_entry_id=intent.cache_entry_id",
                (),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, ("NeedsAttention".into(), "NeedsAttention".into(), 1));
        assert!(
            service
                .get_or_create(
                    fixture.asset_id,
                    fixture.asset_version,
                    CacheKind::Preview,
                    b"identity-mismatch",
                    &[0xff, 0xd8, 0xff, 0xd9],
                )
                .is_err()
        );
        let intents: i64 = connection
            .query_row("SELECT COUNT(*) FROM cache_eviction_intent", (), |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(intents, 1);
    }

    #[test]
    fn active_and_draining_roots_can_own_same_render_key() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let (service, first_plan) = activate_cache(&fixture, faults);
        let first = service
            .get_or_create(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Preview,
                b"shared-render",
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        let second_plan = service
            .prepare_root(&fixture.second_cache_parent, OperationId::new())
            .unwrap();
        service.execute_root_create(second_plan.intent_id).unwrap();
        let second = service
            .get_or_create(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Preview,
                b"shared-render",
                &[0xff, 0xd8, 0xff, 0xd9],
            )
            .unwrap();
        assert_ne!(first.cache_root_id, second.cache_root_id);
        assert!(first_plan.child_path.exists());
        assert!(second_plan.child_path.exists());
        let connection = fixture.database.open_read_connection().unwrap();
        let states: (i64, i64) = connection
            .query_row(
                "SELECT
                SUM(CASE WHEN ownership_state='Active' THEN 1 ELSE 0 END),
                SUM(CASE WHEN ownership_state='Draining' THEN 1 ELSE 0 END)
             FROM cache_root_ownership",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, (1, 1));
        let entries: i64 = connection.query_row(
            "SELECT COUNT(*) FROM cache_entry WHERE asset_id=?1 AND render_key=?2 AND entry_state='Live'",
            (fixture.asset_id.as_bytes().as_slice(), b"shared-render".as_slice()),
            |row| row.get(0),
        ).unwrap();
        assert_eq!(entries, 2);
    }

    #[test]
    fn preview_materialization_does_not_change_library_tree() {
        let faults = FaultInjector::default();
        let fixture = cache_fixture(faults.clone());
        let before = fs::metadata(fixture.library_path.join("GENERATED.JPG")).unwrap();
        let before_hash =
            Sha256::digest(fs::read(fixture.library_path.join("GENERATED.JPG")).unwrap());
        let (cache, _plan) = activate_cache(&fixture, faults);
        let preview = PreviewService::new(fixture.database.clone(), cache);
        let entry = preview
            .materialize(
                fixture.asset_id,
                fixture.asset_version,
                CacheKind::Preview,
                b"source-preview",
            )
            .unwrap();
        assert!(entry.absolute_path.exists());
        let after = fs::metadata(fixture.library_path.join("GENERATED.JPG")).unwrap();
        let after_hash =
            Sha256::digest(fs::read(fixture.library_path.join("GENERATED.JPG")).unwrap());
        assert_eq!(before_hash.as_slice(), after_hash.as_slice());
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().ok(), after.modified().ok());
        assert_eq!(fs::read_dir(&fixture.library_path).unwrap().count(), 1);
    }
}
