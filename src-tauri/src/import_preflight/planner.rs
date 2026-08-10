use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    catalog::{CandidateLookupInput, find_import_candidates},
    db::Database,
    domain::{DomainError, ErrorCode, LibraryId, RootId},
    roots::{path_is_reparse, path_key, resolve_contained_path},
};

use super::{
    DraftDisposition, DraftMediaKind, EnumeratedSource, ImportDraftItem, ImportPreflightDraft,
    ImportPreflightSummary, SourceContext, SourceFile, TargetContext, display_relative,
    proof::{AuditSide, BoundPath, ProcessIoAudit, StableReadHandle, prove_duplicate},
};

#[derive(Debug, Clone)]
struct WorkGroup {
    display_key: String,
    members: Vec<SourceFile>,
    capture_local_key: Option<String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    relative_path: PathBuf,
    suffix: Option<u32>,
}

#[derive(Debug, Clone)]
struct MemberAssessment {
    source: SourceFile,
    target_directory: PathBuf,
    duplicate: Option<Candidate>,
    occupied_suffixes: BTreeSet<u32>,
    warning: Option<String>,
}

pub(super) fn build_draft<F>(
    database: &Database,
    source: &SourceContext,
    target: &TargetContext,
    enumerated: EnumeratedSource,
    audit: &ProcessIoAudit,
    mut check_cancelled: F,
) -> Result<ImportPreflightDraft, DomainError>
where
    F: FnMut() -> Result<(), DomainError>,
{
    let mut items = Vec::new();
    let groups = build_groups(enumerated.files);
    let planned_groups = u64::try_from(groups.len()).unwrap_or(u64::MAX);
    let mut summary = ImportPreflightSummary {
        planned_groups,
        ..ImportPreflightSummary::default()
    };

    for group in groups {
        check_cancelled()?;
        let group_items = plan_group(database, source, target, group, audit, &mut check_cancelled)?;
        let has_new = group_items.iter().any(|item| {
            matches!(
                item.disposition,
                DraftDisposition::New | DraftDisposition::Conflict
            )
        });
        let has_conflict = group_items
            .iter()
            .any(|item| item.disposition == DraftDisposition::Conflict);
        if has_new {
            summary.new_groups += 1;
        }
        if has_conflict {
            summary.conflict_groups += 1;
        }
        for item in &group_items {
            if item.date_unknown {
                summary.unknown_date_files += 1;
            }
            match item.disposition {
                DraftDisposition::New | DraftDisposition::Conflict => {
                    summary.new_files += 1;
                    summary.new_bytes = summary.new_bytes.saturating_add(item.size_bytes);
                }
                DraftDisposition::Skip => summary.skipped_files += 1,
                DraftDisposition::Unreadable => summary.unreadable_files += 1,
            }
        }
        items.extend(group_items);
    }

    for unreadable in enumerated.unreadable {
        summary.unreadable_files += 1;
        items.push(ImportDraftItem {
            logical_group: unreadable
                .relative_path
                .parent()
                .map(display_relative)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "来源根目录".into()),
            file_name: unreadable.file_name,
            source_relative_path: display_relative(&unreadable.relative_path),
            media_kind: unreadable.kind,
            disposition: DraftDisposition::Unreadable,
            target_relative_path: None,
            size_bytes: 0,
            date_unknown: false,
            warning: Some(unreadable.reason),
        });
    }

    items.sort_by(|left, right| {
        left.source_relative_path
            .to_lowercase()
            .cmp(&right.source_relative_path.to_lowercase())
    });
    let draft_digest = digest_draft(&summary, &items);
    Ok(ImportPreflightDraft {
        source_path: source.root_path.to_string_lossy().into_owned(),
        target_path: target.root_path.to_string_lossy().into_owned(),
        summary,
        items,
        ignored_suffixes: enumerated.ignored_suffixes,
        draft_digest,
        io_audit: audit.snapshot(),
    })
}

