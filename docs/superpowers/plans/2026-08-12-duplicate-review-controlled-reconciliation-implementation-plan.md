# 重复复核删除后的受控协调实施计划

日期：2026-08-12

目标分支：`codex/duplicate-review-selection-ui`

实施基线：`8521812`（当前 `master` 尚不包含 `7cd5626` 的重复复核实现，不可直接按本计划施工）

设计依据：`8521812:docs/superpowers/specs/2026-08-12-duplicate-review-controlled-reconciliation-design.md`

取代范围：取代旧计划中“回收后目录修订号变化，原重复检测结果直接过期并要求重新检测”的处理方式；不取代既有重复扫描、重复证明、待删除复核和 Windows 回收站安全链。

## 1. 目标与完成边界

保留当前主流程：

```text
重复复核
  → 标记为待删除
  → 图库右侧“待删除”
  → 待删除页复核
  → Windows 回收站
  → 定点更新目录索引
  → 受控协调重复结果
  → 回到重复复核页
```

本次实现的核心结果：

- 回收成功后不自动重新执行完整重复检测。
- 只重新观察本批次已经获得确定回收事实的路径，并把对应 Asset 追加为 `Missing` 版本。
- 目录修订号继续单调递增；所有新增修订都留下不可变的原因和资产变化事实。
- 只有能够完整证明“当前修订变化完全来自这个回收批次，并且保留端和其余结果成员没有变化”时，才发布一个不可变的派生重复结果版本。
- 派生版本通过排除已回收成员来查询原结果；原始 Run、Set、Observation 和 Proof 不修改。
- 全成功并协调通过后自动进入重复复核页；部分失败或需要处理时继续停留在待删除页。
- 协调不能证明时，删除事实仍然保留，重复结果进入 `Expired`，用户可手动重新检测，但系统不自动扫描。

不在本次范围内：恢复回收站文件、撤销已经成功的物理删除、后台自动完整重复扫描、放宽 `WholePhoto` 的删除授权、重写重复哈希算法。

## 2. 实施约束

1. Shell 回调只负责提交物理回收事实，不在回调事务中运行目录或重复结果协调。
2. 只有批次到达稳定终态后才发布协调结果：`Completed` 或用户取消剩余项后的 `Cancelled`。`CompletedWithErrors`、`NeedsAttention` 和 `Running` 不发布。
3. 同一 `(source_run_id, recycle_batch_id)` 最多一个终态协调结果；启动恢复和重复调用必须幂等。
4. 协调写入必须在一次数据库事务中重新读取所有授权输入，不能信任前端 DTO 或先前的内存快照。
5. 定点目录更新和协调失败不能回滚已经确认的 `deletion_recycled_fact`。
6. 新的普通扫描、导入、RootBinding 变化或无法解释的目录修订仍按原规则使结果过期。
7. 前端导航依据后端返回的终态和剩余问题数，不再只以 `pending.summary.totalCount === 0` 判断。

## 3. 任务一：新增 v11 不可变事实模型

涉及文件：

- `src-tauri/migrations/0011_duplicate_result_reconciliation.sql`（新增）
- `src-tauri/src/db/migration.rs`
- `src-tauri/src/db/tests.rs`

先增加失败测试，验证 schema 版本为 11，并验证下列表、约束、索引和不可变触发器存在。

新增 `catalog_revision_event`：

- `event_id`
- `library_id`
- `root_binding_id`
- `from_revision`
- `to_revision`
- `cause_kind`：至少包含 `AssetObserved`、`ScanMissing`、`RootBindingChanged`、`RecycleTargeted`
- `scan_run_id`（可空）
- `import_task_id`（可空）
- `recycle_batch_id`（可空）
- `created_at`

约束：同一图库的 `to_revision` 唯一，`to_revision = from_revision + 1`，原因关联字段与 `cause_kind` 一致。旧数据库升级时不伪造历史事件；v11 之后每次修订递增必须在同一事务写入事件。

新增 `catalog_revision_asset_change`：

- `event_id`
- `asset_id`
- `before_asset_version`（新资产时可空）
- `after_asset_version`
- `before_presence`（可空）
- `after_presence`
- `change_kind`

以 `(event_id, asset_id)` 为主键，外键指向事件和 Asset revision；禁止更新和删除。

新增 `duplicate_result_reconciliation`：

- `reconciliation_id`
- `source_run_id`
- `parent_reconciliation_id`（可空）
- `recycle_batch_id`
- `from_catalog_revision`
- `to_catalog_revision`
- `reconciliation_state`：`Pending`、`Applied`、`Rejected`
- `removed_asset_count`
- `removed_bytes`
- `rejection_reason`（可空）
- `created_at`
- `completed_at`（可空）

