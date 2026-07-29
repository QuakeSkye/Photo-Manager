mod fault_injection;
mod gates;
mod test_root;

pub use fault_injection::{FaultInjector, FaultPoint};
pub use gates::{
    AdmissionRecord, BuildFingerprint, DatabaseMode, FeatureGate, GateDecision, GateEvaluator,
    SharedHealth,
};
pub use test_root::TestRoot;

use serde::Serialize;

#[derive(Debug)]
pub struct SafetyState {
    database_mode: StatusDatabaseMode,
}

impl SafetyState {
    pub const fn phase_zero() -> Self {
        Self {
            database_mode: StatusDatabaseMode::Unavailable,
        }
    }

    pub const fn from_database_mode(mode: DatabaseMode) -> Self {
        let database_mode = match mode {
            DatabaseMode::ReadWrite => StatusDatabaseMode::ReadWrite,
            DatabaseMode::ReadOnlySafe => StatusDatabaseMode::ReadOnlySafe,
            DatabaseMode::Unavailable => StatusDatabaseMode::Unavailable,
        };
        Self { database_mode }
    }

    pub fn status(&self) -> SafetyStatus {
        SafetyStatus {
            phase: "阶段 0：工程与安全底座",
            global_write_gate: GateState::Closed,
            import_gate: GateState::Closed,
            recycle_gate: GateState::Closed,
            database_mode: self.database_mode,
            message: "阶段 0 安全壳已启动。照片写入功能默认且强制关闭。",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum GateState {
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize)]
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
}
