use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::OptionalExtension;

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode},
    io_scheduler::IoScheduler,
    roots::FileProbe,
};

use super::proof::{StableHashObservation, stable_byte_equal, stable_hash};

#[derive(Debug, Clone)]
struct AssetRow {
    asset_id: Vec<u8>,
    asset_version: i64,
    role: String,
    relative_path: String,
    path_key: Vec<u8>,
    expected: FileProbe,
}

#[derive(Debug, Clone)]
struct RunContext {
    run_id: Vec<u8>,
    library_id: Vec<u8>,
    root_id: Vec<u8>,
    root_binding_id: Vec<u8>,
    volume_id: Vec<u8>,
    root_path: PathBuf,
    catalog_revision: i64,
}

type BeginRunRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String, i64);

#[derive(Debug, Clone)]
struct HashedAsset {
    asset: AssetRow,
    observation: StableHashObservation,
}

#[derive(Debug, Clone)]
struct EquivalenceClass {
    role: String,
    size_bytes: i64,
    hash: [u8; 32],
    members: Vec<HashedAsset>,
}

pub(super) fn execute(
    database: &Arc<Database>,
    scheduler: &Arc<IoScheduler>,
    cancelled: &AtomicBool,
) -> Result<(), DomainError> {
    cancelled.store(false, Ordering::Release);
    let _bulk_io = scheduler.acquire_bulk_io(|| cancelled.load(Ordering::Acquire))?;
    let context = begin_run(database)?;
    match execute_inner(database, scheduler, cancelled, &context) {
        Ok(()) => Ok(()),
        Err(_error) if cancelled.load(Ordering::Acquire) => {
            finish_cancelled(database, &context.run_id)?;
            Ok(())
        }
        Err(error) => {
            finish_failed(database, &context.run_id, &format!("{:?}", error.code))?;
            Err(error)
        }
    }
}

