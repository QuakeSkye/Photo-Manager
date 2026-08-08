use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    db::{
        LATEST_SCHEMA_VERSION, MigrationFingerprint, migration_chain_checksum, migration_manifest,
        schema_checksum,
    },
    domain::{DomainError, ErrorCode},
};

const PACKAGE_LOCK: &[u8] = include_bytes!("../../../package-lock.json");
const CARGO_LOCK: &[u8] = include_bytes!("../../Cargo.lock");
pub const SAFETY_REPORT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseMode {
    ReadWrite,
    ReadOnlySafe,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureGate {
    Import,
    Recycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildFingerprint {
    pub binary_sha256: String,
    pub package_lock_sha256: String,
    pub cargo_lock_sha256: String,
    pub schema_version: u32,
    pub schema_checksum: String,
    pub migration_chain_checksum: String,
    pub migrations: Vec<MigrationFingerprint>,
    pub safety_report_version: u32,
}

impl BuildFingerprint {
    pub fn for_binary(
        binary_path: &Path,
        connection: &rusqlite::Connection,
    ) -> Result<Self, DomainError> {
        let binary = std::fs::read(binary_path).map_err(|error| {
            DomainError::new(
                ErrorCode::Internal,
                "无法计算当前应用构建指纹，照片写入功能保持关闭。",
            )
            .with_diagnostic(error.to_string())
        })?;

        Ok(Self {
            binary_sha256: sha256(&binary),
            package_lock_sha256: sha256(PACKAGE_LOCK),
            cargo_lock_sha256: sha256(CARGO_LOCK),
            schema_version: LATEST_SCHEMA_VERSION,
            schema_checksum: schema_checksum(connection)?,
            migration_chain_checksum: migration_chain_checksum(),
            migrations: migration_manifest(),
            safety_report_version: SAFETY_REPORT_VERSION,
        })
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRecord {
    pub feature: FeatureGate,
    pub fingerprint: BuildFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedHealth {
    pub root_identity: bool,
    pub database: bool,
    pub job_runner: bool,
    pub reconciler: bool,
    pub capability_boundary: bool,
}

impl SharedHealth {
    pub const fn healthy() -> Self {
        Self {
            root_identity: true,
            database: true,
            job_runner: true,
            reconciler: true,
            capability_boundary: true,
        }
    }

    pub const fn all_healthy(self) -> bool {
        self.root_identity
            && self.database
            && self.job_runner
            && self.reconciler
            && self.capability_boundary
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Open,
    Closed,
}

#[derive(Debug, Clone, Default)]
pub struct GateEvaluator {
    import_compiled: bool,
    recycle_compiled: bool,
}

impl GateEvaluator {
    #[cfg(test)]
    pub const fn for_test(import_compiled: bool, recycle_compiled: bool) -> Self {
        Self {
            import_compiled,
            recycle_compiled,
        }
    }

    pub fn global_decision(
        &self,
        database_mode: DatabaseMode,
        health: SharedHealth,
    ) -> GateDecision {
        if database_mode == DatabaseMode::ReadWrite && health.all_healthy() {
            GateDecision::Open
        } else {
            GateDecision::Closed
        }
    }

    pub fn feature_decision(
        &self,
        feature: FeatureGate,
        database_mode: DatabaseMode,
        health: SharedHealth,
        current: &BuildFingerprint,
        admission: Option<&AdmissionRecord>,
    ) -> GateDecision {
        let compiled = match feature {
            FeatureGate::Import => self.import_compiled,
            FeatureGate::Recycle => self.recycle_compiled,
        };
        let exact_admission = admission
            .is_some_and(|record| record.feature == feature && record.fingerprint == *current);

        if compiled
            && exact_admission
            && self.global_decision(database_mode, health) == GateDecision::Open
        {
            GateDecision::Open
        } else {
            GateDecision::Closed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint() -> BuildFingerprint {
        BuildFingerprint {
            binary_sha256: "binary".into(),
            package_lock_sha256: "package".into(),
            cargo_lock_sha256: "cargo".into(),
            schema_version: 1,
            schema_checksum: "schema".into(),
            migration_chain_checksum: "migration-chain".into(),
            migrations: vec![MigrationFingerprint {
                version: 1,
                filename: "0001.sql".into(),
                sha256: "migration".into(),
            }],
            safety_report_version: 1,
        }
    }

    #[test]
    fn phase_zero_features_are_closed_even_when_shared_health_is_good() {
        let gates = GateEvaluator::default();
        let current = fingerprint();
        let admission = AdmissionRecord {
            feature: FeatureGate::Import,
            fingerprint: current.clone(),
        };

        assert_eq!(
            gates.feature_decision(
                FeatureGate::Import,
                DatabaseMode::ReadWrite,
                SharedHealth::healthy(),
                &current,
                Some(&admission),
            ),
            GateDecision::Closed
        );
        assert_eq!(
            gates.feature_decision(
                FeatureGate::Recycle,
                DatabaseMode::ReadWrite,
                SharedHealth::healthy(),
                &current,
                None,
            ),
            GateDecision::Closed
        );
    }

    #[test]
    fn exact_build_binding_is_required_when_a_feature_is_test_enabled() {
        let gates = GateEvaluator::for_test(true, false);
        let current = fingerprint();
        let mut stale = current.clone();
        stale.schema_version = 2;
        let stale_admission = AdmissionRecord {
            feature: FeatureGate::Import,
            fingerprint: stale,
        };

        assert_eq!(
            gates.feature_decision(
                FeatureGate::Import,
                DatabaseMode::ReadWrite,
                SharedHealth::healthy(),
                &current,
                Some(&stale_admission),
            ),
            GateDecision::Closed
        );
    }

    #[test]
    fn any_shared_safety_fault_closes_both_features() {
        let gates = GateEvaluator::for_test(true, true);
        let current = fingerprint();
        let mut health = SharedHealth::healthy();
        health.reconciler = false;
        let import_admission = AdmissionRecord {
            feature: FeatureGate::Import,
            fingerprint: current.clone(),
        };
        let recycle_admission = AdmissionRecord {
            feature: FeatureGate::Recycle,
            fingerprint: current.clone(),
        };

        assert_eq!(
            gates.feature_decision(
                FeatureGate::Import,
                DatabaseMode::ReadWrite,
                health,
                &current,
                Some(&import_admission),
            ),
            GateDecision::Closed
        );
        assert_eq!(
            gates.feature_decision(
                FeatureGate::Recycle,
                DatabaseMode::ReadWrite,
                health,
                &current,
                Some(&recycle_admission),
            ),
            GateDecision::Closed
        );
    }

    #[test]
    fn current_build_fingerprint_binds_binary_locks_schema_and_migration() {
        let executable = std::env::current_exe().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let database = crate::db::Database::open(directory.path()).unwrap();
        let connection = database.open_read_connection().unwrap();
        let fingerprint = BuildFingerprint::for_binary(&executable, &connection).unwrap();

        assert_eq!(fingerprint.binary_sha256.len(), 64);
        assert_eq!(fingerprint.package_lock_sha256.len(), 64);
        assert_eq!(fingerprint.cargo_lock_sha256.len(), 64);
        assert_eq!(fingerprint.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(
            fingerprint.schema_checksum,
            schema_checksum(&connection).unwrap()
        );
        assert_eq!(
            fingerprint.migration_chain_checksum,
            migration_chain_checksum()
        );
        assert_eq!(fingerprint.migrations, migration_manifest());
        assert_eq!(fingerprint.safety_report_version, SAFETY_REPORT_VERSION);
    }

    #[test]
    fn every_fingerprint_component_invalidates_admission() {
        let gates = GateEvaluator::for_test(true, true);
        let current = fingerprint();
        let mut variants = Vec::new();

        let mut changed = current.clone();
        changed.binary_sha256.push('x');
        variants.push(changed);
        let mut changed = current.clone();
        changed.package_lock_sha256.push('x');
        variants.push(changed);
        let mut changed = current.clone();
        changed.cargo_lock_sha256.push('x');
        variants.push(changed);
        let mut changed = current.clone();
        changed.schema_version += 1;
        variants.push(changed);
        let mut changed = current.clone();
        changed.schema_checksum.push('x');
        variants.push(changed);
        let mut changed = current.clone();
        changed.migration_chain_checksum.push('x');
        variants.push(changed);
        let mut changed = current.clone();
        changed.migrations[0].sha256.push('x');
        variants.push(changed);
        let mut changed = current.clone();
        changed.safety_report_version += 1;
        variants.push(changed);

        for stale in variants {
            let record = AdmissionRecord {
                feature: FeatureGate::Import,
                fingerprint: stale,
            };
            assert_eq!(
                gates.feature_decision(
                    FeatureGate::Import,
                    DatabaseMode::ReadWrite,
                    SharedHealth::healthy(),
                    &current,
                    Some(&record),
                ),
                GateDecision::Closed
            );
        }
    }
}
