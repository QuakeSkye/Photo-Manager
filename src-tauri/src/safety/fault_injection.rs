use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::domain::{DomainError, ErrorCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultPoint {
    SqliteBeforeCommit,
    SqliteAfterCommit,
    Win32BeforeCall,
    Win32AfterCall,
    ShellBeforeCallback,
    ShellAfterCallback,
}

#[derive(Debug, Clone, Default)]
pub struct FaultInjector {
    inner: Arc<Mutex<HashMap<FaultPoint, Trigger>>>,
}

#[derive(Debug, Clone, Copy)]
struct Trigger {
    remaining_hits: usize,
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
        let mut triggers = self
            .inner
            .lock()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "故障注入器不可用。"))?;

        let Some(trigger) = triggers.get_mut(&point) else {
            return Ok(());
        };

        trigger.remaining_hits -= 1;
        if trigger.remaining_hits == 0 {
            triggers.remove(&point);
            return Err(
                DomainError::new(ErrorCode::FaultInjected, "测试故障已按计划触发。")
                    .with_diagnostic(format!("{point:?}")),
            );
        }

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
