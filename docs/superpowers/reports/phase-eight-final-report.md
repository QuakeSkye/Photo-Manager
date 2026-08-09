# 阶段 8 最终测试报告（v1.0.1）

> **适用范围警告（2026-08-09 GUI 重构建后）：** 本报告的 27/27 结论只绑定旧安装 EXE SHA-256 `bc855771910129719c4c56d513c024debd832c23fffe71da909ea2c7079801d8`。当前安装的 GUI 重构建 EXE SHA-256 为 `56e4605d56fb87139a937b1dedbdd557421385b37328439111fd067c739d4f02`，未重跑阶段 8，不是本报告已验收候选。参见 `docs/superpowers/reports/v1.0.1-gui-rebuild-report.md`。

日期：2026-08-09
结论：通过（产品验收 27/27；未解决 Critical 0；未解决 High 0）

本报告仅对应下面列出的准确 v1.0.1 安装包和实际安装 EXE。v1.0.0 报告已失效；任何重新构建、二进制、锁文件、schema、迁移或绑定证据变化都会使本报告和两份准入失效。

## 冻结候选

| 对象                       | 冻结值                                                                                              |
| -------------------------- | --------------------------------------------------------------------------------------------------- |
| 源码冻结提交               | `0c7009a`                                                                                           |
| 源码树 SHA-256             | `94f32a75439f0adc7337b97fdfd7cf82390d2896121f4541e4e8a76cd27f80e7`                                  |
| 安装包                     | `Fuji 照片管理器_1.0.1_x64-setup.exe`，3,581,783 字节                                               |
| 安装包 SHA-256             | `6dc9af6740d50d673240e3e668d0249c47b072b3cf46919ff653f78c6e768c8d`                                  |
| 实际安装 EXE               | `C:\Users\15684\AppData\Local\Fuji 照片管理器\local-photo-manager.exe`，14,658,560 字节，版本 1.0.1 |
| 已安装 EXE SHA-256         | `bc855771910129719c4c56d513c024debd832c23fffe71da909ea2c7079801d8`                                  |
| Candidate manifest SHA-256 | `0ea5b93fb5366fbad7132c55344ccb8199a1e0917e33fe4dd998166219e38aea`                                  |
| `package-lock.json`        | `6148b74a64871fad9332fac42245ac937eadad31d6fa052259e3839dc3a0242c`                                  |
| `Cargo.lock`               | `806aef41f7c7fac9c10318b22ea5d3192a1961eb724a3084d66d1c03a9df2b71`                                  |
| schema                     | v8；`e78fd625669f4b76629a25a210cbd5de70eaf35de54605430c2607c29d969ca2`                              |
| 迁移链                     | 8 份；`3d736ebf30cf68c566585e0ae659292e417e752b3464bf8c485e24d554566f11`                            |
| Safety report version      | 3                                                                                                   |

安装包和 EXE 的 Authenticode 状态均为 `NotSigned`。最后一次 verifier 由上述实际安装 EXE 执行，确认当前进程就是 manifest 中的 installed binary，Import/Recycle 两份准入均重新验签通过；此后没有重新构建。

## 安装、升级与数据保留

- 从原 current-user v1.0.0 原位升级至 v1.0.1，通过。
- 安装位置为当前用户 LocalAppData 默认目录，不在 D 盘。
- 卸载前将现有约 20 MB AppData 完整备份到 G 盘专属 Run；源树与备份树 SHA-256 均为 `57878f10e9d8541d212b27f65ce4c0071770b954830762547b748a75c832a84b`。
- 受控卸载后程序 EXE 和 uninstaller 均移除，AppData 目录及其树 SHA-256 不变。
- 使用同一冻结安装包重装后，安装 EXE SHA-256 仍为 `bc8557…01d8`。最终保留该已验证安装状态。

## 数据与磁盘边界

- 最终 Run：`G:\test\local-photo-manager-phase8\fpm-phase8-run-40157cfe-9e18-44f2-80cf-bab09ff6fb9a`。
- 所有测试对象均为生成或明确可丢弃数据；没有对用户唯一照片执行 Import、Recycle 或删除。
- `G:\test\Fuji-XM5` 未作为输入；验收后的目录属性、创建时间和修改时间与预检快照一致。
- 最终 Run 共 200,103 个文件，实际分配 92,995,951,824 字节（86.609 GiB），低于 300,000,000,000 字节硬上限。
- 最终 G 盘可用 523,677,638,656 字节（487.713 GiB），继续满足 100,000,000,000 字节安全余量。
- D 盘只承载源码、构建产物和小型交付证据，不承载阶段 8 测试夹具。

## 规模、机械盘 I/O 与性能

