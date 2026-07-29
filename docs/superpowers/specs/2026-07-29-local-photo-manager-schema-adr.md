# 本地 Fuji 照片管理器 v1 Schema ADR

**状态：已确认，可实施**
**日期：2026-07-29**
**适用范围：v1 SQLite schema、领域状态机、事务与恢复契约**

## 1. 决策

v1 使用一个位于 `LocalAppData` 的 SQLite 数据库保存：

- Root 注册、身份与重绑定历史；
- Library、Asset、Photo 与 JPG/RAF 配对关系；
- 浏览游标和用户待删除标记；
- 导入、回收站删除、重试、恢复和报告的持久化意图与结果；
- 缓存位置、所有权和缓存条目索引；
- 幂等操作记录、schema 版本和安全准入所需元数据。

SQLite 是索引、用户意图、任务状态和审计历史的事实来源；照片是否物理存在、partial 是否存在、文件是否进入 Windows 回收站等事实仍由文件系统与 Shell 结果证明。二者不组成一个 ACID 事务。

本 ADR 冻结的是表之间的语义、约束、状态转换与事务边界。迁移实现可以把字段拆表、合表，或在 `INTEGER`、`TEXT`、`BLOB` 之间选择等价编码，但必须由约束测试证明本 ADR 的全部语义仍成立；不得把本 ADR 已冻结的约束降级为 UI 约定。

术语统一如下：UI 所称“执行删除”在 v1 始终是移入 Windows 回收站；领域表使用 `Deletion*` 表示待删除标记、批次与执行记录，`Recycle` 只作为 Task/Report/OperationReservation 的执行种类与“已进入回收站”的结果词。本文没有任何永久删除照片的含义。

## 2. 设计原则

1. **ID 不是授权**：所有文件动作必须从 ID 重新解析 Root、当前对象版本和文件身份。
2. **持久化意图先于文件动作**：Import、Recycle 和缓存删除都先提交可恢复的意图及所有权证据。
3. **物理事实不可回退**：已经证明的 `Published`、`Indexed`、`Recycled` 和已最终化报告不能被后续失败改写为“未发生”。
4. **历史与当前状态分离**：当前行可以保存查询用汇总状态，但 Attempt、逐文件结果、Root 绑定和报告使用追加记录保留历史。
5. **保守协调**：身份不完整、记录与磁盘矛盾或数据库恢复到旧版本时，结果是停止、重新观察或 `NeedsAttention`，不是猜测成功。
6. **单写者、短事务**：长时间哈希、复制、Shell 调用和图片解析期间不持有 SQLite 写事务。
7. **墓碑优先**：被任务、游标、别名、报告或历史引用的 Library、Root、Asset、Photo 不物理删除。
8. **派生数据可重建，用户决定不可丢失**：缩略图、EXIF 缓存和排序辅助键可以重建；标记、游标、任务、结果和报告必须备份。

## 3. SQLite 基础约定

### 3.1 推荐 SQL 表示

| 领域值 | 推荐 SQLite 类型 | 必须满足的语义 |
|---|---|---|
| 外部可见 ID | `BLOB(16)` + `CHECK(length(id)=16)` | Rust 强类型生成的不可复用 128 位值；所有外键使用同一编码 |
| 内部行号 | `INTEGER PRIMARY KEY` | 只用于数据库内部，不得传给 React 作为授权 |
| 对象版本 | `INTEGER` | 从 1 开始，只递增，禁止减小或复用 |
| UTC 时间 | `INTEGER` | Unix 微秒；只用于事件顺序和展示，不充当文件身份 |
| Windows mtime | `INTEGER` | 无损保存原始 64 位 `FILETIME`，不得经秒级截断后参与校验 |
| 文件大小 | `INTEGER` | 非负 64 位字节数 |
| 枚举 | `TEXT` + `CHECK` | 只接受本 ADR 定义的值；未知值必须经 schema 迁移引入 |
| 布尔值 | `INTEGER` + `CHECK (value IN (0,1))` | 不接受其他整数或 `NULL` |
| 文件 ID、哈希 | `BLOB` | 同时保存算法/格式版本；禁止只存显示字符串 |
| 相对路径 | `TEXT` | 保存原始展示值；不能包含绝对前缀、`..`、ADS 或越界段 |
| 路径比较键 | `BLOB` 或 `TEXT COLLATE BINARY` | 由 Rust 按 Windows 大小写不敏感规则生成的版本化规范键 |

SQLite 自带的 `NOCASE` 不能单独作为 Windows 路径等价判断。迁移必须保存由 Rust 安全路径层生成的 `path_key` 和 `path_key_version`；真实路径校验仍在 Rust 通过句柄完成。

容量阈值在数据库、Rust 和测试中一律使用以下精确字节常量；UI 可以继续显示简洁的 `GB` 文案，但不得另行按十进制换算：

```text
ONE_GB_BYTES             = 1_073_741_824
IMPORT_MIN_RESERVE_BYTES = 10_737_418_240   # 10 GB
CACHE_DEFAULT_BYTES      = 34_359_738_368   # 32 GB
CACHE_MAX_BYTES          = 68_719_476_736   # 64 GB
CACHE_MIN_RESERVE_BYTES  = 21_474_836_480   # 20 GB
```

百分比余量使用整数向上取整计算，避免浮点和不同语言舍入差异：`ceil(total_bytes * percent / 100)`。

诊断信息可以使用 JSON，但 Root 身份、文件身份、版本、状态、所有权、大小、mtime、哈希、目标路径及逐文件结果不得只存在 JSON 中。

### 3.2 连接与事务设置

每个连接必须执行并验证：

```text
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
```

应用保持单实例和单写者。每个写事务都检查最终 `COMMIT` 返回值；SQL 语句执行成功不等于事务已持久化。允许独立只读连接，但只读连接不得改变 pragma、执行迁移或绕过领域仓储层。

## 4. Root 与 Library

### 4.1 `Library`

必需字段：

| 字段 | 类型/约束 | 语义 |
|---|---|---|
| `library_id` | ID，PK | 稳定逻辑图库身份 |
| `library_kind` | 枚举 | v1 只创建 `Fuji`，表结构允许未来增加其他类型 |
| `display_name` | `TEXT NOT NULL` | 用户可读名称，不参与授权 |
| `lifecycle` | `Active | Inactive | Tombstoned` | 当前是否是 v1 正在使用的图库；Inactive 保留全部状态并可可靠切回 |
| `created_at` | UTC 时间 | 创建时间 |
| `tombstoned_at` | UTC 时间，可空 | 永久逻辑墓碑时间；普通图库切换只使用 Inactive，不写该字段 |

`CurrentLibrary(slot INTEGER PRIMARY KEY CHECK(slot=1), library_id UNIQUE FK)` 保存 v1 当前图库。首次绑定完成前允许没有该行；绑定后它与 `Library.lifecycle` 必须由同一仓储事务保持一致：恰有一个当前 Fuji Library 为 `Active`；曾使用但当前未选择的图库为 `Inactive`。不使用 `library_kind='Fuji'` 的全局唯一索引封死未来多图库能力。

可靠的原 Root 重连继续使用原 `LibraryId + RootId`。候选目录无法证明为任何已知 Root 时，必须在一个事务中创建新的 `Library + FujiLibrary Root`、把旧当前 Library 变为 `Inactive` 并切换 `CurrentLibrary`；旧 Asset、Photo、标记、游标、任务和报告均不迁移。以后重新选择某个已知 Root 且可靠身份验证通过时，可以把其原 Library 切回 `Active`，仍不得复制或合并两个 Library 的状态。

### 4.2 `VolumeIdentity`

`VolumeIdentity` 保存一次经 Rust 验证的卷身份，而不是盘符：

- `volume_id`：PK；
- `filesystem_kind`：`NTFS | exFAT | ReFS | Other`；
- Windows 卷 GUID、卷序列号及可获得的设备身份；
- 由 Rust 根据强身份字段生成的 `strong_identity_key`；
- `identity_format_version`；
- `identity_strength`：`Strong | UserConfirmedLimited`；
- `created_at`。

`identity_strength='Strong'` 时 `strong_identity_key` 必须非空，并由 `(identity_format_version, strong_identity_key)` 的部分唯一索引保证同一物理卷只有一个规范 `volume_id`。发生唯一冲突时复用既有 `VolumeIdentity`，不得创建可绕过卷策略的新 ID。本机可写 Fuji Root 必须具有强 NTFS 卷身份。内存卡若无法提供稳定目录身份，可保存 `UserConfirmedLimited` 证据，但不得据此只凭盘符、卷标或同名目录自动认定为原卡；任何写入候选卷若无法证明与全部受保护 ImportSource 卷不同，也必须拒绝。

作为 `ImportSource` 使用过的卷必须在 `VolumePolicy(volume_id PK, deny_write CHECK(deny_write=1), reason, created_at)` 中持久化全局拒写策略。v1 的该行只允许插入，禁止 UPDATE/DELETE，不存在 `retired_at`、解除保护或改回 0 的状态；`ImportSourceProfile` 可以退休，但卷级拒写永久保留。Rust 文件安全层在每次创建、rename、回收或缓存写入前，先把当前强卷证据规范化到唯一 `volume_id`，再查询该集合；身份冲突、证据不足或无法排除受保护卷时同样拒写。UI 隐藏按钮不算执行此约束。

### 4.3 `Root`

必需字段：

| 字段 | 类型/约束 | 语义 |
|---|---|---|
| `root_id` | ID，PK | 稳定 Root 身份 |
| `library_id` | FK，可空 | `FujiLibrary` 必须属于 Library；其他 Root 可为空 |
| `root_kind` | `FujiLibrary | ImportSource | AppDataRoot | CacheRoot` | 能力边界 |
| `capability_mask` | `INTEGER NOT NULL` | Rust 定义且版本化的只读/创建/回收等能力集合 |
| `current_binding_id` | FK，可空 | 当前已验证绑定；离线时仍保留最后绑定 |
| `root_version` | `INTEGER >= 1` | 每次绑定、能力或身份状态变化递增 |
| `lifecycle` | `Online | Offline | IdentityMismatch | Tombstoned` | 当前观察状态 |
| `created_at`、`tombstoned_at` | UTC 时间 | 生命周期 |

约束：

- 一个活动 Library 在 v1 最多有一个活动 `FujiLibrary` Root；用部分唯一索引实现。
- `FujiLibrary` 必须有 `library_id`；其他类型是否关联 Library 由用途决定。
- `Tombstoned` Root 不得获得写能力。
- Root 行、绑定行和 VolumeIdentity 均使用 `ON DELETE RESTRICT`。

### 4.4 `RootBinding`

Root 的每次首次绑定和重绑定追加一行：

- `binding_id`、`root_id`、`binding_generation`；
- `volume_id`；
- 规范化绝对路径的展示值与比较键；
- Root 目录 file ID；不支持时为 `NULL` 并把证据等级记录为 `UserConfirmedLimited`；
- 文件系统类型、reparse 检查结果和验证时间；
- `binding_state`：`Active | Superseded | Unverified`；
- `supersedes_binding_id`。

每个 Root 只能有一个活动 Binding。相同 Root 的可靠重连要求：

- NTFS Fuji Root 同时匹配卷身份和 Root 目录 file ID；盘符不参与身份判断；
- ImportSource 还要匹配已选择的源相对目录；若目录 ID 不稳定，必须由用户重新选择并只读复核；
- 当前存在未终结 Import/Deletion/恢复任务或活动 reservation 时禁止切换相关 Root；
- FujiLibrary 候选身份不匹配时按第 4.1 节创建新的 Library 与新 Root，不更新旧 Root，也不迁移旧索引、标记或游标；ImportSource/Profile 与 CacheRoot 身份不匹配分别按第 8.1 节和第 11 节重新选择并建立其自身新 Root，不得因此创建 Fuji Library。

同一物理目录不能同时拥有冲突 Root 能力。FujiLibrary、ImportSource、Cache 专属目录和 AppData 数据库目录必须进行双向祖先/后代重叠检查。该检查依赖真实目录句柄，不能只靠 SQL 字符串前缀；数据库保存检查结果和 Root 身份用于审计。

## 5. Catalog：Asset、Photo 与配对

### 5.1 `Asset`

`Asset` 是一个物理 JPG/JPEG/RAF 的稳定应用身份：

| 字段 | 类型/约束 | 语义 |
|---|---|---|
| `asset_id` | ID，PK | 不复用 |
| `library_id`、`root_id` | FK，非空 | v1 照片 Asset 归属 |
| `media_kind` | `Jpg | Raf` | `.jpg`、`.jpeg` 均为 `Jpg` |
| `current_version` | `INTEGER >= 1` | 指向当前 AssetRevision |
| `presence_state` | `Present | Missing | Tombstoned` | 最近可靠观察；不是删除成功证明 |
| `deletion_supported` | 布尔 | hard link、非 NTFS 等情况为 0 |
| `created_at`、`tombstoned_at` | UTC 时间 | 生命周期 |

活动 Asset 的以下约束必须由部分唯一索引实现：

- `(root_id, path_key)` 唯一，使用当前 `AssetRevision` 的大小写折叠路径键；
- 在 file ID 可用时 `(root_id, file_id)` 唯一；
- 同一物理文件不能通过两个活动 Asset 进入两个 Photo。

SQLite 无法直接对“当前 Revision”建立简单唯一索引时，迁移可以把当前 `path_key` 和 `file_id` 冗余到 Asset 行，并用触发器或同一事务保持一致。语义不得放宽。

跨 Root 发现的文件在 v1 获得新 AssetId，不继承旧状态。同一 Root 内只有在卷和 file ID 可靠一致时，重命名或移动才可保留 AssetId；存在歧义时墓碑旧 Asset 并创建新 Asset。

### 5.2 `AssetRevision`

每次有效观察造成下列任一变化时，追加新 Revision 并递增 `asset_version`：

- Root 绑定代次或接受的 Root 关联；
- 相对路径/path key；
- file ID；
- 大小或无损 mtime；
- 已计算内容指纹发生变化；
- Present/Missing 状态；
- hard-link/删除能力事实。

必需字段：

- `(asset_id, asset_version)` 复合 PK；
- `root_id`、`root_binding_id`、`relative_path`、`path_key`、`path_key_version`；
- `file_id`、`size_bytes`、`mtime_filetime`；
- 可空的 `content_hash_algorithm`、`content_hash`；
- `presence_state`、`hard_link_count`、`observed_at`、`scan_run_id`。

