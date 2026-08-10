use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use uuid::Uuid;

use crate::{
    db::Database,
    domain::{DomainError, ErrorCode},
};

use super::{DatabaseMode, FeatureGate, SharedHealth};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateCloseReason {
    Startup,
    DatabaseReadOnly,
    WriterUnhealthy,
    RootIdentity,
    JobRunner,
    Reconciler,
    CapabilityBoundary,
    FaultLatched,
}

impl GateCloseReason {
    fn message(self) -> &'static str {
        match self {
            Self::Startup => "正在完成启动安全检查，照片写入暂不可用。",
            Self::DatabaseReadOnly => "数据库处于只读安全模式，Import 与 Recycle 已关闭。",
            Self::WriterUnhealthy => {
                "数据库耐久写入失效，Import 与 Recycle 已关闭；请安全退出并重新启动。"
            }
            Self::RootIdentity => "存储根身份无法证明，Import 与 Recycle 已关闭。",
            Self::JobRunner => "后台任务执行器不可用，Import 与 Recycle 已关闭。",
            Self::Reconciler => "未完成任务无法安全协调，Import 与 Recycle 已关闭。",
            Self::CapabilityBoundary => "文件能力边界失效，Import 与 Recycle 已关闭。",
            Self::FaultLatched => {
                "本次进程已锁存安全故障，Import 与 Recycle 已关闭；请安全退出并重新启动。"
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SafetyPermit {
    gate_instance_id: Uuid,
    feature: FeatureGate,
    generation: u64,
}

#[derive(Debug, Clone, Copy)]
struct GateCore {
    instance_id: Uuid,
    generation: u64,
    ready: bool,
    fault_latched: bool,
    health: SharedHealth,
    fault_reason: Option<GateCloseReason>,
}

#[derive(Debug)]
pub struct RuntimeSafetyGate {
    database: Arc<Database>,
    core: Mutex<GateCore>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyStatus {
    pub phase: &'static str,
    pub global_write_gate: GateState,
    pub import_gate: GateState,
    pub recycle_gate: GateState,
    pub database_mode: DatabaseMode,
    pub reason: Option<GateCloseReason>,
    pub message: String,
}

impl RuntimeSafetyGate {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            core: Mutex::new(GateCore {
                instance_id: Uuid::new_v4(),
                generation: 1,
                ready: false,
                fault_latched: false,
                health: SharedHealth::healthy(),
                fault_reason: Some(GateCloseReason::Startup),
            }),
        }
    }

    pub fn mark_ready(&self) -> Result<(), DomainError> {
        let mut core = self.lock_core()?;
        self.refresh_database_health(&mut core);
        if core.fault_latched {
            return Err(gate_closed_error(core.fault_reason));
        }
        core.ready = true;
        core.fault_reason = None;
        core.generation = core.generation.saturating_add(1);
        Ok(())
    }

    pub fn latch_fault(&self, reason: GateCloseReason) {
        if let Ok(mut core) = self.core.lock() {
            latch_locked(&mut core, reason);
        }
    }

    pub fn set_shared_health(&self, health: SharedHealth) {
        if let Ok(mut core) = self.core.lock() {
            core.health = health;
            if !health.root_identity {
                latch_locked(&mut core, GateCloseReason::RootIdentity);
            } else if !health.database {
                latch_locked(&mut core, GateCloseReason::WriterUnhealthy);
            } else if !health.job_runner {
                latch_locked(&mut core, GateCloseReason::JobRunner);
            } else if !health.reconciler {
                latch_locked(&mut core, GateCloseReason::Reconciler);
            } else if !health.capability_boundary {
                latch_locked(&mut core, GateCloseReason::CapabilityBoundary);
            }
        }
    }

    pub fn require_feature(&self, feature: FeatureGate) -> Result<SafetyPermit, DomainError> {
        let mut core = self.lock_core()?;
        self.refresh_database_health(&mut core);
        self.require_open(&core)?;
        Ok(SafetyPermit {
            gate_instance_id: core.instance_id,
            feature,
            generation: core.generation,
        })
    }

    pub fn revalidate(
        &self,
        permit: &SafetyPermit,
        expected_feature: FeatureGate,
    ) -> Result<(), DomainError> {
        let mut core = self.lock_core()?;
        self.refresh_database_health(&mut core);
        self.require_open(&core)?;
        if permit.gate_instance_id != core.instance_id
            || permit.feature != expected_feature
            || permit.generation != core.generation
        {
            return Err(DomainError::new(
                ErrorCode::CapabilityDenied,
                "写入安全许可已失效，请刷新状态后重试。",
            ));
        }
        Ok(())
    }

    pub fn status(&self) -> SafetyStatus {
        let Ok(mut core) = self.core.lock() else {
            return closed_status(
                self.database.mode(),
                GateCloseReason::FaultLatched,
                GateCloseReason::FaultLatched.message().into(),
            );
        };
        self.refresh_database_health(&mut core);
        let reason = derived_close_reason(&core, self.database.mode());
        match reason {
            None => SafetyStatus {
                phase: concat!("v", env!("CARGO_PKG_VERSION")),
                global_write_gate: GateState::Open,
                import_gate: GateState::Open,
                recycle_gate: GateState::Open,
                database_mode: self.database.mode(),
                reason: None,
                message: "写入安全状态正常；Import 与 Recycle 可用。".into(),
            },
            Some(reason) => closed_status(self.database.mode(), reason, self.close_message(reason)),
        }
    }

    fn close_message(&self, reason: GateCloseReason) -> String {
        if reason == GateCloseReason::DatabaseReadOnly {
            return self
                .database
                .safe_mode_reason()
                .map(|error| error.user_message.clone())
                .unwrap_or_else(|| reason.message().into());
        }
        reason.message().into()
    }

    fn lock_core(&self) -> Result<MutexGuard<'_, GateCore>, DomainError> {
        self.core.lock().map_err(|_| {
            DomainError::new(
                ErrorCode::CapabilityDenied,
                "写入安全状态不可用，Import 与 Recycle 已关闭。",
            )
        })
    }

    fn refresh_database_health(&self, core: &mut GateCore) {
        if self.database.mode() != DatabaseMode::ReadWrite {
            latch_locked(core, GateCloseReason::DatabaseReadOnly);
        } else if !self.database.is_write_healthy() {
            latch_locked(core, GateCloseReason::WriterUnhealthy);
        }
    }

    fn require_open(&self, core: &GateCore) -> Result<(), DomainError> {
        if let Some(reason) = derived_close_reason(core, self.database.mode()) {
            Err(gate_closed_error(Some(reason)))
        } else {
            Ok(())
        }
    }
}

fn latch_locked(core: &mut GateCore, reason: GateCloseReason) {
    if !core.fault_latched {
        core.fault_latched = true;
        core.ready = false;
        core.fault_reason = Some(reason);
        core.generation = core.generation.saturating_add(1);
    }
}

fn derived_close_reason(core: &GateCore, database_mode: DatabaseMode) -> Option<GateCloseReason> {
    if core.fault_latched {
        return core.fault_reason.or(Some(GateCloseReason::FaultLatched));
    }
    if database_mode != DatabaseMode::ReadWrite {
        return Some(GateCloseReason::DatabaseReadOnly);
    }
    if !core.ready {
        return Some(GateCloseReason::Startup);
    }
    if !core.health.root_identity {
        return Some(GateCloseReason::RootIdentity);
    }
    if !core.health.database {
        return Some(GateCloseReason::WriterUnhealthy);
    }
    if !core.health.job_runner {
        return Some(GateCloseReason::JobRunner);
    }
    if !core.health.reconciler {
        return Some(GateCloseReason::Reconciler);
    }
    if !core.health.capability_boundary {
        return Some(GateCloseReason::CapabilityBoundary);
    }
    None
}

fn gate_closed_error(reason: Option<GateCloseReason>) -> DomainError {
    let reason = reason.unwrap_or(GateCloseReason::FaultLatched);
    DomainError::new(ErrorCode::CapabilityDenied, reason.message())
}

fn closed_status(
    database_mode: DatabaseMode,
    reason: GateCloseReason,
    message: String,
) -> SafetyStatus {
    SafetyStatus {
        phase: concat!("v", env!("CARGO_PKG_VERSION")),
        global_write_gate: GateState::Closed,
        import_gate: GateState::Closed,
        recycle_gate: GateState::Closed,
        database_mode,
        reason: Some(reason),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Barrier, atomic::Ordering};

    use rusqlite::Connection;

    use super::*;

    fn gate() -> (Arc<Database>, RuntimeSafetyGate) {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(directory.path()).unwrap());
        let gate = RuntimeSafetyGate::new(database.clone());
        gate.mark_ready().unwrap();
        (database, gate)
    }

