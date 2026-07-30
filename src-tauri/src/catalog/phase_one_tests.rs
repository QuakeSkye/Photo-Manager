use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::Arc,
};

use sha2::{Digest, Sha256};

use crate::{db::Database, roots::FujiLibraryService, safety::TestRoot};

use super::{CatalogService, LibraryScanner, ScanRequest, SortMode};

struct Fixture {
    _parent: tempfile::TempDir,
    library_path: std::path::PathBuf,
    database: Arc<Database>,
    binding: crate::roots::FujiLibraryBinding,
}

fn fixture() -> Fixture {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("generated-fuji-library");
    fs::create_dir(&library_path).unwrap();
    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    Fixture {
        _parent: parent,
        library_path,
        database,
        binding,
    }
}

fn request(fixture: &Fixture) -> ScanRequest {
    ScanRequest {
        library_id: fixture.binding.library_id,
        root_id: fixture.binding.root_id,
        root_binding_id: fixture.binding.root_binding_id,
        root_path: fixture.library_path.clone(),
    }
}

fn generated_jpeg(date: Option<&str>) -> Vec<u8> {
    let Some(date) = date else {
        return vec![0xff, 0xd8, 0xff, 0xd9];
    };
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42_u16.to_le_bytes());
    tiff.extend_from_slice(&8_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    tiff.extend_from_slice(&0x8769_u16.to_le_bytes());
    tiff.extend_from_slice(&4_u16.to_le_bytes());
    tiff.extend_from_slice(&1_u32.to_le_bytes());
    tiff.extend_from_slice(&26_u32.to_le_bytes());
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    tiff.extend_from_slice(&0x9003_u16.to_le_bytes());
    tiff.extend_from_slice(&2_u16.to_le_bytes());
    tiff.extend_from_slice(&20_u32.to_le_bytes());
    tiff.extend_from_slice(&44_u32.to_le_bytes());
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(date.as_bytes());
    tiff.push(0);
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(tiff);
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    jpeg.extend(payload);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    jpeg
}