Revision 一经插入不可更新或删除。只有 CatalogService 的单个事务可以追加 Revision、更新 Asset.current_version，并递增所有受影响 Photo 的 `photo_version`。原地修改即便仍使用同一 file ID，只要大小、mtime 或已知内容指纹变化，也必须形成新 Revision。

### 5.3 `Photo`

必需字段：

- `photo_id`：PK；
- `library_id`：FK；
- `photo_version`：从 1 开始只递增；
- `lifecycle`：`Active | Tombstoned`；
- 当前排序所需的可靠拍摄时间、本地时间解释、日期未知标志、自然文件名键和排序键版本；
- `created_at`、`tombstoned_at`。

以下变化必须在同一 Catalog 事务中递增 `photo_version`：

- 成员 Asset 增加、移除或角色改变；
- 任一成员 `asset_version` 递增；
- 配对证据失效、配对合并或拆分。

纯缩略图、预览或非授权展示字段重建不递增版本。

### 5.4 `PhotoAsset` 与 `PairEvidence`

`PhotoAsset` 保存成员关系历史：

- `photo_asset_id`：PK；
- `photo_id`、`asset_id`：FK；
- `role`：`Jpg | Raf`；
- `pair_evidence_id`：可空 FK；
- `from_photo_version`、`to_photo_version`；`to_photo_version IS NULL` 表示当前成员。

部分唯一约束：

- 每个活动 Photo 最多一个当前 `Jpg` 和一个当前 `Raf`；
- 每个活动 Asset 最多属于一个当前 Photo；
- 同一 `(photo_id, asset_id)` 不能有两条当前关系。

`PairEvidence` 是不可变记录，至少区分：

- `HistoricalExact`：同一 Library、同名主体、候选唯一、两端均有可靠且一致的 `DateTimeOriginal`，且相机型号/序列信息不存在冲突；
- `SameImportGroup`：同一次 ImportJob、同一源目录、同一文件名主体且候选唯一。

历史扫描中任一端日期缺失、不可解析或冲突时不得自动配对。导入组证据可以按已确认矩阵处理单端日期或双方日期未知；两端可靠日期冲突时仍不得配对。证据不足时两个 Asset 分别形成 Photo。

### 5.5 Photo 合并、拆分与墓碑

规则必须由 CatalogService 确定执行：

1. 单张 Photo 后来增加可靠配对成员：保留原 PhotoId。
2. 已独立建档的 JPG 与 RAF 后来可靠配对：保留 JPG 的 PhotoId；RAF 的旧 Photo 进入墓碑。
3. 配对失效且两文件仍存在：JPG 保留原 PhotoId，RAF 获得新 PhotoId。
4. 配对只缺一个成员：剩余成员保留原 PhotoId。
5. 所有成员变化均递增版本并使旧删除标记失效。

`PhotoAlias(old_photo_id PK, canonical_photo_id FK, reason, created_at)` 只服务于浏览位置恢复和历史报告。Alias 不允许成环或形成链；目标必须是最终 Photo。任何标记、导入、删除或外部工具写命令都禁止沿 Alias 转移授权。

`PhotoRelationEvent` 追加记录 Merge、Split、MemberAdded、MemberRemoved 及前后版本，便于恢复与诊断；不靠覆盖历史关系解释旧报告。

## 6. 浏览游标

`ViewingCursor` 以 `(library_id, sort_mode)` 为主键；v1 `sort_mode` 为 `CaptureDesc | CaptureAsc`。字段包括：

- 当前 `photo_id`；
- `photo_version`，仅作恢复诊断，不授予写权限；
- 排序锚快照：`date_class`（`Known | Unknown`）、本地拍摄时间键、自然文件名展示值、版本化自然排序键、锚点 PhotoId；
- `sort_key_version`、`updated_at`。

本地拍摄时间键使用无时区的固定宽度 `YYYY-MM-DDTHH:MM:SS[.fraction]` 语义；缺少时为 `NULL`，不能用文件时间填充。排序契约：已知日期先按所选方向排列；`Unknown` 始终位于末尾；同一拍摄时间按自然文件名，再按 PhotoId。当前 Photo 不存在、离线或墓碑时，使用保存的完整排序锚寻找当前排序中最近的下一张；没有下一张才选择上一张。不能仅保存列表下标。

更新游标是可覆盖的不可重建用户状态，必须在短事务中提交。后台扫描和渐进导入不得因为插入新行而改变其 `photo_id`。

## 7. 通用文件快照、幂等操作与任务所有权

### 7.1 `FileIdentitySnapshot`

所有破坏性意图和物理事实引用不可变快照，而不是只引用当前 Asset 行。快照至少包含：

- `snapshot_id`：PK；
- `root_id`、`root_binding_id`、`volume_id`；
- `relative_path`、`path_key`、`path_key_version`；
- 可用的 `file_id`、`size_bytes`、无损 `mtime_filetime`；
- 可空的内容哈希及算法；
- `captured_at`、`evidence_level`：`Strong | UserConfirmedLimited`。

快照不可更新。只有 `Strong` 快照可授权 FujiLibrary 的导入发布或回收动作；`UserConfirmedLimited` 只用于 ImportSource 的只读识别。执行动作时仍须重新打开句柄并与快照逐项核对；快照本身不是写能力。

### 7.2 `OperationEvent`

所有会改变持久状态的命令携带 `OperationId`。`OperationEvent` 保存：

- `operation_id`：PK；
- 操作类型、目标 ID、请求语义摘要哈希；
- 首次接收时间、最终结果码和关联实体 ID。

重复 OperationId 且请求摘要相同返回已提交结果；相同 OperationId 携带不同请求必须拒绝。删除活动 `DeletionMark` 也不能删除 OperationEvent，因此 UI 重试不会重复切换。

### 7.3 `TaskIntent` 与全局写入门

`TaskIntent` 是 ImportJob 和 DeletionBatch 的共同父记录，必需字段为：

- `task_id`：PK；
- `task_kind`：`Import | Recycle`；
- `lifecycle`：`Prepared | Active | AwaitingUser | AwaitingSource | NeedsAttention | Completed | Cancelled`；
- `row_version`、`created_at`、`resolved_at`。

合法主路径为：

```text
Prepared -> Active | AwaitingUser | AwaitingSource | NeedsAttention
         -> Cancelled
Active   -> AwaitingUser | AwaitingSource | NeedsAttention | Completed | Cancelled
AwaitingUser | AwaitingSource | NeedsAttention -> Active | Cancelled
```

最后一行只有在用户显式确认、创建新 Attempt 并重新验证所需身份后才允许进入 `Active`。`Completed` 与 `Cancelled` 是终态；终态 TaskIntent 禁止 UPDATE/DELETE。部分唯一索引 `UNIQUE(1) WHERE lifecycle NOT IN ('Completed','Cancelled')` 保证 v1 最多只有一个未终结文件任务。`task_kind='Import'` 的 TaskIntent 必须恰好由同 ID 的 ImportJob 使用，`task_kind='Recycle'` 必须恰好由同 ID 的 DeletionBatch 使用；通过延迟外键、触发器或同一创建事务验证，不允许无子类型或跨类型引用。

`TaskAttempt(attempt_id PK, task_id FK, attempt_kind, attempt_ordinal, operation_id UNIQUE FK OperationEvent, created_at)` 提供 Import/Recycle Attempt 的共同身份。`UNIQUE(task_id, attempt_ordinal)` 保证稳定的“第 N 次”顺序；`attempt_kind` 必须与父 TaskIntent 一致，并且每个 TaskAttempt 必须恰好拥有一个同 ID、同类型的 ImportAttempt 或 DeletionAttempt 子行。TaskAttempt 创建后不可更新/删除，本身不另设状态；OperationReservation 与 Report 统一引用它，运行/终态由同 ID 的 ImportAttempt/DeletionAttempt 子行表达，避免用一个多态 ID 猜测两张表。

`FileWriteLease` 是固定 `slot=1` 的可变栅栏行，字段至少包括当前可空 `task_id`、只递增 `lease_generation`、`acquired_at`、`released_at` 和 `row_version`。取得 Lease、递增 generation、把任务/新 Attempt 变为可执行以及建立本 Attempt OperationReservations 必须在同一事务中完成。每次 TaskAttempt-owned 文件动作都携带 `task_id + attempt_id + lease_generation`，并重新查询栅栏、Attempt=`Running` 与对应未释放 OperationReservations；旧 worker、旧回调、终态 Attempt 或旧 generation 不再具有执行资格。CacheEvictionIntent-owned 动作不使用 FileWriteLease，改为每次核对 Intent 状态与其唯一活动 OperationReservation。

任务在第一次 Attempt 前进入 `AwaitingUser | AwaitingSource | NeedsAttention` 时可以尚未取得 Lease，但非终态 TaskIntent 的唯一约束已经禁止另一个 Import/Recycle。任务一旦取得过 Lease，进入上述等待态时保留它；当前 Attempt 的 OperationReservations 按第 13.5 节原子关闭规则释放，使 Reconciler 可以重新观察。只有 TaskIntent 进入 `Completed | Cancelled` 的同一终结事务才清空非空 Lease 的 `task_id` 并递增 generation。进程崩溃后非空 Lease 保留，先由 Reconciler 处理，不得在启动时无条件清空。

### 7.4 `OperationReservation`

OperationReservation 在其 owner 取得文件动作资格前持久化，字段包括：

- `operation_reservation_id PK`、`owner_kind`：`TaskAttempt | CacheEvictionIntent`；
- Task owner 专用的可空 `task_id`、`attempt_id`、冻结 `lease_generation`；
- Cache owner 专用的可空 `cache_eviction_intent_id`、`cache_entry_id`；
- `reservation_kind`：`SourceRead | FinalPath | PartialPath | ExistingAsset | RecycleAsset | CacheEntry`；
- `root_id`、冻结 `root_binding_id`、可空 `asset_id`、可空 `file_id`、可空 `path_key`；
- `created_at`、`released_at`、`release_reason`。

owner CHECK 要求两族字段严格二选一：TaskAttempt owner 的 task_id/attempt_id/lease_generation 非空，cache_eviction_intent_id/cache_entry_id 为空，且 kind 不能为 CacheEntry；CacheEvictionIntent owner 的 eviction_intent_id/cache_entry_id 非空，Task/Attempt/lease 字段与 asset_id 为空，且 kind 必须为 CacheEntry。不得用全空或两族混填绕过 owner 校验。

部分唯一索引保证未释放 OperationReservation 中：

- `UNIQUE(root_id, path_key) WHERE released_at IS NULL AND path_key IS NOT NULL`；
- `UNIQUE(root_id, file_id) WHERE released_at IS NULL AND file_id IS NOT NULL`；
- `UNIQUE(attempt_id, reservation_kind, root_id, asset_id) WHERE owner_kind='TaskAttempt' AND released_at IS NULL AND asset_id IS NOT NULL`；没有 asset_id 的 Task 路径意图仍由前两个索引唯一；
- `UNIQUE(cache_eviction_intent_id) WHERE owner_kind='CacheEvictionIntent'`，保证一个淘汰 Intent 全历史恰有一个 OperationReservation。

每行至少具有 `asset_id | file_id | path_key` 之一；同一活动 OperationReservation 的非空身份字段必须可解释为同一个对象，不能把互不相干的 path/file/asset ID 拼在一行规避唯一索引。

TaskAttempt owner 必须引用当前非终态 Task、该 Task 的当前 Running Attempt 和相同 FileWriteLease generation。CacheEvictionIntent owner 不引用或取得 FileWriteLease；它必须与所引用 Intent 在同一事务创建，并以复合 FK、触发器或受测仓储保证 cache_entry_id、CacheRoot/root_binding_id、path_key、file_id 和冻结 FileIdentitySnapshot 全部等于 Intent 与 CacheEntry 的同一对象。前两个活动路径/file ID 唯一索引不区分 owner_kind，因此 Task 与缓存 owner 也不能同时保留同一物理对象。

CatalogService 是 Asset 观察与建档的唯一入口，扫描和导入均调用幂等 `observe_asset`。扫描遇到活动 OperationReservation 时暂缓协调对应文件；只忽略能由任务记录和身份匹配证明归属的 `.partial`。名称像 partial 但无归属证明的文件不得索引为照片、采用或删除，只能报告。Attempt 结果、报告及聚合状态提交并释放 OperationReservations 后，对相关路径执行定向重扫；COMMIT 前不得通知扫描协调这些路径。

Root、导入源或缓存位置变更前必须查询未终结 TaskIntent、`FileWriteLease.task_id IS NOT NULL` 和相关未释放 OperationReservation；任一存在时拒绝变更。固定 `slot=1` 的空 Lease 行本身不阻塞变更。每个任务始终使用创建时冻结的 RootBinding。

## 8. 导入 schema 与状态机

### 8.1 `ImportSourceProfile`

保存：

- `profile_id`、`library_id`、`source_root_id`；
- 内存卡 `volume_id`；
- 用户选择的源目录相对位置和 path key；
- 可用的目录 file ID、身份强度；
- `last_confirmed_at`、`retired_at`。

重新识别同一张卡只预填来源；不得因为 Profile 匹配而自动扫描。用户点击“开始只读预检”后才递归扫描所选子树，deny-write 仍覆盖整卷。

### 8.2 `ImportJob` 与 `ImportPlanGeneration`

点击“确认并导入……”只打开最终确认模态，不创建 TaskIntent、ImportJob 或任何计划行。只有用户在该最终确认框点击“开始导入”时，才在一个事务中创建 TaskIntent、ImportJob、稳定源项目以及第一个 Confirmed 计划代次。`ImportJob` 必需字段：

- `job_id`：PK，同时 FK 到 `TaskIntent.task_id`；
- `library_id`、`source_profile_id`、源 `volume_id`；
- `target_root_id`、冻结的 `target_binding_id`；
- `current_confirmed_generation_id`、可空 `proposed_generation_id`；
- 聚合 `job_state`、`row_version`、创建/完成时间。

`ImportPlanGeneration` 是追加的用户可见计划。除生命周期字段外，计划载荷一经创建即不可变：

- `generation_id`：PK，`job_id`：FK，`generation_no >= 1`，`UNIQUE(job_id, generation_no)`；
- `generation_state`：`Proposed | Confirmed | Superseded | Rejected`；
- 目标 `root_id + binding_id`、计划源文件数、计划复制文件数、`planned_new_bytes`；
- 生成原因：`InitialPreflight | DuplicateChanged | TargetChanged | NameFamilyChanged | RetryRefresh`；
- `created_at`、可空 `confirmed_at`、可空 `supersedes_generation_id`。

