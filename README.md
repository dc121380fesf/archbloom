<p align="center">
  <img src="assets/icons/icon_128.png" alt="Archbloom" width="96">
</p>

<h1 align="center">Archbloom</h1>

<p align="center"><b>agent-first 建筑 3D 建模工作台 —— 一句话进，一套交付物出。</b></p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-core-orange">
  <img alt="tests" src="https://img.shields.io/badge/tests-254%20green">
  <img alt="clippy" src="https://img.shields.io/badge/clippy-0%20warnings-blue">
  <img alt="license" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue">
</p>

<p align="center"><b>中文</b> · <a href="README.en.md">English</a></p>

> **写在前面**：AI 接入 3D 的个人尝试，期望可以用于真实的 3D 工作场景，目前可初步应用，
> 也可以用于大模型的 3D 建模与空间能力测评，个人低质作品，希望各位大佬提出意见。

agent 造方案、人来拍板：参数化体块与软装一键成场景，效果图 / 8K 全景 / VR 立体 / 漫游视频一次全出，
FBX / IFC / glTF 双向互转，30+ 建模渲染工具经 MCP 直接给 agent 用。

---

## 一图入魂

<p align="center">
  <img src="docs/screenshots/villa_hero.png" alt="精致二层小洋楼" width="82%">
</p>

<p align="center"><i>手写几何装配的二层小洋楼：坡屋顶 / 老虎窗 / 烟囱 / 柱廊 / 阳台栏杆阵 / 门窗开洞 / 程序化贴图 / 天空环境光 —— 全部由 op 脚本生成，路径追踪 512spp 出图。</i></p>

<p align="center">
  <img src="docs/screenshots/comparison.png" alt="多方案对比" width="82%">
</p>

<p align="center"><i>一份简报出 N 个方案变体 + 对比长图 —— agent 主操作、人审核的审稿面。</i></p>

## 一键直出（最小简报）

```json
{ "compose": {
    "template": "community_center",
    "name": "我的社区中心",
    "renderer": "auto",
    "deliverables": {
      "perspective": { "path": "out/view.png", "width": 1600, "height": 900 },
      "walkthrough": { "dir": "out/walk", "waypoints": [[2,26,2],[20,10,2]], "target": [10,8,3] }
    }
} }
```

```bash
archbloom-run my_scene.json
```

场地 / 方案 / 软装 / 交付物全自动，实测 34 秒出全套。模板：`community_center` / `school` /
`residence` / `park`；`variants` 数组一简报出 N 方案对比图。

## 能力总览

| | |
|---|---|
| **建模** | 参数化体块（楼层/开间/进深）、八类家具、门窗开洞、布置变换、布尔、薄墙隔断 |
| **表现** | CPU/GPU 双轨路径追踪（GPU ~50×）、8K 全景、VR 立体全景、漫游视频、方案对比长图 |
| **审阅** | 自包含 HTML 查看器（WebGL2 轨道 / 物体树 / 线框 / 双哈希事实面，零依赖离线可开） |
| **文档** | 桌面工作台带完整文件菜单（新建 / 打开 / 保存 / 另存为 / 导出）；`.al` 自包含存档把场景与贴图收进单文件，换机器直接开 |
| **互转** | FBX 双向（Blender 桥）、IFC 导入导出、glTF/GLB、3ds Max 链、场景 JSON |
| **agent 面** | MCP 服务（stdio）+ CLI + compose 编排层；结构化报错带步骤号，fail-closed |
| **精确内核** | 自研窄 B-rep：解析曲面 + 闭式体积 + 平面布尔 + **平面倒角**（C0.5）+ 边查询；**STEP 输出**（FreeCAD 独立回读验证） |

## 快速开始

1. **看场景**：双击 `archbloom-app`（评审视口），或浏览器打开导出的 `*_review.html`
2. **出场景**：`archbloom-run scene.json` 跑脚本，或让 agent 用 compose 简报直出
3. **接 agent**：`archbloom-mcp` 是 MCP 服务，接入即获全部工具

外部依赖（可选）：Blender（FBX 桥）、3ds Max（.max 链）、ffmpeg（视频装配）——
`check_deps.ps1` 一键检测，缺失时优雅降级、显式报错。

## 工程质量

- **254 测试全绿 / clippy 0 警告**；独立代码审计 15/15 清偿（含敌对输入面负例网）
- 确定性渲染轨（CPU）+ 速度轨（GPU）双轨制；稳定 ID 不复用、双哈希事实面、事务进出
- 自研核心：IR 场景格式、半边拓扑、路径追踪器、WebGL2 查看器、STEP P21 解析器

## 许可证

双许可 **MIT OR Apache-2.0**（见 LICENSE-MIT 与 LICENSE-APACHE，任选其一）—— Rust 生态惯例。第三方组件见 THIRD_PARTY.md。
