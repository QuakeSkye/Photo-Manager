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
- Windows 程序文件沿用 v1.0.0 的 Tauri `currentUser` 默认安装位置；
- 重新构建、安装并完整重跑阶段 8。

本变更不增加搜索、手动配对、完整 RAW 解码、多图库、手动缓存清理、永久删除或其他 v1 范围外功能。程序文件和应用数据继续使用各自的 LocalAppData 子目录；D 盘只保留源码与构建工作区，不作为安装位置或测试卷。规模与安全测试数据仍只允许位于 `G:\test\local-photo-manager-phase8\<run-id>`。

## 3. RuntimeSafetyGate

新增由 Tauri 管理的 `RuntimeSafetyGate`，它是 Import 和 Recycle 写权限的唯一生产来源。它包含：

- 每次进程启动唯一的 gate instance ID；
- 当前单调递增 generation；
- 当前进程内不可逆的 fault latch；
- 数据库模式和 writer 健康句柄；
- 启动恢复协调完成状态；
- job runner、reconciler 和 capability boundary 健康状态；
- 稳定、可显示但不泄漏敏感路径的关闭原因码。

gate 的可变核心状态由同一把锁保护，至少包含 instance ID、generation、启动/开放/故障状态、database mode、共享健康位和 fault reason。所有状态更新与 generation 变化在同一临界区完成。writer 健康句柄使用 acquire 语义实时读取；发现 writer 已中毒时，当前调用必须在同一临界区设置 fault latch 并递增 generation，不能等待异步通知线程。

健康进程中 Import 和 Recycle 默认 `open`，不读取 CandidateManifest、FeatureAdmission 或用户确认。任一共享健康项异常时，两项同时 `closed`。同一进程内 fault latch 只能从健康变为故障，不能由重试按钮重新打开；必须完成安全停止并重新启动应用。重启后也必须重新通过数据库、迁移、恢复协调和 Root 检查，不能盲目恢复开放。

CandidateManifest 和 FeatureAdmission 类型继续服务于发布测试和证据审计，但不参与生产用户授权。

## 4. 短生命周期安全许可

`require_feature(Import|Recycle)` 返回不可伪造的短生命周期 `SafetyPermit`，绑定 gate instance ID、feature 和 generation。许可本身不替代 Root、卷、路径、文件身份或 reservation/lease 证明。

每次 `require_feature()` 和 `revalidate()` 都必须取得 gate 核心状态的一致快照，并同步验证全部条件：

- 当前派生状态为 `Open`；
- permit generation 与当前 generation 相同（`require_feature()` 生成当前值）；
- fault latch 未设置；
- writer 健康句柄当前仍为 `true`；
- database mode 当前仍为 `ReadWrite`；
- permit gate instance ID 与当前实例一致；
- permit feature 与请求的 feature 一致。

generation 只负责废止已经发出的许可，不能代替实时健康检查。任一条件不满足都不得发放或接受许可。

公开写命令在进入服务前取得许可；服务在关键物理边界调用 `revalidate(permit)`：

- Import 计划确认和 Attempt/Lease/Reservation 建立前；
- partial `CREATE_NEW` 前；
- partial 完整验证后、no-replace 发布前；
- Recycle Batch/Attempt/Lease/Reservation 建立前；
- 每个文件最终身份复核后、`IFileOperation` 调用前；
- retry 开始前。

`SafetyPermit` 只授权开始新的物理动作，绝不授权或阻止物理动作后的结果记录。no-replace rename、回收站动作或其他物理动作一旦返回，系统必须无条件尝试提交观察到的结果和相应不可变 Fact，即使 gate 已关闭、generation 已变化或原 permit 已失效。结果 COMMIT 路径不得调用 `require_feature()` 或 `revalidate()`。

结果成功 COMMIT 后，再为下一文件或下一物理动作重新调用 `require_feature()`/`revalidate()`。gate 已关闭时停止后续动作，已记录事实保持不变。

如果 SQLite writer 本身已失效而无法提交结果，则不能假装动作失败或成功：保留动作前已经 COMMIT 的 Intent、OperationReservation、FileWriteLease 和非终态 Attempt，停止任务，不开始下一文件，并使共享 gate 保持关闭。重启后恢复协调重新观察正式路径、partial 或回收结果，再追加可证明的 Fact 或进入 `NeedsAttention`。无法写入 SQLite 时，“结果不明”由预提交 Intent 加缺失的终态 Fact 表达，不能依赖一次同样无法持久化的补写。

Recycle 的可中断粒度必须是单个物理文件：每次只向 STA worker 提交一个已经复核的 Shell 对象，不得预先把剩余文件全部放入不可撤回的 Shell 队列。当前文件的 Shell 结果完成并尝试 COMMIT 后，服务重新检查 gate，再决定是否提交下一文件。配对 Photo 因此允许并准确报告部分回收结果。

generation 一旦变化，旧许可立即失效。正在执行的不可中断系统调用不被强行破坏；完成当前物理动作及其结果提交尝试后必须停止后续文件，并记录准确的 `Partial`、`RetryableFailed` 或 `NeedsAttention`。

## 5. 命令授权矩阵