每个 Job 最多一个 `Proposed` 和一个 `Confirmed` 代次。唯一允许的生命周期更新是 `Proposed -> Confirmed | Rejected` 与 `Confirmed -> Superseded`；Superseded/Rejected 为不可变终态。用户确认 Proposed 时，在同一事务把旧 Confirmed 变为 `Superseded`、新代次变为 `Confirmed` 并更新 Job 指针。仅关闭确认界面不改变任何行；用户明确拒绝提案时只把 Proposed 变为 `Rejected`，不改变当前 Confirmed。等待确认期间再次刷新出不同提案时，必须在一个事务中 Rejected 旧 Proposed 并追加新 Proposed。目标、计数、字节、原因和代次号等计划载荷禁止 UPDATE；生成新代次只改变计划，不执行文件系统写入。

Job 聚合状态为：

```text
Prepared                 -> ValidatingPlan | Cancelled
ValidatingPlan           -> ReadyToStart
                         -> AwaitingPlanConfirmation
                         -> AwaitingSource
                         -> NeedsAttention
                         -> Cancelled
AwaitingPlanConfirmation -> ValidatingPlan | Cancelled
ReadyToStart             -> Running | AwaitingSpace | ValidatingPlan | Cancelled
AwaitingSpace            -> ValidatingPlan | Cancelled
Running                  -> CancelRequested
                         -> AwaitingSource
                         -> Completed
                         -> CompletedWithErrors
                         -> NeedsAttention
                         -> Cancelled
CancelRequested          -> Completed | CompletedWithErrors | NeedsAttention | Cancelled
AwaitingSource | CompletedWithErrors | NeedsAttention -> ValidatingPlan | Cancelled
```

`Completed` 与 `Cancelled` 为终态。`ValidatingPlan` 是只读执行准备，不持有文件写能力；任何回到它的路径都必须由用户显式继续、重新检查空间或重试触发。ImportSpaceCheck 不足时 Job 进入 `AwaitingSpace`，TaskIntent 映射为 `AwaitingUser`。

### 8.3 `ImportSourceItem`、`ImportGroup` 与 `ImportAssetStep`

`ImportSourceItem` 为一个 Job 中的稳定源文件身份，字段包括 `source_item_id PK`、`job_id`、源 FileIdentitySnapshot、原文件名/扩展名/源目录、角色、可靠拍摄日期及解析结果。`UNIQUE(job_id, source_root_id, source_path_key)`，并在 file ID 可用时额外 `UNIQUE(job_id, source_root_id, source_file_id)`；源身份变化不得覆盖原行，而是使当前执行准备进入 `AwaitingSource | NeedsAttention`。

`ImportGroup` 保存某一计划代次中的 JPG/RAF 逻辑组：`group_id PK`、`generation_id FK`、稳定 `group_key`、配对证据、目标日期策略、名称族键和统一后缀序号。`UNIQUE(generation_id, group_key)`；ImportAssetStep 以 `UNIQUE(group_id, role) WHERE group_id IS NOT NULL` 保证每组最多一个 Jpg 与一个 Raf 源项目。规划后缀前必须枚举目标目录中原名、`_2`、`_3` 等完整名称族并逐一作为重复候选；SQLite 历史只能加速枚举，不能排除磁盘候选。

`ImportAssetStep` 是代次内每个源项目的一行不可变计划：

- `step_id`：PK，`generation_id`、`source_item_id`：FK，可空 `group_id`，`role`；
- `plan_kind`：`Copy | SkipDuplicate | Unreadable`；
- Copy/SkipDuplicate 的目标 Root/Binding、最终相对路径/path key；Unreadable 时为空；
- Copy 计划的 `partial_root_id + partial_namespace + partial_seed` 与 `expected_bytes`；它们只定义应用专属 partial 命名空间和确定性输入，不冻结单个物理 partial path、nonce 或 incarnation ordinal；SkipDuplicate/Unreadable 时这些字段为空；
- 预检候选摘要与用户可见警告，但不保存可授权跳过的旧证明。

约束：`UNIQUE(generation_id, source_item_id)`、`UNIQUE(generation_id, final_root_id, final_path_key)`。group_id 必须属于同一 generation，Step 角色必须与 Group 中角色一致。物理 partial path 的全历史唯一性由 ImportPartialOwnership incarnation 保证，不属于 PlanGeneration 唯一键。Job 的所有代次中同一 `source_item_id` 最多形成一个 `ImportPublishedFact`。

已确认代次中的目标名称、组后缀、Copy 集合、expected_bytes 和源快照不可修改。变化只能生成新的 Proposed 代次并重新向用户展示；Attempt 绝不修改计划或静默采用未展示最终名称。同一 Confirmed generation 下，仅在旧 Ownership 已 Cleaned 后创建新的 partial ordinal/nonce/path 不改变最终分类、名称、路径或字节数，因此不生成新 PlanGeneration，也不重新要求用户确认。

JPG/RAF 日期和新增矩阵：

- 两端可靠且日期一致：同组、同日期目录；RAF 位于 `raw` 子目录；
- 仅一端日期可靠：只有 `SameImportGroup` 证据成立时整组使用该日期，否则分开；
- 两端可靠但日期冲突：不配对，分别按各自日期；
- 两端均无可靠日期：有导入组证据时同组进入 `Unknown-Date`，否则分别进入；
- 一端已由完整重复证明存在、另一端新增：只复制新增端，并用本次导入组证据与既有端配对；
- 同名不同内容时，同一组使用同一安全后缀。

EXIF 缺失或不可解析但内容可完整读取时不是 `Unreadable`，应导入 `Unknown-Date`。发布并验证内容后，尝试把目标 LastWriteTime 设为源记录值；设置失败不回滚内容正确的正式文件，只在报告记录警告。

### 8.4 执行计划刷新、唯一空间门与 `ImportAttempt`

每次初次执行、继续或重试都先创建不可变 `ImportPlanValidation`，对当前 Confirmed generation 完成只读刷新：

- 重新验证源卷、源项目、目标 Root/Binding；
- 重新生成每个 SkipDuplicate 的双端重复证明；
- 重新枚举实际完整名称族并复核 Copy/Skip 集合、统一后缀、最终路径与新增字节数；
- 保存 `validation_id PK`、`job_id`、`generation_id`、`validation_ordinal`、开始/完成时间和 `outcome`：`Ready | PlanChanged | AwaitingSource | NeedsAttention`，其中 `UNIQUE(job_id, validation_ordinal)`。

`ImportPlanValidationItem(validation_item_id PK, validation_id FK, step_id FK, disposition, remaining_new_bytes)` 使用 `UNIQUE(validation_id, step_id)`；disposition 为 `CopyNewBytes | SkipWithCurrentProof | ReverifyOwnedPartial | IndexPublished | AlreadyResolved | Unreadable`。`remaining_new_bytes` 只有 CopyNewBytes 可大于 0，ImportPlanValidation 的执行新增字节数必须由全部 Item 求和。ReverifyOwnedPartial 只能由仍为 Owned 的 ImportPartialOwnership、当前同身份 partial 和可读源推导，但不复用旧 VerifiedPartialSnapshot 作为发布授权；IndexPublished 和 AlreadyResolved 只能由已提交 ImportPublishedFact/ImportIndexedFact 或 Execution duplicate proof 推导，不能由路径存在猜测。

同一 Copy Step 的 Ownership 汇总必须确定性映射：未解决 Owned -> ReverifyOwnedPartial；Planned/CleaningOwnedPartial/NeedsAttention -> Validation.outcome=NeedsAttention，不创建新 Attempt；最新 incarnation 为 Cleaned 且没有其他未解决行 -> CopyNewBytes，并在新 Attempt 创建下一 ordinal；ConsumedByPublish -> AlreadyResolved。物理 incarnation 的变化不改变 Confirmed Plan。

文件系统读取与哈希期间不持有 SQLite 写事务；收集完成后，Validation、其全部 ValidationItem、Validation-scope DuplicateProof 与 PathBindingProof 在一个短事务中完整插入。COMMIT 后这些行禁止 UPDATE/DELETE；取消中的未完成读取不创建半份 Validation。每个 ImportDuplicateProof 必须与其唯一 ImportDuplicatePathBindingProof 在同一事务创建，延迟约束或提交前仓储校验保证可见 Proof 恰有一个子 Proof，而不是仅“至多一个”。

只要尚未解决 Step 的 Skip/Copy 分类、组后缀、最终路径或 expected_bytes 发生变化，就写入新的 Proposed ImportPlanGeneration，Job 进入 `AwaitingPlanConfirmation`；已提交 Published/Indexed Fact 或 VerifiedPartialSnapshot 使“本次剩余字节”减少只改变 ValidationItem，不算用户计划变更。旧 Ownership 已 Cleaned 后为同一 Step 分配新 partial incarnation 也不算计划变更。等待确认期间不检查空间、不创建 Attempt、Lease、OperationReservation、目录或 partial。只有 `outcome=Ready` 且未解决计划仍与当前 Confirmed generation 完全一致时才进入空间门。

`ImportSpaceCheck` 是不可变运行记录：`space_check_id PK`、`job_id`、`generation_id`、`validation_id UNIQUE`、从 ValidationItem 求和得到的 `planned_new_bytes`、目标卷总字节、可用字节、余量字节、`checked_at`、`outcome=Passed | Insufficient`。条件使用第 3.1 节常量和整数向上取整：

```text
reserve_bytes = max(IMPORT_MIN_RESERVE_BYTES, ceil(target_volume_bytes * 2 / 100))
available_bytes >= planned_new_bytes + reserve_bytes
```

一次检查只对应一次“开始/重试”请求。`Insufficient` 不创建 Attempt；用户稍后“重新检查”会先创建新的 ImportPlanValidation，再创建新的 ImportSpaceCheck。每个 `Passed` ImportSpaceCheck 最多被一个 ImportAttempt 使用。

ImportSpaceCheck 在读取已冻结 Ready Validation 后以单行插入，插入后禁止 UPDATE/DELETE；容量、结果或关联 Validation 变化只能产生新的 Validation 与 SpaceCheck，不能改写旧空间门。

空间门通过后，才在一个事务中创建 TaskAttempt、ImportAttempt、该代次全部 ImportAttemptStep，取得/核对全局 Lease 并建立本 Attempt OperationReservations。ImportAttempt 必需字段：

- `attempt_id`：PK/FK TaskAttempt，`job_id`、`generation_id`、可空且各自 `UNIQUE` 的 `validation_id`、`space_check_id`；
- `attempt_reason`：`Initial | Retry | Resume | Reconcile | CancelBeforeStart`；
- `attempt_state`、开始/结束时间；不保存 report_id，报告单向引用 Attempt。

`validation_id` 与 `space_check_id` 只在 `CancelBeforeStart` 时允许为空。用户在首个写 Attempt 之前从 Prepared/ValidatingPlan/ReadyToStart/AwaitingPlanConfirmation/AwaitingSpace/AwaitingSource/NeedsAttention 取消时，以一个事务创建无 Step、无 Lease/OperationReservation 的 CancelBeforeStart TaskAttempt/ImportAttempt，把其 OperationReport 直接 Finalized 为 Cancelled，并终结 Job/Task；这样预执行取消仍有可审计报告，但空间不足本身不会伪造一个写 Attempt。

除 CancelBeforeStart 外，约束或受测试的同事务仓储必须保证：ImportAttempt、ImportPlanValidation 与 ImportSpaceCheck 的 `job_id + generation_id` 完全相同，Validation.outcome=`Ready`、SpaceCheck.outcome=`Passed`，且 `ImportSpaceCheck.validation_id = ImportAttempt.validation_id`。不得把一个 Job/代次的 Ready Validation 与另一个 Job/代次或另一轮 Validation 的 SpaceCheck 拼接取得执行权。

Attempt 合法状态：

```text
Prepared -> Running
Prepared -> Cancelled | Interrupted
Running  -> Completed | CompletedWithErrors | AwaitingSource
         -> NeedsAttention | Cancelled | Interrupted
```

这些目标均为 ImportAttempt 终态，终态行禁止 UPDATE/DELETE。崩溃留下的 Prepared/Running ImportAttempt 由 Reconciler 根据已提交 Fact 与磁盘证据，在同一协调事务中合法终结为对应结果或 `Interrupted`，并最终化其报告；只有旧 Attempt 已终结后才能创建新的 `Reconcile` Attempt。不得把旧终态改回 Running。

部分唯一索引 `UNIQUE(job_id) WHERE attempt_state IN ('Prepared','Running')` 保证一个 Job 最多一个未终结 ImportAttempt；正常写 Attempt 在创建事务 COMMIT 前即从 Prepared 变为 Running，`CancelBeforeStart` 则在同一关闭事务中直接终结，外部不应观察到无故长期停留的 Prepared。

`ImportAttemptStep(attempt_step_id PK, attempt_id FK, step_id FK, step_state, error_code, warning_code, started_at, finished_at)` 使用 `UNIQUE(attempt_id, step_id)`。合法主路径：

```text
Planned -> Validating -> SkippedDuplicate
Planned -> Validating -> Copying -> PartialVerified -> Published -> Indexed
Planned -> ValidatingOwnedPartial -> PartialVerified -> Published -> Indexed
                                 -> CleaningOwnedPartial -> CleanedPartial
Planned -> ReconcilingPublished -> Indexed
Planned -> AlreadyResolved
Planned -> Unreadable
Published -> PublishedNeedsIndex
```

`ValidatingOwnedPartial` 只处理 ImportPlanValidationItem=`ReverifyOwnedPartial`，必须使用当前 Attempt 的 ImportPartialClaim 从头完整验证现有 ImportPartialOwnership、partial 与源，并创建属于当前 attempt_step 的新 VerifiedPartialSnapshot；旧 Snapshot 只作历史证据。`ReconcilingPublished` 只处理 ImportPlanValidationItem=`IndexPublished`，必须引用现有 ImportPublishedFact。`AlreadyResolved` 只对应同名 ValidationItem 且必须引用已有 Indexed 或 Execution SkippedDuplicate 事实；`Unreadable` 只对应同名 ValidationItem，记录稳定错误后不执行文件动作。发布前可以进入 `Cancelled | Failed | AwaitingSource | Conflict | PlanStale | NeedsAttention | Interrupted`。ImportAttempt 建立后 Copy 集合、计划字节和目标名称禁止扩大或改变：SkipDuplicate 证明失效只能进入 `PlanStale/NeedsAttention`，Copy 目标被抢占只能进入 `Conflict`；二者都必须结束当前 Attempt，再刷新计划、向用户重新确认并重新通过一次空间门。`PublishedNeedsIndex` 只能由新 Reconcile Attempt 调用 `observe_asset` 补索引，不能回到 Copying 或删除正式文件。

