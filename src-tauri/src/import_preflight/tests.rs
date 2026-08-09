use super::*;
use std::{collections::BTreeMap, fs, io::Read, sync::Arc};

use sha2::{Digest, Sha256};

use crate::{roots::FujiLibraryService, safety::TestRoot};

#[test]
fn supported_extensions_are_case_insensitive_and_limited_to_v1_formats() {
    assert!(supported_kind(Path::new("DSCF0001.JPG")).is_some());
    assert!(supported_kind(Path::new("DSCF0001.jpeg")).is_some());
    assert!(supported_kind(Path::new("DSCF0001.RaF")).is_some());
    assert!(supported_kind(Path::new("DSCF0001.png")).is_none());
    assert!(supported_kind(Path::new("DSCF0001.xmp")).is_none());
}

#[test]
fn windows_dangerous_names_are_rejected_before_planning() {
    assert!(validate_target_file_name("DSCF0001.JPG").is_ok());
    assert!(validate_target_file_name("CON.JPG").is_err());
    assert!(validate_target_file_name("bad:name.JPG").is_err());
    assert!(validate_target_file_name("trailing. ").is_err());
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    size: u64,
    modified: std::time::SystemTime,
    hash: [u8; 32],
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, TreeEntry> {
    fn visit(root: &Path, current: &Path, output: &mut BTreeMap<String, TreeEntry>) {
        let mut entries = fs::read_dir(current)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, output);
                continue;
            }
            let metadata = entry.metadata().unwrap();
            let mut file = fs::File::open(&path).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            output.insert(
                display_relative(path.strip_prefix(root).unwrap()),
                TreeEntry {
                    size: metadata.len(),
                    modified: metadata.modified().unwrap(),
                    hash: Sha256::digest(bytes).into(),
                },
            );
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

fn generated_jpeg(date: Option<&str>, payload: &[u8]) -> Vec<u8> {
    let mut jpeg = vec![0xff, 0xd8];
    if let Some(date) = date {
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
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend(tiff);
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
        jpeg.extend(exif);
    }
    jpeg.extend_from_slice(payload);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    jpeg
}

fn generated_raf(preview: &[u8], payload: &[u8]) -> Vec<u8> {
    let offset = 128_u32;
    let mut raf = vec![0_u8; offset as usize];
    raf[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    raf[84..88].copy_from_slice(&offset.to_be_bytes());
    raf[88..92].copy_from_slice(&u32::try_from(preview.len()).unwrap().to_be_bytes());
    raf.extend_from_slice(preview);
    raf.extend_from_slice(payload);
    raf
}

#[cfg(windows)]
#[test]
fn disposable_card_preflight_is_read_only_and_creates_no_job_generation_or_target_file() {
    use std::os::windows::fs::OpenOptionsExt;

    let app_parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(app_parent.path()).unwrap();
    let target = app_parent.path().join("disposable-phase-five-gallery");
    fs::create_dir(&target).unwrap();

    let card_parent = tempfile::Builder::new()
        .prefix("fpm-disposable-card-")
        .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
        .unwrap();
    let source = card_parent.path().join("DCIM").join("100_FUJI");
    fs::create_dir_all(source.join("raw")).unwrap();

    let source_probe = probe_directory(&source).unwrap();
    let target_probe = probe_directory(&target).unwrap();
    if source_probe.volume_identity_key == target_probe.volume_identity_key {
        // The safety integration requires two actual volumes. Planner/proof tests still run on
        // single-volume builders; this branch prevents weakening the production volume gate.
        return;
    }

    let jpg_one = generated_jpeg(Some("2026:07:29 10:11:12"), b"one-jpeg");
    let raf_one = generated_raf(
        &generated_jpeg(Some("2026:07:29 10:11:12"), b"one-preview"),
        b"one-raw",
    );
    fs::write(source.join("DSCF0001.JPG"), &jpg_one).unwrap();
    fs::write(source.join("raw").join("DSCF0001.RAF"), &raf_one).unwrap();

    let jpg_two = generated_jpeg(Some("2026:07:30 09:00:00"), b"two-jpeg");
    let raf_two = generated_raf(
        &generated_jpeg(Some("2026:07:30 09:00:00"), b"two-preview"),
        b"two-raw",
    );
    fs::write(source.join("DSCF0002.JPG"), &jpg_two).unwrap();
    fs::write(source.join("raw").join("DSCF0002.RAF"), &raf_two).unwrap();

    let unknown = generated_jpeg(Some("2026:02:31 09:00:00"), b"unknown-date");
    fs::write(source.join("DSCF0003.JPEG"), &unknown).unwrap();
    fs::write(source.join("ignored.PNG"), b"ignored").unwrap();
    fs::write(
        source.join("LOCKED.RAF"),
        b"readable only after lock release",
    )
    .unwrap();

    fs::create_dir_all(target.join("2026").join("20260729")).unwrap();
    fs::write(
        target.join("2026").join("20260729").join("DSCF0001.JPG"),
        &jpg_one,
    )
    .unwrap();
    let conflict_jpg_dir = target.join("2026").join("20260730");
    let conflict_raf_dir = conflict_jpg_dir.join("raw");
    fs::create_dir_all(&conflict_raf_dir).unwrap();
    for name in ["DSCF0002.JPG", "DSCF0002_2.JPG"] {
        fs::write(conflict_jpg_dir.join(name), format!("occupied-{name}")).unwrap();
    }
    for name in ["DSCF0002.RAF", "DSCF0002_2.RAF"] {
        fs::write(conflict_raf_dir.join(name), format!("occupied-{name}")).unwrap();
    }

    let source_before = snapshot_tree(&source);
    let target_before = snapshot_tree(&target);
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(source.join("LOCKED.RAF"))
        .unwrap();
    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let libraries = FujiLibraryService::new(database.clone(), app_root.path());
    libraries.bind(&target).unwrap();
    let service = ImportPreflightService::new(database.clone(), app_root.path());

    let setup = service
        .select_source(&service.test_permit(), &source)
        .unwrap();
    assert!(setup.can_start_preflight);
    seed_stale_import_history(&database, &source.join("DSCF0003.JPEG"), &unknown);
    let draft = service.run_preflight(&service.test_permit()).unwrap();
    drop(locked);

    assert_eq!(snapshot_tree(&source), source_before);
    assert_eq!(snapshot_tree(&target), target_before);
    assert_eq!(draft.io_audit.source_write_requests, 0);
    assert_eq!(draft.io_audit.target_write_requests, 0);
    assert!(draft.io_audit.source_full_hashes >= 5);
    assert!(draft.io_audit.source_path_reopens >= 1);
    assert!(draft.io_audit.target_path_reopens >= 1);
    assert_eq!(draft.summary.skipped_files, 1);
    assert_eq!(draft.summary.unreadable_files, 1);
    assert_eq!(draft.summary.unknown_date_files, 1);
    assert_eq!(draft.summary.conflict_groups, 1);
    assert!(draft.items.iter().any(|item| {
        item.file_name == "DSCF0001.RAF"
            && item.disposition == DraftDisposition::New
            && item.target_relative_path.as_deref() == Some("2026\\20260729\\raw\\DSCF0001.RAF")
    }));
    assert!(draft.items.iter().any(|item| {
        item.file_name == "DSCF0002.JPG"
            && item.disposition == DraftDisposition::Conflict
            && item.target_relative_path.as_deref() == Some("2026\\20260730\\DSCF0002_3.JPG")
    }));
    assert!(draft.items.iter().any(|item| {
        item.file_name == "DSCF0002.RAF"
            && item.disposition == DraftDisposition::Conflict
            && item.target_relative_path.as_deref() == Some("2026\\20260730\\raw\\DSCF0002_3.RAF")
    }));
    assert!(draft.items.iter().any(|item| {
        item.file_name == "DSCF0003.JPEG"
            && item.disposition == DraftDisposition::New
            && item.date_unknown
            && item.target_relative_path.as_deref() == Some("Unknown-Date\\DSCF0003.JPEG")
    }));

    let connection = database.open_read_connection().unwrap();
    let import_jobs: i64 = connection
        .query_row("SELECT COUNT(*) FROM import_job", (), |row| row.get(0))
        .unwrap();
    let generations: i64 = connection
        .query_row("SELECT COUNT(*) FROM import_plan_generation", (), |row| {
            row.get(0)
        })
        .unwrap();
    let profiles: i64 = connection
        .query_row("SELECT COUNT(*) FROM import_source_profile", (), |row| {
            row.get(0)
        })
        .unwrap();
    let deny_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM volume_policy WHERE deny_write=1",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (import_jobs, generations, profiles, deny_rows),
        (0, 0, 1, 1)
    );
    drop(connection);

    let restarted = ImportPreflightService::new(database.clone(), app_root.path());
    let prefilled = restarted.setup_snapshot().unwrap();
    assert_eq!(
        prefilled
            .source
            .as_ref()
            .map(|source| source.display_path.as_str()),
        Some(source.to_string_lossy().as_ref())
    );
    assert_eq!(
        restarted.status().unwrap().phase,
        ImportPreflightPhase::Idle
    );

    let error = libraries.inspect_candidate(&source).unwrap_err();
    assert_eq!(error.code, ErrorCode::VolumeWriteDenied);
}

#[cfg(windows)]
fn seed_stale_import_history(database: &Arc<Database>, source_path: &Path, source_bytes: &[u8]) {
    let connection = database.open_read_connection().unwrap();
    let (library_id, root_id, binding_id): (Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT library.library_id, root.root_id, binding.binding_id
             FROM current_library
             JOIN library ON library.library_id=current_library.library_id
             JOIN root ON root.library_id=library.library_id AND root.root_kind='FujiLibrary'
             JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
             WHERE current_library.slot=1",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let source_volume_id: Vec<u8> = connection
        .query_row(
            "SELECT volume_id FROM import_source_profile ORDER BY last_confirmed_at DESC LIMIT 1",
            (),
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    let source_handle = crate::roots::open_read_only(source_path).unwrap();
    let source_probe = crate::roots::probe_file(&source_handle).unwrap();
    let asset_id = Uuid::new_v4();
    let history_id = Uuid::new_v4();
    let stale_relative = "Unknown-Date\\STALE-DSCF0003.JPEG";
    let stale_path_key = path_key(Path::new(stale_relative));
    let source_hash: [u8; 32] = Sha256::digest(source_bytes).into();
    database
        .writer()
        .unwrap()
        .transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO asset(
                        asset_id, library_id, root_id, media_kind, current_version,
                        current_path_key, current_file_id, presence_state,
                        deletion_supported, created_at
                     ) VALUES (?1, ?2, ?3, 'Jpg', 1, ?4, ?5, 'Present', 1, ?6)",
                    rusqlite::params![
                        asset_id.as_bytes().as_slice(),
                        &library_id,
                        &root_id,
                        &stale_path_key,
                        b"stale-file-id".as_slice(),
                        now_micros(),
                    ],
                )
                .unwrap();
            transaction
                .execute(
                    "INSERT INTO asset_revision(
                        asset_id, asset_version, root_id, root_binding_id,
                        relative_path, path_key, path_key_version, file_id,
                        size_bytes, mtime_filetime, content_hash_algorithm,
                        content_hash, presence_state, hard_link_count, observed_at
                     ) VALUES (?1, 1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8,
                               'SHA-256', ?9, 'Present', 1, ?10)",
                    rusqlite::params![
                        asset_id.as_bytes().as_slice(),
                        &root_id,
                        &binding_id,
                        stale_relative,
                        &stale_path_key,
                        b"stale-file-id".as_slice(),
                        i64::try_from(source_probe.size_bytes).unwrap(),
                        source_probe.mtime_filetime,
                        source_hash.as_slice(),
                        now_micros(),
                    ],
                )
                .unwrap();
            transaction
                .execute(
                    "INSERT INTO import_history(
                        history_id, library_id, source_volume_id, source_file_id,
                        source_size_bytes, source_mtime, source_full_hash,
                        hash_algorithm, target_asset_id, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'SHA-256', ?8, ?9)",
                    rusqlite::params![
                        history_id.as_bytes().as_slice(),
                        &library_id,
                        &source_volume_id,
                        &source_probe.file_id,
                        i64::try_from(source_probe.size_bytes).unwrap(),
                        source_probe.mtime_filetime,
                        source_hash.as_slice(),
                        asset_id.as_bytes().as_slice(),
                        now_micros(),
                    ],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
}
