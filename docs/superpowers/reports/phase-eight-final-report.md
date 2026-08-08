# 阶段 8 最终测试报告

日期：2026-08-09
结论：通过（27/27；未解决 Critical 0；未解决 High 0）

## 冻结候选

| 对象 | 结果 |
|---|---|
| 安装包 | `Fuji 照片管理器_1.0.0_x64-setup.exe`，3,569,164 字节 |
| 安装包 SHA-256 | `571e7a7afa12cb1b53e8563b36369cfb42ef332bdc6898f7c754bccb2a20edd8` |
| 实际安装 EXE | `C:\Users\15684\AppData\Local\Fuji 照片管理器\local-photo-manager.exe`，14,589,952 字节，版本 1.0.0 |
| 已安装 EXE SHA-256 | `cc773ddd590dd01acfc222afa8b2e48a5df3f726d4a2a81fe750a38edc716ce6` |
| 候选 manifest SHA-256 | `ae7df348a9369f9bf21a82a5f78b0d53de944861d387ce2bd3309dc2c534db6b` |
| `package-lock.json` | `1544f8bffbaff8cf29739c0992bc174ba8e8d34f0ffb7da3aead10827c1caf50` |
| `Cargo.lock` | `36d64528b649b919ff9749abc5658e8ff8bcf97513c1973bd8ed8be08ff3d930` |
| schema | v8；`e78fd625669f4b76629a25a210cbd5de70eaf35de54605430c2607c29d969ca2` |
| 迁移链 | 8 份；`3d736ebf30cf68c566585e0ae659292e417e752b3464bf8c485e24d554566f11` |

安装包和 EXE 的 Authenticode 状态均为 `NotSigned`。全部候选测试结束后再次计算安装包、EXE、两份锁文件，并由已安装 EXE 复核 schema、迁移和两份独立准入，结果全部匹配。此候选之后没有重新构建。

## 数据边界

- 最终 Run：`G:\test\local-photo-manager-phase8\fpm-phase8-run-f5a9b86f-1d2b-40df-9654-4a44a11dbc9b`。
- 测试数据全部为生成或明确可丢弃数据；没有对用户唯一照片执行导入、回收或删除。
- `G:\test\Fuji-XM5` 未作为测试输入，也未被修改或删除。
- D 盘只保存源码、构建产物和最终小型证据，没有作为阶段 8 测试卷。
- I/O 后 Run 实际分配为 2,350,992,531 字节；限制为 300,000,000,000 字节。当时 G 盘可用 529,361,235,968 字节。

## 规模、机械盘 I/O 与性能

| 项目 | 实测 | 门槛/结论 |
|---|---:|---|
| Photo | 100,000 | 精确通过 |
| Asset | 200,000 | 精确通过 |
| Asset 逻辑容量 | 1,258,291,200,000 字节 | ≥ 1 TiB，通过 |
| 介质 seek penalty | `true` | 已证明为机械硬盘 |
| 夹具创建 | 599,269 ms | 记录项 |
| 数据库填充 | 182,334 ms | 记录项 |
| 时间线降序/升序 | 396 / 386 ms | 查询与稳定顺序通过 |
| 实际顺序写/读 | 各 2,147,483,648 字节 | 14,137 / 1,337 ms |
| 非缓冲随机读 | 67,108,864 字节 | 99,557 ms，通过真实 HDD 校准 |
| 启动到可操作 | 2,226 ms | ≤ 3,000 ms |
| 缓存命中 p95 | 0 ms（计时器分辨率下） | ≤ 150 ms |
| 未缓存反馈/预览 p95 | 0 / 166 ms | ≤ 100 / 2,000 ms |
| D/R p95 | 50 ms | ≤ 250 ms |
| 峰值 working set | 38,309,888 字节 | ≤ 1.5 GiB |
| 后台退化比 | 0.0 | ≤ 2.0 |
| 取消响应 | 57 ms | ≤ 2,000 ms |

全部 8 项性能阈值通过。1 TiB 是 NTFS 稀疏逻辑规模，实际吞吐仅按报告中的真实分配样本表述。

## 安全与耐久性

- Import：源卷写请求 0；源文件不变；既有目标逐字节不变；成功发布与源一致；no-overwrite 拒绝；发布前故障注入拒绝；未使用永久删除。
- Recycle：强制回收站 flags 不含永久删除；2 个文件均收到非空逐文件回调；Windows 回收站恢复后内容哈希一致；Shell 未中止。
- Cache：32 GiB 默认、64 GiB 绝对上限、20 GiB/10% 取大余量规则通过；未知文件保留；只淘汰已证明所有权的对象；重建使用新 ordinal 和新路径。
- SQLite：`WAL`、`synchronous=FULL`、FK、受控 checkpoint（3229/3229 frames）、一致备份、restore-to-new-file、schema 1–8 前向迁移、损坏只读、AppData `SQLITE_FULL` 后共享 writer 中毒和后续写拒绝全部通过。
- 源码质量门：前端 38、E2E 3、Rust 119 通过；2 项外部条件测试被明确忽略，其中真实回收站动作已由安装后候选套件执行，G 盘介质属性已由候选规模测试执行。Clippy `-D warnings` 与安全审计通过。

## DPI、键盘、对比度与窗口

已安装候选在以下矩阵通过：100% / 1440×900、125% / 1280×720、150% / 1100×700。三组均无横向溢出、无对比度违规，存在 19–21 个可见焦点目标，方向键改变当前照片，焦点保留在按钮上。候选截图和机器报告已保存在 `artifacts/phase-eight/`。

## 验收、缺陷与准入

- 产品验收矩阵：27/27。
- 未解决 Critical：0；未解决 High：0。
- 已记录一个无数据安全影响的 Low 测试稳定性项：并行 runner 下曾出现一次 1 秒 UI 测试等待超时；冻结门以单 worker 运行相同 38 项断言并全部通过。
- Import 与 Recycle 分别生成安全报告和准入；两份准入绑定同一候选 manifest，并由准确安装 EXE 反序列化、重新计算证据哈希后通过。
- 真实照片 Import 与 Recycle 仍分别关闭；本报告不构成启用授权。

机器证据入口：`artifacts/phase-eight/phase-eight-final-summary.json`、`acceptance-matrix.json`、`candidate-manifest.json`、`admission-verification.json`、`candidate-verification.json`。

已知限制见 `phase-eight-known-limitations.md`，真实照片启用前的独立确认见 `real-photo-enable-checklist.md`。没有执行远端 push、上传或发布。
