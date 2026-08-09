# 照片参数补全与画布适配缩放实施计划

日期：2026-08-09
依据：`docs/superpowers/specs/2026-08-09-photo-exif-and-fit-zoom-design.md`

## 1. 实施边界

本计划只实现已确认的两组改动：补齐照片详情栏的 8 项参数，以及为中间画布增加“适合窗口 / 100%”双击切换。原始 JPG/RAF 始终只读；不引入外部 ExifTool 进程；不增加缩放按钮、滑杆、任意比例、拖拽平移或自定义胶片配方名称；不改变导入、配对、待删除、回收站、Photoshop 和文件位置动作的安全语义。

工作区现有未跟踪文件 `.superpowers/**` 与 `scripts/ui-check.mjs` 属于用户，实施、格式化、暂存和提交时均明确排除。

## 2. 任务一：新增版本化拍摄参数表

涉及文件：

- `src-tauri/migrations/0009_photo_capture_metadata.sql`（新增）
- `src-tauri/src/db/migration.rs`
- `src-tauri/src/db/tests.rs`

步骤：

1. 先在数据库测试中把最新版本期望改为 9，并增加从 v8 迁移后的结构断言：新表、复合主键、Asset Revision 外键、合理数值约束和不可更新/不可删除触发器均存在。
2. 增加迁移链测试，期望 manifest 最后一项为 `0009_photo_capture_metadata.sql`；先运行相关 Rust 测试，确认因迁移尚不存在而失败。
3. 新建迁移 0009，创建 `asset_capture_metadata`：
   - 主键：`asset_id, asset_version, parser_version`；
   - 可空字段：镜头、光圈分子/分母、快门分子/分母、ISO、焦距分子/分母、像素宽高、胶片模拟稳定 key、胶片模拟原始代码；
   - 外键关联 `asset_revision(asset_id, asset_version)`；
   - 分母非零、ISO/尺寸为正、parser version 为正；
   - 行写入后禁止 UPDATE/DELETE。
4. 在 `migration.rs` 注册迁移、把 `LATEST_SCHEMA_VERSION` 改为 9，并更新迁移 manifest 断言。
5. 运行数据库迁移测试，确认新数据库和 v1–v8 前向迁移全部通过；不得修改既有迁移文件或既有迁移 checksum。

验证命令：

```powershell
npm run test:rust -- db::tests
```

## 3. 任务二：扩展安全 EXIF 与 Fuji MakerNote 解析器

涉及文件：

- `src-tauri/src/media/exif.rs`
- `src-tauri/src/media/mod.rs`

步骤：

1. 先增加表驱动解析测试，覆盖：
   - 小端和大端 TIFF；
   - `LensModel`、`FNumber`、`ExposureTime`、`PhotographicSensitivity`、`FocalLength`、`PixelXDimension`、`PixelYDimension`；
   - SHORT、LONG、RATIONAL 和必要的 SRATIONAL 安全读取；
   - inline value 与 offset value；
   - 越界 offset、恶意 count、整数溢出、错误字段类型和零分母；
   - JPEG SOF 尺寸回退；
   - 一个字段损坏不吞掉其他已解析字段。