fn execute_inner(
    database: &Arc<Database>,
    scheduler: &Arc<IoScheduler>,
    cancelled: &AtomicBool,
    context: &RunContext,
) -> Result<(), DomainError> {
    let assets = load_assets(database, context)?;
    let mut buckets: BTreeMap<(String, i64), Vec<AssetRow>> = BTreeMap::new();
    for asset in assets {
        buckets
            .entry((
                asset.role.clone(),
                i64::try_from(asset.expected.size_bytes).unwrap_or(i64::MAX),
            ))
            .or_default()
            .push(asset);
    }
    let candidate_files = buckets
        .values()
        .filter(|members| members.len() > 1)
        .map(Vec::len)
        .sum::<usize>();
    update_progress(
        database,
        &context.run_id,
        "Hashing",
        0,
        candidate_files,
        0,
        None,
    )?;

    let mut hashed = Vec::new();
    let mut checked = 0_usize;
    for members in buckets.into_values().filter(|members| members.len() > 1) {
        for asset in members {
            if cancelled.load(Ordering::Acquire) {
                return Err(cancelled_error());
            }
            checked += 1;
            let current = asset.relative_path.clone();
            if let Ok(observation) = stable_hash(
                scheduler,
                cancelled,
                &context.root_path,
                &asset.relative_path,
                &asset.expected,
            ) {
                hashed.push(HashedAsset { asset, observation });
            }
            update_progress(
                database,
                &context.run_id,
                "Hashing",
                checked,
                candidate_files,
                hashed.len(),
                Some(&current),
            )?;
        }
    }

    update_progress(
        database,
        &context.run_id,
        "Comparing",
        checked,
        candidate_files,
        hashed.len(),
        None,
    )?;
    let mut hash_groups: BTreeMap<(String, i64, [u8; 32]), Vec<HashedAsset>> = BTreeMap::new();
    for member in hashed {
        hash_groups
            .entry((
                member.asset.role.clone(),
                i64::try_from(member.asset.expected.size_bytes).unwrap_or(i64::MAX),
                member.observation.hash,
            ))
            .or_default()
            .push(member);
    }
    let mut classes = Vec::new();
    for ((role, size_bytes, hash), group) in hash_groups {
        if group.len() < 2 {
            continue;
        }
        let mut partitions: Vec<Vec<HashedAsset>> = Vec::new();
        for member in group {
            let mut member = Some(member);
            for partition in &mut partitions {
                let anchor = &partition[0];
                let candidate = member.as_ref().expect("member is consumed once");
                match stable_byte_equal(
                    scheduler,
                    cancelled,
                    &context.root_path,
                    &anchor.asset.relative_path,
                    &anchor.asset.expected,
                    &candidate.asset.relative_path,
                    &candidate.asset.expected,
                ) {
                    Ok(true) => {
                        partition.push(member.take().expect("member is present"));
                        break;
                    }
                    Ok(false) => {}
                    Err(error) if cancelled.load(Ordering::Acquire) => return Err(error),
                    Err(_) => {}
                }
            }
            if let Some(member) = member {
                partitions.push(vec![member]);
            }
        }
        for partition in partitions.into_iter().filter(|members| members.len() > 1) {
            let mut stable_members = Vec::new();
            for mut member in partition {
                let refreshed = stable_hash(
                    scheduler,
                    cancelled,
                    &context.root_path,
                    &member.asset.relative_path,
                    &member.asset.expected,
                );
                if let Ok(observation) = refreshed {
                    if observation.hash == hash {
                        member.observation = observation;
                        stable_members.push(member);
                    }
                }
            }
            if stable_members.len() > 1 {
                classes.push(EquivalenceClass {
                    role: role.clone(),
                    size_bytes,
                    hash,
                    members: stable_members,
                });
            }
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(cancelled_error());
    }
    publish(database, context, checked, candidate_files, classes)
}

fn begin_run(database: &Arc<Database>) -> Result<RunContext, DomainError> {
    let connection = database.open_read_connection()?;
    let row: Option<BeginRunRow> = connection
        .query_row(
            "SELECT library.library_id, root.root_id, binding.binding_id, binding.volume_id,
                    binding.absolute_path_display, COALESCE(revision.catalog_revision, 0)
             FROM current_library AS current
             JOIN library ON library.library_id=current.library_id AND library.lifecycle='Active'
             JOIN root ON root.library_id=library.library_id
                AND root.root_kind='FujiLibrary' AND root.lifecycle='Online'
             JOIN root_binding AS binding ON binding.binding_id=root.current_binding_id
                AND binding.binding_state='Active'
             LEFT JOIN library_catalog_revision AS revision
                ON revision.library_id=library.library_id
             WHERE current.slot=1",
            (),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let (library_id, root_id, root_binding_id, volume_id, root_path, catalog_revision) =
        row.ok_or_else(|| DomainError::new(ErrorCode::RootOffline, "当前没有在线 Fuji 图库。"))?;
    let running: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM duplicate_scan_run WHERE run_state='Running')",
            (),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if running {
        return Err(DomainError::new(
            ErrorCode::InvalidTransition,
            "已有重复检测正在运行。",
        ));
    }
    drop(connection);
    let run_id = uuid::Uuid::new_v4().as_bytes().to_vec();
    let run_for_tx = run_id.clone();
    let library_for_tx = library_id.clone();
    let binding_for_tx = root_binding_id.clone();
    database.writer()?.transaction(move |transaction| {
        transaction
            .execute(
                "INSERT INTO duplicate_scan_run(
                    run_id, library_id, root_binding_id, catalog_revision,
                    run_state, phase, started_at
                 ) VALUES (?1, ?2, ?3, ?4, 'Running', 'Enumerating', ?5)",
                (
                    &run_for_tx,
                    &library_for_tx,
                    &binding_for_tx,
                    catalog_revision,
                    now_micros(),
                ),
            )
            .map_err(map_sqlite_error)?;
        Ok(())
    })?;
    Ok(RunContext {
        run_id,
        library_id,
        root_id,
        root_binding_id,
        volume_id,
        root_path: PathBuf::from(root_path),
        catalog_revision,
    })
}

