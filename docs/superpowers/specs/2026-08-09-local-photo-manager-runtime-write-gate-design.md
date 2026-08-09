# Fuji 照片管理器 v1.0.1：默认启用写功能与运行时安全门设计

日期：2026-08-09

## 1. 背景与结论修正

v1.0.0 阶段 8 候选虽然生成了绑定准确二进制、锁文件、schema、迁移和报告 checksum 的发布证据，但生产启动没有加载准入，`SafetyStatus` 固定把 Import/Recycle 报告为关闭，而公开 Rust 命令又直接调用写服务，没有强制检查该状态。这个分裂是高风险准入缺陷，因此 v1.0.0 的“27/27、未解决 High 为零、可供真实照片写入”的结论失效。

用户已明确改变产品要求：v1.0.1 不再需要人工确认或外部准入包；现有 v1 Import 和 Recycle 功能在运行环境健康时默认启用。自动故障门仍必须保留，并在数据库、Root 身份、恢复协调或能力边界异常时同时关闭两项写功能。

任何 v1.0.1 构建都会产生新的 EXE SHA-256。v1.0.0 的候选 manifest、安全报告、准入和测试矩阵只作为历史证据保留，不能授权或证明 v1.0.1。

## 2. 范围

本变更只包含：

- 默认启用已经实现的 v1 Import 与 Windows Recycle；
- 在每个 Rust 命令和物理文件动作边界强制执行统一运行时安全门；
- UI 显示同一安全门的真实状态和关闭原因；
- 版本升至 1.0.1；
- Windows 程序文件固定安装到 `D:\B\Fujifilm`；
- 重新构建、安装并完整重跑阶段 8。

本变更不增加搜索、手动配对、完整 RAW 解码、多图库、手动缓存清理、永久删除或其他 v1 范围外功能。SQLite、日志、数据库备份和默认缓存继续使用每个 Windows 用户的 LocalAppData；D 盘只存放程序文件，不作为测试卷。规模与安全测试数据仍只允许位于 `G:\test\local-photo-manager-phase8\<run-id>`。

## 3. RuntimeSafetyGate

新增由 Tauri 管理的 `RuntimeSafetyGate`，它是 Import 和 Recycle 写权限的唯一生产来源。它包含：

- 当前单调递增 generation；
- 当前进程内不可逆的 fault latch；
- 数据库模式和 writer 健康句柄；
- 启动恢复协调完成状态；
- job runner、reconciler 和 capability boundary 健康状态；
- 稳定、可显示但不泄漏敏感路径的关闭原因码。

健康进程中 Import 和 Recycle 默认 `open`，不读取 CandidateManifest、FeatureAdmission 或用户确认。任一共享健康项异常时，两项同时 `closed`。同一进程内 fault latch 只能从健康变为故障，不能由重试按钮重新打开；必须完成安全停止并重新启动应用。重启后也必须重新通过数据库、迁移、恢复协调和 Root 检查，不能盲目恢复开放。

CandidateManifest 和 FeatureAdmission 类型继续服务于发布测试和证据审计，但不参与生产用户授权。

## 4. 短生命周期安全许可

`require_feature(Import|Recycle)` 返回不可伪造的短生命周期 `SafetyPermit`，至少绑定 feature 和 generation。许可本身不替代 Root、卷、路径、文件身份或 reservation/lease 证明。

公开写命令在进入服务前取得许可；服务在关键物理边界调用 `revalidate(permit)`：

- Import 计划确认和 Attempt/Lease/Reservation 建立前；
- partial `CREATE_NEW` 前；
- partial 完整验证后、no-replace 发布前；
- 发布结果和不可变 Fact 提交前；
- Recycle Batch/Attempt/Lease/Reservation 建立前；
- 每个文件最终身份复核后、`IFileOperation` 调用前；
- 每个 Shell 回调结果 COMMIT 前；
- retry 开始前。

generation 一旦变化，旧许可立即失效。正在执行的不可中断系统调用不被强行破坏；完成当前安全边界后必须停止后续文件，并记录准确的 `Partial`、`RetryableFailed` 或 `NeedsAttention`。

