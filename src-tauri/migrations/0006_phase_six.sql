PRAGMA foreign_keys = ON;

-- Phase six makes the import intent/generation pointers explicit.  The phase-zero
-- skeleton deliberately left these nullable because no import job could yet be
-- created.  Creation is now guarded by the trigger below, so every phase-six job
-- has the complete ADR identity chain from its first visible commit.
ALTER TABLE import_job ADD COLUMN source_profile_id BLOB REFERENCES import_source_profile(profile_id) ON DELETE RESTRICT;
ALTER TABLE import_job ADD COLUMN source_volume_id BLOB REFERENCES volume_identity(volume_id) ON DELETE RESTRICT;
ALTER TABLE import_job ADD COLUMN current_confirmed_generation_id BLOB REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT;
ALTER TABLE import_job ADD COLUMN proposed_generation_id BLOB REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT;

ALTER TABLE import_plan_generation ADD COLUMN supersedes_generation_id BLOB REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT;

ALTER TABLE import_source_item ADD COLUMN source_relative_path TEXT;
ALTER TABLE import_source_item ADD COLUMN original_extension TEXT;
ALTER TABLE import_source_item ADD COLUMN capture_local_key TEXT;
ALTER TABLE import_source_item ADD COLUMN parse_result TEXT;

ALTER TABLE import_duplicate_proof ADD COLUMN target_asset_id BLOB REFERENCES asset(asset_id) ON DELETE RESTRICT;
ALTER TABLE import_duplicate_proof ADD COLUMN source_full_hash BLOB;
ALTER TABLE import_duplicate_proof ADD COLUMN target_full_hash BLOB;
ALTER TABLE import_duplicate_proof ADD COLUMN length_equal INTEGER CHECK (length_equal IS NULL OR length_equal = 1);

ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN source_relative_path TEXT;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN source_path_key_version INTEGER;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN source_volume_id BLOB REFERENCES volume_identity(volume_id) ON DELETE RESTRICT;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN source_binding_generation_before INTEGER;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN source_binding_generation_after INTEGER;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN target_relative_path TEXT;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN target_path_key_version INTEGER;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN target_volume_id BLOB REFERENCES volume_identity(volume_id) ON DELETE RESTRICT;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN target_binding_generation_before INTEGER;
ALTER TABLE import_duplicate_path_binding_proof ADD COLUMN target_binding_generation_after INTEGER;

CREATE TABLE generation_plan_digest (
    generation_id BLOB PRIMARY KEY CHECK (length(generation_id) = 16),
    digest BLOB NOT NULL CHECK (length(digest) = 32),
    FOREIGN KEY (generation_id) REFERENCES import_plan_generation(generation_id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER generation_plan_digest_no_update
BEFORE UPDATE ON generation_plan_digest BEGIN SELECT RAISE(ABORT, 'generation digest is immutable'); END;
CREATE TRIGGER generation_plan_digest_no_delete
BEFORE DELETE ON generation_plan_digest BEGIN SELECT RAISE(ABORT, 'generation digest is immutable'); END;

CREATE TRIGGER import_job_phase_six_identity_on_insert
AFTER INSERT ON import_job
WHEN (NEW.source_profile_id IS NULL) != (NEW.source_volume_id IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'phase six import job requires source profile and volume');
END;

CREATE TRIGGER import_job_pointer_consistent
BEFORE UPDATE OF current_confirmed_generation_id, proposed_generation_id ON import_job
WHEN (NEW.current_confirmed_generation_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM import_plan_generation
        WHERE generation_id=NEW.current_confirmed_generation_id
          AND job_id=NEW.job_id AND generation_state='Confirmed'
     ))
  OR (NEW.proposed_generation_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM import_plan_generation
        WHERE generation_id=NEW.proposed_generation_id
          AND job_id=NEW.job_id AND generation_state='Proposed'
     ))
BEGIN
    SELECT RAISE(ABORT, 'import job generation pointer is inconsistent');
END;

CREATE TRIGGER import_source_item_phase_six_payload
AFTER INSERT ON import_source_item
WHEN (NEW.source_relative_path IS NOT NULL
      OR NEW.original_extension IS NOT NULL
      OR NEW.parse_result IS NOT NULL)
 AND (NEW.source_relative_path IS NULL
      OR NEW.original_extension IS NULL
      OR NEW.parse_result IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'phase six source item payload is incomplete');
END;

CREATE TRIGGER import_generation_supersedes_consistent
BEFORE INSERT ON import_plan_generation
WHEN NEW.generation_no > 1 AND (
    NEW.supersedes_generation_id IS NULL OR NOT EXISTS (
        SELECT 1 FROM import_plan_generation
        WHERE generation_id=NEW.supersedes_generation_id
          AND job_id=NEW.job_id
          AND generation_no < NEW.generation_no
    )
)
BEGIN
    SELECT RAISE(ABORT, 'new import generation must supersede an older generation');
