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
pub const SAFETY_REPORT_VERSION: u32 = 3;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