2. 增加 Fuji MakerNote 夹具，覆盖 `FilmMode (0x1401)` 的 Provia、Velvia、Classic Chrome、Eterna、Classic Negative、Bleach Bypass、Nostalgic Neg、Reala ACE，及 `Saturation (0x1003)` 的 Monochrome/Acros 回退、未知代码和无 MakerNote。
3. 运行 media 测试并确认旧解析器不能满足新断言。
4. 引入内部结构化类型：经过约分/验证的 Rational、稳定的胶片模拟 key 与原始代码、完整的 Capture Metadata。所有数值保持原始语义，不在解析器内生成 UI 字符串。
5. 把通用 IFD 读取改造成按字段类型安全取值的最小解析器；所有 slice、offset、count、乘法和类型转换先检查边界，再读取数据。
6. 调整 JPEG 段遍历：取得 EXIF 后仍能在需要时读取 SOF 尺寸，但不扫描图像压缩数据；遇到 SOS/EOI 即停止。
7. 实现 Fuji MakerNote 头部和相对 offset 规则，优先 `0x1401`；只有彩色 FilmMode 未可靠识别且辅助代码明确属于黑白变体时，才使用 `0x1003` 回退。
8. 把 `MEDIA_PARSER_VERSION` 提升到 2。RAF 仍只定位内嵌 JPEG 并在 bounded reader 内解析，不读取完整 RAF。
9. 运行 media 单元测试以及既有 RAF/EXIF 测试，确认旧时间、相机和序列号解析无回归。

验证命令：

```powershell
npm run test:rust -- media
```

## 4. 任务三：把新增参数接入扫描与不可变目录事实

涉及文件：

- `src-tauri/src/catalog/mod.rs`
- `src-tauri/src/catalog/scanner.rs`
- `src-tauri/src/catalog/phase_one_tests.rs`
- `src-tauri/src/catalog/mod.rs` 中现有 catalog 测试模块

步骤：

1. 先增加目录测试：新 Asset 扫描写入 parser v2 参数；同一文件重复扫描不产生重复行；未变化的旧 Asset 在缺少 v2 参数时能够补写；更高 parser version 可与旧版本并存；原有 Asset/Photo version 和配对关系不变化。
2. 增加取消后续扫测试：已经提交的参数记录保持有效，未处理项在下一次扫描继续补齐。
3. 运行 catalog 测试，确认新增断言失败。
4. 扩展 `CatalogObservation`，携带结构化 Capture Metadata，不把前端展示字符串带入 catalog 层。
5. 新增 `ensure_capture_metadata`：按 Asset version + parser version 查询，缺失时插入；存在时不写。新 Asset 与 `unchanged` Asset 两条路径都调用该函数。
6. 保留现有 `asset_media_metadata` 行及其不可变语义；相机、拍摄时间和序列号的既有逻辑不做破坏性搬迁。
7. 扫描报告记录本轮检查文件数、新增参数记录数、总耗时和处理速率。数据用于诊断，不新增阻塞用户的固定耗时门槛。
8. 保证单个字段缺失不产生 Scan Issue；只有 EXIF/TIFF 主结构无法安全读取时沿用现有文件级警告。
9. 运行 catalog、scanner、pairing 和数据库测试。

验证命令：

```powershell
npm run test:rust -- catalog
```

## 5. 任务四：扩展照片详情命令并确定 JPG/RAF 合并顺序

涉及文件：

- `src-tauri/src/commands/library.rs`
- `src/shared/commands/library.ts`
- 相关 Rust 命令测试或 `tests/e2e/capability-boundary.test.ts`

步骤：

1. 先增加 `get_photo_detail` 测试夹具：
   - JPG 和 RAF 参数均完整但值不同，结果逐字段优先 JPG；
   - JPG 某字段为空时只回退该字段到 RAF；
   - RAF-only、JPG-only、未知胶片代码和全部缺失；
   - 数据库查询顺序变化不会改变结果。
2. 扩展 Rust `PhotoDetail` 序列化结构，返回镜头、结构化光圈/快门/焦距、ISO、像素宽高、胶片模拟 key 与原始代码。
3. 调整详情 SQL，显式取得当前支持的最高 parser version，并通过稳定的 `ORDER BY CASE media_kind WHEN 'Jpg' ...` 或等价代码保证 JPG 优先。
4. 在命令层执行逐字段 first-non-null 合并；尺寸只有宽高同时可靠时才返回。
5. 扩展 TypeScript `PhotoDetail` 类型和 demo fixture，使用与 Rust camelCase 序列化一致的可空结构。
6. 保持详情请求只查询 SQLite，不在照片切换时打开原文件；Root 离线时仍可返回已经持久化的参数。
7. 运行 Rust 命令测试、TypeScript 类型检查和能力边界测试。

