# 本地 Fuji 照片管理器 v1：阶段 8 实施计划

日期：2026-08-08
依据：`docs/superpowers/specs/2026-08-08-local-photo-manager-phase-eight-design.md`

## 1. 实施边界

本计划只完成规模硬化、恢复/迁移验证、可访问性验收和 Windows v1 候选发布质量，不增加搜索、历史重复扫描、相似照片、完整 RAW、多图库、手动清理缓存或永久删除。

所有测试夹具和原始运行证据位于 `G:\test\local-photo-manager-phase8\<run-id>`；实际分配硬上限 300 GB。`G:\test\Fuji-XM5` 与其他既有对象只读保护。D 盘只用于源码、编译及最终小型证据。任何需要删除夹具的代码都必须先证明本 Run 的 Root 标记、路径 containment、卷/目录身份和逐文件所有权。

每一任务先写失败测试，再实现最小变更并运行相关测试。阶段 8 不复用旧阶段报告冒充候选证据。

## 2. 任务一：冻结基线与阶段 8 TestRoot 防线

修改：

- `src-tauri/src/safety/test_root.rs`
- `src-tauri/src/safety/mod.rs`
- 新建 `src-tauri/src/safety/phase_eight.rs`
- 新建 `scripts/phase-eight-preflight.ps1`
- `package.json`

工作：

1. 新增 PhaseEightRunRoot，冻结 canonical parent、volume identity、directory identity、Run ID、300 GB allocated 上限和创建时空闲空间。
2. 验证父目录精确为 `G:\test\local-photo-manager-phase8`；拒绝 D 盘、非 NTFS、reparse point、复制标记、父身份变化和越界路径。
3. 使用 Windows allocated-size API，而不是逻辑文件长度，计算专属 Run 实际占用；生成前后都执行上限门。
4. 通过 Storage Property 查询 G 盘是否有 seek penalty/旋转介质，并把原始设备证据写入 Run manifest。无法证明机械盘时使 HDD 项失败。
5. 为外部 PowerShell 预检提供相同检查：路径、格式、空闲空间、既有 `Fuji-XM5` 快照和 Run manifest。
6. 添加测试，证明任意目录、D 盘、reparse、复制 marker、300 GB 越界、父/卷身份变化均被拒绝。

验证：

