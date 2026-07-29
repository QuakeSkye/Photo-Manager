use serde::{Deserialize, Serialize};

use super::{AttemptId, DomainError, ErrorCode, ReservationId, TaskId};

pub trait Transition: Sized + Copy {
    fn can_transition_to(self, next: Self) -> bool;

    fn transition_to(self, next: Self) -> Result<Self, DomainError>
    where
        Self: std::fmt::Debug,
    {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(DomainError::invalid_transition(
                std::any::type_name::<Self>(),
                &format!("{self:?}"),
                &format!("{next:?}"),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Import,
    Recycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycle {
    Prepared,
    Active,
    AwaitingUser,
    AwaitingSource,
    NeedsAttention,
    Completed,
    Cancelled,
}

impl TaskLifecycle {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

impl Transition for TaskLifecycle {
    fn can_transition_to(self, next: Self) -> bool {
        use TaskLifecycle::{
            Active, AwaitingSource, AwaitingUser, Cancelled, Completed, NeedsAttention, Prepared,
        };

        matches!(
            (self, next),
            (
                Prepared,
                Active | AwaitingUser | AwaitingSource | NeedsAttention | Cancelled
            ) | (
                Active,
                AwaitingUser | AwaitingSource | NeedsAttention | Completed | Cancelled
            ) | (
                AwaitingUser | AwaitingSource | NeedsAttention,
                Active | Cancelled
            )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportAttemptState {
    Prepared,
    Running,
    Completed,
    CompletedWithErrors,
    AwaitingSource,
    NeedsAttention,
    Cancelled,
    Interrupted,
}

impl ImportAttemptState {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Prepared | Self::Running)
    }
}

impl Transition for ImportAttemptState {
    fn can_transition_to(self, next: Self) -> bool {
        use ImportAttemptState::{
            AwaitingSource, Cancelled, Completed, CompletedWithErrors, Interrupted, NeedsAttention,
            Prepared, Running,
        };

        matches!(
            (self, next),
            (Prepared, Running | Cancelled | Interrupted)
                | (
                    Running,
                    Completed
                        | CompletedWithErrors
                        | AwaitingSource
                        | NeedsAttention
                        | Cancelled
                        | Interrupted
                )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionAttemptState {
    Prepared,
    Running,
    Completed,
    CompletedWithErrors,
    NeedsAttention,
    Cancelled,
    Interrupted,
}

impl DeletionAttemptState {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Prepared | Self::Running)
    }
}

impl Transition for DeletionAttemptState {
    fn can_transition_to(self, next: Self) -> bool {
        use DeletionAttemptState::{
            Cancelled, Completed, CompletedWithErrors, Interrupted, NeedsAttention, Prepared,
            Running,
        };

        matches!(
            (self, next),
            (Prepared, Running | Cancelled | Interrupted)
                | (
                    Running,
                    Completed | CompletedWithErrors | NeedsAttention | Cancelled | Interrupted
                )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheEvictionState {
    Prepared,
    Deleting,
    AwaitingRoot,
    Evicted,
    AlreadyMissing,
    NeedsAttention,
}

impl CacheEvictionState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Evicted | Self::AlreadyMissing | Self::NeedsAttention
        )
    }
}

impl Transition for CacheEvictionState {
    fn can_transition_to(self, next: Self) -> bool {
        use CacheEvictionState::{
            AlreadyMissing, AwaitingRoot, Deleting, Evicted, NeedsAttention, Prepared,
        };

        matches!(
            (self, next),
            (
                Prepared,
                Deleting | AlreadyMissing | AwaitingRoot | NeedsAttention
            ) | (
                Deleting,
                Evicted | AlreadyMissing | AwaitingRoot | NeedsAttention
            ) | (AwaitingRoot, Deleting | AlreadyMissing | NeedsAttention)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptKind {
    Import,
    Recycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportResult {
    Success,
    Partial,
    NeedsAttention,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptClosePlan {
    pub report_result: ReportResult,
    pub task_lifecycle: TaskLifecycle,
    pub release_terminal_lease: bool,
}

impl AttemptClosePlan {
    pub fn for_import(state: ImportAttemptState) -> Result<Self, DomainError> {
        use ImportAttemptState::{
            AwaitingSource, Cancelled, Completed, CompletedWithErrors, Interrupted, NeedsAttention,
            Prepared, Running,
        };

        match state {
            Completed => Ok(Self::terminal(
                ReportResult::Success,
                TaskLifecycle::Completed,
            )),
            CompletedWithErrors => Ok(Self::waiting(
                ReportResult::Partial,
                TaskLifecycle::AwaitingUser,
            )),
            AwaitingSource => Ok(Self::waiting(
                ReportResult::NeedsAttention,
                TaskLifecycle::AwaitingSource,
            )),
            NeedsAttention | Interrupted => Ok(Self::waiting(
                ReportResult::NeedsAttention,
                TaskLifecycle::NeedsAttention,
            )),
            Cancelled => Ok(Self::terminal(
                ReportResult::Cancelled,
                TaskLifecycle::Cancelled,
            )),
            Prepared | Running => Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "Attempt 尚未进入合法终态，不能执行原子关闭。",
            )),
        }
    }

    pub fn for_deletion(state: DeletionAttemptState) -> Result<Self, DomainError> {
        use DeletionAttemptState::{
            Cancelled, Completed, CompletedWithErrors, Interrupted, NeedsAttention, Prepared,
            Running,
        };

        match state {
            Completed => Ok(Self::terminal(
                ReportResult::Success,
                TaskLifecycle::Completed,
            )),
            CompletedWithErrors => Ok(Self::waiting(
                ReportResult::Partial,
                TaskLifecycle::AwaitingUser,
            )),
            NeedsAttention | Interrupted => Ok(Self::waiting(
                ReportResult::NeedsAttention,
                TaskLifecycle::NeedsAttention,
            )),
            Cancelled => Ok(Self::terminal(
                ReportResult::Cancelled,
                TaskLifecycle::Cancelled,
            )),
            Prepared | Running => Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "Attempt 尚未进入合法终态，不能执行原子关闭。",
            )),
        }
    }

    const fn waiting(report_result: ReportResult, task_lifecycle: TaskLifecycle) -> Self {
        Self {
            report_result,
            task_lifecycle,
            release_terminal_lease: false,
        }
    }

    const fn terminal(report_result: ReportResult, task_lifecycle: TaskLifecycle) -> Self {
        Self {
            report_result,
            task_lifecycle,
            release_terminal_lease: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptCloseStep {
    RevalidateFactsAndLease,
    FinalizeReport,
    FinalizeAttempt,
    DeriveJobAndTask,
    ReleaseReservationsAndClaims,
    ResolveDeletionMarks,
    Commit,
    QueueTargetedRescan,
}

pub const ATTEMPT_CLOSE_SEQUENCE: [AttemptCloseStep; 8] = [
    AttemptCloseStep::RevalidateFactsAndLease,
    AttemptCloseStep::FinalizeReport,
    AttemptCloseStep::FinalizeAttempt,
    AttemptCloseStep::DeriveJobAndTask,
    AttemptCloseStep::ReleaseReservationsAndClaims,
    AttemptCloseStep::ResolveDeletionMarks,
    AttemptCloseStep::Commit,
    AttemptCloseStep::QueueTargetedRescan,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAttempt {
    attempt_id: AttemptId,
    task_id: TaskId,
    attempt_kind: AttemptKind,
    attempt_ordinal: u32,
}

impl TaskAttempt {
    pub fn new(
        attempt_id: AttemptId,
        task_id: TaskId,
        attempt_kind: AttemptKind,
        attempt_ordinal: u32,
    ) -> Result<Self, DomainError> {
        if attempt_ordinal == 0 {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "尝试序号必须从 1 开始。",
            ));
        }

        Ok(Self {
            attempt_id,
            task_id,
            attempt_kind,
            attempt_ordinal,
        })
    }

    pub const fn attempt_id(&self) -> AttemptId {
        self.attempt_id
    }

    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    pub const fn attempt_kind(&self) -> AttemptKind {
        self.attempt_kind
    }

    pub const fn attempt_ordinal(&self) -> u32 {
        self.attempt_ordinal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileWriteLease {
    task_id: Option<TaskId>,
    generation: u64,
    row_version: u64,
}

impl Default for FileWriteLease {
    fn default() -> Self {
        Self {
            task_id: None,
            generation: 0,
            row_version: 1,
        }
    }
}

impl FileWriteLease {
    pub const fn task_id(self) -> Option<TaskId> {
        self.task_id
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub fn acquire_for_attempt(&mut self, task_id: TaskId) -> Result<u64, DomainError> {
        if self.task_id.is_some_and(|current| current != task_id) {
            return Err(DomainError::new(
                ErrorCode::LeaseUnavailable,
                "另一个照片文件任务仍在进行中。",
            ));
        }

        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| DomainError::new(ErrorCode::Internal, "写入栅栏代次已耗尽。"))?;
        self.row_version = self
            .row_version
            .checked_add(1)
            .ok_or_else(|| DomainError::new(ErrorCode::Internal, "写入栅栏版本已耗尽。"))?;
        self.task_id = Some(task_id);
        Ok(self.generation)
    }

    pub fn assert_owner(&self, task_id: TaskId, generation: u64) -> Result<(), DomainError> {
        if self.task_id == Some(task_id) && self.generation == generation {
            Ok(())
        } else {
            Err(DomainError::new(
                ErrorCode::LeaseGenerationMismatch,
                "照片文件任务的执行资格已失效。",
            ))
        }
    }

    pub fn release_terminal_task(
        &mut self,
        task_id: TaskId,
        generation: u64,
    ) -> Result<(), DomainError> {
        self.assert_owner(task_id, generation)?;
        self.task_id = None;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| DomainError::new(ErrorCode::Internal, "写入栅栏代次已耗尽。"))?;
        self.row_version = self
            .row_version
            .checked_add(1)
            .ok_or_else(|| DomainError::new(ErrorCode::Internal, "写入栅栏版本已耗尽。"))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationOwner {
    TaskAttempt {
        task_id: TaskId,
        attempt_id: AttemptId,
        lease_generation: u64,
    },
    CacheEviction {
        eviction_id: uuid::Uuid,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationState {
    Active,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationReservation {
    id: ReservationId,
    owner: ReservationOwner,
    state: ReservationState,
}

impl OperationReservation {
    pub const fn new(id: ReservationId, owner: ReservationOwner) -> Self {
        Self {
            id,
            owner,
            state: ReservationState::Active,
        }
    }

    pub const fn id(&self) -> ReservationId {
        self.id
    }

    pub const fn owner(&self) -> ReservationOwner {
        self.owner
    }

    pub const fn state(&self) -> ReservationState {
        self.state
    }

    pub fn release(&mut self) -> Result<(), DomainError> {
        if self.state == ReservationState::Released {
            return Err(DomainError::new(
                ErrorCode::TerminalState,
                "该文件占用已经释放。",
            ));
        }
        self.state = ReservationState::Released;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_terminal_rejects_all<T>(terminal: T, all: &[T])
    where
        T: Transition + Copy + std::fmt::Debug,
    {
        for next in all {
            assert!(
                terminal.transition_to(*next).is_err(),
                "{terminal:?} unexpectedly transitioned to {next:?}",
            );
        }
    }

    #[test]
    fn task_transition_table_is_exhaustive_at_terminals() {
        let all = [
            TaskLifecycle::Prepared,
            TaskLifecycle::Active,
            TaskLifecycle::AwaitingUser,
            TaskLifecycle::AwaitingSource,
            TaskLifecycle::NeedsAttention,
            TaskLifecycle::Completed,
            TaskLifecycle::Cancelled,
        ];

        assert_eq!(
            TaskLifecycle::Prepared
                .transition_to(TaskLifecycle::Active)
                .unwrap(),
            TaskLifecycle::Active
        );
        assert_terminal_rejects_all(TaskLifecycle::Completed, &all);
        assert_terminal_rejects_all(TaskLifecycle::Cancelled, &all);
    }

    #[test]
    fn import_attempt_terminal_never_returns_to_running() {
        let all = [
            ImportAttemptState::Prepared,
            ImportAttemptState::Running,
            ImportAttemptState::Completed,
            ImportAttemptState::CompletedWithErrors,
            ImportAttemptState::AwaitingSource,
            ImportAttemptState::NeedsAttention,
            ImportAttemptState::Cancelled,
            ImportAttemptState::Interrupted,
        ];

        for state in all.into_iter().filter(|state| state.is_terminal()) {
            assert_terminal_rejects_all(state, &all);
        }
    }

    #[test]
    fn deletion_attempt_terminal_never_returns_to_running() {
        let all = [
            DeletionAttemptState::Prepared,
            DeletionAttemptState::Running,
            DeletionAttemptState::Completed,
            DeletionAttemptState::CompletedWithErrors,
            DeletionAttemptState::NeedsAttention,
            DeletionAttemptState::Cancelled,
            DeletionAttemptState::Interrupted,
        ];

        for state in all.into_iter().filter(|state| state.is_terminal()) {
            assert_terminal_rejects_all(state, &all);
        }
    }

    #[test]
    fn cache_eviction_needs_attention_cannot_retry() {
        assert!(
            CacheEvictionState::NeedsAttention
                .transition_to(CacheEvictionState::Deleting)
                .is_err()
        );
    }

    #[test]
    fn lease_generation_fences_old_workers_and_other_tasks() {
        let first_task = TaskId::new();
        let second_task = TaskId::new();
        let mut lease = FileWriteLease::default();
        let first_generation = lease.acquire_for_attempt(first_task).unwrap();

        assert!(lease.acquire_for_attempt(second_task).is_err());
        assert!(lease.assert_owner(first_task, first_generation).is_ok());

        let second_generation = lease.acquire_for_attempt(first_task).unwrap();
        assert!(second_generation > first_generation);
        assert!(lease.assert_owner(first_task, first_generation).is_err());

        lease
            .release_terminal_task(first_task, second_generation)
            .unwrap();
        assert_eq!(lease.task_id(), None);
        assert!(lease.assert_owner(first_task, second_generation).is_err());
    }

    #[test]
    fn reservation_release_is_monotonic() {
        let mut reservation = OperationReservation::new(
            ReservationId::new(),
            ReservationOwner::TaskAttempt {
                task_id: TaskId::new(),
                attempt_id: AttemptId::new(),
                lease_generation: 1,
            },
        );

        reservation.release().unwrap();
        assert_eq!(reservation.state(), ReservationState::Released);
        assert!(reservation.release().is_err());
    }

    #[test]
    fn atomic_close_table_covers_every_import_terminal() {
        let terminal_states = [
            ImportAttemptState::Completed,
            ImportAttemptState::CompletedWithErrors,
            ImportAttemptState::AwaitingSource,
            ImportAttemptState::NeedsAttention,
            ImportAttemptState::Cancelled,
            ImportAttemptState::Interrupted,
        ];
        for state in terminal_states {
            assert!(AttemptClosePlan::for_import(state).is_ok(), "{state:?}");
        }
        assert!(AttemptClosePlan::for_import(ImportAttemptState::Prepared).is_err());
        assert!(AttemptClosePlan::for_import(ImportAttemptState::Running).is_err());
    }

    #[test]
    fn atomic_close_table_covers_every_deletion_terminal() {
        let terminal_states = [
            DeletionAttemptState::Completed,
            DeletionAttemptState::CompletedWithErrors,
            DeletionAttemptState::NeedsAttention,
            DeletionAttemptState::Cancelled,
            DeletionAttemptState::Interrupted,
        ];
        for state in terminal_states {
            assert!(AttemptClosePlan::for_deletion(state).is_ok(), "{state:?}");
        }
        assert!(AttemptClosePlan::for_deletion(DeletionAttemptState::Prepared).is_err());
        assert!(AttemptClosePlan::for_deletion(DeletionAttemptState::Running).is_err());
    }

    #[test]
    fn targeted_rescan_is_strictly_after_commit_in_close_sequence() {
        assert_eq!(
            ATTEMPT_CLOSE_SEQUENCE,
            [
                AttemptCloseStep::RevalidateFactsAndLease,
                AttemptCloseStep::FinalizeReport,
                AttemptCloseStep::FinalizeAttempt,
                AttemptCloseStep::DeriveJobAndTask,
                AttemptCloseStep::ReleaseReservationsAndClaims,
                AttemptCloseStep::ResolveDeletionMarks,
                AttemptCloseStep::Commit,
                AttemptCloseStep::QueueTargetedRescan,
            ]
        );
    }
}
