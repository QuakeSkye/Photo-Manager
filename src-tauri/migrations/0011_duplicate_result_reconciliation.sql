PRAGMA foreign_keys = ON;

CREATE TABLE catalog_revision_event (
    event_id BLOB PRIMARY KEY CHECK (length(event_id) = 16),
    library_id BLOB NOT NULL CHECK (length(library_id) = 16),
    root_binding_id BLOB NOT NULL CHECK (length(root_binding_id) = 16),
    from_revision INTEGER NOT NULL CHECK (from_revision >= 0),
    to_revision INTEGER NOT NULL CHECK (to_revision = from_revision + 1),
    cause_kind TEXT NOT NULL CHECK (
        cause_kind IN ('AssetObserved','ScanMissing','RootBindingChanged','RecycleTargeted')
    ),
    scan_run_id BLOB CHECK (scan_run_id IS NULL OR length(scan_run_id) = 16),
    recycle_batch_id BLOB CHECK (recycle_batch_id IS NULL OR length(recycle_batch_id) = 16),
    created_at INTEGER NOT NULL,
    UNIQUE (library_id, to_revision),
    FOREIGN KEY (library_id) REFERENCES library(library_id) ON DELETE RESTRICT,
    FOREIGN KEY (root_binding_id) REFERENCES root_binding(binding_id) ON DELETE RESTRICT,
    FOREIGN KEY (scan_run_id) REFERENCES scan_run(scan_run_id) ON DELETE RESTRICT,
    FOREIGN KEY (recycle_batch_id) REFERENCES deletion_batch(batch_id) ON DELETE RESTRICT,
    CHECK ((cause_kind='RecycleTargeted') = (recycle_batch_id IS NOT NULL))
) STRICT;