对 `(source_run_id, recycle_batch_id)` 建唯一约束；用触发器只允许 `Pending → Applied|Rejected` 的一次终结转换，终态记录不可再改。

新增 `duplicate_result_reconciliation_member`：

- `reconciliation_id`
- `set_id`
- `asset_id`
- `asset_version`
- `batch_asset_id`
- `recycled_fact_id`
- `catalog_revision_event_id`
- `removed_bytes`

成员记录只允许引用源 Run 中由原 Proof 覆盖的成员、当前批次的 `DuplicateSubset` 资产、确定的回收事实和 `RecycleTargeted` 目录变化事实；禁止更新和删除。

测试顺序：

```powershell
npm run test:rust -- db::tests::duplicate_result_reconciliation_schema_is_versioned_constrained_and_immutable
```

本任务提交建议：`feat(db): add duplicate reconciliation facts`

## 4. 任务二：让目录修订变化可归因

涉及文件：

- `src-tauri/src/catalog/candidate_lookup.rs`
- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/catalog/phase_one_tests.rs`
- `src-tauri/src/import_execution/tests.rs`

先为下列行为写失败测试：

1. 插入 Asset、更新 Asset、完整扫描标记 Missing 时，修订号每次只递增 1，且同一事务产生一个对应事件。
2. 事件成员准确记录变化前后的 Asset version 与 presence。
3. RootBinding 变化产生 `RootBindingChanged` 事件；该事件没有伪造的资产成员。
4. 事务在“修订号已更新、事件尚未写入”处注入故障时整体回滚。
5. 旧 v10 数据升级后，以当前 `library_catalog_revision` 作为历史锚点；升级前的区间不可被受控协调误判为可证明。

把 `bump_catalog_revision` 改为返回明确的 `(from_revision, to_revision)`，并要求调用者携带 `CatalogRevisionCause` 和完整的 `CatalogAssetChange` 列表。把以下三个现有调用点迁移到统一入口：

- `update_existing`
- `insert_new`
- `mark_missing_assets`

把 `ensure_catalog_revision` 中的 RootBinding 变化也纳入同一事件写入入口。禁止业务代码直接递增 `library_catalog_revision`；用测试扫描 Rust 源码中的直接 SQL 更新，防止后续绕开事实链。

普通完整扫描或导入仍按现有行为工作；新增事件只提供归因，不改变 UI 和扫描结果。

测试顺序：

```powershell
npm run test:rust -- catalog::phase_one_tests
npm run test:rust -- import_execution::tests
```

本任务提交建议：`refactor(catalog): attribute catalog revision changes`

## 5. 任务三：实现回收后的定点目录协调

涉及文件：

- `src-tauri/src/catalog/recycle_reconciliation.rs`（新增）
- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/catalog/phase_one_tests.rs`

新增 `CatalogService::reconcile_recycled_batch(batch_id)`，只从数据库读取满足以下条件的资产：

- 属于指定批次；
- `deletion_batch_asset.resolution_state = 'Recycled'`；
- 存在 `deletion_recycled_fact`；
- 当前 Asset version、RootBinding 和冻结快照仍能唯一对应；
- 尚未存在该批次对应的 `RecycleTargeted` 目录变化成员。

对这些确切路径再次执行只读观察：

- 路径为 Missing：追加一个 `asset_revision(presence_state='Missing')`，更新 Asset 和 Photo version，并把同批次本次变化合并为一个 `RecycleTargeted` 修订事件。
- 路径仍是冻结的原文件、已被不同文件占据、Root 不可用或身份不唯一：不写伪造的 Missing revision，返回结构化拒绝原因。
- Asset 已由同一批次幂等地标为 Missing：复用既有事实，不重复增加 version 或 revision。
- Asset 已被其他目录事件改变：记录冲突并交由重复结果协调拒绝。

定点更新后调用现有 `pairing::reconcile_library`，让图库时间线和 JPG/RAF 配对反映 Missing 成员；不枚举整个 Root，不重新计算 SHA-256。

测试覆盖：单文件、多文件同批次只增加一次修订、重复调用、路径被替换、RootBinding 改变、在回收事实与定点更新之间发生普通扫描、事务故障恢复。

本任务提交建议：`feat(catalog): reconcile recycled assets by batch`

## 6. 任务四：实现不可变的重复结果协调引擎

涉及文件：

