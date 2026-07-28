# 本地 Fuji 照片管理器 v1 分阶段实施计划

- 状态：待用户确认后执行
- 日期：2026-07-28
- 目标平台：Windows 10/11 x64
- 技术路线：Tauri 2 + React/TypeScript + Node.js 24 LTS + Rust + SQLite
- 上位规格：
  - ../specs/2026-07-28-local-photo-manager-design.md
  - ../specs/2026-07-28-local-photo-manager-ui-design.md

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

这九个阶段是对产品规格第 19 节六个交付阶段的细化，不改变 v1 范围：

| 产品规格阶段 | 本计划阶段 |
|---|---|
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

Windows 句柄级文件操作集中在少量基础设施模块中，业务服务不能直接散落调用 std::fs 写 API 或 Shell 删除 API。

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
   - 参考 Tauri 官方的 [Windows 前置条件](https://v2.tauri.app/start/prerequisites/) 与 [建项流程](https://v2.tauri.app/start/create-project/)，Node 版本状态以 [Node.js 官方发布页](https://nodejs.org/en/download/archive/v20.19.6) 为准
2. 创建 Tauri 2、React、TypeScript 工程和锁文件。
3. 建立格式化、lint、Rust clippy、单元测试、前端测试和 Windows 集成测试命令。
4. 实现单实例应用；第二次启动只激活原窗口。
5. 建立强类型 ID、对象版本和领域错误，不让前端传任意写路径。
6. 建立 Root capability 模型与全局 ImportSource deny-write 集合。
7. 建立 SQLite 单写者：
   - 每连接 foreign_keys=ON
   - journal_mode=WAL
   - synchronous=FULL
   - 所有事务检查最终 COMMIT
8. 建立版本化迁移、每日一致备份、升级前备份和损坏时只读安全模式。
   - 保留最近 7 个日常版本和 3 个升级前版本。
   - 导入历史、删除报告和关键操作事件长期保留。
9. 先定义 ImportJob、ImportAssetStep、DeletionBatch、DeletionAssetStep 的纯状态转换表；非法与重复转换拒绝。
10. 建立可确定触发的故障注入框架，覆盖 SQLite commit、Win32 文件调用和 Shell 回调前后。
11. 建立测试 Root 标识；破坏性测试若收到非测试 Root 必须失败。
12. 实现全局写入门、Import 门和 Recycle 门，并使准入记录绑定准确构建产物。
13. 添加安全 API 扫描，阻止生产路径误用覆盖式 rename、永久删除或通用 shell 字符串。

### 验收出口

- npm 和 cargo 的基础检查全部通过。
- 应用能够启动、退出和重复激活。
- 数据库 PRAGMA、备份和只读安全模式由自动测试证明。
- 前端不能调用通用 fs/shell 写接口。
- 状态机非法转换、重复 OperationId 和 COMMIT 失败测试通过。
- Import 执行与 Recycle 执行默认关闭。

### 本阶段不做

照片扫描、业务 UI、Photoshop、导入、回收站调用和任何真实图库写入。

## 6. 阶段 1：只读图库内核

### 目标

在不改变历史照片的前提下，建立 Root、Asset、Photo、配对、时间线和预览数据。

### 工作包

1. 实现 FujiLibrary 首次绑定、卷身份、Root 目录文件身份与可靠重绑定。
2. 只读递归扫描 JPG/JPEG/RAF；跳过并报告 reparse point 和不支持格式。
3. 使用卷身份、文件 ID、相对路径、大小和 mtime 协调稳定 AssetId。
4. 识别 hard link 并标记“不支持删除”。
5. 从安全层已打开的只读句柄或字节流提取 EXIF。
6. 做一个 RAF 技术验证：只提取内嵌 JPEG，不做完整 RAW 解码；失败时返回明确占位状态。
7. 实现保守 JPG/RAF 配对和 photo_version。
8. 实现拍摄时间排序、日期未知固定末尾、自然文件名排序和稳定 PhotoId tie-break。
9. 实现按视口优先的缩略图/预览缓存、32 GB 默认上限、64 GB 绝对上限和缓存 Root 校验。
   - 自选 CacheRoot 不得位于 FujiLibrary、ImportSource、数据库目录或 reparse point。
   - 缓存卷始终保留 max(20 GB, 卷容量的 10%)。
10. 实现启动先读 SQLite、后台增量扫描和手动重扫；启动扫描不做全图库内容哈希。

### 验收出口

- 对同一测试图库重复扫描得到稳定索引和顺序。
- 配对、未知日期、外部缺失、Root 离线和重绑定测试通过。
- reparse point 不被跟随；hard link 不获得删除能力。
- 扫描与预览前后，测试原片的内容哈希、名称、属性和目录树完全一致。
- 通过进程级观察确认扫描、EXIF、RAF 预览和缓存不会向图库写请求。

### 本阶段不做

完整照片 UI、D/R、外部工具、导入写入和回收站删除。

## 7. 阶段 2：白色沉浸式只读浏览 UI

### 目标

交付第一个可供用户真实浏览图库的只读 Alpha。

### 前置设计门

首次 Root 绑定、盘符重绑定和扫描报告页面尚未在核心照片视图规格中冻结。开始这些界面前，先完成一份简短 UI 补充规格并由用户确认。

### 工作包

1. 落地已确认的白色/浅灰视觉 token。
2. 实现顶栏、纵向虚拟 Filmstrip、中央适应窗口画布、右侧详情和底部状态栏。
3. 右栏采用“顶部参数 + 弹性留白 + 沉底动作区”结构。
4. 通过受控 AssetId + version 预览协议取图，不把真实路径交给通用资源协议。
5. 实现新到旧/旧到新切换、日期上下文和日期未知末尾。
6. 实现 Filmstrip 点击、左右键和照片/Filmstrip 上的滚轮浏览；首尾不循环。
7. 实现按图库与排序保存 ViewingCursor；目标离线或缺失时选择最近下一张。
8. 建立图库视图和待删除视图外壳；待删除此时只显示空状态。
9. 完成 100%、125%、150% DPI 和规定窗口尺寸的视觉快照测试。

### 验收出口

- 用户可以绑定并浏览真实图库，但应用仍没有照片写能力。
- 中央画布始终是最大区域；右栏底部区不被元数据推出视口。
- 长文件名、缺失 EXIF、JPG-only、RAF-only 和配对 Photo 状态清楚。
- 已有索引启动、缓存切图和内存占用达到阶段性性能基线。
- 该版本的文件树前后完全不变。

### 本阶段不做

待删除标记、Photoshop、文件位置、导入执行和删除执行。

## 8. 阶段 3：待删除标记与复核闭环

### 目标

交付可用于日常粗筛的 Beta，但仍不移动任何照片。

### 工作包

1. 实现 ToggleDeletionMark 的短 SQLite 事务和 OperationId 幂等。
2. D 只在事务成功后更新徽标、计数并前进；失败保持当前照片。
3. 实现当前会话内多步 R 撤销；按 OperationId 撤销并回到受影响 Photo。
4. 串行化标记写入；pending 时阻止重复标记和导航，不排队。
5. 实现 Root 离线时只能移除标记，不能新增或通过 R 恢复标记。
6. Photo 成员、身份、大小或 mtime 变化时使旧标记失效。
7. 实现待删除 Filmstrip、当前项取消、问题项和批次摘要查询。
8. 待删除页不包含 Photoshop、文件位置、复选框或部分执行。
9. 最终删除入口由 Recycle 功能门控制，在阶段 7 前明确保持不可执行。

### 验收出口

- D/R 的成功、失败、重复输入、auto-repeat、失焦、弹窗和离线矩阵通过。
- 配对变化不会让新 Asset 静默继承旧删除决定。
- 重启后标记仍在，但 R 历史为空。
- 用户可以完成“浏览 → 标记 → 待删除复核 → 取消标记”闭环。
- 原片目录树和文件内容零变化。

### 本阶段不做

任何 IFileOperation、永久删除、应用回收站或物理文件移动。

## 9. 阶段 4：Photoshop 与文件位置

### 目标

完成图库页的两个外部工具按钮，不扩散通用 shell 权限。

### 工作包

1. 在实现设置页前补充并让用户确认一个简短的设置/首次选择 UI 规格。
2. 首次点击时由 Rust 原生选择器选择并验证 Photoshop.exe，保存 ToolId。
   - 首次交给 Photoshop 前明确说明：文件交给外部程序后，其修改、保存或 sidecar 行为不受本应用控制。
3. 设置页提供“重新选择 Photoshop”。
4. JPG+RAF 和 RAF-only 明确打开 RAF；JPG-only 禁用 Photoshop。
5. JPG+RAF 的文件位置定位 JPG；JPG-only 定位 JPG；RAF-only 定位 RAF。
6. 直接调用已注册可执行文件并传独立参数，不经过命令行字符串拼接。
7. 待删除页 DOM 中不渲染 Photoshop、文件位置按钮或占位。

### 验收出口

- 三种 JPG/RAF 组合逐项通过。
- 伪造 ToolId、跨 Library PhotoId、过期版本和任意路径请求被拒绝。
- 本应用进程在启动外部工具前不修改照片；验收只使用可丢弃的 JPG/RAF 副本，不用唯一原片验证外部程序行为。

### 本阶段不做

监控 Photoshop 保存结果、在待删除页开放工具、导入或回收站删除。

## 10. 阶段 5：只读导入预检

### 目标

让用户在复制前知道哪些文件会新增、跳过、改名或失败，同时内存卡和图库都保持只读。

### 前置设计门

导入预检、进度、恢复和报告页面尚未在核心 UI 规格中冻结。开始本阶段代码前，先完成一份简短 UI 补充规格并由用户确认。

### 工作包

1. 通过 Rust 原生选择器选择内存卡子目录，保存卷身份和相对文件夹。
2. 同一张卡再次插入时自动启动只读预检，但绝不自动复制。
3. 递归只扫描所选子树；整个源卷加入 deny-write。
4. 整批处理所选子树中的全部受支持文件；v1 不提供逐文件勾选。
5. 只读读取元数据、源身份、哈希和必要字节，不让第三方库按路径自行打开。
6. 实现 ImportHistory 候选缩小，但不让历史记录单独触发跳过。
7. 只有当前可访问目标经完整哈希且逐字节相同才判定已导入。
8. 实现目标日期路径、Unknown-Date、一致后缀、名称安全和空间规划。
9. 计算新增容量，要求执行完成后目标卷仍保留 max(10 GB, 卷容量的 2%)。
10. 展示新增、重复、冲突、日期未知和无法读取的整批预检报告。
11. 暂不创建日期目录、partial 或正式文件。

### 验收出口

- stale ImportHistory、目标离线、目标缺失、内容变化和旧数据库恢复不会错误跳过源文件。
- 模拟哈希碰撞时，逐字节比较能拒绝误判。
- 更换同盘符的另一张卡不会继承原卡身份。
- 对完整预检、EXIF、RAF 预览、哈希、取消和异常流程做进程级审计，源卷写请求为零。
- 预检前后内存卡和图库目录树、内容、名称与可观察属性不变。

### 本阶段不做

创建目录、复制、partial、发布、历史图库重复扫描或删除内存卡文件。

## 11. 阶段 6：安全导入执行与恢复

### 目标

只复制真正新增的照片，任何时候都不覆盖目标、不删除源文件，并能解释应用崩溃后的中间状态。

### 工作包

1. 确认后先持久化不可变 ImportJob 和全部 ImportAssetStep；每个 partial 计划明确包含 JobId + StepId + 随机 nonce + partial 相对路径。
2. 从已注册 Root 逐级重验卷、目录身份、containment 和 reparse point。
3. 执行前重新校验目标卷完成后仍能保留 max(10 GB, 卷容量的 2%)。
4. 在最终目录以 CREATE_NEW 创建唯一 partial，并记录实际 file ID。
5. 复制完成后 FlushFileBuffers、关闭写句柄，再从目标卷重读。
6. 对 partial 与源执行长度、完整哈希和逐字节比较，并重新验证源未变化。
7. 使用句柄级 SetFileInformationByHandle 和 ReplaceIfExists=FALSE 发布。
8. 发布后独立提交 Asset、ImportHistory 和报告；索引失败不删除已发布文件。
9. 实现取消、拔卡、换卡、源变化、空间不足、名称抢占、部分完成和显式重试。
10. 重启发现未完成 ImportJob 时先显示摘要，由用户选择“继续”或“取消”，禁止静默续传；原卡缺失或错误卡进入 AwaitingSource。
11. 实现启动协调矩阵；未知 partial 不采用、不删除，Published 文件不回滚。
12. 已发布并索引的照片渐进加入时间线，但不改变当前 PhotoId。

### Import go/no-go

真实 FujiLibrary 开放导入前，以下项目必须全部通过：

- 并发程序在发布瞬间抢占同名目标，原目标仍一个字节不变。
- 静态和运行时检查均证明发布路径不会调用覆盖式 rename。
- 在每个文件系统动作与 SQLite commit 前后注入崩溃，结果都收敛为定义状态。
- 未落库 file ID、身份不符或未知来源的 partial 不会被采用或清理。
- 拔卡、换卡和源文件读取中变化进入 AwaitingSource 或 NeedsAttention。
- Flush、关闭、重读、哈希、逐字节比较和源身份复核全链路通过。
- 取消或数据库失败不会删除或改名已 Published 文件。
- 所有既有正式文件在测试前后内容、名称和属性完全不变。
- 完整流程对源卡的写请求审计为零。
- 非本机 NTFS、源目标同卷、Root/reparse/mount point 变化、危险文件名或空间不足都被运行时拒绝。
- 正式文件与 partial 的全部恢复组合以及旧数据库备份恢复均按规格收敛。
- 产品规格第 18.3、18.4 和 18.7 节相关测试全部进入机器可读 Import 安全报告。
- Import 准入记录与实际测试的二进制、锁文件和 schema 完全匹配。

任何一项未通过，Import 执行门保持关闭；只读导入预检仍可使用。

### 验收出口

- 使用可丢弃 NTFS 测试 Root 完成新增、重复、冲突、日期未知、取消和恢复矩阵。
- 对复制后的每个成功文件重新验证与源逐字节一致。
- 用户报告明确区分已发布、已跳过、部分完成、失败、冲突和 NeedsAttention。
- 顺序固定为“构建 Import Beta 候选包 → 安装该包 → 对已安装二进制运行 Import 安全测试 → 固定哈希”；任何重新构建都会使结果失效。
- 通过独立 Import 安全报告后，为准确受测构建生成 Import 准入；用户仍需对每次复制显式确认。

### 本阶段不做

Windows 回收站、永久删除、源卡清理或历史图库重复扫描。

## 12. 阶段 7：Windows 回收站删除

### 目标

在待删除清单最终复核后，把全部剩余有效 Photo 强制移入 Windows 回收站；任何结果不明都立即停止。

### 前置设计门

待删除核心视图和最终确认框已经冻结；删除结果报告页面尚未冻结。实现报告页前先补充并确认简短 UI 规格，不改变已确认的复核页按钮和布局。

### 工作包

1. 完成“执行删除…”按钮、整批摘要和最终确认对话框。
2. 确认后冻结不可变 DeletionBatch 与逐 Asset 意图，并清空会话撤销栈。
   - 确认框打开时禁用 D/R、左右键和滚轮切图。
   - DeletionBatch 执行期间暂停全部 D/R，但允许只读浏览；批次结束后才重新开放标记。
3. 执行前重新验证 Root、NTFS、Photo 版本、成员、文件 ID、大小、mtime、containment 和 hard link。
4. 使用专用 COM STA worker 承载 IFileOperation 与 progress sink。
5. 每个 Photo 一次 IFileOperation，配对成员在同一次操作中排队。
6. 明确使用 FOFX_RECYCLEONDELETE | FOF_NOERRORUI | FOFX_EARLYFAILURE | FOF_NOCONFIRMATION；禁止 FOF_WANTNUKEWARNING，不存在永久删除或降级路径。
7. 逐 Asset 记录 PostDeleteItem、hrDelete、回收对象和 GetAnyOperationsAborted。
8. 只有成功回调、非空回收对象、源路径消失且结果成功提交后才记 Recycled。
9. 处理 JPG/RAF 部分成功、Shell 取消、结果矛盾、数据库写失败和启动协调。
10. CriticalSafetyViolation 立即停止剩余批次。

### Recycle go/no-go

真实 FujiLibrary 开放删除前，以下项目必须全部通过：

- 生产可达路径不存在 DeleteFile、remove_file、SHFileOperation 或永久删除降级。
- 在 Windows 10/11 的可丢弃 NTFS 测试数据上验证 flags、COM STA 生命周期和停止语义。
- 回收站禁用、配额不足、超大文件、锁定、Shell 取消和空回收对象均不会永久删除。
- 批次意图先于 Shell 调用提交；每个 Photo 结果提交后才处理下一 Photo。
- 在调用、回调与结果提交的每个边界完成崩溃注入。
- SQLite BUSY、IOERR、FULL、CORRUPT 或成功证据缺失会停止整个批次。
- 配对部分失败只报告 Partial，不伪造完整成功。
- 从 Windows 回收站恢复后，内容与原文件一致且不会恢复旧删除标记。
- 从旧 SQLite 备份启动不会自动重放旧删除批次。
- Root 离线/重绑定、Photo 版本或成员变化、File ID/大小/mtime 不符、hard link、reparse point、源缺失或原路径被新文件占用都被运行时拒绝。
- 产品规格第 18.5 和 18.7 节的全部恢复分支进入机器可读 Recycle 安全报告。
- Recycle 准入记录与实际测试的二进制、锁文件和 schema 完全匹配。

任何一项未通过，Recycle 执行门保持关闭；D/R 和待删除复核仍可使用。

### 验收出口

- 先只用可丢弃文件完成全部回收与恢复测试。
- 逐文件报告和 NeedsAttention 场景可被用户理解。
- 二次确认中没有永久删除、继续删除或绕过回收站选项。
- 阶段 7 只证明实现可在可丢弃 Root 上安全工作，不生成真实图库 Recycle 准入。

### 本阶段不做

应用回收站、永久清空、自动恢复、修改导入逻辑或部分勾选执行。

## 13. 阶段 8：规模硬化与 v1 发布

### 目标

不再增加产品功能，只验证规模、恢复、可访问性和最终发布质量。

### 工作包

1. 用生成数据验证 10 万 Photo、20 万 Asset 和至少 1 TB 索引规模。
2. 校准机械硬盘有限并发、顺序读取优先和浏览时后台任务降速。
3. 验证 32 GB 默认缓存、64 GB 上限、LRU 和磁盘安全余量。
4. 验证已有索引启动、缓存切图、未缓存预览、D/R、内存和取消响应指标。
5. 完整恢复每日备份和升级前备份；旧备份启动不自动重放写任务。
6. 完成 100%、125%、150% DPI、键盘焦点、对比度和窗口缩放验收。
7. 构建 Windows 候选安装包。
8. 安装该准确候选包，验证全新安装、升级、卸载后用户数据保留规则。
9. 对已安装候选二进制重新运行完整 Import 与 Recycle 安全测试，包括全量故障注入、零写入、原片不变、no-overwrite、永久删除禁止和回收站恢复。
10. 记录二进制 SHA-256、锁文件、schema 和报告版本；测试后任何重新构建都会使结果失效并要求重跑。
11. 汇总 Import 与 Recycle 两份独立安全报告和已知限制。
12. 只有候选安装包完成两套专项报告和全部共享安全回归后，才为该准确构建分别生成 Import 与 Recycle 准入；两份准入保持独立开关，但绑定同一个最终二进制、锁文件和 schema。

### 验收出口

- 上位产品规格第 20 节的 21 条验收条件全部通过。
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
5. 阶段 2、4、5、7 涉及尚未冻结的 Root、设置、导入或报告页面，分别在首次实现前完成小型 UI 规格，不直接猜测界面。
6. 如果写功能延期，阶段 3 的粗筛 Beta 可以长期安全使用，不强迫为了“完整 v1”提前开放导入或删除。

## 16. 下一步

计划获批后只启动阶段 0，不同时开展业务功能。阶段 0 首个动作是补齐 Rust/MSVC 开发环境，然后提交一个默认关闭 Import/Recycle 的可运行 Tauri 空壳与安全测试骨架。
