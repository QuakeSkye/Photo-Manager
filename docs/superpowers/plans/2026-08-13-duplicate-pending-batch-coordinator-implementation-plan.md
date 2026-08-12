# 重复文件批量加入待删除协调实施计划

## 1. 目标与基线

依据设计：`docs/superpowers/specs/2026-08-13-duplicate-pending-batch-coordinator-design.md`

目标是修复“全页面批量选择调用单照片接口”的语义冲突：前端一次提交全部选择，后端按逻辑照片分组，在后台完成可取消、fail-fast 的内容复证，全部成功后用一个 SQLite 事务建立多个独立 `DuplicateSubset` 标记，成功后进入待删除页。

必须保留：

- 一个逻辑照片一个活动 `DeletionMark`。
- 整批全有或全无，不允许部分成功。
- 加入待删除前和真正回收前的双重复证。
- 原始照片只读，最终处理只进入 Windows 回收站。
- 现有普通图库 D/R、待删除复核、回收、重复结果协调和孤立扫描恢复语义。

本计划不实施精确文件名预筛；该项由独立设计 `2026-08-12-duplicate-filename-prefilter-design.md` 处理。

## 2. 模块与文件边界

预计新增：

- `src-tauri/src/pending_delete/duplicate_batch.rs`
  - 批量请求/状态 DTO。
  - 后端任务状态机、进度和取消。
  - 整批预检、按照片分组、原子提交与幂等协调。
- `src-tauri/src/pending_delete/duplicate_batch_tests.rs`
  - 批量协调后端测试。

预计修改：

- `src-tauri/src/pending_delete/mod.rs`
  - 注册并重导出批量模块。
  - 将现有单照片解析、标记写入和请求哈希函数收窄为协调器可复用的内部 API。
  - 保留普通 D/R 与快照接口不变。
- `src-tauri/src/duplicate_review/proof.rs`
  - 增加单次同步流式待删/保留证明。
- `src-tauri/src/commands/pending_delete.rs`
  - 暴露开始、查询和取消批量任务命令。
- `src-tauri/src/lib.rs`
  - 注册新命令；共享现有 `PendingDeleteService`，不增加第二个数据库写入所有者。
- `src/shared/commands/duplicate-review.ts`
  - 增加批量任务 DTO 和三条 IPC 包装。
  - 停止从前端调用旧同步入口。
- `src/features/duplicate-review/DuplicateReviewView.tsx`
  - 弹框状态机、轮询、进度、取消、失败和成功跳转。
- `src/features/duplicate-review/DuplicateReviewView.test.tsx`
  - 弹框和路由回调测试。
- `src/shared/ui/Modal.tsx`
  - 解耦“确认忙碌”和“取消可用”，允许动态取消文案并独立禁止 Escape。
- `src/app/styles.css`
  - 批量验证进度、当前路径和弹框内错误样式。
- `src/app/App.test.tsx`
  - 仅在路由集成断言需要时更新。

不修改迁移 SQL、schema 版本或图库原始文件。

## 3. 任务一：冻结批量接口和状态契约

先在 Rust 与 TypeScript 中定义同构契约。

### Rust DTO

在 `duplicate_batch.rs` 定义：

- `StartDuplicatePendingBatchRequest`
  - `run_id`
  - `operation_id`
  - `selections`
- `DuplicatePendingBatchPhase`
  - `Validating`
  - `Committing`
  - `Completed`
  - `Failed`
  - `Cancelled`
- `DuplicatePendingBatchStatus`
  - 任务与操作 ID
  - 阶段
  - `total_files` / `validated_files`
  - `total_bytes` / `validated_bytes`
  - 当前待删/保留路径
  - 成功照片数、文件数、容量
  - 可选失败信息
- `DuplicatePendingBatchFailure`
  - 稳定错误类别
  - 用户消息
  - 待删/保留路径
  - 可选诊断

所有外部 DTO 使用 `camelCase`、`deny_unknown_fields`（请求）和确定性字段类型。

### TypeScript DTO

在 `duplicate-review.ts` 建立对应联合类型，避免用自由字符串解释状态。

### 先写测试

