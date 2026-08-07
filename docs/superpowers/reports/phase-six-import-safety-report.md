# 阶段 6：安全导入执行与恢复工程安全报告

- 结果：PASS
- 范围：仅安全导入执行与恢复；未进入阶段 7
- 真实文件夹具：可丢弃来源卷 + 自动生成测试 Fuji Root
- 用户唯一照片导入：否
- 来源写请求：0
- 并发抢占测试中既有正式目标变化：0
- 正式发布 ReplaceIfExists：FALSE
- 远端推送：否
- Schema 版本：6
- Migration SHA-256：`46251d356caebb66e7108cd13e207bbc412dccf2c985039e976e10e7cdd963ca`
- 已测试 Rust 二进制：`local_photo_manager-d755f2393db2edad.exe`
- 二进制 SHA-256：`226bab1f2bf2188a1a6aebec2b30f572d233216541ea98e35f03b7c63379322b`
- Cargo.lock SHA-256：`bad5fa5c7e6f458cfd429725043e706f02ad7af3b230d7bd9b6b9b1ffdf149c6`
- package-lock.json SHA-256：`55f5604eb9b670a245b117e9daa310e28c36a8ddf3ec4479221dd0a2dbc70b63`

## 验证项

- PASS — final Start Import creates TaskIntent, ImportJob and initial Confirmed generation
- PASS — all duplicate validation handles survive the one proof/result transaction
- PASS — plan changes create Proposed generation and stop before space/attempt
- PASS — exact one-per-attempt space gate uses max(10 GiB, ceil(total*2%))
- PASS — lease, attempt, report, reservations and Planned ownership are acquired atomically
- PASS — partial creation is CREATE_NEW and flushes before complete reread
- PASS — publish is handle-level and explicitly no-replace
- PASS — publish facts precede CatalogService indexing and formal files are never rolled back
- PASS — recovery coordinates Publishing and CleaningOwnedPartial before closing an interrupted attempt
- PASS — missing-after-cleaning is recorded without path-based deletion
- PASS — Planned recovery becomes NeedsAttention and cannot create a new incarnation
- PASS — UI has a separate final modal and exposes source write count
- PASS — real filesystem tests are restricted to generated TestRoot and disposable source fixtures
- PASS — race test proves an existing formal target and the partial both remain unchanged
- PASS — two-volume test proves source bytes/mtime and test-root marker remain unchanged
- PASS — duplicate test commits separate Validation and Execution proofs without changing either endpoint
- PASS — changed plan stops before SpaceCheck/Attempt and pre-start cancellation changes no file
- PASS — source process write counter is fixed at zero

本报告由 `npm run test:phase6` 在完整格式、静态检查、前端/端到端/Rust 测试、Clippy 与安全审计全部成功后生成。它不是对用户唯一照片执行导入的记录，也不开放回收站执行能力。
