# 导入去重、配对与历史重复复核实施计划

日期：2026-08-10  
依据：`docs/superpowers/specs/2026-08-10-import-dedup-pairing-and-duplicate-review-design.md`

## 1. 实施边界与顺序

本计划分为两个可独立验收的交付门：

- **交付门 A：导入正确性。** 先完成全图库候选发现、不可变解析事实和 Copy/Skip 四象限配对，阻止以后继续制造重复副本。
- **交付门 B：历史重复复核。** 再完成用户手动启动的严格重复检测、DuplicateSubset 待删除、保留副本复核和界面优化。

实现必须保留来源零写入、目标 no-replace、完整重复证明、计划重新确认、不可变事实、Windows 回收站唯一删除路径和启动恢复语义。自动测试只使用生成夹具或明确可丢弃目录，不读取、修改或回收用户唯一照片。

实施前记录工作区状态；格式化、暂存和提交只包含本计划明确文件，不覆盖用户已有改动。

## 2. 任务一：增加 schema v10 与约束测试

涉及文件：

- `src-tauri/migrations/0010_import_dedup_duplicate_review.sql`
- `src-tauri/src/db/migration.rs`
- `src-tauri/src/db/tests.rs`
- `src-tauri/src/domain/ids.rs`

先写失败测试：

1. schema 9 可原子迁移到 schema 10，旧 Library、Asset、Photo、Import、DeletionMark 和报告行数不变。
2. 旧 `deletion_mark` 自动解释为 `mark_scope='WholePhoto'`。
3. `ImportAssetResolutionFact` 拒绝 evidence 全空、混填、跨 Step、跨 AssetVersion和第二个 Step fact。
4. `DuplicateEquivalenceProof` 拒绝跨 Run/Set、同一 Asset 自比、角色不一致和缺少稳定快照。
5. `DuplicateSubset` 快照缺 retention guard、WholePhoto 携带 retention guard、保留 Asset 等于待删 Asset时，在进入 Batch 前失败。
6. migration manifest、checksum、前向迁移和完整 schema 约束测试更新到 v10。

迁移新增：

- `library_catalog_revision`：每个 Library 的单调修订号与更新时间。
- `asset_import_candidate_key`：AssetVersion 派生的文件名、扩展名、大小、拍摄时间、Root/Binding/path key 和格式版本；旧版本行只追加，当前查询必须 join `asset.current_version`。
- `import_asset_resolution_fact`：Step 到 AssetVersion 的 `PublishedIndexed | SkippedDuplicate` 不可变解析事实。
- `duplicate_scan_run`、`duplicate_file_observation`、`duplicate_equivalence_set`、`duplicate_equivalence_proof`。
- `deletion_mark.mark_scope`：`WholePhoto | DuplicateSubset`，默认 `WholePhoto`。
- `duplicate_retention_guard`：每个 DuplicateSubset 的待删 Asset 快照到保留 AssetVersion、保留快照和等价证明的不可变链。
- 所需唯一索引、外键、CHECK、不可更新/删除触发器和跨链一致性触发器。

避免在迁移 SQL 中根据文件名猜测解析事实或重复关系。旧 Asset 候选键和旧 Import 解析事实由后续运行时协调补齐。

验证命令：

```powershell
npm run test:rust -- db
npm run clippy
```

## 3. 任务二：建立 Catalog 修订号和全图库候选查询键

涉及文件：

- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/catalog/scanner.rs`
- `src-tauri/src/catalog/candidate_lookup.rs`（新增）
- `src-tauri/src/catalog/phase_one_tests.rs`

先写失败测试：

1. Catalog 建立或切换 AssetVersion 时同步追加正确的 candidate key。
2. 同一 Asset 观察未发生身份或 metadata 变化时不递增 library catalog revision。
3. 新增、缺失、路径、file ID、大小、mtime 或当前 AssetVersion 变化时修订号只递增一次。
4. 相同文件名、扩展名、大小且时间相容的 Present Asset 可从任意图库子目录命中。
5. 可靠时间冲突、非当前 AssetVersion、Missing Asset、其他 Library 和不同 Root 不进入候选。
6. 任一端可靠拍摄时间缺失时允许成为候选，但不能直接判为重复。

实现步骤：

1. 新增 `catalog::candidate_lookup`，对外只返回包含 AssetId、AssetVersion、Root/Binding 和相对路径的只读候选，不返回“是否重复”结论。
2. 从 `CatalogObservation.relative_path` 派生大小写折叠 basename/extension，写入带格式版本的 candidate key。
3. 将修订号递增放在真正改变当前物理 Asset 集合或身份的 Catalog 短事务中；普通 metadata 补充是否递增按结果可操作性测试固定，不能让无变化扫描过期。
4. 扫描完成时为缺少当前候选键的 Present Asset 做数据库内派生补齐；不读取或哈希照片内容。
5. 为全图库查询增加覆盖索引，并用规模夹具检查查询计划不全表扫描历史 revision。

验证命令：

```powershell
npm run test:rust -- catalog
npm run test:rust -- phase_one
```

## 4. 任务三：把全图库候选接入导入预检与计划刷新

涉及文件：

- `src-tauri/src/import_preflight/planner.rs`
- `src-tauri/src/import_preflight/proof.rs`
- `src-tauri/src/import_preflight/mod.rs`
- `src-tauri/src/import_preflight/tests.rs`
- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/import_execution/tests.rs`