fn build_groups(files: Vec<SourceFile>) -> Vec<WorkGroup> {
    let mut grouped: BTreeMap<String, Vec<SourceFile>> = BTreeMap::new();
    for file in files {
        let parent_path = file.relative_path.parent().unwrap_or_else(|| Path::new(""));
        let evidence_parent = if file.kind == DraftMediaKind::Raf
            && parent_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("raw"))
        {
            parent_path.parent().unwrap_or_else(|| Path::new(""))
        } else {
            parent_path
        };
        let parent = Some(evidence_parent)
            .map(|value| display_relative(value).to_lowercase())
            .unwrap_or_default();
        grouped
            .entry(format!("{parent}\0{}", file.stem.to_lowercase()))
            .or_default()
            .push(file);
    }
    let mut result = Vec::new();
    for (_key, mut members) in grouped {
        members.sort_by_key(|member| match member.kind {
            DraftMediaKind::Jpg => 0,
            DraftMediaKind::Raf => 1,
        });
        let jpg_count = members
            .iter()
            .filter(|member| member.kind == DraftMediaKind::Jpg)
            .count();
        let raf_count = members
            .iter()
            .filter(|member| member.kind == DraftMediaKind::Raf)
            .count();
        if members.len() == 2 && jpg_count == 1 && raf_count == 1 {
            let left_date = members[0].capture_local_key.clone();
            let right_date = members[1].capture_local_key.clone();
            let compatible = match (&left_date, &right_date) {
                (Some(left), Some(right)) => capture_day(left) == capture_day(right),
                _ => true,
            };
            if compatible {
                let capture_local_key = left_date.or(right_date);
                result.push(WorkGroup {
                    display_key: members[0].stem.clone(),
                    members,
                    capture_local_key,
                });
                continue;
            }
        }
        for member in members {
            let display_key = member.stem.clone();
            let capture_local_key = member.capture_local_key.clone();
            result.push(WorkGroup {
                display_key,
                members: vec![member],
                capture_local_key,
            });
        }
    }
    result
}

fn plan_group<F>(
    database: &Database,
    source_context: &SourceContext,
    target_context: &TargetContext,
    group: WorkGroup,
    audit: &ProcessIoAudit,
    check_cancelled: &mut F,
) -> Result<Vec<ImportDraftItem>, DomainError>
where
    F: FnMut() -> Result<(), DomainError>,
{
    let date_directory = date_directory(group.capture_local_key.as_deref());
    let date_unknown = group.capture_local_key.is_none();
    let mut assessments = Vec::new();
    for member in group.members {
        check_cancelled()?;
        let target_directory = match member.kind {
            DraftMediaKind::Jpg => date_directory.clone(),
            DraftMediaKind::Raf => date_directory.join("raw"),
        };
        assessments.push(assess_member(
            database,
            source_context,
            target_context,
            member,
            target_directory,
            audit,
            check_cancelled,
        )?);
    }

    let duplicate_suffixes = assessments
        .iter()
        .filter_map(|assessment| assessment.duplicate.as_ref()?.suffix)
        .collect::<BTreeSet<_>>();
    let duplicate_count = assessments
        .iter()
        .filter(|assessment| assessment.duplicate.is_some())
        .count();

    let chosen_suffix = if duplicate_suffixes.len() == 1 {
        let suffix = *duplicate_suffixes.first().unwrap();
        let compatible = assessments.iter().all(|assessment| {
            assessment
                .duplicate
                .as_ref()
                .is_some_and(|candidate| candidate.suffix == Some(suffix))
                || !assessment.occupied_suffixes.contains(&suffix)
        });
        compatible.then_some(suffix)
    } else if duplicate_count == 0 {
        Some(first_common_free_suffix(&assessments))
    } else {
        None
    };

    if chosen_suffix.is_none() && duplicate_count > 0 && assessments.len() > 1 {
        // Existing members cannot be bound to one consistent suffix. Split conservatively so a
        // duplicate is never copied and a new counterpart is never silently paired to it.
        let mut items = Vec::new();
        for assessment in assessments {
            items.push(plan_single_assessment(
                &group.display_key,
                assessment,
                date_unknown,
            ));
        }
        return Ok(items);
    }

    let suffix = chosen_suffix.unwrap_or_else(|| first_common_free_suffix(&assessments));
    let conflict = suffix > 1;
    Ok(assessments
        .into_iter()
        .map(|assessment| {
            let target_relative_path = if let Some(duplicate) = &assessment.duplicate {
                Some(display_relative(&duplicate.relative_path))
            } else {
                Some(display_relative(
                    &assessment
                        .target_directory
                        .join(planned_name(&assessment.source, suffix)),
                ))
            };
            ImportDraftItem {
                logical_group: group.display_key.clone(),
                file_name: assessment.source.file_name.clone(),
                source_relative_path: display_relative(&assessment.source.relative_path),
                media_kind: assessment.source.kind,
                disposition: if assessment.duplicate.is_some() {
                    DraftDisposition::Skip
                } else if conflict {
                    DraftDisposition::Conflict
                } else {
                    DraftDisposition::New
                },
                target_relative_path,
                size_bytes: assessment.source.size_bytes,
                date_unknown,
                warning: assessment.warning,
            }
        })
        .collect())
}