CREATE TABLE catalog_revision_asset_change (
    event_id BLOB NOT NULL CHECK (length(event_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    before_asset_version INTEGER CHECK (before_asset_version IS NULL OR before_asset_version >= 1),
    after_asset_version INTEGER NOT NULL CHECK (after_asset_version >= 1),
    before_presence TEXT CHECK (
        before_presence IS NULL OR before_presence IN ('Present','Missing','Tombstoned')
    ),
    after_presence TEXT NOT NULL CHECK (after_presence IN ('Present','Missing','Tombstoned')),
    change_kind TEXT NOT NULL CHECK (change_kind IN ('Inserted','Updated','Missing')),
    PRIMARY KEY (event_id, asset_id),
    FOREIGN KEY (event_id) REFERENCES catalog_revision_event(event_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id) REFERENCES asset(asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id, after_asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX catalog_revision_event_recycle_batch
ON catalog_revision_event(recycle_batch_id)
WHERE recycle_batch_id IS NOT NULL;

CREATE TRIGGER catalog_revision_event_chain
BEFORE INSERT ON catalog_revision_event
WHEN NOT EXISTS (
    SELECT 1 FROM library_catalog_revision revision
    WHERE revision.library_id=NEW.library_id
      AND revision.root_binding_id=NEW.root_binding_id
      AND revision.catalog_revision=NEW.to_revision
)
BEGIN
    SELECT RAISE(ABORT, 'catalog revision event does not match current revision');
END;

CREATE TRIGGER catalog_revision_asset_change_chain
BEFORE INSERT ON catalog_revision_asset_change
WHEN NOT EXISTS (
    SELECT 1
    FROM catalog_revision_event event
    JOIN asset ON asset.asset_id=NEW.asset_id
    JOIN asset_revision after_revision
      ON after_revision.asset_id=NEW.asset_id
     AND after_revision.asset_version=NEW.after_asset_version
    LEFT JOIN asset_revision before_revision
      ON before_revision.asset_id=NEW.asset_id
     AND before_revision.asset_version=NEW.before_asset_version
    WHERE event.event_id=NEW.event_id
      AND asset.library_id=event.library_id
      AND after_revision.root_binding_id=event.root_binding_id
      AND after_revision.presence_state=NEW.after_presence
      AND (
          (NEW.before_asset_version IS NULL
           AND NEW.before_presence IS NULL
           AND NEW.change_kind='Inserted')
          OR
          (NEW.before_asset_version IS NOT NULL
           AND NEW.after_asset_version=NEW.before_asset_version+1
           AND before_revision.presence_state=NEW.before_presence)
      )
)
BEGIN
    SELECT RAISE(ABORT, 'catalog revision asset change chain is inconsistent');
END;

CREATE TRIGGER catalog_revision_event_no_update
BEFORE UPDATE ON catalog_revision_event BEGIN
    SELECT RAISE(ABORT, 'catalog revision events are immutable');
END;
CREATE TRIGGER catalog_revision_event_no_delete
BEFORE DELETE ON catalog_revision_event BEGIN
    SELECT RAISE(ABORT, 'catalog revision events are immutable');
END;
CREATE TRIGGER catalog_revision_asset_change_no_update
BEFORE UPDATE ON catalog_revision_asset_change BEGIN
    SELECT RAISE(ABORT, 'catalog revision asset changes are immutable');
END;
CREATE TRIGGER catalog_revision_asset_change_no_delete
BEFORE DELETE ON catalog_revision_asset_change BEGIN
    SELECT RAISE(ABORT, 'catalog revision asset changes are immutable');
END;

CREATE TABLE duplicate_result_reconciliation (
    reconciliation_id BLOB PRIMARY KEY CHECK (length(reconciliation_id) = 16),
    source_run_id BLOB NOT NULL CHECK (length(source_run_id) = 16),
    parent_reconciliation_id BLOB CHECK (
        parent_reconciliation_id IS NULL OR length(parent_reconciliation_id) = 16
    ),
    recycle_batch_id BLOB NOT NULL CHECK (length(recycle_batch_id) = 16),
    from_catalog_revision INTEGER NOT NULL CHECK (from_catalog_revision >= 0),
    to_catalog_revision INTEGER NOT NULL CHECK (to_catalog_revision >= from_catalog_revision),
    reconciliation_state TEXT NOT NULL CHECK (
        reconciliation_state IN ('Pending','Applied','Rejected')
    ),
    removed_asset_count INTEGER NOT NULL DEFAULT 0 CHECK (removed_asset_count >= 0),
    removed_bytes INTEGER NOT NULL DEFAULT 0 CHECK (removed_bytes >= 0),
    rejection_reason TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE (source_run_id, recycle_batch_id),
    FOREIGN KEY (source_run_id) REFERENCES duplicate_scan_run(run_id) ON DELETE RESTRICT,
    FOREIGN KEY (parent_reconciliation_id)
        REFERENCES duplicate_result_reconciliation(reconciliation_id) ON DELETE RESTRICT,
    FOREIGN KEY (recycle_batch_id) REFERENCES deletion_batch(batch_id) ON DELETE RESTRICT,
    CHECK ((reconciliation_state='Pending') = (completed_at IS NULL)),
    CHECK ((reconciliation_state='Rejected') = (rejection_reason IS NOT NULL))
) STRICT;

CREATE TABLE duplicate_result_reconciliation_member (
    reconciliation_id BLOB NOT NULL CHECK (length(reconciliation_id) = 16),
    set_id BLOB NOT NULL CHECK (length(set_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    batch_asset_id BLOB NOT NULL UNIQUE CHECK (length(batch_asset_id) = 16),
    recycled_fact_id BLOB NOT NULL UNIQUE CHECK (length(recycled_fact_id) = 16),
    catalog_revision_event_id BLOB NOT NULL CHECK (length(catalog_revision_event_id) = 16),
    removed_bytes INTEGER NOT NULL CHECK (removed_bytes >= 0),
    PRIMARY KEY (reconciliation_id, asset_id),
    FOREIGN KEY (reconciliation_id)
        REFERENCES duplicate_result_reconciliation(reconciliation_id) ON DELETE RESTRICT,
    FOREIGN KEY (set_id) REFERENCES duplicate_equivalence_set(set_id) ON DELETE RESTRICT,
    FOREIGN KEY (asset_id, asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    FOREIGN KEY (batch_asset_id) REFERENCES deletion_batch_asset(batch_asset_id) ON DELETE RESTRICT,
    FOREIGN KEY (recycled_fact_id)
        REFERENCES deletion_recycled_fact(recycled_fact_id) ON DELETE RESTRICT,
    FOREIGN KEY (catalog_revision_event_id)
        REFERENCES catalog_revision_event(event_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX duplicate_result_reconciliation_latest
ON duplicate_result_reconciliation(source_run_id, completed_at DESC)
WHERE reconciliation_state!='Pending';

CREATE TRIGGER duplicate_result_reconciliation_member_chain
BEFORE INSERT ON duplicate_result_reconciliation_member
WHEN NOT EXISTS (
    SELECT 1
    FROM duplicate_result_reconciliation reconciliation
    JOIN duplicate_equivalence_set equivalent_set
      ON equivalent_set.set_id=NEW.set_id
    JOIN deletion_batch_asset batch_asset
      ON batch_asset.batch_asset_id=NEW.batch_asset_id
    JOIN deletion_batch_photo batch_photo
      ON batch_photo.batch_photo_id=batch_asset.batch_photo_id
    JOIN deletion_mark_asset_snapshot marked
      ON marked.mark_asset_snapshot_id=batch_asset.mark_asset_snapshot_id
    JOIN deletion_recycled_fact recycled
      ON recycled.recycled_fact_id=NEW.recycled_fact_id
     AND recycled.batch_asset_id=batch_asset.batch_asset_id
    JOIN catalog_revision_event event
      ON event.event_id=NEW.catalog_revision_event_id
    JOIN catalog_revision_asset_change change
      ON change.event_id=event.event_id AND change.asset_id=NEW.asset_id
    WHERE reconciliation.reconciliation_id=NEW.reconciliation_id
      AND reconciliation.reconciliation_state='Pending'
      AND equivalent_set.run_id=reconciliation.source_run_id
      AND batch_photo.batch_id=reconciliation.recycle_batch_id
      AND event.recycle_batch_id=reconciliation.recycle_batch_id
      AND batch_asset.asset_id=NEW.asset_id
      AND marked.asset_id=NEW.asset_id
      AND marked.asset_version=NEW.asset_version
      AND change.before_asset_version=NEW.asset_version
      AND change.after_presence='Missing'
      AND EXISTS (
          SELECT 1
          FROM duplicate_equivalence_proof proof
          JOIN duplicate_file_observation observation
            ON observation.observation_id IN (
                proof.anchor_observation_id,proof.member_observation_id
            )
          WHERE proof.set_id=NEW.set_id
            AND proof.run_id=reconciliation.source_run_id
            AND observation.asset_id=NEW.asset_id
            AND observation.asset_version=NEW.asset_version
      )
)
BEGIN
    SELECT RAISE(ABORT, 'duplicate reconciliation member chain is inconsistent');
END;

CREATE TRIGGER duplicate_result_reconciliation_applied_totals
BEFORE UPDATE OF reconciliation_state ON duplicate_result_reconciliation
WHEN NEW.reconciliation_state='Applied' AND (
    NEW.removed_asset_count!=(
        SELECT COUNT(*) FROM duplicate_result_reconciliation_member member
        WHERE member.reconciliation_id=NEW.reconciliation_id
    )
    OR NEW.removed_bytes!=COALESCE((
        SELECT SUM(member.removed_bytes)
        FROM duplicate_result_reconciliation_member member
        WHERE member.reconciliation_id=NEW.reconciliation_id
    ),0)
)
BEGIN
    SELECT RAISE(ABORT, 'applied duplicate reconciliation totals are inconsistent');
END;

CREATE TRIGGER duplicate_result_reconciliation_transition
BEFORE UPDATE ON duplicate_result_reconciliation
WHEN OLD.reconciliation_state!='Pending'
  OR NEW.reconciliation_id IS NOT OLD.reconciliation_id
  OR NEW.source_run_id IS NOT OLD.source_run_id
  OR NEW.parent_reconciliation_id IS NOT OLD.parent_reconciliation_id
  OR NEW.recycle_batch_id IS NOT OLD.recycle_batch_id
  OR NEW.from_catalog_revision IS NOT OLD.from_catalog_revision
  OR NEW.to_catalog_revision IS NOT OLD.to_catalog_revision
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.reconciliation_state='Pending'
BEGIN
    SELECT RAISE(ABORT, 'invalid duplicate reconciliation transition');
END;

CREATE TRIGGER duplicate_result_reconciliation_no_delete
BEFORE DELETE ON duplicate_result_reconciliation BEGIN
    SELECT RAISE(ABORT, 'duplicate reconciliations are append-only');
END;
CREATE TRIGGER duplicate_result_reconciliation_member_no_update
BEFORE UPDATE ON duplicate_result_reconciliation_member BEGIN
    SELECT RAISE(ABORT, 'duplicate reconciliation members are immutable');
END;
CREATE TRIGGER duplicate_result_reconciliation_member_no_delete
BEFORE DELETE ON duplicate_result_reconciliation_member BEGIN
    SELECT RAISE(ABORT, 'duplicate reconciliation members are immutable');
END;

PRAGMA user_version = 11;
