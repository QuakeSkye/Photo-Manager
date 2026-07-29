PRAGMA user_version = 2;

CREATE TABLE scan_run (
    scan_run_id BLOB PRIMARY KEY CHECK (length(scan_run_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    root_id BLOB NOT NULL CHECK (length(root_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    run_state TEXT NOT NULL CHECK (run_state IN ('Running', 'Completed', 'Failed', 'Interrupted')),
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    observed_assets INTEGER NOT NULL DEFAULT 0 CHECK (observed_assets >= 0),
    ignored_files INTEGER NOT NULL DEFAULT 0 CHECK (ignored_files >= 0),
    skipped_reparse INTEGER NOT NULL DEFAULT 0 CHECK (skipped_reparse >= 0),
    error_count INTEGER NOT NULL DEFAULT 0 CHECK (error_count >= 0),
    CHECK ((run_state = 'Running') = (completed_at IS NULL)),
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX ux_scan_run_active_root
ON scan_run(root_id)
WHERE run_state = 'Running';

CREATE TRIGGER scan_run_terminal_no_update
BEFORE UPDATE ON scan_run
WHEN OLD.run_state != 'Running'
BEGIN
    SELECT RAISE(ABORT, 'terminal scan run is immutable');
END;

CREATE TRIGGER scan_run_no_delete
BEFORE DELETE ON scan_run BEGIN
    SELECT RAISE(ABORT, 'scan run history is immutable');
END;

CREATE TABLE asset_media_metadata (
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    capture_local_key TEXT,
    camera_model TEXT,
    camera_serial TEXT,
    preview_state TEXT NOT NULL CHECK (preview_state IN ('JpegSource', 'RafEmbeddedJpeg', 'Unavailable')),
    preview_offset INTEGER CHECK (preview_offset IS NULL OR preview_offset >= 0),
    preview_length INTEGER CHECK (preview_length IS NULL OR preview_length > 0),
    parser_version INTEGER NOT NULL CHECK (parser_version >= 1),
    observed_at INTEGER NOT NULL,
    PRIMARY KEY (asset_id, asset_version),
    FOREIGN KEY (asset_id, asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    CHECK (
        (preview_state = 'Unavailable' AND preview_offset IS NULL AND preview_length IS NULL)
        OR (preview_state = 'JpegSource' AND preview_offset IS NULL AND preview_length IS NULL)
        OR (preview_state = 'RafEmbeddedJpeg' AND preview_offset IS NOT NULL AND preview_length IS NOT NULL)
    )
) STRICT;

CREATE TRIGGER asset_media_metadata_no_update
BEFORE UPDATE ON asset_media_metadata BEGIN
    SELECT RAISE(ABORT, 'asset media metadata is immutable');
END;

CREATE TRIGGER asset_media_metadata_no_delete
BEFORE DELETE ON asset_media_metadata BEGIN
    SELECT RAISE(ABORT, 'asset media metadata is immutable');
END;

CREATE TABLE pair_evidence (
    pair_evidence_id BLOB PRIMARY KEY CHECK (length(pair_evidence_id) = 16),
    evidence_kind TEXT NOT NULL CHECK (evidence_kind IN ('HistoricalExact', 'SameImportGroup')),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    jpg_asset_id BLOB NOT NULL CHECK (length(jpg_asset_id) = 16),
    raf_asset_id BLOB NOT NULL CHECK (length(raf_asset_id) = 16),
    jpg_asset_version INTEGER NOT NULL CHECK (jpg_asset_version >= 1),
    raf_asset_version INTEGER NOT NULL CHECK (raf_asset_version >= 1),
    capture_local_key TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (jpg_asset_id, jpg_asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (raf_asset_id, raf_asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    UNIQUE (evidence_kind, jpg_asset_id, jpg_asset_version, raf_asset_id, raf_asset_version),
    CHECK (jpg_asset_id != raf_asset_id)
) STRICT;

CREATE TRIGGER pair_evidence_no_update
BEFORE UPDATE ON pair_evidence BEGIN
    SELECT RAISE(ABORT, 'pair evidence is immutable');
END;

CREATE TRIGGER pair_evidence_no_delete
BEFORE DELETE ON pair_evidence BEGIN
    SELECT RAISE(ABORT, 'pair evidence is immutable');
END;

CREATE TRIGGER photo_asset_pair_evidence_exists_insert
BEFORE INSERT ON photo_asset
WHEN NEW.pair_evidence_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1 FROM pair_evidence WHERE pair_evidence_id = NEW.pair_evidence_id
 )
BEGIN
    SELECT RAISE(ABORT, 'photo asset pair evidence is missing');
END;

CREATE TRIGGER photo_asset_pair_evidence_exists_update
BEFORE UPDATE OF pair_evidence_id ON photo_asset
WHEN NEW.pair_evidence_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1 FROM pair_evidence WHERE pair_evidence_id = NEW.pair_evidence_id
 )
BEGIN
    SELECT RAISE(ABORT, 'photo asset pair evidence is missing');
END;

CREATE TABLE photo_relation_event (
    relation_event_id BLOB PRIMARY KEY CHECK (length(relation_event_id) = 16),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('Merge', 'Split', 'MemberAdded', 'MemberRemoved')),
    photo_id BLOB NOT NULL CHECK (length(photo_id) = 16),
    related_photo_id BLOB CHECK (related_photo_id IS NULL OR length(related_photo_id) = 16),
    asset_id BLOB CHECK (asset_id IS NULL OR length(asset_id) = 16),
    before_version INTEGER NOT NULL CHECK (before_version >= 1),
    after_version INTEGER NOT NULL CHECK (after_version > before_version),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (related_photo_id) REFERENCES photo(photo_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER photo_relation_event_no_update
BEFORE UPDATE ON photo_relation_event BEGIN
    SELECT RAISE(ABORT, 'photo relation event is immutable');
END;

CREATE TRIGGER photo_relation_event_no_delete
BEFORE DELETE ON photo_relation_event BEGIN
    SELECT RAISE(ABORT, 'photo relation event is immutable');
END;

CREATE TABLE cache_policy (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    active_cache_root_id BLOB CHECK (active_cache_root_id IS NULL OR length(active_cache_root_id) = 16),
    capacity_bytes INTEGER NOT NULL CHECK (capacity_bytes > 0 AND capacity_bytes <= 68719476736),
    confirmed_at INTEGER NOT NULL,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    FOREIGN KEY (active_cache_root_id) REFERENCES root(root_id) ON DELETE RESTRICT
) STRICT;

INSERT INTO cache_policy(slot, active_cache_root_id, capacity_bytes, confirmed_at, row_version)
VALUES (1, NULL, 34359738368, 0, 1);

ALTER TABLE cache_entry ADD COLUMN mtime_filetime INTEGER NOT NULL DEFAULT 0;

CREATE TRIGGER cache_reservation_owner_consistent
BEFORE INSERT ON operation_reservation
WHEN NEW.owner_kind = 'CacheEvictionIntent' AND NOT EXISTS (
    SELECT 1
    FROM cache_eviction_intent AS intent
    JOIN cache_entry AS entry ON entry.cache_entry_id = intent.cache_entry_id
    WHERE intent.eviction_intent_id = NEW.cache_eviction_intent_id
      AND intent.cache_entry_id = NEW.cache_entry_id
      AND intent.cache_root_id = NEW.root_id
      AND intent.root_binding_id = NEW.root_binding_id
      AND entry.cache_root_id = NEW.root_id
      AND entry.path_key = NEW.path_key
      AND entry.file_id IS NEW.file_id
)
BEGIN
    SELECT RAISE(ABORT, 'cache reservation owner or identity is inconsistent');
END;

CREATE TRIGGER cache_eviction_fact_chain_consistent
BEFORE INSERT ON cache_eviction_fact
WHEN NOT EXISTS (
    SELECT 1 FROM cache_eviction_intent
    WHERE eviction_intent_id = NEW.eviction_intent_id
      AND cache_entry_id = NEW.cache_entry_id
)
BEGIN
    SELECT RAISE(ABORT, 'cache eviction fact does not match intent');
END;

CREATE TRIGGER cache_root_create_no_delete
BEFORE DELETE ON cache_root_create_intent BEGIN
    SELECT RAISE(ABORT, 'cache root create intent is immutable');
END;

CREATE TRIGGER cache_entry_state_transition
BEFORE UPDATE OF entry_state ON cache_entry
WHEN NOT (
    OLD.entry_state = NEW.entry_state
    OR (OLD.entry_state = 'Live' AND NEW.entry_state IN ('Missing', 'Evicted', 'NeedsAttention'))
)
BEGIN
    SELECT RAISE(ABORT, 'illegal cache entry transition');
END;

CREATE TRIGGER cache_entry_terminal_no_update
BEFORE UPDATE ON cache_entry
WHEN OLD.entry_state IN ('Missing', 'Evicted', 'NeedsAttention')
BEGIN
    SELECT RAISE(ABORT, 'terminal cache entry incarnation is immutable');
END;

CREATE TRIGGER cache_entry_no_delete
BEFORE DELETE ON cache_entry BEGIN
    SELECT RAISE(ABORT, 'cache entry history is immutable');
END;