### 8.5 重复证明与不可回退事实

`ImportDuplicateProof` 分为不可混用的 Validation 与 Execution 两种 scope，公共字段必须保存：

- `duplicate_proof_id PK`、`proof_scope`：`Validation | Execution`、`step_id FK`；
- owner 字段：可空 `validation_id`、可空 `execution_attempt_step_id`；
- Execution 专用的可空 `lease_generation`、`source_read_operation_reservation_id`、`target_existing_operation_reservation_id`；
- 源、目标比较前后的 FileIdentitySnapshot、两端完整哈希算法/摘要、长度相同及逐字节相同结果；
- 目标 Asset 与证明时间；本行不保存反向 path_binding_proof_id。

owner CHECK 要求：Validation scope 只有 validation_id 非空，且 `UNIQUE(validation_id, step_id) WHERE proof_scope='Validation'`；Execution scope 只有 execution_attempt_step_id、lease_generation 和两项 OperationReservation 非空，且 `UNIQUE(execution_attempt_step_id) WHERE proof_scope='Execution'`。两种证明都必须使 owner、step_id、SourceItem、目标 Asset 和 Confirmed generation 同链；Execution proof 还必须绑定当前 Running Attempt、当前 FileWriteLease generation，以及属于该 Attempt 的 SourceRead/ExistingAsset OperationReservation。

`ImportDuplicatePathBindingProof(path_binding_proof_id PK, duplicate_proof_id UNIQUE FK, ...)` 是不可变的一对一行，也是两表之间唯一 FK 方向；ImportDuplicateProof 不保存反向 FK。单行必须同时保存 source 与 target 两端的 `root_id + binding_id + relative_path + path_key + path_key_version`、各自从路径重开句柄解析出的 volume/file ID、containment/reparse 结果以及比较前后 RootBinding 版本。它继承父 Proof 的 owner/scope，缺任一端即无效。

生成 Validation 或 Execution proof 时，源与目标的原比较句柄必须保持打开；完整哈希和逐字节比较结束后，分别按计划源路径与目标候选路径重新打开第二个句柄，并证明每个重开句柄解析到各自起始比较句柄的同一 RootBinding、path key、volume/file ID。两端路径证明与 DuplicateProof 在同一事务提交。源路径被替换/重定向、目标只在别处存在、任一端重开身份或 containment/reparse 不一致，都不能由内容相同替代该证明。

Validation proof 只决定计划中的 SkipDuplicate、ValidationItem 和空间字节，永不授权 ImportAttemptStep 进入 SkippedDuplicate。真正跳过前，worker 必须在当前 Attempt 内再次以稳定只读句柄对源与目标完成完整哈希、逐字节比较，并取得比较前后两端 Root/Binding/file ID/大小/mtime 与当前 path binding 证据；源/目标比较句柄及两端路径重开句柄必须继续拒绝共享写入/删除并保持到结果事务最终 COMMIT。随后在一个短事务中插入该 attempt_step 专属 Execution ImportDuplicateProof 及其 PathBindingProof，并把 AttemptStep 原子变为 SkippedDuplicate。事务提交前再次核对 Attempt、lease generation 和两项 OperationReservation；COMMIT 失败时不写跳过结论并停止该 Attempt。任一变化只能成为 `PlanStale/NeedsAttention`，不得复用 Validation proof、不得在该 Attempt 中扩展为 Copy。ImportHistory 不能代替任一种证明。

`ImportPublishedFact(published_fact_id PK, source_item_id UNIQUE, step_id UNIQUE, attempt_step_id UNIQUE, verified_partial_snapshot_id UNIQUE, publish_intent_id UNIQUE, target_snapshot_id, published_at)` 记录 no-replace rename 后的物理事实；只允许插入。`ImportIndexedFact(indexed_fact_id PK, published_fact_id UNIQUE, asset_id, indexed_at)` 记录重新观察与索引成功；只允许插入。数据库可以缓存查询汇总状态，但不得删除、替换或把上述事实清空。

`ImportHistory` 保存源身份/内容指纹与目标 Asset/ImportPublishedFact 的长期候选映射。恢复旧数据库后必须重新观察目标文件并生成当前 Validation proof；真正跳过时仍须生成当前 attempt_step 专属 Execution proof，历史行不能独立导致跳过。

### 8.6 partial 所有权

`ImportPartialOwnership` 是一次物理 partial incarnation，保存 `ownership_id PK`、`step_id FK ImportAssetStep`、`ownership_ordinal >= 1`、`created_by_attempt_step_id UNIQUE FK ImportAttemptStep`、随机 nonce、由 Step namespace/seed + ordinal + nonce 生成的实际 Root/Binding/relative path/path key、创建后 file ID、`created_size_observed` 和状态。约束为 `UNIQUE(step_id, ownership_ordinal)`、全历史 `UNIQUE(root_id, path_key)`；created_by_attempt_step_id 永久记录创建该 incarnation 的 AttemptStep，不因恢复接管而改写。

`file_id` 与 `created_size_observed` 在 `Planned` 时必须为空；`Owned | CleaningOwnedPartial | ConsumedByPublish` 时必须非空。未调用 CREATE_NEW 的 Planned 可直接进入 `Cleaned` 并继续保持为空；发生在 CREATE_NEW 与 Owned COMMIT 之间的 `NeedsAttention` 也允许为空。由 Owned 转入的 `Cleaned | NeedsAttention` 则必须保留原非空创建身份，不得清空。上述状态相关空值规则必须由 CHECK、触发器或受测试仓储强制。

CopyNewBytes Attempt 的创建事务必须先为该 Step 分配下一 ordinal/nonce/path，插入 `Planned` Ownership，并建立精确绑定该 path 的 PartialPath OperationReservation；Attempt/Lease/OperationReservation/Ownership 任一写入或 COMMIT 失败时都不得 CREATE_NEW。部分唯一索引 `UNIQUE(step_id) WHERE state IN ('Planned','Owned','CleaningOwnedPartial','NeedsAttention')` 保证同一 Step 最多一个未解决 incarnation。ConsumedByPublish 不再复制；NeedsAttention 未解决时禁止创建新 incarnation。

允许的状态为：

```text
Planned -> Owned | Cleaned | NeedsAttention
Owned   -> ConsumedByPublish | CleaningOwnedPartial | NeedsAttention
CleaningOwnedPartial -> Cleaned | NeedsAttention
```

CREATE_NEW 成功后立即从创建句柄冻结 Root/Binding/path/file ID；`created_size_observed` 只是创建时观察值，通常为 0，不是最终大小约束。把 Ownership 变为 Owned 与创建 `claim_reason=Created` 的 Claim 必须在同一短事务提交。若崩溃发生在 CREATE_NEW 与 file-ID/Owned COMMIT 之间，数据库保留 Planned 行与 OperationReservation，而磁盘文件没有已提交所有权证明；恢复时必须把该 incarnation 置为 NeedsAttention，不能仅凭计划 path/nonce 认领、删除，也不能绕过它创建新 incarnation。

只有 worker 能证明本 Attempt 尚未调用 CREATE_NEW 时，才可在正常取消/关闭事务把 Planned 直接置为 Cleaned 并释放 PartialPath OperationReservation；进程崩溃后不得作此推断，恢复看到 Planned 一律按上一段进入 NeedsAttention。

已证明 Owned 但内容不完整时，只有同时满足 Root/Binding/path/file ID 匹配、当前 Takeover Claim/Attempt/lease/OperationReservation 有效且来源/ownership 安全清理规则通过，才可准备清理。必须先在短事务把 Ownership 与当前 AttemptStep 变为 `CleaningOwnedPartial` 并 COMMIT，保持 Claim、Lease 与 OperationReservations；随后只通过已验证且仍打开的 partial 句柄删除，禁止按路径 remove。`ImportPartialCleanupFact(cleanup_fact_id PK, ownership_id UNIQUE FK, claim_id UNIQUE FK, attempt_step_id UNIQUE FK, deleted_snapshot_id, outcome, cleaned_at)` 只允许插入，outcome 为 `DeleteSucceeded | ObservedMissingAfterCleaningIntent`。删除结果、CleanupFact、Ownership=Cleaned、AttemptStep=CleanedPartial、Claim 释放和相关 OperationReservation 释放必须在一个事务原子提交。任一身份/删除/COMMIT 结果不明则保持 CleaningOwnedPartial 或进入 NeedsAttention，不得补记 Cleaned。崩溃恢复只协调已有 CleaningOwnedPartial 状态和同一冻结身份：文件仍在且身份完全匹配时可继续同一句柄级清理，文件缺失时只能记录 ObservedMissingAfterCleaningIntent，身份异常时进入 NeedsAttention。只有 Cleaned 后，后续新 Attempt 才可在同一 Confirmed generation 为同一 Step 创建下一 Ownership ordinal/nonce/path。

每次 Attempt 使用 Owned partial 都必须拥有不可变身份的 `ImportPartialClaim(claim_id PK, ownership_id FK, attempt_step_id UNIQUE FK, claim_reason, lease_generation, source_read_operation_reservation_id UNIQUE FK, partial_path_operation_reservation_id UNIQUE FK, final_path_operation_reservation_id UNIQUE FK, claimed_at, released_at, release_reason)`；`claim_reason` 只能为 `Created | Takeover`：

- 首次把 Ownership 变为 Owned 时，在同一事务创建 `claim_reason=Created` 的 Claim；
- 崩溃/中断后的新 Reconcile Attempt 使用 `claim_reason=Takeover`，显式引用同一 logical step 的既有 Ownership；
- 部分唯一索引 `UNIQUE(ownership_id) WHERE released_at IS NULL` 保证同一 partial 同时只有一个 Claim；
- Takeover 只允许在 created_by Attempt 及所有较早 Claim 的 Attempt 已终结、报告已 Finalized、旧 Claim 已释放后创建；当前 AttemptStep 必须属于同一 Job/generation/step_id，Attempt=`Running`，Claim 的三项 OperationReservation 必须属于当前 Attempt 与当前 FileWriteLease generation；
- Attempt 原子关闭时释放未消费 Claim。Ownership 仍可保持 Owned，供用户确认后的下一 Reconcile Attempt 创建新 Takeover Claim；旧 Attempt、旧 Claim 和旧 Snapshot 永不改写。

复制完成、`FlushFileBuffers` 成功并关闭写句柄后，或 Reconcile Attempt 取得 Takeover Claim 后，必须从目标卷重新打开 partial 和源文件完整读取。每个当前 Claim 都从头执行验证；成功时追加不可变 `VerifiedPartialSnapshot`：

- `verified_partial_snapshot_id`：PK，`ownership_id FK`、`claim_id UNIQUE FK`、`attempt_step_id UNIQUE FK`；Ownership 可有多个历史 Snapshot，但每个 attempt_step/Claim 最多一个；
- partial 的 FileIdentitySnapshot，以及验证前后的源 FileIdentitySnapshot；
- 长度、两端完整哈希算法/摘要、逐字节一致结果；
- Flush/关闭完成时间、验证时间；
- 设置 LastWriteTime 后重新取得的最终 partial FileIdentitySnapshot，以及时间保留结果/警告。

只有新 `VerifiedPartialSnapshot` 中源前后身份一致、内容完整相同，最终 partial 的 Root/Binding/path key/file ID 与 Ownership 创建时冻结身份相同，最终 size 等于 Step.expected_bytes 且内容等于冻结源，Claim 仍由当前 Running AttemptStep 和当前 lease generation 持有时，才允许该 AttemptStep 进入 `PartialVerified`。不得要求最终 size 等于 created_size_observed。发布前必须再次以句柄核对该新快照，并把其 ID 与 Claim 写入发布意图；no-replace rename 成功后的 ImportPublishedFact 必须唯一引用该 Snapshot。只保存复制循环累计哈希、复用旧 Attempt 的 Snapshot、只引用 Owned 行或只检查 partial 文件名都不能授权发布。

复合 FK、触发器或同事务仓储必须强制整条链同源：Ownership.step_id = Claim.AttemptStep.step_id；Snapshot.ownership_id/claim_id/attempt_step_id 必须等于同一 Claim 及其 AttemptStep；该 AttemptStep 必须属于 Ownership 原 Job/generation。ImportPublishedFact 的 source_item_id、step_id、attempt_step_id、verified_partial_snapshot_id 和目标快照必须沿当前 Claim 分别等于计划源、计划 Step、执行 Step、新验证快照和计划最终 Root/Binding/path。任一错配都不得发布或补记事实。

### 8.7 `ImportPublishIntent`

正式 rename 前必须先提交 `ImportPublishIntent(publish_intent_id PK, attempt_step_id UNIQUE FK, partial_claim_id UNIQUE FK, verified_partial_snapshot_id UNIQUE FK, final_path_operation_reservation_id UNIQUE FK, lease_generation, intent_state, created_at, finished_at)`。状态为：

```text
Prepared -> Publishing -> Published | NeedsAttention
```

Intent 创建事务必须验证 AttemptStep=`PartialVerified`，PartialClaim 仍由同一 AttemptStep 当前持有，新 VerifiedPartialSnapshot 属于同一 Claim/AttemptStep/Ownership，FinalPath OperationReservation 属于同一 Task/Attempt、同一 lease generation 且恰好等于该 Step 的计划最终 Root/Binding/path；这些关系由复合 FK、触发器或受测试仓储强制。Prepared COMMIT 后才允许再次核对句柄与栅栏并执行 no-replace rename。rename 成功后，必须在一个短事务中插入唯一 ImportPublishedFact、把 Intent/AttemptStep 变为 Published、Ownership 变为 ConsumedByPublish 并释放 Claim；若 COMMIT 失败则停止，Reconciler 只能以同一 Intent 和当前磁盘身份协调，不能创建第二个发布 Intent、改名重试或删除正式文件。Published/NeedsAttention 为不可变终态。

## 9. 待删除与回收站 schema