- Rust 序列化状态字段与前端约定一致。
- 空请求和无效 ID 在任务启动前拒绝。
- 终态明确不可取消，`validating` 明确可取消。

验证：

```powershell
node scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml duplicate_pending_batch_contract
npx tsc -b --pretty false
```

## 4. 任务二：实现单次同步流式内容证明

在 `duplicate_review/proof.rs` 增加面向批量任务的证明函数，保持现有 `stable_pair_proof` 供回收阶段和兼容路径使用，待批量验证稳定后再决定是否内部复用。

### 实现要求

1. 通过受约束 Root 解析两端路径，拒绝 reparse 与非普通文件。
2. 只读打开待删端与保留端。
3. 读取前验证 file ID、大小、mtime 和 hard-link 策略。
4. 两端使用固定 1 MiB 缓冲区同步读取。
5. 每块读取前检查 `AtomicBool` 取消标志。
6. 同时比较字节并分别计算 SHA-256。
7. 结束时要求长度、哈希和所有字节一致。
8. 在原句柄上验证读取后身份，再重新打开路径验证仍指向同一文件身份。
9. 返回可直接写入待删/保留快照的两端稳定观察。

### 测试先行

- 相同内容成功并返回相同完整哈希。
- 同大小不同内容失败。
- 读取中取消返回稳定取消错误。
- 读取前或读取后身份变化失败。
- 任一端缺失、reparse 或不可读失败。
- 测试读取计数证明首个失败后不会打开计划中的下一对文件。

验证：

```powershell
node scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml duplicate_pair_stream
```

## 5. 任务三：实现整批解析、预检和确定性分组

在 `duplicate_batch.rs` 实现无后台线程依赖的同步核心，为状态机调用。

### 规范化

- 解析 `runId`、批次 `operationId` 和所有 Asset/Set ID。
- 按稳定键排序选择，计算规范化整批请求哈希。
- 拒绝重复待删 Asset、待删端同时作为保留端、跨角色保留和重复选择。
- 从数据库重新加载每个选择，不信任前端提供的照片归属。
- 按 `PhotoId + photoVersion` 分组，每组按 AssetId 稳定排序。

### 整批预检

文件读取前一次完成：

- Run 为 `Completed`，当前修订与 RootBinding 仍一致。
- Set、Observation、Proof 和 Asset version 链一致。
- 每个等价集合至少保留一份未选成员。
- 保留端不在整批待删集合。
- 每个照片没有活动待删除标记。

任何一项失败立即返回首个稳定错误，验证器调用次数必须为零。

### 测试先行

- 两张照片可被确定性分成两组。
- 同一照片的 JPG/RAF 进入同一组。
- 请求顺序变化得到相同规范化哈希和分组顺序。
- 各类非法选择在读文件前 fail-fast。
- 既有活动 `WholePhoto` 或 `DuplicateSubset` 标记使整批失败。

验证：

```powershell
node scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml duplicate_pending_batch_preflight
```

## 6. 任务四：实现单事务多标记与幂等重放

复用现有 `create_duplicate_subset_mark` 的数据库约束，但拆分“插入操作事件”和“写标记内容”，使一个外层事务可以创建多张照片的标记。

### 子操作身份

- 使用固定领域分隔符，例如 `DuplicatePendingBatchPhoto/v1`。
- 从批次 `OperationId` 与 `PhotoId` 通过 SHA-256 确定性派生 16 字节子 `OperationId`。
- 固定 UUID variant/version 位或使用项目内明确记录的 16 字节 ID 规则，保证跨进程重试一致。
- 每张照片的子请求哈希覆盖 Run、Photo version 和该组全部选择。

### 单事务步骤

1. 再验证 Run、修订、照片/Asset 版本、活动标记和证明对应关系。
2. 按稳定照片顺序检查所有子操作的幂等状态。
3. 若全部子操作都已存在且请求/结果链一致，返回已有整批结果。
4. 若只存在部分子操作，返回安全链不一致错误，不补写。
5. 若不存在，写入全部子操作、标记、文件身份快照和 retention guard。
6. 事务提交前故障使全部回滚。

### 测试先行

