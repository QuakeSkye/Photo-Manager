# 重复清理后的受控结果协调设计

## 背景

现有流程已经把重复复核选择保存为 `DuplicateSubset` 待删除标记，并统一进入待删除页面复核，最终只通过 Windows 回收站处理。回收成功后的定向扫描会更新 Catalog 并递增 `library_catalog_revision`；重复检测 Run 又严格绑定完成时的 Catalog revision，因此任何成功回收都会让整个重复结果变成 `Expired`，要求用户重新执行全图库完整哈希。

这种处理对无法解释的图库变化是正确的，但对应用自身完成、具有完整 Batch/Item/Asset 事实链的重复清理过于保守。删除前已经重新验证待删端和保留端，回收结果与定向扫描也能明确说明哪些 Asset 从 `Present` 变为 `Missing`。在没有其他变化时，可以安全地从原等价集合中移除成功回收的成员，而无须再次读取整个图库。

## 目标

- 保留现有“重复复核 → 加入待删除 → 待删除页面复核 → Windows 回收站”的统一删除流程。
- 应用自身成功完成的 `DuplicateSubset` 回收通过定向扫描和不可变事实进行受控协调，不强制重新检测。
- 删除后准确更新等价集合、照片族、冗余文件数和理论可释放空间。
- 全部成功并完成协调后自动返回重复复核页。
- 部分失败或仍有问题项时继续停留在待删除页。
- 任何无法完全解释的 Catalog 变化仍使结果过期，不降低现有删除和保留副本保护。
- 旧检测、删除、扫描和协调事实全部可审计，不修改历史证据。

## 非目标

- 不通过增量协调发现扫描完成后新产生的重复文件。
- 不为应用外删除、移动、替换或修改文件推断结果。
- 不跳过加入待删除前或回收前的完整哈希、逐字节比较和身份复核。
- 不改变 Windows 回收站作为唯一删除执行路径。
- 不自动启动新的全图库重复检测。
- 不把普通 `WholePhoto` 删除默认解释为重复结果协调输入。

## 方案选择

采用“不可变派生结果版本”方案：保留原 Completed Run 和全部证明，受控协调成功后追加一个派生结果版本，记录来源 Run、回收 Batch、Catalog 变更范围、被移除成员和协调后的有效 revision。查询重复结果时使用最新有效派生版本。

不采用以下方案：

- 删除后重新完整检测：安全但重复执行全图库 I/O，用户完成清理后仍需等待。
- 直接忽略 revision 并在查询时隐藏 Missing Asset：无法证明 revision 中是否混入其他变化，也缺少协调审计边界。
- 原地更新 Run、Set 或 Proof：破坏不可变证据模型，无法还原检测时的真实结果。

## 一、端到端流程

```text
重复复核勾选文件
        ↓
加入待删除并创建 DuplicateSubset Mark/RetentionGuard
        ↓
进入待删除页逐项复核
        ↓
回收前重新证明待删端与保留端
        ↓
Windows 回收站返回逐文件结果
        ↓
定向扫描本 Batch 涉及的路径
        ↓
核对回收事实、Catalog 变化和来源 Run
        ↓
受控协调成功 ──────────────── 无法解释
        ↓                         ↓
发布派生结果版本              原结果 Expired
        ↓
全部处理成功：自动返回重复复核
部分失败/仍有问题：停留在待删除页
```

“全部处理成功”表示当前执行或重试结束后，本次待删除 Batch 的所有可执行项均有确定终态，且待删除页没有由该 Batch 留下的 `Partial`、`RetryableFailed` 或 `NeedsAttention` 项。用户主动取消剩余删除可以结束待删除流程，但只有已经成功回收的文件进入协调。

## 二、协调授权输入

协调不能仅凭“文件现在不存在”运行。一次协调必须绑定：

- 当前有效的来源 `DuplicateScanRun` 或其最新派生版本；
- `RecycleBatch`；
- Batch 中成功回收的 `DuplicateSubset` Item；
- Item 冻结的待删 AssetId/AssetVersion/身份快照；
- 对应 `DuplicateRetentionGuard`、保留 AssetVersion 和保留身份快照；
- Shell 返回的逐文件确定成功事实；
- 回收完成后的定向 Catalog 扫描报告；
- 扫描前后的 RootBinding 和 Catalog revision。

普通 `WholePhoto` 回收不会自动协调重复结果。若未来需要支持，必须有独立设计证明它与某个等价集合成员之间的关系。

## 三、安全协调条件

只有以下条件全部成立时才发布派生结果：

