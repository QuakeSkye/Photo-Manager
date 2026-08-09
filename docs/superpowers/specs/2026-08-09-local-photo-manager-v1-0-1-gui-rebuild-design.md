# Fuji 照片管理器 v1.0.1 GUI 重构建设计

日期：2026-08-09

## 1. 背景与目标

已安装的 v1.0.1 EXE PE Subsystem 为 `Windows Console (3)`，因此启动 GUI 时出现控制台窗口；`src-tauri/src/main.rs` 缺少 release 构建使用的 `windows_subsystem = "windows"` 属性。由 Codex 安装环境创建的开始菜单和桌面快捷方式还指向了 `C:\Users\CodexSandboxOffline\...`，而非实际当前用户安装目录。

本次保持产品版本 1.0.1，只修复 GUI 子系统和当前用户快捷方式。用户明确要求重新构建但不重跑阶段 8，因此新二进制不得继承旧候选的验收结论。

## 2. 实施方案

1. 在 `src-tauri/src/main.rs` 增加仅在非 debug 构建生效的 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`。debug 构建保留控制台，便于开发诊断；release/NSIS 构建使用 `Windows GUI (2)`。
2. 沿用 Tauri 标准 NSIS current-user 安装，不引入自定义 NSIS 模板。安装完成后读取开始菜单和桌面 `.lnk`；目标必须等于实际安装 EXE，参数为空，工作目录等于实际安装目录。
3. 若 Codex 的隔离安装环境再次把快捷方式目标写成沙箱用户名，则仅为当前用户重建这两个快捷方式，目标绑定实际安装 EXE。此修正用于当前机器安装状态，不伪称安装包在所有环境下完成了快捷方式专项验收。
4. 新安装包和重构建说明保存到独立目录 `artifacts/v1.0.1-gui-rebuild/`。保留 `artifacts/phase-eight-v1.0.1/` 的旧准确候选，不覆盖旧安装包、manifest 或机器证据。

## 3. 验证边界

本次只执行以下冒烟检查，不执行阶段 8：

- release EXE PE Subsystem 精确为 `Windows GUI (2)`；
- 安装包、release EXE、实际安装 EXE 的版本、大小和 SHA-256 被重新记录；
- 安装路径仍为当前用户 LocalAppData，且不在 D 盘；
- 开始菜单和桌面快捷方式目标、参数与工作目录正确；
- 从快捷方式启动后应用窗口出现，没有新的 `cmd.exe` 或控制台窗口；随后正常关闭应用；
- 用户 AppData 与照片目录不作为写入或删除测试对象。

这些检查不覆盖 10 万/20 万规模、Import/Recycle、故障注入、SQLite、缓存、I/O 或 DPI 完整矩阵。

## 4. 证据与失效规则

- 重新构建会产生新的 EXE 和安装包 SHA-256；旧阶段 8 报告仍真实描述旧 hash，但不适用于新安装状态。
- 最终报告和新目录 README 必须醒目标记“未重跑阶段 8，不是已验收候选”。
- 不修改旧机器证据 JSON，不生成新的 Import/Recycle admission，不把冒烟检查计为 27/27。
- 创建本地 Git 提交，不 push、不发布远端。

## 5. 完成条件

只有 GUI subsystem、实际安装路径、两个快捷方式和无控制台启动冒烟全部通过，才交付新的 v1.0.1 安装包；若任一项失败，则报告未完成并保留旧已验收候选文件不变。