### 9.1 `DeletionMark` 与 `DeletionMarkAssetSnapshot`

`DeletionMark` 使用软释放保存审计历史；“未标记”在领域层等价于活动视图中没有记录：

- `mark_id`：PK；
- `photo_id`、冻结 `photo_version`；
- 创建 `operation_id`、`created_at`；
- 可空 `released_at`、释放 `operation_id`、`release_reason`。

部分唯一索引保证每个 Photo 最多一个 `released_at IS NULL` 的活动标记。

创建标记的同一事务必须为全部当前成员创建 `DeletionMarkAssetSnapshot`：

- `mark_id`、`asset_id`、冻结 `asset_version`、成员角色；
- 对应不可变 FileIdentitySnapshot；
- PK 为 `(mark_id, asset_id)`；同一标记角色唯一。

活动标记有效当且仅当：

- Photo 仍活动且 `photo_version` 相同；
- 当前成员集合与快照完全一致；
- 每个 Asset 仍活动、版本相同，且 Root、路径、file ID、大小和 mtime 匹配；
- hard link、Root 能力和可回收条件仍满足。

任一不一致只使标记进入“需要重新确认”，不得自动更新快照或把旧标记转移到新成员。用户取消后可创建全新的标记。标记事务和 OperationEvent 必须在 UI 前进前成功提交。

### 9.2 `DeletionBatch`、Photo 与 Asset 冻结

用户确认文件数、容量和回收站说明后，在一个事务中创建：

- `DeletionBatch`：`batch_id` PK（同时 FK TaskIntent）、Library、冻结 RootBinding、`batch_state`、总 Photo/文件数/字节数、确认 OperationId 和时间；
- `DeletionBatchPhoto`：`batch_photo_id PK`、Batch、Photo、冻结 photo_version、来源 `mark_id UNIQUE`、预期成员数和字节数，另有 `UNIQUE(batch_id, photo_id)`；
- `DeletionBatchAsset`：`batch_asset_id PK`、BatchPhoto、Asset、角色、冻结 asset_version、来源 `mark_asset_snapshot_id UNIQUE`、当前 resolution 状态，另有 `UNIQUE(batch_photo_id, asset_id)` 和 `UNIQUE(batch_photo_id, role)`；
- 初始 TaskAttempt、DeletionAttempt、每个待处理 BatchAsset 的 DeletionAttemptAsset、全部 RecycleAsset OperationReservations 和全局 FileWriteLease。

DeletionBatchAsset 只能通过 `INSERT ... SELECT` 引用并复制来源 `DeletionMarkAssetSnapshot` 已冻结的 AssetId、版本、角色和 FileIdentitySnapshotId；触发器或同事务仓储必须验证复制值逐项相等。不得在批次创建时从当前 Asset 或文件系统生成一份“更新后的独立快照”，也不得借批次静默刷新旧标记。一个 mark_id 全历史只能被一个 BatchPhoto 消费；进入 Batch 后只允许沿该 Batch 新建 Attempt 或解决剩余意图，不能再次进入新 Batch。

BatchPhoto/Asset 的计划字段在确认后不可更新或删除。批次只包含确认时仍有效的全部活动标记，不提供部分勾选。Batch、初始 Attempt、快照引用、OperationReservations 和 Lease 任一写入或最终 COMMIT 失败时全部不成立，不能调用 Shell。Batch 创建成功后清空内存中的本会话 `R` 撤销栈；撤销栈不写入数据库。

Batch 聚合状态：

```text
Prepared -> Running
Running  -> CancelRequested | Completed | CompletedWithErrors | NeedsAttention | Cancelled
CancelRequested -> Completed | CompletedWithErrors | NeedsAttention | Cancelled
CompletedWithErrors | NeedsAttention -> Running | Cancelled
```

最后一行必须由用户显式重试并创建新 Attempt 后发生。`Completed` 与 `Cancelled` 为终态；存在任何未解决 BatchAsset 时不得进入 Completed。

### 9.3 `DeletionAttempt` 与逐 Asset 结果

初次执行和每次显式重试都创建新 `DeletionAttempt`：

- `attempt_id`：PK/FK TaskAttempt，`batch_id`；
- `attempt_reason`：`Initial | Retry | Reconcile`；
- `attempt_state`、开始/结束时间；不保存 report_id，报告单向引用 Attempt。

Attempt 合法状态：

```text
Prepared -> Running
Prepared -> Cancelled | Interrupted
Running  -> Completed | CompletedWithErrors | NeedsAttention | Cancelled | Interrupted
```

所有目标状态均为终态，终态行禁止 UPDATE/DELETE。崩溃后的 Prepared/Running Attempt 必须先由 Reconciler 根据已提交逐 Asset 结果合法终结并最终化报告，才能创建新的 Reconcile Attempt；旧回调不得写入新 Attempt。

部分唯一索引 `UNIQUE(batch_id) WHERE attempt_state IN ('Prepared','Running')` 保证一个 Batch 最多一个未终结 DeletionAttempt；只有第 13.5 节关闭事务成功后才允许建立下一 Attempt。

`DeletionAttemptAsset` 每次只引用仍待处理的 BatchAsset，保存：

- `attempt_asset_id PK`、`attempt_id`、`batch_asset_id`，且 `UNIQUE(attempt_id, batch_asset_id)`；
- 状态 `Prepared | Validating | Recycling | Recycled | RetryableFailed | NeedsAttention | Cancelled | Interrupted`；
- 最接近 Shell 调用时重新取得的身份快照与校验结果；
- `hrDelete`、`PerformOperations`、`GetAnyOperationsAborted` 结果；
- `psiNewlyCreated` 是否非空及可持久化的回收对象证据；
- 调用后原路径/身份观察、错误分类和提交时间。

状态主路径：

```text
Prepared -> Validating -> Recycling -> Recycled
                                  -> RetryableFailed
                                  -> NeedsAttention
Prepared | Validating -> Cancelled | Interrupted
Recycling             -> Interrupted
```

Attempt 终态后不可更新或删除；重试写新 Attempt。`DeletionRecycledFact(recycled_fact_id PK, batch_asset_id UNIQUE, attempt_asset_id UNIQUE, recycled_at, evidence_snapshot)` 只允许插入，且只在 `hrDelete` 成功、`psiNewlyCreated` 非空、原路径不存在、回收对象证据与冻结 mark snapshot 可解释一致并成功提交逐文件结果后创建。复合 FK、触发器或同事务仓储必须保证 `DeletionAttemptAsset.batch_asset_id = DeletionRecycledFact.batch_asset_id`，并保证该 Attempt 与 BatchAsset 属于同一 DeletionBatch；不能用另一个 Asset/Batch 的成功回调拼接事实。它永不删除或改写。

批次 Asset 的查询用 resolution 状态可以为：

```text
Pending | RetryableFailed | NeedsAttention | Recycled | CancelledRemaining | ResolvedWithoutDelete
```

合法变化：

- `Pending/RetryableFailed -> Recycled | RetryableFailed | NeedsAttention | CancelledRemaining`；
- `NeedsAttention -> Recycled | RetryableFailed | NeedsAttention | CancelledRemaining | ResolvedWithoutDelete`；只有重新扫描证明原身份仍在且用户显式重试后，才可由新 Attempt 得到前三种执行结果；无法证明时只能 `ResolvedWithoutDelete`，并保留原报告；
- `Recycled` 不可离开；
- `CancelledRemaining` 和 `ResolvedWithoutDelete` 不触发文件动作。

Photo 级 `Partial` 由成员中同时存在 `Recycled` 和非 `Recycled` 推导，不单独覆盖逐文件事实。全部成员 Recycled 后释放活动 DeletionMark；取消剩余意图时也释放标记，但 Batch、Attempt、快照和报告长期保留。

### 9.4 Shell 临界窗口与协调

在 `PreDeleteItem` 或最接近 Shell 操作的位置再次比较 BatchAsset 快照。`PostDeleteItem` 后逐 Asset 提交结果；提交失败立即停止整个写任务。普通失败且原身份文件仍在可成为 `RetryableFailed`；以下情况必须成为 `NeedsAttention` 并停止后续删除：

- Shell 报成功但 `psiNewlyCreated` 为空；
- 原路径消失但缺少完整回收证据；
- 结果矛盾或数据库无法提交；
- 原路径出现不同文件身份；
- 最终校验与 Shell 调用之间观察到替换。

源路径消失本身永远不能补记 Recycled。已记录 Recycled 后文件重新出现，Catalog 将其视为用户从 Windows 回收站恢复：按可靠身份恢复或创建正常 Asset/Photo，但不恢复旧 DeletionMark；旧 DeletionRecycledFact 保持不变。

## 10. 操作报告

`ScanRun(scan_run_id PK, library_id, root_id, started_at, finished_at, outcome)` 标识一次只读扫描。它只保存运行边界；逐类计数、跳过 reparse point 和错误明细写入对应 OperationReport。重复扫描创建新 ScanRun，不覆盖旧报告。

### 10.1 `OperationReport`

报告类型为 `Scan | Import | Recycle`，字段包括：

- `report_id`：PK、类型、Library/Root；
- 可空 `scan_run_id UNIQUE FK`、可空 `task_attempt_id UNIQUE FK TaskAttempt`；
- `report_state`：`Open | Finalized`；
- 开始/完成时间、稳定摘要计数和总字节数；完成时间在 Open 时可空、Finalized 时非空；
- 可空整体结果 `Success | Partial | Failed | NeedsAttention | Cancelled`；Open 时必须为空，Finalized 时必须非空；
- 可空 `supersedes_report_id`，只表示后续报告关系，不覆盖旧报告。

所有权 CHECK 保证 Scan 报告恰有 scan_run_id 且没有 task_attempt_id，Import/Recycle 报告恰有类型一致的 task_attempt_id 且没有 scan_run_id。Attempt/ScanRun 不保存反向 report_id；报告是唯一 FK 方向，避免双向字段不一致。`UNIQUE` 保证至多一份；Attempt/ScanRun 创建事务的延迟约束、触发器或提交前仓储校验进一步保证每个 Attempt/ScanRun 恰有一份类型正确的 Report，不能提交无报告的 owner。普通 Attempt 初始为 Open；CancelBeforeStart 可以在同一事务内创建后直接 Finalized。

Attempt 终态与报告结果使用以下确定映射；逐项事实决定 `Success/Partial/Failed`，不能由调用返回值猜测：

| Attempt 终态 | 报告结果 |
|---|---|
| `Completed` 且全部计划项具有相应 Skipped/Indexed/Recycled/Resolved 事实 | `Success` |
| `CompletedWithErrors` 且至少一个项目已 Skipped/Published/Recycled | `Partial` |
| `CompletedWithErrors` 且没有成功事实、也没有不确定证据 | `Failed` |
| `AwaitingSource` | 按已有成功事实显示 `Partial` 或 `Failed`；Task/Job 另行保持 `AwaitingSource` |
| `NeedsAttention` 或 `Interrupted` | `NeedsAttention` |
| `Cancelled` | `Cancelled`；摘要仍必须列出取消前已发生的不可回退事实 |

`OperationReportItem(report_item_id PK, report_id FK, item_kind, stable_item_id, result_code, warning_code, display_path_snapshot, size_bytes, user_parameters)` 使用 `UNIQUE(report_id, item_kind, stable_item_id)`。一个报告若同时需要 Photo 汇总与 Asset 明细，使用不同 item_kind；内部 RootId、LibraryId 等可以存在数据库，但报告 UI 不要求展示。

运行期间可以向 Open 报告追加 Item 和更新汇总；一旦 Finalized，报告及 Item 禁止 UPDATE/DELETE。ImportAttempt/DeletionAttempt 子行进入终态与其 Report 最终化必须在同一原子关闭事务中完成，不能出现“终态子类型 Attempt + Open Report”或“Running Attempt + Finalized Report”。后续重试生成新 Attempt 和新报告；Job/Batch 详情按同一 TaskIntent 的 `attempt_ordinal` 聚合，不把多个报告覆盖成一行。

`OperationResolutionEvent` 是追加且幂等的后续事实：

- `resolution_event_id`：PK，`operation_id UNIQUE FK OperationEvent`；
- `report_id FK`、`task_id FK`，以及按类型可空的 `batch_asset_id/source_item_id/asset_id`；
- `event_kind`：`RescanConfirmed | RetryStarted | TaskCancelled | CancelledRemaining | ResolvedWithoutDelete | RestoredFromRecycleBin`；其中 TaskCancelled 只用于终结等待中的 Import，CancelledRemaining 只用于终结仍有未回收成员的 DeletionBatch；
- 事件前后的 resolution 摘要、证据快照、`created_at`。

report_id 必须指向某个 Import/Recycle Report，且该 Report 的 TaskAttempt.task_id 必须等于事件 task_id；所有非空 subject ID 都必须属于同一 Task/Library。类型 CHECK/触发器/受测仓储还必须保证：RetryStarted 不带 subject；RescanConfirmed 恰带一个与 Task kind 相容的 subject；TaskCancelled 只用于 Import 且不带 subject；CancelledRemaining 只用于 Recycle 且不带 subject；ResolvedWithoutDelete 只用于 Recycle 且必须带 batch_asset_id；RestoredFromRecycleBin 只用于 Recycle 且必须带同一恢复链的 batch_asset_id 与 asset_id。相同 OperationId 与相同请求摘要返回原事件；摘要不同必须拒绝。ResolutionEvent 禁止 UPDATE/DELETE，只能记录问题已重扫、已取消剩余意图、已从回收站恢复等后续事实，不能把旧失败报告编辑成成功，也不提供新的文件写授权。

导入历史、删除报告和关键操作事件长期保留。报告只用于说明与恢复入口，不提供新的文件写授权。

## 11. 缓存所有权

### 11.1 `CachePolicy` 与 `CacheRootOwnership`

`CachePolicy` 保存当前 CacheRoot、容量上限和确认时间：

- 默认 `CACHE_DEFAULT_BYTES`；
- 必须大于 0；
- 绝对上限 `CACHE_MAX_BYTES`；
- 不预分配；
- 所在卷始终保留 `max(CACHE_MIN_RESERVE_BYTES, ceil(卷容量 * 10 / 100))`。

用户选择的是缓存父目录。应用在其中创建稳定、专属、含 CacheRootId 的子目录；`CacheRootOwnership` 保存：