- `src-tauri/src/duplicate_review/reconciliation.rs`（新增）
- `src-tauri/src/duplicate_review/mod.rs`
- `src-tauri/src/duplicate_review/tests.rs`

新增 `DuplicateResultReconciler`，入口接受 `batch_id`，但所有实际授权从数据库重新构造。先写失败测试覆盖安全矩阵，再实现。

### 6.1 构造候选协调

通过以下链路找到受影响的源 Run 和集合：

```text
DeletionBatch
  → DeletionBatchAsset
  → DeletionBatchRetentionGuard
  → DuplicateRetentionGuard
  → DuplicateEquivalenceProof
  → DuplicateObservation
  → DuplicateSet / DuplicateRun
```

按 `source_run_id` 分组；一个批次若涉及多个 Run，分别创建协调记录，不合并授权。只把有完整 retention guard 的 `DuplicateSubset` 成功项作为可移除成员。

### 6.2 事务内验证

在发布 `Applied` 前逐项验证：

1. 源 Run 在建立删除标记时处于 Completed 且当时未过期。
2. 每个删除资产唯一映射到源集合成员、批次资产、回收事实和 `RecycleTargeted` 事件成员。
3. Shell 成功证据、源路径 Missing 证据和目录 Missing revision 同时存在。
4. retention guard 指向的保留 Asset 仍是相同 version、`Present`、同一 RootBinding 和文件身份。
5. 源结果中未回收的成员身份均未改变。
6. `from_catalog_revision..to_catalog_revision` 的每一个修订都有连续事件，没有缺口。
7. 区间内所有 Asset 变化均由当前批次的确定回收项解释；允许同批次中与源 Run 无关的已证明回收项，但若 `WholePhoto` 项本身属于该源 Run，则拒绝。
8. 区间内不存在 RootBinding、普通扫描、导入、新增或其他批次变化。
9. 应用本次移除后，每个集合只在剩余成员数大于等于 2 时保留；单例集合从派生结果中消失。

全部通过时写入成员并终结为 `Applied`；任一条件不成立时终结为 `Rejected`，保存稳定、可面向用户翻译的拒绝代码。不要因协调失败修改物理回收事实。

### 6.3 幂等和链式协调

- 相同 `(run, batch)` 再次执行返回同一个终态记录。
- 新批次以该 Run 最新 `Applied` 记录的 `to_catalog_revision` 为 `from_catalog_revision`，形成 parent 链。
- 若最新记录是 `Rejected`，后续不再从该过期 Run 派生。
- 崩溃留下 `Pending` 时，启动恢复重新验证并只终结一次。

核心测试至少包括：全成功 Applied、部分批次未终结不发布、成功后取消剩余项只移除成功成员、保留端变化、非成员变化、修订事件缺口、混入另一个批次、WholePhoto 命中源 Run、集合降为单例、两次连续批次、重复恢复调用。

本任务提交建议：`feat(duplicates): reconcile proven recycled members`

## 7. 任务五：让重复结果查询读取最新派生版本

涉及文件：

- `src-tauri/src/duplicate_review/mod.rs`
- `src-tauri/src/duplicate_review/tests.rs`
- `src/shared/commands/duplicate-review.ts`

重构 `completion_expiry` 和 `load_families`：

- 取源 Run 最新的 `Applied` 链尾修订号作为有效修订，而不是始终使用 `duplicate_scan_run.catalog_revision`。
- 用 Applied 链的 reconciliation members 排除已移除 Asset；不更新或删除原 Set、Observation、Proof。
- 对剩余成员重新计算 family、set、`redundantFiles`、`reclaimableBytes` 和总摘要。
- 剩余少于 2 个成员的 Set 不返回；一个 Family 的 JPG/RAF 都消失时，Family 不返回。
- 当前目录修订号等于有效修订号时结果有效；更高修订号没有新的 Applied 协调时按原逻辑 `Expired`。
- 最新协调为 `Rejected` 时返回该拒绝原因并清空 families，但保留删除收据和过期摘要。

扩展 DTO：

```ts
interface DuplicateReviewReceipt {
  reconciliationId: string;
  batchId: string;
  state: 'applied' | 'rejected';
  recycledFiles: number;
  recycledBytes: number;
  completedAt: number;
  message: string;
}
```

在 `DuplicateReviewStatus` 和 `DuplicateReviewResults` 中增加 `resultVersionId` 与 `lastReceipt`。前端选中状态以 `resultVersionId` 为代际键；结果版本改变后清空旧勾选，防止把旧 Asset version 再次提交。