验证命令：

```powershell
npm run test:rust -- commands::library
npm run lint
npm run test:e2e
```

## 6. 任务五：展示并格式化右侧 8 项参数

涉及文件：

- `src/features/library/PhotoInspector.test.tsx`
- `src/features/library/PhotoInspector.tsx`
- `src/app/styles.css`

步骤：

1. 扩展 PhotoInspector fixture，先断言 8 行固定顺序以及如下格式：`F2.8`、`1/500 秒`、`ISO 400`、`35 mm`、`6240 × 4160`、`Classic Chrome`。
2. 增加格式边界测试：大于一秒的快门、不能整齐表示为倒数的快门、非整数焦距、缺失字段、未知胶片代码。断言缺一个字段不会让其他字段消失。
3. 运行组件测试，确认现有硬编码“未记录”实现失败。
4. 在 PhotoInspector 中增加小而纯的格式化函数；根据原始数值生成 UI 文本，不制造原数据没有的精度。
5. 把“照片参数”内容改为确认的 8 行，保留 Accordion 的 `aria-expanded`、`aria-controls`、默认展开和独立折叠行为。
6. 调整低高度样式：收紧身份区、Accordion 和行间距；保持 `.inspector-scroll` 可滚动、底部 `.library-actions` 固定，任何参数行都不使用 `display: none` 隐藏。
7. 运行 PhotoInspector 与 App 测试。

验证命令：

```powershell
npm test -- src/features/library/PhotoInspector.test.tsx src/app/App.test.tsx
```

## 7. 任务六：实现可测量的适合窗口与 100% 画布

涉及文件：

- `src/features/library/PhotoCanvas.test.tsx`（新增）
- `src/features/library/PhotoCanvas.tsx`
- `src/features/library/preview.tsx`
- `src/test/setup.ts`
- `src/app/styles.css`

步骤：

1. 为测试环境提供最小 `ResizeObserver` mock；不得把仅测试需要的逻辑带入生产组件。
2. 先新增 PhotoCanvas 组件测试，覆盖：
   - 竖图和横图按 `min(1, availableWidth/imageWidth, availableHeight/imageHeight)` 完整适配；
   - 小图不放大；
   - 比例状态显示 `适合窗口 23%`，布局使用未取整比例；
   - ResizeObserver 回调后重新计算；
   - 双击已加载图片进入/退出 100%，单击、双击空白、加载占位和不可用占位不切换；
   - 切换 `photoId/photoVersion` 恢复适合窗口并清除滚动；
   - 100% 初始滚动位置位于图片中心；
   - 加载中、不可用和空图库隐藏比例文字。
3. 运行新测试并确认旧组件失败。
4. 扩展 `PreviewImage` 的最小接口，在 `<img onLoad>` 后把 `naturalWidth/naturalHeight` 传给 PhotoCanvas，并允许把双击处理器只绑定到真实图片；缩略图调用方不受影响。
5. PhotoCanvas 维护 `fit | actual` 状态、自然尺寸、可用尺寸和滚动容器 ref。切换 Photo 时重置状态；用 ResizeObserver 观察内容区。
6. 使用实际布局宽高设置图片显示盒尺寸，不使用只改变绘制而不改变滚动范围的 CSS transform。fit 模式居中且 overflow hidden；actual 模式使用自然像素尺寸和内部 auto overflow。
7. 进入 actual 后在布局完成的时机设置 `scrollLeft/scrollTop` 到中心；组件卸载或快速换图时取消过期回调。
8. 右下角添加不可点击、不可聚焦的 `.zoom-status` 一行文字；不增加 button、range、select 或 click handler。
9. 尊重 `prefers-reduced-motion`，不为缩放切换新增动画依赖。
10. 运行 PhotoCanvas、Preview、App 相关测试。