1. 来源 Run 在建立相关 `DuplicateSubset` Mark 时有效，且 Mark 明确引用该 Run/Set/Proof。
2. 成功回收的每个物理文件都能唯一对应到来源结果中的一个成员。
3. Shell 结果为确定成功；取消、未知、失败或未调用 Shell 的 Item 不得作为已移除成员。
4. 定向扫描确认成功项的原路径不再解析到原文件对象，Catalog 对应 Asset 已变为 `Missing`。
5. 定向扫描范围覆盖本 Batch 全部成功项和相关父目录，没有漏扫路径。
6. 保留 Asset 当前仍为 `Present`，Root/Binding、路径、file ID、大小和 mtime 与回收前重新验证事实一致。
7. 所有未回收的结果成员仍为原 AssetVersion，身份与原检测观察一致。
8. 从来源有效 revision 到目标 revision 的所有 Catalog 变更都可由本次成功回收清单解释。
9. 没有新增 Asset、非目标 Asset 缺失、路径改变、file ID 改变、内容身份变化、RootBinding 改变或其他未归因变更。
10. 移除成员后，每个保留下来的等价集合仍至少有两个成员；只有一个成员的集合从可操作结果中消失。

任一条件不满足都不发布部分协调结果，来源结果进入 `Expired`，并保存明确原因。协调失败不回滚已经完成的 Windows 回收站操作，也不猜测剩余重复关系。

## 四、Catalog 变更归因

Catalog revision 继续保持全局单调递增，不为重复复核建立特殊 revision 旁路。新增不可变的 Catalog 变更归因记录，把一次定向扫描产生的 revision 区间绑定到 Recycle Batch 和明确 Asset 清单。

协调器读取来源结果的有效 revision 到定向扫描目标 revision 之间的变更事实：

- 若区间内只有本 Batch 成功回收成员的 `Present → Missing`，允许继续。
- 若区间为空，说明 Catalog 尚未观察到回收结果，协调保持 Pending，不发布。
- 若混入任何其他变化，协调失败并使结果过期。
- 多个应用内 Batch 串行完成时，每个 Batch 依次从最新有效派生版本继续协调，不跨越未解释区间。

I/O 调度继续保证 Duplicate Scan、Import、Recycle 和 Catalog 扫描的互斥关系。协调本身只读取事实并执行短数据库事务，不重新哈希全部图库。

## 五、不可变派生结果

新增逻辑记录 `DuplicateResultReconciliation`，至少包含：

```text
reconciliation_id
source_run_id
parent_reconciliation_id nullable
recycle_batch_id
from_catalog_revision
to_catalog_revision
state Pending | Applied | Rejected
removed_asset_count
removed_bytes
rejection_reason nullable
created_at
completed_at nullable
```

并追加成员记录：

```text
reconciliation_member_id
reconciliation_id
set_id
asset_id
asset_version
recycle_item_id
catalog_change_fact_id
```

原 `DuplicateEquivalenceSet.member_count`、Observation 和 Proof 不更新。最新结果通过“原始成员减去 Applied 协调链中已移除成员”派生：

- 剩余成员数 `>= 2`：集合继续有效。
- 剩余成员数 `< 2`：集合不再返回。
- 逻辑照片族根据仍存在的 JPG/RAF 集合重新组合显示状态。
- 冗余副本数重新计算为各有效集合 `remaining_count - 1`。
- 理论可释放空间重新计算为各有效集合 `file_size × (remaining_count - 1)`。

协调链绑定最新 `to_catalog_revision`。以后建立新的 `DuplicateSubset` Mark 时，服务同时校验来源 Run、最新 Applied 链和当前 Catalog revision；旧 Run 的原始证明仍只是候选证据，建立 Mark 前继续重新进行完整哈希、逐字节比较与身份复核。

## 六、删除结果和状态模型

Recycle 返回给前端的状态增加可导航的协调结论：

```text
duplicateReconciliation:
  notApplicable
  pending
  applied
  rejected

successfulDuplicateFileCount
successfulDuplicateBytes
remainingProblemCount
duplicateReviewRunId nullable
reconciliationMessage nullable
```

状态语义：

- `notApplicable`：本次没有成功回收 DuplicateSubset 文件。
- `pending`：Shell 已成功，但定向扫描或协调尚未完成；界面不得提前显示更新结果。
- `applied`：派生结果已发布，可返回重复复核页。
- `rejected`：发现无法解释的变化，重复结果已过期。

应用启动恢复会继续处理已经有 Shell 确定结果、但尚未完成定向扫描或协调的 Batch。协调命令必须幂等，同一 Batch 只能产生一个最终 Applied 或 Rejected 结果。

## 七、页面导航

### 全部成功

当回收、定向扫描和协调全部完成，且待删除中没有本 Batch 的剩余问题项：

1. 刷新 Library、PendingDelete 和 DuplicateReview 状态。
2. 路由自动切换到 `duplicate-review`。
3. 清空重复复核页未提交的本地选择。
4. 显示协调后的最新结果及一次性处理回执。

页面刷新或应用重启后仍可从最新协调事实恢复结果；一次性回执可以降级为最近处理摘要，但不能重新出现已经移除的成员。

### 部分失败或问题项

只要存在 `Partial`、`RetryableFailed`、`NeedsAttention` 或仍可重试的剩余文件：

- 保持 `pending-delete` 路由；
- 显示成功、失败和待处理数量；
- 允许重试或取消剩余删除；
- 不自动返回重复复核页。