- 两张照片一次产生两个活动标记、两套子操作和正确快照。
- 同一照片 JPG/RAF 只产生一个标记。
- 第二张标记写入时故障，标记/子操作/快照均为零新增。
- `SqliteBeforeCommit` 故障全部回滚。
- 完全相同批次重试返回同一标记集合。
- 相同批次 ID 搭配不同请求被拒绝。
- 人工构造部分子操作链时进入安全错误，不补齐。

验证：

```powershell
node scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml duplicate_pending_batch_commit
```

## 7. 任务五：实现后端任务状态机、进度与取消

在 `PendingDeleteService` 中持有独立的批量任务协调器状态；不创建第二个 `PendingDeleteService`。

### 状态所有权

- `Mutex<Option<TaskState>>` 保存当前或最近任务状态。
- `AtomicBool` 保存当前验证取消请求。
- `AtomicBool` 或状态锁保证同一时间只有一个 `validating/committing` 任务。
- 后台线程结束时必须无条件清理活动标志，避免永久 Busy。

### 启动

- 同步完成轻量请求解析与并发门检查。
- 发布初始 `validating` 状态。
- 启动命名后台线程，立即返回状态。

### 验证循环

- 按稳定顺序验证每个选择。
- 开始一对文件前更新当前路径。
- 成功后原子增加已验证文件数和容量。
- 第一个错误映射为用户可理解的失败类别后终止。
- 取消不是失败，进入 `cancelled`。

### 提交

- 全部验证成功后，在拿到不可取消提交所有权时切换 `committing`。
- 调用任务四的单事务核心。
- 成功进入 `completed`；失败进入 `failed`，确保无部分标记。

### 并发与生命周期测试

- 第二个任务在活动任务结束前被拒绝。
- `validating` 可取消，状态最终为 `cancelled` 且标记数为零。
- `committing` 返回不可取消状态。
- 首个失败后验证器不再调用后续项。
- 后台线程 panic/错误路径释放活动门并产生可查询失败状态。
- 进程仅在验证阶段退出不需要数据库恢复记录。

验证：

```powershell
node scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml duplicate_pending_batch_task
```

## 8. 任务六：接入 Tauri 命令和前端命令层

### Rust 命令

在 `commands/pending_delete.rs` 增加：

- `start_duplicate_pending_batch`
- `get_duplicate_pending_batch_status`
- `cancel_duplicate_pending_batch`

耗时状态查询若访问数据库则使用 `spawn_blocking`；纯内存状态读取保持短同步路径。将三条命令注册到 `lib.rs`。

旧 `add_duplicate_subset_to_pending_delete` 暂时保留为后端兼容入口和回归测试对象，但从前端移除调用；确认无内部消费者后再单独决定是否删除，避免同一改动扩大范围。

### TypeScript 命令

增加对应函数和严格 DTO。`start` 的 `operationId` 在一次弹框重试生命周期中稳定保存；用户修复后重新发起新的确认才生成新 ID。

### 契约验证

- IPC 参数名为 camelCase，Rust `request` 包装层与 Tauri 调用一致。
- 终态 DTO 能被 TypeScript 穷尽判断。
- 命令失败统一经过现有 `readError`，任务业务失败优先使用状态内错误。

验证：

```powershell
npm run lint
node scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml commands::pending_delete
```

## 9. 任务七：扩展 Modal 的独立取消语义

当前 `Modal.busy` 会同时禁用取消按钮和 Escape，不满足“验证中确认禁用但取消可用”。以向后兼容方式增加：

- `cancelLabel?: string`
- `cancelDisabled?: boolean`
- `dismissDisabled?: boolean`
- `confirmBusyLabel?: string`

默认值保持现有所有调用方行为。重复确认弹框显式控制：

- 摘要：可取消/可确认/可 Escape。
- `validating`：确认禁用，取消文案为“取消验证”，禁止 Escape。
- 正在取消：确认禁用、取消禁用、禁止 Escape。
- `committing`：两个按钮都禁用、禁止 Escape。
- `failed/cancelled`：恢复摘要交互。