    #[test]
    fn healthy_ready_gate_opens_import_and_recycle_without_admission() {
        let (_database, gate) = gate();
        let status = gate.status();
        assert_eq!(status.global_write_gate, GateState::Open);
        assert_eq!(status.import_gate, GateState::Open);
        assert_eq!(status.recycle_gate, GateState::Open);
        gate.require_feature(FeatureGate::Import).unwrap();
        gate.require_feature(FeatureGate::Recycle).unwrap();
    }

    #[test]
    fn require_feature_synchronously_latches_unannounced_writer_poison() {
        let (database, gate) = gate();
        database
            .write_health_handle()
            .store(false, Ordering::Release);

        let error = gate.require_feature(FeatureGate::Import).unwrap_err();
        assert_eq!(error.code, ErrorCode::CapabilityDenied);
        assert_eq!(gate.status().reason, Some(GateCloseReason::WriterUnhealthy));

        database
            .write_health_handle()
            .store(true, Ordering::Release);
        assert!(gate.require_feature(FeatureGate::Import).is_err());
    }

    #[test]
    fn revalidate_synchronously_rejects_unannounced_writer_poison() {
        let (database, gate) = gate();
        let permit = gate.require_feature(FeatureGate::Recycle).unwrap();
        database
            .write_health_handle()
            .store(false, Ordering::Release);

        assert!(gate.revalidate(&permit, FeatureGate::Recycle).is_err());
        assert_eq!(gate.status().reason, Some(GateCloseReason::WriterUnhealthy));
    }

