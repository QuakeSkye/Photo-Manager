use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
};

use crate::domain::{DomainError, ErrorCode};

pub const REQUIRED_RECYCLE_FLAGS: u32 = 0x0008_0000 | 0x0000_0400 | 0x0010_0000 | 0x0000_0010;

#[derive(Debug, Clone)]
pub struct ShellAsset {
    pub attempt_asset_id: Vec<u8>,
    pub absolute_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ShellPhoto {
    pub assets: Vec<ShellAsset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellPhotoResult {
    pub perform_operations_result: i32,
    pub operations_aborted: bool,
}

pub trait RecycleCallbacks: Send + Sync {
    fn pre_delete(&self, asset: &ShellAsset) -> Result<(), DomainError>;
    fn post_delete(
        &self,
        asset: &ShellAsset,
        hr_delete: i32,
        recycle_object_evidence: Option<Vec<u8>>,
    ) -> Result<(), DomainError>;
}

pub trait RecycleShell: Send + Sync {
    fn recycle_photo(
        &self,
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
    ) -> Result<ShellPhotoResult, DomainError>;
}

#[derive(Debug)]
pub struct StaRecycleWorker {
    sender: mpsc::SyncSender<WorkerCommand>,
}

enum WorkerCommand {
    Recycle {
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
        reply: mpsc::SyncSender<Result<ShellPhotoResult, DomainError>>,
    },
    RecycleAndRestore {
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
        reply: mpsc::SyncSender<Result<ShellPhotoResult, DomainError>>,
    },
}

impl std::fmt::Debug for WorkerCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Recycle")
    }
}

impl StaRecycleWorker {
    pub fn spawn() -> Result<Self, DomainError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("recycle-com-sta".into())
            .spawn(move || worker_loop(receiver))
            .map_err(|error| {
                DomainError::new(ErrorCode::Internal, "无法启动 Windows 回收站 STA worker。")
                    .with_diagnostic(error.to_string())
            })?;
        Ok(Self { sender })
    }

    pub(crate) fn recycle_and_restore_for_phase_eight(
        &self,
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
    ) -> Result<ShellPhotoResult, DomainError> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender
            .send(WorkerCommand::RecycleAndRestore {
                photo,
                callbacks,
                reply,
            })
            .map_err(|_| DomainError::new(ErrorCode::Internal, "回收站 STA worker 已停止。"))?;
        receive
            .recv()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "回收站恢复测试未返回结果。"))?
    }
}

impl RecycleShell for StaRecycleWorker {
    fn recycle_photo(
        &self,
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
    ) -> Result<ShellPhotoResult, DomainError> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender
            .send(WorkerCommand::Recycle {
                photo,
                callbacks,
                reply,
            })
            .map_err(|_| DomainError::new(ErrorCode::Internal, "回收站 STA worker 已停止。"))?;
        receive
            .recv()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "回收站 STA worker 未返回结果。"))?
    }
}

#[cfg(windows)]
fn worker_loop(receiver: mpsc::Receiver<WorkerCommand>) {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(map_windows_error);
    while let Ok(command) = receiver.recv() {
        match command {
            WorkerCommand::Recycle {
                photo,
                callbacks,
                reply,
            } => {
                let result = initialized
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|()| perform_photo(photo, callbacks));
                let _ = reply.send(result);
            }
            WorkerCommand::RecycleAndRestore {
                photo,
                callbacks,
                reply,
            } => {
                let result = initialized
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|()| perform_photo_inner(photo, callbacks, true));
                let _ = reply.send(result);
            }
        }
    }
    if initialized.is_ok() {
        unsafe { CoUninitialize() };
    }
}

#[cfg(not(windows))]
fn worker_loop(receiver: mpsc::Receiver<WorkerCommand>) {
    while let Ok(command) = receiver.recv() {
        match command {
            WorkerCommand::Recycle { reply, .. } => {
                let _ = reply.send(Err(DomainError::new(
                    ErrorCode::CapabilityDenied,
                    "Windows 回收站仅可在 Windows 上使用。",
                )));
            }
            WorkerCommand::RecycleAndRestore { reply, .. } => {
                let _ = reply.send(Err(DomainError::new(
                    ErrorCode::CapabilityDenied,
                    "Windows 回收站仅可在 Windows 上使用。",
                )));
            }
        }
    }
}