fn plan_single_assessment(
    group_key: &str,
    assessment: MemberAssessment,
    date_unknown: bool,
) -> ImportDraftItem {
    let (disposition, target, warning) = if let Some(duplicate) = assessment.duplicate {
        (
            DraftDisposition::Skip,
            duplicate.relative_path,
            assessment.warning,
        )
    } else {
        let suffix = first_free_suffix(&assessment.occupied_suffixes);
        let disposition = if suffix == 1 {
            DraftDisposition::New
        } else {
            DraftDisposition::Conflict
        };
        (
            disposition,
            assessment
                .target_directory
                .join(planned_name(&assessment.source, suffix)),
            assessment.warning,
        )
    };
    ImportDraftItem {
        logical_group: group_key.to_string(),
        file_name: assessment.source.file_name.clone(),
        source_relative_path: display_relative(&assessment.source.relative_path),
        media_kind: assessment.source.kind,
        disposition,
        target_relative_path: Some(display_relative(&target)),
        size_bytes: assessment.source.size_bytes,
        date_unknown,
        warning,
    }
}

fn assess_member<F>(
    database: &Database,
    source_context: &SourceContext,
    target_context: &TargetContext,
    source: SourceFile,
    target_directory: PathBuf,
    audit: &ProcessIoAudit,
    check_cancelled: &mut F,
) -> Result<MemberAssessment, DomainError>
where
    F: FnMut() -> Result<(), DomainError>,
{
    let mut candidates = history_candidates(database, source_context, target_context, &source)?;
    candidates.extend(catalog_candidates(database, target_context, &source)?);
    let family =
        enumerate_complete_suffix_family(target_context, &target_directory, &source, audit)?;
    let occupied_suffixes = family
        .iter()
        .filter_map(|candidate| candidate.suffix)
        .collect::<BTreeSet<_>>();
    candidates.extend(family);
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(path_key(&candidate.relative_path)));

    let source_bound = BoundPath {
        root_id: source_context.root_id.clone(),
        binding_id: source_context.binding_id.clone(),
        root_path: source_context.root_path.clone(),
        relative_path: source.relative_path.clone(),
        expected_root: source_context.root_probe.clone(),
    };
    let mut duplicate = None;
    let mut warning = None;
    for candidate in candidates {
        check_cancelled()?;
        let target_path =
            match resolve_contained_path(&target_context.root_path, &candidate.relative_path) {
                Ok(value) if value.is_file() => value,
                _ => continue,
            };
        if path_is_reparse(&target_path)? {
            warning.get_or_insert_with(|| "同名候选是 reparse point，未用作重复证明。".to_string());
            continue;
        }
        let target_bound = BoundPath {
            root_id: target_context.root_id.clone(),
            binding_id: target_context.binding_id.clone(),
            root_path: target_context.root_path.clone(),
            relative_path: candidate.relative_path.clone(),
            expected_root: target_context.root_probe.clone(),
        };
        match prove_duplicate(&source_bound, &target_bound, audit) {
            Ok(Some(proof)) if proof_is_complete(&proof) => {
                duplicate = Some(candidate);
                break;
            }
            Ok(_) => {}
            Err(_) => {
                warning.get_or_insert_with(|| {
                    "目标候选在比较期间变化或无法稳定读取，未判定为已存在。".to_string()
                });
            }
        }
    }

    // A target-side race may be handled as occupied, but a source-side race invalidates this
    // item. Reopen the planned source path and compare it with the full-read snapshot.
    let source_path = source_bound.resolve()?;
    let source_reopened = StableReadHandle::open(&source_path, AuditSide::Source, audit)?;
    let current_source = source_reopened.probe(audit)?;
    if current_source != source.identity {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "来源文件在计划期间发生变化，请重新开始只读预检。",
        ));
    }

    Ok(MemberAssessment {
        source,
        target_directory,
        duplicate,
        occupied_suffixes,
        warning,
    })
}