fn generated_raf(jpeg: &[u8]) -> Vec<u8> {
    let mut raf = vec![0; 92];
    raf[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    raf[84..88].copy_from_slice(&92_u32.to_be_bytes());
    raf[88..92].copy_from_slice(&u32::try_from(jpeg.len()).unwrap().to_be_bytes());
    raf.extend_from_slice(jpeg);
    raf
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    directory: bool,
    length: u64,
    readonly: bool,
    modified: Option<std::time::SystemTime>,
    sha256: Option<Vec<u8>>,
}

fn tree_snapshot(root: &Path) -> BTreeMap<String, TreeEntry> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<String, TreeEntry>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('/', "\\");
            let metadata = fs::symlink_metadata(&path).unwrap();
            let directory = metadata.is_dir();
            let sha256 = metadata
                .is_file()
                .then(|| Sha256::digest(fs::read(&path).unwrap()).to_vec());
            result.insert(
                relative,
                TreeEntry {
                    directory,
                    length: metadata.len(),
                    readonly: metadata.permissions().readonly(),
                    modified: metadata.modified().ok(),
                    sha256,
                },
            );
            if directory {
                visit(root, &path, result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn repeat_scan_pairs_sorts_unknown_last_and_changes_no_original() {
    let fixture = fixture();
    let raw = fixture.library_path.join("raw");
    fs::create_dir(&raw).unwrap();
    let newest = generated_jpeg(Some("2026:07:29 12:00:00"));
    fs::write(fixture.library_path.join("DSCF9.JPG"), &newest).unwrap();
    fs::write(raw.join("DSCF9.RAF"), generated_raf(&newest)).unwrap();
    fs::write(
        fixture.library_path.join("DSCF10.JPEG"),
        generated_jpeg(Some("2025:07:29 12:00:00")),
    )
    .unwrap();
    fs::write(
        fixture.library_path.join("UNKNOWN.JPG"),
        generated_jpeg(None),
    )
    .unwrap();
    fs::write(
        fixture.library_path.join("ignored.png"),
        b"generated ignored fixture",
    )
    .unwrap();
    let before = tree_snapshot(&fixture.library_path);
    let catalog = Arc::new(CatalogService::new(fixture.database.clone()));
    let scanner = LibraryScanner::new(catalog.clone());

    let first = scanner.scan(request(&fixture)).unwrap();
    assert_eq!(first.completion.observed_assets, 4);
    assert_eq!(first.ignored_by_extension.get(".png"), Some(&1));
    let first_ids = catalog_rows(&fixture.database);
    let second = scanner.scan(request(&fixture)).unwrap();
    assert_eq!(second.completion.observed_assets, 4);
    assert_eq!(catalog_rows(&fixture.database), first_ids);
    assert_eq!(tree_snapshot(&fixture.library_path), before);

    let descending = catalog
        .timeline(fixture.binding.library_id, SortMode::CaptureDesc)
        .unwrap();
    assert_eq!(descending.len(), 3);
    assert_eq!(descending[0].natural_name_display, "DSCF9.JPG");
    assert_eq!(descending[1].natural_name_display, "DSCF10.JPEG");
    assert_eq!(descending[2].natural_name_display, "UNKNOWN.JPG");
    let ascending = catalog
        .timeline(fixture.binding.library_id, SortMode::CaptureAsc)
        .unwrap();
    assert_eq!(ascending[0].natural_name_display, "DSCF10.JPEG");
    assert_eq!(ascending[1].natural_name_display, "DSCF9.JPG");
    assert_eq!(ascending[2].natural_name_display, "UNKNOWN.JPG");
    let paired: i64 = fixture.database.open_read_connection().unwrap().query_row(
        "SELECT COUNT(*) FROM photo WHERE lifecycle='Active' AND (
           SELECT COUNT(*) FROM photo_asset WHERE photo_id=photo.photo_id AND to_photo_version IS NULL
         )=2",
        (), |row| row.get(0),
    ).unwrap();
    assert_eq!(paired, 1);
}

#[test]
fn cancelled_full_scan_keeps_partial_index_but_never_changes_originals_or_reconciles_missing() {
    let fixture = fixture();
    for index in 1..=4 {
        fs::write(
            fixture.library_path.join(format!("DSCF{index}.JPG")),
            generated_jpeg(Some(&format!("2026:07:{index:02} 12:00:00"))),
        )
        .unwrap();
    }
    let before = tree_snapshot(&fixture.library_path);
    let catalog = Arc::new(CatalogService::new(fixture.database.clone()));
    let scanner = LibraryScanner::new(catalog.clone());
    let mut cancellation_polls = 0;

    let report = scanner
        .scan_with_observer(
            request(&fixture),
            || {
                cancellation_polls += 1;
                cancellation_polls > 2
            },
            |_| {},
        )
        .unwrap();

    assert!(report.cancelled);
    assert!(report.completion.observed_assets > 0);
    assert!(report.completion.observed_assets < 4);
    assert_eq!(tree_snapshot(&fixture.library_path), before);
    assert_eq!(
        catalog
            .timeline(fixture.binding.library_id, SortMode::CaptureDesc)
            .unwrap()
            .len(),
        report.completion.observed_assets
    );
    let connection = fixture.database.open_read_connection().unwrap();
    let run_state: String = connection
        .query_row(
            "SELECT run_state FROM scan_run ORDER BY started_at DESC LIMIT 1",
            (),
            |row| row.get(0),
        )
        .unwrap();
    let result: String = connection
        .query_row(
            "SELECT result FROM operation_report WHERE report_kind='Scan' ORDER BY started_at DESC LIMIT 1",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run_state, "Interrupted");
    assert_eq!(result, "Cancelled");
}

#[test]
fn changed_pair_member_propagates_versions_and_conservative_split() {
    let fixture = fixture();
    let jpeg = generated_jpeg(Some("2026:07:29 12:00:00"));
    let jpg_path = fixture.library_path.join("DSCF1.JPG");
    let raf_path = fixture.library_path.join("DSCF1.RAF");
    fs::write(&jpg_path, &jpeg).unwrap();
    fs::write(&raf_path, generated_raf(&jpeg)).unwrap();
    let catalog = Arc::new(CatalogService::new(fixture.database.clone()));
    let scanner = LibraryScanner::new(catalog.clone());
    scanner.scan(request(&fixture)).unwrap();
    let before = catalog_rows(&fixture.database);
    let jpg_before = before.get("dscf1.jpg").unwrap();
    let raf_before = before.get("dscf1.raf").unwrap();
    assert_eq!(jpg_before.2, raf_before.2);

    let conflicting = generated_raf(&generated_jpeg(Some("2026:07:30 12:00:00")));
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&raf_path)
        .unwrap();
    file.write_all(&conflicting).unwrap();
    file.sync_all().unwrap();
    drop(file);
    scanner.scan(request(&fixture)).unwrap();
    let after = catalog_rows(&fixture.database);
    let jpg_after = after.get("dscf1.jpg").unwrap();
    let raf_after = after.get("dscf1.raf").unwrap();
    assert_eq!(jpg_after.0, jpg_before.0);
    assert_eq!(raf_after.0, raf_before.0);
    assert!(raf_after.1 > raf_before.1);
    assert_ne!(jpg_after.2, raf_after.2);
    assert!(
        catalog
            .timeline(fixture.binding.library_id, SortMode::CaptureDesc)
            .unwrap()
            .len()
            == 2
    );
}

#[test]
fn hard_link_is_browsable_but_never_deletion_supported() {
    let fixture = fixture();
    let original = fixture.library_path.join("DSCF20.JPG");
    let linked = fixture.library_path.join("DSCF20-copy.JPG");
    fs::write(&original, generated_jpeg(Some("2026:07:29 12:00:00"))).unwrap();
    fs::hard_link(&original, &linked).unwrap();
    let catalog = Arc::new(CatalogService::new(fixture.database.clone()));
    LibraryScanner::new(catalog)
        .scan(request(&fixture))
        .unwrap();
    let connection = fixture.database.open_read_connection().unwrap();
    let row: (i64, i64) = connection
        .query_row(
            "SELECT deletion_supported, revision.hard_link_count FROM asset
         JOIN asset_revision revision
           ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
         WHERE asset.presence_state='Present' LIMIT 1",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row.0, 0);
    assert!(row.1 >= 2);
}

#[test]
fn viewing_cursor_uses_stable_anchor_when_current_photo_disappears() {
    let fixture = fixture();
    fs::write(
        fixture.library_path.join("NEW.JPG"),
        generated_jpeg(Some("2026:01:01 00:00:00")),
    )
    .unwrap();
    fs::write(
        fixture.library_path.join("OLD.JPG"),
        generated_jpeg(Some("2024:01:01 00:00:00")),
    )
    .unwrap();
    let ephemeral = tempfile::Builder::new()
        .prefix("ephemeral-photo-")
        .tempdir_in(&fixture.library_path)
        .unwrap();
    fs::write(
        ephemeral.path().join("MIDDLE.JPG"),
        generated_jpeg(Some("2025:01:01 00:00:00")),
    )
    .unwrap();
    let catalog = Arc::new(CatalogService::new(fixture.database.clone()));
    let scanner = LibraryScanner::new(catalog.clone());
    scanner.scan(request(&fixture)).unwrap();
    let timeline = catalog
        .timeline(fixture.binding.library_id, SortMode::CaptureDesc)
        .unwrap();
    assert_eq!(
        timeline
            .iter()
            .map(|photo| photo.natural_name_display.as_str())
            .collect::<Vec<_>>(),
        vec!["NEW.JPG", "MIDDLE.JPG", "OLD.JPG"]
    );
    catalog
        .save_viewing_cursor(
            fixture.binding.library_id,
            SortMode::CaptureDesc,
            timeline[1].photo_id,
        )
        .unwrap();
    drop(ephemeral);
    scanner.scan(request(&fixture)).unwrap();
    let restored = catalog
        .restore_viewing_cursor(fixture.binding.library_id, SortMode::CaptureDesc)
        .unwrap()
        .unwrap();
    let remaining = catalog
        .timeline(fixture.binding.library_id, SortMode::CaptureDesc)
        .unwrap();
    let restored_name = remaining
        .iter()
        .find(|photo| photo.photo_id == restored)
        .unwrap();
    assert_eq!(restored_name.natural_name_display, "OLD.JPG");
}

fn catalog_rows(database: &Database) -> BTreeMap<String, (Vec<u8>, i64, Vec<u8>, i64)> {
    let connection = database.open_read_connection().unwrap();
    let mut statement = connection.prepare(
        "SELECT revision.path_key, asset.asset_id, asset.current_version,
                relation.photo_id, photo.photo_version
         FROM asset JOIN asset_revision revision
           ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
         LEFT JOIN photo_asset relation ON relation.asset_id=asset.asset_id AND relation.to_photo_version IS NULL
         LEFT JOIN photo ON photo.photo_id=relation.photo_id
         WHERE asset.presence_state='Present' ORDER BY revision.path_key"
    ).unwrap();
    statement
        .query_map((), |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .map(|row| {
            let (path, asset, version, photo, photo_version) = row.unwrap();
            (
                String::from_utf8(path).unwrap(),
                (asset, version, photo, photo_version),
            )
        })
        .collect()
}