#[cfg(windows)]
fn perform_photo(
    photo: ShellPhoto,
    callbacks: Arc<dyn RecycleCallbacks>,
) -> Result<ShellPhotoResult, DomainError> {
    perform_photo_inner(photo, callbacks, false)
}

#[cfg(windows)]
// The recycled IShellItem evidence never leaves this STA thread. Arc is required
// because the COM progress sink and the worker retain the same callback state.
#[allow(clippy::arc_with_non_send_sync)]
fn perform_photo_inner(
    photo: ShellPhoto,
    callbacks: Arc<dyn RecycleCallbacks>,
    restore_for_test: bool,
) -> Result<ShellPhotoResult, DomainError> {
    use std::sync::Mutex;
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            UI::Shell::{
                FOF_NOCONFIRMATION, FOF_NOERRORUI, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE,
                FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
                SHCreateItemFromParsingName,
            },
        },
        core::HSTRING,
    };

    let operation: IFileOperation = unsafe {
        CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER).map_err(map_windows_error)?
    };
    let flags = FOFX_RECYCLEONDELETE | FOF_NOERRORUI | FOFX_EARLYFAILURE | FOF_NOCONFIRMATION;
    debug_assert_eq!(flags.0, REQUIRED_RECYCLE_FLAGS);
    unsafe { operation.SetOperationFlags(flags) }.map_err(map_windows_error)?;

    let callback_error = Arc::new(Mutex::new(None));
    let mut shell_items = Vec::with_capacity(photo.assets.len());
    let mut sinks: Vec<IFileOperationProgressSink> = Vec::with_capacity(photo.assets.len());
    let mut recycled_items = Vec::with_capacity(photo.assets.len());
    let original_assets = photo.assets.clone();
    for asset in photo.assets {
        let path = HSTRING::from(asset.absolute_path.to_string_lossy().as_ref());
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(&path, None) }.map_err(map_windows_error)?;
        let recycled_item = Arc::new(Mutex::new(None));
        let sink: IFileOperationProgressSink = ProgressSink {
            asset,
            callbacks: callbacks.clone(),
            callback_error: callback_error.clone(),
            recycled_item: recycled_item.clone(),
        }
        .into();
        unsafe { operation.DeleteItem(&item, &sink) }.map_err(map_windows_error)?;
        shell_items.push(item);
        sinks.push(sink);
        recycled_items.push(recycled_item);
    }

    let perform_operations_result = match unsafe { operation.PerformOperations() } {
        Ok(()) => 0,
        Err(error) => error.code().0,
    };
    let operations_aborted = unsafe { operation.GetAnyOperationsAborted() }
        .map(|value| value.as_bool())
        .unwrap_or(true);

    drop(sinks);
    drop(shell_items);
    if let Some(error) = callback_error
        .lock()
        .map_err(|_| DomainError::new(ErrorCode::Internal, "回收回调错误状态不可用。"))?
        .clone()
    {
        return Err(error);
    }
    if restore_for_test {
        restore_recycled_items(&original_assets, &recycled_items)?;
    }
    Ok(ShellPhotoResult {
        perform_operations_result,
        operations_aborted,
    })
}

#[cfg(windows)]
use windows::Win32::UI::Shell::{
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
};

#[cfg(windows)]
#[windows::core::implement(IFileOperationProgressSink)]
struct ProgressSink {
    asset: ShellAsset,
    callbacks: Arc<dyn RecycleCallbacks>,
    callback_error: Arc<std::sync::Mutex<Option<DomainError>>>,
    recycled_item: Arc<std::sync::Mutex<Option<IShellItem>>>,
}