先增加 Windows 两卷或等价受控 Root 回归夹具：

1. 目标已有 `2026\20260809\DSCF1234.JPG`，其 EXIF 日期为 2026-08-10；来源相同文件预检指向旧路径并 Skip，不创建 20260810 副本。
2. JPG/RAF 两端都位于近似日期目录时均 Skip。
3. 只有一端位于近似日期目录时得到一端 Skip、一端 Copy 的同一 ImportGroup。
4. 同名同扩展同大小但字节不同，完整证明拒绝 Skip，并按精确日期目录完整名称族规划 Copy/Conflict。
5. 全图库候选在比较期间变化、离线、变为 reparse point 或 path/file ID 不再匹配时不得 Skip。
6. 预检后全图库候选变化导致 Copy/Skip 或目标路径改变时生成 Proposed generation 并返回确认，不静默扩大 Attempt。

实现步骤：

1. `assess_member` 的候选来源改为 `ImportHistory -> CatalogCandidateLookup -> 精确目标目录名称族`。
2. 按规范 path key 去重并保留确定顺序；同一个实际对象不能被重复哈希。
3. 所有 Catalog 候选继续进入现有 `prove_duplicate`，不得使用 candidate key、旧 content_hash 或拍摄时间直接 Skip。
4. 统一预检和执行 Validation 的候选刷新逻辑，避免初次预检能发现而 Validation 丢失全图库候选。
5. 保持完整名称族和统一 JPG/RAF 后缀选择不变；异目录重复候选只影响 Skip，不把现有历史文件移动到精确目录。
6. 在测试中冻结来源与旧目标的 SHA-256、大小、mtime，确认预检与执行零写入。

验证命令：

```powershell
npm run test:rust -- import_preflight
npm run test:rust -- import_execution
npm run safety:audit
```

## 5. 任务四：写入 ImportAssetResolutionFact

涉及文件：

- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/import_execution/resolution.rs`（新增）
- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/import_execution/tests.rs`
- `src-tauri/src/db/tests.rs`

先写失败测试：

1. Copy Step 在 `ImportIndexedFact` 同一事务得到 `PublishedIndexed` resolution。
2. Skip Step 只有在 Execution DuplicateProof、PathBindingProof 和当前 AssetRevision 一致时得到 `SkippedDuplicate` resolution。
3. Skip 目标尚未建档时，只读 `CatalogService.observe_asset` 后解析到该 Asset，不复制目标。
4. 在 SkippedDuplicate 提交后、Catalog observe 前或 resolution 提交前注入崩溃，启动协调可幂等补齐。
5. 目标路径后来解析到其他对象时不得改绑到同内容 Asset，进入 NeedsAttention。
6. 同一 Step 重试不创建第二个 resolution fact；冲突 fact 作为数据库完整性错误关闭写入门。

实现步骤：

1. 为 Copy 路径在索引事实事务中追加统一 resolution fact。
2. 为 Skip 路径增加“重复证明 -> 当前 Catalog Asset”解析阶段；授权输入只使用执行期 proof 和绑定 snapshot。
3. 若目标不在 Catalog，使用已经验证的目标相对路径做只读观察；前端或数据库路径字符串不得直接进入写操作。
4. 新增启动协调入口，从不可变 IndexedFact 或 Execution DuplicateProof 补齐缺失 resolution；只追加，不更新旧事实。
5. 将解析失败映射到明确报告和 NeedsAttention，不能把已经安全发布的文件回滚。

