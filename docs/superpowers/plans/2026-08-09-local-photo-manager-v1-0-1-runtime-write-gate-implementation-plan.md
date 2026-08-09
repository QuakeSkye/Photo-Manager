# Fuji 照片管理器 v1.0.1 运行时写入安全门实施计划

日期：2026-08-09
依据：`docs/superpowers/specs/2026-08-09-local-photo-manager-runtime-write-gate-design.md`

## 1. 目标与不可变约束

本计划只修复并交付 v1.0.1 的 Import/Recycle 默认启用与统一运行时安全门，然后针对唯一的准确安装候选重新执行完整阶段 8。它不增加搜索、永久删除、多图库或其他新功能。

以下约束在全部任务中不可放宽：

- 测试数据只允许位于新的 `G:\test\local-photo-manager-phase8\<run-id>` 专属 Run；不得在 D 盘测试，不得访问或修改 `G:\test\Fuji-XM5`。
- G 盘实际 allocated bytes 不得超过 300 GB；1 TiB 指逻辑规模，必须同时报告真实分配量和真实读取量。
- 不对用户唯一照片执行 Import、Recycle、清理或卸载数据测试。
- 不 push、不发布远端；最终只创建本地 Git 提交。
- v1.0.0 的候选、安全报告和 27/27 结论立即视为历史且不能授权 v1.0.1。
- 物理动作前必须通过 gate；物理动作返回后，观察结果和不可变 Fact 的 COMMIT 不得再由 gate 拒绝。
- writer 无法 COMMIT 物理结果时保留已提交 Intent、Reservation、Lease 和非终态 Attempt，立即停止，不伪造结果，由重启恢复协调重新观察。
- `require_feature()` 与 `revalidate()` 每次都在同一 gate 锁内同步检查实时 writer 健康、数据库模式、fault latch、派生状态、instance、feature 和 generation；generation 不能替代实时健康检查。
- 最终候选构建后不得再改变源码、锁文件、schema、迁移或构建配置；任何重建都使安装后全部证据失效。

每项代码任务均采用 red-green-refactor：先加入能失败的测试并保存失败证据，再实现最小变更，最后运行相关回归。测试名称可在实现时按模块惯例微调，但断言语义不得缩减。

## 2. 任务一：冻结 v1.0.0 失效结论与建立 v1.0.1 证据命名空间

修改：

- `docs/superpowers/reports/phase-eight-final-report.md`
- `docs/superpowers/reports/phase-eight-release-notes.md`
- `docs/superpowers/reports/phase-eight-known-limitations.md`
- 新建 `artifacts/phase-eight-v1.0.1/README.md`
- `scripts/phase-eight-report.mjs`
- `scripts/phase-eight-quality-gate.ps1`

步骤：

1. 先为报告生成器增加失败测试/fixture：如果输出仍引用 v1.0.0 的 installer/EXE hash、旧 Run ID 或旧准入结论，v1.0.1 质量门必须失败。
2. 在旧总报告和发布说明首屏加入明确的 superseded 标记：v1.0.0 只保留历史证据，真实 Import/Recycle 不得依其结论启用。
3. 新证据全部写入 `artifacts/phase-eight-v1.0.1`；不改写旧原始 JSON、截图或 hash。
4. 质量门要求新报告的 product version、candidate ID、Run ID 和绑定 hash 均属于 v1.0.1 命名空间。

验证：

