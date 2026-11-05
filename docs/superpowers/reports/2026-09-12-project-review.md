# 项目审查：需求、性能与实现逻辑

审查日期：2026-09-12。对象：当前工作区，package/Cargo 版本 1.0.3，包含原有未提交改动。业务源码未修改；临时复现测试已从正式测试目录移除。

## 结论

当前最影响使用的问题有明确代码原因：导入查重和历史重复检测反复读取同一文件，导入规划又重复枚举目标日期目录；部分重读绕过共享 I/O 调度。对机械硬盘，这些操作量与用户反馈的“查重慢、磁盘繁忙、实际复制正常”相符。尚未对用户真实图库做性能采样，因此不能给出实测加速倍数。

安全约束本身值得保留。首先应合并重复读取、明确每阶段的证明责任、减少目录枚举，并补齐任务状态流转。与此同时，恢复死路、当前图库隔离、更新计划确认等问题比普通界面优化更需要处理。

## 已确认的实际需求

- 导入来源仅为相机内存卡，接受来源卷永久拒写及来源/图库不同卷。
- 默认按整张逻辑照片处理 JPG＋RAF，一起标记和删除。
- 图库为本机机械硬盘，几千张照片，JPG/RAF 合计约 500GB。
- 最主要痛点是导入查重及历史重复检测耗时；卡到硬盘的实际复制速度基本正常。
- 500GB 是图库总量，不代表每次插卡需要处理 500GB。下面的读量估算均按本次实际处理数据计算。

## 1. 与卡顿直接相关的实现问题

### F01：导入对已存在文件反复建立完整重复证明

证据级别：完整调用链静态核对；以下为应用逻辑读量，未测量设备实际读量。文件系统缓存、错误候选、失败重试会影响实际耗时。

| 本次文件类型 | 来源完整读取 | 图库/目标完整读取 | 目标写入 |
| --- | ---: | ---: | ---: |
| 全新文件，无重复候选，走完导入流程 | 约 5 遍 | 约 2 遍 | 1 遍 |
| 已存在文件，首个候选即相同，走完确认与跳过 | 约 10 遍 | 约 8 遍 | 0 |

全新文件的来源读取由预检哈希、确认后全量刷新哈希、复制、发布前哈希和逐字节比较组成。已存在文件还在预检、刷新计划、持久化验证和执行跳过四处重新证明相等；单次证明又先计算双端哈希，再从头读取两端比较字节。

例如，本次卡上有 10GB 已存在照片，首个候选均匹配且走完整流程，代码可能请求约 100GB 来源读取和 80GB 图库读取。这个例子不能直接解释为实际物理磁盘一定读取 180GB；也不能把完整流程读量都算到最初一次预检中。仅最初预检，匹配文件约为来源 3 遍、图库 2 遍。

主要证据：[源文件预检完整哈希](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/mod.rs:1146)、[确认后重新建立计划](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:1043)、[单次重复证明的重复读取](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/proof.rs:300)、[持久化验证](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:1697)、[执行跳过](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:1979)、[复制后的哈希和比较](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/file_ops.rs:127)。

建议：在同一次双端流式读取中计算两端 SHA-256 并逐块比较，继续保存读取前后身份和路径复核结果。先消掉单次证明内部多余的一遍读取，再检查相邻验证阶段能否传递仍然有效的证明和句柄；不要直接将跨用户确认阶段的旧哈希当成执行授权。

### F02：历史查重对真正重复的文件读约 4～6 倍数据

同一重复集合含 n 份文件，每份大小 S：初次哈希 nS；锚点和其余成员每次比较先双端哈希再双端比字节，共 4(n−1)S；发布前再哈希 nS。合计为 (6n−4)S。

两份相同文件的读取量是其合计容量的 4 倍；三份为约 4.67 倍。非候选文件不读取内容，哈希不同的候选通常只读一遍。因此不能将 4 倍直接套到整个 500GB 图库。

证据：[初轮哈希](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/scanner.rs:134)、[逐对比较](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/scanner.rs:187)、[比较内部再次哈希](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/proof.rs:75)、[末轮哈希](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/scanner.rs:216)。

项目已经有[一次读取同时哈希、比较和复核的实现](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/proof.rs:160)，目前主要用于批量加入待删除。检测阶段可统一使用它，并校验返回的新哈希仍匹配候选组，消费对应的新快照。若证据链正确合并，可将该模型的读取降到 (3n−2)S，理论约减半。这是应用读量目标，不是耗时减半承诺。