验证命令：

```powershell
npm run test:rust -- import_execution
npm run test:rust -- catalog
npm run test:rust -- db
```

## 6. 任务五：改造 SameImportGroup 四象限配对

涉及文件：

- `src-tauri/src/catalog/pairing.rs`
- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/catalog/coordinator.rs`
- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/catalog/phase_one_tests.rs`
- `src-tauri/src/import_execution/tests.rs`

先写矩阵测试：

| JPG | RAF | 期望 |
|---|---|---|
| Copy | Copy | 一个 JPG+RAF Photo |
| Copy | Skip | 一个 JPG+RAF Photo |
| Skip | Copy | 一个 JPG+RAF Photo |
| Skip | Skip | 一个 JPG+RAF Photo |

补充反例：重复角色、缺一端 resolution、日期可靠但冲突、当前 AssetRevision 变化、Asset Missing、Asset 已在冲突配对中，均不得自动合并。

实现步骤：

1. `load_same_import_pairs` 改为从 ImportGroup 两个 Step 的 resolution fact 取 Asset，不再强制两端 PublishedFact。
2. 对两端统一验证 resolution evidence snapshot 与当前 AssetRevision。
3. SameImportGroup 明确候选优先于 HistoricalExact 的全局同 stem 歧义，但不得占用已由另一份强证据配对的 Asset。
4. 导入 Attempt 结束前调用 import-group-only 协调；后台扫描完成时调用完整协调，补齐旧数据。
5. Photo 合并、拆分、版本、Alias、DeletionMark 失效规则沿用现有实现。
6. 记录修复前后所有照片文件 SHA-256、大小和 mtime，证明协调只写 SQLite。

交付门 A 完成验证：

```powershell
npm run format:check
npm run lint
npm run test:rust -- import_preflight
npm run test:rust -- import_execution
npm run test:rust -- catalog
npm run clippy
npm run safety:audit
```

## 7. 任务六：实现手动历史重复检测服务

涉及文件：

- `src-tauri/src/duplicate_review/mod.rs`（新增）
- `src-tauri/src/duplicate_review/scanner.rs`（新增）
- `src-tauri/src/duplicate_review/proof.rs`（新增；优先复用 import proof 的稳定读取公共组件）
- `src-tauri/src/duplicate_review/tests.rs`（新增）
- `src-tauri/src/io_scheduler/mod.rs`
- `src-tauri/src/commands/duplicate_review.rs`（新增）
- `src-tauri/src/commands/mod.rs`
- `src-tauri/src/lib.rs`

Tauri commands：

- `get_duplicate_review_status`
- `start_duplicate_review_scan`
- `cancel_duplicate_review_scan`
- `get_duplicate_review_results`
- `open_duplicate_asset_location`（接收 AssetId/AssetVersion，不接受可执行路径字符串）

先写失败测试：

1. 未点击开始时不读取照片；打开结果页只返回 Idle/已有结果。
2. 先按 `role + size` 分桶，单成员桶不进行完整哈希。
3. 两份、三份、改名但内容相同的 JPG/RAF 得到正确 Observation、Set 和 Proof。
4. 模拟相同摘要但字节不同，不进入同一 Set。
5. 哈希或逐字节比较期间身份变化，不发布该成员的可操作 proof。
6. 取消后 Run 为 Cancelled，`get_results` 不返回半成品操作集合。
7. 与扫描、Import、Recycle 的 I/O/任务门互斥，浏览状态查询仍响应。
8. 来源照片和图库文件在检测前后字节、mtime 不变。

实现步骤：

1. 从 import duplicate proof 抽取无业务 owner 的稳定双端比较原语，Import 和 DuplicateReview 分别保存自己的不可变证据。
2. DuplicateReviewService 使用命名后台 worker 和短状态锁；文件 I/O 不持有状态 Mutex 或 SQLite 写事务。
3. Run 冻结 LibraryId、RootBindingId 和 catalog revision；结果只在 Completed Run 发布。
4. JPG/RAF 分开形成等价集合；逻辑照片族聚合只使用设计规格中的两类唯一正向关系。
5. 完整路径仅作为展示字段返回，所有后续命令继续使用内部 ID/version 和当前 Root 验证。

验证命令：

```powershell
npm run test:rust -- duplicate_review
npm run test:rust -- io_scheduler
npm run safety:audit
```

## 8. 任务七：实现结果过期和保留建议