- `node scripts/phase-eight-report.mjs --help`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/phase-eight-quality-gate.ps1`

预期：在新候选证据尚未生成时质量门明确失败，并说明缺少 v1.0.1 证据；不得回退采用 v1.0.0 文件。

## 3. 任务二：实现一致快照 RuntimeSafetyGate

修改：

- 新建 `src-tauri/src/safety/runtime_gate.rs`
- `src-tauri/src/safety/gates.rs`
- `src-tauri/src/safety/mod.rs`
- `src-tauri/src/domain/mod.rs`（仅在需要稳定 gate 错误码时）
- `src-tauri/src/db/connection.rs`（只暴露必要的只读实时健康信息）

先写失败测试：

1. `healthy_ready_gate_opens_import_and_recycle_without_admission`。
2. `require_feature_synchronously_latches_unannounced_writer_poison`：仅把 writer health atomic 设为 false，不发送 generation 通知，下一次 `require_feature()` 必须拒绝并锁存故障。
3. `revalidate_synchronously_rejects_unannounced_writer_poison`：已发 permit 在同样条件下立即拒绝。
4. 错误 instance ID、错误 feature、旧 generation、非 `ReadWrite`、未 ready、fault latch 分别拒绝。
5. 任一共享健康位失败同时关闭 Import/Recycle；同一进程内恢复健康位也不得清除 latch。
6. 并发线程在 ready/health/generation 变化时循环申请许可，证明检查只产生完整的开放或关闭快照，不会拼接出虚假 Open。
7. `SafetyStatus` 和 `require_feature()` 来自同一快照，phase 为 v1.0.1，关闭原因码稳定且不包含路径。

实现：

1. `RuntimeSafetyGate` 持有进程唯一 instance ID、受单一 mutex 保护的核心状态，以及 `Arc<Database>` 或等价的实时数据库健康来源。
2. `SafetyPermit` 字段保持私有，只能由 gate 构造，绑定 instance ID、`FeatureGate` 和 generation。
3. `require_feature(feature)` 在持锁期间 acquire-read writer health、读取当前 database mode，并校验 ready/open、fault latch 和所有共享健康位；发现 writer 失效时在同一临界区锁存 fault、递增 generation 后返回拒绝。
4. `revalidate(permit, expected_feature)` 在同一临界区执行上述实时检查，再校验 permit instance、feature 和 generation。
5. 所有改变可用性的更新和 generation 递增都在同一锁内；公开快照同样由该锁产生。
6. 保留 `BuildFingerprint`、`CandidateManifest` 和发布证据类型，但删除它们对生产运行时开放状态的影响；旧 `GateEvaluator` 若无其他审计用途则缩减为发布证据校验器，不能成为第二个生产 gate。

验证：

- `npm run test:rust -- safety::runtime_gate`
- `npm run test:rust -- safety::gates`

## 4. 任务三：启动协调、实时故障锁存与唯一服务注入

修改：

- `src-tauri/src/lib.rs`
- `src-tauri/src/safety/runtime_gate.rs`
- `src-tauri/src/catalog/mod.rs` 或实际恢复协调入口
- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/recycle/mod.rs`
- `src-tauri/src/cache/mod.rs`
- 相应 Rust 测试模块

先写失败测试：

1. gate 在数据库打开后仍为 startup-closed，只有迁移/完整性、Import/Recycle 未终结任务观察协调、Root/Cache 身份校验和 runner 初始化全部完成后才能 `mark_ready()`。
2. 任一步骤失败均保持关闭并给出稳定原因；服务不得接收不同 gate 实例。
3. SQLite `FULL/IOERR/CORRUPT` 或 writer poisoning 会锁存共享故障；普通单文件缺失、no-overwrite 冲突或用户取消不会污染全局 gate。
4. 启动恢复在 gate 未开放时可以观察并提交先前物理动作的结果 Fact，但不得开始任何新的 rename、partial 创建/删除或 Shell 回收。

实现：

1. 在 `lib.rs` 中数据库创建后立即创建唯一 `Arc<RuntimeSafetyGate>`，以 closed/startup 状态注入 ImportPreflight、ImportExecution、Recycle 及需要上报共享故障的服务。
2. 把构造期间隐式执行的恢复拆为显式 startup reconciliation，使顺序可测；恢复只允许观察和提交既有动作结果，不启动新物理动作。
3. 所有恢复和身份检查完成后一次性 `mark_ready()`；失败调用 `latch_fault(reason)`，不得因按钮重试在同进程重新开放。
4. 将同一 gate 实例注册为 Tauri state；删除独立、可能分叉的 `SafetyState` 状态来源。
5. 对严重数据库错误调用统一分类器；即使分类通知尚未运行，任务二的实时 writer 检查仍必须独立阻止许可。

验证：

- `npm run test:rust -- safety`
- `npm run test:rust -- db::tests`
- `npm run test:rust -- import_execution`
- `npm run test:rust -- recycle`

## 5. 任务四：封闭全部命令与内部入口旁路

修改：