| 项目                |                   实测 | 门槛/结论                    |
| ------------------- | ---------------------: | ---------------------------- |
| Photo               |                100,000 | 精确通过                     |
| Asset               |                200,000 | 精确通过                     |
| Asset 逻辑容量      | 1,258,291,200,000 字节 | ≥ 1 TiB，通过                |
| 介质 seek penalty   |                 `true` | Windows 设备属性证明为机械盘 |
| 夹具创建            |             600,612 ms | 记录项                       |
| 数据库填充          |             174,077 ms | 记录项                       |
| 时间线降序/升序     |           414 / 394 ms | 稳定顺序与查询通过           |
| 实际顺序写/读       |  各 2,147,483,648 字节 | 13,508 / 1,277 ms            |
| 非缓冲随机读        |        67,108,864 字节 | 99,200 ms                    |
| 启动到可操作        |               1,998 ms | ≤ 3,000 ms                   |
| 缓存命中 p95        | 0 ms（计时器分辨率下） | ≤ 150 ms                     |
| 未缓存反馈/预览 p95 |             0 / 135 ms | ≤ 100 / 2,000 ms             |
| D/R p95             |                  43 ms | ≤ 250 ms                     |
| Working set         |        38,236,160 字节 | ≤ 1.5 GiB                    |
| 后台退化比          |                    0.0 | ≤ 2.0                        |
| 取消响应            |                  62 ms | ≤ 2,000 ms                   |

全部性能门槛通过。1 TiB 是 NTFS 稀疏逻辑容量；报告没有把它表述为实际写满或读取 1 TiB。

## Import、Recycle 与运行时安全门

- 健康启动不读取人工确认或外部 admission，Import/Recycle 默认同时 `Open`。
- `require_feature()` 与 `revalidate()` 在同一锁内同步读取 gate 状态、generation、fault latch、实时 writer health、数据库 ReadWrite mode、gate instance 和 feature；未通知的 writer poisoning、错误 feature、错误实例和旧 generation 全部被拒绝。
- 共享故障同时关闭 Import/Recycle，fault latch 在进程内单调且必须安全重启后重新通过启动检查。
- gate 只阻止新的物理动作。Import publish 或 Recycle Shell 动作完成后，即使 gate 同时关闭，观察到的结果仍尝试提交 Fact；提交后才决定是否开始下一文件。
- Import：源卷写请求 0；源文件不变；既有目标逐字节不变；成功发布与源一致；CREATE_NEW/no-overwrite 和发布前故障注入通过；没有永久删除。
- Recycle：强制 flags 不含永久删除；每次 STA 调用只处理一个 asset；2 个生成文件均收到非空回收对象证据；Windows 回收站恢复后哈希一致；Shell 未中止。
- writer 在物理动作后失效时不伪造终态，也不释放 Intent、Reservation 或 Lease；启动协调观察结果后收敛。跨 gate 实例的 permit 在任何 Intent 或 Shell 动作之前被拒绝。

## SQLite、缓存和 UI

- SQLite：WAL、`synchronous=FULL`、FK、有界 checkpoint（3229/3229 frames）、一致备份、restore-to-new-file、schema 1–8 逐级迁移、损坏只读、AppData `SQLITE_FULL` writer poisoning 和后续写拒绝全部通过。
- Cache：默认 32 GiB、绝对上限 64 GiB、`max(20 GiB, 10%)` 余量规则通过；未知文件保留；只淘汰已证明所有权的对象；重建使用新 ordinal 与新路径。
- UI：100% / 1440×900、125% / 1280×720、150% / 1100×700 三组均无横向溢出、无对比度违规，键盘方向导航和焦点可见性通过。
- 冻结源码质量门：前端 39/39、E2E 3/3、Rust 125 通过、0 失败；2 项外部条件测试显式忽略，相关真实回收站和 G 盘 seek-penalty 已由安装候选套件执行。Clippy `-D warnings` 和安全审计通过。

## 验收、缺陷与交付

- 产品验收矩阵：27/27。
- 未解决 Critical：0；未解决 High：0。
- 一个无数据安全影响的 Low 测试稳定性项已缓解：冻结门固定单 worker，39 项前端断言全部通过。
- Import 与 Recycle 安全报告分别绑定同一 installed EXE、installer、locks、schema、迁移链、候选 manifest 和报告 checksum。
- 健康环境下真实 Import/Recycle 不再需要软件内人工解锁；自动故障 gate 始终保留。使用前操作清单不是权限开关，也不能绕过自动 gate。
- 没有 push、上传或远端发布。

机器证据和安装包位于 `artifacts/phase-eight-v1.0.1/`。已知限制见 `phase-eight-known-limitations.md`，首次真实照片操作前请执行 `real-photo-enable-checklist.md`。
