# Fuji 照片管理器 v1.0.1 GUI 重构建实施计划

日期：2026-08-09  
依据：`docs/superpowers/specs/2026-08-09-local-photo-manager-v1-0-1-gui-rebuild-design.md`

## 1. 边界

本次保持版本 1.0.1，只修复 release EXE 的 Windows GUI subsystem，并校正当前用户安装后的桌面与开始菜单快捷方式。不得重跑或声称通过阶段 8，不得修改旧候选机器证据，不得 push 或发布远端。

## 2. 修改与源码验证

1. 在 `src-tauri/src/main.rs` 首行加入 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`。
2. 运行 Rust 格式检查、`cargo check --release` 与现有安全审计，确认改动不影响 release 编译及安全规则。
3. 检查 Git diff，确保产品代码只有预期的一行语义改动。

## 3. 重构建与独立归档

1. 使用现有锁文件执行一次 `npm run tauri -- build --bundles nsis`。
2. 不覆盖 `artifacts/phase-eight-v1.0.1/`；将新安装包复制到 `artifacts/v1.0.1-gui-rebuild/`。
3. 计算 release EXE 与安装包的 SHA-256、大小和版本，并验证 release EXE 的 PE subsystem 精确为 `Windows GUI (2)`。

## 4. 安装与快捷方式

1. 静默安装新生成的 current-user NSIS 安装包，安装位置沿用当前用户 LocalAppData，不使用 D 盘。
2. 从卸载注册信息或实际文件定位安装目录，确认安装 EXE 的 SHA-256 与 release EXE 一致。
3. 读取桌面和开始菜单快捷方式的 TargetPath、Arguments、WorkingDirectory；若隔离安装环境写入沙箱用户名，仅重建当前用户的这两个快捷方式并再次核验。

## 5. 非阶段 8 冒烟与证据

1. 从快捷方式启动应用，确认 GUI 窗口出现，且没有因本次启动新增 `cmd.exe` 或控制台窗口；随后正常关闭。
2. 写入 `artifacts/v1.0.1-gui-rebuild/README.md` 和机器可读 manifest，记录构建/安装 hash、版本、路径、快捷方式与冒烟结果。
3. 在最终报告首部标记旧阶段 8 结论只绑定旧 EXE hash，新安装的 EXE 未重跑阶段 8、不是已验收候选。
4. 运行 `git diff --check`，只暂存本任务文件，明确排除用户的 `.superpowers/**` 与 `scripts/ui-check.mjs`，创建本地 Git 提交。

## 6. 完成判定

仅当 PE subsystem、安装路径、安装 EXE hash、两个快捷方式和无控制台启动冒烟全部通过时，才交付新安装包。任一项失败即报告未完成，且不改变旧候选证据的历史真实性。
