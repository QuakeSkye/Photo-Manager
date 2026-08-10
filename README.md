# Fuji 照片管理器

> 面向 Windows 的本地 Fuji 照片管理工具，用于安全浏览、导入和复核 JPG/JPEG/RAF 图库。

当前版本：`v1.0.2`

## 项目简介

Fuji 照片管理器是一款以本地文件和数据安全为核心的桌面应用。它可以递归索引现有 Fuji 图库，根据 EXIF 拍摄时间组织照片，保守配对 JPG 与 RAF，并提供快速浏览、导入预检、待删除复核和外部工具联动。

项目优先保护原片：扫描与浏览不会修改现有照片；导入不会覆盖已有文件；删除操作只会尝试移入 Windows 回收站，不提供永久删除降级路径。

## 核心功能

- **本地图库索引**：递归识别 JPG、JPEG 和 RAF 文件，不移动或重命名历史照片。
- **时间线浏览**：根据可靠的 EXIF 拍摄时间组织照片，支持按时间升序或降序查看。
- **JPG/RAF 配对**：在证据可靠时将同一张照片的 JPG 与 RAF 组成一个逻辑项目，不确定时保持分离。
- **快速预览**：使用主预览区、虚拟胶片带和照片信息面板浏览大型图库。
- **照片信息**：展示拍摄时间、相机、镜头、曝光参数、尺寸和文件信息。
- **快捷筛选**：使用方向键切换照片，按 `D` 标记或取消待删除状态，按 `R` 撤销当前会话最近的标记操作。
- **待删除复核**：集中查看待处理照片，确认后将对应文件移入 Windows 回收站。
- **安全导入**：从用户选择的来源文件夹进行只读预检，只复制新增内容，并在执行前展示导入计划。
- **外部工具**：可配置 Adobe Photoshop 打开当前 RAF，也可在资源管理器中定位照片。
- **缓存与报告**：支持可配置的预览缓存、扫描报告、导入报告和异常恢复状态。

## 数据安全设计

照片通常不可替代，因此本项目将安全约束置于普通功能之前：

- 扫描、EXIF 提取、缩略图和预览不会修改现有 JPG/RAF。
- 导入来源按只读边界处理，应用不会向来源卷创建缓存、日志或临时文件。
- 导入目标采用不覆盖语义；目标已存在或身份发生变化时，操作会停止或重新确认。
- 删除只通过 Windows Shell 移入回收站；回收站不可用时不会降级为永久删除。
- 文件写入前会重新验证目录、文件身份和运行时安全状态。
- SQLite 使用 WAL、外键约束和完整同步策略记录索引、任务与恢复信息。

> **重要：** Windows 回收站不是照片备份，SQLite 数据库备份也不包含 JPG/RAF 原片。正式使用前，请在独立介质上保留经过校验的照片备份，并先使用可丢弃数据验证导入和回收流程。

## 系统要求

### 运行环境

- Windows 10/11 x64
- Microsoft Edge WebView2 Runtime
- 建议将 Fuji 图库和导入目标放在稳定的 NTFS 文件系统中

### 开发环境

- Node.js `24.x`（项目要求 `>=24 <25`）
- npm
- Rust `1.85+`，使用 MSVC toolchain
- Microsoft C++ Build Tools，并安装“使用 C++ 的桌面开发”工作负载
- Microsoft Edge WebView2 Runtime

