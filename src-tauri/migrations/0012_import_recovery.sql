-- Old Planned rows have no creation-boundary evidence and remain conservative.
ALTER TABLE import_partial_ownership ADD COLUMN creation_started INTEGER NOT NULL DEFAULT 1 CHECK (creation_started IN (0,1));
DROP TRIGGER import_partial_ownership_transition;
CREATE TRIGGER import_partial_ownership_transition BEFORE UPDATE OF state ON import_partial_ownership
WHEN NOT (
 (OLD.state='Planned' AND NEW.state IN ('Owned','NeedsAttention'))
 OR (OLD.state='Planned' AND OLD.creation_started=0 AND NEW.state='Cleaned')
 OR (OLD.state='Owned' AND NEW.state IN ('CleaningOwnedPartial','ConsumedByPublish','NeedsAttention'))
 OR (OLD.state='CleaningOwnedPartial' AND NEW.state IN ('Cleaned','NeedsAttention'))
) BEGIN SELECT RAISE(ABORT,'illegal partial ownership transition'); END;
CREATE TRIGGER import_creation_monotonic BEFORE UPDATE OF creation_started ON import_partial_ownership
WHEN NOT (OLD.creation_started=0 AND NEW.creation_started=1 AND OLD.state='Planned')
BEGIN SELECT RAISE(ABORT,'creation boundary is monotonic'); END;
CREATE TABLE import_recovery_exclusion (
 source_item_id BLOB PRIMARY KEY REFERENCES import_source_item(source_item_id),
 confirmed_at INTEGER NOT NULL
) STRICT;
CREATE TRIGGER import_exclusion_no_update BEFORE UPDATE ON import_recovery_exclusion BEGIN SELECT RAISE(ABORT,'exclusion is immutable'); END;
CREATE TRIGGER import_exclusion_no_delete BEFORE DELETE ON import_recovery_exclusion BEGIN SELECT RAISE(ABORT,'exclusion is immutable'); END;
CREATE TABLE import_retained_path (
 root_id BLOB NOT NULL REFERENCES root(root_id),
 path_key BLOB NOT NULL,
 job_id BLOB NOT NULL REFERENCES import_job(job_id),
 relative_path TEXT NOT NULL,
 size_bytes INTEGER,
 reason TEXT NOT NULL,
 PRIMARY KEY(root_id,path_key)
) STRICT;
CREATE TRIGGER import_retained_path_guard BEFORE INSERT ON operation_reservation
WHEN NEW.reservation_kind <> 'SourceRead' AND EXISTS(SELECT 1 FROM import_retained_path p WHERE p.root_id=NEW.root_id AND p.path_key=NEW.path_key)
BEGIN SELECT RAISE(ABORT,'path retained for import recovery'); END;

PRAGMA user_version = 12;
