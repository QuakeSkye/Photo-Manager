# Filmstrip 尺寸、等比预览与详情栏折叠实施计划

日期：2026-08-07
依据：`docs/superpowers/specs/2026-08-07-filmstrip-layout-and-inspector-design.md`

## 1. 实施边界

本计划只修改阶段 4 已存在的前端布局和交互，不增加 Rust 命令、数据库字段、元数据解析或阶段 5 功能。图库与待删除页共用新的 Filmstrip；Accordion 只用于图库详情栏。测试不得导入、删除照片或调用 Photoshop。

## 2. 任务一：锁定 Filmstrip 虚拟尺寸和等比规则

涉及文件：

- `src/features/library/VirtualFilmstrip.test.tsx`
- `src/features/library/VirtualFilmstrip.tsx`
- `src/app/styles.css`

步骤：

1. 先把 VirtualFilmstrip 测试期望从旧的 `82px` 行步长改为 `112px`，覆盖 1000 项轨道高度、远距离滚动后的第 101 项定位，以及跳转到最后一项仍被虚拟窗口渲染。
2. 增加对 Filmstrip 根元素 CSS 自定义属性的断言，锁定 `106px` 卡片高度和 `112px` 行步长来自同一组组件常量。
3. 运行该测试并确认旧实现失败。
4. 在 `VirtualFilmstrip.tsx` 中定义 `FILMSTRIP_ITEM_HEIGHT = 106` 和 `FILMSTRIP_ROW_HEIGHT = 112`，所有轨道高度、可见区间、选中项滚入视口和 `translateY` 统一使用行步长。
5. 通过 Filmstrip 根元素的内联 CSS 自定义属性把卡片高度和行步长交给样式层，避免 TypeScript 与 CSS 出现互不关联的魔法数。
6. 在 `styles.css` 中把缩略图卡片高度改为自定义属性，把 `.preview-image.thumbnail` 从 `object-fit: cover` 改为 `contain`，使用中性背景承接比例留白，并把纵向间距收紧为 `6px`。
7. 保留 `overflow-x: hidden`、稳定滚动条槽、按钮左右 `6px` 留白和虚拟 overscan。
8. 运行 `npm test -- src/features/library/VirtualFilmstrip.test.tsx`，确认测试通过。

## 3. 任务二：实现三栏响应式宽度和选中质感

涉及文件：

- `src/app/styles.css`
- `src/app/App.test.tsx`

步骤：

1. 把 `.photo-workspace` 列宽改为 `clamp(160px, 15vw, 180px) minmax(0, 1fr) clamp(220px, 18vw, 260px)`。
2. 删除 `max-width: 1200px` 中针对照片工作区的旧 `96px / 自适应 / 260px` 覆盖；设置页与扫描页的 `.tool-workspace` 断点保持不变。
3. 把离线和扫描横幅的 `left`、`right` 同步改为与三栏相同的 clamp 表达式，防止横幅压住 Filmstrip 或详情栏。
4. 为普通选中项增加蓝色主描边、浅蓝内描边、白色背景和轻阴影；为待删除选中项提供对应的红色内外层级。
5. 保留独立的 `:focus-visible` 可见焦点，不让焦点环覆盖选中语义。
6. 在现有应用测试中保留并运行“点击 Filmstrip 后 D/R/方向键仍生效”的回归用例；确认布局改动没有改变按钮焦点和全局快捷键例外规则。

## 4. 任务三：实现图库详情 Accordion

涉及文件：

- `src/features/library/PhotoInspector.tsx`
- `src/features/library/PhotoInspector.test.tsx`（新增）
- `src/app/styles.css`

步骤：

1. 新增 PhotoInspector 组件测试，使用包含 JPG+RAF、拍摄时间、文件时间、相机型号和容量的 `PhotoDetail` fixture。
2. 测试两个分组按钮“时间与文件”和“照片参数”初始 `aria-expanded="true"`，且各自的 `aria-controls` 指向存在的内容容器。
3. 测试两个分组可独立折叠和重新展开；折叠一个分组不影响另一个分组。
4. 测试折叠后 Photoshop、打开当前文件位置和待删除按钮仍存在，证明操作区不属于 Accordion。
5. 运行新增测试并确认旧实现失败。
6. 在 PhotoInspector 内实现轻量 `MetadataAccordion`：使用原生 `button`、`useId`、局部 `useState(true)` 和稳定内容容器，不引入第三方依赖。
7. 文件名与 JPG/RAF 徽标保持常显；“时间与文件”承载拍摄时间、参考文件时间和容量；“照片参数”承载相机型号及现有参数行，并在标题处显示相机型号摘要或“未记录”。
8. 两个分组每次重新进入图库时默认展开，状态不写入设置、数据库或操作记录。
9. 添加 Accordion 标题、箭头、摘要、内容和折叠状态样式；详情滚动区与底部动作区现有 flex 边界保持不变。
10. 运行 `npm test -- src/features/library/PhotoInspector.test.tsx src/app/App.test.tsx`。

## 5. 任务四：待删除页和业务边界回归

涉及文件：

- `src/app/App.test.tsx`
- `src/features/pending-delete/PendingDeleteView.tsx`（仅在测试揭示必要调整时修改）

步骤：

1. 确认待删除页自动获得共享的新 Filmstrip 尺寸、`contain` 预览和红色选中样式。
2. 保持待删除详情栏为原有复核结构，不加入图库 Accordion。
3. 运行现有断言，确认待删除页不渲染 Photoshop 和打开文件位置按钮。
4. 确认图库 JPG-only、RAF-only、JPG+RAF 的 Photoshop 可用性规则未改变。
5. 不运行真实 Photoshop，不选择外部程序，不导入或删除照片。

## 6. 任务五：格式、自动测试和布局验收

步骤：

1. 运行 `npm run format:check`；如仅本次文件不符合格式，使用项目 Prettier 对明确文件执行机械格式化。
2. 运行 `npm run lint`。
3. 运行 `npm run test:ui` 和 `npm run test:e2e`。
4. 运行 `npm run build`。
5. 运行 `npm run test:phase4`，覆盖 Rust、Clippy、安全审计和阶段 4 报告；不得用测试绕过业务边界。
6. 在 `1100×700`、`1200×800`、`1440×900` 三个视口检查图库和待删除页：
   - Filmstrip 和详情栏计算宽度满足 clamp 规则；
   - Filmstrip `scrollWidth <= clientWidth`；
   - 横向、纵向和非标准比例预览均完整显示；
   - 中央大图不裁切；
   - Accordion 展开/折叠不遮挡底部动作区；
   - 点击缩略图后 `D/R/←/→` 仍生效。
7. 若当前浏览器安全策略不允许自动化访问 localhost，记录该限制，使用自动测试、构建和可执行的本地 UI 检查完成其余验证，不改用未获授权的浏览器绕过。

## 7. 任务六：提交与交付

1. 检查 `git diff --check` 和最终 diff，只纳入本计划对应的规格、计划、源代码与测试；保留用户现有未跟踪文件。
2. 创建本地实现提交，不推送远端。
3. 汇报改动、测试结果、任何无法自动完成的视觉检查，以及本地提交哈希，等待用户验收。