#[cfg(windows)]
#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for ProgressSink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _result: windows::core::HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
        _result: windows::core::HRESULT,
        _new_item: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _destination: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _destination: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
        _result: windows::core::HRESULT,
        _new_item: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _destination: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _destination: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
        _result: windows::core::HRESULT,
        _new_item: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn PreDeleteItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.callbacks
            .pre_delete(&self.asset)
            .map_err(|error| self.store(error))
    }

    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        result: windows::core::HRESULT,
        new_item: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        if let (Some(item), Ok(mut target)) = (new_item.cloned(), self.recycled_item.lock()) {
            *target = Some(item);
        }
        let evidence = new_item.as_ref().and_then(shell_item_evidence);
        self.callbacks
            .post_delete(&self.asset, result.0, evidence)
            .map_err(|error| self.store(error))
    }

    fn PreNewItem(
        &self,
        _flags: u32,
        _destination: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _flags: u32,
        _destination: windows::core::Ref<'_, IShellItem>,
        _name: &windows::core::PCWSTR,
        _template: &windows::core::PCWSTR,
        _attributes: u32,
        _result: windows::core::HRESULT,
        _new_item: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _total: u32, _completed: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
fn restore_recycled_items(
    assets: &[ShellAsset],
    recycled_items: &[Arc<std::sync::Mutex<Option<IShellItem>>>],
) -> Result<(), DomainError> {
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            UI::Shell::{
                FOF_NOCONFIRMATION, FOF_NOERRORUI, FOFX_EARLYFAILURE, FileOperation,
                IFileOperation, SHCreateItemFromParsingName,
            },
        },
        core::HSTRING,
    };

    for (asset, recycled) in assets.iter().zip(recycled_items) {
        let item = recycled
            .lock()
            .map_err(|_| DomainError::new(ErrorCode::Internal, "回收对象测试状态不可用。"))?
            .clone()
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "回收对象为空，不能执行恢复测试。",
                )
            })?;
        let parent = asset
            .absolute_path
            .parent()
            .ok_or_else(|| DomainError::new(ErrorCode::UnsafePath, "恢复测试目标缺少父目录。"))?;
        let parent_path = HSTRING::from(parent.to_string_lossy().as_ref());
        let destination: IShellItem = unsafe { SHCreateItemFromParsingName(&parent_path, None) }
            .map_err(map_windows_error)?;
        let name = HSTRING::from(
            asset
                .absolute_path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| DomainError::new(ErrorCode::UnsafePath, "恢复测试文件名无效。"))?,
        );
        let operation: IFileOperation = unsafe {
            CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
                .map_err(map_windows_error)?
        };
        unsafe {
            operation.SetOperationFlags(FOF_NOERRORUI | FOFX_EARLYFAILURE | FOF_NOCONFIRMATION)
        }
        .map_err(map_windows_error)?;
        unsafe { operation.MoveItem(&item, &destination, &name, None) }
            .map_err(map_windows_error)?;
        unsafe { operation.PerformOperations() }.map_err(map_windows_error)?;
        if !asset.absolute_path.is_file() {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "Windows 回收站恢复测试未恢复原路径。",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
impl ProgressSink_Impl {
    fn store(&self, error: DomainError) -> windows::core::Error {
        if let Ok(mut target) = self.callback_error.lock() {
            if target.is_none() {
                *target = Some(error.clone());
            }
        }
        windows::core::Error::new(
            windows::core::HRESULT(0x8000_4004_u32 as i32),
            &error.user_message,
        )
    }
}

#[cfg(windows)]
fn shell_item_evidence(item: &IShellItem) -> Option<Vec<u8>> {
    use windows::{
        Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH},
        },
        core::PWSTR,
    };

    fn display(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> Option<String> {
        let value: PWSTR = unsafe { item.GetDisplayName(kind) }.ok()?;
        let text = unsafe { value.to_string() }.ok();
        unsafe { CoTaskMemFree(Some(value.0.cast())) };
        text.filter(|text| !text.is_empty())
    }

    display(item, SIGDN_FILESYSPATH)
        .or_else(|| display(item, SIGDN_DESKTOPABSOLUTEPARSING))
        .map(String::into_bytes)
}

