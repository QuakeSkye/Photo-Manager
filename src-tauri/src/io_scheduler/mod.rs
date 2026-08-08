use std::{
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::domain::{DomainError, ErrorCode};

const PRIORITY_COUNT: usize = 5;
const DEFAULT_BROWSE_IDLE: Duration = Duration::from_secs(2);
const WAIT_SLICE: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IoPriority {
    CurrentPreview = 0,
    MetadataAndMark = 1,
    ImportCurrentFile = 2,
    ScanAndExif = 3,
    BackgroundHashAndCache = 4,
}

impl IoPriority {
    const fn index(self) -> usize {
        self as usize
    }

    const fn is_background(self) -> bool {
        matches!(self, Self::ScanAndExif | Self::BackgroundHashAndCache)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IoSchedulerMetrics {
    pub rotational: bool,
    pub concurrency_limit: usize,
    pub waiting: [usize; PRIORITY_COUNT],
    pub acquired: [u64; PRIORITY_COUNT],
    pub completed: [u64; PRIORITY_COUNT],
    pub cancelled_waits: [u64; PRIORITY_COUNT],
    pub max_queue_depth: usize,
    pub running: usize,
}

#[derive(Debug)]
struct SchedulerState {
    waiting: [usize; PRIORITY_COUNT],
    acquired: [u64; PRIORITY_COUNT],
    completed: [u64; PRIORITY_COUNT],
    cancelled_waits: [u64; PRIORITY_COUNT],
    max_queue_depth: usize,
    running: usize,
    last_browse_activity: Option<Instant>,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self {
            waiting: [0; PRIORITY_COUNT],
            acquired: [0; PRIORITY_COUNT],
            completed: [0; PRIORITY_COUNT],
            cancelled_waits: [0; PRIORITY_COUNT],
            max_queue_depth: 0,
            running: 0,
            last_browse_activity: None,
        }
    }
}

#[derive(Debug)]
pub struct IoScheduler {
    state: Mutex<SchedulerState>,
    changed: Condvar,
    rotational: bool,
    concurrency_limit: usize,
    browse_idle: Duration,
}

impl IoScheduler {
    pub fn for_rotational() -> Self {
        Self::with_configuration(true, 2, DEFAULT_BROWSE_IDLE)
    }

    pub fn for_solid_state() -> Self {
        Self::with_configuration(false, 4, DEFAULT_BROWSE_IDLE)
    }

    fn with_configuration(
        rotational: bool,
        concurrency_limit: usize,
        browse_idle: Duration,
    ) -> Self {
        Self {
            state: Mutex::new(SchedulerState::default()),
            changed: Condvar::new(),
            rotational,
            concurrency_limit: concurrency_limit.max(1),
            browse_idle,
        }
    }

    pub fn note_browse_activity(&self) -> Result<(), DomainError> {
        let mut state = self.state.lock().map_err(lock_error)?;
        state.last_browse_activity = Some(Instant::now());
        self.changed.notify_all();
        Ok(())
    }

    pub fn acquire<C>(
        &self,
        priority: IoPriority,
        mut cancelled: C,
    ) -> Result<IoPermit<'_>, DomainError>
    where
        C: FnMut() -> bool,
    {
        let index = priority.index();
        let mut state = self.state.lock().map_err(lock_error)?;
        state.waiting[index] += 1;
        state.max_queue_depth = state
            .max_queue_depth
            .max(state.waiting.iter().sum::<usize>());
        loop {
            if cancelled() {
                state.waiting[index] -= 1;
                state.cancelled_waits[index] += 1;
                self.changed.notify_all();
                return Err(DomainError::new(
                    ErrorCode::InvalidTransition,
                    "后台 I/O 已安全取消。",
                ));
            }
            if can_acquire(&state, priority, self.concurrency_limit, self.browse_idle) {
                state.waiting[index] -= 1;
                state.running += 1;
                state.acquired[index] += 1;
                return Ok(IoPermit {
                    scheduler: self,
                    priority,
                    started: Instant::now(),
                });
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, WAIT_SLICE)
                .map_err(lock_error)?;
            state = next;
        }
    }

    pub fn metrics(&self) -> Result<IoSchedulerMetrics, DomainError> {
        let state = self.state.lock().map_err(lock_error)?;
        Ok(IoSchedulerMetrics {
            rotational: self.rotational,
            concurrency_limit: self.concurrency_limit,
            waiting: state.waiting,
            acquired: state.acquired,
            completed: state.completed,
            cancelled_waits: state.cancelled_waits,
            max_queue_depth: state.max_queue_depth,
            running: state.running,
        })
    }
}

fn can_acquire(
    state: &SchedulerState,
    priority: IoPriority,
    concurrency_limit: usize,
    browse_idle: Duration,
) -> bool {
    if state.running >= concurrency_limit {
        return false;
    }
    if state.waiting[..priority.index()]
        .iter()
        .any(|waiting| *waiting > 0)
    {
        return false;
    }
    if priority.is_background()
        && state
            .last_browse_activity
            .is_some_and(|activity| activity.elapsed() < browse_idle)
    {
        return false;
    }
    true
}

#[derive(Debug)]
pub struct IoPermit<'a> {
    scheduler: &'a IoScheduler,
    priority: IoPriority,
    started: Instant,
}

impl Drop for IoPermit<'_> {
    fn drop(&mut self) {
        let _elapsed = self.started.elapsed();
        if let Ok(mut state) = self.scheduler.state.lock() {
            state.running = state.running.saturating_sub(1);
            state.completed[self.priority.index()] += 1;
            self.scheduler.changed.notify_all();
        }
    }
}

fn lock_error<T>(_error: T) -> DomainError {
    DomainError::new(ErrorCode::Internal, "I/O 调度器状态不可用。")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    use super::*;

    #[test]
    fn rotational_scheduler_uses_finite_concurrency_and_records_metrics() {
        let scheduler = IoScheduler::for_rotational();
        let first = scheduler
            .acquire(IoPriority::CurrentPreview, || false)
            .unwrap();
        let second = scheduler
            .acquire(IoPriority::ScanAndExif, || false)
            .unwrap();
        let metrics = scheduler.metrics().unwrap();
        assert!(metrics.rotational);
        assert_eq!(metrics.concurrency_limit, 2);
        assert_eq!(metrics.running, 2);
        drop(first);
        drop(second);
        assert_eq!(
            scheduler.metrics().unwrap().completed.iter().sum::<u64>(),
            2
        );
    }

    #[test]
    fn browsing_temporarily_suppresses_background_work() {
        let scheduler = Arc::new(IoScheduler::with_configuration(
            true,
            1,
            Duration::from_millis(80),
        ));
        scheduler.note_browse_activity().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = scheduler.clone();
        let thread = std::thread::spawn(move || {
            let _permit = worker
                .acquire(IoPriority::BackgroundHashAndCache, || false)
                .unwrap();
            sender.send(Instant::now()).unwrap();
        });
        assert!(receiver.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(receiver.recv_timeout(Duration::from_millis(150)).is_ok());
        thread.join().unwrap();
    }

    #[test]
    fn cancelled_wait_does_not_consume_a_permit() {
        let scheduler = IoScheduler::for_rotational();
        let cancelled = AtomicBool::new(true);
        let error = scheduler
            .acquire(IoPriority::ScanAndExif, || {
                cancelled.load(Ordering::Acquire)
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidTransition);
        let metrics = scheduler.metrics().unwrap();
        assert_eq!(metrics.running, 0);
        assert_eq!(metrics.cancelled_waits[IoPriority::ScanAndExif.index()], 1);
    }
}