- `npm run test:rust -- phase_eight`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/phase-eight-preflight.ps1 -Parent G:\test -DryRun`

## 3. 任务二：迁移清单、schema 指纹和候选绑定模型

修改：

- `src-tauri/src/db/migration.rs`
- `src-tauri/src/db/tests.rs`
- `src-tauri/src/safety/gates.rs`
- `src-tauri/src/safety/mod.rs`
- 新建 `src-tauri/src/safety/candidate.rs`

工作：

1. 将全部已发布迁移公开为有序 manifest：version、文件名、逐文件 SHA-256 和组合 SHA-256；不再把“最新迁移 checksum”误称为完整迁移链。
2. 生成规范 schema checksum，覆盖所有 `sqlite_schema` DDL、schema version 和迁移历史，排除运行时行数据。
3. 扩展 BuildFingerprint：安装包、实际安装 EXE、两份锁文件、schema checksum、逐迁移 manifest、报告协议版本。
4. 定义候选 manifest、ImportAdmission 和 RecycleAdmission 的严格反序列化与 hash 复核；未知字段、缺项、重复项和 hash 不一致全部拒绝。
5. 保持真实图库 Import/Recycle 默认关闭。准入存在但未得到后续单独用户确认时不得打开真实数据门；TestRoot 自测路径仍由 TestRoot 防线独立控制。
6. 添加逐字段失效测试，证明二进制、安装包、锁文件、schema、任一迁移或任一绑定报告变化都会使旧准入失效。

验证：

- `npm run test:rust -- safety::gates`
- `npm run test:rust -- db::tests::migration`

## 4. 任务三：SQLite checkpoint、备份恢复、迁移与空间不足硬化

修改：

- `src-tauri/src/db/connection.rs`
- `src-tauri/src/db/backup.rs`
- `src-tauri/src/db/migration.rs`
- `src-tauri/src/db/tests.rs`
- `src-tauri/src/safety/fault_injection.rs`

工作：

1. 让受控 checkpoint 返回 busy/log/checkpointed page 计数；有界长读场景不得无限阻塞，并记录可审计结果。
2. 为备份增加完整验证和离线 restore-to-new-file 流程：quick_check、FK、迁移历史、schema checksum、关键表清单全部通过后才允许替换；替换前保留原损坏数据库，不原地覆盖。
3. 从 schema 0–7 的真实快照逐级迁移，逐版本校验行数、FK、唯一约束、状态枚举、Fact 不可变触发器和 migration checksum。
4. 在升级前备份、每项迁移、COMMIT、恢复文件切换的边界注入故障；失败必须保持旧版本可恢复或只读，不暴露半迁移。
5. 添加确定性 DiskFull/IOERR/CORRUPT/BUSY 注入。写任务一旦遇到耐久错误，当前项后立即停止，共享健康状态同时关闭 Import/Recycle。
6. 验证 7 个日常、3 个升级前版本，及 Root、VolumePolicy、ViewingCursor、标记快照、Attempt、Reservation、Fact、报告、CacheEntry/Intent 的完整恢复。
7. 验证报告长期保留；备份 UI/报告不得称为照片备份。

验证：

- `npm run test:rust -- db::tests`
- `npm run safety:audit`

## 5. 任务四：共享 I/O 调度器与浏览响应

修改：

- 新建 `src-tauri/src/io_scheduler/mod.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/catalog/coordinator.rs`
- `src-tauri/src/catalog/scanner.rs`
- `src-tauri/src/cache/preview.rs`
- `src-tauri/src/cache/mod.rs`
- `src-tauri/src/import_preflight/proof.rs`
- `src-tauri/src/import_preflight/planner.rs`

工作：

1. 实现既有五级 I/O 优先级、可取消 permit、浏览活动窗口和空闲恢复。
2. 旋转介质默认低并发、顺序读取优先；SSD 保持有界并发。并发度来自设备属性和测量校准，不使用无界线程。
3. 当前照片/附近预览可抢占或暂停后台哈希；标记与分页元数据不等待低优先级扫描。
4. 取消/暂停在 2 秒内传播到扫描、EXIF、哈希和缓存生成；已进入不可中断文件动作的步骤完成当前安全边界后停止。
5. 添加确定性调度测试：优先级、FIFO/顺序性、浏览抑制、空闲恢复、取消、旋转/SSD 并发上限和无饥饿。
6. 记录队列深度、等待时间、运行时间、读字节和介质类型，供阶段 8 性能报告使用。

验证：

- `npm run test:rust -- io_scheduler`
- `npm run test:rust -- catalog`
- `npm run test:rust -- cache`

## 6. 任务五：缓存容量、余量与所有权校准

修改：

- `src-tauri/src/cache/mod.rs`
- `src-tauri/src/cache/preview.rs`
- `src-tauri/src/commands/cache.rs`
- `src-tauri/src/db/tests.rs`

工作：

1. 保持 32 GB 默认、64 GB 绝对上限和 20 GiB/10% 取大余量的冻结字节常量。
2. 为 1 TB G 盘校准余量、incoming bytes、allocated size 和容量缩减；不得按稀疏 logical size误算实际缓存占用。
3. 覆盖 Active/Draining 同键 incarnation、Evicted/Missing 重建、NeedsAttention 阻塞、离线恢复、盘符复用和身份变化。
4. 扩展故障测试到 Intent COMMIT、句柄、身份验证、delete disposition、关闭、替代对象和 Fact COMMIT；未知文件及父目录内容逐字节不变。
5. 验证缩容和 Root 切换为渐进任务，设置调用不等待大规模同步清理。

验证：

- `npm run test:rust -- cache`
- `npm test -- src/app/App.test.tsx`

## 7. 任务六：10 万 Photo、20 万 Asset、1 TB 规模工具

修改：

- 新建 `src-tauri/src/phase_eight/mod.rs`
- 新建 `src-tauri/src/phase_eight/scale.rs`
- 新建 `src-tauri/src/phase_eight/metrics.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/main.rs`
- 新建 `scripts/run-phase-eight.ps1`

工作：

1. 为已安装 EXE 增加严格受 TestRoot 保护的 `--phase8-self-test` 入口。入口只能操作通过 PhaseEightRunRoot 验证的对象，不能接受任意输出路径。
2. 分批生成精确 100,000 Photo、200,000 Asset、关系、Revision、稳定排序键和至少 1 TB logical bytes；使用分层目录和 NTFS sparse 文件，实际 allocated size 保持远低于 300 GB。
3. 生成一组实际分配、确定性内容的 I/O 样本，记录实际读字节；不宣称实际读取 1 TB。
4. 验证时间线稳定分页、PhotoId tie-break、ViewingCursor、待删除派生计数、启动只加载当前 Revision/关系、LRU、定向重扫和恢复索引。
5. 在同一已安装 EXE 进程中测量 8 项性能指标，输出原始样本、p50/p95、峰值 working set 和门槛结论。
6. 前后记录 G 盘空闲空间、Run allocated size 和 `Fuji-XM5` 保护快照；任一越界立即失败。

验证：

- `npm run test:rust -- phase_eight::scale`
- 安装后运行 `local-photo-manager.exe --phase8-self-test scale --run-root <G-run-root> --output <G-report-path>`

## 8. 任务七：DPI、键盘、对比度和窗口验收

修改：

- 新建 `scripts/phase-eight-ui-check.mjs`
- 新建 `scripts/phase-eight-candidate-ui.ps1`
- `src/app/App.test.tsx`
- 必要时仅修改有失败证据的现有 CSS/组件

工作：

1. 通过 CDP 驱动已安装候选的 WebView2，使用 100%、125%、150% device scale，覆盖 1440×900、1280×720、1100×700。
2. 截图和 DOM 测量写入 G 盘 Run；最终只复制压缩报告和必要验收图到仓库。
3. 自动计算文本/背景对比度，验证 4.5:1 与 3:1 门槛；检查焦点环、键盘可达性、目标尺寸、overflow、底部动作和关键状态非颜色依赖。
4. 驱动 `←/→/D/R`、auto-repeat、pending、输入框、模态作用域及焦点返回。
5. 覆盖长路径、无 EXIF、JPG-only、RAF-only、配对、0/1/12/大量待删除、问题项、导入、设置和报告。
6. 人工复核候选截图并在报告中记录审阅者、时间和逐矩阵结论；不能用源码快照替代候选截图。

验证：

- `npm run test:ui`
- `node scripts/phase-eight-ui-check.mjs --installed-exe <exact-path> --run-root <G-run-root>`

## 9. 任务八：Windows v1 候选打包、安装、升级和卸载

修改：

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- 新建 `scripts/phase-eight-candidate.ps1`

工作：

1. 将产品版本统一为 1.0.0，启用 Windows NSIS 候选包；锁文件随版本变更更新并在最终构建前冻结。
2. 从阶段 7 提交构建一次旧版安装包用于升级路径验证；该构建发生在最终候选之前，不作为候选证据。
3. 在源码测试全部通过后只构建一次最终候选。记录 installer hash 后不得再次构建。
4. 静默安装旧版、写入可丢弃 AppData 状态、升级到候选并验证保留；卸载候选后验证用户数据库/备份保留规则。
5. 全新安装候选，定位真实安装目录和 EXE，验证 Authenticode 状态、文件版本、单实例和卸载登记。
6. 冻结 installer/EXE/locks/schema/migration manifest。候选后任何源码或产物变化都使流水线退出并要求从构建步骤重跑。

验证：

- `npm run tauri -- build --bundles nsis`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/phase-eight-candidate.ps1`

