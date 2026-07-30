-- Phase 3 keeps deletion decisions in SQLite only. These indexes support the
-- authoritative pending-review query without introducing executable batches.
CREATE INDEX ix_deletion_mark_active_created
ON deletion_mark(created_at, photo_id)
WHERE released_at IS NULL;

CREATE INDEX ix_deletion_mark_snapshot_mark
ON deletion_mark_asset_snapshot(mark_id, role, asset_id);

CREATE INDEX ix_photo_active_library_review
ON photo(library_id, lifecycle, photo_id, photo_version);

-- Recycle remains blocked by the independent runtime feature gate. This bit
-- only records that an online NTFS Fuji root is eligible for deletion review.
UPDATE root
SET capability_mask = capability_mask | 64,
    root_version = root_version + 1
WHERE root_kind = 'FujiLibrary'
  AND EXISTS (
      SELECT 1 FROM root_binding binding
      WHERE binding.binding_id = root.current_binding_id
        AND binding.filesystem_kind = 'NTFS'
        AND binding.binding_state = 'Active'
  );

PRAGMA user_version = 3;