- `src-tauri/src/commands/import_preflight.rs`
- `src-tauri/src/commands/import_execution.rs`
- `src-tauri/src/commands/recycle.rs`
- `src-tauri/src/import_preflight/mod.rs`
- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/recycle/mod.rs`
- 新建或扩展命令授权测试

先写失败测试：

1. gate 关闭时 `select_import_source`、`start_import_preflight`、`start_import`、`retry_import`、`start_recycle`、`retry_recycle` 都在零服务调用、零数据库写、零文件动作处失败。
2. 查询状态/报告/确认摘要和取消剩余项在 gate 关闭时继续可用。
3. 命令取得许可后立刻改变 generation，服务入口必须拒绝旧 permit；直接调用服务也无法省略 permit。
4. 错 feature 的 permit 不能跨 Import/Recycle 使用。

实现：

1. 六个公开写命令从同一 Tauri `RuntimeSafetyGate` 获取 feature permit，并把 permit 作为必填值传入服务。
2. `ImportPreflightService::select_source/run_preflight`、`ImportExecutionService::start/retry`、`RecycleService::start/retry` 的生产签名必须要求 permit；不得保留无 gate 的公开同名旁路。
3. 取消和只读查询不要求 feature permit，但原有数据库健康、身份和 lease 约束保持不变。
4. phase-eight self-test 也只能通过真实服务+真实 gate；测试 fixture 可显式创建健康 TestRoot gate，不能调用无授权后门。

验证：

- `npm run test:rust -- commands`
- `npm run test:rust -- import_preflight`
- `npm run test:rust -- import_execution`
- `npm run test:rust -- recycle`

## 6. 任务五：Import 物理动作/结果提交边界

修改：

- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/import_execution/tests.rs`
- `src-tauri/src/safety/fault_injection.rs`
- `src-tauri/src/phase_eight/safety_suite.rs`

先写失败测试：

1. 创建 TaskIntent、Attempt、Reservation、Lease 前 permit 失效时没有任何新任务状态。
2. partial `CREATE_NEW` 前 gate 关闭时不创建文件；命令检查后失效的旧 permit 无法越过该边界。
3. partial 已创建/写入后 gate 关闭，当前动作的 ownership/claim/观察状态仍尝试 COMMIT，但 no-replace publish 不得开始。
4. `publish_no_replace` 成功后、`import_published_fact` COMMIT 前关闭 gate，Fact、intent Published、ownership consumed 和 claim release 仍必须在同一事务提交；下一 step 不得开始。
5. 上述时点 writer COMMIT 失败时，Prepared/Publishing intent、Reservation、Lease、ownership/claim 和非终态 Attempt 保留，既不标成功也不释放；重启 `reconcile_publish_intents` 观察 final/partial 身份后只追加一次 Fact，绝不第二次 rename/overwrite。
6. app-owned partial 删除前 gate 关闭时不删除；删除动作返回后 gate 关闭仍提交 `import_partial_cleanup_fact`。writer 失败时保留 Cleaning 状态供启动观察。
7. 多文件任务在第一文件 Fact 提交后 gate 关闭，第二文件的 create/publish 均未发生；源卷逐字节不变，已有目标逐字节不变。
8. partial 已由 `CREATE_NEW` 创建、但把 Planned ownership 推进为 Owned/claim 的 COMMIT 失败时，重启必须使用预提交的 partial path reservation、nonce 和文件身份重新观察；只允许证明并接管当前 incarnation 或进入 NeedsAttention，不得按路径猜测所有权、覆盖或删除。

实现：

1. 在 start/retry 的任务状态创建前 revalidate 初始 permit。
2. 每个 step 的新物理动作前从同一 gate 获取/复核 Import permit：partial create/copy、no-replace publish、owned partial cleanup 分别形成明确边界。
3. 在物理调用返回以后直到观察结果事务结束的代码段禁止调用 gate；Fact 事务只受 writer/fence/身份约束。
4. Fact COMMIT 成功后再检查 gate，决定是否进入下一物理动作。gate 关闭产生安全停止状态，不把已完成文件回滚或改写为失败。
5. writer 错误直接冒泡并停止循环，沿用非终态 recovery 表达；禁止 `mark_step_error`、close attempt 或 release lease 覆盖结果不明状态。
6. 加入精确 fault hook，仅用于把 gate 关闭安排在物理动作返回与 Fact COMMIT 之间；生产路径不引入 sleep 或竞态猜测。
7. 扩展启动恢复以覆盖 Planned partial 的创建后/COMMIT 前窗口；恢复必须先证明 reservation、nonce、contained path 和稳定文件身份，证据不足时保持所有权并进入 NeedsAttention。

验证：