#[cfg(windows)]
fn map_windows_error(error: windows::core::Error) -> DomainError {
    DomainError::new(
        ErrorCode::CapabilityDenied,
        "Windows Shell 未能保证把文件移入回收站；没有使用永久删除降级。",
    )
    .with_diagnostic(format!("HRESULT 0x{:08X}: {error}", error.code().0 as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_flags_are_exact_and_exclude_nuke_warning() {
        assert_eq!(REQUIRED_RECYCLE_FLAGS, 0x0018_0410);
        assert_eq!(REQUIRED_RECYCLE_FLAGS & 0x0000_4000, 0);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an explicit disposable NTFS parent and moves fixtures through the real Windows Recycle Bin"]
    fn real_sta_worker_recycles_pair_and_restores_identical_content() {
        use std::{
            fs,
            path::Path,
            sync::atomic::{AtomicUsize, Ordering},
        };

        use crate::safety::TestRoot;

        struct Callbacks {
            callbacks: AtomicUsize,
        }
        impl RecycleCallbacks for Callbacks {
            fn pre_delete(&self, asset: &ShellAsset) -> Result<(), DomainError> {
                if !asset.absolute_path.is_file() {
                    return Err(DomainError::new(
                        ErrorCode::IdentityMismatch,
                        "PreDeleteItem 测试文件已缺失。",
                    ));
                }
                Ok(())
            }
            fn post_delete(
                &self,
                asset: &ShellAsset,
                hr_delete: i32,
                evidence: Option<Vec<u8>>,
            ) -> Result<(), DomainError> {
                if hr_delete < 0 || evidence.as_ref().is_none_or(Vec::is_empty) {
                    return Err(DomainError::new(
                        ErrorCode::IdentityMismatch,
                        "PostDeleteItem 缺少成功和非空回收对象证据。",
                    ));
                }
                if asset.absolute_path.exists() {
                    return Err(DomainError::new(
                        ErrorCode::IdentityMismatch,
                        "PostDeleteItem 后原路径仍存在。",
                    ));
                }
                self.callbacks.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let parent = std::env::var_os("FPM_RECYCLE_NTFS_TEST_PARENT")
            .map(PathBuf::from)
            .expect("set FPM_RECYCLE_NTFS_TEST_PARENT to a disposable NTFS directory");
        let holder = tempfile::Builder::new()
            .prefix("fpm-recycle-real-")
            .tempdir_in(parent)
            .unwrap();
        let root = TestRoot::create_in(holder.path()).unwrap();
        let jpeg = root.path().join("PAIR.JPG");
        let raf = root.path().join("PAIR.RAF");
        let jpeg_bytes = b"phase-seven-real-jpeg".to_vec();
        let raf_bytes = b"phase-seven-real-raf".to_vec();
        fs::write(&jpeg, &jpeg_bytes).unwrap();
        fs::write(&raf, &raf_bytes).unwrap();
        let callbacks = Arc::new(Callbacks {
            callbacks: AtomicUsize::new(0),
        });
        let worker = StaRecycleWorker::spawn().unwrap();

        let result = worker
            .recycle_and_restore_for_phase_eight(
                ShellPhoto {
                    assets: vec![
                        ShellAsset {
                            attempt_asset_id: vec![1; 16],
                            absolute_path: jpeg.clone(),
                        },
                        ShellAsset {
                            attempt_asset_id: vec![2; 16],
                            absolute_path: raf.clone(),
                        },
                    ],
                },
                callbacks.clone(),
            )
            .unwrap();

        assert_eq!(result.perform_operations_result, 0);
        assert!(!result.operations_aborted);
        assert_eq!(callbacks.callbacks.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read(jpeg).unwrap(), jpeg_bytes);
        assert_eq!(fs::read(raf).unwrap(), raf_bytes);
        assert!(TestRoot::verify(Path::new(root.path())).is_ok());
    }
}
