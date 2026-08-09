CREATE TABLE asset_capture_metadata (
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_version INTEGER NOT NULL CHECK (asset_version >= 1),
    parser_version INTEGER NOT NULL CHECK (parser_version >= 1),
    lens_model TEXT,
    f_number_numerator INTEGER,
    f_number_denominator INTEGER,
    exposure_time_numerator INTEGER,
    exposure_time_denominator INTEGER,
    iso_value INTEGER CHECK (iso_value IS NULL OR iso_value > 0),
    focal_length_numerator INTEGER,
    focal_length_denominator INTEGER,
    pixel_width INTEGER CHECK (pixel_width IS NULL OR pixel_width > 0),
    pixel_height INTEGER CHECK (pixel_height IS NULL OR pixel_height > 0),
    film_simulation_key TEXT,
    film_simulation_code INTEGER,
    PRIMARY KEY (asset_id, asset_version, parser_version),
    FOREIGN KEY (asset_id, asset_version)
        REFERENCES asset_revision(asset_id, asset_version) ON DELETE RESTRICT,
    CHECK ((f_number_numerator IS NULL) = (f_number_denominator IS NULL)),
    CHECK (f_number_denominator IS NULL OR f_number_denominator != 0),
    CHECK ((exposure_time_numerator IS NULL) = (exposure_time_denominator IS NULL)),
    CHECK (exposure_time_denominator IS NULL OR exposure_time_denominator != 0),
    CHECK ((focal_length_numerator IS NULL) = (focal_length_denominator IS NULL)),
    CHECK (focal_length_denominator IS NULL OR focal_length_denominator != 0),
    CHECK ((film_simulation_key IS NOT NULL) OR (film_simulation_code IS NULL))
) STRICT;

CREATE TRIGGER asset_capture_metadata_no_update
BEFORE UPDATE ON asset_capture_metadata BEGIN
    SELECT RAISE(ABORT, 'asset capture metadata is immutable');
END;

CREATE TRIGGER asset_capture_metadata_no_delete
BEFORE DELETE ON asset_capture_metadata BEGIN
    SELECT RAISE(ABORT, 'asset capture metadata is immutable');
END;

PRAGMA user_version = 9;