- `cache_root_id`（PK/FK Root）；该 Root 的唯一活动 RootBinding 只绑定应用专属子目录；
- `parent_directory_snapshot_id UNIQUE`，引用选择时创建的不可变 `DirectoryIdentitySnapshot`；
- `child_root_binding_id UNIQUE`，必须等于 CacheRoot.current_binding_id；
- 随机 ownership nonce、专属目录 file ID、创建时间；
- `ownership_state`：`Active | Draining | Offline | IdentityMismatch | Retired`。

`DirectoryIdentitySnapshot` 保存所选父目录的 volume_id、规范绝对路径展示值/path key、目录 file ID、文件系统、身份强度、containment/reparse 结果和 captured_at。它是选择证据，不是 RootBinding，不授予在父目录递归删除或写任意文件的能力。缓存创建能力只从专属子目录的 CacheRoot Binding 解析，因此不违反“一个 Root 一个活动 Binding”。

位置切换的已确认 OperationId 只授予一次性配置动作。任何目录创建前，先提交 `CacheRootCreateIntent(create_intent_id PK, operation_id UNIQUE FK OperationEvent, parent_directory_snapshot_id FK, planned_cache_root_id UNIQUE, planned_child_name_key, planned_child_path_key UNIQUE, ownership_nonce, intent_state, observed_child_snapshot_id, created_at, finished_at)`；冻结的子目录名称必须由 CacheRootId 与随机 nonce 构成，部分唯一索引 `UNIQUE(1) WHERE intent_state='Prepared'` 保证同一时刻至多一个待执行 Intent。合法状态为 `Prepared -> Bound | NeedsAttention | Cancelled`，终态禁止 UPDATE/DELETE。

Intent 提交后重新验证父 DirectoryIdentitySnapshot，才可在该父目录直接创建计划中的唯一专属子目录；不能创建其他层级、改名重试或采用同名未知目录。创建并取得子目录 file ID 后，必须在一个事务中建立 CacheRoot、唯一活动 Binding 和 Ownership，并把 Intent 变为 `Bound`；若目录创建后数据库 COMMIT 失败，该目录保持未知且不得自动删除或认领，Reconciler 只能追加观察证据并把 Intent 终结为 `NeedsAttention`。后续缓存创建/淘汰只从子目录 Binding 解析，不再使用父快照作为写授权。

只能有一个 Active CacheRoot。更改位置时先创建并验证新专属目录，再以单个 SQLite 事务切换 Active；旧 Root 进入 Draining，保留所有权记录并按 LRU 渐进回收。任务运行中禁止切换。取消设置确认时数据库不发生改变。

CacheRoot 的父目录和专属目录均不得位于 FujiLibrary、ImportSource 或数据库目录内部，也不得反向包含它们，不得经过 reparse point。ImportSource 卷不能成为 CacheRoot 卷。

### 11.2 `CacheEntry`

必需字段：

- `cache_entry_id PK`、`cache_root_id`、同 render key 内从 1 递增的 `entry_ordinal`、随机 ownership nonce；
- 来源 `asset_id`、`asset_version`、缓存种类和渲染参数键；
- 含 nonce 的全历史唯一相对路径/path key；
- 创建后取得的 file ID、大小、创建与最后访问时间；
- `entry_state`：`Live | Missing | Evicted | NeedsAttention`。

CacheEntry 表示一次物理文件 incarnation，不复用旧 entry_id。全历史约束为 `UNIQUE(cache_root_id, path_key)` 与 `UNIQUE(cache_root_id, asset_id, asset_version, cache_kind, render_key, entry_ordinal)`；当前文件身份使用 `UNIQUE(cache_root_id, file_id) WHERE entry_state IN ('Live','NeedsAttention') AND file_id IS NOT NULL`。同 render key 的“当前 incarnation”使用 `UNIQUE(cache_root_id, asset_id, asset_version, cache_kind, render_key) WHERE entry_state IN ('Live','NeedsAttention')`，其中必须包含 cache_root_id，使 Active 与 Draining Root 可各自保存同一源版本。

`Live -> Missing | Evicted | NeedsAttention`；Missing/Evicted/NeedsAttention 对该 incarnation 均为终态。只有旧 incarnation 已可靠进入 Missing 或 Evicted，才可用 `CREATE_NEW`、新 entry_id、递增 ordinal 和新 nonce/path 创建下一 incarnation；它以后拥有自己的唯一 CacheEvictionIntent。旧 Entry/Intent/Fact 永不改写。NeedsAttention 或仍 Live 的旧 incarnation 必须继续占用当前 render key，禁止通过静默新建绕过异常。

LRU 自动淘汰只有在以下条件全部满足时才允许删除缓存文件：

- 路径位于已验证的应用专属子目录；
- CacheRoot 的卷和目录身份匹配；
- CacheEntry 的 path key、file ID 和大小匹配；
- 除本 CacheEvictionIntent 自己的 cache-owned OperationReservation 外，该路径不与任何其他活动 OperationReservation 冲突。

父目录中的其他文件、专属目录中数据库无记录的未知文件、身份不匹配文件均不得递归删除。数据库回退造成的新缓存孤儿可以保留并报告，不能为了回收空间降低所有权证明。缓存不提供手动清理入口。

### 11.3 `CacheEvictionIntent`

自动 LRU 每删除一个缓存文件前，先在短事务中插入 `CacheEvictionIntent`：

- `eviction_intent_id`：PK，内部生成的 `operation_id UNIQUE FK OperationEvent`；
- `cache_entry_id UNIQUE`、`cache_root_id`、冻结 `root_binding_id`；同一 CacheEntry 全历史只创建一个 EvictionIntent，不通过新 ordinal 重试；
- 删除前 FileIdentitySnapshot、计划原因、`intent_state`、创建/完成时间。

Intent 与 owner_kind=CacheEvictionIntent 的唯一 CacheEntry OperationReservation 必须在同一事务创建并通过 total-participation 校验；任一行或 COMMIT 失败都不获得删除资格。该 OperationReservation 不要求、占用或伪造 FileWriteLease，但必须冻结同一 CacheEntry/CacheRoot/Binding/path/file 身份，并与 Task owner 共用活动 path/file ID 唯一索引。

状态为：

```text
Prepared    -> Deleting | AlreadyMissing | AwaitingRoot | NeedsAttention
Deleting    -> Evicted | AlreadyMissing | AwaitingRoot | NeedsAttention
AwaitingRoot -> Deleting | AlreadyMissing | NeedsAttention
```

Prepared/AwaitingRoot/Deleting 为非终态，Evicted/AlreadyMissing/NeedsAttention 为终态。Intent 提交后才允许重新验证专属目录 containment、Root/Binding、path key、file ID、大小和 OperationReservation；文件动作前还必须确认该 OperationReservation 仍未释放且 owner 正是本 Intent，并先以短事务把 Prepared/AwaitingRoot CAS 为 Deleting。该状态 COMMIT 后才调用删除 API。`CacheEvictionFact(eviction_fact_id PK, eviction_intent_id UNIQUE, cache_entry_id UNIQUE, outcome, observed_at)` 只允许插入且禁止 UPDATE/DELETE；复合 FK、触发器或受测仓储必须保证 Fact.cache_entry_id 等于其 Intent.cache_entry_id。outcome 为 `DeleteSucceeded | ObservedMissingAfterIntent`。

删除结果使用一个短事务原子提交 Fact、Intent 终态、Entry 状态和 OperationReservation 释放：本次删除调用成功且原路径不存在时为 `DeleteSucceeded + Evicted + Entry.Evicted`；崩溃恢复只能观察到文件不存在时为 `ObservedMissingAfterIntent + AlreadyMissing + Entry.Missing`，不能伪称本应用已删除。身份不一致、未知文件、明确失败或结果不明时不写 Fact，而在同一事务把 Intent 与 Entry 置为 NeedsAttention、释放 OperationReservation 并停止本轮淘汰；COMMIT 失败则保留原非终态 Intent/OperationReservation 并停止。

Reconciler 只协调已有非终态 Intent 和同一条活动 OperationReservation：进程崩溃后继续原 Intent；根目录或卷临时离线时进入 AwaitingRoot，可靠的同一 Root/Binding 身份恢复后仍沿原 Intent 继续；文件缺失时按 ObservedMissingAfterIntent 关闭。卷/Root/Binding、path/file ID 或 containment 身份异常时原子进入 NeedsAttention 并释放 OperationReservation；NeedsAttention 禁止自动重试，也不得为同一 Entry 新建 Intent/ordinal。绝不认领或删除数据库无 Intent/Entry/OperationReservation 的文件。

## 12. 外键、唯一约束与删除策略总表

以下语义必须在迁移中由 FK、`CHECK`、普通/部分唯一索引、触发器或受测试的单写仓储共同实现：

| 不变量 | 实现要求 |
|---|---|
| Library/Root/Asset/Photo/任务引用不悬空 | `foreign_keys=ON`，核心 FK 使用 `ON DELETE RESTRICT` |
| 首次绑定后 v1 恰有一个当前 Fuji Library | `CurrentLibrary(slot=1)` + Library lifecycle 同事务校验；未知 Root 创建新 Library，不复用旧 library_id |
| 一个 Library 最多一个活动 FujiRoot | Root 部分唯一索引 |
| 一个 Root 一个活动 Binding | RootBinding 部分唯一索引 |
| 强卷身份不能重复或绕过策略 | `(identity_format_version, strong_identity_key)` 部分唯一；VolumePolicy 插入后禁止 UPDATE/DELETE |
| 活动 Asset 路径与文件身份唯一 | 当前 path key、可用 file ID 的部分唯一索引 |
| 一个 Asset 只属于一个当前 Photo | PhotoAsset 部分唯一索引 |
| 一个 Photo 最多一个 JPG、一个 RAF | `(photo_id, role)` 当前关系部分唯一索引 |
| PhotoAlias 不成环、不成链 | 触发器或同事务 Rust 校验；破坏性仓储拒绝 Alias |
| 每个 Photo 一个活动 DeletionMark | DeletionMark 部分唯一索引 |
| 一个标记最多进入一个删除批次 | `DeletionBatchPhoto.mark_id UNIQUE`；BatchAsset 只能引用该 mark 的成员快照 |
| OperationId 幂等 | OperationEvent PK；请求摘要必须匹配 |
| 同时只有一个未终结文件任务 | TaskIntent 非终态常量部分唯一索引 + FileWriteLease 固定主键 `slot=1` 与 generation 栅栏 |
| Task/Attempt 子类型正确 | TaskIntent/TaskAttempt 与同 ID Import 或 Recycle 子行恰好一对一，kind 必须一致 |
| 活动路径/文件不被跨 owner 占用 | OperationReservation owner 二选一 CHECK；活动 `(root,path)`/`(root,file)` 唯一索引同时覆盖 TaskAttempt 与 CacheEvictionIntent owner |
| Import 计划、刷新与空间门唯一 | generation no、group key、generation/source item、final path、validation ordinal、validation/step、SpaceCheck/Validation 的唯一索引；Validation/SpaceCheck/Attempt 同 Job/generation 且 Passed SpaceCheck 与 Attempt 一对一 |
| Duplicate proof scope/路径不混用 | Validation proof 按 `(validation,step)` 唯一；Execution proof 按 attempt_step 唯一并绑定当前 lease 与 SourceRead/ExistingAsset OperationReservation；两者各自恰有一个同时包含 source+target 的 PathBindingProof |
| Attempt 子项不重复 | `ImportAttemptStep(attempt_id,step_id)`、`DeletionAttemptAsset(attempt_id,batch_asset_id)` 唯一 |
| 删除批次成员不重复 | Batch 内 Photo、BatchPhoto 内 Asset/role 唯一；mark snapshot FK 唯一 |
| 每次重试保留历史 | TaskAttempt 新 ID/ordinal；终态子行禁止更新/删除 |
| Published/Recycled 不回退 | ImportPublishedFact、ImportIndexedFact、DeletionRecycledFact 的 UNIQUE + 禁止 UPDATE/DELETE；Fact 与其 AttemptStep/BatchAsset 必须同链 |
| partial incarnation/接管不复用旧证明 | Ownership `(step,ordinal)` 与全历史 `(root,path)` 唯一，同 step 仅一个未解决 incarnation；Ownership 保留 created_by，同一 Ownership 仅一个活动 Claim；每 attempt_step 一个新 Snapshot；Cleanup/Publish 链与当前 Attempt/lease/OperationReservation 同链 |
| 一个 Attempt 一个单向报告 | `OperationReport.task_attempt_id UNIQUE` + owner 创建事务 total-participation 校验；Attempt 不保存 report_id |
| 最终报告与解决事件不可改写 | Finalized 后触发器拒绝 Report/Item UPDATE/DELETE；ResolutionEvent.OperationId 唯一、Report/Task/subject 同链且事件只追加 |
| 缓存只删自有 incarnation | CacheRoot 的唯一 Binding 指向专属子目录；父目录只用 DirectoryIdentitySnapshot；CacheEntry path 全历史唯一，render key 只对 Live/NeedsAttention incarnation 唯一 |
| 缓存专属目录先有意图 | CacheRootCreateIntent 在 mkdir 前提交；计划 CacheRoot、OperationId 唯一，且最多一个 Prepared Intent |
| 缓存淘汰先有意图 | 每 Entry 一个 CacheEvictionIntent，与唯一 cache-owner OperationReservation 同事务创建；Fact/Intent/Entry/OperationReservation 同对象且原子关闭，不依赖 FileWriteLease |

硬删除只允许用于尚未提交的事务内临时行，或经证明可重建且不被报告/任务引用的派生索引。活动标记使用软释放；Root、Asset、Photo、Attempt、Fact、Report、OperationEvent 和 RootBinding 均不级联删除。

## 13. 事务边界

### 13.1 Catalog 观察

一个 `observe_asset` 事务完成：

1. 按 Root/file ID/path key 查找候选并拒绝歧义；
2. 插入或追加 AssetRevision；
3. 更新 Asset.current_version；
4. 协调 PhotoAsset；
5. 递增受影响 Photo.photo_version；
6. 使依赖旧版本的 DeletionMark 在查询时呈现无效；
7. 提交后再通知 UI。

同一观察重复执行必须幂等。若发生唯一约束冲突，重新读取并协调，不得覆盖另一 Asset。

### 13.2 标记

一次 D/R 数据库动作在单一短事务中提交 OperationEvent、DeletionMark/释放动作、全部成员快照及必要操作日志。COMMIT 失败时 UI 和游标都不前进。