成功回收的成员可以记录为 Pending 协调输入，但在该删除流程取得稳定终态前不向前端发布新的重复结果。用户取消剩余删除后，协调只移除已成功回收成员；被取消或失败的成员继续存在于结果中，前提是所有安全条件仍成立。

### 协调被拒绝

如果删除已经成功但协调被拒绝：

- 若待删除问题已清空，可以返回重复复核页；
- 页面显示 `重复检查过期` 和具体原因；
- 提供用户主动点击的 `重新检测`，但不自动开始；
- 删除成功回执仍然可见，避免用户误以为回收失败。

## 八、重复复核页面

### 仍有重复

页面顶部在摘要卡片之前显示：

```text
重复文件已处理
已移入 Windows 回收站 46 个文件 · 1.5 GB
图库索引和重复结果已更新，无须重新检测。
```

列表展示协调后的成员：

- 原 3 份删除 1 份后显示 `2 份（重复 1 份）`。
- 已成功回收文件不再显示。
- JPG 只剩 1 份、RAF 仍重复时，照片族改为“仅 RAF 重复”。
- 页面选择清空，固定底栏显示“尚未选择文件”。
- `开始检测` 保持为用户主动重新检查的次要动作，不暗示当前结果无效。

### 全部处理完成

没有剩余有效集合时显示完成空态：

```text
本次发现的重复文件已处理完
已移入 Windows 回收站 46 个文件 · 1.5 GB
图库索引已更新，无须重新检测。

[返回图库]  [再次检查重复]
```

`再次检查重复` 使用低于“返回图库”的强调级别，点击后仍进入现有手动检测流程。页面不得把“没有剩余已知重复”表述为“整个图库现在绝对没有重复”。

### 后续外部变化

协调成功后若普通扫描观察到无法归因的新变化，最新派生结果照常进入 `Expired`。用户需要重新检测才能获得覆盖这些变化的新结果。

## 九、错误和恢复

- Shell 成功、Catalog 尚未更新：保持协调 Pending，启动或后台协调继续定向扫描。
- Catalog 已更新、应用在发布协调前崩溃：从 Batch、扫描报告和 revision 区间幂等补发协调结果。
- 同一 Batch 重复协调：返回既有最终结果，不追加第二条成员移除事实。
- 成功项后来又在原路径出现新文件：这是新增/替换，不得解释为回收撤销，结果过期。
- 保留副本在回收后、协调前变化：协调拒绝；已经回收的文件仍可从 Windows 回收站恢复。
- 协调事务失败：不修改旧结果状态；保持 Pending 并可安全重试。只有证据明确冲突时才写 Rejected。
- 协调 Applied 后不得通过撤销待删除标记恢复成员；文件恢复只能由 Windows 回收站和后续 Catalog 扫描观察，观察到恢复文件属于新变化并使结果过期。

## 十、测试与验收

### 数据库与协调

- 原 Run、Set、Observation 和 Proof 在协调后字节级不变。
- Applied 协调只能引用同一来源链、Batch、Item、AssetVersion 和 Catalog 变更事实。
- 同一 Batch 不可生成两个最终协调结果。
- 仅目标 Asset `Present → Missing` 的 revision 区间可 Applied。
- 混入新增、非目标 Missing、路径或身份变化时 Rejected。
- 3 份减 1 份仍返回 2 份；2 份减 1 份后集合消失。
- 多个 Batch 可按 parent 链串行协调，统计不重复扣减。

### 回收与恢复

- 全部 DuplicateSubset 文件成功、定向扫描完成后状态为 Applied。
- 部分失败期间保持待删除问题和 Pending，不提前发布结果。
- 用户取消剩余项后，只协调确定成功项。
- Shell 成功后注入崩溃，重启可恢复扫描和协调且不再次调用 Shell。
- 保留副本变化、RootBinding 变化或扫描范围不完整时协调拒绝。

### 前端

- Applied 且无待删除问题时自动进入重复复核页。
- Partial/NeedsAttention 时继续停留在待删除页。
- 处理回执准确显示成功物理文件数和字节数。
- 协调后的摘要、集合数量、成员列表和理论空间准确。
- 全部处理完显示完成空态，不自动开始新检测。
- Rejected 同时显示删除成功回执和结果过期原因。
- 页面刷新后不重新显示已协调移除的成员。

### 安全回归

- 建立 Mark 前和回收前的双端完整重复证明继续执行。
- WholePhoto、Import、普通 Catalog 扫描和 Windows 回收站失败路径保持原语义。
- 无法解释的变化不能通过协调绕过 revision 过期保护。
- 自动测试只使用生成的可丢弃文件，不操作用户照片。

## 十一、与既有设计的关系

本设计替换 `2026-08-10-import-dedup-pairing-and-duplicate-review-design.md` 中“回收导致 catalog revision 改变后旧重复结果一律过期”的规则，并扩展 `2026-08-11-duplicate-review-delete-selection-ui-design.md` 的提交后行为。

不变的原则是：旧证明不是永久删除授权；每次加入待删除和真正回收前仍重新验证保留副本；应用外或无法归因的图库变化仍使结果过期；重复检测仍只由用户手动启动。