验证命令：

```powershell
npm test -- src/features/library/PhotoCanvas.test.tsx src/app/App.test.tsx
```

## 8. 任务七：按区域分流滚轮并完成集成回归

涉及文件：

- `src/features/library/LibraryView.tsx`
- `src/features/library/PhotoCanvas.tsx`
- `src/app/App.test.tsx`

步骤：

1. 先扩展 App 测试：fit 模式中间滚轮切换照片；actual 模式中间滚轮不切换且不被 `preventDefault`；actual 模式下 Filmstrip 滚轮仍切换照片；键盘方向键仍可切换并使新照片恢复 fit。
2. 运行测试并确认现有统一 `handleWheel` 绑定无法满足 actual 模式。
3. 让 PhotoCanvas 只在 fit 模式调用现有照片导航 handler；actual 模式不安装/不调用该 handler，把滚轮交给内部滚动容器。
4. 保持 VirtualFilmstrip 继续调用照片导航 handler，不让中间画布缩放状态进入 Filmstrip。
5. 右侧详情滚轮保持局部滚动，不冒充照片导航入口。
6. 在 LibraryView 提供明确的 wheel accumulator/lock 重置函数，并在画布进入 actual 和切换 Photo 时调用，防止旧累计量触发意外跳图。
7. 运行全部前端测试和 e2e 能力边界测试。

验证命令：

```powershell
npm run test:ui
npm run test:e2e
```

## 9. 任务八：格式、全量验证与视觉验收

涉及文件：本次已确认的源代码、迁移、测试、设计与计划；不纳入用户未跟踪文件。

步骤：

1. 对本次明确修改的文件运行 Prettier/Rustfmt，再运行 `npm run format:check`；不得用全仓格式化覆盖用户文件。
2. 运行 `npm run lint`、`npm run test:ui`、`npm run test:e2e`、`npm run test:rust`、`npm run clippy`、`npm run safety:audit` 和 `npm run build`。
3. 确认 schema version 9、迁移链 checksum 和安全指纹相关测试已按新 schema 真实更新；不得沿用或修改旧 v1.0.1 发布候选证据来声称新构建已验收。
4. 使用 demo/生成图片在 `1100×700`、`1200×800`、`1440×900` 检查：
   - 横图、竖图、小图完整适配；
   - 适合比例文字正确且不遮挡滚动条；
   - 双击进入 100% 后初始居中并可滚动；
   - 中间与 Filmstrip 的滚轮语义正确；
   - 右侧 8 项参数可访问，底部动作保持可见；
   - 页面无由原图自然尺寸造成的全局横向滚动。
5. 若存在用户提供或可丢弃的真实 Fuji JPG/RAF 副本，仅做只读解析对照；检查 X-M5 参数和胶片模拟。没有真实夹具时明确记录该视觉/元数据验证尚需用户验收，不伪造结论。
6. 记录扫描诊断数据：测试文件数、补齐数、总耗时和每秒处理数；把结果与原扫描基线对比，异常退化必须在交付前定位。
7. 运行 `git diff --check`，审阅最终 diff 和 `git status`，只暂存本任务文件。
8. 创建本地实现提交，不 push、不发布、不安装；汇报提交 hash、测试结果、真实照片验证边界和用户需要复验的项目。

全量验证命令：

```powershell
npm run format:check
npm run lint
npm run test:ui
npm run test:e2e
npm run test:rust
npm run clippy
npm run safety:audit
npm run build
```

## 10. 完成判定

只有当数据库迁移、解析器、补充扫描、详情命令、8 项参数、竖图适配、双击 100%、区域化滚轮和既有安全回归全部通过时才提交实现。任何真实 Fuji MakerNote 无法可靠映射的代码必须按“未知模式（代码）”降级，不得猜测；任何无法自动完成的真实照片或 DPI 验收必须在交付中明确列出。
