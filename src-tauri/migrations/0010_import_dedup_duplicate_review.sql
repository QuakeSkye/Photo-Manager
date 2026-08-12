PRAGMA foreign_keys = ON;

CREATE TABLE library_catalog_revision (
    library_id BLOB PRIMARY KEY CHECK (length(library_id) = 16),
    catalog_revision INTEGER NOT NULL DEFAULT 0 CHECK (catalog_revision >= 0),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE asset_import_candidate_key (
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    key_format_version INTEGER NOT NULL CHECK (key_format_version >= 1),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    basename_folded TEXT NOT NULL,
    extension_folded TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    capture_local_key TEXT,
    relative_path TEXT NOT NULL,
    path_key BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (asset_id, asset_version, key_format_version),
    FOREIGN KEY (asset_id, asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX ix_asset_import_candidate_lookup
ON asset_import_candidate_key(
    library_id, root_id, basename_folded, extension_folded, size_bytes,
    capture_local_key, key_format_version, asset_id, asset_version
);

CREATE INDEX ix_asset_import_candidate_current_path
ON asset_import_candidate_key(root_id, root_binding_id, path_key, asset_id, asset_version);

CREATE TRIGGER asset_import_candidate_key_no_update
BEFORE UPDATE ON asset_import_candidate_key BEGIN
    SELECT RAISE(ABORT, 'asset import candidate keys are immutable');
END;

CREATE TRIGGER asset_import_candidate_key_no_delete
BEFORE DELETE ON asset_import_candidate_key BEGIN
    SELECT RAISE(ABORT, 'asset import candidate keys are immutable');
END;

CREATE TRIGGER asset_import_candidate_key_chain
BEFORE INSERT ON asset_import_candidate_key
WHEN NOT EXISTS (
    SELECT 1
    FROM asset AS asset
    JOIN asset_revision AS revision
      ON revision.asset_id=asset.asset_id
     AND revision.asset_version=NEW.asset_version
    WHERE asset.asset_id=NEW.asset_id
      AND asset.library_id=NEW.library_id
      AND asset.root_id=NEW.root_id
      AND revision.root_id=NEW.root_id
      AND revision.root_binding_id=NEW.root_binding_id
      AND revision.relative_path=NEW.relative_path
      AND revision.path_key=NEW.path_key
      AND revision.size_bytes=NEW.size_bytes
)
BEGIN
    SELECT RAISE(ABORT, 'candidate key must describe its asset revision');
END;

CREATE TABLE import_asset_resolution_fact (
    resolution_fact_id BLOB PRIMARY KEY CHECK (length(resolution_fact_id) = 16),
    step_id BLOB NOT NULL UNIQUE CHECK (length(step_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    resolution_kind TEXT NOT NULL
        CHECK (resolution_kind IN ('PublishedIndexed', 'SkippedDuplicate')),
    indexed_fact_id BLOB UNIQUE CHECK (indexed_fact_id IS NULL OR length(indexed_fact_id) = 16),
    duplicate_proof_id BLOB UNIQUE CHECK (duplicate_proof_id IS NULL OR length(duplicate_proof_id) = 16),
    evidence_snapshot_id BLOB NOT NULL CHECK (length(evidence_snapshot_id) = 16),
    resolved_at INTEGER NOT NULL,
    FOREIGN KEY (step_id) REFERENCES import_asset_step(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id, asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (indexed_fact_id) REFERENCES import_indexed_fact(indexed_fact_id) ON DELETE RESTRICT,
    FOREIGN KEY (duplicate_proof_id) REFERENCES import_duplicate_proof(duplicate_proof_id) ON DELETE RESTRICT,
    FOREIGN KEY (evidence_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    CHECK (
        (resolution_kind='PublishedIndexed' AND indexed_fact_id IS NOT NULL AND duplicate_proof_id IS NULL)
        OR
        (resolution_kind='SkippedDuplicate' AND indexed_fact_id IS NULL AND duplicate_proof_id IS NOT NULL)
    )
) STRICT;

CREATE TRIGGER import_asset_resolution_fact_chain
BEFORE INSERT ON import_asset_resolution_fact
WHEN NOT (
    (NEW.resolution_kind='PublishedIndexed' AND EXISTS (
        SELECT 1
        FROM import_indexed_fact AS indexed
        JOIN import_published_fact AS published
          ON published.published_fact_id=indexed.published_fact_id
        JOIN file_identity_snapshot AS snapshot
          ON snapshot.snapshot_id=NEW.evidence_snapshot_id
        JOIN asset_revision AS revision
          ON revision.asset_id=NEW.asset_id
         AND revision.asset_version=NEW.asset_version
        WHERE indexed.indexed_fact_id=NEW.indexed_fact_id
          AND indexed.asset_id=NEW.asset_id
          AND published.step_id=NEW.step_id
          AND published.target_snapshot_id=NEW.evidence_snapshot_id
          AND snapshot.root_id=revision.root_id
          AND snapshot.root_binding_id=revision.root_binding_id
          AND snapshot.path_key=revision.path_key
          AND snapshot.file_id IS revision.file_id
          AND snapshot.size_bytes=revision.size_bytes
          AND snapshot.mtime_filetime=revision.mtime_filetime
    ))
    OR
    (NEW.resolution_kind='SkippedDuplicate' AND EXISTS (
        SELECT 1
        FROM import_duplicate_proof AS proof
        JOIN import_duplicate_path_binding_proof AS binding
          ON binding.duplicate_proof_id=proof.duplicate_proof_id
        JOIN file_identity_snapshot AS snapshot
          ON snapshot.snapshot_id=NEW.evidence_snapshot_id
        JOIN asset_revision AS revision
          ON revision.asset_id=NEW.asset_id
         AND revision.asset_version=NEW.asset_version
        WHERE proof.duplicate_proof_id=NEW.duplicate_proof_id
          AND proof.proof_scope='Execution'
          AND proof.step_id=NEW.step_id
          AND proof.target_asset_id=NEW.asset_id
          AND proof.target_after_snapshot_id=NEW.evidence_snapshot_id
          AND binding.target_root_id=revision.root_id
          AND binding.target_binding_id=revision.root_binding_id
          AND binding.target_path_key=revision.path_key
          AND binding.target_file_id=revision.file_id
          AND snapshot.root_id=revision.root_id
          AND snapshot.root_binding_id=revision.root_binding_id
          AND snapshot.path_key=revision.path_key
          AND snapshot.file_id IS revision.file_id
          AND snapshot.size_bytes=revision.size_bytes
          AND snapshot.mtime_filetime=revision.mtime_filetime
    ))
)
BEGIN
    SELECT RAISE(ABORT, 'import asset resolution evidence chain is inconsistent');
END;

CREATE TRIGGER import_asset_resolution_fact_no_update
BEFORE UPDATE ON import_asset_resolution_fact BEGIN
    SELECT RAISE(ABORT, 'import asset resolution facts are immutable');
END;

CREATE TRIGGER import_asset_resolution_fact_no_delete
BEFORE DELETE ON import_asset_resolution_fact BEGIN
    SELECT RAISE(ABORT, 'import asset resolution facts are immutable');
END;

CREATE TABLE duplicate_scan_run (
    run_id BLOB PRIMARY KEY CHECK (length(run_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    catalog_revision INTEGER NOT NULL CHECK (catalog_revision >= 0),
    run_state TEXT NOT NULL CHECK (run_state IN ('Running','Completed','Cancelled','Failed')),
    phase TEXT NOT NULL CHECK (phase IN ('Enumerating','Hashing','Comparing','Publishing','Finished')),
    checked_files INTEGER NOT NULL DEFAULT 0 CHECK (checked_files >= 0),
    candidate_files INTEGER NOT NULL DEFAULT 0 CHECK (candidate_files >= 0),
    hashed_files INTEGER NOT NULL DEFAULT 0 CHECK (hashed_files >= 0),
    current_relative_path TEXT,
    error_code TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    CHECK ((run_state='Running') = (completed_at IS NULL))
) STRICT;

CREATE TRIGGER duplicate_scan_run_payload_immutable
BEFORE UPDATE ON duplicate_scan_run
WHEN OLD.run_id IS NOT NEW.run_id
  OR OLD.library_id IS NOT NEW.library_id
  OR OLD.root_binding_id IS NOT NEW.root_binding_id
  OR OLD.catalog_revision IS NOT NEW.catalog_revision
  OR OLD.started_at IS NOT NEW.started_at
BEGIN
    SELECT RAISE(ABORT, 'duplicate scan run identity is immutable');
END;

CREATE TRIGGER duplicate_scan_run_terminal_immutable
BEFORE UPDATE ON duplicate_scan_run
WHEN OLD.run_state!='Running'
BEGIN
    SELECT RAISE(ABORT, 'terminal duplicate scan runs are immutable');
END;

CREATE TRIGGER duplicate_scan_run_no_delete
BEFORE DELETE ON duplicate_scan_run BEGIN
    SELECT RAISE(ABORT, 'duplicate scan runs are append-only');
END;

CREATE TABLE duplicate_file_observation (
    observation_id BLOB PRIMARY KEY CHECK (length(observation_id) = 16),
    run_id BLOB NOT NULL CHECK (length(run_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    role TEXT NOT NULL CHECK (role IN ('Jpg','Raf')),
    before_snapshot_id BLOB NOT NULL CHECK (length(before_snapshot_id) = 16),
    after_snapshot_id BLOB NOT NULL CHECK (length(after_snapshot_id) = 16),
    full_hash BLOB NOT NULL CHECK (length(full_hash) = 32),
    observed_at INTEGER NOT NULL,
    UNIQUE (run_id, asset_id),
    FOREIGN KEY (run_id) REFERENCES duplicate_scan_run(run_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id, asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (before_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (after_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER duplicate_file_observation_chain
BEFORE INSERT ON duplicate_file_observation
WHEN NOT EXISTS (
    SELECT 1
    FROM duplicate_scan_run AS run
    JOIN asset AS asset ON asset.asset_id=NEW.asset_id
    JOIN asset_revision AS revision
      ON revision.asset_id=NEW.asset_id AND revision.asset_version=NEW.asset_version
    JOIN file_identity_snapshot AS before_snapshot
      ON before_snapshot.snapshot_id=NEW.before_snapshot_id
    JOIN file_identity_snapshot AS after_snapshot
      ON after_snapshot.snapshot_id=NEW.after_snapshot_id
    WHERE run.run_id=NEW.run_id
      AND run.run_state='Running'
      AND asset.library_id=run.library_id
      AND asset.media_kind=NEW.role
      AND before_snapshot.root_id=revision.root_id
      AND before_snapshot.root_binding_id=revision.root_binding_id
      AND before_snapshot.path_key=revision.path_key
      AND before_snapshot.file_id IS revision.file_id
      AND before_snapshot.size_bytes=revision.size_bytes
      AND before_snapshot.mtime_filetime=revision.mtime_filetime
      AND after_snapshot.root_id=before_snapshot.root_id
      AND after_snapshot.root_binding_id=before_snapshot.root_binding_id
      AND after_snapshot.path_key=before_snapshot.path_key
      AND after_snapshot.file_id IS before_snapshot.file_id
      AND after_snapshot.size_bytes=before_snapshot.size_bytes
      AND after_snapshot.mtime_filetime=before_snapshot.mtime_filetime
      AND before_snapshot.content_hash_algorithm='SHA-256'
      AND before_snapshot.content_hash=NEW.full_hash
      AND after_snapshot.content_hash_algorithm='SHA-256'
      AND after_snapshot.content_hash=NEW.full_hash
)
BEGIN
    SELECT RAISE(ABORT, 'duplicate observation requires stable asset snapshots');
END;

CREATE TRIGGER duplicate_file_observation_no_update
BEFORE UPDATE ON duplicate_file_observation BEGIN
    SELECT RAISE(ABORT, 'duplicate file observations are immutable');
END;
CREATE TRIGGER duplicate_file_observation_no_delete
BEFORE DELETE ON duplicate_file_observation BEGIN
    SELECT RAISE(ABORT, 'duplicate file observations are immutable');
END;

CREATE TABLE duplicate_equivalence_set (
    set_id BLOB PRIMARY KEY CHECK (length(set_id) = 16),
    run_id BLOB NOT NULL CHECK (length(run_id) = 16),
    role TEXT NOT NULL CHECK (role IN ('Jpg','Raf')),
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    member_count INTEGER NOT NULL CHECK (member_count >= 2),
    file_size_bytes INTEGER NOT NULL CHECK (file_size_bytes >= 0),
    total_bytes INTEGER NOT NULL CHECK (total_bytes = file_size_bytes * member_count),
    created_at INTEGER NOT NULL,
    UNIQUE (run_id, role, content_hash, file_size_bytes),
    FOREIGN KEY (run_id) REFERENCES duplicate_scan_run(run_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER duplicate_equivalence_set_no_update
BEFORE UPDATE ON duplicate_equivalence_set BEGIN
    SELECT RAISE(ABORT, 'duplicate equivalence sets are immutable');
END;
CREATE TRIGGER duplicate_equivalence_set_no_delete
BEFORE DELETE ON duplicate_equivalence_set BEGIN
    SELECT RAISE(ABORT, 'duplicate equivalence sets are immutable');
END;

CREATE TABLE duplicate_equivalence_proof (
    proof_id BLOB PRIMARY KEY CHECK (length(proof_id) = 16),
    run_id BLOB NOT NULL CHECK (length(run_id) = 16),
    set_id BLOB NOT NULL CHECK (length(set_id) = 16),
    anchor_observation_id BLOB NOT NULL CHECK (length(anchor_observation_id) = 16),
    member_observation_id BLOB NOT NULL CHECK (length(member_observation_id) = 16),
    anchor_snapshot_id BLOB NOT NULL CHECK (length(anchor_snapshot_id) = 16),
    member_snapshot_id BLOB NOT NULL CHECK (length(member_snapshot_id) = 16),
    byte_equal INTEGER NOT NULL CHECK (byte_equal = 1),
    created_at INTEGER NOT NULL,
    UNIQUE (set_id, anchor_observation_id, member_observation_id),
    FOREIGN KEY (run_id) REFERENCES duplicate_scan_run(run_id) ON DELETE RESTRICT,
    FOREIGN KEY (set_id) REFERENCES duplicate_equivalence_set(set_id) ON DELETE RESTRICT,
    FOREIGN KEY (anchor_observation_id) REFERENCES duplicate_file_observation(observation_id) ON DELETE RESTRICT,
    FOREIGN KEY (member_observation_id) REFERENCES duplicate_file_observation(observation_id) ON DELETE RESTRICT,
    FOREIGN KEY (anchor_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (member_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    CHECK (anchor_observation_id != member_observation_id)
) STRICT;

CREATE TRIGGER duplicate_equivalence_proof_chain
BEFORE INSERT ON duplicate_equivalence_proof
WHEN NOT EXISTS (
    SELECT 1
    FROM duplicate_equivalence_set AS equivalent_set
    JOIN duplicate_file_observation AS anchor
      ON anchor.observation_id=NEW.anchor_observation_id
    JOIN duplicate_file_observation AS member
      ON member.observation_id=NEW.member_observation_id
    WHERE equivalent_set.set_id=NEW.set_id
      AND equivalent_set.run_id=NEW.run_id
      AND anchor.run_id=NEW.run_id
      AND member.run_id=NEW.run_id
      AND anchor.role=equivalent_set.role
      AND member.role=equivalent_set.role
      AND anchor.full_hash=equivalent_set.content_hash
      AND member.full_hash=equivalent_set.content_hash
      AND anchor.after_snapshot_id=NEW.anchor_snapshot_id
      AND member.after_snapshot_id=NEW.member_snapshot_id
      AND anchor.asset_id!=member.asset_id
)
BEGIN
    SELECT RAISE(ABORT, 'duplicate equivalence proof chain is inconsistent');
END;

CREATE TRIGGER duplicate_equivalence_proof_no_update
BEFORE UPDATE ON duplicate_equivalence_proof BEGIN
    SELECT RAISE(ABORT, 'duplicate equivalence proofs are immutable');
END;
CREATE TRIGGER duplicate_equivalence_proof_no_delete
BEFORE DELETE ON duplicate_equivalence_proof BEGIN
    SELECT RAISE(ABORT, 'duplicate equivalence proofs are immutable');
END;

ALTER TABLE deletion_mark ADD COLUMN mark_scope TEXT NOT NULL DEFAULT 'WholePhoto'
    CHECK (mark_scope IN ('WholePhoto','DuplicateSubset'));

CREATE TRIGGER deletion_mark_scope_immutable
BEFORE UPDATE OF mark_scope ON deletion_mark
BEGIN
    SELECT RAISE(ABORT, 'deletion mark scope is immutable');
END;

CREATE TABLE duplicate_retention_guard (
    guard_id BLOB PRIMARY KEY CHECK (length(guard_id) = 16),
    mark_asset_snapshot_id BLOB NOT NULL UNIQUE CHECK (length(mark_asset_snapshot_id) = 16),
    retained_asset_id BLOB NOT NULL CHECK (length(retained_asset_id) = 16),
    retained_asset_version INTEGER NOT NULL CHECK (retained_asset_version >= 1),
    retained_snapshot_id BLOB NOT NULL CHECK (length(retained_snapshot_id) = 16),
    equivalence_proof_id BLOB NOT NULL CHECK (length(equivalence_proof_id) = 16),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (mark_asset_snapshot_id)
        REFERENCES deletion_mark_asset_snapshot(mark_asset_snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (retained_asset_id, retained_asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (retained_snapshot_id)
        REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (equivalence_proof_id)
        REFERENCES duplicate_equivalence_proof(proof_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER duplicate_retention_guard_chain
BEFORE INSERT ON duplicate_retention_guard
WHEN NOT EXISTS (
    SELECT 1
    FROM deletion_mark_asset_snapshot AS marked
    JOIN deletion_mark AS mark ON mark.mark_id=marked.mark_id
    JOIN duplicate_equivalence_proof AS proof
      ON proof.proof_id=NEW.equivalence_proof_id
    JOIN duplicate_file_observation AS anchor
      ON anchor.observation_id=proof.anchor_observation_id
    JOIN duplicate_file_observation AS member
      ON member.observation_id=proof.member_observation_id
    JOIN file_identity_snapshot AS retained_snapshot
      ON retained_snapshot.snapshot_id=NEW.retained_snapshot_id
    WHERE marked.mark_asset_snapshot_id=NEW.mark_asset_snapshot_id
      AND mark.mark_scope='DuplicateSubset'
      AND marked.asset_id!=NEW.retained_asset_id
      AND retained_snapshot.root_id=(SELECT root_id FROM asset_revision WHERE asset_id=NEW.retained_asset_id AND asset_version=NEW.retained_asset_version)
      AND retained_snapshot.root_binding_id=(SELECT root_binding_id FROM asset_revision WHERE asset_id=NEW.retained_asset_id AND asset_version=NEW.retained_asset_version)
      AND retained_snapshot.path_key=(SELECT path_key FROM asset_revision WHERE asset_id=NEW.retained_asset_id AND asset_version=NEW.retained_asset_version)
      AND (
          (anchor.asset_id=marked.asset_id AND member.asset_id=NEW.retained_asset_id)
          OR (member.asset_id=marked.asset_id AND anchor.asset_id=NEW.retained_asset_id)
      )
)
BEGIN
    SELECT RAISE(ABORT, 'duplicate retention guard chain is inconsistent');
END;

CREATE TRIGGER duplicate_retention_guard_no_update
BEFORE UPDATE ON duplicate_retention_guard BEGIN
    SELECT RAISE(ABORT, 'duplicate retention guards are immutable');
END;
CREATE TRIGGER duplicate_retention_guard_no_delete
BEFORE DELETE ON duplicate_retention_guard BEGIN
    SELECT RAISE(ABORT, 'duplicate retention guards are immutable');
END;

CREATE TRIGGER deletion_mark_whole_photo_has_no_retention_guard
BEFORE INSERT ON duplicate_retention_guard
WHEN EXISTS (
    SELECT 1
    FROM deletion_mark_asset_snapshot AS snapshot
    JOIN deletion_mark AS mark ON mark.mark_id=snapshot.mark_id
    WHERE snapshot.mark_asset_snapshot_id=NEW.mark_asset_snapshot_id
      AND mark.mark_scope='WholePhoto'
)
BEGIN
    SELECT RAISE(ABORT, 'whole photo marks cannot carry retention guards');
END;

CREATE TABLE deletion_batch_retention_guard (
    batch_guard_id BLOB PRIMARY KEY CHECK (length(batch_guard_id) = 16),
    batch_asset_id BLOB NOT NULL UNIQUE CHECK (length(batch_asset_id) = 16),
    source_guard_id BLOB NOT NULL UNIQUE CHECK (length(source_guard_id) = 16),
    retained_asset_id BLOB NOT NULL CHECK (length(retained_asset_id) = 16),
    retained_asset_version INTEGER NOT NULL CHECK (retained_asset_version >= 1),
    retained_snapshot_id BLOB NOT NULL CHECK (length(retained_snapshot_id) = 16),
    equivalence_proof_id BLOB NOT NULL CHECK (length(equivalence_proof_id) = 16),
    FOREIGN KEY (batch_asset_id) REFERENCES deletion_batch_asset(batch_asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_guard_id) REFERENCES duplicate_retention_guard(guard_id) ON DELETE RESTRICT,
    FOREIGN KEY (retained_asset_id, retained_asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (retained_snapshot_id) REFERENCES file_identity_snapshot(snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (equivalence_proof_id) REFERENCES duplicate_equivalence_proof(proof_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER deletion_batch_retention_guard_chain
BEFORE INSERT ON deletion_batch_retention_guard
WHEN NOT EXISTS (
    SELECT 1
    FROM deletion_batch_asset AS batch_asset
    JOIN duplicate_retention_guard AS source_guard
      ON source_guard.mark_asset_snapshot_id=batch_asset.mark_asset_snapshot_id
    WHERE batch_asset.batch_asset_id=NEW.batch_asset_id
      AND source_guard.guard_id=NEW.source_guard_id
      AND source_guard.retained_asset_id=NEW.retained_asset_id
      AND source_guard.retained_asset_version=NEW.retained_asset_version
      AND source_guard.retained_snapshot_id=NEW.retained_snapshot_id
      AND source_guard.equivalence_proof_id=NEW.equivalence_proof_id
)
BEGIN
    SELECT RAISE(ABORT, 'batch retention guard must copy the immutable mark guard');
END;

CREATE TRIGGER deletion_batch_retention_guard_no_update
BEFORE UPDATE ON deletion_batch_retention_guard BEGIN
    SELECT RAISE(ABORT, 'batch retention guards are immutable');
END;
CREATE TRIGGER deletion_batch_retention_guard_no_delete
BEFORE DELETE ON deletion_batch_retention_guard BEGIN
    SELECT RAISE(ABORT, 'batch retention guards are immutable');
END;

CREATE TRIGGER deletion_batch_duplicate_guards_complete
BEFORE UPDATE OF batch_state ON deletion_batch
WHEN NEW.batch_state='Running' AND EXISTS (
    SELECT 1
    FROM deletion_batch_photo AS batch_photo
    JOIN deletion_mark AS mark ON mark.mark_id=batch_photo.mark_id
    JOIN deletion_batch_asset AS batch_asset ON batch_asset.batch_photo_id=batch_photo.batch_photo_id
    LEFT JOIN deletion_batch_retention_guard AS guard
      ON guard.batch_asset_id=batch_asset.batch_asset_id
    WHERE batch_photo.batch_id=NEW.batch_id
      AND mark.mark_scope='DuplicateSubset'
      AND guard.batch_guard_id IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'duplicate subset batch requires a retention guard for every asset');
END;

PRAGMA user_version = 10;