fn catalog_candidates(
    database: &Database,
    target_context: &TargetContext,
    source: &SourceFile,
) -> Result<Vec<Candidate>, DomainError> {
    let library_id = LibraryId::from_uuid(crate::catalog::uuid_from(&target_context.library_id)?);
    let root_id = RootId::from_uuid(crate::catalog::uuid_from(&target_context.root_id)?);
    find_import_candidates(
        database,
        CandidateLookupInput {
            library_id,
            root_id,
            original_filename: source.file_name.clone(),
            size_bytes: i64::try_from(source.size_bytes).unwrap_or(i64::MAX),
            capture_local_key: source.capture_local_key.clone(),
        },
    )
    .map(|rows| {
        rows.into_iter()
            .map(|row| {
                let relative_path = PathBuf::from(row.relative_path);
                Candidate {
                    suffix: suffix_for_name(
                        relative_path
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default(),
                        &source.stem,
                        &source.extension,
                    ),
                    relative_path,
                }
            })
            .collect()
    })
}

fn history_candidates(
    database: &Database,
    source_context: &SourceContext,
    target_context: &TargetContext,
    source: &SourceFile,
) -> Result<Vec<Candidate>, DomainError> {
    let connection = database.open_read_connection()?;
    let mut statement = connection
        .prepare(
            "SELECT revision.relative_path
             FROM import_history AS history
             JOIN asset ON asset.asset_id=history.target_asset_id
             JOIN asset_revision AS revision
               ON revision.asset_id=asset.asset_id
              AND revision.asset_version=asset.current_version
             WHERE history.library_id=?1
               AND history.source_volume_id=?2
               AND asset.root_id=?3
               AND asset.presence_state='Present'
               AND revision.presence_state='Present'
               AND history.source_size_bytes=?4
               AND (
                    (history.source_file_id IS NOT NULL AND history.source_file_id=?5)
                    OR history.source_full_hash=?6
               )
             ORDER BY CASE WHEN history.source_file_id=?5 THEN 0 ELSE 1 END,
                      history.created_at DESC",
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    let rows = statement
        .query_map(
            rusqlite::params![
                &target_context.library_id,
                &source_context.volume_id,
                &target_context.root_id,
                i64::try_from(source.size_bytes).unwrap_or(i64::MAX),
                &source.identity.file_id,
                source.full_hash.as_slice(),
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    let mut candidates = Vec::new();
    for row in rows {
        let relative_path = PathBuf::from(row.map_err(crate::db::migration::map_sqlite_error)?);
        candidates.push(Candidate {
            suffix: suffix_for_name(
                relative_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default(),
                &source.stem,
                &source.extension,
            ),
            relative_path,
        });
    }
    Ok(candidates)
}

fn enumerate_complete_suffix_family(
    target_context: &TargetContext,
    target_directory: &Path,
    source: &SourceFile,
    audit: &ProcessIoAudit,
) -> Result<Vec<Candidate>, DomainError> {
    let directory = resolve_contained_path(&target_context.root_path, target_directory)?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !directory.is_dir() || path_is_reparse(&directory)? {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "计划目标目录不是可安全枚举的普通目录。",
        ));
    }
    audit.enumerate(AuditSide::Target);
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&directory).map_err(target_io_error)? {
        let entry = entry.map_err(target_io_error)?;
        if !entry.file_type().map_err(target_io_error)?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(suffix) = suffix_for_name(&name, &source.stem, &source.extension) else {
            continue;
        };
        candidates.push(Candidate {
            relative_path: target_directory.join(name),
            suffix: Some(suffix),
        });
    }
    candidates.sort_by_key(|candidate| candidate.suffix.unwrap_or(u32::MAX));
    Ok(candidates)
}