### 13.3 Import

1. 点击“确认并导入……”只开最终确认模态且零持久任务；只有用户在该模态点击“开始导入”时，才原子提交 TaskIntent、Job、SourceItem、第一代 Confirmed Plan、Group 和 Step；不创建目录、partial、Attempt 或 Lease。
2. 每次开始/继续/重试先执行只读 ImportPlanValidation。结果变化时只追加 Proposed ImportPlanGeneration 并返回用户确认；`Ready` 才能继续。
3. 对 Ready generation 执行本次请求唯一一次 ImportSpaceCheck；不足不创建 Attempt。通过后在一个短事务中创建 TaskAttempt/ImportAttempt/ImportAttemptStep/Open OperationReport，取得或刷新 FileWriteLease generation，并建立 OperationReservations。每个 CopyNewBytes 项同时创建下一 ordinal/nonce/path 的 Planned ImportPartialOwnership 与精确 PartialPath OperationReservation；ReverifyOwnedPartial 项在确认旧 Attempt/Claim 均已关闭后创建当前 Takeover Claim。COMMIT 前把 Attempt/Job/Task 变为 Running/Running/Active。
4. 每次文件系统动作前提交相应 Intent；动作完成后单独提交该 Step 的结果或不可回退 Fact。每次动作前还要核对 Task/Attempt 仍活动、Lease generation 和 OperationReservation 属于本 Attempt。CopyNewBytes 只能对本 Attempt 的 Planned Ownership path 调用一次 CREATE_NEW；成功后立即从仍打开的创建句柄取得身份，并原子提交 Ownership=Owned 与 Created Claim。SkipDuplicate 在真正跳过前必须重新完成双端比较，并把 Execution DuplicateProof、PathBindingProof 与 SkippedDuplicate Step 状态原子提交。
5. 初次 Copy 或接管 partial 都必须在当前 ImportPartialClaim 下从头完整验证；只有最终文件身份仍等于 Ownership 创建身份、size 等于 expected_bytes、内容等于冻结源并提交当前 attempt_step 的新 VerifiedPartialSnapshot，才可进入 PartialVerified。Owned 但不完整时必须先 COMMIT CleaningOwnedPartial，再按第 8.6 节用已验证句柄清理；Cleaned 后由下一 Attempt 创建新 incarnation。验证成功后先提交绑定 Claim、新快照、FinalPath OperationReservation 与当前 lease generation 的 ImportPublishIntent，才可使用 no-replace rename。正式发布后即使索引事务失败，也只补索引，不删除或重新复制正式文件。
6. 任何结果 COMMIT 失败立即停止，不处理下一文件；重启后 Reconciler 观察磁盘事实。
7. 长哈希、逐字节比较、复制、Flush、设置时间和 rename 期间不持有 SQLite 写事务。
8. Attempt 开始后任何过期 skip、名称抢占或新增容量变化只产生 PlanStale/Conflict/NeedsAttention，不修改本 Attempt 的计划。

### 13.4 Recycle

1. 确认时以单一事务创建 TaskIntent、DeletionBatch/DeletionBatchPhoto/DeletionBatchAsset、初始 TaskAttempt/DeletionAttempt/DeletionAttemptAsset/Open OperationReport、OperationReservations 和 FileWriteLease；DeletionBatchAsset 只复制有效 DeletionMarkAssetSnapshot，并在 COMMIT 前把 Attempt/Batch/Task 变为 Running/Running/Active。事务失败时不清空 R 栈、不调用 Shell。
2. COMMIT 后逐 Photo 重新打开句柄校验；每个 Photo 使用独立 IFileOperation。调用前核对 Task/Attempt、Lease generation、OperationReservation 和冻结 mark snapshot，不在 Shell 调用期间持有 SQLite 写事务。
3. 每个 PostDeleteItem 结果立即提交；提交失败或 Critical 结果停止批次。旧 callback 必须同时匹配 attempt_id 与 lease generation，否则拒绝。
4. 重试在一个事务中创建新 TaskAttempt/DeletionAttempt/DeletionAttemptAsset/Open OperationReport、递增 FileWriteLease generation 并建立新 OperationReservations，只选择仍在原位且身份匹配的未成功成员，并在同一 COMMIT 前把 DeletionAttempt/Batch/Task 设为 Running/Running/Active；任一失败均不获得执行权。
5. 任何 DeletionRecycledFact 都先于下一 Photo；部分成功不得被 Batch 聚合态覆盖。

### 13.5 Attempt、Task、报告与锁的原子关闭

每个 ImportAttempt 或 DeletionAttempt 停止处理后，使用一个短事务按以下顺序关闭：

1. 重新读取该 Attempt 的全部 Step/Asset 结果和不可回退 Fact，拒绝缺项、矛盾或陈旧 lease generation；无 Step 且从未取得 Lease 的 CancelBeforeStart 明确跳过 lease/reservation 校验，但仍验证 Task/Job、OperationId 与“尚无写 Attempt/文件事实”的条件。
2. 追加尚缺的稳定 ReportItem，计算第 10.1 节规定的报告结果并把 Open Report 变为 Finalized。
3. 把 Attempt 设置为且仅设置为一个合法终态；崩溃协调可使用 `Interrupted`，但不能改写已经存在的 Published/Recycled Fact。
4. 从子项推导 Job/Batch 聚合态，再映射 TaskIntent：Running=`Active`，等待计划确认/普通错误待用户决定=`AwaitingUser`，缺卡=`AwaitingSource`，证据不足=`NeedsAttention`，真正全部完成/取消=`Completed | Cancelled`。
5. 软释放本 Attempt 的全部 OperationReservations 与未消费 ImportPartialClaim 并记录 release_reason；若 Task 已终结且 FileWriteLease.task_id 等于该 Task，同时清空 task_id 并递增 lease_generation。Task 从未取得 Lease（例如 CancelBeforeStart）时该步骤为 no-op；Task 未终结时保留 Lease，但下一 Attempt 取得执行权时仍须递增 generation。
6. 完成必要的 DeletionMark 软释放或保留：仅全部 Recycled 或用户取消剩余意图时释放；问题未解决时保持历史关联。
7. 检查最终 COMMIT。只有成功后才通知 UI，并为释放的 OperationReservation 路径排入定向重扫。

上述任一步或 COMMIT 失败时，不得处理下一文件、启动新 Attempt、释放 Lease、向 UI 报告终态或让扫描协调受保护路径。启动 Reconciler 必须先完成或保守中止该关闭事务。Report Finalized、Attempt 终态、Task 终态、OperationReservation/Lease 释放不能拆成互相可见的多个事务。

若用户取消的是“已有一个或多个已终结 Attempt、当前没有未终结 Attempt”的等待任务，不伪造新的写 Attempt：使用带 OperationId 的单一解决事务，先验证全部旧 Attempt/Report 已终结且没有活动 OperationReservation，再追加 `OperationResolutionEvent`（Import 使用 `TaskCancelled`，Recycle 使用 `CancelledRemaining`）、把 Job/Batch 与 Task 变为 Cancelled、按规则释放 DeletionMark，并仅在 Lease 仍属于该 Task 时清空它和递增 generation。任一条件或 COMMIT 失败均保持原等待态与 Lease；该事务是第 13.5 节原子关闭规则的无 Attempt 特例。

### 13.6 CAS 与写冲突

所有可变聚合行使用 `row_version` 或当前领域版本做 compare-and-swap：

```text
UPDATE ... SET row_version = row_version + 1
WHERE id = ? AND row_version = ?;
```

影响行数不是 1 即视为并发/陈旧命令，重新读取而不是强制覆盖。单写者不取消对象版本检查，因为后台扫描与 UI 命令仍可能按不同快照排队。

### 13.7 CacheRoot 创建与缓存淘汰

1. 选择或切换缓存位置时，先在短事务中提交 OperationEvent 与 `Prepared` CacheRootCreateIntent；COMMIT 前不得创建目录。
2. COMMIT 后重新验证父 DirectoryIdentitySnapshot，只按 Intent 中冻结的名称创建一个直接子目录。成功后在单一事务建立 Root/Binding/Ownership、切换 Active/Draining 并终结 Intent；任一步失败均不得采用、改名、递归清理或自动重试磁盘目录。
3. 淘汰前在同一事务提交 `Prepared` CacheEvictionIntent 与其唯一 cache-owner OperationReservation；不取得 FileWriteLease。删除前重验专属目录、Binding、Entry 身份和 OperationReservation owner。Fact、Intent 终态、Entry 状态与 OperationReservation 释放按第 11.3 节映射在一个事务落盘，COMMIT 失败立即停止本轮淘汰；崩溃或临时离线只恢复同一个非终态 Intent/OperationReservation，身份异常进入不可自动重试的 NeedsAttention。

## 14. 状态机执行规则

1. 每个状态字段均使用 CHECK 枚举。
2. 合法转换表在 Rust domain 层集中定义，数据库仓储使用“旧状态 + 版本”的条件更新。
3. 非法、重复、跨终态转换返回领域错误；不得用宽泛 `UPDATE state=?`。
4. Task-owned OperationReservation 必须与 TaskAttempt kind、子类型、父 Task 和当前 FileWriteLease generation 一致；cache-owned OperationReservation 必须与 CacheEvictionIntent/Entry/Root 身份一致且不得引用 FileWriteLease。任何文件动作前均重验对应 owner 仍为可执行非终态。
5. ImportPlanGeneration 的计划载荷一经创建不可改；生命周期只允许 `Proposed -> Confirmed | Rejected` 与 `Confirmed -> Superseded`。Attempt 只能引用一个 Confirmed generation、其 Ready validation 和 Passed space check，且不得扩展 Copy 集合、字节数或名称。
6. 进入 ImportAttempt/DeletionAttempt、TaskIntent、ImportPublishIntent、CacheRootCreateIntent 或 CacheEvictionIntent 终态，插入 ImportDuplicateProof/VerifiedPartialSnapshot/ImportPartialCleanupFact/ImportPublishedFact/ImportIndexedFact/DeletionRecycledFact/CacheEvictionFact，最终化 OperationReport 或追加 OperationResolutionEvent 后，由触发器或仓储保护禁止改写或删除；已提交 Validation/ValidationItem/SpaceCheck、TaskAttempt、ImportPartialOwnership 与 ImportPartialClaim 的身份字段同样禁止 UPDATE/DELETE，Claim 只允许一次软释放。
7. Attempt 终态、报告 Finalized、聚合状态和锁释放只允许通过第 13.5 节关闭仓储；普通状态更新 API 不得分别写这些字段。
8. 聚合状态只缓存可由子项推导的查询结果；TaskIntent lifecycle 必须与 Job/Batch 状态映射一致。启动完整性检查发现不一致时进入只读安全模式，不能静默修正不可回退事实。
9. `NeedsAttention` 不是失败的同义词；它表示证据不足，禁止自动重试或产生新的文件动作。

## 15. 迁移、备份与恢复

### 15.1 迁移

`SchemaMigration(version PK, checksum, applied_at, app_build_hash)` 保存前向迁移历史。流程：

1. 校验当前 schema 版本、迁移 checksum、`quick_check` 和 `foreign_key_check`。
2. 使用 SQLite Online Backup API 创建升级前一致快照；失败则不升级。
3. 在排他迁移窗口中执行前向迁移；检查所有 DDL/DML 与最终 COMMIT。
4. 复核行数、唯一约束、状态枚举、FK 和不可回退 Fact。
5. 失败时关闭新连接并从升级前快照恢复；不尝试半迁移运行。

不支持应用二进制对较新 schema 做自动降级。破坏性表重建必须先复制到新表、校验约束和行数，再原子替换；不得以清空数据库解决迁移冲突。

### 15.2 备份

- 每天首次启动创建一次一致快照；保留最近 7 个日常版本。
- 每次 schema 升级前额外备份；保留最近 3 个升级前版本。
- 使用 Online Backup API 或等价一致机制，不能复制活动数据库文件冒充备份。
- 备份位于 LocalAppData 独立目录，不含照片或缓存。
- 导入历史、删除 Attempt/Fact/报告及关键 OperationEvent 长期保留。
- 数据库备份只保护应用状态，绝不称为照片备份。

### 15.3 恢复与协调

从备份恢复后：

1. 先以只读方式验证 schema、FK、完整性和 Root 身份；
2. 规范化并检查 CurrentLibrary、强 VolumeIdentity、单调 VolumePolicy、未终结 TaskIntent、TaskAttempt/子类型状态、FileWriteLease generation，以及 OperationReservation 的二选一 owner 与跨 owner 活动唯一性；任一矛盾先进入只读安全模式；
3. 保留未终结 TaskIntent 和 task_id 非空的 FileWriteLease，显示恢复摘要；不得为“解锁”而无条件清空它们；
4. 重新观察文件系统，使用 Fact、FileIdentitySnapshot、Validation/Execution DuplicateProof、ImportPartialOwnership/Claim/VerifiedPartialSnapshot、ImportPublishIntent、Shell 证据、CacheRootCreateIntent 和 CacheEvictionIntent/OperationReservation 协调；
5. 崩溃留下的 Prepared/Running Attempt 必须先协调其已提交 Publish/CleaningOwnedPartial 意图，再按第 13.5 节最终化为可证明结果或 Interrupted/NeedsAttention，并最终化旧报告、释放可释放的旧 Claim/Task-owned OperationReservations；不得为关闭 Attempt 而提前释放尚未协调的 Cleaning Claim/OperationReservation。之后才允许用户确认并创建新的 Reconcile/Resume Attempt 与 Takeover Claim；
6. 不自动重放旧删除批次，不自动恢复写任务；用户确认后才继续或取消。Import 继续前必须重新执行 ImportPlanValidation；计划变化时先生成 Proposed ImportPlanGeneration 并重新确认；
7. 不覆盖、回收、采用或清理任何无法证明归属的正式文件、partial 或缓存文件；
8. Published 但未 Indexed 的文件只补索引；Recycled 缺证据时保持 NeedsAttention。若旧 Attempt 已有 ImportPublishIntent，只在该旧 Attempt 协调事务中判定其 rename 结果并随后关闭。恢复看到 Planned Ownership 一律进入 NeedsAttention，不认领或删除其 path；Owned incarnation 只能由新 Reconcile Attempt 的 Takeover Claim 接管并重新完整验证；CleaningOwnedPartial 只沿已提交清理状态与同一身份协调。Cleaned 后下一 Attempt 可在同一 Confirmed generation 创建新 ordinal/nonce/path，无需新计划确认；旧 VerifiedPartialSnapshot 不授权新 Attempt 发布；
9. 非终态 CacheEvictionIntent 保留同一 cache-owned OperationReservation：进程崩溃或可靠身份下的临时离线只恢复同一 Intent；身份异常进入 NeedsAttention 并释放 OperationReservation，不新建 ordinal；
10. 数据库损坏、VolumePolicy 缺失、唯一约束或状态/报告映射不一致时进入只读安全模式，Import 与 Recycle 同时关闭。