回收前还在用旧证明：[调用处](D:/A/MyProject/ai/Aifirtst/src-tauri/src/pending_delete/mod.rs:557)。每对完整读取约 6S，新流式证明约 2S；同样可以优化，但删除前的最终复核必须保留。

### F03：每个源文件重新枚举整个目标日期目录

[enumerate_complete_suffix_family](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/planner.rs:527) 每调用一次就 read_dir，并遍历所有目录项。N 个源文件落在已有 M 个文件的同一日期目录，一轮规划就可能遍历 N×M 个目录项，确认刷新再来一次。

这与用户的同日批量拍摄场景直接相关。导入预检并非无条件读取整个图库所有文件内容，但这种重复目录枚举依然造成大量元数据工作。

建议：一次规划内按目标目录构建名称索引，重复使用并累计本批预留名称；发布前继续检查文件真实占用、身份及不覆盖约束。

### F04：共享调度器没有覆盖所有大读，增量扫描也仍逐文件解析

图库扫描与历史查重使用批量 I/O 锁；导入规划的 prove_duplicate 并未接收共享调度器，导入复制/复核路径也没有统一取得该批量锁。不能仅凭存在 IoScheduler 就认为所有大任务已经互斥。

证据：[图库扫描批量锁](D:/A/MyProject/ai/Aifirtst/src-tauri/src/catalog/scanner.rs:99)、[重复扫描批量锁](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/scanner.rs:69)、[规划中的直接证明调用](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/planner.rs:395)。

启动后已有图库通常自动增量扫描：[启动策略](D:/A/MyProject/ai/Aifirtst/src-tauri/src/catalog/coordinator.rs:123)。但全量/增量最终都走同一遍历，文件进入数据库的“未变化”判断之前已经打开并解析 EXIF：[扫描读取](D:/A/MyProject/ai/Aifirtst/src-tauri/src/catalog/scanner.rs:307)。这不是重复读取全部照片内容，却仍会逐个触碰机械盘上的文件。

建议：按物理设备统一大任务调度；让扫描在身份、版本和解析器版本允许时复用未变化的元数据。先减少读量和冲突，再测并发配置，不能简单增加线程。

### F05：查重进度无法解释真正耗时的阶段

[Comparing 阶段](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/scanner.rs:187) 包括全部逐字节比较及末轮哈希，中间没有持续更新比较进度，完成后才切到 Publishing。已完成哈希并不等于整个检测完成。

建议显示当前阶段、已读/待读字节、当前文件、阶段耗时、等待其他任务的原因。验证和比较循环也应检查统一取消信号。导入 ValidatingPlan 当前不提供有效的完整取消流程。

### F06：缩略图读取完整 JPEG，浏览缓存不受容量约束

[get_photo_preview](D:/A/MyProject/ai/Aifirtst/src-tauri/src/commands/library.rs:536) 直接调用 read_controlled，绕过磁盘 CacheService；JPG 路径读取完整文件。缩略图和主预览使用相同接口。

[前端 previewCache](D:/A/MyProject/ai/Aifirtst/src/shared/commands/library.ts:224) 成功后永久保留 Promise 中的 Base64 图像字符串，没有按字节上限淘汰。虚拟列表卸载组件不会移除这些缓存。浏览照片越多，累计保留的图片数据越多；设置中的磁盘缓存容量无法限制这部分内存。

这是静态确认的无界保留路径，尚未在用户图库测峰值内存。建议分别生成有限尺寸缩略图和主预览，接入磁盘缓存，增加有字节预算的内存 LRU，并取消已不需要的请求。

## 2. 需要优先修复的业务逻辑