## 5. 命令授权矩阵

| 操作 | gate 行为 |
|---|---|
| 查询图库、预览、报告、状态 | gate 关闭时仍允许只读访问 |
| Import 来源选择与永久 deny-write 注册 | 必须通过 Import gate |
| Import 只读预检及其持久化事实 | 必须通过 Import gate |
| Import 开始、继续、重试 | 命令入口和服务边界重复检查 Import permit |
| Import 取消、查看恢复摘要 | gate 关闭时仍允许安全停止和只读查看 |
| `D/R` 标记与撤销 | 继续由数据库健康和 Root/Photo 身份规则约束；不得执行文件动作 |
| Recycle 确认摘要与状态 | gate 关闭时允许只读查看 |
| Recycle 开始与重试 | 命令入口和每文件 Shell 边界重复检查 Recycle permit |
| Recycle 取消剩余项 | gate 关闭时仍允许安全停止 |

任一 Tauri command、后台恢复入口或内部重试路径都不得绕过同一 gate。前端禁用只是用户体验，不能充当安全边界。

## 6. 故障分类

以下故障触发共享 fault latch，并同时关闭 Import/Recycle：

- SQLite `FULL/IOERR/CORRUPT` 或 writer 耐久状态中毒；
- schema、迁移、FK 或完整性检查失败；
- 启动恢复协调无法解释未终结写任务；
- capability boundary 或全局 lease/reconciler 不变量失败；
- 当前 AppDataRoot、FujiLibrary、Active/Draining CacheRoot，或当前/未终结照片写任务涉及的 ImportSource 身份无法证明。

单个文件缺失、来源卷被拔出、普通 no-overwrite 冲突、用户取消 Shell 或某一回收对象失败属于 Attempt 级业务结果。它们停止或部分完成当前 Attempt 并生成报告，但只有在暴露共享不变量或耐久故障时才污染全局 gate。

所有已发布文件、已回收结果和不可变 Fact 在 gate 关闭后保持原样，不回滚、不删除、不改写。

## 7. 启动与 UI

启动顺序固定为：

1. 打开 SQLite 并验证 FULL/WAL、schema、迁移和完整性；
2. 恢复协调未终结 Import/Recycle、reservation、lease 和缓存意图；
3. 验证当前图库、缓存和永久只读来源卷策略；
4. 初始化所有服务和后台 runner；
5. 只有上述步骤成功才把共享健康状态设为 ready；
6. UI 读取 `RuntimeSafetyGate` 的同一快照。

`SafetyStatus` 的 phase 更新为 v1.0.1，`GateState` 支持 `open|closed`。设置页增加只读“写入安全状态”，显示共享健康、Import、Recycle 和关闭原因。执行按钮在关闭时禁用，但报告、取消、浏览和移除已有标记保持可用。前端不得缓存开放状态作为权限；每次 Rust 调用仍独立验证。

## 8. 固定 Windows 安装路径

程序文件固定安装到 `D:\B\Fujifilm`：

