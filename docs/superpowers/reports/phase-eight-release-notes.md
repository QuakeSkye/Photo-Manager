# Fuji 照片管理器 v1.0.1 发布说明

日期：2026-08-09

v1.0.1 修复了 v1.0.0 中生产写命令没有真正受声明 gate 强制约束的高风险问题。v1.0.0 的阶段 8 完成结论已失效，不能用于授权真实照片操作。

主要变化：

- 新增唯一 `RuntimeSafetyGate`。健康启动时 Import 与 Recycle 默认开放，不再需要人工准入或软件内解锁。
- 所有来源选择、预检、Import 开始/重试、Recycle 开始/重试，以及每个目录创建、partial 创建、发布和单 asset Shell 动作都强制使用绑定实例、feature 和 generation 的 permit。
- `require_feature()` / `revalidate()` 每次同步检查实时 writer health、数据库模式和一致 gate 快照，未通知的 writer poisoning 不能穿透。
- 物理动作完成后不再用 gate 拒绝结果 Fact：始终尝试提交观察结果，提交后才决定是否开始下一文件；writer 无法提交时保留非终态 Intent、Reservation、Lease 和结果不明状态供重启协调。
- Recycle 改为每个 asset 一次 STA Shell 调用；仍只使用 Windows 回收站 flags，没有永久删除或降级路径。
- 设置页显示实时写入安全状态；gate 关闭时执行按钮禁用且原因可见，浏览、报告、取消和移除标记保持可用。
- 重新完成 100,000 Photo、200,000 Asset、至少 1 TiB 逻辑规模，机械盘 I/O、SQLite、缓存、DPI/键盘/对比度和准确安装候选安全验收。

冻结候选：

- 安装包 SHA-256：`6dc9af6740d50d673240e3e668d0249c47b072b3cf46919ff653f78c6e768c8d`
- 已安装 EXE SHA-256：`bc855771910129719c4c56d513c024debd832c23fffe71da909ea2c7079801d8`
- 安装位置：`C:\Users\15684\AppData\Local\Fuji 照片管理器`

重要安全说明：Windows 回收站不是照片备份，SQLite 备份也不是照片备份。`IFileOperation` 最终校验与 Shell 执行之间仍存在非原子残余窗口。安装包未签名；任何签名、重建或绑定输入变化都必须完整重测。

本候选没有推送、上传或发布到任何远端。