| 编号/优先级 | 可触发问题 | 证据与修复方向 |
| --- | --- | --- |
| L01 / P1，已复现 | A 图库有标记，切 B 再标记。当前待删除列表只显示 B 的一项，回收确认却算两项。 | [全局读取回收标记](D:/A/MyProject/ai/Aifirtst/src-tauri/src/recycle/mod.rs:1175) 没有限定 current_library。统一页面、确认、执行的图库范围；多 Root 后续虽会拒绝，确认范围已经错误。复现没有执行实际回收。 |
| L02 / P1，静态确认 | 预检后目标变化，后台产生新计划，页面显示新计划但再次确认仍发送旧摘要。 | [轮询只更新 execution](D:/A/MyProject/ai/Aifirtst/src/features/import/ImportView.tsx:100)，[提交旧 draftDigest](D:/A/MyProject/ai/Aifirtst/src/features/import/ImportView.tsx:206)。确认展示与提交必须引用同一份最新计划。 |
| L03 / P1，静态确认 | 非 Strong 来源（如代码中按非 NTFS 处理的来源）有未完成导入，重启后要求重选原卡，但重选又被未完成任务挡住。 | [启动只预填 Strong](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/mod.rs:1063)、[恢复需要 selected_source](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/mod.rs:476)、[选择来源被禁止](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/mod.rs:314)。需提供受任务约束的原来源重新验证入口。未对真实 exFAT 卡做复现。 |
| L04 / P1，静态确认 | 多文件导入中断后，尚未开始复制的 Planned 步骤也变 NeedsAttention，阻断全任务重试。 | [批量建立 Planned](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:1793)、[统一变 NeedsAttention](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:2742)、[全局拒绝重试](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:1459)。要区分尚未开始与文件创建结果不确定的步骤，并实现正常关闭时的工作线程收尾。 |
| L05 / P1，静态确认 | 等待空间、等待来源、需要处理的导入只能重试，不能正常放弃；未完成任务又阻止配置和重新预检。 | [UI 恢复按钮](D:/A/MyProject/ai/Aifirtst/src/features/import/ImportView.tsx:436)。不能仅接上现成取消 API；还需审核取消终态、旧 lease、reservation 的完整释放。 |
| L06 / P1，静态确认 | 100_FUJI 和 101_FUJI 中各有 DSCF0001.JPG，可能预检后在持久化导入计划时报唯一约束错误。 | [目录分组退化成 stem 展示名](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_preflight/planner.rs:180)、[按展示名重新分组](D:/A/MyProject/ai/Aifirtst/src-tauri/src/import_execution/mod.rs:303)。应分离组 ID 与名称；同时统一预留整批目标路径，避免同日同名被规划到同一路径。 |
| L07 / P2，静态确认 | 三份重复先清理一份后，页面认为协调后的结果有效，剩余两份却不能继续加入待删除。 | [显示使用 effective_revision](D:/A/MyProject/ai/Aifirtst/src-tauri/src/duplicate_review/mod.rs:279)，[提交仍比较原 run_revision](D:/A/MyProject/ai/Aifirtst/src-tauri/src/pending_delete/mod.rs:848)。统一结果版本解析和授权验证。 |
| L08 / P2，静态确认 | 导入中离开页面再回来，后台仍执行，界面却当成待恢复；导入或增量扫描完成后返回图库可能仍显示旧列表。 | [导入页初次加载](D:/A/MyProject/ai/Aifirtst/src/features/import/ImportView.tsx:71)、[扫描仅更新 scan](D:/A/MyProject/ai/Aifirtst/src/app/App.tsx:199)、[返回仅切路由](D:/A/MyProject/ai/Aifirtst/src/app/App.tsx:341)。需要应用级任务状态与索引版本驱动的刷新。 |
| L09 / P2，已复现 | 当天备份文件损坏或只写了一部分，再次创建日备份却返回成功。 | [只检查 exists](D:/A/MyProject/ai/Aifirtst/src-tauri/src/db/backup.rs:61)。已有备份应验证，生成应先写临时文件、验证后发布；失败产物不能算有效日备份。 |
| L10 / P2，已复现 | 等待 I/O 许可时点击取消扫描，最终状态为 Failed 而不是 Cancelled。 | [扫描将调度错误作为失败](D:/A/MyProject/ai/Aifirtst/src-tauri/src/catalog/scanner.rs:115)。取消信号应沿整个扫描流程统一映射并持久化取消状态。 |

P1 表示可能涉及操作范围或阻断正常使用，应优先处理；不代表已证明照片损坏。静态确认与实际复现分开标注。

## 3. 需求上不合理或需要明确的地方

### R01：安全要求规定了证明结果，却逐渐固化为多轮重复计算

完整哈希、逐字节比较、身份验证和禁止覆盖都是合理边界；“每个阶段重新从头计算全部内容”应被审视为实现策略。最新文件名预筛设计甚至明确要求第三阶段再哈希一次。应修改这类算法级规格，允许由同一稳定读取产生并消费等价证据，同时明确哪一次验证承担执行前授权责任。

建议把验收从“调用了几次哈希”改为“可证明内容相同、身份稳定、没有越过确认范围，并且一次流程的完整读取次数有上限”。

### R02：配对候选唯一性范围过大，文件名复用会拆散历史照片

[配对代码](D:/A/MyProject/ai/Aifirtst/src-tauri/src/catalog/pairing.rs:322) 先在整个图库按 stem 分组，组内必须刚好一 JPG 和一 RAF，之后才比较拍摄时间。

