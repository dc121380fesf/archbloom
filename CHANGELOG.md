# 变更日志

## 0.2.1 — 2026-10-17 · 体验两件 · 口径统一

- **视口升级：WebGL2 纹理视口**（原为 WebGL1 平色三角）。接上管线里早有但被壳层丢弃的 `uv` / 贴图索引 / 材质语义：贴图 × 材质底色 × 日光/天空环境光照（与路径追踪同口径近似），gamma 出图；按贴图分批渲染、异步加载，加载失败退回底色不致视口崩溃。
- **出图面板**：预设（透视 / 全景等距柱状 2:1）、分辨率 720p→4K、采样 16/64/128、渲染轨 auto/cpu/gpu；一键出图落到场景文件旁（`场景名_预设_宽x高_spp.png`），回报耗时与实际所用轨（auto 落 GPU 时明报适配器名）。校验面：尺寸钳 16384、spp 钳 4096、全景高度恒收敛半宽。
- **版本口径统一**：全部 crate 改为继承 `workspace.package.version`（原 `render` / `app` 写死 0.1.0 不随 workspace 走）；版本由 0.1.0 修正为 **0.2.1**，与 CHANGELOG / 包名 / 二进制属性三处对齐。
- **许可证**：`LICENSE` 占位改为标准双证 `LICENSE-MIT` + `LICENSE-APACHE`（MIT OR Apache-2.0），并给 `render` / `app` 补上 `license.workspace = true`（此前这两个 crate 未声明许可）。
- **工程债**：清理 rust 1.97 clippy 新口径 5 处（文档续行、`neg_cmp_op_on_partial_ord`、无用 format、类型复杂度）；`!(depth > 0.0)` 重写为 `!is_finite() || <= 0`，NaN/inf 口径保持 fail-closed 并收紧。
- **质量**：127 测试全绿，`clippy --workspace --all-targets -D warnings` 0 警告。

## 0.2.0 — 2026-10-16 · 更名版 · 开源发布

- **开源定证：双许可 MIT OR Apache-2.0**（替换内部预览版占位；Rust 生态惯例）。隐私体检通过：库内零命中、历史收拢为 noreply 单提交（API 上传的 166 笔 QQ 邮箱提交已成无引用悬空对象）。

- **产品更名：Archcore → Archbloom**（无中文名；经六候选占用调查为最优项，详见 README 命名备注）
- 用户可见面全量更名：productName/identifier/三进制（archbloom-app/run/mcp）/README/文档/Web 查看器标题/MCP 自报家门
- 内核工程代号 archcore-* 保留（crate/IR schema 兼容性），IR schema 不变更
- 新增：Hana 家风 UI 图标 13 枚（幻画师设计，已入 Tauri 图标位）；check_deps 依赖检测脚本

## 0.1.0 — 2026-10-16 · 首个封装版本

能力快照：

- **建模**：参数化体块、八类家具、门窗开洞、布置变换、布尔、薄墙/隔断
- **表现**：CPU/GPU 双轨路径追踪（GPU ~50×）、8K 全景、VR 立体全景、漫游视频（ffmpeg 装配）、方案对比长图
- **审阅**：自包含 HTML 查看器（WebGL2、物体树、双哈希事实面）
- **互转**：FBX 双向（Blender 桥）、IFC 导入导出、glTF/GLB、3ds Max 链、场景 JSON
- **一键直出**：compose 简报 + 四模板 + 多方案对比
- **质量**：127 测试全绿、clippy 0、独立审计 15/15 清偿

已知边界：曲面造型（旋转/放样/圆角）未支持；材质为 Lambert+镜面（粗糙度/透射待 L3）；
景观组件（地形/园路/水体/植被）挂起待启。