| 操作 | gate 行为 |
|---|---|
| 查询图库、预览、报告、状态 | gate 关闭时仍允许只读访问 |
| Import 来源选择与永久 deny-write 注册 | 必须通过 Import gate |
| Import 只读预检及其持久化事实 | 必须通过 Import gate |
| Import 开始、继续、重试 | 命令入口和每次新物理动作前重复检查 Import permit；动作后结果 COMMIT 不受 gate 拒绝 |
| Import 取消、查看恢复摘要 | gate 关闭时仍允许安全停止和只读查看 |
| `D/R` 标记与撤销 | 继续由数据库健康和 Root/Photo 身份规则约束；不得执行文件动作 |
| Recycle 确认摘要与状态 | gate 关闭时允许只读查看 |
| Recycle 开始与重试 | 命令入口和每文件 Shell 动作前重复检查 Recycle permit；Shell 结果 COMMIT 不受 gate 拒绝 |
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

## 8. Windows 安装路径

v1.0.1 沿用 v1.0.0 的 Tauri NSIS `currentUser` 默认安装方式：

- 保持现有产品 identifier 和安装模式，以便 NSIS 识别并升级当前 v1.0.0 用户级安装；
- 本机预期程序目录为 `C:\Users\15684\AppData\Local\Fuji 照片管理器`；其他 Windows 用户使用其对应的 `%LOCALAPPDATA%\Fuji 照片管理器`；
- 不增加自定义 NSIS template，不把程序文件改装到 D 盘；
- 升级前关闭运行中的旧应用；迁移/卸载旧程序文件不得删除用户数据库、报告、备份或设置；
- EXE、uninstaller、快捷方式、DisplayIcon 和卸载注册信息必须一致指向实际 current-user 安装目录；
- 安装测试从注册信息和安装目录定位 EXE，再验证 canonical path、文件身份、版本和 SHA-256，不能使用 `target\release` 产物代替。

SQLite、日志、备份和默认缓存仍由 Tauri `app_local_data_dir` 解析到应用数据 LocalAppData 目录。程序安装目录与应用数据目录不能混作图库、ImportSource 或 CacheRoot。`FPM_TEST_ROOT` 只允许受 TestRoot 标记保护的阶段测试进程使用，不能改变生产默认 AppData。

## 9. 测试策略

先增加失败测试，再实现最小变更：

### 9.1 Gate 与命令

- 健康启动默认同时开放 Import/Recycle；
- 每个公开写命令在 gate 关闭时零服务调用、零文件动作；
- 命令检查后注入 generation 变化，关键服务边界拒绝旧 permit；
- 分别伪造错误 instance ID、错误 feature 和旧 generation，`revalidate()` 全部拒绝；
- writer 健康句柄在没有 gate 通知的情况下先变为 `false`，紧接着调用 `require_feature()` 和 `revalidate()`，两者必须同步发现并拒绝；
- database mode 非 `ReadWrite`、fault latch 已设置或派生状态非 `Open` 时不得发放许可；
- 并发改变健康状态和 generation，任何一次检查只能观察到锁保护的一致 gate 快照，不能组合出虚假的开放状态；
- 任一共享故障同时关闭两项，并在同一进程保持关闭；
- 普通 Attempt 错误不错误污染共享 gate；
- Import no-replace rename 成功后、`ImportPublishedFact` COMMIT 前关闭 gate，Fact 仍必须提交，下一文件不得开始；
- Windows 回收当前文件后、`DeletionRecycledFact` COMMIT 前关闭 gate，Fact 仍必须提交，下一文件不得交给 STA worker；
- 物理动作成功但 writer 无法 COMMIT 时，预提交 Intent、Reservation、Lease 和非终态 Attempt 保留；不得开始下一文件，重启协调按重新观察结果收敛；
- 所有内部 retry、恢复和后台入口覆盖同一授权矩阵。

### 9.2 UI

- 状态类型和文案与 Rust 一致；
- 健康时 Import/Recycle 执行入口可用；
- 关闭时按钮禁用、原因可见、浏览/报告/取消仍可用；
- 键盘、焦点、对比度、DPI 和窗口矩阵继续通过。

### 9.3 安装器

- 从已安装 v1.0.0 升级到 v1.0.1；只使用生成或可丢弃 AppData；
- GUI、静默和被动安装均验证为 current-user LocalAppData 安装，不得落入 D 盘工作区；
- 安装目录不可用时明确失败，不把构建目录中的 EXE 当作已安装候选；
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
3. 冻结源码、锁文件、schema、迁移和 NSIS 配置；
4. 构建唯一候选并记录 installer SHA-256；
5. 验证 v1.0.0 current-user 升级和默认安装路径；
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
4. gate 在物理动作完成后关闭时，已观察结果仍提交 Fact，而下一物理动作被阻止；
5. writer 无法提交物理结果时保留预提交所有权和结果不明状态，并由重启协调收敛；
6. `require_feature()`/`revalidate()` 同步检查实时 writer 健康、database mode、fault latch、状态、instance、feature 和 generation；
7. 没有前端、IPC、后台 runner、retry 或恢复旁路；
8. 程序文件准确安装在 current-user LocalAppData 默认目录，且 D 盘工作区不承载安装或测试数据；
9. 新候选所有证据绑定新的 installer/EXE/locks/schema/migrations checksum；
10. v1.0.0 证据被明确标记为历史且不能授权 v1.0.1。

任一项缺少直接证据均阻止完成。