## 10. 任务九：准确已安装二进制的完整安全重测

修改：

- `src-tauri/src/phase_eight/import_suite.rs`
- `src-tauri/src/phase_eight/recycle_suite.rs`
- `src-tauri/src/phase_eight/database_suite.rs`
- `src-tauri/src/phase_eight/cache_suite.rs`
- `scripts/run-phase-eight.ps1`

工作：

1. 把阶段 6/7 的可复用夹具从 `#[cfg(test)]` 提取为只接受 PhaseEightRunRoot 的候选自测套件；生产服务路径保持同一实现，不复制简化算法。
2. Import 套件覆盖全部状态/故障边界、双端重复证明、源卷零写入、源树与既有目标逐字节不变、partial、no-replace、名称抢占、空间不足、恢复和补索引。
3. Recycle 套件覆盖强制 flags、回收站关闭/配额/锁/Shell 取消/空对象、身份变化、回调/COMMIT 故障、部分结果、永久删除禁止和真实回收站恢复哈希。
4. Cache 套件覆盖逐文件所有权、Intent/Fact、替代对象竞态、无关文件和父目录不变。
5. Database 套件覆盖 WAL/checkpoint、备份恢复、逐级迁移、损坏只读和 AppData 空间不足。
6. 每次调用先复核当前进程映像绝对路径和 frozen EXE hash；结果写入候选 manifest 所属 Run。运行结束再次复核所有绑定 hash。