测试验证原始表行数和内容完全不变，并验证查询层正确投影派生结果。

本任务提交建议：`feat(duplicates): query reconciled result versions`

## 8. 任务六：把回收批次推进为可恢复的协调状态机

涉及文件：

- `src-tauri/src/recycle/mod.rs`
- `src-tauri/src/recycle/tests.rs`
- `src-tauri/src/lib.rs`

将回收后的顺序固定为：

```text
Shell 回调提交回收事实
  → close_attempt 结算批次
  → 批次是否稳定终态？
      否：返回待用户处理状态
      是：定点目录协调
          → 重复结果协调
          → 返回包含协调结果的 RecycleStatus
```

不要把目录服务或重复协调器放入 `CallbackBarrier`。在 `RecycleService` 中注入 `CatalogService` 和 `DuplicateResultReconciler`，并在 `start`、`retry`、`cancel_remaining` 和 `reconcile_startup` 的批次结算之后调用统一的 `finalize_stable_batch`。

扩展 `RecycleStatus`：

- `duplicateReconciliation`: `notApplicable | pending | applied | rejected`
- `successfulDuplicateFileCount`
- `successfulDuplicateBytes`
- `remainingProblemCount`
- `duplicateReviewRunId`
- `reconciliationMessage`

状态规则：

- `CompletedWithErrors` / `NeedsAttention`：`pending`，保留失败项并停留待删除页，不改变重复结果。
- 全成功 `Completed`：执行定点更新与协调。
- 用户取消剩余项后的 `Cancelled`：若存在成功的 DuplicateSubset 项，也执行定点更新与协调，只移除成功成员。
- 无 DuplicateSubset 成功项：`notApplicable`。
- 目录定点更新或证明检查失败：`rejected`，返回可恢复的收据和过期原因，不把批次伪装成物理删除失败。

启动顺序调整为先构造 Catalog 与 DuplicateResultReconciler，再构造 RecycleService。`reconcile_startup` 先恢复未结 Shell attempt，再扫描所有稳定终态但协调未终结的批次；重复执行不增加 Asset version、修订事件或协调记录。

测试覆盖每个阶段故障点：物理回收后崩溃、批次结算后崩溃、Missing revision 后崩溃、Pending reconciliation 后崩溃，以及每个点恢复两次。

本任务提交建议：`feat(recycle): coordinate duplicate results after recycle`

## 9. 任务七：更新命令 DTO 和演示后端

涉及文件：

- `src-tauri/src/commands/recycle.rs`
- `src-tauri/src/commands/duplicate_review.rs`
- `src/shared/commands/recycle.ts`
- `src/shared/commands/duplicate-review.ts`
- `src/shared/commands/library.ts`
- 相关 command/serialization 测试

Rust 使用 `camelCase` 序列化新增字段；TypeScript 使用判别联合表达协调状态，避免用 message 文本驱动导航。

演示后端增加至少四种确定场景：

- DuplicateSubset 全成功且 Applied
- 部分失败仍 AwaitingUser
- 取消剩余后 Applied
- 物理删除成功但协调 Rejected

演示状态必须与真实 DTO 一致，避免 App 测试只通过 demo 的简化路径。

本任务提交建议：`feat(commands): expose duplicate reconciliation outcomes`

## 10. 任务八：修正待删除与重复复核之间的导航

涉及文件：

- `src/app/App.tsx`
- `src/app/App.test.tsx`

增加一个统一的 `refreshAfterRecycle(status)`，同时刷新：

- library snapshot
- pending-delete snapshot
- recycle status
- duplicate-review status

导航判定顺序：

1. `remainingProblemCount > 0`，或 phase 为 `awaitingUser` / `needsAttention`：留在 `pending-delete`，选中第一个问题项。
2. 协调为 `applied` 且待删除中没有其他项目：进入 `duplicate-review`。
3. 协调为 `rejected` 且没有剩余待删除项目：进入 `duplicate-review`，展示已删除收据和结果过期原因。
4. 仍有其他待删除项目：留在 `pending-delete`，不跳过用户复核。
5. 与重复清理无关且待删除为空：沿用返回图库的行为。

同一规则用于 `confirmRecycle` 和 `cancelRecycleRemaining`。不再用 `pending.summary.totalCount === 0` 单独决定回图库。

另外把 `DuplicateReviewView.onPendingAdded` 改为刷新待删除快照后进入 `pending-delete`，让已勾选副本明确进入图库右侧待删除页完成最终复核；如果还有其他待删除项，也一起在该页展示。

