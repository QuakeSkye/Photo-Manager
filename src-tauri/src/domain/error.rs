use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidId,
    InvalidTransition,
    TerminalState,
    StaleVersion,
    DuplicateOperation,
    CapabilityDenied,
    VolumeWriteDenied,
    RootConflict,
    RootOffline,
    IdentityMismatch,
    UnsafePath,
    UnsupportedMedia,
    MediaUnreadable,
    CacheOwnership,
    LeaseUnavailable,
    LeaseGenerationMismatch,
    ReservationConflict,
    DatabaseBusy,
    DatabaseDurability,
    DatabaseCorrupt,
    ReadOnlySafeMode,
    TestRootRequired,
    FaultInjected,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[error("{user_message}")]
#[serde(rename_all = "camelCase")]
pub struct DomainError {
    pub code: ErrorCode,
    pub user_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl DomainError {
    pub fn new(code: ErrorCode, user_message: impl Into<String>) -> Self {
        Self {
            code,
            user_message: user_message.into(),
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }

    pub fn invalid_transition(entity: &'static str, from: &str, to: &str) -> Self {
        Self::new(ErrorCode::InvalidTransition, "当前状态不允许执行此操作。")
            .with_diagnostic(format!("{entity}: {from} -> {to}"))
    }

    pub fn stale_version(entity: &'static str) -> Self {
        Self::new(ErrorCode::StaleVersion, "对象已发生变化，请刷新后重试。").with_diagnostic(entity)
    }
}