fn suffix_for_name(file_name: &str, source_stem: &str, source_extension: &str) -> Option<u32> {
    let path = Path::new(file_name);
    let extension = path.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case(source_extension) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.eq_ignore_ascii_case(source_stem) {
        return Some(1);
    }
    let prefix = format!("{source_stem}_");
    if stem.len() <= prefix.len() || !stem[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        return None;
    }
    let suffix = stem[prefix.len()..].parse::<u32>().ok()?;
    (suffix >= 2 && !stem[prefix.len()..].starts_with('0')).then_some(suffix)
}

fn first_common_free_suffix(assessments: &[MemberAssessment]) -> u32 {
    let mut suffix = 1_u32;
    while assessments
        .iter()
        .any(|assessment| assessment.occupied_suffixes.contains(&suffix))
    {
        suffix = suffix.saturating_add(1);
        if suffix == u32::MAX {
            break;
        }
    }
    suffix
}

fn first_free_suffix(occupied: &BTreeSet<u32>) -> u32 {
    let mut suffix = 1_u32;
    while occupied.contains(&suffix) {
        suffix = suffix.saturating_add(1);
    }
    suffix
}

fn planned_name(source: &SourceFile, suffix: u32) -> String {
    if suffix == 1 {
        source.file_name.clone()
    } else {
        format!("{}_{suffix}.{}", source.stem, source.extension)
    }
}

fn date_directory(capture_local_key: Option<&str>) -> PathBuf {
    let Some(day) = capture_local_key.and_then(capture_day) else {
        return PathBuf::from("Unknown-Date");
    };
    PathBuf::from(&day[..4]).join(day.replace('-', ""))
}

fn capture_day(value: &str) -> Option<String> {
    let day = value.get(..10)?;
    (day.as_bytes().get(4) == Some(&b'-') && day.as_bytes().get(7) == Some(&b'-'))
        .then(|| day.to_string())
}

fn proof_is_complete(proof: &super::proof::DuplicateProof) -> bool {
    proof.source_hash == proof.target_hash
        && proof.source_before == proof.source_after
        && proof.target_before == proof.target_after
        && proof.source_before.file_id.is_some()
        && proof.target_before.file_id.is_some()
}

fn digest_draft(summary: &ImportPreflightSummary, items: &[ImportDraftItem]) -> String {
    let mut digest = Sha256::new();
    digest.update(format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        summary.planned_groups,
        summary.new_groups,
        summary.new_files,
        summary.new_bytes,
        summary.skipped_files,
        summary.conflict_groups,
        summary.unknown_date_files,
        summary.unreadable_files,
    ));
    for item in items {
        digest.update(item.logical_group.as_bytes());
        digest.update([0]);
        digest.update(item.source_relative_path.as_bytes());
        digest.update([item.media_kind as u8, item.disposition as u8]);
        digest.update(
            item.target_relative_path
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        );
        digest.update(item.size_bytes.to_le_bytes());
        digest.update([u8::from(item.date_unknown)]);
    }
    hex::encode(digest.finalize())
}