- `npm run test:rust -- import_execution::tests`
- `npm run test:rust -- phase_eight::safety_suite`

## 7. 任务六：Recycle 单文件 Shell 粒度与回调 Fact 语义

修改：

- `src-tauri/src/recycle/mod.rs`
- `src-tauri/src/recycle/shell.rs`
- `src-tauri/src/recycle/tests.rs`
- `src-tauri/src/safety/fault_injection.rs`
- `src-tauri/src/phase_eight/safety_suite.rs`

先写失败测试：

1. Batch/Attempt/Reservation/Lease 建立前 permit 失效时零 Shell 调用。
2. 每次交给 STA worker 的 `ShellPhoto` 只含一个 `ShellAsset`；配对照片的两个 asset 必须是两个可中断物理动作。
3. 每个 asset 最终身份复核后、Shell 调用前 gate 关闭时该 asset 不进入 worker。
4. 第一 asset 已移入回收站、`DeletionRecycledFact` COMMIT 前关闭 gate，回调仍提交观察结果和 Fact；第二 asset 从未提交给 worker。
5. Shell 成功但 writer COMMIT 失败时，asset 保持 Recycling、Attempt/Reservation/Lease 保持非终态，停止任务且不调用第二 asset；重启观察 source/recycle evidence 后收敛到唯一 Fact 或 NeedsAttention。
6. paired Photo 的部分回收结果准确报告，不把尚未开始的 asset 假报为回收成功、普通失败或永久删除。
7. 强制验证 `FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT` 和 Recycle Bin evidence；任何永久删除路径仍被拒绝。

实现：

1. 将 `execute_attempt` 从按 Photo 预装全部 assets 改为按 asset 顺序执行；`prepare_photo`、summary 和 reconciliation 拆成单 asset 边界，同时保留 Photo 汇总。
2. 每个 asset 在 COMMIT Validating/Recycling 状态和最终身份复核后，立即 revalidate Recycle permit，再创建只含该 asset 的 Shell 请求。
3. `post_delete` 在 Shell 动作返回后无条件尝试 writer 事务；该路径不得读取 permit、generation 或 gate 状态。
4. 结果事务成功后再获取/复核 permit，决定是否提交下一 asset；writer 失败时不运行 reconcile/close 逻辑去覆盖非终态状态。
5. 启动 reconcile 只观察既有动作，不自动重放 Shell。

验证：

- `npm run test:rust -- recycle::tests`
- `npm run test:rust -- phase_eight::safety_suite`
- Windows TestRoot 上的真实回收站恢复测试（仅生成数据）

## 8. 任务七：UI 使用真实安全状态且不成为授权边界

修改：

- `src/shared/commands/safety.ts`
- `src/app/App.tsx`
- `src/app/App.test.tsx`
- `src/features/import/ImportView.tsx`
- `src/features/import/ImportView.test.tsx`
- `src/features/pending-delete/PendingDeleteView.tsx`
- `src/features/settings/SettingsView.tsx`
- `src/shared/ui/TopBar.tsx`
- 相关 CSS

先写失败测试：

1. `GateState` 支持 `open | closed`，健康状态显示 v1.0.1 且 Import/Recycle 按钮可用。
2. gate 关闭时 Import 来源选择、预检、开始/重试和 Recycle 开始/重试按钮禁用，关闭原因可见。
3. 浏览、报告、状态读取、取消剩余和撤销标记仍可用。
4. 前端先看到 Open、Rust 随后拒绝时正确刷新为 Closed 并保留任务状态，不把 UI 缓存当授权。
5. 设置页“写入安全状态”可通过键盘到达，焦点、非颜色提示、对比度和窄窗口无回归。

实现：

1. App 启动读取 `get_safety_status`，在写操作返回安全门错误和有限周期刷新时更新同一只读状态。
2. 把 Import/Recycle gate 状态下传到对应执行入口；禁用只用于 UX，Rust command 仍是最终边界。
3. 设置页显示 global/import/recycle、数据库模式和稳定关闭原因；不显示敏感绝对路径。

验证：

- `npm test -- src/app/App.test.tsx src/features/import/ImportView.test.tsx`
- `npm run test:ui`
- `npm run build`

## 9. 任务八：版本 1.0.1、锁文件与源码质量门

修改：

- `package.json`
- `package-lock.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`
- `src-tauri/tauri.conf.json`
- 必要的报告协议版本常量

步骤：

