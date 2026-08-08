-- Phase 7 completes the durable evidence carried by each recycle callback.
-- It deliberately does not create a recycle admission record.

ALTER TABLE deletion_attempt_asset ADD COLUMN perform_operations_result INTEGER;
ALTER TABLE deletion_attempt_asset ADD COLUMN operations_aborted INTEGER
    CHECK (operations_aborted IS NULL OR operations_aborted IN (0, 1));
ALTER TABLE deletion_attempt_asset ADD COLUMN recycle_object_evidence BLOB;
ALTER TABLE deletion_attempt_asset ADD COLUMN source_path_missing INTEGER
    CHECK (source_path_missing IS NULL OR source_path_missing IN (0, 1));
ALTER TABLE deletion_attempt_asset ADD COLUMN error_code TEXT;
ALTER TABLE deletion_attempt_asset ADD COLUMN committed_at INTEGER;

CREATE TRIGGER deletion_batch_transition
BEFORE UPDATE OF batch_state ON deletion_batch
WHEN NOT (
    (OLD.batch_state='Prepared' AND NEW.batch_state='Running')
    OR (OLD.batch_state='Running' AND NEW.batch_state IN ('CancelRequested','Completed','CompletedWithErrors','NeedsAttention','Cancelled'))
    OR (OLD.batch_state='CancelRequested' AND NEW.batch_state IN ('Completed','CompletedWithErrors','NeedsAttention','Cancelled'))
    OR (OLD.batch_state IN ('CompletedWithErrors','NeedsAttention') AND NEW.batch_state IN ('Running','Cancelled'))
)
BEGIN SELECT RAISE(ABORT, 'illegal deletion batch transition'); END;

CREATE TRIGGER deletion_batch_terminal_no_update
BEFORE UPDATE ON deletion_batch
WHEN OLD.batch_state IN ('Completed','Cancelled')
BEGIN SELECT RAISE(ABORT, 'terminal deletion batch is immutable'); END;

CREATE TRIGGER deletion_batch_no_delete
BEFORE DELETE ON deletion_batch
BEGIN SELECT RAISE(ABORT, 'deletion batch is immutable'); END;

CREATE TRIGGER deletion_batch_photo_no_update
BEFORE UPDATE ON deletion_batch_photo
BEGIN SELECT RAISE(ABORT, 'deletion batch photo is immutable'); END;

CREATE TRIGGER deletion_batch_photo_no_delete
BEFORE DELETE ON deletion_batch_photo
BEGIN SELECT RAISE(ABORT, 'deletion batch photo is immutable'); END;

CREATE TRIGGER deletion_batch_asset_plan_immutable
BEFORE UPDATE OF batch_photo_id,asset_id,role,asset_version,mark_asset_snapshot_id
ON deletion_batch_asset
BEGIN SELECT RAISE(ABORT, 'deletion batch asset plan is immutable'); END;

CREATE TRIGGER deletion_batch_asset_resolution_transition
BEFORE UPDATE OF resolution_state ON deletion_batch_asset
WHEN NOT (
    (OLD.resolution_state IN ('Pending','RetryableFailed') AND NEW.resolution_state IN ('Recycled','RetryableFailed','NeedsAttention','CancelledRemaining'))
    OR (OLD.resolution_state='NeedsAttention' AND NEW.resolution_state IN ('Recycled','RetryableFailed','NeedsAttention','CancelledRemaining','ResolvedWithoutDelete'))
)
BEGIN SELECT RAISE(ABORT, 'illegal deletion batch asset resolution transition'); END;

CREATE TRIGGER deletion_batch_asset_no_delete
BEFORE DELETE ON deletion_batch_asset
BEGIN SELECT RAISE(ABORT, 'deletion batch asset is immutable'); END;

CREATE TRIGGER deletion_attempt_asset_transition
BEFORE UPDATE OF asset_state ON deletion_attempt_asset
WHEN NOT (
    (OLD.asset_state='Prepared' AND NEW.asset_state IN ('Validating','Cancelled','Interrupted'))
    OR (OLD.asset_state='Validating' AND NEW.asset_state IN ('Recycling','Cancelled','Interrupted','RetryableFailed','NeedsAttention'))
    OR (OLD.asset_state='Recycling' AND NEW.asset_state IN ('Recycled','RetryableFailed','NeedsAttention','Interrupted'))
)
BEGIN SELECT RAISE(ABORT, 'illegal deletion attempt asset transition'); END;

CREATE TRIGGER deletion_attempt_asset_no_delete
BEFORE DELETE ON deletion_attempt_asset
BEGIN SELECT RAISE(ABORT, 'deletion attempt asset is immutable'); END;

CREATE TRIGGER deletion_recycled_fact_chain
BEFORE INSERT ON deletion_recycled_fact
WHEN NOT EXISTS (
    SELECT 1
    FROM deletion_attempt_asset AS attempt_asset
    JOIN deletion_attempt AS attempt ON attempt.attempt_id=attempt_asset.attempt_id
    JOIN deletion_batch_asset AS batch_asset ON batch_asset.batch_asset_id=attempt_asset.batch_asset_id
    JOIN deletion_batch_photo AS batch_photo ON batch_photo.batch_photo_id=batch_asset.batch_photo_id
    WHERE attempt_asset.attempt_asset_id=NEW.attempt_asset_id
      AND attempt_asset.batch_asset_id=NEW.batch_asset_id
      AND attempt_asset.asset_state='Recycled'
      AND attempt.batch_id=batch_photo.batch_id
      AND attempt_asset.shell_result >= 0
      AND attempt_asset.recycle_object_present=1
      AND attempt_asset.source_path_missing=1
      AND attempt_asset.recycle_object_evidence IS NOT NULL
)
BEGIN SELECT RAISE(ABORT, 'recycled fact must match a proven attempt asset chain'); END;

CREATE INDEX ix_deletion_batch_asset_unresolved
ON deletion_batch_asset(resolution_state,batch_photo_id);

CREATE INDEX ix_deletion_attempt_asset_attempt_state
ON deletion_attempt_asset(attempt_id,asset_state);

PRAGMA user_version = 7;