## 16. 必须通过的 schema 与恢复测试

### 16.1 约束测试

- 所有连接均验证 FK/WAL/FULL 设置。
- CurrentLibrary 与 Library lifecycle 不一致、新 Root 复用旧 Library、两个当前 Library 必须失败；可靠重连必须复用原 Library/Root。
- 相同 strong_identity_key 的第二个 VolumeIdentity、VolumePolicy UPDATE/DELETE/deny_write=0、受保护卷的新 volume_id 绕过必须失败。
- 活动 RootBinding、Asset path/file ID、PhotoAsset 角色、活动 DeletionMark、OperationReservation 和 FileWriteLease 的重复插入必须失败；OperationReservation owner 全空、混填、Task owner 缺 Lease、Cache owner 引用 Lease/错误 Entry，或两种 owner 占用同一路径/file ID 也必须失败。
- 第二个非终态 TaskIntent、Task/Attempt kind 与子类型不一致、同 Task ordinal、同 Attempt Step/Asset、同 mark 第二次进入 Batch 必须失败。
- Import generation/group/source item/group role/final path、Ownership `(step,ordinal)`/全历史 path/同 step 未解决 incarnation、Validation/Step、SpaceCheck/Validation、Validation/Execution DuplicateProof owner、活动 PartialClaim、每 attempt_step Snapshot、活动 Attempt、Deletion BatchPhoto/BatchAsset、CacheRootCreateIntent 及 Report owner 的全部唯一冲突必须失败。
- CacheRoot 同时拥有父/子两个活动 Binding 必须失败；CacheEntry 跨 Root 允许同源键，同 Root 的 Live/NeedsAttention render key 重复与全历史 path 重复必须失败。旧 Entry 只有进入 Missing/Evicted 后才允许以新 ID/ordinal/nonce/path CREATE_NEW 下一 incarnation；NeedsAttention 不允许绕过。
- Attempt/ScanRun 无 Report、重复 Report、Report 类型错误，以及 ResolutionEvent 的 Report/Task/subject/event_kind 任一跨链错配必须失败。
- FK 悬空、非法枚举、负大小、零/倒退版本和非法容量必须失败。
- Alias 环、Alias 链及沿 Alias 发起破坏性命令必须失败。
- Finalized OperationReport、终态 ImportAttempt/DeletionAttempt/TaskIntent/ImportPublishIntent/CacheRootCreateIntent/CacheEvictionIntent、已提交 TaskAttempt/Validation/ValidationItem/SpaceCheck、ImportPartialOwnership/Claim 身份、ImportDuplicateProof、VerifiedPartialSnapshot、ImportPartialCleanupFact、ImportPublishedFact、ImportIndexedFact、DeletionRecycledFact、CacheEvictionFact 和 OperationResolutionEvent 的更新/删除必须失败；Claim 只能软释放一次。

### 16.2 版本与配对测试

- Asset file ID、Root/Binding、路径、大小、mtime、已知内容指纹或 presence 变化递增 AssetVersion。
- 任一成员 AssetVersion 或 PhotoAsset 关系变化递增 PhotoVersion。
- 标记快照与任一版本/成员/身份不一致时不可进入 Batch。
- 单张加成员、两个 Photo 合并、拆分、缺失成员和墓碑 Alias 严格遵守 PhotoId 规则。
- 历史 EXIF 证据不足时漏配；SameImportGroup 正向证据和日期矩阵正确。

### 16.3 Import 状态与故障注入

- 预检 skip 在执行准备中生成 Validation proof；任一端比较期间变化不得提交该 Proof。Validation proof 即使仍新鲜也不能让 AttemptStep 进入 SkippedDuplicate。
- 真正跳过前必须在当前 Attempt 再次完整哈希、逐字节比较并原子提交 attempt_step 专属 Execution proof/PathBindingProof/SkippedDuplicate；复用 Validation proof、旧 Attempt proof、旧 lease/OperationReservation 或任一端前后身份变化必须失败。
- 两种 ImportDuplicateProof 都必须拥有通过且同 owner/scope、同时包含 source+target 的 ImportDuplicatePathBindingProof。原比较句柄保持打开时，两端计划路径重开句柄必须分别解析回各自起始 RootBinding/path key/volume/file ID；源或目标任一路径被替换、只在其他路径找到相同内容、containment/reparse 不一致时不得跳过。
- skip、名称族、后缀、目标路径或新增字节变化只生成 Proposed generation；用户未确认前不得检查空间或创建 Attempt。
- 拒绝旧 Proposed 后可追加新 Proposed，但旧 Confirmed 指针不变；不能改写旧提案载荷或同时保留两个 Proposed。
- 每次 Initial/Retry/Resume/Reconcile 只有一个 ImportSpaceCheck 可被 ImportAttempt 使用，精确字节常量和整数向上取整跨 Rust/SQLite/UI 测试一致。
- 首个写 Attempt 前取消只创建无 Step、无 Lease/OperationReservation 的 CancelBeforeStart Attempt，并在同一事务得到 Finalized/Cancelled Report 与终态 Job/Task；不得发生任何文件系统写动作。
- AlreadyResolved 与 Unreadable Step 不执行文件动作但必须进入对应终态并可关闭 Attempt；缺少已有事实的 AlreadyResolved 必须失败。
- Attempt 创建后使 duplicate 证明过期、抢占名称或增加所需容量，只能 PlanStale/Conflict/NeedsAttention，不得转 Copy、改名或扩容。
- 旧数据库缺少 `_2` 历史时仍能枚举完整后缀族并发现相同内容。
- Planned Ownership/PartialPath OperationReservation 创建、CREATE_NEW、file ID+Owned+Created Claim 提交、Flush、VerifiedPartialSnapshot COMMIT、设置 mtime、Takeover Claim、CleaningOwnedPartial COMMIT、句柄删除、CleanupFact/状态/Claim/OperationReservation 原子关闭、ImportPublishIntent、no-replace rename 和 ImportPublishedFact 的每个边界均注入崩溃。
- Planned Ownership 的 file_id/created_size_observed 必须为空；Owned/CleaningOwnedPartial/ConsumedByPublish 必须非空。未 CREATE_NEW 的 Planned->Cleaned 与 CREATE_NEW 后未提交身份的 Planned->NeedsAttention 可保持为空；从 Owned 转入 Cleaned/NeedsAttention 时清空创建身份必须失败。
- CREATE_NEW 与 Owned COMMIT 之间崩溃必须得到 Planned->NeedsAttention，不能按 path/nonce 认领或删除，也不能创建下一 incarnation；正常且可证明尚未调用 CREATE_NEW 的 Planned 取消才可变 Cleaned。
- 新 Reconcile Attempt 只有在旧 Attempt/Report/Claim 已关闭后才能接管同一个 Owned incarnation；必须取得当前 lease 与 SourceRead/PartialPath/FinalPath OperationReservations、创建 Takeover Claim、重新完整验证并生成当前 attempt_step 的新 Snapshot。旧 Snapshot 复用、两个活动 Claim 或跨 Job/step 接管必须失败。
- created_size_observed 为 0 或中间值时不影响最终验证；最终 Snapshot 必须保持创建时 Binding/path/file ID，且 size 等于 expected_bytes、内容等于冻结源。拿最终 size 与 created_size_observed 比较必须被测试拒绝。
- Owned 不完整只允许先提交 CleaningOwnedPartial 再通过已验证句柄删除；路径删除、清理前释放 Claim/OperationReservation、结果半提交或身份异常不得产生 Cleaned。Cleaned 后下一 Attempt 可在同 Confirmed generation 创建新 ordinal/nonce/path 且不生成 Proposed plan；ConsumedByPublish 和 NeedsAttention 均禁止新 incarnation。
- 没有当前 Claim/VerifiedPartialSnapshot/ImportPublishIntent、快照与当前 partial 不一致、Ownership/Step/Attempt/OperationReservation/目标任一错链，或快照已被另一 Intent/ImportPublishedFact 使用时必须拒绝发布。
- 已发布未索引只补索引；正式文件永不因数据库失败被删除。
- 每次 Retry/Resume/Reconcile 创建新 TaskAttempt ordinal；崩溃的旧 Running Attempt 先终结并最终化旧报告，旧结果不变。
- 运行中真实空间不足停止后续项、保留已发布项并形成准确报告。

### 16.4 Recycle 状态与故障注入

- Batch、初始 Attempt、Open Report、Lease 和 OperationReservations 的确认事务在每个写点故障时必须全部回滚且不调用 Shell。
- BatchAsset 逐项等于来源 MarkAssetSnapshot；从当前 Asset 刷新值、一个 mark 进入第二 Batch 或同一 Batch 重复成员必须失败。
- Batch 冻结到 PreDeleteItem 之间修改文件 ID、大小、mtime、成员或 Root 时拒绝执行。
- 验证后替换文件、Shell 取消、回收站关闭、`psiNewlyCreated=NULL`、部分 JPG/RAF 成功和回调结果矛盾均进入保守状态。
- 每个 PostDeleteItem 后模拟 SQLite `BUSY/IOERR/FULL/CORRUPT`，确认批次立即停止。
- 重试只处理原位置身份匹配的剩余项，原子创建新 Attempt/Report/OperationReservations、递增 lease generation 并把 Attempt/Batch/Task 置 Running/Running/Active；旧 callback 必须被拒绝。
- DeletionRecycledFact 的 attempt_asset 与 batch_asset 不同链时必须失败；NeedsAttention 未重新证明身份或未由用户显式重试时不得回到执行路径。
- DeletionRecycledFact 永不回退；回收站恢复后旧标记不自动恢复。

### 16.5 OperationReservation、迁移与恢复测试

- 扫描、Import、Recycle 和 CacheEviction 对同一路径/file ID 并发时只能一个 owner 持有 OperationReservation；cache owner 不创建或占用 FileWriteLease。
- 非终态任务期间第二个 Task 以及 Root、ImportSource 和 CacheRoot 切换必须失败；Attempt 关闭释放 OperationReservations 后允许定向重扫，但 Task 终态前全局 Lease 不让位给另一任务。
- Attempt 终态、Report Finalized、Job/Batch/Task 聚合态、OperationReservation/Lease 释放的每个写点发生 COMMIT 故障时，不得暴露半关闭状态或启动下一文件。
- 扫描不观察已证明归属的 partial；未知 partial 不被采用或删除。
- CacheRootCreateIntent COMMIT 前不得 mkdir；mkdir 后绑定事务失败、同名目录预先存在或父身份变化时不得采用、改名或清理该目录。
- CacheEvictionIntent 与其唯一 cache-owned OperationReservation 未在同一事务 COMMIT 前不得删除；同一 Entry 的第二个 Intent、Fact/Intent/OperationReservation Entry 错配必须失败。Fact/Intent/Entry/OperationReservation 原子映射逐写点故障不得半提交；崩溃和临时离线恢复只继续同一非终态 Intent/OperationReservation，身份异常进入 NeedsAttention 后不得递归清理、为旧 Entry 另开 ordinal 或伪造 DeleteSucceeded。Missing/Evicted 后再生必须创建新 Entry incarnation 与自己的 Intent。
- 从每个已发布 schema 版本逐级迁移，校验行数、FK、唯一约束、状态和 Fact。
- 每日及升级前备份可恢复 CurrentLibrary、VolumePolicy、Root、标记快照、游标排序锚、ImportPlanGeneration、Import/Deletion Attempt、ImportPublishIntent、Fact、OperationReport、OperationResolutionEvent、CacheRootCreateIntent 和缓存所有权/淘汰 Intent。
- 恢复旧备份后，磁盘新增正式文件、未知 partial 和未知缓存条目均不会被覆盖或删除。

### 16.6 规模与查询测试

在 10 万 Photo、20 万 Asset、1 TB 级模拟图库上验证：

- 时间线查询使用稳定排序键和 PhotoId tie-break，不做全表内存排序；
- ViewingCursor 最近下一张查询命中索引；
- 待删除计数由活动有效标记和未解决 Batch 问题派生；
- 启动只读取当前 Revision/关系，不加载全部 Attempt 和报告明细；
- LRU 查询、定向重扫和恢复协调均有覆盖索引，不以放宽安全校验换取性能。

## 17. 明确不由 schema 承诺的事项

- SQLite 备份不保护唯一照片内容。
- Schema 不消除 `IFileOperation` 最终校验与 Shell 执行之间的极短外部变化窗口；该窗口由最接近调用的身份校验、逐文件回调证据、Critical 停止与故障测试控制。
- v1 不支持永久删除、应用内回收站、历史图库重复清理、画面相似判断、手动配对/拆分或完整 RAW 解码。
- UI 可隐藏内部 ID，但不能因此删除数据库中的身份、版本、证据或报告字段。

## 18. 实施准入

阶段 0 只有在以下条件全部满足后才算完成：

1. 首版迁移实现本 ADR 的核心表、FK、精确唯一约束、状态 CHECK、CurrentLibrary、单调 VolumePolicy 和不可回退 Fact；
2. Rust domain 层拥有集中、可穷举测试的 Task/Import/Recycle/CacheEviction 状态转换与 Attempt 原子关闭表；
3. `observe_asset`、DeletionMark、ImportPlanGeneration/ImportPlanValidation/ImportSpaceCheck、Import、Recycle 和缓存淘汰仓储使用对象版本/CAS；
4. DuplicatePathBindingProof、VerifiedPartialSnapshot/ImportPublishIntent 发布链、mark snapshot 到 BatchAsset 的只读复制以及 Report/ResolutionEvent 单向关系均有约束测试；
5. 备份、迁移失败恢复、Interrupted Attempt 协调和只读安全模式测试通过；
6. 安全准入记录绑定 schema 版本、迁移 checksum、应用二进制和依赖锁文件哈希；
7. schema、Root/身份、JobRunner、Reconciler 或数据库完整性发生变化时，旧 Import/Recycle 准入自动失效并重新测试。