fn load_assets(
    database: &Arc<Database>,
    context: &RunContext,
) -> Result<Vec<AssetRow>, DomainError> {
    let connection = database.open_read_connection()?;
    let mut statement = connection
        .prepare(
            "SELECT asset.asset_id, asset.current_version, asset.media_kind,
                    revision.relative_path, revision.path_key, revision.file_id,
                    revision.size_bytes, revision.mtime_filetime, revision.hard_link_count,
                    metadata.capture_local_key, metadata.camera_model, metadata.camera_serial
             FROM asset
             JOIN asset_revision AS revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             LEFT JOIN asset_media_metadata AS metadata
               ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
             WHERE asset.library_id=?1 AND asset.root_id=?2
               AND asset.presence_state='Present' AND revision.presence_state='Present'
               AND revision.root_binding_id=?3
             ORDER BY asset.media_kind, revision.size_bytes, revision.path_key",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(
            (
                &context.library_id,
                &context.root_id,
                &context.root_binding_id,
            ),
            |row| {
                let size = row.get::<_, i64>(6)?;
                Ok(AssetRow {
                    asset_id: row.get(0)?,
                    asset_version: row.get(1)?,
                    role: row.get(2)?,
                    relative_path: row.get(3)?,
                    path_key: row.get(4)?,
                    expected: FileProbe {
                        file_id: row.get(5)?,
                        size_bytes: u64::try_from(size).unwrap_or(0),
                        mtime_filetime: row.get(7)?,
                        hard_link_count: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(1),
                        reparse_point: false,
                    },
                })
            },
        )
        .map_err(map_sqlite_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)
}