1. 统一 npm、Cargo、Tauri 版本为 1.0.1，保持 identifier、NSIS target 和 `currentUser` 默认安装模式不变，不加入 D 盘定制安装模板。
2. 只用包管理器更新锁文件中的根包版本，不随意升级依赖。
3. 运行格式、lint、TypeScript、全部前后端测试、Clippy 和安全审计。
4. 生成 source-quality report，记录 Git commit、锁文件 hash、schema version/checksum、迁移逐项与链 checksum。

验证：

- `npm run format:check`
- `npm run lint`
- `npm test`
- `npm run test:e2e`
- `npm run test:rust`
- `npm run clippy`
- `npm run safety:audit`
- `git diff --check`

## 10. 任务九：扩展安装候选与 gate 故障测试工具

修改：

- `src-tauri/src/phase_eight/mod.rs`
- `src-tauri/src/phase_eight/safety_suite.rs`
- `src-tauri/src/phase_eight/candidate_binding.rs`
- `scripts/run-phase-eight.ps1`
- `scripts/phase-eight-ui-check.mjs`
- `scripts/phase-eight-quality-gate.ps1`
- `scripts/phase-eight-report.mjs`

先写失败验证：

1. self-test 传入构建目录 EXE、错误 EXE hash、错误 installer hash、旧 lock/schema/migration checksum 或旧 v1.0.0 manifest 时拒绝。
2. 未提供 runtime gate 并发、物理动作后 Fact、writer 结果不明和重启收敛证据时最终报告失败。
3. Run root 不在指定 G 路径、marker/identity 不匹配、allocated 上限或保护快照变化时立即失败。

实现：

1. 候选自测新增 gate suite，直接覆盖任务二、五、六的关键边界；套件使用生产服务实现和受保护 TestRoot。
2. 每次 self-test 启动前后都复核当前进程 image path、EXE hash、candidate manifest 和 Run root identity。
3. 报告增加“动作前 gate / 动作后 Fact / writer 不明状态 / restart reconciliation”四组原始证据，不以普通单元测试数量代替。
4. 所有写入 G 的脚本在每个大步骤前后检查 allocated bytes 和 `Fuji-XM5` 保护快照。

验证：

- `npm run test:rust -- phase_eight`
- `npm run test:phase8:preflight`
- 故意传入旧 v1.0.0 hash，确认候选脚本 fail closed

## 11. 任务十：构建一次候选并验证 current-user 升级安装

前置条件：任务一至九的源码质量门全部通过，工作树中除明确列入候选的文件外无变更。此时冻结源码、locks、schema、迁移和构建配置。

步骤：

1. 记录冻结 Git commit 和全部输入 checksum。
2. 只构建一次 v1.0.1 NSIS candidate：`npm run tauri -- build --bundles nsis`。
3. 立即记录 installer canonical path、size、SHA-256、Authenticode 状态和版本；创建 frozen candidate manifest。
4. 关闭旧应用，仅使用生成/可丢弃 AppData 验证 v1.0.0 到 v1.0.1 的 current-user 升级；从卸载注册信息定位安装目录，而不是假设路径。
5. 验证实际安装目录为当前用户 LocalAppData 默认产品目录，预期本机为 `C:\Users\15684\AppData\Local\Fuji 照片管理器`；确认不在 D 盘。
6. 验证 EXE、uninstaller、快捷方式、DisplayIcon、卸载项、单实例、文件版本与产品版本一致；记录已安装 EXE canonical path、file ID、size 和 SHA-256。
7. 执行一次受控卸载验证程序文件移除且可丢弃 AppData 保留，然后重新安装同一个 installer；重新定位并证明 EXE hash 完全相同。
8. 若任何文件需要修改或重建，丢弃本任务及后续全部证据，回到任务八重新冻结。

验证输出写入新 G Run，并只把小型压缩摘要复制回 `artifacts/phase-eight-v1.0.1`。

## 12. 任务十一：对准确安装 EXE 完整重跑阶段 8

使用新建且通过 marker、canonical containment、卷身份和 allocated 上限检查的 G Run。不得复用 v1.0.0 的 Run 或结果。

按固定顺序执行：