END;

CREATE TRIGGER import_attempt_ready_chain
BEFORE INSERT ON import_attempt
WHEN NEW.attempt_reason != 'CancelBeforeStart' AND NOT EXISTS (
    SELECT 1
    FROM import_plan_validation AS validation
    JOIN import_space_check AS space
      ON space.space_check_id=NEW.space_check_id
     AND space.validation_id=validation.validation_id
    WHERE validation.validation_id=NEW.validation_id
      AND validation.job_id=NEW.job_id
      AND validation.generation_id=NEW.generation_id
      AND validation.outcome='Ready'
      AND space.job_id=NEW.job_id
      AND space.generation_id=NEW.generation_id
      AND space.outcome='Passed'
)
BEGIN
    SELECT RAISE(ABORT, 'import attempt requires its own ready validation and passed space check');
END;

CREATE TRIGGER import_space_check_exact_bytes
BEFORE INSERT ON import_space_check
WHEN NEW.planned_new_bytes != COALESCE((
    SELECT SUM(remaining_new_bytes)
    FROM import_plan_validation_item
    WHERE validation_id=NEW.validation_id
), 0)
BEGIN
    SELECT RAISE(ABORT, 'space check bytes must equal validation remaining bytes');
END;

CREATE TRIGGER import_publish_phase_six_chain
BEFORE INSERT ON import_publish_intent
WHEN NOT EXISTS (
    SELECT 1
    FROM import_attempt_step AS attempt_step
    JOIN import_attempt AS attempt ON attempt.attempt_id=attempt_step.attempt_id
    JOIN import_partial_claim AS claim ON claim.claim_id=NEW.partial_claim_id
    JOIN import_partial_ownership AS ownership ON ownership.ownership_id=claim.ownership_id
    JOIN import_asset_step AS plan_step ON plan_step.step_id=attempt_step.step_id
    JOIN operation_reservation AS reservation ON reservation.reservation_id=NEW.final_path_reservation_id
    JOIN file_write_lease AS lease ON lease.slot=1
    WHERE attempt_step.attempt_step_id=NEW.attempt_step_id
      AND attempt_step.step_state='PartialVerified'
      AND attempt.attempt_state='Running'
      AND claim.attempt_step_id=attempt_step.attempt_step_id
      AND claim.released_at IS NULL
      AND ownership.step_id=plan_step.step_id
      AND reservation.attempt_id=attempt.attempt_id
      AND reservation.root_id=plan_step.final_root_id
      AND reservation.root_binding_id=plan_step.final_binding_id
      AND reservation.path_key=plan_step.final_path_key
      AND reservation.lease_generation=NEW.lease_generation
      AND reservation.released_at IS NULL
      AND lease.task_id=attempt.job_id
      AND lease.lease_generation=NEW.lease_generation
)
BEGIN
    SELECT RAISE(ABORT, 'phase six publish chain is stale or mismatched');
END;

CREATE TRIGGER import_published_fact_no_update
BEFORE UPDATE ON import_published_fact BEGIN SELECT RAISE(ABORT, 'published fact is immutable'); END;
CREATE TRIGGER import_published_fact_no_delete
BEFORE DELETE ON import_published_fact BEGIN SELECT RAISE(ABORT, 'published fact is immutable'); END;
CREATE TRIGGER import_indexed_fact_no_update
BEFORE UPDATE ON import_indexed_fact BEGIN SELECT RAISE(ABORT, 'indexed fact is immutable'); END;
CREATE TRIGGER import_indexed_fact_no_delete
BEFORE DELETE ON import_indexed_fact BEGIN SELECT RAISE(ABORT, 'indexed fact is immutable'); END;

CREATE TRIGGER import_partial_cleanup_chain
BEFORE INSERT ON import_partial_cleanup_fact
WHEN NOT EXISTS (
    SELECT 1
    FROM import_partial_ownership AS ownership
    JOIN import_partial_claim AS claim ON claim.ownership_id=ownership.ownership_id
    JOIN import_attempt_step AS attempt_step ON attempt_step.attempt_step_id=claim.attempt_step_id
    WHERE ownership.ownership_id=NEW.ownership_id
      AND ownership.state='CleaningOwnedPartial'
      AND claim.claim_id=NEW.claim_id
      AND claim.released_at IS NULL
      AND attempt_step.attempt_step_id=NEW.attempt_step_id
      AND attempt_step.step_state='CleaningOwnedPartial'
)
BEGIN
    SELECT RAISE(ABORT, 'partial cleanup fact requires committed cleaning ownership and claim');
END;

PRAGMA user_version = 6;