    #[test]
    fn stale_generation_wrong_feature_and_wrong_instance_are_rejected() {
        let (_database, gate) = gate();
        let import = gate.require_feature(FeatureGate::Import).unwrap();
        assert!(gate.revalidate(&import, FeatureGate::Recycle).is_err());

        let mut wrong_instance = import.clone();
        wrong_instance.gate_instance_id = Uuid::new_v4();
        assert!(
            gate.revalidate(&wrong_instance, FeatureGate::Import)
                .is_err()
        );

        let mut stale = import;
        stale.generation = stale.generation.saturating_sub(1);
        assert!(gate.revalidate(&stale, FeatureGate::Import).is_err());
    }

    #[test]
    fn shared_fault_closes_both_features_and_is_monotonic() {
        let (_database, gate) = gate();
        let mut health = SharedHealth::healthy();
        health.reconciler = false;
        gate.set_shared_health(health);
        assert_eq!(gate.status().import_gate, GateState::Closed);
        assert_eq!(gate.status().recycle_gate, GateState::Closed);

        gate.set_shared_health(SharedHealth::healthy());
        assert!(gate.mark_ready().is_err());
        assert_eq!(gate.status().global_write_gate, GateState::Closed);
    }

    #[test]
    fn startup_is_closed_until_marked_ready() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(directory.path()).unwrap());
        let gate = RuntimeSafetyGate::new(database);
        assert_eq!(gate.status().reason, Some(GateCloseReason::Startup));
        assert!(gate.require_feature(FeatureGate::Import).is_err());
        gate.mark_ready().unwrap();
        assert_eq!(gate.status().global_write_gate, GateState::Open);
    }

    #[test]
    fn newer_database_message_reaches_the_runtime_status() {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(directory.path().join("catalog.sqlite3")).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                crate::db::migration::LATEST_SCHEMA_VERSION + 1,
            )
            .unwrap();
        drop(connection);
        let database = Arc::new(Database::open(directory.path()).unwrap());
        let gate = RuntimeSafetyGate::new(database);

        let status = gate.status();

        assert_eq!(status.reason, Some(GateCloseReason::DatabaseReadOnly));
        assert!(status.message.contains("当前程序版本过旧"));
        assert!(status.message.contains("安装最新版本"));
    }

    #[test]
    fn concurrent_fault_transition_never_leaves_a_revalidatable_stale_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(directory.path()).unwrap());
        let gate = Arc::new(RuntimeSafetyGate::new(database));
        gate.mark_ready().unwrap();
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let gate = gate.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                gate.require_feature(FeatureGate::Import).ok()
            }));
        }
        barrier.wait();
        gate.latch_fault(GateCloseReason::CapabilityBoundary);

        for worker in workers {
            if let Some(permit) = worker.join().unwrap() {
                assert!(gate.revalidate(&permit, FeatureGate::Import).is_err());
            }
        }
        assert_eq!(gate.status().global_write_gate, GateState::Closed);
    }
}