涉及文件：

- `src-tauri/src/duplicate_review/mod.rs`
- `src-tauri/src/duplicate_review/scanner.rs`
- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/commands/duplicate_review.rs`
- `src-tauri/src/duplicate_review/tests.rs`

先写失败测试：

1. 应用重启、墙上时间变化和无 Asset 变化的普通扫描不使结果过期。
2. Asset 新增、缺失、路径/file ID/大小/mtime/版本变化或 RootBinding 改变使结果显示 Expired。
3. Expired 结果保留审计行但不返回可选成员；未提交选择由前端清空。
4. 当前不可读/离线文件不成为建议保留项。
5. 在合格成员中按“EXIF 日期一致的规范路径 -> 路径长度 -> 规范 path key”稳定选出建议项。
6. UI/服务都拒绝选择一个等价集合的全部成员。
7. 冗余份数和理论可释放字节严格按每个媒体 Set 的 `(n-1)` 计算，不随建议项改变。

实现步骤：

1. 查询状态时比较 Run 的 binding/revision 与当前值；不要使用 elapsed time。
2. 把建议保留计算写成无 I/O 纯函数并做表驱动测试。
3. 结果 DTO 明确区分 `idle | running | completedEmpty | completedWithDuplicates | expired | cancelled | failed`。
4. Expired 只允许进入页面查看原因和手动重新开始，不能从旧结果创建 Mark。

验证命令：

```powershell
npm run test:rust -- duplicate_review
npm run test:rust -- catalog
```

## 9. 任务八：扩展 DuplicateSubset 待删除标记

涉及文件：

- `src-tauri/src/pending_delete/mod.rs`
- `src-tauri/src/pending_delete/tests.rs`（新增）
- `src-tauri/src/commands/pending_delete.rs`
- `src-tauri/src/duplicate_review/mod.rs`
- `src-tauri/src/db/tests.rs`

新增 command：

- `add_duplicate_subset_to_pending_delete`

请求只接收 Completed Run、Set、待删 AssetId/version 和保留 AssetId/version；不接收可授权的绝对路径。

先写失败测试：

1. 建立 Mark 前重新对待删端与保留端做完整哈希、逐字节比较和双端身份复核。
2. 一次位置整套选择可为关联 Photo 创建准确的 JPG/RAF 子集快照；单文件选择只冻结明确 Asset。
3. 同一 Photo 已有 WholePhoto Mark 时拒绝 DuplicateSubset，反向也拒绝。
4. 选择变化通过 release old + append new 实现，不更新旧 Mark/Snapshot/Guard。
5. 不能选择全部成员，不能把待删成员同时作为保留成员，不能使用其他 Set/role 的保留成员。
6. proof 过期、catalog revision 变化、任一文件变化或不可读时不创建 Mark。
7. PendingDeleteSnapshot 准确返回 `markScope`、每个待删完整路径、角色、大小和对应保留路径。
8. 普通 D/R 会话栈和 WholePhoto 测试保持不变；重复页面建立的 Mark 不伪造 D/R 会话撤销历史。

实现步骤：

1. 将 `create_mark` 拆为 WholePhoto 和显式 Asset subset 两个入口，共用身份 snapshot 写入。
2. DuplicateSubset 在文件 I/O 完成后用一个短事务原子写 Mark、待删 snapshots、保留 snapshots/guards 和 operation event。
3. PendingDelete DTO 增加来源/范围与路径清单；旧 WholePhoto 返回保持兼容。
4. 待删除摘要的逻辑项、物理文件数和字节数同时覆盖两类 Mark，不重复计数。

验证命令：

```powershell
npm run test:rust -- pending_delete
npm run test:rust -- db
```

## 10. 任务九：回收执行前重新证明保留副本

涉及文件：

- `src-tauri/src/recycle/mod.rs`
- `src-tauri/src/recycle/tests.rs`
- `src-tauri/src/commands/recycle.rs`
- `src-tauri/src/pending_delete/mod.rs`
- `src-tauri/src/db/tests.rs`

先写失败测试：

1. Batch 创建时精确复制 DuplicateSubset snapshots 和 retention guards；WholePhoto 不需要 guard。
2. 同一 Batch 选择了 guard 指向的保留 Asset 时，确认事务失败且不调用 Shell。
3. Mark 后保留端缺失、离线、身份变化、内容变化或无法稳定打开时，待删项进入 NeedsAttention，不调用 Shell。
4. 待删端变化仍由现有 mark snapshot 校验阻止。
5. 重新证明成功后才允许当前待删物理文件进入 IFileOperation。
6. JPG 成功、RAF guard 失败等部分结果准确进入现有 Partial/NeedsAttention 报告，不自动重试剩余项。
7. Windows 回收站不可用或返回对象不明时继续禁止永久删除降级。
8. 回收完成后的定向扫描递增 catalog revision，使旧重复结果变为 Expired。

实现步骤：

1. 在 `PreDeleteItem` 验证阶段识别 mark scope；DuplicateSubset 额外加载 guard。
2. 使用稳定双端比较原语重新完整哈希和逐字节比较；保持验证句柄到删除 reservation/状态提交所需的最近安全边界。
3. 任何 guard 错误通过现有 attempt/batch 原子状态映射记录，不删除对应 Asset。
4. 执行报告和确认模型增加“重复清理文件/保留副本”数量，但不暴露内部 proof ID。

验证命令：

```powershell
npm run test:rust -- recycle
npm run test:rust -- pending_delete
npm run safety:audit
```

## 11. 任务十：实现重复复核页面与待删除展示

涉及文件：

- `src/shared/commands/duplicate-review.ts`（新增）
- `src/shared/commands/library.ts`
- `src/shared/commands/recycle.ts`
- `src/features/duplicate-review/DuplicateReviewView.tsx`（新增）
- `src/features/duplicate-review/DuplicateReviewView.test.tsx`（新增）
- `src/features/pending-delete/PendingDeleteView.tsx`
- `src/features/pending-delete/PendingDeleteView.test.tsx`
- `src/shared/ui/TopBar.tsx`
- `src/app/App.tsx`
- `src/app/App.test.tsx`
- `src/app/styles.css`

先写 UI 测试：

1. 顶部动作顺序为“导入照片 -> 重复入口 -> 设置”。
2. 入口正确显示 `检查重复`、`重复文件 N` 和 `重复检查过期`；completedEmpty 回到低强调 `检查重复`。
3. 进入页面和打开 expired 结果不自动调用 start command；只有“开始检测”触发扫描。
4. 运行态显示阶段、已检查数、候选数、当前路径和取消；完成态显示规格要求的六项摘要。
5. 结果卡显示 `JPG：n 份（重复 n-1 份）`、RAF 数量、两种都重复状态和所有完整路径。
6. 建议保留不产生 checkbox 默认值；证据过期/离线成员不可选择。
7. “选择这一整套副本”和单 JPG/RAF 选择都生成正确确认摘要；选择全部成员时按钮禁用并解释原因。
8. 加入待删除确认列出每个删除路径、保留路径、角色、文件数和总大小。
9. PendingDeleteView 用“重复清理”区分 DuplicateSubset，并同时展示待删目录和保留目录；普通 WholePhoto 视觉保持不变。
10. 所有按钮具备可访问名称、键盘焦点和错误状态；路径使用换行/复制友好展示，不把路径字符串回传为授权。

实现步骤：

1. App 新增 `duplicate-review` route 和状态轮询，离开页面时不取消用户明确启动的后台检测。
2. TopBar 增加 duplicate entry DTO/handler，按按钮数组顺序渲染在 Import 后。
3. DuplicateReviewView 分离 ScanState、Summary、FamilyCard、LocationSet 和 AddToPendingDialog，避免单组件持有所有状态。
4. 待删除数据模型从单一目录摘要扩展为可渲染 Asset 清单和 retention 清单。
5. 结果变 Expired 时立即清空未提交选择并显示原因；用户点击后仍需手动开始。

验证命令：

```powershell
npm test -- src/features/duplicate-review/DuplicateReviewView.test.tsx
npm test -- src/features/pending-delete/PendingDeleteView.test.tsx
npm test -- src/app/App.test.tsx
npm run lint
```

## 12. 任务十一：完成图库布局与 Inspector 优化

涉及文件：

- `src/features/library/LibraryView.tsx`
- `src/features/library/LibraryView.test.tsx`
- `src/features/library/PhotoInspector.tsx`
- `src/features/library/PhotoInspector.test.tsx`
- `src/shared/ui/TopBar.tsx`
- `src/app/styles.css`
- `src/app/App.test.tsx`
- `scripts/ui-check.mjs`

先写失败测试：

1. 照片工作区顶部不再渲染 `scan-banner`。
2. 增量扫描运行时底部中央显示“正在更新图库 · 已检查 n 个文件 · 查看进度”，结束后中央留空。
3. `← → 浏览 · D 标记 · R 撤销` 位于待删除按钮下方，底部不再出现该提示。
4. 有 EXIF 时 Inspector 只显示拍摄日期、相机本地时间和照片大小。
5. 无 EXIF 时不显示任何日期、时间或“未记录”占位，只显示照片大小。
6. `文件时间`、旧 `容量` 标签和“按相机记录的本地时间显示”均不存在。
7. Photoshop 按钮具有专用 class 和一行标签，其他按钮 class/字号不变。
8. D 键帽内容有明确居中 class。

样式实现：

- `.photo-workspace` 右列改为 `clamp(300px, 20vw, 360px)`。
- metadata label 列改为 58 px。
- 镜头值使用 10 px、单行、不换行专用样式。
- Photoshop action 使用 12 px 和 `white-space: nowrap`。
- D 键帽使用 grid/flex 居中和 `line-height: 1`。
- 扫描状态改为 status-bar 中央内容，不使用 fixed overlay。

视觉验收：

1. 使用生成 demo 数据渲染 `SIGMA 18-50mm F2.8 DC DN | Contemporary 021`。
2. 在 1100、1366、1920 px 应用宽度检查中央照片仍可用、镜头和 Photoshop 标签不换行。
3. 在 Windows 100%、125%、150% 显示缩放执行截图检查。
4. 不用减小全局字体或隐藏溢出文本来伪造通过。

验证命令：

```powershell
npm test -- src/features/library/LibraryView.test.tsx
npm test -- src/features/library/PhotoInspector.test.tsx
npm test -- src/app/App.test.tsx
npm run lint
```

## 13. 任务十二：集成、性能、安全和发布验证

### 集成场景

1. 近似日期历史 JPG/RAF + 相同内存卡来源：两端 Skip、无新副本、一个可靠配对 Photo。
2. 仅历史 JPG + 来源 JPG/RAF：JPG Skip、RAF Copy、一个可靠配对 Photo。
3. 仅历史 RAF 的反向场景。
4. 已有两套逐字节重复照片：手动检测显示 JPG/RAF 份数和全部目录，不自动选择。
5. 用户选择一个旧目录整套副本，确认页和待删除页显示删除/保留完整路径；回收后只保留选择的规范路径文件。
6. 在待删除后替换或移走保留副本：回收停止，待删副本仍存在并进入 NeedsAttention。

### 性能验收

- 全图库导入候选查询使用覆盖索引，不因每个来源文件遍历全部 AssetRevision。
- DuplicateReview 对 size 单成员桶不读取内容；完整哈希进度持续更新且可取消。
- 机械盘重复检测期间窗口、状态查询和取消保持响应。
- 规模夹具记录扫描文件数、完整哈希文件数、候选集合数、耗时和峰值状态内存。

### 完整验证命令

```powershell
npm run format:check
npm run lint
npm test
npm run test:e2e
npm run test:rust
npm run clippy
npm run safety:audit
npm run build
```

Windows 专用受控环境可用时额外运行：

```powershell
npm run test:windows
```

生产构建前检查：

- `git diff --check` 无错误。
- 工作区只包含本计划文件。
- schema/migration checksum、应用版本策略和 README 行为说明同步。
- 所有测试目录均为生成或明确可丢弃目录。
- 不在唯一真实照片上执行回收、故障注入或写入验收。

## 14. 完成标准

### 交付门 A

- 任意历史子目录中的同名、同大小、拍摄时间相容 Asset 可成为导入候选。
- 只有当前强重复证明能导致 Skip。
- 四种 Copy/Skip JPG/RAF 组合均通过解析事实形成正确 Photo。
- 导入不覆盖、不移动历史文件，来源和既有目标零写入。

### 交付门 B

- 重复检测只由用户启动，只发布完整 SHA-256 + 逐字节相同且身份稳定的结果。
- 结果准确显示 JPG/RAF 份数、全部位置、冗余份数和理论空间。
- 建议保留不自动勾选；不能选择删除最后一份。
- DuplicateSubset 先进入待删除复核，执行前重新证明保留副本。
- 最终处理只进入 Windows 回收站，任何不确定性均保留待删文件。
- 顶部入口、底部扫描状态、快捷键位置、照片大小、EXIF 显示、镜头单行和 Photoshop 按钮布局符合批准规格。