完整的 Windows 开发环境准备方式参见 [Tauri 2 官方前置条件](https://v2.tauri.app/zh-cn/start/prerequisites/)。

## 基本使用流程

1. **选择图库**：首次启动后选择现有 Fuji 照片根目录。
2. **等待扫描**：应用只读扫描 JPG/JPEG/RAF，并建立本地索引和预览缓存。
3. **浏览与筛选**：使用胶片带、方向键或鼠标浏览；按 `D` 标记待删除照片，按 `R` 撤销。
4. **导入新照片**：选择内存卡或其他来源文件夹，先执行只读预检，再核对计划并确认导入。
5. **复核待删除项**：进入待删除页面检查文件数量和容量，确认后移入 Windows 回收站。
6. **配置外部工具**：在设置中选择 Photoshop 和预览缓存位置。

应用不会在打开导入页面或检测到来源时自动复制照片，实际导入和回收操作均需要明确确认。

## 从源码运行

克隆仓库：

```powershell
git clone https://gitee.com/SunCoulson/photo-manager.git
cd photo-manager
```

安装依赖：

```powershell
npm ci
```

启动 Tauri 开发环境：

```powershell
npm run tauri -- dev
```

仅启动前端开发服务器：

```powershell
npm run dev
```

仅前端模式无法使用由 Rust/Tauri 提供的文件扫描、导入、回收站和本地数据库能力。

## 构建 Windows 安装包

执行：

```powershell
npm run tauri -- build
```

项目当前使用 NSIS 目标。构建成功后，安装包通常位于：

```text
src-tauri/target/release/bundle/nsis/
```

仓库当前没有提供公开下载的预构建安装包；自行构建的安装程序也未配置组织代码签名。

## 开发与测试命令

| 命令                   | 用途                                          |
| ---------------------- | --------------------------------------------- |
| `npm run format`       | 格式化 TypeScript、CSS、Markdown 和 Rust 代码 |
| `npm run format:check` | 检查代码格式                                  |
| `npm run lint`         | 运行 ESLint 和 TypeScript 类型检查            |
| `npm test`             | 运行 Vitest 测试                              |
| `npm run test:ui`      | 运行前端 UI 测试                              |
| `npm run test:e2e`     | 运行端到端测试套件                            |
| `npm run test:rust`    | 运行 Rust 测试                                |
| `npm run clippy`       | 以警告即错误模式运行 Clippy                   |
| `npm run safety:audit` | 检查禁止的危险文件操作模式                    |
| `npm run check`        | 运行常规格式、静态检查、测试和安全审计        |

部分 Windows 安全验收脚本需要专用测试目录、可丢弃数据或特定磁盘环境。不要直接在唯一一份真实照片上运行阶段验收脚本。

## 技术栈

- **桌面框架**：Tauri 2
- **前端**：React 19、TypeScript 5、Vite 7
- **后端**：Rust 2024 edition
- **数据库**：SQLite（rusqlite）
- **测试**：Vitest、Testing Library、Rust tests、Clippy
- **Windows 集成**：Windows Shell、`IFileOperation`、文件与卷身份验证

## 目录结构

```text
.
├─ src/                         React/TypeScript 前端
│  ├─ app/                     应用外壳、路由和全局交互
│  ├─ features/                图库、导入、扫描、待删除和设置页面
│  └─ shared/                  Tauri 命令封装与共享 UI
├─ src-tauri/                   Rust/Tauri 桌面后端
│  └─ src/
│     ├─ catalog/              扫描、配对和时间线
│     ├─ import_preflight/     只读导入预检与重复证明
│     ├─ import_execution/     安全复制与不覆盖发布
│     ├─ recycle/              Windows 回收站集成
│     ├─ safety/               运行时安全门与故障处理
│     ├─ db/                   SQLite 连接、迁移、仓储和备份
│     └─ cache/                预览缓存与所有权管理
├─ tests/                       集成与端到端测试
├─ scripts/                     构建、检查和阶段验收脚本
└─ docs/superpowers/            设计规格、实施计划和测试报告
```

## 当前限制

- 目前只面向 Fuji JPG/JPEG/RAF 图库和 Windows 10/11 x64。
- 同一时间只使用一个当前图库，不提供多图库并行管理。
- RAF 主要使用内嵌 JPEG 进行预览，不包含完整 RAW 解码和专业显影能力。
- 暂不支持评分、星标、全文搜索、相似照片、AI 识图、人脸识别或地图。
- 不会自动整理、移动、重命名或清理历史图库中的重复文件。
- 不提供云同步、账号系统或照片文件备份。
- Windows 回收站、磁盘硬件和外部程序的行为不在应用的绝对控制范围内。

更多边界参见[已知限制](docs/superpowers/reports/phase-eight-known-limitations.md)。

## 项目文档

- [总体设计规格](docs/superpowers/specs/2026-07-28-local-photo-manager-design.md)
- [数据库与状态模型 ADR](docs/superpowers/specs/2026-07-29-local-photo-manager-schema-adr.md)
- [运行时写入安全门设计](docs/superpowers/specs/2026-08-09-local-photo-manager-runtime-write-gate-design.md)
- [EXIF 信息与适应窗口设计](docs/superpowers/specs/2026-08-09-photo-exif-and-fit-zoom-design.md)
- [图库排序与扫描恢复设计](docs/superpowers/specs/2026-08-10-library-sort-scan-version-design.md)
- [发布说明](docs/superpowers/reports/phase-eight-release-notes.md)
- [已知限制](docs/superpowers/reports/phase-eight-known-limitations.md)

## 参与贡献

1. Fork 本仓库并从最新分支创建功能分支。
2. 保持改动聚焦，并为行为变化补充相应测试。
3. 提交前至少运行与改动范围相符的格式检查、静态检查和测试。
4. 创建 Pull Request，说明改动目的、验证方式及任何照片数据安全影响。

涉及扫描、导入、回收站、缓存清理、SQLite 状态或文件系统写入的改动，应同时说明其安全边界与故障恢复行为。
