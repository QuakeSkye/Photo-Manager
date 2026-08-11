# 重复复核待删除选择界面实施计划

日期：2026-08-11  
基线：`7cd562652a5df0898727e8fe5856d54d70dbb937`  
依据：`docs/superpowers/specs/2026-08-11-duplicate-review-delete-selection-ui-design.md`

## 1. 实施边界

这是对已实现重复复核功能的增量修改。保留现有检测、结果 DTO、加入待删除命令、确认对话框和后端保留证明，只替换前端的去留选择模型、批量选择入口、文件行样式和底部操作栏布局。

涉及文件：

- `src/features/duplicate-review/DuplicateReviewView.tsx`
- `src/features/duplicate-review/DuplicateReviewView.test.tsx`
- `src/features/duplicate-review/selection.ts`（新增）
- `src/features/duplicate-review/selection.test.ts`（新增）
- `src/app/styles.css`

不修改 Tauri command、数据库 schema、重复证明、DeletionMark 或回收执行逻辑。

## 2. 任务一：抽离并测试选择规则

先在 `selection.test.ts` 写失败测试：

1. 全页面批量选择返回所有安全集合中可选的非建议保留成员。
2. 建议保留、不可选成员和没有可靠建议保留项的整个集合被跳过。
3. 部分选中时只报告剩余数量；全部合格项选中时切换为清除模式。
4. 清除模式移除页面全部选择，包括手动选择的建议项。
5. 手动选择最后一个可靠保留成员时被拒绝。
6. 建议项被手动选择后，提交选择会确定性地使用另一个未选中的可选成员作为保留端。
7. 文件数和容量按物理文件准确汇总。

在 `selection.ts` 实现无 React、无 I/O 的纯函数：

- 派生所有 Set 和成员索引；
- 手动切换选择并返回拒绝原因；
- 计算批量合格成员、剩余数量和 select/clear 模式；
- 执行全页面批量选择或全量清除；
- 为每个待删成员派生当前实际保留成员并构造 `DuplicateSubsetSelection`；
- 计算已选物理文件数量和总容量。

所有状态键继续使用 `assetId`，提交时同时携带既有 `assetVersion`；展示路径不参与授权。

## 3. 任务二：替换现有行级交互

在 `DuplicateReviewView.test.tsx` 增加失败测试：

1. 不再出现 `保留` radio 和“选择这一整套副本”按钮。
2. 每个成员只有一个动态无障碍名称的待删除复选框。
3. 建议项默认未选且显示绿色“建议保留”徽章，但仍可在安全时手动勾选。
4. 勾选行出现 `is-selected`、主文件名中划线和“待删除”文字；完整路径仍存在。
5. 尝试选中集合最后一个可靠保留成员时保持未选并显示错误。
6. “在资源管理器中查看”不改变选择。

实现步骤：

1. 删除 `retained` state、`onRetain` radio 和按目录整套选择 handler。
2. `DuplicateSet` 只渲染集合标题和 `DuplicateFileRow`。
3. 文件行按“复选框 / 文件信息 / 状态 / 资源管理器动作”四列布局。
4. 文件信息第一行显示文件名及建议徽章，第二行显示完整路径、大小和时间；仅选中文件名使用中划线。
5. 手动切换统一调用 `selection.ts` 的保护函数，拒绝结果进入可访问错误消息。

## 4. 任务三：增加全页面批量切换

增加 UI 测试：

1. 初始按钮显示 `勾选全部非建议保留（N）`。
2. 部分手动选中后显示 `勾选剩余非建议保留（N）`。
3. 首次点击选择所有家族、所有折叠状态中的合格成员，不只处理当前视口。
4. 全部合格项选中后显示 `取消全部勾选（N）`，再次点击清空页面全部选择。
5. 无合格成员时按钮禁用并显示明确文案。
6. 按钮暴露正确的 `aria-pressed`，操作结果通过 `role=status` 报告。

按钮放在六项结果摘要与照片族列表之间，使用次级按钮样式，不使用执行删除的危险色。

## 5. 任务四：把操作栏固定到窗口底部

将 `duplicate-selection-bar` 从 `duplicate-review-main` 内部移动为 `duplicate-review-screen` 的第三个根级网格子项：

```text
TopBar
可滚动 duplicate-review-main
固定 duplicate-selection-bar
```

样式要求：

- `.duplicate-review-screen` 使用 `52px minmax(0, 1fr) auto` 三行网格。
- 主内容独立滚动，底栏不再使用内容流中的 `position: sticky`。
- 底栏显示选择摘要、安全说明和带数量的 `加入待删除（N）…`。
- 零选择时显示 `尚未选择文件` 并禁用提交按钮，底栏不消失。
- 底栏使用不透明背景、顶部边框和向上轻阴影，确认 Modal 的层级继续高于底栏。

增加断言确认底栏是 `main` 的同级元素，而不是任意结果卡或结果列表的子元素。CSS/布局人工检查覆盖 1100 px 最小宽度和长路径。

## 6. 验证

按顺序运行：

```powershell
npm test -- src/features/duplicate-review/selection.test.ts
npm test -- src/features/duplicate-review/DuplicateReviewView.test.tsx
npm test -- src/app/App.test.tsx
npm run lint
npm run format:check
git diff --check
```

若完整前端测试成本可接受，再运行：

```powershell
npm run test:ui
npm run build
```

完成标准：现有重复检测和确认提交流程测试继续通过；页面不存在“保留”单选框；勾选语义、批量切换、安全保护、对齐样式和窗口底部操作栏均符合批准规格。