已复现：2025 年 DSCF0001.JPG/RAF 原本是一张照片；加入 2026 年同名、不同可靠拍摄时间的一对，再扫描后四个文件都成独立照片，原配对也被拆开。

这与“同名候选唯一”的现有保守设计有关，不应只称为编码笔误。对于长期积累的图库，应在文件名、可靠拍摄时间和相机信息过滤之后再检查唯一性。不同年份的明确证据不应仅因名称复用而互相干扰；证据不足仍保持分离。

### R03：当前“重复检测”的实际范围是精确同名重复

[专项设计](D:/A/MyProject/ai/Aifirtst/docs/superpowers/specs/2026-08-12-duplicate-filename-prefilter-design.md:15) 明确要求完整文件名和大小写完全相同，排除改名后内容相同的文件。这是有意的性能边界，不能误报为实现缺陷。

建议在界面及 README 明确标为“同名重复检测”。如以后需要覆盖 _2 后缀、扩展名大小写变化或人工改名，应该提供明确的完整内容检测模式，而不是让用户误以为当前扫描已覆盖全部重复。

### R04：普通选片与重复清理的 JPG/RAF 语义需要分开说明

本次确认的默认行为是 JPG＋RAF 同删。普通 D 使用 WholePhoto；历史重复清理使用 DuplicateSubset，允许只选择一种格式。

如果要求重复清理也整组处理，必须明确：当 JPG 重复但 RAF 独有时，该组不能仅凭 JPG 的重复证明一起删除。可保留这种格式级复核作为明确例外，或在整组模式下禁止选入。不能默默补删未选且没有保留副本证明的 RAF。Windows 回收的两个物理文件也不能承诺原子成功。

### R05：恢复、取消和性能验收需要覆盖真实任务闭环

规格写了恢复状态，但存在“提示重选，却禁止重选”“提示重试，却永久不满足重试条件”的死路。每个非终态都应明确用户能继续、等待、重新验证原来源或放弃剩余任务中的哪些动作。

性能验收也不应仅按照片数量或稀疏文件总容量判断。现有[性能脚本](D:/A/MyProject/ai/Aifirtst/src-tauri/src/phase_eight/performance.rs:78) 的缓存命中直接读文件，未缓存预览用 512KiB 生成字节写缓存；这不等同于真实 React→Tauri→整张 JPEG→Base64→浏览器解码链路，也不覆盖当前导入查重的读量。

## 4. 建议实施顺序

1. 补上 L01 图库隔离与 L02～L05 的任务/确认/恢复闭环，防止错误操作范围及任务卡死。
2. 为导入查重与历史查重增加逐阶段字节数、完整读取次数、队列等待时间和取消响应统计，用生成数据及可丢弃样本建立基线。
3. 统一流式哈希＋字节比较证明，先降低单次验证读量；再整合相邻阶段的重复证明，保留执行前验证。
4. 一次规划只枚举一次目标日期目录，维护批次目标名预留；同步修复跨目录同名组 ID。
5. 按机械设备协调扫描、导入、查重和预览，避免大任务无约束争用；补充比较阶段进度和及时取消。
6. 接入真正缩略图与受限内存/磁盘缓存，再评估图库分页和增量快照。几千张阶段，优先收益在文件 I/O 和任务闭环，不必先重做全部界面。

## 5. 验证范围与证据

- 前端现有测试：7 个文件，68 条通过。
- Rust 现有测试：155 条通过，2 条按原定义忽略。
- 临时生成数据复现：4 条通过，分别确认无效日备份被接受、同名跨年配对拆散、取消误报失败、旧图库标记进入新图库回收确认。
- [复现源代码](D:/A/MyProject/ai/Aifirtst/artifacts/project-review-2026-09-12/review_findings_probe.rs) 保留为审查附件。其断言记录当前缺陷行为，修复后的回归测试应改为期望的正确行为。
- 所谓 [e2e 测试](D:/A/MyProject/ai/Aifirtst/tests/e2e/capability-boundary.test.ts:1) 目前主要是读取源码/配置并进行正则断言，不能替代实际跨前后端业务流程测试。
- 本次未在用户照片上执行导入、移动、删除或回收；没有对真实 500GB 图库测量磁盘吞吐。恢复、跨目录导入和异步确认问题由调用链及约束核对，未冒充实机复现。
- 原有 7 个未提交修改文件保留。本次新增报告及复现附件，未修改业务实现。

