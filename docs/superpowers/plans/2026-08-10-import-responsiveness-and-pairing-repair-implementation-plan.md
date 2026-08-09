# 导入响应性与 JPG/RAF 配对修复实施计划

日期：2026-08-10  
依据：`docs/superpowers/specs/2026-08-10-import-responsiveness-and-pairing-repair-design.md`

## 1. 实施边界

本计划依次修复只读预检阻塞、确认导入阻塞和导入配对缺失。保留全量哈希、计划验证、空间门、partial、no-replace 发布、来源零写入、不可变事实与启动恢复语义。不修改用户照片，不使用真实内存卡执行自动测试。

工作区现有未跟踪文件 `.superpowers/**`、`local-test-install/**` 与 `scripts/ui-check.mjs` 属于用户，实施、格式化、暂存和提交时均明确排除。

## 2. 任务一：后台化只读预检 command

涉及文件：

- `src-tauri/src/commands/import_preflight.rs`
- `src-tauri/src/import_preflight/mod.rs`
- `src-tauri/src/import_preflight/tests.rs`

步骤：

1. 把 `start_import_preflight` 改为异步 Tauri command，在主线程完成扫描互斥和安全门检查后，克隆 `Arc<ImportPreflightService>` 与 owned permit。
2. 使用 `tauri::async_runtime::spawn_blocking` 执行现有同步 `run_preflight`，正确映射任务 join 失败为 `DomainError::Internal`。
3. 保持状态查询与取消为短同步命令；检查预检实现不在文件 I/O 期间持有状态 Mutex。
4. 增加/调整状态与取消测试，确保 Running、Cancelled、Failed、Completed 转换和重复启动门保持正确。
5. 运行 import_preflight Rust 测试和格式检查。

验证命令：

```powershell
npm run test:rust -- import_preflight
```

## 3. 任务二：拆分任务接受与后台导入执行

涉及文件：

- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/commands/import_execution.rs`
- `src-tauri/src/import_execution/tests.rs`
- `src/features/import/ImportView.tsx`
- `src/features/import/ImportView.test.tsx`

步骤：

1. 从同步 `start` 中提取短暂的任务接受阶段：验证 permit/digest，创建或确认持久化 generation，设置 `ValidatingPlan`。
2. 为 `ImportExecutionService` 增加进程内原子 worker 门；命令路径在接受任务后启动命名后台线程并立即返回状态。
3. 后台线程执行现有刷新计划、验证、空间检查、Attempt、复制、验证、发布和索引流程；所有退出路径释放 worker 门，并把领域错误转换为可查询状态。
4. 同步 `start`/`retry` 入口保留给现有确定性测试；生产 command 使用新的后台入口。重试 command 同样不得在主线程执行全量刷新或复制。
5. 前端在接受状态返回后立即关闭确认框并显示 `validatingPlan`/`running`；轮询覆盖后台运行阶段，启动失败时保留可操作错误。
6. 增加受控 worker 测试，覆盖立即返回、重复启动拒绝、错误释放 worker 门、取消和恢复状态；更新前端确认与轮询测试。

验证命令：

```powershell
npm run test:rust -- import_execution
npm test -- src/features/import/ImportView.test.tsx
```

## 4. 任务三：贯通新导入媒体元数据

涉及文件：

- `src-tauri/src/import_preflight/mod.rs`
- `src-tauri/src/import_preflight/planner.rs`
- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/catalog/mod.rs`
- 相关 import_preflight/import_execution 测试

步骤：

1. 先增加导入集成断言：新 JPG/RAF Asset 首版的拍摄时间、机型、序列号、预览与拍摄参数不为空且与源解析结果一致。
2. 扩展内部 `SourceFile`，保留 `MediaInspection`；预检草稿对前端的序列化结构不增加可编辑字段。
3. `inspect_source_file` 在同一次只读句柄生命周期内返回媒体解析、完整哈希和前后身份，避免重复打开源文件。
4. `index_published` 根据冻结的 SourceFile 构造完整 `CatalogObservation`，把媒体元数据写入目标 Asset 首个不可变版本。
5. 运行预检、媒体、目录和导入执行测试。

验证命令：

```powershell
npm run test:rust -- import_preflight
npm run test:rust -- import_execution
npm run test:rust -- catalog
```

## 5. 任务四：实现 SameImportGroup 配对与旧数据修复

涉及文件：

- `src-tauri/src/catalog/pairing.rs`
- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/catalog/scanner.rs`
- `src-tauri/src/import_execution/mod.rs`
- `src-tauri/src/catalog/phase_one_tests.rs`
- `src-tauri/src/import_execution/tests.rs`

步骤：

1. 先增加回归夹具：导入同名 JPG/RAF 后时间线只有一个 Photo；旧缺陷数据在下一次增量扫描后合并；身份变化、缺项和不完整事实链不合并。
2. 在 pairing 层加载 `import_group.pair_evidence='SameImportGroup'` 的候选，通过 PublishedFact、IndexedFact、当前 AssetRevision 和发布 FileIdentitySnapshot 验证两端身份。
3. 将配对候选区分为 `HistoricalExact` 与 `SameImportGroup`，创建对应不可变 PairEvidence；现有历史时间配对语义不变。
4. 提供仅合并可信 ImportGroup 的目录服务入口。成功导入索引完成后调用该入口；启动后台扫描完成时也调用，以修复旧数据。
5. 修复只更新 SQLite Photo/关系/证据，不读取、写入、改名或触碰照片文件。
6. 对导入后的目标文件记录 size、mtime、SHA-256，验证修复前后完全不变。

验证命令：

```powershell
npm run test:rust -- catalog
npm run test:rust -- import_execution
```

## 6. 任务五：顺序集成与完整回归

步骤：

1. 按任务一至四逐项运行定向测试，前一项通过后才进入后一项。
2. 对本次修改的 Rust 文件运行 rustfmt，对 TypeScript/Markdown 文件运行 Prettier；不格式化用户未跟踪文件。
3. 运行前端、e2e、Rust、Clippy、安全审计和生产构建。
4. 检查 `git diff --check` 与工作区状态，确认只包含本次明确文件。
5. 若 Windows GUI 环境可用，使用生成或可丢弃照片副本验证预检进度、确认后立即进入验证页、导入完成后 JPG/RAF 单 Photo；不得用用户唯一照片做验证。

完整验证命令：

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

## 7. 完成标准

- 大容量预检和导入期间窗口消息、状态轮询和取消保持响应。
- 点击开始导入后不再以“正在确认”覆盖真实验证/复制阶段。
- 全量安全验证和文件发布约束无回归。
- 新导入同组 JPG/RAF 显示为一个 `JPG ✓ / RAF ✓` Photo，并具有完整媒体元数据。
- 已有错误导入组在应用启动后台扫描后安全合并；身份不可靠者保持拆分。
- 所有自动测试只操作生成夹具或可丢弃副本，用户照片零写入。
