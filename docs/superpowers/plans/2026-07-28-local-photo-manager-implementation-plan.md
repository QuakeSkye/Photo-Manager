# 本地 Fuji 照片管理器 v1 分阶段实施计划

- 状态：整体架构与 v1 范围已最终确认；待按阶段执行
- 日期：2026-07-28
- 最终确认：2026-07-29
- 目标平台：Windows 10/11 x64
- 技术路线：Tauri 2 + React/TypeScript + Node.js 24 LTS + Rust + SQLite
- 上位规格：
  - ../specs/2026-07-28-local-photo-manager-design.md
  - ../specs/2026-07-28-local-photo-manager-ui-design.md
  - ../specs/2026-07-29-local-photo-manager-import-location-ui-design.md
  - ../specs/2026-07-29-local-photo-manager-operations-ui-design.md
  - ../specs/2026-07-29-local-photo-manager-schema-adr.md

## 1. 拆分结论

v1 不做一次性大交付，采用九个顺序阶段。先交付完全只读的产品，再开放只写 SQLite 的粗筛标记，之后分别开放安全导入和 Windows 回收站删除。

写功能采用“全局写入门 + Import 门 + Recycle 门”三级结构。功能私有缺陷可以单独关闭对应功能；Root 身份、SQLite、JobRunner、Reconciler、路径/对象身份校验或 Tauri 能力边界等共享安全层异常时，Import 与 Recycle 必须同时关闭。只读浏览和 SQLite 标记仍可在数据库健康时独立使用。

推荐顺序：

1. 工程与安全底座
2. 只读图库内核
3. 白色沉浸式浏览 UI
4. 待删除标记与复核
5. Photoshop 与文件位置
6. 只读导入预检
7. 安全导入执行
8. Windows 回收站删除
9. 规模硬化与 v1 发布

每阶段必须先通过自己的验收出口，再开始下一阶段。不得以“后面统一补测试”为理由提前让真实图库接触尚未通过门禁的写功能。

这九个阶段是对产品规格第 19 节七个交付阶段的细化，不改变 v1 范围：

| 产品规格阶段 | 本计划阶段 |
|---|---|
| 架构契约 | 0：先冻结 schema、状态机、身份与恢复契约 |
| 只读基础 | 0–2：把工程底座、只读内核和浏览 UI 分开验收 |
| 浏览闭环 | 2–3：在只读 UI 之后单独验收 D/R 与复核 |
| 外部工具 | 4 |
| 安全导入 | 5–6：把零写入预检与真实复制分开验收 |
| 集中删除 | 7 |
| 硬化验收 | 分散到 0、5、6、7 的准入门，跨功能与规模回归留在 8 |

## 2. 可交付版本

| 版本 | 包含阶段 | 用户能够完成的事情 | 文件系统风险 |
|---|---|---|---|
| 只读 Alpha | 0–2 | 绑定图库、扫描、浏览、排序、恢复位置 | 不写图库 |
| 粗筛 Beta | 0–4 | 浏览、D/R 标记、待删除复核、打开 Photoshop/位置 | 本应用自身只写 SQLite；外部 Photoshop 的保存行为不受本应用控制 |
| 导入预检 Beta | 0–5 | 查看内存卡中哪些是新增、重复、冲突 | 内存卡和图库均不写 |
| 导入 Beta | 0–6 | 确认后安全复制新照片到图库 | 只新增；不覆盖、不删源 |
| 完整 v1 | 0–8 | 再增加整批移入 Windows 回收站及完整硬化 | 无永久删除路径 |

对当前需求，粗筛 Beta 已经是第一个有实际价值的里程碑。它能管理待删除标记，但“执行删除”保持关闭，因此不会因为早期版本缺陷移动唯一照片。

## 3. 全阶段共同规则

### 3.1 真实数据门禁

- 破坏性和故障注入测试只能使用应用创建的可丢弃测试 Root。
- 测试模式必须硬拒绝真实 FujiLibrary 与真实 ImportSource。
- 在只读链路完成写请求审计前，不读取真实内存卡做预检。
- 在导入 go/no-go 全部通过前，不允许向真实 FujiLibrary 执行 ImportJob。
- 在回收站 go/no-go 全部通过前，不允许对真实 FujiLibrary 创建可执行 DeletionBatch。
- 用户当前只有一份的数据不能充当破坏性测试样本；需要 RAF/JPG 样本时，只使用另外复制到测试目录的副本。

### 3.2 功能能力边界

- React 不获得通用文件系统或 shell 权限。
- Tauri 命令只接受 RootId、PhotoId、AssetId、JobId、ToolId、对象版本和 OperationId。
- 展示路径不能作为写命令授权。
- 所有照片相关写动作只能经过 Rust 的 Root/卷/对象身份校验层。
- Import 与 Recycle 各有独立的编译或运行时门；默认关闭，测试通过后显式打开。
- 两个功能门之上还有全局写入门；共享安全层异常、数据库只读安全模式或构建证明失效时，两项写功能同时关闭。
- 写功能准入绑定到准确构建产物：至少记录二进制 SHA-256、依赖锁文件哈希、schema 版本和安全报告版本。
- 二进制、锁文件、schema、Root/DB/jobs/reconcile/Win32 安全封装或相关依赖发生变化时，旧准入自动失效，必须重新构建并重跑对应安全报告。
- 同一时间只运行一个会改变照片文件系统的任务。
- 卷一旦被识别为 ImportSource，其整卷 deny-write 在 v1 中永久且单调生效；该卷不得承载 AppData、当前或旧的 Draining CacheRoot、FujiLibrary 或任何应用必须写入的位置。
- ImportJob、DeletionBatch 或启动协调存在非终态工作时，Rust 必须拒绝切换 FujiLibrary、ImportSource 或 CacheRoot；任务始终使用启动时冻结的 Root 身份。
- CatalogService 是 Asset 观察与 upsert 的唯一入口；扫描和 Import 共用幂等 `observe_asset`，活动 Import/Recycle 对路径和文件身份建立 OperationReservation。
- 已发布、已回收等文件系统事实不得回退或覆盖；重试一律创建引用原步骤的新 Attempt。
- 本规格中的 `GB` 统一按 `1 GB = 1,073,741,824 bytes` 计算；数据库只保存整数 bytes，UI 再换算显示。

### 3.3 每阶段完成定义

一个阶段只有同时满足以下条件才算完成：