App 测试覆盖：提交后进入待删除、全成功回重复复核、部分失败留在待删除、取消剩余后回重复复核、Rejected 进入过期结果、普通删除仍回图库、存在其他待删除项时不错误跳转。

本任务提交建议：`feat(app): route duplicate cleanup by reconciliation state`

## 11. 任务九：展示处理收据和协调后的结果

涉及文件：

- `src/features/duplicate-review/DuplicateReviewView.tsx`
- `src/features/duplicate-review/DuplicateReviewView.test.tsx`
- `src/app/styles.css`

在重复复核页顶部、统计卡之前展示一次非模态收据：

```text
重复文件已处理
已移入 Windows 回收站 N 个文件 · 1.5 GB
图库索引和重复结果已更新，无须重新检测。
```

`Applied` 后：

- 使用协调后的 families 和 summary 正常渲染剩余重复项。
- 清空旧的待删除勾选。
- 若已无重复集合，展示“本次发现的重复文件已处理完”。
- 主按钮“返回图库”，次按钮“再次检查重复”；再次检查仍由用户主动触发。

`Rejected` 后：

- 同时明确展示“已成功移入回收站”的收据和“现有重复结果已过期”的原因。
- 不暗示删除失败，不自动触发扫描。
- 提供“再次检查重复”按钮。

使用 `role="status"` 报告成功收据，Rejected 原因使用现有可访问错误/提示区域；页面固定底栏继续保持在窗口底部，不因收据出现而回到内容流中间。

本任务提交建议：`feat(ui): show reconciled duplicate cleanup receipt`

## 12. 任务十：端到端安全回归与文档收口

涉及文件：

- `src-tauri/src/duplicate_review/tests.rs`
- `src-tauri/src/recycle/tests.rs`
- `src/app/App.test.tsx`
- `src/features/duplicate-review/DuplicateReviewView.test.tsx`
- `docs/superpowers/specs/2026-08-12-duplicate-review-controlled-reconciliation-design.md`（仅在实现发现必须澄清时更新）

端到端场景：

1. 扫描出 3 个相同文件，删除 1 个，协调后仍显示 2 个、冗余数从 2 变 1。
2. 扫描出 2 个相同文件，删除 1 个，协调后集合消失且显示处理完成状态。
3. 一批删除多个集合成员，字节和文件数收据准确。
4. 一项成功、一项失败：失败项留在待删除，重复结果未提前变化；重试成功后一次发布。
5. 一项成功、一项失败后取消剩余：只排除成功项。
6. Shell 成功后保留端被外部改动：物理删除收据保留，协调 Rejected，结果 Expired。
7. Shell 成功后发生普通扫描、导入或 RootBinding 变化：不错误 Applied。
8. 启动恢复重复执行两次，所有不可变事实和版本计数保持幂等。
9. 后续再发生普通目录变化，已协调结果按正常规则过期。
10. WholePhoto 普通删除流程、导入、扫描和现有重复选择保护全部回归通过。

按风险从小到大运行：

```powershell
npm test -- src/features/duplicate-review/DuplicateReviewView.test.tsx src/app/App.test.tsx
npm run test:rust -- duplicate_review::tests
npm run test:rust -- recycle::tests
npm run test:rust -- catalog::phase_one_tests
npm run lint
npm run format:check
npm run test:ui
npm run test:e2e
npm run test:rust
npm run clippy
npm run safety:audit
npm run build
git diff --check
```

Windows 实机门禁补充验证：Shell 回收站成功证据、部分失败/取消、应用重启恢复和真实 NTFS 路径被替换场景。若 CI 不具备 Windows Shell 条件，跨平台单元测试必须覆盖协调状态机，Windows 测试只验证 Shell 适配层与真实文件行为。

## 13. 最终验收标准

- 用户无需为了应用自己刚完成的重复清理再次运行完整检测。
- 图库索引中的成功删除 Asset 已是新的 Missing version，图库不会继续显示不存在的文件。
- 重复结果只通过不可变派生记录变化，原始扫描和证明保持可审计。
- 任意无法解释的目录变化都不能被错误地包装成 Applied。
- 部分失败始终留在待删除页，直到重试或取消形成稳定终态。
- 全成功或取消剩余后，若协调完成，重复复核页立即显示更新后的结果与准确收据。
- Rejected 明确区分“文件已经删除”和“结果无法安全沿用”，且只提供手动重新检测。
- 重启恢复幂等，故障注入不会产生重复 Missing version、修订事件或协调终态。
- WholePhoto 删除及既有重复选择安全保护无回归。
