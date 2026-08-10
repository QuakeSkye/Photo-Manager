# 图库排序响应、扫描恢复与版本展示实施计划

日期：2026-08-10  
依据：`docs/superpowers/specs/2026-08-10-library-sort-scan-version-design.md`

## 1. 实施边界

本计划在已提交的导入预检后台化、导入执行后台化、元数据贯通和 JPG/RAF 配对修复之上，完成图库只读命令响应、排序定位、扫描兼容性提示和 1.0.2 版本展示。不得修改用户照片文件，不自动替换现有安装目录或数据库。

现有未跟踪文件 `.superpowers/**`、`local-test-install/**` 和 `scripts/ui-check.mjs` 属于用户；格式化、暂存和提交均明确排除。

## 2. 任务一：用测试锁定排序与扫描交互

涉及文件：

- `src/app/App.test.tsx`
- `src/features/scanning/ScanView.test.tsx`
- 必要的 demo 命令夹具

步骤：

1. 修改排序测试，断言切到最早/最新后直接选中新列表第一项，不再依赖键盘回到首项。
2. 增加图库路由排序只请求图库快照的断言；待删除路由保留双快照刷新。
3. 增加排序失败保持旧排序与旧选择的测试。
4. 增加数据库只读/版本过新时续扫按钮禁用和原因可见的测试。
5. 增加正常数据库可续扫的回归测试。

## 3. 任务二：后台化图库阻塞命令

涉及文件：

- `src-tauri/src/commands/library.rs`
- `src-tauri/src/commands/pending_delete.rs`
- 相关 Rust 测试

步骤：

1. 从同步 command 中提取接受 owned `Arc` 的领域包装函数。
2. 将图库快照、照片详情、照片预览和待删除快照命令改为 `async fn`。
3. 使用 `tauri::async_runtime::spawn_blocking` 执行 SQLite、机械盘读取和 Base64 编码。
4. 统一映射后台任务 join 失败；保留原 `DomainError`。
5. 保持扫描状态、取消等轻量内存命令同步。

## 4. 任务三：修正排序请求与选择提交

涉及文件：

- `src/app/App.tsx`
- `src/app/App.test.tsx`

步骤：

1. 图库路由只请求目标排序的图库快照；待删除路由同时请求所需双快照。
2. 成功后一次性提交排序、快照和新列表第一项。
3. 失败时不修改排序、快照或选择。
4. 排序进行中阻止重复请求，并确保浏览游标只保存已提交状态。

## 5. 任务四：扫描可写性前置检查与真实诊断

涉及文件：

- `src-tauri/src/commands/library.rs`
- `src-tauri/src/db/connection.rs`
- `src-tauri/src/safety/runtime_gate.rs`
- `src/features/scanning/ScanView.tsx`
- `src/app/App.tsx`
- `src/shared/commands/safety.ts`
- 相关 Rust/React 测试

步骤：

1. 为数据库提供不泄漏 writer 的扫描可写性检查，区分只读模式与写入器失效。
2. `retry_library_scan` 在启动线程前调用检查并立即返回稳定错误。
3. 安全状态在数据库只读时使用已保存的用户可见原因，保留内部诊断隔离。
4. 扫描页接收安全状态；不可写时禁用续扫并显示原因。
5. 验证 schema 过新被描述为升级需求而非数据库损坏。

## 6. 任务五：统一 1.0.2 与右下角版本标签

涉及文件：

- `package.json`
- `package-lock.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`
- `src-tauri/tauri.conf.json`
- `vite.config.ts`
- 前端类型声明、应用根组件和样式
- 版本一致性测试或构建脚本

步骤：

1. 将全部产品版本来源升级到 1.0.2。
2. Vite 从 `package.json` 注入构建常量，不在组件中维护第二份版本。
3. 应用根节点渲染固定右下角版本标签，并为状态栏预留空间。
4. 增加版本一致性校验，阻止前端、Cargo 与 Tauri 配置再次同名异构。
5. 覆盖主要路由与启动状态的版本可见性。

## 7. 验证与发布构建

按风险由小到大运行：

```powershell
npm test -- src/app/App.test.tsx
npm run test:rust -- commands
npm run test:rust -- safety
npm run lint
npm test
npm run test:e2e
npm run test:rust
npm run clippy
npm run safety:audit
npm run build
npm run tauri -- build
```

同时运行 scoped Prettier、rustfmt、`git diff --check` 和版本一致性检查。完整 `format:check` 若仍只被用户未跟踪的 `scripts/ui-check.mjs` 阻塞，必须明确记录，不能改写该文件。

## 8. 完成标准

- 首次启动扫描期间切换排序，Windows 窗口不进入“未响应”。
- 两种排序均定位新列表第 1 张。
- schema 较新的数据库提示安装新版并禁用续扫；正常数据库可续扫。
- 所有页面右下角显示 `v1.0.2`，安装包文件版本与界面一致。
- 1.0.2 安装包包含此前所有导入链路和 JPG/RAF 配对修复。
- 用户照片零写入，用户未跟踪文件保持不变。