1. 100,000 Photo、200,000 Asset、至少 1 TiB logical bytes 规模、分页稳定性和恢复索引。
2. 机械硬盘 seek-penalty 证明、顺序/随机 I/O、浏览 p50/p95、取消传播、内存 working set、缓存上限和磁盘余量校准。
3. SQLite FULL/WAL/checkpoint、backup/restore、schema 1–8 逐级 migration、损坏只读模式、AppData 空间不足和 writer poisoning。
4. 完整 Import：故障注入、源卷零写入、已有照片逐字节不变、CREATE_NEW/no-replace、目标抢占、publish Fact、cleanup ownership、writer 结果不明和重启协调。
5. 完整 Recycle：单文件 STA、强制 flags、Shell 取消/失败/部分完成、永久删除禁止、DeletionRecycledFact、writer 结果不明、重启协调和 Windows 回收站真实恢复；仅使用生成数据。
6. Cache：容量/余量、逐文件所有权、Intent/Fact、替代对象竞态、未知文件和父目录不变。
7. UI：100%/125%/150% DPI，1440×900、1280×720、1100×700，键盘、焦点、对比度、overflow、健康 gate Open 和注入共享故障后的双 Closed。
8. 每套件前后复核 installed EXE 和所有 frozen input checksum；任一漂移立即停止并作废候选证据。

主命令：

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-phase-eight.ps1 -Suite all -InstalledExe <准确安装路径> -RunRoot <新G盘Run>`
- `node scripts/phase-eight-ui-check.mjs --installed-exe <准确安装路径> --run-root <新G盘Run>`

## 13. 任务十二：27 条验收、最终报告与本地交付提交

修改/生成：

- `artifacts/phase-eight-v1.0.1/*.json`
- `docs/superpowers/reports/phase-eight-final-report.md`
- `docs/superpowers/reports/phase-eight-known-limitations.md`
- `docs/superpowers/reports/phase-eight-release-notes.md`
- `docs/superpowers/reports/real-photo-enable-checklist.md`

步骤：

1. 将原 27 条产品验收逐条映射到同一 v1.0.1 frozen candidate 的直接机器证据和必要人工证据。
2. 增加设计第 11 节十项补充证明，特别单列：实时 writer 检查、错误 permit 拒绝、动作后 Fact 不受 gate 拒绝、writer 失败的非终态保留与重启收敛、无 IPC/runner/retry/recovery 旁路。
3. 发布审计继续绑定 installer SHA-256、installed EXE SHA-256、package-lock、Cargo.lock、schema、迁移逐项/链以及全部报告 checksum；这些只证明候选，不作为用户运行时确认。
4. 缺陷表要求 unresolved Critical=0、High=0；skip、旧证据、机器介质不符、hash 漂移或人工矩阵缺项均视为不通过。
5. 已知限制明确 Windows Recycle 非原子、回收站和 SQLite backup 都不是照片备份、1 TiB logical 与实际分配/读取的区别，以及无法安全自动验证的环境项。
6. “真实照片启用前清单”不再要求软件内人工解锁，但要求用户单独确认备份、目标目录、来源只读策略、回收站容量和恢复演练；清单本身不能绕过自动 gate。
7. 运行最终 verifier，再次读取准确安装 EXE 并重新计算全部 hash。验证后不得重建。
8. 仅暂存本计划涉及的跟踪文件及新 v1.0.1 证据；明确排除用户的 `.superpowers/**` 和 `scripts/ui-check.mjs` 未跟踪文件。
9. 创建本地 Git 交付提交，不 push、不发布远端。最终保留已验证的 v1.0.1 current-user 安装状态。

最终验证：

- `npm run test:phase8:quality`
- `node scripts/phase-eight-report.mjs`
- `git diff --check`
- `git status --short`

## 14. 最终准入判定

只有同时满足以下条件才可标记完成：

- 原 27 条产品验收为 27/27；
- 设计补充十项全部有准确候选的直接证据；
- unresolved Critical=0、High=0；
- installed EXE、installer、locks、schema、迁移和报告 checksum 全部匹配 frozen manifest；
- 完整 Import/Recycle、故障注入、源卷零写入、已有照片不变、no-overwrite、缓存所有权、永久删除禁止和回收站恢复全部通过；
- G 盘实际分配量始终不超过 300 GB，D 盘没有测试数据，`G:\test\Fuji-XM5` 保护快照不变；
- 最终验证后没有重新构建；
- 本地 Git 交付提交成功且没有远端 push/release。

任何一项不满足都必须报告为未完成，保留准确失败证据，不得用旧报告、设计预期或单元测试替代候选实测。
