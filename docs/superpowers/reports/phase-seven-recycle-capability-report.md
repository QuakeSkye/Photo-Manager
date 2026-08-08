# 阶段 7：Windows 回收站工程能力测试记录

- 结果：PASS
- 范围：仅可丢弃 NTFS 测试 Root 上的 Recycle 工程能力
- 用户唯一照片操作：否
- 永久删除或降级：无
- 真实 FujiLibrary Recycle 准入：未生成
- 真实 FujiLibrary Recycle 门：未开启
- 阶段 8：未进入
- 远端推送：否
- Schema 版本：7
- Migration SHA-256：`922aaecc7a8b1af0139048f5679e84a812a178e830536ee116d39b31312e39be`
- 已测试 Release 二进制：`local-photo-manager.exe`
- 二进制 SHA-256：`813cd30700070ab915e7fb36c3abbccd12a64bba29bdc74f751246fd9f7591ef`
- Cargo.lock SHA-256：`cedf32ddb9ad738c08ce8637d2b0ee875083ef488af5bdd79af3d7fbdded3f93`
- package-lock.json SHA-256：`f1fa1c14e1d1ad87c6361f3e72003fa69f2534673efebd6cd534e78053213939`

## 验证项

- PASS — final confirmation freezes logical Photo count, physical file count and bytes
- PASS — TaskIntent, Batch, initial Attempt, Report, Lease and Reservations share one transaction
- PASS — BatchAsset copies only immutable DeletionMarkAssetSnapshot identity
- PASS — dedicated COM STA worker uses one IFileOperation per Photo
- PASS — recycle flags are mandatory and permanent-delete fallback is absent
- PASS — PreDeleteItem revalidates the frozen identity
- PASS — PostDeleteItem synchronously commits each RecycledFact before returning
- PASS — unknown, partial and database-failure outcomes stop conservatively
- PASS — retry creates a new Attempt and cancel preserves old facts
- PASS — startup reconciliation never automatically replays Shell
- PASS — pending entry is derived to zero only after all intents resolve
- PASS — UI confirmation includes all three frozen quantities and recycle-only warning
- PASS — real Windows STA pair recycle and restore test passed on disposable NTFS
- PASS — safety audit passed with no permanent-delete production path

本记录只证明当前精确构建在应用生成的可丢弃 NTFS TestRoot 上具备回收站工程能力。它不是阶段 8 的真实 FujiLibrary Recycle 准入，运行时仍会拒绝没有 TestRoot 标记的真实图库；测试后任何重新构建都会使本记录中的二进制哈希失效。
