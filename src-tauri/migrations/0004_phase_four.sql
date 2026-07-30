PRAGMA user_version = 4;

CREATE TABLE external_tool (
    tool_id BLOB PRIMARY KEY CHECK (length(tool_id) = 16),
    tool_kind TEXT NOT NULL CHECK (tool_kind = 'Photoshop'),
    executable_path_display TEXT NOT NULL,
    executable_path_key BLOB NOT NULL,
    file_id BLOB NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
    mtime_filetime INTEGER NOT NULL,
    validation_state TEXT NOT NULL CHECK (validation_state IN ('Valid', 'Invalid')),
    created_at INTEGER NOT NULL,
    last_validated_at INTEGER NOT NULL,
    invalidated_at INTEGER,
    CHECK ((validation_state = 'Invalid') = (invalidated_at IS NOT NULL))
) STRICT;

CREATE TABLE external_tool_policy (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    photoshop_tool_id BLOB,
    row_version INTEGER NOT NULL DEFAULT 1 CHECK (row_version >= 1),
    FOREIGN KEY (photoshop_tool_id) REFERENCES external_tool(tool_id) ON DELETE RESTRICT
) STRICT;

INSERT INTO external_tool_policy(slot, photoshop_tool_id, row_version)
VALUES (1, NULL, 1);

CREATE TRIGGER external_tool_no_delete
BEFORE DELETE ON external_tool BEGIN
    SELECT RAISE(ABORT, 'external tool history is retained');
END;

CREATE TRIGGER external_tool_identity_immutable
BEFORE UPDATE OF tool_id, tool_kind, executable_path_display, executable_path_key,
                 file_id, size_bytes, mtime_filetime, created_at
ON external_tool BEGIN
    SELECT RAISE(ABORT, 'external tool identity is immutable');
END;