fn publish(
    database: &Arc<Database>,
    context: &RunContext,
    checked: usize,
    candidates: usize,
    classes: Vec<EquivalenceClass>,
) -> Result<(), DomainError> {
    let context = context.clone();
    database.writer()?.transaction(move |transaction| {
        let current: Option<(Vec<u8>, i64)> = transaction
            .query_row(
                "SELECT root_binding_id, catalog_revision FROM library_catalog_revision
                 WHERE library_id=?1",
                [&context.library_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sqlite_error)?;
        if current.as_ref() != Some(&(context.root_binding_id.clone(), context.catalog_revision)) {
            return Err(DomainError::new(
                ErrorCode::StaleVersion,
                "重复检测完成前图库目录已变化，结果未发布。",
            ));
        }
        for equivalent in &classes {
            let set_id = uuid::Uuid::new_v4().as_bytes().to_vec();
            let mut observations = HashMap::new();
            for member in &equivalent.members {
                let before = insert_snapshot(transaction, &context, member, true)?;
                let after = insert_snapshot(transaction, &context, member, false)?;
                let observation_id = uuid::Uuid::new_v4().as_bytes().to_vec();
                transaction
                    .execute(
                        "INSERT INTO duplicate_file_observation(
                            observation_id, run_id, asset_id, asset_version, role,
                            before_snapshot_id, after_snapshot_id, full_hash, observed_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            &observation_id,
                            &context.run_id,
                            &member.asset.asset_id,
                            member.asset.asset_version,
                            &member.asset.role,
                            &before,
                            &after,
                            member.observation.hash.as_slice(),
                            now_micros(),
                        ],
                    )
                    .map_err(map_sqlite_error)?;
                observations.insert(member.asset.asset_id.clone(), (observation_id, after));
            }
            transaction
                .execute(
                    "INSERT INTO duplicate_equivalence_set(
                        set_id, run_id, role, content_hash, member_count,
                        file_size_bytes, total_bytes, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        &set_id,
                        &context.run_id,
                        &equivalent.role,
                        equivalent.hash.as_slice(),
                        i64::try_from(equivalent.members.len()).unwrap_or(i64::MAX),
                        equivalent.size_bytes,
                        equivalent.size_bytes.saturating_mul(
                            i64::try_from(equivalent.members.len()).unwrap_or(i64::MAX)
                        ),
                        now_micros(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            let anchor = &equivalent.members[0];
            let (anchor_observation, anchor_snapshot) = observations
                .get(&anchor.asset.asset_id)
                .expect("anchor observation was inserted");
            for member in equivalent.members.iter().skip(1) {
                let (member_observation, member_snapshot) = observations
                    .get(&member.asset.asset_id)
                    .expect("member observation was inserted");
                transaction
                    .execute(
                        "INSERT INTO duplicate_equivalence_proof(
                            proof_id, run_id, set_id, anchor_observation_id,
                            member_observation_id, anchor_snapshot_id,
                            member_snapshot_id, byte_equal, created_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
                        (
                            uuid::Uuid::new_v4().as_bytes().as_slice(),
                            &context.run_id,
                            &set_id,
                            anchor_observation,
                            member_observation,
                            anchor_snapshot,
                            member_snapshot,
                            now_micros(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
            }
        }
        transaction
            .execute(
                "UPDATE duplicate_scan_run SET run_state='Completed', phase='Finished',
                        checked_files=?1, candidate_files=?2, hashed_files=?3,
                        current_relative_path=NULL, completed_at=?4
                 WHERE run_id=?5 AND run_state='Running'",
                (
                    i64::try_from(checked).unwrap_or(i64::MAX),
                    i64::try_from(candidates).unwrap_or(i64::MAX),
                    classes
                        .iter()
                        .map(|class| class.members.len())
                        .sum::<usize>() as i64,
                    now_micros(),
                    &context.run_id,
                ),
            )
            .map_err(map_sqlite_error)?;
        Ok(())
    })
}

fn insert_snapshot(
    transaction: &rusqlite::Transaction<'_>,
    context: &RunContext,
    member: &HashedAsset,
    before: bool,
) -> Result<Vec<u8>, DomainError> {
    let probe = if before {
        &member.observation.before
    } else {
        &member.observation.after
    };
    let snapshot = uuid::Uuid::new_v4().as_bytes().to_vec();
    transaction
        .execute(
            "INSERT INTO file_identity_snapshot(
                snapshot_id, root_id, root_binding_id, volume_id, relative_path,
                path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                content_hash_algorithm, content_hash, captured_at, evidence_level
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9,
                       'SHA-256', ?10, ?11, 'Strong')",
            rusqlite::params![
                &snapshot,
                &context.root_id,
                &context.root_binding_id,
                &context.volume_id,
                &member.asset.relative_path,
                &member.asset.path_key,
                &probe.file_id,
                i64::try_from(probe.size_bytes).unwrap_or(i64::MAX),
                probe.mtime_filetime,
                member.observation.hash.as_slice(),
                now_micros(),
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(snapshot)
}

fn update_progress(
    database: &Arc<Database>,
    run_id: &[u8],
    phase: &str,
    checked: usize,
    candidates: usize,
    hashed: usize,
    current: Option<&str>,
) -> Result<(), DomainError> {
    let run_id = run_id.to_vec();
    let phase = phase.to_string();
    let current = current.map(str::to_string);
    database.writer()?.transaction(move |transaction| {
        transaction
            .execute(
                "UPDATE duplicate_scan_run SET phase=?1, checked_files=?2,
                        candidate_files=?3, hashed_files=?4, current_relative_path=?5
                 WHERE run_id=?6 AND run_state='Running'",
                rusqlite::params![
                    phase,
                    i64::try_from(checked).unwrap_or(i64::MAX),
                    i64::try_from(candidates).unwrap_or(i64::MAX),
                    i64::try_from(hashed).unwrap_or(i64::MAX),
                    current,
                    run_id,
                ],
            )
            .map_err(map_sqlite_error)?;
        Ok(())
    })
}

fn finish_cancelled(database: &Arc<Database>, run_id: &[u8]) -> Result<(), DomainError> {
    finish(database, run_id, "Cancelled", None)
}

fn finish_failed(database: &Arc<Database>, run_id: &[u8], code: &str) -> Result<(), DomainError> {
    finish(database, run_id, "Failed", Some(code))
}

fn finish(
    database: &Arc<Database>,
    run_id: &[u8],
    state: &str,
    error_code: Option<&str>,
) -> Result<(), DomainError> {
    let run_id = run_id.to_vec();
    let state = state.to_string();
    let error_code = error_code.map(str::to_string);
    database.writer()?.transaction(move |transaction| {
        transaction
            .execute(
                "UPDATE duplicate_scan_run SET run_state=?1, phase='Finished',
                        error_code=?2, completed_at=?3, current_relative_path=NULL
                 WHERE run_id=?4 AND run_state='Running'",
                (state, error_code, now_micros(), run_id),
            )
            .map_err(map_sqlite_error)?;
        Ok(())
    })
}

fn cancelled_error() -> DomainError {
    DomainError::new(ErrorCode::InvalidTransition, "重复检测已安全取消。")
}