fn target_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::RootOffline,
        "无法只读枚举 Fuji 图库中的目标名称族。",
    )
    .with_diagnostic(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::FileProbe;

    fn source(path: &str, kind: DraftMediaKind, date: Option<&str>) -> SourceFile {
        let path = PathBuf::from(path);
        SourceFile {
            relative_path: path.clone(),
            file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            stem: path.file_stem().unwrap().to_string_lossy().into_owned(),
            extension: path.extension().unwrap().to_string_lossy().into_owned(),
            kind,
            size_bytes: 10,
            capture_local_key: date.map(str::to_string),
            inspection: crate::media::MediaInspection {
                exif: crate::media::ExifMetadata {
                    capture_local_key: date.map(str::to_string),
                    ..crate::media::ExifMetadata::default()
                },
                preview: match kind {
                    DraftMediaKind::Jpg => crate::media::PreviewState::JpegSource,
                    DraftMediaKind::Raf => crate::media::PreviewState::Unavailable,
                },
            },
            full_hash: [1; 32],
            identity: FileProbe {
                file_id: Some(vec![1]),
                size_bytes: 10,
                mtime_filetime: 1,
                hard_link_count: 1,
                reparse_point: false,
            },
        }
    }

    #[test]
    fn jpg_raf_group_matrix_covers_raw_child_single_date_unknown_and_conflict() {
        let same = build_groups(vec![
            source(
                "DSCF0001.JPG",
                DraftMediaKind::Jpg,
                Some("2026-07-29T10:00:00"),
            ),
            source(
                "raw\\DSCF0001.RAF",
                DraftMediaKind::Raf,
                Some("2026-07-29T10:00:01"),
            ),
        ]);
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].members.len(), 2);
        assert_eq!(
            date_directory(same[0].capture_local_key.as_deref()),
            PathBuf::from("2026").join("20260729")
        );

        let single_date = build_groups(vec![
            source(
                "DSCF0002.JPG",
                DraftMediaKind::Jpg,
                Some("2026-07-30T10:00:00"),
            ),
            source("raw\\DSCF0002.RAF", DraftMediaKind::Raf, None),
        ]);
        assert_eq!(single_date.len(), 1);
        assert_eq!(single_date[0].members.len(), 2);
        assert_eq!(
            single_date[0].capture_local_key.as_deref(),
            Some("2026-07-30T10:00:00")
        );

        let unknown = build_groups(vec![
            source("DSCF0003.JPG", DraftMediaKind::Jpg, None),
            source("raw\\DSCF0003.RAF", DraftMediaKind::Raf, None),
        ]);
        assert_eq!(unknown.len(), 1);
        assert_eq!(date_directory(None), PathBuf::from("Unknown-Date"));

        let conflict = build_groups(vec![
            source(
                "DSCF0004.JPG",
                DraftMediaKind::Jpg,
                Some("2026-07-30T10:00:00"),
            ),
            source(
                "raw\\DSCF0004.RAF",
                DraftMediaKind::Raf,
                Some("2026-07-31T10:00:00"),
            ),
        ]);
        assert_eq!(conflict.len(), 2);
        assert!(conflict.iter().all(|group| group.members.len() == 1));
    }

    #[test]
    fn complete_suffix_parser_is_strict_and_preserves_original_extension_case() {
        assert_eq!(suffix_for_name("DSCF0001.JPG", "DSCF0001", "JPG"), Some(1));
        assert_eq!(
            suffix_for_name("dscf0001_2.jpg", "DSCF0001", "JPG"),
            Some(2)
        );
        assert_eq!(suffix_for_name("DSCF0001_02.JPG", "DSCF0001", "JPG"), None);
        assert_eq!(suffix_for_name("DSCF0001_3.JPEG", "DSCF0001", "JPG"), None);
        let file = source("DSCF0001.JpG", DraftMediaKind::Jpg, None);
        assert_eq!(planned_name(&file, 7), "DSCF0001_7.JpG");
    }
}