- 使用与锁定的 Tauri CLI 2.11.4 相匹配的自定义 NSIS template；[Tauri 官方 Windows Installer 文档](https://v2.tauri.app/distribute/windows-installer/)允许通过 `bundle.windows.nsis.template` 配置完整模板；
- 保持 `currentUser` 安装模式和现有产品 identifier，以便识别当前 v1.0.0 用户级安装；
- 安装页面不提供目录选择；复制前再次强制 `$INSTDIR` 为准确路径；
- `/D` 或其他命令行参数不得改变最终目录；
- D 盘不存在、目录无法创建、目标不是目录或权限不足时安装失败，不回退到 C 盘、LocalAppData 或其他位置；
- 升级前关闭运行中的旧应用；迁移/卸载旧程序文件不得删除 LocalAppData 用户数据库、报告、备份或设置；
- EXE 和 uninstaller 位于固定目录，快捷方式、DisplayIcon、卸载注册信息必须指向该目录；
- 安装测试验证路径的 canonical form、文件身份、版本、SHA-256 和注册信息。

SQLite、日志、备份和默认缓存仍由 Tauri `app_local_data_dir` 解析到 LocalAppData。`FPM_TEST_ROOT` 只允许受 TestRoot 标记保护的阶段测试进程使用，不能改变生产默认 AppData。

## 9. 测试策略

先增加失败测试，再实现最小变更：

### 9.1 Gate 与命令

- 健康启动默认同时开放 Import/Recycle；
- 每个公开写命令在 gate 关闭时零服务调用、零文件动作；
- 命令检查后注入 generation 变化，关键服务边界拒绝旧 permit；
- 任一共享故障同时关闭两项，并在同一进程保持关闭；
- 普通 Attempt 错误不错误污染共享 gate；
- 所有内部 retry、恢复和后台入口覆盖同一授权矩阵。

### 9.2 UI

- 状态类型和文案与 Rust 一致；
- 健康时 Import/Recycle 执行入口可用；
- 关闭时按钮禁用、原因可见、浏览/报告/取消仍可用；
- 键盘、焦点、对比度、DPI 和窗口矩阵继续通过。

### 9.3 安装器

- 从已安装 v1.0.0 升级到 v1.0.1；只使用生成或可丢弃 AppData；
- GUI、静默、被动及带 `/D` 参数的安装都只能落在 `D:\B\Fujifilm`；
- 路径不可用时明确失败且不产生另一位置的安装；
- 卸载后程序文件移除，生成的用户 AppData 保留；若无法在不接触现有真实 AppData 的环境中安全验证，明确列为限制；
- 最终重新安装同一候选，再针对该准确安装 EXE 运行全部候选测试。

### 9.4 完整候选重测

新建 G 盘专属 Run，重新完成：

- 100,000 Photo、200,000 Asset、至少 1 TiB 逻辑规模；
- 机械硬盘实际顺序/随机 I/O、浏览响应、内存和取消指标；
- 完整 Import、故障注入、源卷零写入、既有照片不变和 no-overwrite；
- 完整 Recycle、逐文件回调、永久删除禁止和回收站恢复；
- 缓存上限、余量、所有权、替代对象和未知文件保留；
- SQLite WAL/checkpoint、备份恢复、schema 1–8 迁移、损坏只读和 AppData 空间不足；
- 100%/125%/150% DPI、键盘、对比度和窗口缩放；
- 健康候选真实 UI 中 Import/Recycle 状态为开放；注入共享故障后两项同时关闭。

## 10. 发布顺序与完成条件

1. 统一版本为 1.0.1；
2. 全部源码测试、Clippy、格式和安全审计通过；
3. 冻结源码、锁文件、schema、迁移和 NSIS template；
4. 构建唯一候选并记录 installer SHA-256；
5. 验证 v1.0.0 升级和固定安装路径；
6. 准确安装 v1.0.1 并记录 EXE SHA-256；
7. 对该安装 EXE 完整重跑阶段 8；
8. 重新生成候选 manifest、全部报告 checksum、27 条矩阵和缺陷清单；
9. 任一重建使步骤 5–8 的证据全部失效；
10. 只有 27/27、未解决 Critical/High 为零、运行时 gate 测试全部通过后创建新的本地 Git 提交；
11. 不 push、不创建远端发布；最终保留 v1.0.1 安装状态。

所有测试继续只使用生成或明确可丢弃的数据，不对用户唯一照片执行 Import、Recycle、卸载数据验证或任何删除。

## 11. 验收补充

除原 27 条产品条件外，本变更必须直接证明：

1. 健康 v1.0.1 无需人工确认即可使用 Import/Recycle；
2. 状态显示、命令入口和服务物理边界使用同一 gate；
3. stale permit、共享故障和恢复失败不能执行新文件动作；
4. 没有前端、IPC、后台 runner、retry 或恢复旁路；
5. 程序文件准确安装在 `D:\B\Fujifilm`，AppData 不迁入 D；
6. 新候选所有证据绑定新的 installer/EXE/locks/schema/migrations checksum；
7. v1.0.0 证据被明确标记为历史且不能授权 v1.0.1。

任一项缺少直接证据均阻止完成。
