mod candidate;
mod fault_injection;
mod gates;
mod phase_eight;
mod test_root;

pub use candidate::{ArtifactFingerprint, CandidateManifest, FeatureAdmission};
pub use fault_injection::{FaultInjector, FaultPoint};
pub use gates::{
    AdmissionRecord, BuildFingerprint, DatabaseMode, FeatureGate, GateDecision, GateEvaluator,
    SharedHealth,
};
pub use phase_eight::{
    PHASE_EIGHT_MAX_ALLOCATED_BYTES, PHASE_EIGHT_MIN_FREE_RESERVE_BYTES, PHASE_EIGHT_PARENT,
    PhaseEightMediaProfile, PhaseEightRunRoot,
};
pub use test_root::TestRoot;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::Serialize;

#[derive(Debug)]
pub struct SafetyState {
    database_mode: StatusDatabaseMode,
    database_write_healthy: Arc<AtomicBool>,
}

impl SafetyState {
    pub fn phase_zero() -> Self {
        Self {
            database_mode: StatusDatabaseMode::Unavailable,
            database_write_healthy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn from_database(database: &crate::db::Database) -> Self {
        let database_mode = match database.mode() {
            DatabaseMode::ReadWrite => StatusDatabaseMode::ReadWrite,
            DatabaseMode::ReadOnlySafe => StatusDatabaseMode::ReadOnlySafe,
            DatabaseMode::Unavailable => StatusDatabaseMode::Unavailable,
        };
        Self {
            database_mode,
            database_write_healthy: database.write_health_handle(),
        }
    }

    pub fn status(&self) -> SafetyStatus {
        let globally_healthy = self.database_mode == StatusDatabaseMode::ReadWrite
            && self.database_write_healthy.load(Ordering::Acquire);
        SafetyStatus {
            phase: "阶段 8：规模硬化与 v1 发布",
            global_write_gate: if globally_healthy {
                GateState::Open
            } else {
                GateState::Closed
            },
            import_gate: GateState::Closed,
            recycle_gate: GateState::Closed,
            database_mode: self.database_mode,
            message: if globally_healthy {
                "共享耐久写入层正常；真实照片 Import 与 Recycle 仍等待独立准入和确认。"
            } else {
                "共享耐久写入层异常；Import 与 Recycle 已同时关闭。"
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum GateState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StatusDatabaseMode {
    ReadWrite,
    ReadOnlySafe,
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyStatus {
    phase: &'static str,
    global_write_gate: GateState,
    import_gate: GateState,
    recycle_gate: GateState,
    database_mode: StatusDatabaseMode,
    message: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_zero_always_fails_closed() {
        let status = SafetyState::phase_zero().status();

        assert!(matches!(status.global_write_gate, GateState::Closed));
        assert!(matches!(status.import_gate, GateState::Closed));
        assert!(matches!(status.recycle_gate, GateState::Closed));
    }

    #[test]
    fn durability_fault_closes_the_shared_gate() {
        let directory = tempfile::tempdir().unwrap();
        let database = crate::db::Database::open(directory.path()).unwrap();
        let state = SafetyState::from_database(&database);
        assert!(matches!(state.status().global_write_gate, GateState::Open));

        database
            .write_health_handle()
            .store(false, Ordering::Release);
        assert!(matches!(
            state.status().global_write_gate,
            GateState::Closed
        ));
        assert!(matches!(state.status().import_gate, GateState::Closed));
        assert!(matches!(state.status().recycle_gate, GateState::Closed));
    }
}