验证：

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run-phase-eight.ps1 -Suite all -InstalledExe <exact-path> -RunRoot <G-run-root>`

## 11. 任务十：27 条验收、独立准入和最终报告

修改：

- 新建 `scripts/phase-eight-report.mjs`
- 新建 `scripts/phase-eight-verify.mjs`
- 新建 `docs/superpowers/reports/phase-eight-final-report.md`
- 新建 `docs/superpowers/reports/phase-eight-known-limitations.md`
- 新建 `docs/superpowers/reports/real-photo-enable-checklist.md`
- 生成 `artifacts/phase-eight-*.json`
- `package.json`

工作：

1. 为上位规格第 20 节 27 条条件建立逐条证据映射；每条必须有当前 frozen candidate 的直接机器证据及必要人工证据。
2. 单独生成 Import 和 Recycle 报告/准入；共同绑定 installer、installed EXE、package-lock、Cargo.lock、schema、逐迁移 manifest 和全部报告 hash。
3. 缺陷清单强制 Critical/High 未解决数为零；缺证、跳过、旧报告、hash 漂移或机器类型不符均失败。
4. 总报告披露 logical/allocated/read bytes、硬件、全部性能原始统计、DPI 矩阵、SQLite 结果和未解决缺陷。
5. 已知限制明确 IFileOperation 非原子残余窗口、Windows 回收站不是照片备份、SQLite 备份不是照片备份、稀疏 1 TB 与实际读取样本的区别。
6. 真实照片启用清单要求用户对 Import 与 Recycle 分别确认；本阶段不执行确认、不导入或删除用户唯一照片。
7. `phase-eight-verify` 在最终提交前重新计算全部 hash；任一变化使准入和总报告失效。

验证：

- `npm run test:phase8`
- `node scripts/phase-eight-verify.mjs`
- `git diff --check`

## 12. 最终顺序与提交

最终顺序不可调整：

1. 完成任务一至七并运行全部源码测试；
2. 冻结源码、依赖锁和 schema；
3. 构建一次候选安装包；
4. 安装并定位准确 EXE；
5. 对已安装 EXE 完成任务九全部候选测试；
6. 生成人工 DPI 审阅记录；
7. 生成两份独立准入、27 条矩阵和总报告；
8. 重新计算并验证全部 checksum；
9. 确认 Critical/High 未解决为零；
10. 只暂存阶段 8 相关文件和最终小型证据，创建本地 Git 提交；
11. 不 push、不创建远端发布，也不打开真实照片门。

如果候选构建后需要修改任何源码、依赖、schema、迁移或构建配置，立即废弃全部候选证据，从步骤 3 重新开始。