1. 本阶段代码和数据库迁移已完成。
2. 单元、集成、UI 或 Windows 专项测试按阶段要求通过。
3. 安全不变量没有被临时绕过。
4. 新增错误状态有用户可理解的反馈，不伪造成功。
5. 文档、测试报告和已知限制已更新。
6. 生成一个可回退的本地 Git 提交或阶段标签。
7. 用户验收本阶段后才进入下一阶段。

## 4. 建议代码边界

前端保持按功能切片：

- src/app：应用外壳、路由、全局快捷键作用域
- src/features/library：图库、时间线、Filmstrip、照片画布
- src/features/pending-delete：待删除复核和批次摘要
- src/features/import：预检、执行进度、恢复和报告
- src/features/settings：Root、缓存和 Photoshop 设置
- src/features/reports：扫描、导入、删除操作记录与只读报告
- src/shared：视觉 token、基础控件、类型化命令客户端

Rust 核心保持按安全职责切片：

- src-tauri/src/commands：窄 Tauri 命令边界
- src-tauri/src/domain：ID、对象版本、状态机和领域错误
- src-tauri/src/db：迁移、单写者、备份和只读安全模式
- src-tauri/src/roots：卷、Root、能力、重绑定和 containment
- src-tauri/src/catalog：扫描、Asset/Photo、配对和时间线
- src-tauri/src/media：EXIF、RAF 内嵌预览、缩略图和缓存
- src-tauri/src/import：预检、去重、计划、复制、发布和恢复
- src-tauri/src/recycle：DeletionBatch、COM worker、结果和协调
- src-tauri/src/external_tools：Photoshop 和资源管理器
- src-tauri/src/jobs：互斥、进度、取消和故障注入点
- src-tauri/src/reconcile：启动协调和 NeedsAttention
- src-tauri/src/reports：不可变操作报告查询、Attempt 关联和问题生命周期

Windows 句柄级文件操作集中在少量基础设施模块中，业务服务不能直接散落调用 std::fs 写 API 或 Shell 删除 API。数据库实体、唯一约束、版本传播、墓碑和状态转换以 Schema ADR 为实现契约；若代码需要改变其语义，必须先修订 ADR 与迁移测试。

## 5. 阶段 0：工程与安全底座

### 目标

建立可运行、可测试、默认没有照片写能力的桌面应用骨架。该阶段只形成工程基础，不扫描真实照片。

### 工作包