组件测试覆盖 Tab 焦点循环和 Escape 在各阶段的行为。

验证：

```powershell
npm test -- src/shared/ui
```

如果项目没有独立 Modal 测试文件，在 `DuplicateReviewView.test.tsx` 覆盖该行为，不为测试而创建空目录结构。

## 10. 任务八：实现确认弹框任务交互

在 `DuplicateReviewView.tsx` 建立局部弹框状态机，不把批量任务状态混入重复扫描状态。

### 摘要态

- 保留滚动列表、总文件数和容量。
- 点击“加入待删除”调用 `start_duplicate_pending_batch`。
- 不清空选择，不关闭弹框。

### 轮询态

- 任务活动时每 350ms 查询状态。
- 显示 `正在验证 X / N`、容量进度、当前待删与保留路径。
- 轮询单次失败在弹框内显示连接错误并继续有限重试；不得假装任务已取消或成功。
- 组件卸载时清理 timer，但不自动取消用户明确启动的任务。

### 取消态

- 点击“取消验证”调用取消命令。
- 显示“正在取消”，继续轮询直到 `cancelled`。
- 到达 `cancelled` 后恢复摘要，保留全部选择。

### 失败态

- 在弹框内部显示第一个失败的两端路径、原因和下一步。
- 提供关闭弹框调整选择，以及重新确认整批。
- 主页面错误不作为弹框业务失败的唯一展示位置。

### 成功态

- 只在 `completed` 后清空选择并关闭弹框。
- 调用现有 `onPendingAdded()`；由 `App` 刷新图库和待删除快照并切换 `#/pending-delete`。
- 若刷新/跳转失败，显示全局错误，但后端标记已可通过待删除入口恢复，绝不重提批次。

### 前端测试先行

- 点击确认后弹框保持并进入验证进度。
- 轮询进度更新数字、容量和路径。
- 验证中取消，最终恢复摘要且勾选不变。
- 失败显示在弹框内，勾选不变。
- `committing` 禁止取消和 Escape。
- `completed` 只调用一次 `onPendingAdded`，关闭弹框并清空选择。
- 轮询暂时失败不会错误清空选择或重复启动任务。

验证：

```powershell
npm test -- src/features/duplicate-review/DuplicateReviewView.test.tsx
```

## 11. 任务九：完整安全与回归验证

按以下顺序运行，先快速失败再执行完整构建：

```powershell
npx prettier --check src/shared/ui/Modal.tsx src/features/duplicate-review/DuplicateReviewView.tsx src/features/duplicate-review/DuplicateReviewView.test.tsx src/shared/commands/duplicate-review.ts src/app/styles.css
npm run lint
npm test
npm run test:rust
npm run clippy
npm run safety:audit
npm run build
git diff --check
```

重点人工核对生成夹具，不使用用户唯一照片：

1. 至少两张逻辑照片的重复结果，批量选择后确认弹框显示进度。
2. 成功后待删除页出现两个独立“重复清理”项目，各自路径和保留路径正确。
3. 人为使第一项保留端变化，确认立即失败且待删除标记数不增加。
4. 验证中取消，弹框恢复且选择仍在。
5. 提交成功后模拟前端未跳转，重新打开待删除入口仍能看到全部项目。

禁止在人工测试中点击最终“移入 Windows 回收站”，除非使用明确可丢弃测试根并单独获得授权。

## 12. 交付与完成标准

完成必须同时满足：

- 全页面跨照片批量选择可一次进入待删除页。
- 数据库中每张逻辑照片仍是独立活动标记。
- 任一失败、取消或提交故障时整批零新增标记。
- 用户始终能在确认弹框内看到进度、取消反馈或首个失败原因。
- 成功后严格沿用待删除页复核与 Windows 回收站流程。
- 不新增数据库迁移，不修改照片文件。
- 完整前端、Rust、Clippy、安全审计与生产构建通过。

版本提交建议拆分为：

1. `feat: add atomic duplicate pending batch coordinator`
2. `feat: show duplicate batch validation progress`

若实现过程中必须改变数据库 schema、允许部分成功或改变回收语义，停止实施并重新提交设计确认，不在本计划内自行扩展。
