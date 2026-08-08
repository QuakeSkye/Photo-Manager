-- Phase 8 adds only scale-hardening indexes. It does not add product behavior.
CREATE INDEX ix_photo_timeline_capture_desc
ON photo(library_id, date_class, capture_local_key DESC, natural_sort_key, photo_id)
WHERE lifecycle = 'Active';

CREATE INDEX ix_photo_timeline_capture_asc
ON photo(library_id, date_class, capture_local_key ASC, natural_sort_key, photo_id)
WHERE lifecycle = 'Active';

CREATE INDEX ix_photo_asset_current_photo
ON photo_asset(photo_id, asset_id, role)
WHERE to_photo_version IS NULL;

CREATE INDEX ix_asset_current_library_presence
ON asset(library_id, presence_state, asset_id, current_version, media_kind);

PRAGMA user_version = 8;
