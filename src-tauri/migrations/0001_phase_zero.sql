PRAGMA user_version = 1;

CREATE TABLE schema_migration (
    version INTEGER PRIMARY KEY CHECK (version >= 1),
    checksum TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    app_build_hash TEXT NOT NULL
) STRICT;

CREATE TABLE build_admission (
    feature TEXT PRIMARY KEY CHECK (feature IN ('Import', 'Recycle')),
    binary_sha256 TEXT NOT NULL,
    package_lock_sha256 TEXT NOT NULL,
    cargo_lock_sha256 TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    migration_checksum TEXT NOT NULL,
    safety_report_version INTEGER NOT NULL CHECK (safety_report_version >= 1),
    admitted_at INTEGER NOT NULL
) STRICT;

CREATE TABLE library (
    library_id BLOB PRIMARY KEY CHECK (length(library_id) = 16),
    library_kind TEXT NOT NULL CHECK (library_kind = 'Fuji'),
    display_name TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('Active', 'Inactive', 'Tombstoned')),
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    created_at INTEGER NOT NULL,
    tombstoned_at INTEGER,
    CHECK ((lifecycle = 'Tombstoned') = (tombstoned_at IS NOT NULL))
) STRICT;

CREATE TRIGGER library_tombstone_immutable
BEFORE UPDATE ON library
WHEN OLD.lifecycle = 'Tombstoned'
BEGIN
    SELECT RAISE(ABORT, 'library tombstone is immutable');
END;

