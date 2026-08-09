use std::{fs, path::Path};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::{DomainError, ErrorCode};

use super::{BuildFingerprint, FeatureGate};

pub const CANDIDATE_MANIFEST_VERSION: u32 = 2;
pub const FEATURE_ADMISSION_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactFingerprint {
    pub absolute_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl ArtifactFingerprint {
    pub fn capture(path: &Path) -> Result<Self, DomainError> {
        let absolute = fs::canonicalize(path).map_err(candidate_io_error)?;
        let bytes = fs::read(&absolute).map_err(candidate_io_error)?;
        Ok(Self {
            absolute_path: absolute.to_string_lossy().into_owned(),
            size_bytes: bytes.len() as u64,
            sha256: sha256(&bytes),
        })
    }

    pub fn verify(&self) -> Result<(), DomainError> {
        if Self::capture(Path::new(&self.absolute_path))? != *self {
            return Err(candidate_mismatch());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateManifest {
    pub report_version: u32,
    pub app_version: String,
    pub installer: ArtifactFingerprint,
    pub installed_binary: ArtifactFingerprint,
    pub build: BuildFingerprint,
}

impl CandidateManifest {
    pub fn capture(
        installer: &Path,
        installed_binary: &Path,
        connection: &Connection,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            report_version: CANDIDATE_MANIFEST_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            installer: ArtifactFingerprint::capture(installer)?,
            installed_binary: ArtifactFingerprint::capture(installed_binary)?,
            build: BuildFingerprint::for_binary(installed_binary, connection)?,
        })
    }

    pub fn verify(&self, connection: &Connection) -> Result<(), DomainError> {
        if self.report_version != CANDIDATE_MANIFEST_VERSION
            || self.app_version != env!("CARGO_PKG_VERSION")
        {
            return Err(candidate_mismatch());
        }
        self.installer.verify()?;
        self.installed_binary.verify()?;
        let current = BuildFingerprint::for_binary(
            Path::new(&self.installed_binary.absolute_path),
            connection,
        )?;
        if current != self.build {
            return Err(candidate_mismatch());
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String, DomainError> {
        let canonical = serde_json::to_vec(self).map_err(|error| {
            DomainError::new(ErrorCode::Internal, "无法编码候选清单，准入保持关闭。")
                .with_diagnostic(error.to_string())
        })?;
        Ok(sha256(&canonical))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeatureAdmission {
    pub report_version: u32,
    pub feature: FeatureGate,
    pub candidate_manifest_sha256: String,
    pub safety_report: ArtifactFingerprint,
    pub evidence: Vec<ArtifactFingerprint>,
    pub acceptance_conditions_passed: u32,
    pub unresolved_critical: u32,
    pub unresolved_high: u32,
}

impl FeatureAdmission {
    pub fn verify(&self, candidate: &CandidateManifest) -> Result<(), DomainError> {
        if self.report_version != FEATURE_ADMISSION_VERSION
            || self.candidate_manifest_sha256 != candidate.sha256()?
            || self.acceptance_conditions_passed != 27
            || self.unresolved_critical != 0
            || self.unresolved_high != 0
            || self.evidence.is_empty()
        {
            return Err(candidate_mismatch());
        }
        self.safety_report.verify()?;
        for evidence in &self.evidence {
            evidence.verify()?;
        }
        Ok(())
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn candidate_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::CapabilityDenied,
        "无法读取冻结候选或绑定证据，准入保持关闭。",
    )
    .with_diagnostic(error.to_string())
}

fn candidate_mismatch() -> DomainError {
    DomainError::new(
        ErrorCode::CapabilityDenied,
        "冻结候选、schema、迁移或报告证据已经变化，全部候选测试失效。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> (tempfile::TempDir, crate::db::Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = crate::db::Database::open(directory.path()).unwrap();
        (directory, database)
    }

    #[test]
    fn artifact_change_invalidates_candidate_and_admission() {
        let directory = tempfile::tempdir().unwrap();
        let installer = directory.path().join("candidate.exe");
        let binary = directory.path().join("installed.exe");
        let report = directory.path().join("import.json");
        fs::write(&installer, b"installer").unwrap();
        fs::write(&binary, b"binary").unwrap();
        fs::write(&report, b"report").unwrap();
        let (_database_directory, database) = database();
        let connection = database.open_read_connection().unwrap();
        let candidate = CandidateManifest::capture(&installer, &binary, &connection).unwrap();
        let report_fingerprint = ArtifactFingerprint::capture(&report).unwrap();
        let admission = FeatureAdmission {
            report_version: FEATURE_ADMISSION_VERSION,
            feature: FeatureGate::Import,
            candidate_manifest_sha256: candidate.sha256().unwrap(),
            safety_report: report_fingerprint.clone(),
            evidence: vec![report_fingerprint],
            acceptance_conditions_passed: 27,
            unresolved_critical: 0,
            unresolved_high: 0,
        };
        candidate.verify(&connection).unwrap();
        admission.verify(&candidate).unwrap();

        fs::write(&binary, b"rebuilt").unwrap();
        assert!(candidate.verify(&connection).is_err());
        fs::write(&report, b"changed report").unwrap();
        assert!(admission.verify(&candidate).is_err());
    }

    #[test]
    fn missing_acceptance_or_risk_gate_rejects_admission() {
        let directory = tempfile::tempdir().unwrap();
        let evidence_path = directory.path().join("evidence.json");
        fs::write(&evidence_path, b"evidence").unwrap();
        let fingerprint = ArtifactFingerprint::capture(&evidence_path).unwrap();
        let (_database_directory, database) = database();
        let connection = database.open_read_connection().unwrap();
        let binary = std::env::current_exe().unwrap();
        let candidate = CandidateManifest::capture(&binary, &binary, &connection).unwrap();
        let mut admission = FeatureAdmission {
            report_version: FEATURE_ADMISSION_VERSION,
            feature: FeatureGate::Recycle,
            candidate_manifest_sha256: candidate.sha256().unwrap(),
            safety_report: fingerprint.clone(),
            evidence: vec![fingerprint],
            acceptance_conditions_passed: 26,
            unresolved_critical: 0,
            unresolved_high: 0,
        };
        assert!(admission.verify(&candidate).is_err());
        admission.acceptance_conditions_passed = 27;
        admission.unresolved_high = 1;
        assert!(admission.verify(&candidate).is_err());
    }
}
