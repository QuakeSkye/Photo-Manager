use std::{
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::{
    domain::{DomainError, ErrorCode},
    safety::{FaultInjector, FaultPoint},
};

use super::migration::map_sqlite_error;

type WriterJob = Box<dyn FnOnce(&mut Connection, &FaultInjector) + Send + 'static>;

enum WriterMessage {
    Run(WriterJob),
    Shutdown,
}

pub struct DbWriter {
    sender: Sender<WriterMessage>,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
    healthy: Arc<AtomicBool>,
}

impl std::fmt::Debug for DbWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DbWriter")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl DbWriter {
    pub fn spawn(
        connection: Connection,
        path: PathBuf,
        fault_injector: FaultInjector,
    ) -> Result<Self, DomainError> {
        let (sender, receiver) = mpsc::channel();
        let healthy = Arc::new(AtomicBool::new(true));
        let thread = thread::Builder::new()
            .name("sqlite-single-writer".into())
            .spawn(move || {
                let mut connection = connection;
                while let Ok(message) = receiver.recv() {
                    match message {
                        WriterMessage::Run(job) => job(&mut connection, &fault_injector),
                        WriterMessage::Shutdown => break,
                    }
                }
            })
            .map_err(|error| {
                DomainError::new(ErrorCode::Internal, "无法启动数据库单写者。")
                    .with_diagnostic(error.to_string())
            })?;

        Ok(Self {
            sender,
            thread: Some(thread),
            path,
            healthy,
        })
    }

    pub fn execute<T, F>(&self, operation: F) -> Result<T, DomainError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection, &FaultInjector) -> Result<T, DomainError> + Send + 'static,
    {
        if !self.healthy.load(Ordering::Acquire) {
            return Err(DomainError::new(
                ErrorCode::ReadOnlySafeMode,
                "数据库耐久写入门已经关闭；请先恢复应用状态。",
            ));
        }
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::Run(Box::new(move |connection, injector| {
                let _ = result_sender.send(operation(connection, injector));
            })))
            .map_err(|_| DomainError::new(ErrorCode::Internal, "数据库单写者已停止。"))?;

        let result = result_receiver
            .recv()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "数据库写入结果丢失。"))?;
        if result.as_ref().is_err_and(is_durability_failure) {
            self.healthy.store(false, Ordering::Release);
        }
        result
    }

    pub fn transaction<T, F>(&self, operation: F) -> Result<T, DomainError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, DomainError> + Send + 'static,
    {
        self.execute(move |connection, injector| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let result = operation(&transaction)?;
            injector.checkpoint(FaultPoint::SqliteBeforeCommit)?;
            transaction.commit().map_err(map_sqlite_error)?;
            injector.checkpoint(FaultPoint::SqliteAfterCommit)?;
            Ok(result)
        })
    }

    pub fn health_handle(&self) -> Arc<AtomicBool> {
        self.healthy.clone()
    }
}

fn is_durability_failure(error: &DomainError) -> bool {
    matches!(
        error.code,
        ErrorCode::DatabaseDurability | ErrorCode::DatabaseCorrupt | ErrorCode::ReadOnlySafeMode
    )
}

impl Drop for DbWriter {
    fn drop(&mut self) {
        let _ = self.sender.send(WriterMessage::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
