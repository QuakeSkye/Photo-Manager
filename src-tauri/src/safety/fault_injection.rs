use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::domain::{DomainError, ErrorCode};

use super::{GateCloseReason, RuntimeSafetyGate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultPoint {
    SqliteBeforeCommit,
    SqliteAfterCommit,
    Win32BeforeCall,
    Win32AfterCall,
    ShellBeforeCallback,
    ShellAfterCallback,
    ImportAfterPhysicalBeforeFact,
    RecycleAfterPhysicalBeforeFact,
}

#[derive(Debug, Clone, Default)]
pub struct FaultInjector {
    inner: Arc<Mutex<HashMap<FaultPoint, Trigger>>>,
    gate_latches: Arc<Mutex<HashMap<FaultPoint, GateLatchTrigger>>>,
}

#[derive(Debug, Clone, Copy)]
struct Trigger {
    remaining_hits: usize,
}

#[derive(Debug, Clone)]
struct GateLatchTrigger {
    remaining_hits: usize,
    gate: Arc<RuntimeSafetyGate>,
    reason: GateCloseReason,
}

impl FaultInjector {
    pub fn fail_on_hit(&self, point: FaultPoint, hit: usize) -> Result<(), DomainError> {
        if hit == 0 {
            return Err(DomainError::new(
                ErrorCode::FaultInjected,
                "故障注入命中次数必须从 1 开始。",
            ));
        }

        self.inner
            .lock()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "故障注入器不可用。"))?
            .insert(
                point,
                Trigger {
                    remaining_hits: hit,
                },
            );
        Ok(())
    }

    pub fn checkpoint(&self, point: FaultPoint) -> Result<(), DomainError> {
        {
            let mut triggers = self
                .inner
                .lock()
                .map_err(|_| DomainError::new(ErrorCode::Internal, "故障注入器不可用。"))?;
            if let Some(trigger) = triggers.get_mut(&point) {
                trigger.remaining_hits -= 1;
                if trigger.remaining_hits == 0 {
                    triggers.remove(&point);
                    return Err(DomainError::new(
                        ErrorCode::FaultInjected,
                        "测试故障已按计划触发。",
                    )
                    .with_diagnostic(format!("{point:?}")));
                }
            }
        }

        let latch = {
            let mut triggers = self
                .gate_latches
                .lock()
                .map_err(|_| DomainError::new(ErrorCode::Internal, "安全门故障注入器不可用。"))?;
            let Some(trigger) = triggers.get_mut(&point) else {
                return Ok(());
            };
            trigger.remaining_hits -= 1;
            if trigger.remaining_hits == 0 {
                triggers.remove(&point)
            } else {
                None
            }
        };
        if let Some(trigger) = latch {
            trigger.gate.latch_fault(trigger.reason);
        }
        Ok(())
    }

    pub(crate) fn latch_gate_on_hit(
        &self,
        point: FaultPoint,
        hit: usize,
        gate: Arc<RuntimeSafetyGate>,
        reason: GateCloseReason,
    ) -> Result<(), DomainError> {
        if hit == 0 {
            return Err(DomainError::new(
                ErrorCode::FaultInjected,
                "安全门故障注入命中次数必须从 1 开始。",
            ));
        }
        self.gate_latches
            .lock()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "安全门故障注入器不可用。"))?
            .insert(
                point,
                GateLatchTrigger {
                    remaining_hits: hit,
                    gate,
                    reason,
                },
            );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_is_deterministic_and_one_shot() {
        let injector = FaultInjector::default();
        injector
            .fail_on_hit(FaultPoint::SqliteBeforeCommit, 2)
            .unwrap();

        assert!(injector.checkpoint(FaultPoint::SqliteBeforeCommit).is_ok());
        assert_eq!(
            injector
                .checkpoint(FaultPoint::SqliteBeforeCommit)
                .unwrap_err()
                .code,
            ErrorCode::FaultInjected
        );
        assert!(injector.checkpoint(FaultPoint::SqliteBeforeCommit).is_ok());
    }
}