1. 安装并验证 Windows Tauri 开发前置条件：
   - Rust stable MSVC 工具链
   - Microsoft C++ Build Tools 的桌面 C++ 工作负载
   - Edge WebView2 Runtime
   - 把当前已结束维护的 Node.js 20 升级到受支持的 Node.js 24 LTS
   - 当前环境检查已确认 Node.js 20.19.0、npm 10.8.2 可用，但 rustc 与 cargo 尚未出现在当前终端 PATH；MSVC 和 WebView2 状态在本阶段复核
   - 参考 Tauri 官方的 [Windows 前置条件](https://v2.tauri.app/start/prerequisites/) 与 [建项流程](https://v2.tauri.app/start/create-project/)，Node 版本状态以 [Node.js 官方发布周期](https://nodejs.org/en/about/previous-releases) 为准
2. 创建 Tauri 2、React、TypeScript 工程和锁文件。
3. 建立格式化、lint、Rust clippy、单元测试、前端测试和 Windows 集成测试命令。
4. 实现单实例应用；第二次启动只激活原窗口。
5. 建立强类型 ID、对象版本和领域错误，不让前端传任意写路径。
6. 建立 Root capability 模型与全局 ImportSource deny-write 集合。
   - 使用强卷身份规范键去重；ImportSource deny-write 只允许新增，不提供 retire/reset。
   - AppData、FujiLibrary、Active/Draining CacheRoot 或其他应用写位置所在卷不能注册为 ImportSource。
7. 建立 SQLite 单写者：
   - 每连接 foreign_keys=ON
   - journal_mode=WAL
   - synchronous=FULL
   - 设置有界 busy_timeout、受控 WAL checkpoint，并禁止扫描持有无界长读事务
   - 所有事务检查最终 COMMIT
8. 建立版本化迁移、每日一致备份、升级前备份和损坏时只读安全模式。
   - 保留最近 7 个日常版本和 3 个升级前版本。
   - 导入历史、删除报告和关键操作事件长期保留。
   - AppData 空间不足或数据库耐久写失败时关闭 Import/Recycle，保留可读报告并进入可解释的安全状态。
9. 按 Schema ADR 落地外键、唯一约束、大小写折叠路径、墓碑、`asset_version/photo_version`、标记时 Asset 快照、ViewingCursor 排序锚点和操作报告模型。
10. 定义 TaskIntent、Import/Deletion Job、不可变 TaskAttempt 身份和 Import/Deletion Attempt 子状态的完整转换表；包括崩溃后的 Interrupted/Reconcile、AwaitingSource/AwaitingUser、报告最终化、FileWriteLease/OperationReservation 原子释放和定向重扫顺序，非法转换、终态回退与重复 OperationId 一律拒绝。
11. 建立 Catalog 单一观察入口、活动路径/文件身份 OperationReservation，以及任务结束后的定向重扫协议。
12. 建立可确定触发的故障注入框架，覆盖 SQLite commit、Win32 文件调用和 Shell 回调前后。
13. 建立测试 Root 标识；破坏性测试若收到非测试 Root 必须失败。
14. 实现全局写入门、Import 门和 Recycle 门，并使准入记录绑定准确构建产物。
15. 添加安全 API 扫描，阻止生产路径误用覆盖式 rename、永久删除或通用 shell 字符串。

### 验收出口

- npm 和 cargo 的基础检查全部通过。
- 应用能够启动、退出和重复激活。
- 数据库 PRAGMA、备份和只读安全模式由自动测试证明。
- 前端不能调用通用 fs/shell 写接口。
- Schema ADR 的外键、唯一约束、版本传播、墓碑、不可变 Attempt、非法转换、重复 OperationId 和 COMMIT 失败测试通过。
- Import 执行与 Recycle 执行默认关闭。

### 本阶段不做

照片扫描、业务 UI、Photoshop、导入、回收站调用和任何真实图库写入。

## 6. 阶段 1：只读图库内核

### 目标

在不改变历史照片的前提下，建立 Root、Asset、Photo、配对、时间线和预览数据。

### 工作包

1. 实现 FujiLibrary 首次绑定与可靠重绑定：NTFS 使用卷 GUID/序列号和根目录文件 ID，盘符不作为身份；可靠重连复用原 Library/Root，身份不同则原子创建新 Library/Root 并把旧 Library 保留为 Inactive；每次能力使用前通过句柄复核 containment 与 reparse 状态。
2. 只读递归扫描 JPG/JPEG/RAF；跳过并报告 reparse point 和不支持格式。
3. 使用卷身份、文件 ID、相对路径、大小和 mtime 协调稳定 AssetId；任何安全快照变化递增 `asset_version` 并传播到 `photo_version`。
4. 识别 hard link 并标记“不支持删除”。
5. 从安全层已打开的只读句柄或字节流提取 EXIF。
6. 做一个 RAF 技术验证：只提取内嵌 JPEG，不做完整 RAW 解码；失败时返回明确占位状态。
7. 实现保守 JPG/RAF 配对和确定性 Photo 生命周期：历史扫描只有两端可靠拍摄时间一致时才配对；导入组证据由阶段 5 接入；合并、拆分、成员缺失均按 Schema ADR 产生版本、墓碑和仅用于导航的别名。
8. 实现拍摄时间排序、日期未知固定末尾、自然文件名排序和稳定 PhotoId tie-break。
9. 实现以 `AssetId + asset_version` 为键、按视口优先的缩略图/预览缓存、32 GB 默认上限、64 GB 绝对上限和缓存 Root 校验。
   - 用户选择的是缓存父目录；应用只在带稳定身份的专属子目录中以 CREATE_NEW 创建并登记文件 ID。
   - 首次创建缓存专属子目录前，必须先提交非终态 `CacheRootCreateIntent`，绑定已验证的父目录身份、预定专属目录名和 OperationId；只有 COMMIT 成功后才能执行 mkdir，目录身份登记和 Active CacheRoot 切换成功后才终结该 Intent。崩溃恢复继续协调同一非终态 Intent，不得另建 Intent 绕过；进入 `NeedsAttention` 后禁止自动重试，必须等待用户处理。
   - 自选 CacheRoot 与 FujiLibrary、ImportSource、数据库目录必须双向不重叠，也不得经过 reparse point。
   - LRU 只永久删除数据库登记、身份匹配且仍位于专属目录中的缓存；未知文件、身份变化和用户父目录中的其他内容一律保留，禁止递归清空父目录。
   - 缓存淘汰在同一事务提交轻量 CacheEvictionIntent 与由它拥有的 OperationReservation；cache owner 不取得照片 FileWriteLease，但与 Task owner 共用活动 path/file ID 唯一约束。随后通过禁止共享写入/删除的已验证句柄核对并删除同一缓存对象；崩溃恢复和临时离线只协调同一非终态 Intent/Reservation，不得为同一 Entry 另开 ordinal，明确失败、身份异常或结果不明进入 `NeedsAttention` 后也不得自动重试。生产缓存路径禁止使用“先验路径、后 remove_file(path)”模式。
   - CacheEntry 表示一次物理 incarnation；当前可用/占用 incarnation 的渲染唯一键包含 CacheRoot，使 Active 新 Root 与 Draining 旧 Root 可以同时保留同一 Asset 的缓存所有权。Evicted/Missing 后按需重建必须以 CREATE_NEW 创建新 Entry、递增 ordinal 和新的全历史唯一路径；旧 Entry/Intent/Fact 不改写，NeedsAttention incarnation 继续阻止同 key 静默再生。
   - 缓存卷始终保留 max(20 GB, 卷容量的 10%)。
10. 实现启动先读 SQLite、后台增量扫描和手动重扫；启动扫描不做全图库内容哈希。扫描忽略应用 partial，并延迟协调 OperationReservation 覆盖的路径，任务提交后再定向重扫。

### 验收出口

- 对同一测试图库重复扫描得到稳定索引和顺序。
- 配对、文件名回卷、EXIF 缺失、Photo 合并/拆分、版本传播、未知日期、外部缺失、Root 离线和重绑定测试通过。
- reparse point 不被跟随；hard link 不获得删除能力。
- 扫描与预览前后，测试原片的内容哈希、名称、属性和目录树完全一致。
- 缓存淘汰故障注入证明不会清理用户父目录、未知文件或身份不匹配的旧 CacheRoot。
- CacheRootCreateIntent 与 EvictionIntent 的同一非终态 Intent 恢复、`NeedsAttention` 禁止自动重试、同一已验证句柄删除，以及 Active/Draining Root 同时持有同一 Asset 缓存的测试通过；Evicted/Missing 后同 render key 能以新 Entry/唯一路径安全重建且旧审计事实不变。
- 通过进程级观察确认扫描、EXIF、RAF 预览和缓存不会向图库发起写请求。

### 本阶段不做

完整照片 UI、D/R、外部工具、导入写入和回收站删除。

## 7. 阶段 2：白色沉浸式只读浏览 UI

### 目标

交付第一个可供用户真实浏览图库的只读 Alpha。

### 已满足的设计门

首次 Root 绑定、原 Root 重连、新图库切换和扫描报告已在操作 UI 补充规格中冻结；本阶段按该规格实现，不在编码时重新解释。

### 工作包

1. 落地已确认的白色/浅灰视觉 token。
2. 实现顶栏、纵向虚拟 Filmstrip、中央适应窗口画布、右侧详情和底部状态栏。
3. 右栏采用“顶部参数 + 弹性留白 + 沉底动作区”结构。
4. 通过受控 AssetId + version 预览协议取图，不把真实路径交给通用资源协议。
5. 实现新到旧/旧到新切换、日期上下文和日期未知末尾。
6. 实现 Filmstrip 点击、左右键和照片/Filmstrip 上的滚轮浏览；首尾不循环。
7. 实现按图库与排序保存 ViewingCursor，同时保存稳定排序锚点；目标离线、墓碑或缺失时选择最近下一张，导航别名不得被破坏性命令使用。
8. 建立图库视图、共享顶栏的条件导航槽位和待删除路由保护；本阶段待处理数量恒为 0，因此不渲染待删除入口，直接访问待删除路由时回到图库，不显示空待删除页。
9. 在共享顶栏提供非页签的“设置”工具按钮；实现设置外壳中的“照片文件夹”和“操作记录”，不把它们变成照片视图。
10. 实现首次绑定、新图库全量扫描、原 Root 重连增量扫描的进度、取消、失败、未完成和完成状态；首次扫描完成前保持在扫描工作页，不渲染虚假空图库，完成后进入图库并保存扫描报告。
11. 完成 100%、125%、150% DPI 和规定窗口尺寸的视觉快照测试。

### 验收出口

- 用户可以绑定并浏览真实图库，但应用仍没有照片写能力。
- 中央画布始终是最大区域；右栏底部区不被元数据推出视口。
- 长文件名、缺失 EXIF、JPG-only、RAF-only 和配对 Photo 状态清楚。
- 已有索引启动、缓存切图和内存占用达到阶段性性能基线。
- 待处理数量为 0 时共享顶栏没有待删除入口或空白占位，陈旧待删除路由稳定回到图库且不闪现空页面。
- 首次绑定/新图库全量扫描和原 Root 增量扫描的取消、失败、完成及重开恢复状态可理解；设置入口可达且不增加照片页签。
- 该版本的文件树前后完全不变。

### 本阶段不做

待删除标记、Photoshop、文件位置、导入执行和删除执行。

## 8. 阶段 3：待删除标记与复核闭环

### 目标

交付可用于日常粗筛的 Beta，但仍不移动任何照片。

### 工作包

1. 实现 ToggleDeletionMark 的短 SQLite 事务和 OperationId 幂等；标记同时持久化全部 `DeletionMarkAssetSnapshot`。
2. D 只在事务成功后更新徽标、计数并前进；失败保持当前照片。
3. 实现当前会话内多步 R 撤销；按 OperationId 撤销并回到受影响 Photo。
4. 串行化标记写入；pending 时阻止重复标记和导航，不排队。
5. 实现 Root 离线时只能移除标记，不能新增或通过 R 恢复标记。
6. 扫描、进入待删除页和创建批次时比较标记快照；Photo 成员、Root、文件 ID、相对路径、大小、mtime 或对象版本任一变化都使旧标记失效，刷新预览后必须重新按 D。
7. 实现待删除 Filmstrip、当前项取消和问题项，并提供共享的 `PendingDeleteSummary` 查询；其待处理 Photo 数量同时覆盖有效标记与仍需处理的问题项，作为顶栏入口、数量和路由保护的唯一数据源。
8. 实现 `PendingDeleteSummary` 的导航状态转换：0→1 只显示“待删除 1”，仍留在图库并按成功标记后的既有规则前进；事务失败不改变入口、选择或路由。
9. 实现清单归零行为：在待删除视图取消最后一项造成 1→0 时，立即回到图库、定位刚取消标记的 Photo，再隐藏入口；在其他视图归零时只隐藏入口。任何路径都不得短暂显示空待删除页。
10. 启动与会话恢复始终默认进入图库；若持久标记或问题项使 `PendingDeleteSummary.total_count > 0`，则立即恢复“待删除 N”入口但不自动打开复核页，且 R 历史仍为空。
11. 待删除页不包含 Photoshop、文件位置、复选框或部分执行。
12. 从活动 `DeletionMark` 及其 `DeletionMarkAssetSnapshot` 的当前有效性派生问题项，并建立 Open/Resolved/Superseded 生命周期；本阶段完全不创建 Deletion Attempt。真正的 Attempt 只在阶段 7 创建并执行或重试 `DeletionBatch` 时产生，终态历史不可改写；最终删除入口由 Recycle 功能门控制，在阶段 7 前明确保持不可执行。
13. 为快照失效、成员变化、缺失、离线和 hard link 分别提供“取消失效标记”及允许时的“重新扫描确认”；取消后回图库重新按 D，离线或 hard link 不提供重新标记或执行入口。

### 验收出口

- D/R 的成功、失败、重复输入、auto-repeat、失焦、弹窗和离线矩阵通过。
- 配对或外部文件变化不会让新内容静默继承旧删除决定；旧缓存也不会掩盖需要重新确认的状态。
- 首次通过 D 或右栏按钮成功标记会产生 0→1：显示“待删除 1”，继续前进到下一张但不自动切换到待删除页；提交失败时入口和当前 Photo 均不变化。
- 重启后标记仍在、R 历史为空；启动仍进入图库，同时按 `PendingDeleteSummary` 恢复“待删除 N”入口。
- 在待删除视图取消最后一项会产生 1→0：返回图库并定位刚取消的 Photo，入口随即隐藏，过程中不出现空页面；零项陈旧路由也回到图库。
- 用户可以完成“浏览 → 标记 → 待删除复核 → 取消标记”闭环。
- 原片目录树和文件内容零变化。

### 本阶段不做

任何 IFileOperation、永久删除、应用回收站或物理文件移动。

## 9. 阶段 4：Photoshop 与文件位置

### 目标

完成图库页的两个外部工具按钮，不扩散通用 shell 权限。

### 工作包

1. 在阶段 2 的设置外壳上补齐“预览缓存”和“Photoshop”分组；首次绑定、照片文件夹和扫描 UI 不在本阶段重复实现。
2. 首次点击时由 Rust 原生选择器选择并验证 Photoshop.exe，保存 ToolId。
   - 首次交给 Photoshop 前明确说明：文件交给外部程序后，其修改、保存或 sidecar 行为不受本应用控制。
3. 设置页提供“重新选择 Photoshop”；ToolId 失效时禁用打开动作并显示重新选择，不向界面暴露可执行文件内部 ID。
4. JPG+RAF 和 RAF-only 明确打开 RAF；JPG-only 禁用 Photoshop。
5. JPG+RAF 的文件位置定位 JPG；JPG-only 定位 JPG；RAF-only 定位 RAF。
6. 直接调用已注册可执行文件并传独立参数，不经过命令行字符串拼接。
7. 待删除页 DOM 中不渲染 Photoshop、文件位置按钮或占位。
8. 设置页展示缓存上限、实际占用与磁盘可用空间，不提供手动清理；位置或容量变更先显示影响摘要，确认后原子生效，取消不改变设置。
9. CacheRoot 切换复用阶段 1 已实现的 `CacheRootCreateIntent` 协议：确认变更后、mkdir 新专属子目录前先提交 Intent，成功登记新目录身份后才切换 Active Root；不得绕过该协议直接建目录。切换后保留旧缓存所有权记录，后台仅在旧 Root 身份和文件归属可证明时渐进淘汰；旧盘离线或身份不符时保持原样。
10. Import、Recycle 或启动协调处于非终态时，前端禁用 Root/来源/缓存切换，Rust 同时硬拒绝绕过请求；共享状态条提供“返回进行中任务”。

### 验收出口

- 三种 JPG/RAF 组合逐项通过。
- 伪造 ToolId、跨 Library PhotoId、过期版本和任意路径请求被拒绝。
- 本应用进程在启动外部工具前不修改照片；验收只使用可丢弃的 JPG/RAF 副本，不用唯一原片验证外部程序行为。
- 缓存位置和容量的确认、取消、渐进淘汰、旧盘离线与所有权异常矩阵通过；不存在手动递归清理入口。

### 本阶段不做

监控 Photoshop 保存结果、在待删除页开放工具、导入或回收站删除。

## 10. 阶段 5：只读导入预检

### 目标

让用户在复制前知道哪些文件会新增、跳过、改名或失败，同时内存卡和图库都保持只读。

### 已满足的设计门

选择位置、预检确认、进度、恢复和导入报告已经在导入位置与操作 UI 补充规格中冻结；本阶段按书面状态和动作矩阵实现。

### 工作包

1. 通过 Rust 原生选择器分别选择内存卡子目录和当前 FujiLibrary；来源保存卷身份、文件系统、相对文件夹及可用的目录文件 ID，目标变更按 Root 重连/新图库规则处理。
2. 同一张卡再次插入时只自动识别并预填上次来源；必须由用户点击“开始只读预检”才扫描，绝不因插卡或打开页面自动预检或复制。
3. 递归只扫描所选子树；整个源卷加入 deny-write。
4. 整批处理所选子树中的全部受支持文件；v1 不提供逐文件勾选。
5. 只读读取元数据、源身份、哈希和必要字节，不让第三方库按路径自行打开；原生选择器、卷探测、自动识别、取消及全部第三方组件纳入整进程源卷零写入审计。
6. 实现 ImportHistory 候选缩小，但不让历史记录单独触发跳过。
7. 只有当前可访问目标经完整哈希且逐字节相同才判定已导入；源与目标均使用禁止共享写入/删除的稳定只读句柄，在比较前后复核 RootBinding、规范路径、文件 ID、大小和 mtime，并从计划路径重新打开证明仍解析到同一对象。
8. 候选查找包含应用命名规则形成的原名、`_2`、`_3` 等完整后缀族；数据库历史只加速定位，不改变内容证明结果。
9. 实现目标日期路径、Unknown-Date、一致后缀、名称安全和 JPG/RAF 组级矩阵：两端时间冲突时拆组；单端时间补足的正向证据先作为本次计划草案的一部分展示，首次最终确认后必须固化到同一 ImportJob；已存在一端可与新增另一端安全配对。
10. 为预检结果建立带摘要 hash 的只读计划草案；首次预检尚未创建 ImportJob，也不把草案称为 plan generation。首次最终确认由阶段 6 原子创建 ImportJob 和首个 Confirmed generation；已有 Job 后，只有分类（含 skip/new/conflict）、目标名称、目标路径或预计新增字节变化才追加 Proposed generation 并重新展示确认，身份与证明刷新但这些计划字段不变时只追加新的 Validation。
11. 计算新增容量并展示执行空间门需要的总量；本阶段不执行空间检查或写入。
12. 展示新增、重复、冲突、日期未知和无法读取的整批预检报告；可读但 EXIF 缺失或不可解析的文件归入日期未知，不归入读取失败。
13. 暂不创建日期目录、partial 或正式文件。

### 验收出口

- stale ImportHistory、后缀历史缺失、目标离线、目标缺失、目标比较中变化、内容变化和旧数据库恢复不会错误跳过源文件。
- 模拟哈希碰撞时，逐字节比较能拒绝误判。
- 更换同盘符的另一张卡不会继承原卡身份。
- 对原生选择器、卷识别、完整预检、EXIF、RAF 预览、哈希、取消和异常流程做进程级审计，源卷写请求为零。
- 预检前后内存卡和图库目录树、内容、名称与可观察属性不变。

### 本阶段不做

创建目录、复制、partial、发布、历史图库重复扫描或删除内存卡文件。

## 11. 阶段 6：安全导入执行与恢复

### 目标

只复制真正新增的照片，任何时候都不覆盖目标、不删除源文件，并能解释应用崩溃后的中间状态。

### 工作包

1. 用户首次点击最终“开始导入”后，先以事务持久化 TaskIntent、ImportJob 和首个 Confirmed plan generation，但不取得文件写能力、不创建 Attempt、目录或文件；随后保持只读并刷新全部 skip 证明、目标路径、名称冲突、组后缀、文件数和新增字节。只有未解决 Step 的 Skip/Copy 分类、组后缀或目标名称、最终目标路径、单文件计划字节相对 Confirmed generation 发生变化时，才追加 Proposed generation 并返回确认页展示差异；仅证明或身份刷新且这些计划字段不变时，在同一 Confirmed generation 下追加 Validation。
2. 只有用户确认当前 generation 且最终刷新仍完全一致时，才冻结本次执行的复制集、跳过集、目标名称、来源/目标 RootBinding 和 `planned_new_bytes`。
3. 对该冻结 generation 即将创建的本次 Attempt 只执行一次整批空间门：可用空间不少于 `planned_new_bytes + max(10 GB, 卷容量的 2%)`。通过后在一个 SQLite 事务中创建初始 TaskAttempt/ImportAttempt、全部 ImportAttemptStep、Open OperationReport、全局 FileWriteLease 和全部 OperationReservation；每个 CopyNewBytes Step 同时创建 `Planned` ImportPartialOwnership incarnation，分配递增 ordinal、nonce 和全历史唯一 partial 路径。COMMIT 前把 Attempt/Job/Task 变为 Running/Running/Active；ImportJob、计划 Step 和 Confirmed generation 已由第 1 步持久化，不得重复创建或改写。运行中不做逐文件空间轮询，底层空间不足时安全停止并报告。
4. 在 OperationReservation 已持有时重新验证 Root、目录、containment 和 reparse point。Attempt 内若普通外部变化会改变未解决 Step 的 Skip/Copy 分类、组后缀或已确认名称、最终目标路径、单文件计划字节，不静默扩容、改名或改写分类；当前 Attempt 停止受影响项，追加 Proposed generation 并返回确认页，只有新 generation 经用户确认后才能再次执行。
5. 对 Skipped 项保留双端 DuplicateProof；写入 SkipDuplicate 事实前，必须对来源与当前目标重新取得禁止共享写入/删除的稳定句柄，执行完整哈希和逐字节比较，并在比较前后复核 RootBinding、规范 path key、file ID、大小和 mtime，再从计划的源/目标路径分别重新打开并证明仍解析到各自同一对象。比较句柄和两端路径重开句柄保持到 Execution DuplicateProof、双端 PathBindingProof 与 SkippedDuplicate 的原子事务最终 COMMIT；提交失败时不记跳过并停止 Attempt。只核对身份或复用旧证明都不得执行跳过；若新证明使第 4 步列出的任一计划字段变化，按第 4 步返回新 generation 确认，尤其禁止在当前 Attempt 把旧 SkipDuplicate 改成新增复制；若无法建立完整证明但计划变化尚不能确定，则停止并进入 NeedsAttention，不猜测结果。
6. 在本 Attempt 的 Planned Ownership 路径以 CREATE_NEW 创建唯一 partial；读取实际 Root/Binding/path/file ID 后，以同一事务提交 `ImportPartialOwnership=Owned` 和当前 AttemptStep 的 `Created` ImportPartialClaim，Claim 绑定当前 SourceRead/PartialPath/FinalPath OperationReservation 与 FileWriteLease generation。创建时大小只作观察值，最终验证使用冻结源/计划 expected bytes。文件创建与该 COMMIT 之间崩溃时，该 Planned incarnation 进入 NeedsAttention，不得仅凭名称认领、删除或另建 incarnation 绕过。
7. 复制完成后 FlushFileBuffers、关闭写句柄，再以读取、属性与 DELETE 权限且禁止共享写入/删除的同一句柄重读完整 partial。
8. 对 partial 与源执行长度、完整哈希和逐字节比较并重新验证源；设置源 LastWriteTime 后创建属于当前 Claim/AttemptStep 的不可变 VerifiedPartialSnapshot，包含 Binding、path key、file ID、大小、mtime 和完整哈希。该句柄持续保持到 no-replace 发布，任一变化都进入 NeedsAttention。
9. 在 no-replace rename 前先提交非终态 `ImportPublishIntent`，不可变绑定当前 Claim、该 `VerifiedPartialSnapshot`、当前 FinalPath 的 `OperationReservation` 和当前 `FileWriteLease` generation；只有该事务 COMMIT 成功且全链仍一致时，才使用上述已验证句柄执行 SetFileInformationByHandle 且 ReplaceIfExists=FALSE 的同目录发布。恢复只协调同一 Intent，不能根据路径猜测或另开发布 ordinal。
10. 发布后通过 CatalogService 的幂等 `observe_asset` 独立提交 Asset、ImportHistory 和报告；索引失败不删除已发布文件。
11. 实现取消、拔卡、换卡、源变化、空间不足、名称抢占、部分完成和显式重试。只有未解决 Step 的 Skip/Copy 分类、组后缀或目标名称、最终目标路径、单文件计划字节相对 Confirmed generation 变化时，才追加 Proposed generation 并要求用户重新确认；这些字段都未变化时，复用原 Confirmed generation，追加新的 Validation、一次 SpaceCheck 和新 Attempt。已发布/已索引事实或已验证 partial 使本轮剩余复制字节减少，只反映在新 Validation/SpaceCheck 中；安全清理旧不完整 Ownership 后分配新的 partial ordinal/nonce/path 也只是执行 incarnation，都不产生新 generation。每个新 Attempt 仍只做一次整批空间门，旧 Validation、SpaceCheck、Attempt 和结果均不可改写。
12. Published 但未 Indexed 的步骤只补观察与索引，绝不重新复制、删除或回退正式文件。
13. 重启发现未完成 ImportJob 时先显示摘要，由用户选择“继续”或“取消”，禁止静默续传；原卡缺失或错误卡进入 AwaitingSource，崩溃时的 Running Attempt 先合法终结为 Interrupted/NeedsAttention 并释放旧 Claim。继续使用已证明 Owned 的 partial 时，新 Reconcile AttemptStep 只能为同一 Job/Generation/逻辑 Step 建立唯一活动 Takeover Claim，绑定当前 Lease/OperationReservation，随后从头完整验证并生成新 Snapshot/PublishIntent；旧 Claim、Snapshot、Intent 或 lease generation 不能授权发布。若证明 partial 不完整且源仍安全可用，先提交 CleaningOwnedPartial，再经同一已验证句柄清理；结果事务追加如实区分 DeleteSucceeded/ObservedMissingAfterCleaningIntent 的 ImportPartialCleanupFact，并原子关闭 Ownership/Claim/Reservation。只有 Cleaned 后，后续新 Copy Attempt 才能创建新 Ownership ordinal/nonce/path。
14. 终结事务先持久化逐项事实、最终化 Attempt 报告并推导 Job/Task 状态，再释放 FileWriteLease/OperationReservation；COMMIT 成功后才定向重扫。提交失败时保留占用并停止，不能处理下一文件。
15. 已发布并索引的照片渐进加入时间线，但不改变当前 PhotoId。活动 Import 通过共享状态条可返回查看，并禁用删除执行；离开进度页不隐式取消或改变任务。

### Import go/no-go

真实 FujiLibrary 开放导入前，以下项目必须全部通过：

- 并发程序在发布瞬间抢占同名目标，原目标仍一个字节不变。
- 静态和运行时检查均证明发布路径不会调用覆盖式 rename。
- 在每个文件系统动作与 SQLite commit 前后注入崩溃，结果都收敛为定义状态。
- 未落库 file ID、身份不符或未知来源的 partial 不会被采用或清理。
- 拔卡、换卡和源文件读取中变化进入 AwaitingSource 或 NeedsAttention。
- 预检后目标缺失、替换或变化不会沿用旧跳过结论；计划变化先回确认页，唯一空间门只发生在最终复制集冻结之后。
- 双端 DuplicateProof 证明计划路径当前仍解析到同一对象；后缀族比较结果可重复。
- Planned Ownership → CREATE_NEW → 创建时 file ID/Created Claim → Flush/关闭 → 同一禁止共享写删句柄重读 → 最终大小等于冻结源/expected bytes → 哈希、逐字节比较、VerifiedPartialSnapshot 和源身份复核全链路通过；创建时通常为 0 的大小不得错误阻止正常复制。
- 取消或数据库失败不会删除或改名已 Published 文件。
- Import Attempt 不改写旧终态；Published 未 Indexed 只补索引，运行中空间不足只停止后续步骤并保留已发布事实。
- Attempt/报告/Job/Task 的终结与 FileWriteLease/OperationReservation/ImportPartialClaim 释放由单个事务完成；提交失败不释放占用、不继续下一文件。不完整 Owned partial 的 Cleaning 意图/句柄清理/结果提交故障注入通过，Cleaned 后可在同一 Confirmed generation 由新 Attempt 创建新 Ownership incarnation，NeedsAttention 时不可绕过。
- 所有既有正式文件在测试前后内容、名称和属性完全不变。
- 完整流程对源卡的写请求审计为零。
- ImportSource deny-write 在停用来源 Profile 后仍永久生效；来源卷与 AppData、Active/Draining CacheRoot 或任何应用写卷冲突时在选择阶段即被拒绝。
- 非本机 NTFS、源目标同卷、Root/reparse/mount point 变化、危险文件名或空间不足都被运行时拒绝。
- 正式文件与 partial 的全部恢复组合以及旧数据库备份恢复均按规格收敛。
- 产品规格第 18.3、18.4、18.7 和 18.8 节相关测试全部进入机器可读 Import 安全报告。
- Import 准入记录与实际测试的二进制、锁文件和 schema 完全匹配。

任何一项未通过，Import 执行门保持关闭；只读导入预检仍可使用。

### 验收出口

- 使用可丢弃 NTFS 测试 Root 完成新增、重复、冲突、日期未知、取消和恢复矩阵。
- 对复制后的每个成功文件重新验证与源逐字节一致。
- 用户报告明确区分已发布、已跳过、未开始、部分完成、文件时间警告、失败、冲突、AwaitingSource 和 NeedsAttention，并可从设置的操作记录重新打开。
- 顺序固定为“构建 Import Beta 候选包 → 安装该包 → 对已安装二进制运行 Import 安全测试 → 固定哈希”；任何重新构建都会使结果失效。
- 通过独立 Import 安全报告后，为准确受测构建生成 Import 准入；用户仍需对每次复制显式确认。

### 本阶段不做

Windows 回收站、永久删除、源卡清理或历史图库重复扫描。

## 12. 阶段 7：Windows 回收站删除

### 目标

在待删除清单最终复核后，把全部剩余有效 Photo 强制移入 Windows 回收站；任何结果不明都立即停止。

### 已满足的设计门

待删除核心视图、最终确认、异常处理和删除结果报告均已冻结；本阶段按核心与操作 UI 规格实现，不改变复核页按钮和布局。

### 工作包

1. 完成“执行删除…”按钮、整批摘要和最终确认对话框。
2. 确认后在一个 SQLite 事务中从仍有效的 `DeletionMarkAssetSnapshot` 创建不可变 DeletionBatch、初始 Attempt、全部逐 Asset 意图、Open OperationReport、全局 FileWriteLease 和 OperationReservation，并在 COMMIT 前把 Attempt/Batch/Task 变为 Running/Running/Active；只有最终 COMMIT 成功后才清空会话撤销栈。BatchAsset 只能引用或逐字段复制原标记快照，现场读取只用于验证相等，绝不能刷新删除授权。
   - 确认框打开时禁用 D/R、左右键和滚轮切图。
   - DeletionBatch 执行期间暂停全部 D/R，但允许只读浏览；批次结束后才重新开放标记。
3. 上述事务 COMMIT 且 OperationReservation 已持有后，执行前重新验证 Root、NTFS、Photo/Asset 版本、标记时成员快照、文件 ID、相对路径、大小、mtime、containment 和 hard link。
4. 使用专用 COM STA worker 承载 IFileOperation 与 progress sink。
5. 每个 Photo 一次 IFileOperation，配对成员在同一次操作中排队。
6. 明确使用 FOFX_RECYCLEONDELETE | FOF_NOERRORUI | FOFX_EARLYFAILURE | FOF_NOCONFIRMATION；禁止 FOF_WANTNUKEWARNING，不存在永久删除或降级路径。
7. 在 PreDeleteItem 或最接近 Shell 动作的位置再次核对冻结身份；逐 Asset 记录 PostDeleteItem、hrDelete、回收对象和 GetAnyOperationsAborted。
8. PostDeleteItem 必须同步等待该 Asset 的逐文件结果和 RecycledFact COMMIT；只有成功回调、非空回收对象、源路径消失、回收结果身份可解释且数据库提交成功后才向 Shell 允许继续。提交失败或证据矛盾立即从回调返回错误，进入 NeedsAttention 并停止后续批次。
9. 处理 JPG/RAF 部分成功、Shell 取消、结果矛盾、数据库写失败和启动协调；终结事务先最终化 Attempt 报告并推导 Batch/Task 状态，再释放 FileWriteLease/OperationReservation，COMMIT 后才定向重扫。
10. CriticalSafetyViolation 立即停止剩余批次。
11. RetryableFailed/Partial 只允许对仍在原位且身份匹配的剩余文件创建新 Attempt，并重新显示文件数/容量确认；用户也可取消剩余意图。NeedsAttention 先重扫，无法证明时不猜测成功，只允许取消后续意图并保留真实历史报告。
12. Windows 回收站恢复出的文件经扫描作为未标记照片出现，不自动恢复旧标记或继续删除另一成员。
13. 每次删除批次结果提交后重新生成 `PendingDeleteSummary`：全部成功或剩余意图均已取消且没有活动问题时，待处理数量归零，立即返回图库、隐藏待删除入口，并定位到当前排序中最近的有效照片。
14. 若批次结果仍含待重试的 `RetryableFailed`、`NeedsAttention`、`Partial` 或其他活动问题，相关 Photo 继续计入 `PendingDeleteSummary`，保留“待删除 N”入口和当前复核页。
15. 活动 Recycle 暂停 D/R 并禁用导入执行；共享状态条提供“返回进行中删除”。只读图库浏览和待删除结果查看不隐式取消任务。

### Recycle 工程 go/no-go

以下项目必须全部通过，阶段 7 的回收站实现才可认定为在可丢弃 Root 上具备工程能力；本节通过本身不开放真实 FujiLibrary 删除，也不生成 Recycle 准入。真实图库准入只在阶段 8 对最终候选安装包重新验证并绑定准确二进制、锁文件和 schema 后生成：

- 生产可达路径不存在 DeleteFile、remove_file、SHFileOperation 或永久删除降级。
- 在 Windows 10/11 的可丢弃 NTFS 测试数据上验证 flags、COM STA 生命周期和停止语义。
- 回收站禁用、配额不足、超大文件、锁定、Shell 取消和空回收对象均不会永久删除。
- 批次意图先于 Shell 调用提交；每个 Photo 结果提交后才处理下一 Photo。
- Batch、初始 Attempt、FileWriteLease 和全部 OperationReservation 在确认事务内原子提交；任何校验都在占用持有后执行。
- 在调用、回调与结果提交的每个边界完成崩溃注入。
- SQLite BUSY、IOERR、FULL、CORRUPT 或成功证据缺失会停止整个批次。
- 配对部分失败只报告 Partial，不伪造完整成功。
- 从 Windows 回收站恢复后，内容与原文件一致且不会恢复旧删除标记。
- 从旧 SQLite 备份启动不会自动重放旧删除批次。
- Root 离线/重绑定、Photo 版本或成员变化、File ID/大小/mtime 不符、hard link、reparse point、源缺失或原路径被新文件占用都被运行时拒绝。
- 在最终校验后、PreDeleteItem 前后替换文件的故障注入不会伪造成功；身份无法证明时停止并保留可恢复的回收站事实。
- 产品规格第 13.6、18.5 和 18.8 节的全部恢复分支进入机器可读 Recycle 安全报告。
- 阶段 7 的 Recycle 工程能力测试记录与实际测试的二进制、锁文件和 schema 完全匹配；它不是、也不能被读取为真实图库 Recycle 准入。

任何一项未通过，阶段 7 的 Recycle 工程能力门保持关闭；D/R 和待删除复核仍可使用。即使全部通过，真实 FujiLibrary 的 Recycle 执行门也必须继续关闭，直到阶段 8 为最终候选包生成独立准入。

### 验收出口

- 先只用可丢弃文件完成全部回收与恢复测试。
- 逐文件报告和 NeedsAttention 场景可被用户理解。
- 全部成功的最后一批会从待删除页返回图库、隐藏入口并定位最近下一张；仍有失败、结果不明或部分成功时入口和复核页保持可见，数量与逐文件报告一致。
- 二次确认中没有永久删除、继续删除或绕过回收站选项。
- 已知限制明确记录：IFileOperation 的 Shell 路径操作与先前文件句柄之间无法形成完全原子事务；在本机非对抗威胁模型下以紧邻校验、回收后核对、强制回收站和立即停止降低残余风险。
- 阶段 7 只证明实现可在可丢弃 Root 上安全工作，不生成真实图库 Recycle 准入。

### 本阶段不做

应用回收站、永久清空、自动恢复、修改导入逻辑或部分勾选执行。

## 13. 阶段 8：规模硬化与 v1 发布

### 目标

不再增加产品功能，只验证规模、恢复、可访问性和最终发布质量。

### 工作包

1. 用生成数据验证 10 万 Photo、20 万 Asset 和至少 1 TB 索引规模。
2. 校准机械硬盘有限并发、顺序读取优先和浏览时后台任务降速。
3. 验证 32 GB 默认缓存、64 GB 上限、LRU、磁盘安全余量、位置/容量确认、旧 CacheRoot 渐进淘汰和身份异常停止语义。
4. 验证已有索引启动、缓存切图、未缓存预览、D/R、内存和取消响应指标。
5. 完整恢复每日备份和升级前备份；旧备份启动不自动重放写任务。
6. 完成 100%、125%、150% DPI、键盘焦点、对比度和窗口缩放验收。
7. 构建 Windows 候选安装包。
8. 安装该准确候选包，验证全新安装、升级、卸载后用户数据保留规则。
9. 对已安装候选二进制重新运行完整 Import 与 Recycle 安全测试，包括全量故障注入、零写入、原片不变、no-overwrite、永久删除禁止和回收站恢复。
10. 记录二进制 SHA-256、锁文件、schema 和报告版本；测试后任何重新构建都会使结果失效并要求重跑。
11. 验证 SQLite busy_timeout、WAL checkpoint、长读事务限制、AppData 空间不足及报告长期保留策略。
12. 汇总 Import 与 Recycle 两份独立安全报告和已知限制，包括 IFileOperation 的非原子残余窗口。
13. 只有候选安装包完成两套专项报告和全部共享安全回归后，才为该准确构建分别生成 Import 与 Recycle 准入；两份准入保持独立开关，但绑定同一个最终二进制、锁文件和 schema。

### 验收出口

- 上位产品规格第 20 节的 27 条验收条件全部通过。
- 未解决的 Critical 或数据安全缺陷为零。
- 性能指标有真实测量数据，不使用“感觉流畅”替代。
- 真实照片功能仍可分别关闭 Import 与 Recycle。
- 任一共享安全层异常会通过全局写入门同时关闭 Import 与 Recycle。
- 发布说明明确：Windows 回收站和 SQLite 备份不是照片备份。

### 本阶段不做

搜索、历史重复扫描、相似照片、完整 RAW、专业选片、多图库 UI 或任何新的 v1 范围。

## 14. 验证命令约定

工程创建后统一提供以下入口；具体底层命令可以调整，但入口名称保持稳定：

- npm run format:check
- npm run lint
- npm test
- npm run test:ui
- npm run test:e2e
- npm run test:rust
- npm run test:windows
- npm run safety:audit
- npm run tauri build

安全专项测试必须输出机器可读结果和简短人工报告。Import 与 Recycle 不共享一个笼统的“安全测试通过”标志。

## 15. 实施与验收节奏

1. 一次只把一个阶段标为进行中。
2. 每阶段开始前列出将修改的模块和本阶段明确不做的内容。
3. 每阶段完成后提交代码、测试结果、截图或报告。
4. 用户验收当前构建后，再进入下一阶段。
5. Root、设置、导入、恢复和报告页面已经形成上位 UI 规格；若实施时需要改变状态、动作或安全边界，先修订规格并由用户确认，不直接在代码中猜测。
6. 如果写功能延期，阶段 3 的粗筛 Beta 可以长期安全使用，不强迫为了“完整 v1”提前开放导入或删除。

## 16. 下一步

本计划已经获批。实际实施时只启动阶段 0，不同时开展业务功能。阶段 0 首个动作是补齐 Rust/MSVC 开发环境并按 Schema ADR 建立安全底座，然后提交一个默认关闭 Import/Recycle 的可运行 Tauri 空壳与安全测试骨架；本次文档终审不自动开始编码。