CREATE TABLE current_library (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    library_id BLOB NOT NULL UNIQUE CHECK (length(library_id) = 16),
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE volume_identity (
    volume_id BLOB PRIMARY KEY CHECK (length(volume_id) = 16),
    filesystem_kind TEXT NOT NULL CHECK (filesystem_kind IN ('NTFS', 'exFAT', 'ReFS', 'Other')),
    strong_identity_key BLOB,
    identity_format_version INTEGER NOT NULL CHECK (identity_format_version >= 1),
    identity_strength TEXT NOT NULL CHECK (identity_strength IN ('Strong', 'UserConfirmedLimited')),
    created_at INTEGER NOT NULL,
    CHECK (
        (identity_strength = 'Strong' AND strong_identity_key IS NOT NULL)
        OR (identity_strength = 'UserConfirmedLimited')
    )
) STRICT;

CREATE UNIQUE INDEX ux_volume_identity_strong
ON volume_identity(identity_format_version, strong_identity_key)
WHERE strong_identity_key IS NOT NULL;

CREATE TABLE volume_policy (
    volume_id BLOB PRIMARY KEY CHECK (length(volume_id) = 16),
    deny_write INTEGER NOT NULL CHECK (deny_write = 1),
    reason TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (volume_id) REFERENCES volume_identity(volume_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER volume_policy_no_update
BEFORE UPDATE ON volume_policy BEGIN
    SELECT RAISE(ABORT, 'volume_policy is append-only');
END;

CREATE TRIGGER volume_policy_no_delete
BEFORE DELETE ON volume_policy BEGIN
    SELECT RAISE(ABORT, 'volume_policy is append-only');
END;

CREATE TABLE root (
    root_id BLOB PRIMARY KEY CHECK (length(root_id) = 16),
    library_id BLOB CHECK (library_id IS NULL OR length(library_id) = 16),
    root_kind TEXT NOT NULL CHECK (root_kind IN ('FujiLibrary', 'ImportSource', 'AppDataRoot', 'CacheRoot')),
    capability_mask INTEGER NOT NULL CHECK (capability_mask >= 0),
    current_binding_id BLOB CHECK (current_binding_id IS NULL OR length(current_binding_id) = 16),
    root_version INTEGER NOT NULL DEFAULT 1 CHECK (root_version >= 1),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('Online', 'Offline', 'IdentityMismatch', 'Tombstoned')),
    created_at INTEGER NOT NULL,
    tombstoned_at INTEGER,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    CHECK ((root_kind = 'FujiLibrary') = (library_id IS NOT NULL)),
    CHECK ((lifecycle = 'Tombstoned') = (tombstoned_at IS NOT NULL))
) STRICT;

CREATE TRIGGER root_tombstone_immutable
BEFORE UPDATE ON root
WHEN OLD.lifecycle = 'Tombstoned'
BEGIN
    SELECT RAISE(ABORT, 'root tombstone is immutable');
END;

CREATE UNIQUE INDEX ux_root_active_fuji_per_library
ON root(library_id)
WHERE root_kind = 'FujiLibrary' AND lifecycle != 'Tombstoned';

CREATE TABLE root_binding (
    binding_id BLOB PRIMARY KEY CHECK (length(binding_id) = 16),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    binding_generation INTEGER NOT NULL CHECK (binding_generation >= 1),
    volume_id BLOB NOT NULL CHECK (length(volume_id) = 16),
    absolute_path_display TEXT NOT NULL,
    absolute_path_key BLOB NOT NULL,
    root_directory_file_id BLOB,
    filesystem_kind TEXT NOT NULL CHECK (filesystem_kind IN ('NTFS', 'exFAT', 'ReFS', 'Other')),
    evidence_level TEXT NOT NULL CHECK (evidence_level IN ('Strong', 'UserConfirmedLimited')),
    reparse_checked INTEGER NOT NULL CHECK (reparse_checked IN (0, 1)),
    binding_state TEXT NOT NULL CHECK (binding_state IN ('Active', 'Superseded', 'Unverified')),
    supersedes_binding_id BLOB CHECK (supersedes_binding_id IS NULL OR length(supersedes_binding_id) = 16),
    verified_at INTEGER NOT NULL,
    UNIQUE (root_id, binding_generation),
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (volume_id) REFERENCES volume_identity(volume_id) ON DELETE RESTRICT,
    FOREIGN KEY (supersedes_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_root_binding_active
ON root_binding(root_id)
WHERE binding_state = 'Active';

CREATE TABLE asset (
    asset_id BLOB PRIMARY KEY CHECK (length(asset_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    media_kind TEXT NOT NULL CHECK (media_kind IN ('Jpg', 'Raf')),
    current_version INTEGER NOT NULL CHECK (current_version >= 1),
    current_path_key BLOB NOT NULL,
    current_file_id BLOB,
    presence_state TEXT NOT NULL CHECK (presence_state IN ('Present', 'Missing', 'Tombstoned')),
    deletion_supported INTEGER NOT NULL CHECK (deletion_supported IN (0, 1)),
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    created_at INTEGER NOT NULL,
    tombstoned_at INTEGER,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    CHECK ((presence_state = 'Tombstoned') = (tombstoned_at IS NOT NULL))
) STRICT;

CREATE TRIGGER asset_tombstone_immutable
BEFORE UPDATE ON asset
WHEN OLD.presence_state = 'Tombstoned'
BEGIN
    SELECT RAISE(ABORT, 'asset tombstone is immutable');
END;

CREATE UNIQUE INDEX ux_asset_active_path
ON asset(root_id, current_path_key)
WHERE presence_state != 'Tombstoned';

CREATE UNIQUE INDEX ux_asset_active_file
ON asset(root_id, current_file_id)
WHERE presence_state != 'Tombstoned' AND current_file_id IS NOT NULL;

CREATE TABLE asset_revision (
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    relative_path TEXT NOT NULL,
    path_key BLOB NOT NULL,
    path_key_version INTEGER NOT NULL CHECK (path_key_version >= 1),
    file_id BLOB,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    mtime_filetime INTEGER NOT NULL,
    content_hash_algorithm TEXT,
    content_hash BLOB,
    presence_state TEXT NOT NULL CHECK (presence_state IN ('Present', 'Missing', 'Tombstoned')),
    hard_link_count INTEGER NOT NULL CHECK (hard_link_count >= 1),
    observed_at INTEGER NOT NULL,
    scan_run_id BLOB,
    PRIMARY KEY (asset_id, asset_version),
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    CHECK ((content_hash_algorithm IS NULL) = (content_hash IS NULL))
) STRICT;

CREATE TRIGGER asset_revision_no_update
BEFORE UPDATE ON asset_revision BEGIN
    SELECT RAISE(ABORT, 'asset_revision is immutable');
END;

CREATE TRIGGER asset_revision_no_delete
BEFORE DELETE ON asset_revision BEGIN
    SELECT RAISE(ABORT, 'asset_revision is immutable');
END;

CREATE TABLE photo (
    photo_id BLOB PRIMARY KEY CHECK (length(photo_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    photo_version INTEGER NOT NULL CHECK (photo_version >= 1),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('Active', 'Tombstoned')),
    date_class TEXT NOT NULL CHECK (date_class IN ('Known', 'Unknown')),
    capture_local_key TEXT,
    natural_name_display TEXT NOT NULL,
    natural_sort_key BLOB NOT NULL,
    sort_key_version INTEGER NOT NULL CHECK (sort_key_version >= 1),
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    created_at INTEGER NOT NULL,
    tombstoned_at INTEGER,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    CHECK ((date_class = 'Known') = (capture_local_key IS NOT NULL)),
    CHECK ((lifecycle = 'Tombstoned') = (tombstoned_at IS NOT NULL))
) STRICT;

CREATE TRIGGER photo_tombstone_immutable
BEFORE UPDATE ON photo
WHEN OLD.lifecycle = 'Tombstoned'
BEGIN
    SELECT RAISE(ABORT, 'photo tombstone is immutable');
END;

CREATE TABLE photo_asset (
    photo_asset_id BLOB PRIMARY KEY CHECK (length(photo_asset_id) = 16),
    photo_id BLOB NOT NULL CHECK (length(photo_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    role TEXT NOT NULL CHECK (role IN ('Jpg', 'Raf')),
    pair_evidence_id BLOB,
    from_photo_version INTEGER NOT NULL CHECK (from_photo_version >= 1),
    to_photo_version INTEGER CHECK (to_photo_version IS NULL OR to_photo_version > from_photo_version),
    FOREIGN KEY (photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_photo_asset_current_role
ON photo_asset(photo_id, role)
WHERE to_photo_version IS NULL;

CREATE UNIQUE INDEX ux_photo_asset_current_asset
ON photo_asset(asset_id)
WHERE to_photo_version IS NULL;

CREATE UNIQUE INDEX ux_photo_asset_current_relation
ON photo_asset(photo_id, asset_id)
WHERE to_photo_version IS NULL;

CREATE TABLE photo_alias (
    old_photo_id BLOB PRIMARY KEY CHECK (length(old_photo_id) = 16),
    canonical_photo_id BLOB NOT NULL CHECK (length(canonical_photo_id) = 16),
    reason TEXT NOT NULL CHECK (reason IN ('Merge', 'SplitNavigation')),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (old_photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (canonical_photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    CHECK (old_photo_id != canonical_photo_id)
) STRICT;

CREATE TRIGGER photo_alias_no_chain
BEFORE INSERT ON photo_alias
WHEN EXISTS (SELECT 1 FROM photo_alias WHERE old_photo_id = NEW.canonical_photo_id)
   OR EXISTS (SELECT 1 FROM photo_alias WHERE canonical_photo_id = NEW.old_photo_id)
BEGIN
    SELECT RAISE(ABORT, 'photo aliases cannot chain or cycle');
END;

CREATE TABLE viewing_cursor (
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    sort_mode TEXT NOT NULL CHECK (sort_mode IN ('CaptureDesc', 'CaptureAsc')),
    photo_id BLOB NOT NULL CHECK (length(photo_id) = 16),
    photo_version INTEGER NOT NULL CHECK (photo_version >= 1),
    date_class TEXT NOT NULL CHECK (date_class IN ('Known', 'Unknown')),
    capture_local_key TEXT,
    natural_name_display TEXT NOT NULL,
    natural_sort_key BLOB NOT NULL,
    anchor_photo_id BLOB NOT NULL CHECK (length(anchor_photo_id) = 16),
    sort_key_version INTEGER NOT NULL CHECK (sort_key_version >= 1),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (library_id, sort_mode),
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (anchor_photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    CHECK ((date_class = 'Known') = (capture_local_key IS NOT NULL))
) STRICT;

CREATE TABLE file_identity_snapshot (
    snapshot_id BLOB PRIMARY KEY CHECK (length(snapshot_id) = 16),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    volume_id BLOB NOT NULL CHECK (length(volume_id) = 16),
    relative_path TEXT NOT NULL,
    path_key BLOB NOT NULL,
    path_key_version INTEGER NOT NULL CHECK (path_key_version >= 1),
    file_id BLOB,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    mtime_filetime INTEGER NOT NULL,
    content_hash_algorithm TEXT,
    content_hash BLOB,
    captured_at INTEGER NOT NULL,
    evidence_level TEXT NOT NULL CHECK (evidence_level IN ('Strong', 'UserConfirmedLimited')),
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    FOREIGN KEY (volume_id) REFERENCES volume_identity(volume_id) ON DELETE RESTRICT,
    CHECK ((content_hash_algorithm IS NULL) = (content_hash IS NULL))
) STRICT;

CREATE TRIGGER file_identity_snapshot_no_update
BEFORE UPDATE ON file_identity_snapshot BEGIN
    SELECT RAISE(ABORT, 'file identity snapshots are immutable');
END;

CREATE TRIGGER file_identity_snapshot_no_delete
BEFORE DELETE ON file_identity_snapshot BEGIN
    SELECT RAISE(ABORT, 'file identity snapshots are immutable');
END;

CREATE TABLE operation_event (
    operation_id BLOB PRIMARY KEY CHECK (length(operation_id) = 16),
    operation_kind TEXT NOT NULL,
    target_id BLOB,
    request_hash BLOB NOT NULL,
    first_received_at INTEGER NOT NULL,
    final_result_code TEXT,
    related_entity_id BLOB
) STRICT;

CREATE TRIGGER operation_event_no_delete
BEFORE DELETE ON operation_event BEGIN
    SELECT RAISE(ABORT, 'operation events are append-only');
END;

CREATE TABLE task_intent (
    task_id BLOB PRIMARY KEY CHECK (length(task_id) = 16),
    task_kind TEXT NOT NULL CHECK (task_kind IN ('Import', 'Recycle')),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('Prepared', 'Active', 'AwaitingUser', 'AwaitingSource', 'NeedsAttention', 'Completed', 'Cancelled')),
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    CHECK ((lifecycle IN ('Completed', 'Cancelled')) = (resolved_at IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX ux_task_intent_single_non_terminal
ON task_intent((1))
WHERE lifecycle NOT IN ('Completed', 'Cancelled');

CREATE TRIGGER task_intent_transition
BEFORE UPDATE OF lifecycle ON task_intent
WHEN NOT (
    (OLD.lifecycle = 'Prepared' AND NEW.lifecycle IN ('Active', 'AwaitingUser', 'AwaitingSource', 'NeedsAttention', 'Cancelled'))
    OR (OLD.lifecycle = 'Active' AND NEW.lifecycle IN ('AwaitingUser', 'AwaitingSource', 'NeedsAttention', 'Completed', 'Cancelled'))
    OR (OLD.lifecycle IN ('AwaitingUser', 'AwaitingSource', 'NeedsAttention') AND NEW.lifecycle IN ('Active', 'Cancelled'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal task transition');
END;

CREATE TRIGGER task_intent_terminal_no_update
BEFORE UPDATE ON task_intent
WHEN OLD.lifecycle IN ('Completed', 'Cancelled')
BEGIN
    SELECT RAISE(ABORT, 'terminal task is immutable');
END;

CREATE TRIGGER task_intent_terminal_no_delete
BEFORE DELETE ON task_intent
WHEN OLD.lifecycle IN ('Completed', 'Cancelled')
BEGIN
    SELECT RAISE(ABORT, 'terminal task is immutable');
END;

CREATE TABLE task_attempt (
    attempt_id BLOB PRIMARY KEY CHECK (length(attempt_id) = 16),
    task_id BLOB NOT NULL CHECK (length(task_id) = 16),
    attempt_kind TEXT NOT NULL CHECK (attempt_kind IN ('Import', 'Recycle')),
    attempt_ordinal INTEGER NOT NULL CHECK (attempt_ordinal >= 1),
    operation_id BLOB NOT NULL UNIQUE CHECK (length(operation_id) = 16),
    created_at INTEGER NOT NULL,
    UNIQUE (task_id, attempt_ordinal),
    FOREIGN KEY (task_id) REFERENCES task_intent(task_id) ON DELETE RESTRICT,
    FOREIGN KEY (operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER task_attempt_no_update
BEFORE UPDATE ON task_attempt BEGIN
    SELECT RAISE(ABORT, 'task attempt identity is immutable');
END;

CREATE TRIGGER task_attempt_no_delete
BEFORE DELETE ON task_attempt BEGIN
    SELECT RAISE(ABORT, 'task attempt identity is immutable');
END;

CREATE TRIGGER task_attempt_kind_matches_parent
BEFORE INSERT ON task_attempt
WHEN NOT EXISTS (
    SELECT 1 FROM task_intent
    WHERE task_id = NEW.task_id AND task_kind = NEW.attempt_kind
)
BEGIN
    SELECT RAISE(ABORT, 'task attempt kind must match parent task');
END;

CREATE TABLE file_write_lease (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    task_id BLOB CHECK (task_id IS NULL OR length(task_id) = 16),
    lease_generation INTEGER NOT NULL CHECK (lease_generation >= 0),
    acquired_at INTEGER,
    released_at INTEGER,
    row_version INTEGER NOT NULL CHECK (row_version >= 1),
    FOREIGN KEY (task_id) REFERENCES task_intent(task_id) ON DELETE RESTRICT,
    CHECK ((task_id IS NULL AND released_at IS NOT NULL) OR (task_id IS NOT NULL AND acquired_at IS NOT NULL))
) STRICT;

INSERT INTO file_write_lease(slot, task_id, lease_generation, acquired_at, released_at, row_version)
VALUES (1, NULL, 0, NULL, 0, 1);

CREATE TRIGGER file_write_lease_monotonic
BEFORE UPDATE ON file_write_lease
WHEN NEW.lease_generation <= OLD.lease_generation
  OR NEW.row_version != OLD.row_version + 1
BEGIN
    SELECT RAISE(ABORT, 'file write lease generation and version must advance');
END;

CREATE TABLE operation_reservation (
    reservation_id BLOB PRIMARY KEY CHECK (length(reservation_id) = 16),
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('TaskAttempt', 'CacheEvictionIntent')),
    task_id BLOB CHECK (task_id IS NULL OR length(task_id) = 16),
    attempt_id BLOB CHECK (attempt_id IS NULL OR length(attempt_id) = 16),
    lease_generation INTEGER,
    cache_eviction_intent_id BLOB CHECK (cache_eviction_intent_id IS NULL OR length(cache_eviction_intent_id) = 16),
    cache_entry_id BLOB CHECK (cache_entry_id IS NULL OR length(cache_entry_id) = 16),
    reservation_kind TEXT NOT NULL CHECK (reservation_kind IN ('SourceRead', 'FinalPath', 'PartialPath', 'ExistingAsset', 'RecycleAsset', 'CacheEntry')),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    asset_id BLOB CHECK (asset_id IS NULL OR length(asset_id) = 16),
    file_id BLOB,
    path_key BLOB,
    created_at INTEGER NOT NULL,
    released_at INTEGER,
    release_reason TEXT,
    FOREIGN KEY (task_id) REFERENCES task_intent(task_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_id) REFERENCES task_attempt(attempt_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (cache_eviction_intent_id) REFERENCES cache_eviction_intent(eviction_intent_id) ON DELETE RESTRICT,
    FOREIGN KEY (cache_entry_id) REFERENCES cache_entry(cache_entry_id) ON DELETE RESTRICT,
    CHECK (asset_id IS NOT NULL OR file_id IS NOT NULL OR path_key IS NOT NULL),
    CHECK (
        (
            owner_kind = 'TaskAttempt'
            AND task_id IS NOT NULL AND attempt_id IS NOT NULL AND lease_generation IS NOT NULL
            AND cache_eviction_intent_id IS NULL AND cache_entry_id IS NULL
            AND reservation_kind != 'CacheEntry'
        )
        OR
        (
            owner_kind = 'CacheEvictionIntent'
            AND task_id IS NULL AND attempt_id IS NULL AND lease_generation IS NULL
            AND cache_eviction_intent_id IS NOT NULL AND cache_entry_id IS NOT NULL
            AND reservation_kind = 'CacheEntry' AND asset_id IS NULL
        )
    ),
    CHECK ((released_at IS NULL) = (release_reason IS NULL))
) STRICT;

CREATE UNIQUE INDEX ux_reservation_active_path
ON operation_reservation(root_id, path_key)
WHERE released_at IS NULL AND path_key IS NOT NULL;

CREATE UNIQUE INDEX ux_reservation_active_file
ON operation_reservation(root_id, file_id)
WHERE released_at IS NULL AND file_id IS NOT NULL;

CREATE UNIQUE INDEX ux_reservation_attempt_asset
ON operation_reservation(attempt_id, reservation_kind, root_id, asset_id)
WHERE owner_kind = 'TaskAttempt' AND released_at IS NULL AND asset_id IS NOT NULL;

CREATE UNIQUE INDEX ux_reservation_cache_intent
ON operation_reservation(cache_eviction_intent_id)
WHERE owner_kind = 'CacheEvictionIntent';

CREATE TRIGGER operation_reservation_task_owner_consistent
BEFORE INSERT ON operation_reservation
WHEN NEW.owner_kind = 'TaskAttempt' AND NOT EXISTS (
    SELECT 1
    FROM task_attempt AS attempt
    JOIN file_write_lease AS lease ON lease.slot = 1
    WHERE attempt.attempt_id = NEW.attempt_id
      AND attempt.task_id = NEW.task_id
      AND lease.task_id = NEW.task_id
      AND lease.lease_generation = NEW.lease_generation
)
BEGIN
    SELECT RAISE(ABORT, 'reservation owner or lease generation is stale');
END;

CREATE TRIGGER operation_reservation_binding_consistent
BEFORE INSERT ON operation_reservation
WHEN NOT EXISTS (
    SELECT 1 FROM root_binding
    WHERE binding_id = NEW.root_binding_id AND root_id = NEW.root_id
)
BEGIN
    SELECT RAISE(ABORT, 'reservation root binding does not belong to root');
END;

CREATE TRIGGER operation_reservation_release_only
BEFORE UPDATE ON operation_reservation
WHEN OLD.released_at IS NOT NULL
  OR NEW.released_at IS NULL
  OR NEW.release_reason IS NULL
  OR NEW.reservation_id IS NOT OLD.reservation_id
  OR NEW.owner_kind IS NOT OLD.owner_kind
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.attempt_id IS NOT OLD.attempt_id
  OR NEW.lease_generation IS NOT OLD.lease_generation
  OR NEW.cache_eviction_intent_id IS NOT OLD.cache_eviction_intent_id
  OR NEW.cache_entry_id IS NOT OLD.cache_entry_id
  OR NEW.reservation_kind IS NOT OLD.reservation_kind
  OR NEW.root_id IS NOT OLD.root_id
  OR NEW.root_binding_id IS NOT OLD.root_binding_id
  OR NEW.asset_id IS NOT OLD.asset_id
  OR NEW.file_id IS NOT OLD.file_id
  OR NEW.path_key IS NOT OLD.path_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'operation reservation only supports one soft release');
END;

CREATE TRIGGER operation_reservation_no_delete
BEFORE DELETE ON operation_reservation BEGIN
    SELECT RAISE(ABORT, 'operation reservation history is immutable');
END;

CREATE TABLE import_job (
    job_id BLOB PRIMARY KEY CHECK (length(job_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    source_root_id BLOB NOT NULL CHECK (length(source_root_id) = 16),
    target_root_id BLOB NOT NULL CHECK (length(target_root_id) = 16),
    target_binding_id BLOB NOT NULL CHECK (length(target_binding_id) = 16),
    job_state TEXT NOT NULL CHECK (job_state IN ('Prepared', 'ValidatingPlan', 'AwaitingPlanConfirmation', 'ReadyToStart', 'AwaitingSpace', 'Running', 'CancelRequested', 'AwaitingSource', 'Completed', 'CompletedWithErrors', 'NeedsAttention', 'Cancelled')),
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    FOREIGN KEY (job_id) REFERENCES task_intent(task_id) ON DELETE RESTRICT,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_plan_generation (
    generation_id BLOB PRIMARY KEY CHECK (length(generation_id) = 16),
    job_id BLOB NOT NULL CHECK (length(job_id) = 16),
    generation_no INTEGER NOT NULL CHECK (generation_no >= 1),
    generation_state TEXT NOT NULL CHECK (generation_state IN ('Proposed', 'Confirmed', 'Superseded', 'Rejected')),
    target_root_id BLOB NOT NULL CHECK (length(target_root_id) = 16),
    target_binding_id BLOB NOT NULL CHECK (length(target_binding_id) = 16),
    planned_source_files INTEGER NOT NULL CHECK (planned_source_files >= 0),
    planned_copy_files INTEGER NOT NULL CHECK (planned_copy_files >= 0),
    planned_new_bytes INTEGER NOT NULL CHECK (planned_new_bytes >= 0),
    generation_reason TEXT NOT NULL CHECK (generation_reason IN ('InitialPreflight', 'DuplicateChanged', 'TargetChanged', 'NameFamilyChanged', 'RetryRefresh')),
    created_at INTEGER NOT NULL,
    confirmed_at INTEGER,
    UNIQUE (job_id, generation_no),
    FOREIGN KEY (job_id) REFERENCES import_job(job_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_import_generation_proposed
ON import_plan_generation(job_id)
WHERE generation_state = 'Proposed';

CREATE UNIQUE INDEX ux_import_generation_confirmed
ON import_plan_generation(job_id)
WHERE generation_state = 'Confirmed';

CREATE TRIGGER import_generation_payload_immutable
BEFORE UPDATE ON import_plan_generation
WHEN OLD.job_id != NEW.job_id
  OR OLD.generation_no != NEW.generation_no
  OR OLD.target_root_id != NEW.target_root_id
  OR OLD.target_binding_id != NEW.target_binding_id
  OR OLD.planned_source_files != NEW.planned_source_files
  OR OLD.planned_copy_files != NEW.planned_copy_files
  OR OLD.planned_new_bytes != NEW.planned_new_bytes
  OR OLD.generation_reason != NEW.generation_reason
BEGIN
    SELECT RAISE(ABORT, 'import plan payload is immutable');
END;

CREATE TRIGGER import_generation_transition
BEFORE UPDATE OF generation_state ON import_plan_generation
WHEN NOT (
    (OLD.generation_state = 'Proposed' AND NEW.generation_state IN ('Confirmed', 'Rejected'))
    OR (OLD.generation_state = 'Confirmed' AND NEW.generation_state = 'Superseded')
)
BEGIN
    SELECT RAISE(ABORT, 'illegal plan generation transition');
END;

CREATE TABLE import_plan_validation (
    validation_id BLOB PRIMARY KEY CHECK (length(validation_id) = 16),
    job_id BLOB NOT NULL CHECK (length(job_id) = 16),
    generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
    validation_ordinal INTEGER NOT NULL CHECK (validation_ordinal >= 1),
    outcome TEXT NOT NULL CHECK (outcome IN ('Ready', 'PlanChanged', 'AwaitingSource', 'NeedsAttention')),
    started_at INTEGER NOT NULL,
    completed_at INTEGER NOT NULL,
    UNIQUE (job_id, validation_ordinal),
    FOREIGN KEY (job_id) REFERENCES import_job(job_id) ON DELETE RESTRICT,
    FOREIGN KEY (generation_id) REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_space_check (
    space_check_id BLOB PRIMARY KEY CHECK (length(space_check_id) = 16),
    job_id BLOB NOT NULL CHECK (length(job_id) = 16),
    generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
    validation_id BLOB NOT NULL UNIQUE CHECK (length(validation_id) = 16),
    planned_new_bytes INTEGER NOT NULL CHECK (planned_new_bytes >= 0),
    target_total_bytes INTEGER NOT NULL CHECK (target_total_bytes >= 0),
    target_available_bytes INTEGER NOT NULL CHECK (target_available_bytes >= 0),
    reserve_bytes INTEGER NOT NULL CHECK (reserve_bytes >= 10737418240),
    checked_at INTEGER NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('Passed', 'Insufficient')),
    FOREIGN KEY (job_id) REFERENCES import_job(job_id) ON DELETE RESTRICT,
    FOREIGN KEY (generation_id) REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT,
    FOREIGN KEY (validation_id) REFERENCES import_plan_validation(validation_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_source_item (
    source_item_id BLOB PRIMARY KEY CHECK (length(source_item_id) = 16),
    job_id BLOB NOT NULL CHECK (length(job_id) = 16),
    source_root_id BLOB NOT NULL CHECK (length(source_root_id) = 16),
    source_path_key BLOB NOT NULL,
    source_file_id BLOB,
    source_snapshot_id BLOB NOT NULL CHECK (length(source_snapshot_id) = 16),
    original_filename TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('Jpg', 'Raf')),
    created_at INTEGER NOT NULL,
    UNIQUE (job_id, source_root_id, source_path_key),
    FOREIGN KEY (job_id) REFERENCES import_job(job_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_import_source_item_file
ON import_source_item(job_id, source_root_id, source_file_id)
WHERE source_file_id IS NOT NULL;

CREATE TABLE import_group (
    group_id BLOB PRIMARY KEY CHECK (length(group_id) = 16),
    generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
    group_key BLOB NOT NULL,
    pair_evidence TEXT CHECK (pair_evidence IS NULL OR pair_evidence IN ('HistoricalExact', 'SameImportGroup')),
    target_date_class TEXT NOT NULL CHECK (target_date_class IN ('Known', 'Unknown')),
    name_family_key BLOB NOT NULL,
    suffix_ordinal INTEGER NOT NULL CHECK (suffix_ordinal >= 1),
    UNIQUE (generation_id, group_key),
    FOREIGN KEY (generation_id) REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_asset_step (
    step_id BLOB PRIMARY KEY CHECK (length(step_id) = 16),
    generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
    source_item_id BLOB NOT NULL CHECK (length(source_item_id) = 16),
    group_id BLOB CHECK (group_id IS NULL OR length(group_id) = 16),
    role TEXT NOT NULL CHECK (role IN ('Jpg', 'Raf')),
    plan_kind TEXT NOT NULL CHECK (plan_kind IN ('Copy', 'SkipDuplicate', 'Unreadable')),
    final_root_id BLOB CHECK (final_root_id IS NULL OR length(final_root_id) = 16),
    final_binding_id BLOB CHECK (final_binding_id IS NULL OR length(final_binding_id) = 16),
    final_relative_path TEXT,
    final_path_key BLOB,
    partial_namespace BLOB,
    partial_seed BLOB,
    expected_bytes INTEGER CHECK (expected_bytes IS NULL OR expected_bytes >= 0),
    UNIQUE (generation_id, source_item_id),
    FOREIGN KEY (generation_id) REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_item_id) REFERENCES import_source_item(source_item_id) ON DELETE RESTRICT,
    FOREIGN KEY (group_id) REFERENCES import_group(group_id) ON DELETE RESTRICT,
    FOREIGN KEY (final_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (final_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    CHECK (
        (plan_kind = 'Unreadable' AND final_root_id IS NULL AND final_binding_id IS NULL AND final_path_key IS NULL AND expected_bytes IS NULL)
        OR
        (plan_kind = 'SkipDuplicate' AND final_root_id IS NOT NULL AND final_binding_id IS NOT NULL AND final_path_key IS NOT NULL AND expected_bytes IS NULL)
        OR
        (plan_kind = 'Copy' AND final_root_id IS NOT NULL AND final_binding_id IS NOT NULL AND final_path_key IS NOT NULL AND partial_namespace IS NOT NULL AND partial_seed IS NOT NULL AND expected_bytes IS NOT NULL)
    )
) STRICT;

CREATE UNIQUE INDEX ux_import_group_role
ON import_asset_step(group_id, role)
WHERE group_id IS NOT NULL;

CREATE UNIQUE INDEX ux_import_final_path
ON import_asset_step(generation_id, final_root_id, final_path_key)
WHERE final_path_key IS NOT NULL;

CREATE TABLE import_plan_validation_item (
    validation_item_id BLOB PRIMARY KEY CHECK (length(validation_item_id) = 16),
    validation_id BLOB NOT NULL CHECK (length(validation_id) = 16),
    step_id BLOB NOT NULL CHECK (length(step_id) = 16),
    disposition TEXT NOT NULL CHECK (disposition IN ('CopyNewBytes', 'SkipWithCurrentProof', 'ReverifyOwnedPartial', 'IndexPublished', 'AlreadyResolved', 'Unreadable')),
    remaining_new_bytes INTEGER NOT NULL CHECK (remaining_new_bytes >= 0),
    UNIQUE (validation_id, step_id),
    FOREIGN KEY (validation_id) REFERENCES import_plan_validation(validation_id) ON DELETE RESTRICT,
    FOREIGN KEY (step_id) REFERENCES import_asset_step(step_id) ON DELETE RESTRICT,
    CHECK ((disposition = 'CopyNewBytes') OR remaining_new_bytes = 0)
) STRICT;

CREATE TRIGGER import_validation_no_update
BEFORE UPDATE ON import_plan_validation BEGIN SELECT RAISE(ABORT, 'validation is immutable'); END;
CREATE TRIGGER import_validation_no_delete
BEFORE DELETE ON import_plan_validation BEGIN SELECT RAISE(ABORT, 'validation is immutable'); END;
CREATE TRIGGER import_validation_item_no_update
BEFORE UPDATE ON import_plan_validation_item BEGIN SELECT RAISE(ABORT, 'validation item is immutable'); END;
CREATE TRIGGER import_validation_item_no_delete
BEFORE DELETE ON import_plan_validation_item BEGIN SELECT RAISE(ABORT, 'validation item is immutable'); END;
CREATE TRIGGER import_space_check_no_update
BEFORE UPDATE ON import_space_check BEGIN SELECT RAISE(ABORT, 'space check is immutable'); END;
CREATE TRIGGER import_space_check_no_delete
BEFORE DELETE ON import_space_check BEGIN SELECT RAISE(ABORT, 'space check is immutable'); END;

CREATE TABLE import_attempt (
    attempt_id BLOB PRIMARY KEY CHECK (length(attempt_id) = 16),
    job_id BLOB NOT NULL CHECK (length(job_id) = 16),
    generation_id BLOB CHECK (generation_id IS NULL OR length(generation_id) = 16),
    validation_id BLOB UNIQUE CHECK (validation_id IS NULL OR length(validation_id) = 16),
    space_check_id BLOB UNIQUE CHECK (space_check_id IS NULL OR length(space_check_id) = 16),
    attempt_reason TEXT NOT NULL CHECK (attempt_reason IN ('Initial', 'Retry', 'Resume', 'Reconcile', 'CancelBeforeStart')),
    attempt_state TEXT NOT NULL CHECK (attempt_state IN ('Prepared', 'Running', 'Completed', 'CompletedWithErrors', 'AwaitingSource', 'NeedsAttention', 'Cancelled', 'Interrupted')),
    started_at INTEGER,
    finished_at INTEGER,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    FOREIGN KEY (attempt_id) REFERENCES task_attempt(attempt_id) ON DELETE RESTRICT,
    FOREIGN KEY (job_id) REFERENCES import_job(job_id) ON DELETE RESTRICT,
    FOREIGN KEY (generation_id) REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT,
    FOREIGN KEY (validation_id) REFERENCES import_plan_validation(validation_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_check_id) REFERENCES import_space_check(space_check_id) ON DELETE RESTRICT,
    CHECK (
        (attempt_reason = 'CancelBeforeStart' AND generation_id IS NULL AND validation_id IS NULL AND space_check_id IS NULL)
        OR
        (attempt_reason != 'CancelBeforeStart' AND generation_id IS NOT NULL AND validation_id IS NOT NULL AND space_check_id IS NOT NULL)
    ),
    CHECK ((attempt_state IN ('Prepared', 'Running')) = (finished_at IS NULL))
) STRICT;

CREATE UNIQUE INDEX ux_import_attempt_active
ON import_attempt(job_id)
WHERE attempt_state IN ('Prepared', 'Running');

CREATE TRIGGER import_attempt_transition
BEFORE UPDATE OF attempt_state ON import_attempt
WHEN NOT (
    (OLD.attempt_state = 'Prepared' AND NEW.attempt_state IN ('Running', 'Cancelled', 'Interrupted'))
    OR (OLD.attempt_state = 'Running' AND NEW.attempt_state IN ('Completed', 'CompletedWithErrors', 'AwaitingSource', 'NeedsAttention', 'Cancelled', 'Interrupted'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal import attempt transition');
END;

CREATE TRIGGER import_attempt_terminal_no_update
BEFORE UPDATE ON import_attempt
WHEN OLD.attempt_state NOT IN ('Prepared', 'Running')
BEGIN
    SELECT RAISE(ABORT, 'terminal import attempt is immutable');
END;

CREATE TRIGGER import_attempt_no_delete
BEFORE DELETE ON import_attempt BEGIN
    SELECT RAISE(ABORT, 'import attempt is immutable');
END;

CREATE TRIGGER import_attempt_kind_and_parent
BEFORE INSERT ON import_attempt
WHEN NOT EXISTS (
    SELECT 1
    FROM task_attempt
    WHERE attempt_id = NEW.attempt_id
      AND task_id = NEW.job_id
      AND attempt_kind = 'Import'
)
BEGIN
    SELECT RAISE(ABORT, 'import attempt must match its task attempt and job');
END;

CREATE TABLE import_attempt_step (
    attempt_step_id BLOB PRIMARY KEY CHECK (length(attempt_step_id) = 16),
    attempt_id BLOB NOT NULL CHECK (length(attempt_id) = 16),
    step_id BLOB NOT NULL CHECK (length(step_id) = 16),
    step_state TEXT NOT NULL CHECK (step_state IN ('Planned', 'Validating', 'SkippedDuplicate', 'Copying', 'ValidatingOwnedPartial', 'PartialVerified', 'Published', 'Indexed', 'CleaningOwnedPartial', 'CleanedPartial', 'ReconcilingPublished', 'AlreadyResolved', 'Unreadable', 'PublishedNeedsIndex', 'Cancelled', 'Failed', 'AwaitingSource', 'Conflict', 'PlanStale', 'NeedsAttention', 'Interrupted')),
    error_code TEXT,
    warning_code TEXT,
    started_at INTEGER,
    finished_at INTEGER,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    UNIQUE (attempt_id, step_id),
    FOREIGN KEY (attempt_id) REFERENCES import_attempt(attempt_id) ON DELETE RESTRICT,
    FOREIGN KEY (step_id) REFERENCES import_asset_step(step_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_duplicate_proof (
    duplicate_proof_id BLOB PRIMARY KEY CHECK (length(duplicate_proof_id) = 16),
    proof_scope TEXT NOT NULL CHECK (proof_scope IN ('Validation', 'Execution')),
    step_id BLOB NOT NULL CHECK (length(step_id) = 16),
    validation_id BLOB CHECK (validation_id IS NULL OR length(validation_id) = 16),
    execution_attempt_step_id BLOB CHECK (execution_attempt_step_id IS NULL OR length(execution_attempt_step_id) = 16),
    lease_generation INTEGER,
    source_read_reservation_id BLOB CHECK (source_read_reservation_id IS NULL OR length(source_read_reservation_id) = 16),
    target_existing_reservation_id BLOB CHECK (target_existing_reservation_id IS NULL OR length(target_existing_reservation_id) = 16),
    source_before_snapshot_id BLOB NOT NULL CHECK (length(source_before_snapshot_id) = 16),
    source_after_snapshot_id BLOB NOT NULL CHECK (length(source_after_snapshot_id) = 16),
    target_before_snapshot_id BLOB NOT NULL CHECK (length(target_before_snapshot_id) = 16),
    target_after_snapshot_id BLOB NOT NULL CHECK (length(target_after_snapshot_id) = 16),
    full_hash_equal INTEGER NOT NULL CHECK (full_hash_equal = 1),
    byte_equal INTEGER NOT NULL CHECK (byte_equal = 1),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (step_id) REFERENCES import_asset_step(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (validation_id) REFERENCES import_plan_validation(validation_id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_read_reservation_id) REFERENCES operation_reservation(reservation_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_existing_reservation_id) REFERENCES operation_reservation(reservation_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_before_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_after_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_before_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_after_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    CHECK (
        (proof_scope = 'Validation' AND validation_id IS NOT NULL AND execution_attempt_step_id IS NULL AND lease_generation IS NULL AND source_read_reservation_id IS NULL AND target_existing_reservation_id IS NULL)
        OR
        (proof_scope = 'Execution' AND validation_id IS NULL AND execution_attempt_step_id IS NOT NULL AND lease_generation IS NOT NULL AND source_read_reservation_id IS NOT NULL AND target_existing_reservation_id IS NOT NULL)
    )
) STRICT;

CREATE UNIQUE INDEX ux_import_duplicate_validation
ON import_duplicate_proof(validation_id, step_id)
WHERE proof_scope = 'Validation';

CREATE UNIQUE INDEX ux_import_duplicate_execution
ON import_duplicate_proof(execution_attempt_step_id)
WHERE proof_scope = 'Execution';

CREATE TABLE import_duplicate_path_binding_proof (
    path_binding_proof_id BLOB PRIMARY KEY CHECK (length(path_binding_proof_id) = 16),
    duplicate_proof_id BLOB NOT NULL UNIQUE CHECK (length(duplicate_proof_id) = 16),
    source_root_id BLOB NOT NULL CHECK (length(source_root_id) = 16),
    source_binding_id BLOB NOT NULL CHECK (length(source_binding_id) = 16),
    source_path_key BLOB NOT NULL,
    source_file_id BLOB NOT NULL,
    target_root_id BLOB NOT NULL CHECK (length(target_root_id) = 16),
    target_binding_id BLOB NOT NULL CHECK (length(target_binding_id) = 16),
    target_path_key BLOB NOT NULL,
    target_file_id BLOB NOT NULL,
    source_contained INTEGER NOT NULL CHECK (source_contained = 1),
    target_contained INTEGER NOT NULL CHECK (target_contained = 1),
    source_reparse_free INTEGER NOT NULL CHECK (source_reparse_free = 1),
    target_reparse_free INTEGER NOT NULL CHECK (target_reparse_free = 1),
    FOREIGN KEY (duplicate_proof_id) REFERENCES import_duplicate_proof(duplicate_proof_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_partial_ownership (
    ownership_id BLOB PRIMARY KEY CHECK (length(ownership_id) = 16),
    step_id BLOB NOT NULL CHECK (length(step_id) = 16),
    ownership_ordinal INTEGER NOT NULL CHECK (ownership_ordinal >= 1),
    created_by_attempt_step_id BLOB NOT NULL UNIQUE CHECK (length(created_by_attempt_step_id) = 16),
    nonce BLOB NOT NULL,
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    relative_path TEXT NOT NULL,
    path_key BLOB NOT NULL,
    file_id BLOB,
    created_size_observed INTEGER CHECK (created_size_observed IS NULL OR created_size_observed >= 0),
    state TEXT NOT NULL CHECK (state IN ('Planned', 'Owned', 'CleaningOwnedPartial', 'Cleaned', 'ConsumedByPublish', 'NeedsAttention')),
    UNIQUE (step_id, ownership_ordinal),
    UNIQUE (root_id, path_key),
    FOREIGN KEY (step_id) REFERENCES import_asset_step(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by_attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    CHECK (
        (state = 'Planned' AND file_id IS NULL AND created_size_observed IS NULL)
        OR (state IN ('Owned', 'CleaningOwnedPartial', 'ConsumedByPublish') AND file_id IS NOT NULL AND created_size_observed IS NOT NULL)
        OR state IN ('Cleaned', 'NeedsAttention')
    )
) STRICT;

CREATE UNIQUE INDEX ux_import_partial_unresolved
ON import_partial_ownership(step_id)
WHERE state IN ('Planned', 'Owned', 'CleaningOwnedPartial', 'NeedsAttention');

CREATE TRIGGER import_partial_ownership_identity_immutable
BEFORE UPDATE ON import_partial_ownership
WHEN OLD.step_id != NEW.step_id
  OR OLD.ownership_ordinal != NEW.ownership_ordinal
  OR OLD.created_by_attempt_step_id != NEW.created_by_attempt_step_id
  OR OLD.nonce != NEW.nonce
  OR OLD.root_id != NEW.root_id
  OR OLD.root_binding_id != NEW.root_binding_id
  OR OLD.relative_path != NEW.relative_path
  OR OLD.path_key != NEW.path_key
BEGIN
    SELECT RAISE(ABORT, 'partial ownership identity is immutable');
END;

CREATE TRIGGER import_partial_ownership_transition
BEFORE UPDATE OF state ON import_partial_ownership
WHEN NOT (
    (OLD.state = 'Planned' AND NEW.state IN ('Owned', 'NeedsAttention'))
    OR (OLD.state = 'Owned' AND NEW.state IN ('CleaningOwnedPartial', 'ConsumedByPublish', 'NeedsAttention'))
    OR (OLD.state = 'CleaningOwnedPartial' AND NEW.state IN ('Cleaned', 'NeedsAttention'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal partial ownership transition');
END;

CREATE TRIGGER import_partial_ownership_no_delete
BEFORE DELETE ON import_partial_ownership BEGIN
    SELECT RAISE(ABORT, 'partial ownership history is immutable');
END;

CREATE TABLE import_partial_claim (
    claim_id BLOB PRIMARY KEY CHECK (length(claim_id) = 16),
    ownership_id BLOB NOT NULL CHECK (length(ownership_id) = 16),
    attempt_step_id BLOB NOT NULL UNIQUE CHECK (length(attempt_step_id) = 16),
    claim_reason TEXT NOT NULL CHECK (claim_reason IN ('Created', 'Takeover')),
    lease_generation INTEGER NOT NULL CHECK (lease_generation >= 1),
    source_read_reservation_id BLOB NOT NULL UNIQUE CHECK (length(source_read_reservation_id) = 16),
    partial_path_reservation_id BLOB NOT NULL UNIQUE CHECK (length(partial_path_reservation_id) = 16),
    final_path_reservation_id BLOB NOT NULL UNIQUE CHECK (length(final_path_reservation_id) = 16),
    claimed_at INTEGER NOT NULL,
    released_at INTEGER,
    release_reason TEXT,
    FOREIGN KEY (ownership_id) REFERENCES import_partial_ownership(ownership_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_read_reservation_id) REFERENCES operation_reservation(reservation_id) ON DELETE RESTRICT,
    FOREIGN KEY (partial_path_reservation_id) REFERENCES operation_reservation(reservation_id) ON DELETE RESTRICT,
    FOREIGN KEY (final_path_reservation_id) REFERENCES operation_reservation(reservation_id) ON DELETE RESTRICT,
    CHECK ((released_at IS NULL) = (release_reason IS NULL))
) STRICT;

CREATE UNIQUE INDEX ux_import_partial_claim_active
ON import_partial_claim(ownership_id)
WHERE released_at IS NULL;

CREATE TRIGGER import_partial_claim_identity_immutable
BEFORE UPDATE ON import_partial_claim
WHEN OLD.ownership_id != NEW.ownership_id
  OR OLD.attempt_step_id != NEW.attempt_step_id
  OR OLD.claim_reason != NEW.claim_reason
  OR OLD.lease_generation != NEW.lease_generation
  OR OLD.source_read_reservation_id != NEW.source_read_reservation_id
  OR OLD.partial_path_reservation_id != NEW.partial_path_reservation_id
  OR OLD.final_path_reservation_id != NEW.final_path_reservation_id
BEGIN
    SELECT RAISE(ABORT, 'partial claim identity is immutable');
END;

CREATE TRIGGER import_partial_claim_release_once
BEFORE UPDATE ON import_partial_claim
WHEN OLD.released_at IS NOT NULL
  OR NEW.released_at IS NULL
  OR NEW.release_reason IS NULL
BEGIN
    SELECT RAISE(ABORT, 'partial claim only supports one soft release');
END;

CREATE TRIGGER import_partial_claim_no_delete
BEFORE DELETE ON import_partial_claim BEGIN
    SELECT RAISE(ABORT, 'partial claim history is immutable');
END;

CREATE TABLE verified_partial_snapshot (
    verified_partial_snapshot_id BLOB PRIMARY KEY CHECK (length(verified_partial_snapshot_id) = 16),
    ownership_id BLOB NOT NULL CHECK (length(ownership_id) = 16),
    claim_id BLOB NOT NULL UNIQUE CHECK (length(claim_id) = 16),
    attempt_step_id BLOB NOT NULL UNIQUE CHECK (length(attempt_step_id) = 16),
    final_partial_snapshot_id BLOB NOT NULL CHECK (length(final_partial_snapshot_id) = 16),
    source_before_snapshot_id BLOB NOT NULL CHECK (length(source_before_snapshot_id) = 16),
    source_after_snapshot_id BLOB NOT NULL CHECK (length(source_after_snapshot_id) = 16),
    content_hash BLOB NOT NULL,
    byte_equal INTEGER NOT NULL CHECK (byte_equal = 1),
    time_preserved INTEGER NOT NULL CHECK (time_preserved IN (0, 1)),
    verified_at INTEGER NOT NULL,
    FOREIGN KEY (ownership_id) REFERENCES import_partial_ownership(ownership_id) ON DELETE RESTRICT,
    FOREIGN KEY (claim_id) REFERENCES import_partial_claim(claim_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (final_partial_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_before_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_after_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_publish_intent (
    publish_intent_id BLOB PRIMARY KEY CHECK (length(publish_intent_id) = 16),
    attempt_step_id BLOB NOT NULL UNIQUE CHECK (length(attempt_step_id) = 16),
    partial_claim_id BLOB NOT NULL UNIQUE CHECK (length(partial_claim_id) = 16),
    verified_partial_snapshot_id BLOB NOT NULL UNIQUE CHECK (length(verified_partial_snapshot_id) = 16),
    final_path_reservation_id BLOB NOT NULL UNIQUE CHECK (length(final_path_reservation_id) = 16),
    lease_generation INTEGER NOT NULL CHECK (lease_generation >= 1),
    intent_state TEXT NOT NULL CHECK (intent_state IN ('Prepared', 'Publishing', 'Published', 'NeedsAttention')),
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    FOREIGN KEY (attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (partial_claim_id) REFERENCES import_partial_claim(claim_id) ON DELETE RESTRICT,
    FOREIGN KEY (verified_partial_snapshot_id) REFERENCES verified_partial_snapshot(verified_partial_snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (final_path_reservation_id) REFERENCES operation_reservation(reservation_id) ON DELETE RESTRICT,
    CHECK ((intent_state IN ('Prepared', 'Publishing')) = (finished_at IS NULL))
) STRICT;

CREATE TRIGGER import_publish_transition
BEFORE UPDATE OF intent_state ON import_publish_intent
WHEN NOT (
    (OLD.intent_state = 'Prepared' AND NEW.intent_state = 'Publishing')
    OR (OLD.intent_state = 'Publishing' AND NEW.intent_state IN ('Published', 'NeedsAttention'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal publish intent transition');
END;

CREATE TRIGGER import_publish_terminal_no_update
BEFORE UPDATE ON import_publish_intent
WHEN OLD.intent_state IN ('Published', 'NeedsAttention')
BEGIN
    SELECT RAISE(ABORT, 'terminal publish intent is immutable');
END;

CREATE TRIGGER import_publish_no_delete
BEFORE DELETE ON import_publish_intent BEGIN
    SELECT RAISE(ABORT, 'publish intent history is immutable');
END;

CREATE TABLE import_partial_cleanup_fact (
    cleanup_fact_id BLOB PRIMARY KEY CHECK (length(cleanup_fact_id) = 16),
    ownership_id BLOB NOT NULL UNIQUE CHECK (length(ownership_id) = 16),
    claim_id BLOB NOT NULL UNIQUE CHECK (length(claim_id) = 16),
    attempt_step_id BLOB NOT NULL UNIQUE CHECK (length(attempt_step_id) = 16),
    deleted_snapshot_id BLOB NOT NULL CHECK (length(deleted_snapshot_id) = 16),
    outcome TEXT NOT NULL CHECK (outcome IN ('DeleteSucceeded', 'ObservedMissingAfterCleaningIntent')),
    cleaned_at INTEGER NOT NULL,
    FOREIGN KEY (ownership_id) REFERENCES import_partial_ownership(ownership_id) ON DELETE RESTRICT,
    FOREIGN KEY (claim_id) REFERENCES import_partial_claim(claim_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (deleted_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER import_duplicate_proof_no_update
BEFORE UPDATE ON import_duplicate_proof BEGIN SELECT RAISE(ABORT, 'duplicate proof is immutable'); END;
CREATE TRIGGER import_duplicate_proof_no_delete
BEFORE DELETE ON import_duplicate_proof BEGIN SELECT RAISE(ABORT, 'duplicate proof is immutable'); END;
CREATE TRIGGER duplicate_path_proof_no_update
BEFORE UPDATE ON import_duplicate_path_binding_proof BEGIN SELECT RAISE(ABORT, 'path proof is immutable'); END;
CREATE TRIGGER duplicate_path_proof_no_delete
BEFORE DELETE ON import_duplicate_path_binding_proof BEGIN SELECT RAISE(ABORT, 'path proof is immutable'); END;
CREATE TRIGGER verified_partial_no_update
BEFORE UPDATE ON verified_partial_snapshot BEGIN SELECT RAISE(ABORT, 'verified partial snapshot is immutable'); END;
CREATE TRIGGER verified_partial_no_delete
BEFORE DELETE ON verified_partial_snapshot BEGIN SELECT RAISE(ABORT, 'verified partial snapshot is immutable'); END;
CREATE TRIGGER partial_cleanup_fact_no_update
BEFORE UPDATE ON import_partial_cleanup_fact BEGIN SELECT RAISE(ABORT, 'cleanup fact is immutable'); END;
CREATE TRIGGER partial_cleanup_fact_no_delete
BEFORE DELETE ON import_partial_cleanup_fact BEGIN SELECT RAISE(ABORT, 'cleanup fact is immutable'); END;

CREATE TRIGGER duplicate_path_proof_chain_consistent
BEFORE INSERT ON import_duplicate_path_binding_proof
WHEN NOT EXISTS (
    SELECT 1
    FROM import_duplicate_proof AS proof
    JOIN file_identity_snapshot AS source_before
      ON source_before.snapshot_id=proof.source_before_snapshot_id
    JOIN file_identity_snapshot AS source_after
      ON source_after.snapshot_id=proof.source_after_snapshot_id
    JOIN file_identity_snapshot AS target_before
      ON target_before.snapshot_id=proof.target_before_snapshot_id
    JOIN file_identity_snapshot AS target_after
      ON target_after.snapshot_id=proof.target_after_snapshot_id
    WHERE proof.duplicate_proof_id=NEW.duplicate_proof_id
      AND source_before.root_id=NEW.source_root_id
      AND source_after.root_id=NEW.source_root_id
      AND source_before.root_binding_id=NEW.source_binding_id
      AND source_after.root_binding_id=NEW.source_binding_id
      AND source_before.path_key=NEW.source_path_key
      AND source_after.path_key=NEW.source_path_key
      AND source_before.file_id=NEW.source_file_id
      AND source_after.file_id=NEW.source_file_id
      AND target_before.root_id=NEW.target_root_id
      AND target_after.root_id=NEW.target_root_id
      AND target_before.root_binding_id=NEW.target_binding_id
      AND target_after.root_binding_id=NEW.target_binding_id
      AND target_before.path_key=NEW.target_path_key
      AND target_after.path_key=NEW.target_path_key
      AND target_before.file_id=NEW.target_file_id
      AND target_after.file_id=NEW.target_file_id
)
BEGIN
    SELECT RAISE(ABORT, 'duplicate path proof does not match its snapshots');
END;

CREATE TRIGGER import_partial_claim_chain_consistent
BEFORE INSERT ON import_partial_claim
WHEN NOT EXISTS (
    SELECT 1
    FROM import_partial_ownership AS ownership
    JOIN import_attempt_step AS step
      ON step.attempt_step_id=NEW.attempt_step_id
     AND step.step_id=ownership.step_id
    JOIN operation_reservation AS source_reservation
      ON source_reservation.reservation_id=NEW.source_read_reservation_id
    JOIN operation_reservation AS partial_reservation
      ON partial_reservation.reservation_id=NEW.partial_path_reservation_id
    JOIN operation_reservation AS final_reservation
      ON final_reservation.reservation_id=NEW.final_path_reservation_id
    WHERE ownership.ownership_id=NEW.ownership_id
      AND source_reservation.attempt_id=step.attempt_id
      AND partial_reservation.attempt_id=step.attempt_id
      AND final_reservation.attempt_id=step.attempt_id
      AND source_reservation.lease_generation=NEW.lease_generation
      AND partial_reservation.lease_generation=NEW.lease_generation
      AND final_reservation.lease_generation=NEW.lease_generation
      AND source_reservation.reservation_kind='SourceRead'
      AND partial_reservation.reservation_kind='PartialPath'
      AND final_reservation.reservation_kind='FinalPath'
      AND source_reservation.released_at IS NULL
      AND partial_reservation.released_at IS NULL
      AND final_reservation.released_at IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'partial claim chain or lease generation is inconsistent');
END;

CREATE TRIGGER verified_partial_chain_consistent
BEFORE INSERT ON verified_partial_snapshot
WHEN NOT EXISTS (
    SELECT 1
    FROM import_partial_claim AS claim
    JOIN import_partial_ownership AS ownership
      ON ownership.ownership_id=claim.ownership_id
    JOIN file_identity_snapshot AS final_snapshot
      ON final_snapshot.snapshot_id=NEW.final_partial_snapshot_id
    WHERE claim.claim_id=NEW.claim_id
      AND claim.released_at IS NULL
      AND claim.ownership_id=NEW.ownership_id
      AND claim.attempt_step_id=NEW.attempt_step_id
      AND final_snapshot.root_id=ownership.root_id
      AND final_snapshot.root_binding_id=ownership.root_binding_id
      AND final_snapshot.path_key=ownership.path_key
      AND final_snapshot.file_id=ownership.file_id
      AND final_snapshot.content_hash=NEW.content_hash
)
BEGIN
    SELECT RAISE(ABORT, 'verified partial snapshot chain is inconsistent');
END;

CREATE TRIGGER import_publish_chain_consistent
BEFORE INSERT ON import_publish_intent
WHEN NOT EXISTS (
    SELECT 1
    FROM import_partial_claim AS claim
    JOIN verified_partial_snapshot AS verified
      ON verified.verified_partial_snapshot_id=NEW.verified_partial_snapshot_id
    JOIN operation_reservation AS reservation
      ON reservation.reservation_id=NEW.final_path_reservation_id
    WHERE claim.claim_id=NEW.partial_claim_id
      AND claim.released_at IS NULL
      AND claim.attempt_step_id=NEW.attempt_step_id
      AND claim.final_path_reservation_id=NEW.final_path_reservation_id
      AND claim.lease_generation=NEW.lease_generation
      AND verified.claim_id=claim.claim_id
      AND verified.attempt_step_id=NEW.attempt_step_id
      AND reservation.released_at IS NULL
      AND reservation.reservation_kind='FinalPath'
      AND reservation.lease_generation=NEW.lease_generation
)
BEGIN
    SELECT RAISE(ABORT, 'publish intent chain or lease generation is inconsistent');
END;

CREATE TABLE deletion_mark (
    mark_id BLOB PRIMARY KEY CHECK (length(mark_id) = 16),
    photo_id BLOB NOT NULL CHECK (length(photo_id) = 16),
    photo_version INTEGER NOT NULL CHECK (photo_version >= 1),
    created_operation_id BLOB NOT NULL CHECK (length(created_operation_id) = 16),
    created_at INTEGER NOT NULL,
    released_at INTEGER,
    released_operation_id BLOB CHECK (released_operation_id IS NULL OR length(released_operation_id) = 16),
    release_reason TEXT,
    FOREIGN KEY (photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT,
    FOREIGN KEY (released_operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT,
    CHECK (
        (released_at IS NULL AND released_operation_id IS NULL AND release_reason IS NULL)
        OR (released_at IS NOT NULL AND released_operation_id IS NOT NULL AND release_reason IS NOT NULL)
    )
) STRICT;

CREATE UNIQUE INDEX ux_deletion_mark_active
ON deletion_mark(photo_id)
WHERE released_at IS NULL;

CREATE TRIGGER deletion_mark_release_once
BEFORE UPDATE ON deletion_mark
WHEN OLD.released_at IS NOT NULL
  OR NEW.released_at IS NULL
  OR NEW.released_operation_id IS NULL
  OR NEW.release_reason IS NULL
  OR NEW.mark_id IS NOT OLD.mark_id
  OR NEW.photo_id IS NOT OLD.photo_id
  OR NEW.photo_version IS NOT OLD.photo_version
  OR NEW.created_operation_id IS NOT OLD.created_operation_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'deletion mark only supports one soft release');
END;

CREATE TRIGGER deletion_mark_no_delete
BEFORE DELETE ON deletion_mark BEGIN
    SELECT RAISE(ABORT, 'deletion mark history is immutable');
END;

CREATE TABLE deletion_mark_asset_snapshot (
    mark_asset_snapshot_id BLOB PRIMARY KEY CHECK (length(mark_asset_snapshot_id) = 16),
    mark_id BLOB NOT NULL CHECK (length(mark_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    role TEXT NOT NULL CHECK (role IN ('Jpg', 'Raf')),
    file_identity_snapshot_id BLOB NOT NULL CHECK (length(file_identity_snapshot_id) = 16),
    UNIQUE (mark_id, asset_id),
    UNIQUE (mark_id, role),
    FOREIGN KEY (mark_id) REFERENCES deletion_mark(mark_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (file_identity_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER deletion_mark_snapshot_no_update
BEFORE UPDATE ON deletion_mark_asset_snapshot BEGIN
    SELECT RAISE(ABORT, 'deletion mark snapshot is immutable');
END;

CREATE TRIGGER deletion_mark_snapshot_no_delete
BEFORE DELETE ON deletion_mark_asset_snapshot BEGIN
    SELECT RAISE(ABORT, 'deletion mark snapshot is immutable');
END;

CREATE TABLE deletion_batch (
    batch_id BLOB PRIMARY KEY CHECK (length(batch_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    batch_state TEXT NOT NULL CHECK (batch_state IN ('Prepared', 'Running', 'CancelRequested', 'Completed', 'CompletedWithErrors', 'NeedsAttention', 'Cancelled')),
    total_photos INTEGER NOT NULL CHECK (total_photos >= 0),
    total_files INTEGER NOT NULL CHECK (total_files >= 0),
    total_bytes INTEGER NOT NULL CHECK (total_bytes >= 0),
    confirmed_operation_id BLOB NOT NULL UNIQUE CHECK (length(confirmed_operation_id) = 16),
    confirmed_at INTEGER NOT NULL,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    FOREIGN KEY (batch_id) REFERENCES task_intent(task_id) ON DELETE RESTRICT,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    FOREIGN KEY (confirmed_operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE deletion_batch_photo (
    batch_photo_id BLOB PRIMARY KEY CHECK (length(batch_photo_id) = 16),
    batch_id BLOB NOT NULL CHECK (length(batch_id) = 16),
    photo_id BLOB NOT NULL CHECK (length(photo_id) = 16),
    photo_version INTEGER NOT NULL CHECK (photo_version >= 1),
    mark_id BLOB NOT NULL UNIQUE CHECK (length(mark_id) = 16),
    expected_members INTEGER NOT NULL CHECK (expected_members IN (1, 2)),
    expected_bytes INTEGER NOT NULL CHECK (expected_bytes >= 0),
    UNIQUE (batch_id, photo_id),
    FOREIGN KEY (batch_id) REFERENCES deletion_batch(batch_id) ON DELETE RESTRICT,
    FOREIGN KEY (photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (mark_id) REFERENCES deletion_mark(mark_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE deletion_batch_asset (
    batch_asset_id BLOB PRIMARY KEY CHECK (length(batch_asset_id) = 16),
    batch_photo_id BLOB NOT NULL CHECK (length(batch_photo_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    role TEXT NOT NULL CHECK (role IN ('Jpg', 'Raf')),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    mark_asset_snapshot_id BLOB NOT NULL UNIQUE CHECK (length(mark_asset_snapshot_id) = 16),
    resolution_state TEXT NOT NULL CHECK (resolution_state IN ('Pending', 'RetryableFailed', 'NeedsAttention', 'Recycled', 'CancelledRemaining', 'ResolvedWithoutDelete')),
    UNIQUE (batch_photo_id, asset_id),
    UNIQUE (batch_photo_id, role),
    FOREIGN KEY (batch_photo_id) REFERENCES deletion_batch_photo(batch_photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (mark_asset_snapshot_id) REFERENCES deletion_mark_asset_snapshot(mark_asset_snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER deletion_batch_asset_snapshot_copy
BEFORE INSERT ON deletion_batch_asset
WHEN NOT EXISTS (
    SELECT 1
    FROM deletion_batch_photo AS batch_photo
    JOIN deletion_mark_asset_snapshot AS snapshot
      ON snapshot.mark_id=batch_photo.mark_id
    WHERE batch_photo.batch_photo_id=NEW.batch_photo_id
      AND snapshot.mark_asset_snapshot_id=NEW.mark_asset_snapshot_id
      AND snapshot.asset_id=NEW.asset_id
      AND snapshot.asset_version=NEW.asset_version
      AND snapshot.role=NEW.role
)
BEGIN
    SELECT RAISE(ABORT, 'batch asset must copy its immutable mark snapshot');
END;

CREATE TABLE deletion_attempt (
    attempt_id BLOB PRIMARY KEY CHECK (length(attempt_id) = 16),
    batch_id BLOB NOT NULL CHECK (length(batch_id) = 16),
    attempt_reason TEXT NOT NULL CHECK (attempt_reason IN ('Initial', 'Retry', 'Reconcile')),
    attempt_state TEXT NOT NULL CHECK (attempt_state IN ('Prepared', 'Running', 'Completed', 'CompletedWithErrors', 'NeedsAttention', 'Cancelled', 'Interrupted')),
    started_at INTEGER,
    finished_at INTEGER,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    FOREIGN KEY (attempt_id) REFERENCES task_attempt(attempt_id) ON DELETE RESTRICT,
    FOREIGN KEY (batch_id) REFERENCES deletion_batch(batch_id) ON DELETE RESTRICT,
    CHECK ((attempt_state IN ('Prepared', 'Running')) = (finished_at IS NULL))
) STRICT;

CREATE UNIQUE INDEX ux_deletion_attempt_active
ON deletion_attempt(batch_id)
WHERE attempt_state IN ('Prepared', 'Running');

CREATE TRIGGER deletion_attempt_transition
BEFORE UPDATE OF attempt_state ON deletion_attempt
WHEN NOT (
    (OLD.attempt_state = 'Prepared' AND NEW.attempt_state IN ('Running', 'Cancelled', 'Interrupted'))
    OR (OLD.attempt_state = 'Running' AND NEW.attempt_state IN ('Completed', 'CompletedWithErrors', 'NeedsAttention', 'Cancelled', 'Interrupted'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal deletion attempt transition');
END;

CREATE TRIGGER deletion_attempt_terminal_no_update
BEFORE UPDATE ON deletion_attempt
WHEN OLD.attempt_state NOT IN ('Prepared', 'Running')
BEGIN
    SELECT RAISE(ABORT, 'terminal deletion attempt is immutable');
END;

CREATE TRIGGER deletion_attempt_no_delete
BEFORE DELETE ON deletion_attempt BEGIN
    SELECT RAISE(ABORT, 'deletion attempt is immutable');
END;

CREATE TRIGGER deletion_attempt_kind_and_parent
BEFORE INSERT ON deletion_attempt
WHEN NOT EXISTS (
    SELECT 1
    FROM task_attempt
    WHERE attempt_id = NEW.attempt_id
      AND task_id = NEW.batch_id
      AND attempt_kind = 'Recycle'
)
BEGIN
    SELECT RAISE(ABORT, 'deletion attempt must match its task attempt and batch');
END;

CREATE TABLE deletion_attempt_asset (
    attempt_asset_id BLOB PRIMARY KEY CHECK (length(attempt_asset_id) = 16),
    attempt_id BLOB NOT NULL CHECK (length(attempt_id) = 16),
    batch_asset_id BLOB NOT NULL CHECK (length(batch_asset_id) = 16),
    asset_state TEXT NOT NULL CHECK (asset_state IN ('Prepared', 'Validating', 'Recycling', 'Recycled', 'RetryableFailed', 'NeedsAttention', 'Cancelled', 'Interrupted')),
    validation_snapshot_id BLOB CHECK (validation_snapshot_id IS NULL OR length(validation_snapshot_id) = 16),
    shell_result INTEGER,
    recycle_object_present INTEGER CHECK (recycle_object_present IS NULL OR recycle_object_present IN (0, 1)),
    started_at INTEGER,
    finished_at INTEGER,
    UNIQUE (attempt_id, batch_asset_id),
    FOREIGN KEY (attempt_id) REFERENCES deletion_attempt(attempt_id) ON DELETE RESTRICT,
    FOREIGN KEY (batch_asset_id) REFERENCES deletion_batch_asset(batch_asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (validation_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE operation_report (
    report_id BLOB PRIMARY KEY CHECK (length(report_id) = 16),
    report_kind TEXT NOT NULL CHECK (report_kind IN ('Scan', 'Import', 'Recycle')),
    library_id BLOB CHECK (library_id IS NULL OR length(library_id) = 16),
    root_id BLOB CHECK (root_id IS NULL OR length(root_id) = 16),
    scan_run_id BLOB UNIQUE,
    task_attempt_id BLOB UNIQUE CHECK (task_attempt_id IS NULL OR length(task_attempt_id) = 16),
    report_state TEXT NOT NULL CHECK (report_state IN ('Open', 'Finalized')),
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    result TEXT CHECK (result IS NULL OR result IN ('Success', 'Partial', 'Failed', 'NeedsAttention', 'Cancelled')),
    item_count INTEGER NOT NULL DEFAULT 0 CHECK (item_count >= 0),
    total_bytes INTEGER NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
    supersedes_report_id BLOB CHECK (supersedes_report_id IS NULL OR length(supersedes_report_id) = 16),
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_attempt_id) REFERENCES task_attempt(attempt_id) ON DELETE RESTRICT,
    FOREIGN KEY (supersedes_report_id) REFERENCES operation_report(report_id) ON DELETE RESTRICT,
    CHECK (
        (report_kind = 'Scan' AND scan_run_id IS NOT NULL AND task_attempt_id IS NULL)
        OR (report_kind IN ('Import', 'Recycle') AND scan_run_id IS NULL AND task_attempt_id IS NOT NULL)
    ),
    CHECK (
        (report_state = 'Open' AND completed_at IS NULL AND result IS NULL)
        OR (report_state = 'Finalized' AND completed_at IS NOT NULL AND result IS NOT NULL)
    )
) STRICT;

CREATE TRIGGER operation_report_finalized_no_update
BEFORE UPDATE ON operation_report
WHEN OLD.report_state = 'Finalized'
BEGIN
    SELECT RAISE(ABORT, 'finalized report is immutable');
END;

CREATE TRIGGER operation_report_no_delete
BEFORE DELETE ON operation_report BEGIN
    SELECT RAISE(ABORT, 'operation report is immutable');
END;

CREATE TRIGGER operation_report_task_kind
BEFORE INSERT ON operation_report
WHEN NEW.task_attempt_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM task_attempt
    WHERE attempt_id = NEW.task_attempt_id
      AND (
          (attempt_kind = 'Import' AND NEW.report_kind = 'Import')
          OR (attempt_kind = 'Recycle' AND NEW.report_kind = 'Recycle')
      )
)
BEGIN
    SELECT RAISE(ABORT, 'operation report kind must match task attempt kind');
END;

CREATE TABLE operation_report_item (
    report_item_id BLOB PRIMARY KEY CHECK (length(report_item_id) = 16),
    report_id BLOB NOT NULL CHECK (length(report_id) = 16),
    item_kind TEXT NOT NULL,
    stable_item_id BLOB NOT NULL,
    result_code TEXT NOT NULL,
    warning_code TEXT,
    display_path_snapshot TEXT,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    user_parameters TEXT,
    UNIQUE (report_id, item_kind, stable_item_id),
    FOREIGN KEY (report_id) REFERENCES operation_report(report_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER operation_report_item_finalized_no_update
BEFORE UPDATE ON operation_report_item
WHEN EXISTS (
    SELECT 1 FROM operation_report
    WHERE report_id = OLD.report_id AND report_state = 'Finalized'
)
BEGIN
    SELECT RAISE(ABORT, 'finalized report item is immutable');
END;

CREATE TRIGGER operation_report_item_finalized_no_insert
BEFORE INSERT ON operation_report_item
WHEN EXISTS (
    SELECT 1 FROM operation_report
    WHERE report_id = NEW.report_id AND report_state = 'Finalized'
)
BEGIN
    SELECT RAISE(ABORT, 'cannot append to a finalized report');
END;

CREATE TRIGGER operation_report_item_no_delete
BEFORE DELETE ON operation_report_item BEGIN
    SELECT RAISE(ABORT, 'operation report item is immutable');
END;

CREATE TABLE operation_resolution_event (
    resolution_event_id BLOB PRIMARY KEY CHECK (length(resolution_event_id) = 16),
    operation_id BLOB NOT NULL UNIQUE CHECK (length(operation_id) = 16),
    report_id BLOB NOT NULL CHECK (length(report_id) = 16),
    task_id BLOB NOT NULL CHECK (length(task_id) = 16),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('RescanConfirmed', 'RetryStarted', 'TaskCancelled', 'CancelledRemaining', 'ResolvedWithoutDelete', 'RestoredFromRecycleBin')),
    evidence_snapshot BLOB,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT,
    FOREIGN KEY (report_id) REFERENCES operation_report(report_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id) REFERENCES task_intent(task_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER operation_resolution_event_chain_consistent
BEFORE INSERT ON operation_resolution_event
WHEN NOT EXISTS (
    SELECT 1
    FROM operation_report AS report
    JOIN task_attempt AS attempt
      ON attempt.attempt_id=report.task_attempt_id
    WHERE report.report_id=NEW.report_id
      AND report.report_state='Finalized'
      AND attempt.task_id=NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolution event must follow a finalized report for the same task');
END;

CREATE TRIGGER operation_resolution_event_no_update
BEFORE UPDATE ON operation_resolution_event BEGIN
    SELECT RAISE(ABORT, 'resolution event is immutable');
END;

CREATE TRIGGER operation_resolution_event_no_delete
BEFORE DELETE ON operation_resolution_event BEGIN
    SELECT RAISE(ABORT, 'resolution event is immutable');
END;

CREATE TABLE import_published_fact (
    published_fact_id BLOB PRIMARY KEY CHECK (length(published_fact_id) = 16),
    source_item_id BLOB NOT NULL UNIQUE,
    step_id BLOB NOT NULL UNIQUE,
    attempt_step_id BLOB NOT NULL UNIQUE,
    verified_partial_snapshot_id BLOB NOT NULL UNIQUE,
    publish_intent_id BLOB NOT NULL UNIQUE,
    target_snapshot_id BLOB NOT NULL UNIQUE CHECK (length(target_snapshot_id) = 16),
    published_at INTEGER NOT NULL,
    FOREIGN KEY (source_item_id) REFERENCES import_source_item(source_item_id) ON DELETE RESTRICT,
    FOREIGN KEY (step_id) REFERENCES import_asset_step(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_step_id) REFERENCES import_attempt_step(attempt_step_id) ON DELETE RESTRICT,
    FOREIGN KEY (verified_partial_snapshot_id) REFERENCES verified_partial_snapshot(verified_partial_snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (publish_intent_id) REFERENCES import_publish_intent(publish_intent_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE import_indexed_fact (
    indexed_fact_id BLOB PRIMARY KEY CHECK (length(indexed_fact_id) = 16),
    published_fact_id BLOB NOT NULL UNIQUE CHECK (length(published_fact_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    indexed_at INTEGER NOT NULL,
    FOREIGN KEY (published_fact_id) REFERENCES import_published_fact(published_fact_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE deletion_recycled_fact (
    recycled_fact_id BLOB PRIMARY KEY CHECK (length(recycled_fact_id) = 16),
    batch_asset_id BLOB NOT NULL UNIQUE CHECK (length(batch_asset_id) = 16),
    attempt_asset_id BLOB NOT NULL UNIQUE,
    evidence_snapshot BLOB NOT NULL,
    recycled_at INTEGER NOT NULL,
    FOREIGN KEY (batch_asset_id) REFERENCES deletion_batch_asset(batch_asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_asset_id) REFERENCES deletion_attempt_asset(attempt_asset_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE directory_identity_snapshot (
    directory_snapshot_id BLOB PRIMARY KEY CHECK (length(directory_snapshot_id) = 16),
    volume_id BLOB NOT NULL CHECK (length(volume_id) = 16),
    absolute_path_display TEXT NOT NULL,
    path_key BLOB NOT NULL,
    directory_file_id BLOB,
    filesystem_kind TEXT NOT NULL CHECK (filesystem_kind IN ('NTFS', 'exFAT', 'ReFS', 'Other')),
    identity_strength TEXT NOT NULL CHECK (identity_strength IN ('Strong', 'UserConfirmedLimited')),
    contained INTEGER NOT NULL CHECK (contained = 1),
    reparse_free INTEGER NOT NULL CHECK (reparse_free = 1),
    captured_at INTEGER NOT NULL,
    FOREIGN KEY (volume_id) REFERENCES volume_identity(volume_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE cache_root_create_intent (
    create_intent_id BLOB PRIMARY KEY CHECK (length(create_intent_id) = 16),
    operation_id BLOB NOT NULL UNIQUE CHECK (length(operation_id) = 16),
    parent_directory_snapshot_id BLOB NOT NULL CHECK (length(parent_directory_snapshot_id) = 16),
    planned_cache_root_id BLOB NOT NULL UNIQUE CHECK (length(planned_cache_root_id) = 16),
    planned_child_name_key BLOB NOT NULL,
    planned_child_path_key BLOB NOT NULL UNIQUE,
    ownership_nonce BLOB NOT NULL,
    intent_state TEXT NOT NULL CHECK (intent_state IN ('Prepared', 'Bound', 'NeedsAttention', 'Cancelled')),
    observed_child_snapshot_id BLOB,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    FOREIGN KEY (operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT,
    FOREIGN KEY (parent_directory_snapshot_id) REFERENCES directory_identity_snapshot(directory_snapshot_id) ON DELETE RESTRICT,
    CHECK ((intent_state = 'Prepared') = (finished_at IS NULL))
) STRICT;

CREATE UNIQUE INDEX ux_cache_root_create_prepared
ON cache_root_create_intent((1))
WHERE intent_state = 'Prepared';

CREATE TRIGGER cache_root_create_transition
BEFORE UPDATE OF intent_state ON cache_root_create_intent
WHEN NOT (OLD.intent_state = 'Prepared' AND NEW.intent_state IN ('Bound', 'NeedsAttention', 'Cancelled'))
BEGIN
    SELECT RAISE(ABORT, 'illegal cache root create transition');
END;

CREATE TRIGGER cache_root_create_terminal_no_update
BEFORE UPDATE ON cache_root_create_intent
WHEN OLD.intent_state IN ('Bound', 'NeedsAttention', 'Cancelled')
BEGIN
    SELECT RAISE(ABORT, 'terminal cache root create intent is immutable');
END;

CREATE TABLE cache_root_ownership (
    cache_root_id BLOB PRIMARY KEY CHECK (length(cache_root_id) = 16),
    parent_directory_snapshot_id BLOB NOT NULL UNIQUE CHECK (length(parent_directory_snapshot_id) = 16),
    child_root_binding_id BLOB NOT NULL UNIQUE CHECK (length(child_root_binding_id) = 16),
    ownership_nonce BLOB NOT NULL,
    directory_file_id BLOB NOT NULL,
    ownership_state TEXT NOT NULL CHECK (ownership_state IN ('Active', 'Draining', 'Offline', 'IdentityMismatch', 'Retired')),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (cache_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (parent_directory_snapshot_id) REFERENCES directory_identity_snapshot(directory_snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (child_root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_cache_root_ownership_active
ON cache_root_ownership((1))
WHERE ownership_state = 'Active';

CREATE TABLE cache_entry (
    cache_entry_id BLOB PRIMARY KEY CHECK (length(cache_entry_id) = 16),
    cache_root_id BLOB NOT NULL CHECK (length(cache_root_id) = 16),
    entry_ordinal INTEGER NOT NULL CHECK (entry_ordinal >= 1),
    ownership_nonce BLOB NOT NULL,
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    cache_kind TEXT NOT NULL CHECK (cache_kind IN ('Thumbnail', 'Preview')),
    render_key BLOB NOT NULL,
    relative_path TEXT NOT NULL,
    path_key BLOB NOT NULL,
    file_id BLOB,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    entry_state TEXT NOT NULL CHECK (entry_state IN ('Live', 'Missing', 'Evicted', 'NeedsAttention')),
    created_at INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL,
    UNIQUE (cache_root_id, path_key),
    UNIQUE (cache_root_id, asset_id, asset_version, cache_kind, render_key, entry_ordinal),
    FOREIGN KEY (cache_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_cache_entry_current_render
ON cache_entry(cache_root_id, asset_id, asset_version, cache_kind, render_key)
WHERE entry_state IN ('Live', 'NeedsAttention');

CREATE UNIQUE INDEX ux_cache_entry_current_file
ON cache_entry(cache_root_id, file_id)
WHERE entry_state IN ('Live', 'NeedsAttention') AND file_id IS NOT NULL;

CREATE TABLE cache_eviction_intent (
    eviction_intent_id BLOB PRIMARY KEY CHECK (length(eviction_intent_id) = 16),
    operation_id BLOB NOT NULL UNIQUE CHECK (length(operation_id) = 16),
    cache_entry_id BLOB NOT NULL UNIQUE CHECK (length(cache_entry_id) = 16),
    cache_root_id BLOB NOT NULL CHECK (length(cache_root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    deletion_snapshot_id BLOB NOT NULL CHECK (length(deletion_snapshot_id) = 16),
    intent_state TEXT NOT NULL CHECK (intent_state IN ('Prepared', 'Deleting', 'AwaitingRoot', 'Evicted', 'AlreadyMissing', 'NeedsAttention')),
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    FOREIGN KEY (operation_id) REFERENCES operation_event(operation_id) ON DELETE RESTRICT,
    FOREIGN KEY (cache_entry_id) REFERENCES cache_entry(cache_entry_id) ON DELETE RESTRICT,
    FOREIGN KEY (cache_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    FOREIGN KEY (deletion_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    CHECK ((intent_state IN ('Prepared', 'Deleting', 'AwaitingRoot')) = (finished_at IS NULL))
) STRICT;

CREATE TRIGGER cache_eviction_transition
BEFORE UPDATE OF intent_state ON cache_eviction_intent
WHEN NOT (
    (OLD.intent_state = 'Prepared' AND NEW.intent_state IN ('Deleting', 'AlreadyMissing', 'AwaitingRoot', 'NeedsAttention'))
    OR (OLD.intent_state = 'Deleting' AND NEW.intent_state IN ('Evicted', 'AlreadyMissing', 'AwaitingRoot', 'NeedsAttention'))
    OR (OLD.intent_state = 'AwaitingRoot' AND NEW.intent_state IN ('Deleting', 'AlreadyMissing', 'NeedsAttention'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal cache eviction transition');
END;

CREATE TRIGGER cache_eviction_terminal_no_update
BEFORE UPDATE ON cache_eviction_intent
WHEN OLD.intent_state IN ('Evicted', 'AlreadyMissing', 'NeedsAttention')
BEGIN
    SELECT RAISE(ABORT, 'terminal cache eviction intent is immutable');
END;

CREATE TRIGGER cache_eviction_no_delete
BEFORE DELETE ON cache_eviction_intent BEGIN
    SELECT RAISE(ABORT, 'cache eviction intent is immutable');
END;

CREATE TABLE cache_eviction_fact (
    eviction_fact_id BLOB PRIMARY KEY CHECK (length(eviction_fact_id) = 16),
    eviction_intent_id BLOB NOT NULL UNIQUE CHECK (length(eviction_intent_id) = 16),
    cache_entry_id BLOB NOT NULL UNIQUE CHECK (length(cache_entry_id) = 16),
    outcome TEXT NOT NULL CHECK (outcome IN ('DeleteSucceeded', 'ObservedMissingAfterIntent')),
    observed_at INTEGER NOT NULL,
    FOREIGN KEY (eviction_intent_id) REFERENCES cache_eviction_intent(eviction_intent_id) ON DELETE RESTRICT,
    FOREIGN KEY (cache_entry_id) REFERENCES cache_entry(cache_entry_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER import_published_fact_immutable_update
BEFORE UPDATE ON import_published_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER import_published_fact_immutable_delete
BEFORE DELETE ON import_published_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER import_indexed_fact_immutable_update
BEFORE UPDATE ON import_indexed_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER import_indexed_fact_immutable_delete
BEFORE DELETE ON import_indexed_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER deletion_recycled_fact_immutable_update
BEFORE UPDATE ON deletion_recycled_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER deletion_recycled_fact_immutable_delete
BEFORE DELETE ON deletion_recycled_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER cache_eviction_fact_immutable_update
BEFORE UPDATE ON cache_eviction_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
CREATE TRIGGER cache_eviction_fact_immutable_delete
BEFORE DELETE ON cache_eviction_fact BEGIN SELECT RAISE(ABORT, 'fact is immutable'); END;
