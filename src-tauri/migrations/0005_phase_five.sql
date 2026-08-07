PRAGMA foreign_keys = ON;

CREATE TABLE import_source_profile (
    profile_id BLOB PRIMARY KEY CHECK (length(profile_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    source_root_id BLOB NOT NULL UNIQUE CHECK (length(source_root_id) = 16),
    volume_id BLOB NOT NULL CHECK (length(volume_id) = 16),
    source_relative_path TEXT NOT NULL,
    source_path_key BLOB NOT NULL,
    source_directory_file_id BLOB,
    identity_strength TEXT NOT NULL CHECK (identity_strength IN ('Strong', 'UserConfirmedLimited')),
    last_confirmed_at INTEGER NOT NULL,
    retired_at INTEGER,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_root_id) REFERENCES root(root_id) ON DELETE RESTRICT,
    FOREIGN KEY (volume_id) REFERENCES volume_identity(volume_id) ON DELETE RESTRICT,
    UNIQUE (library_id, volume_id, source_path_key)
) STRICT;

CREATE INDEX ix_import_source_profile_prefill
ON import_source_profile(library_id, retired_at, last_confirmed_at DESC);

CREATE TABLE import_history (
    history_id BLOB PRIMARY KEY CHECK (length(history_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    source_volume_id BLOB NOT NULL CHECK (length(source_volume_id) = 16),
    source_file_id BLOB,
    source_size_bytes INTEGER NOT NULL CHECK (source_size_bytes >= 0),
    source_mtime INTEGER NOT NULL,
    source_full_hash BLOB NOT NULL CHECK (length(source_full_hash) = 32),
    hash_algorithm TEXT NOT NULL CHECK (hash_algorithm = 'SHA-256'),
    target_asset_id BLOB NOT NULL CHECK (length(target_asset_id) = 16),
    published_fact_id BLOB CHECK (published_fact_id IS NULL OR length(published_fact_id) = 16),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_volume_id) REFERENCES volume_identity(volume_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (published_fact_id) REFERENCES import_published_fact(published_fact_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX ix_import_history_source_identity
ON import_history(library_id, source_volume_id, source_file_id, source_size_bytes, source_mtime);

CREATE INDEX ix_import_history_source_hash
ON import_history(library_id, source_size_bytes, source_full_hash);

CREATE TRIGGER import_history_no_update
BEFORE UPDATE ON import_history BEGIN
    SELECT RAISE(ABORT, 'import history is append-only');
END;

CREATE TRIGGER import_history_no_delete
BEFORE DELETE ON import_history BEGIN
    SELECT RAISE(ABORT, 'import history is append-only');
END;

PRAGMA user_version = 5;
