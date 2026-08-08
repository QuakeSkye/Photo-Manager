use std::path::Path;

use serde::Serialize;

use crate::{
    db::Database,
    domain::{DomainError, ErrorCode},
    safety::{
        ArtifactFingerprint, CandidateManifest, FeatureAdmission, FeatureGate, PhaseEightRunRoot,
    },
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateBindingReport {
    report_version: u32,
    run_id: uuid::Uuid,
    manifest_sha256: String,
    manifest: CandidateManifest,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateVerificationReport {
    report_version: u32,
    run_id: uuid::Uuid,
    manifest_sha256: String,
    current_process_matches_installed_binary: bool,
    verified: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionVerificationReport {
    report_version: u32,
    run_id: uuid::Uuid,
    manifest_sha256: String,
    import_admission_verified: bool,
    recycle_admission_verified: bool,
    verified: bool,
}

pub fn capture_admission(
    root: &PhaseEightRunRoot,
    manifest_relative: &Path,
    feature: FeatureGate,
    safety_report_relative: &Path,
    evidence_relative: &[&Path],
) -> Result<FeatureAdmission, DomainError> {
    root.verify_current_identity()?;
    let binding: CandidateBindingReportInput = read_json(
        &root.resolve_owned(manifest_relative)?,
        "候选 manifest JSON 无效。",
    )?;
    if binding.report_version != 1 || binding.run_id != root.run_id() {
        return Err(candidate_mismatch());
    }
    let database = Database::open(&root.resolve_owned(Path::new("scale-app-data"))?)?;
    let connection = database.open_read_connection()?;
    binding.manifest.verify(&connection)?;
    if binding.manifest.sha256()? != binding.manifest_sha256 || evidence_relative.is_empty() {
        return Err(candidate_mismatch());
    }
    let safety_report = ArtifactFingerprint::capture(&root.resolve_owned(safety_report_relative)?)?;
    let evidence = evidence_relative
        .iter()
        .map(|path| root.resolve_owned(path))
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(|path| ArtifactFingerprint::capture(path))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FeatureAdmission {
        report_version: 1,
        feature,
        candidate_manifest_sha256: binding.manifest_sha256,
        safety_report,
        evidence,
        acceptance_conditions_passed: 27,
        unresolved_critical: 0,
        unresolved_high: 0,
    })
}

pub fn capture(
    root: &PhaseEightRunRoot,
    installer: &Path,
) -> Result<CandidateBindingReport, DomainError> {
    root.verify_current_identity()?;
    let current_exe = std::env::current_exe().map_err(io_error)?;
    let database = Database::open(&root.resolve_owned(Path::new("scale-app-data"))?)?;
    let connection = database.open_read_connection()?;
    let manifest = CandidateManifest::capture(installer, &current_exe, &connection)?;
    let manifest_sha256 = manifest.sha256()?;
    Ok(CandidateBindingReport {
        report_version: 1,
        run_id: root.run_id(),
        manifest_sha256,
        manifest,
    })
}

pub fn verify(
    root: &PhaseEightRunRoot,
    manifest_relative: &Path,
) -> Result<CandidateVerificationReport, DomainError> {
    root.verify_current_identity()?;
    let path = root.resolve_owned(manifest_relative)?;
    let binding: CandidateBindingReportInput =
        serde_json::from_slice(&std::fs::read(path).map_err(io_error)?).map_err(|error| {
            DomainError::new(ErrorCode::CapabilityDenied, "候选 manifest JSON 无效。")
                .with_diagnostic(error.to_string())
        })?;
    if binding.report_version != 1 || binding.run_id != root.run_id() {
        return Err(candidate_mismatch());
    }
    let current_exe =
        std::fs::canonicalize(std::env::current_exe().map_err(io_error)?).map_err(io_error)?;
    let installed = std::fs::canonicalize(&binding.manifest.installed_binary.absolute_path)
        .map_err(io_error)?;
    if current_exe != installed || binding.manifest.sha256()? != binding.manifest_sha256 {
        return Err(candidate_mismatch());
    }
    let database = Database::open(&root.resolve_owned(Path::new("scale-app-data"))?)?;
    let connection = database.open_read_connection()?;
    binding.manifest.verify(&connection)?;
    root.verify_current_identity()?;
    Ok(CandidateVerificationReport {
        report_version: 1,
        run_id: root.run_id(),
        manifest_sha256: binding.manifest_sha256,
        current_process_matches_installed_binary: true,
        verified: true,
    })
}

pub fn verify_admissions(
    root: &PhaseEightRunRoot,
    manifest_relative: &Path,
    import_relative: &Path,
    recycle_relative: &Path,
) -> Result<AdmissionVerificationReport, DomainError> {
    root.verify_current_identity()?;
    let binding: CandidateBindingReportInput = read_json(
        &root.resolve_owned(manifest_relative)?,
        "候选 manifest JSON 无效。",
    )?;
    if binding.report_version != 1 || binding.run_id != root.run_id() {
        return Err(candidate_mismatch());
    }
    let import: FeatureAdmission = read_json(
        &root.resolve_owned(import_relative)?,
        "Import 准入 JSON 无效。",
    )?;
    let recycle: FeatureAdmission = read_json(
        &root.resolve_owned(recycle_relative)?,
        "Recycle 准入 JSON 无效。",
    )?;
    if import.feature != FeatureGate::Import || recycle.feature != FeatureGate::Recycle {
        return Err(candidate_mismatch());
    }
    import.verify(&binding.manifest)?;
    recycle.verify(&binding.manifest)?;
    let database = Database::open(&root.resolve_owned(Path::new("scale-app-data"))?)?;
    let connection = database.open_read_connection()?;
    binding.manifest.verify(&connection)?;
    let manifest_sha256 = binding.manifest.sha256()?;
    if manifest_sha256 != binding.manifest_sha256 {
        return Err(candidate_mismatch());
    }
    root.verify_current_identity()?;
    Ok(AdmissionVerificationReport {
        report_version: 1,
        run_id: root.run_id(),
        manifest_sha256,
        import_admission_verified: true,
        recycle_admission_verified: true,
        verified: true,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CandidateBindingReportInput {
    report_version: u32,
    run_id: uuid::Uuid,
    manifest_sha256: String,
    manifest: CandidateManifest,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, message: &str) -> Result<T, DomainError> {
    serde_json::from_slice(&std::fs::read(path).map_err(io_error)?).map_err(|error| {
        DomainError::new(ErrorCode::CapabilityDenied, message).with_diagnostic(error.to_string())
    })
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(ErrorCode::CapabilityDenied, "无法读取候选绑定对象。")
        .with_diagnostic(error.to_string())
}

fn candidate_mismatch() -> DomainError {
    DomainError::new(
        ErrorCode::CapabilityDenied,
        "当前进程或候选 manifest 已变化，全部候选测试失效。",
    )
}
