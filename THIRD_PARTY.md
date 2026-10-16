# 第三方组件与许可见证

Archbloom（工程代号 archcore）使用以下第三方组件（均不改变其原许可）：

| 组件 | 许可 | 用途 |
|---|---|---|
| manifold-rust | Apache-2.0 | 布尔运算内核（外借，不自研） |
| wgpu | MIT OR Apache-2.0 | GPU 渲染（Vulkan/DX12/Metal） |
| naga | MIT OR Apache-2.0 | WGSL 着色器（wgpu 依赖） |
| image | MIT OR Apache-2.0 | 图像编解码 |
| rayon | MIT OR Apache-2.0 | CPU 并行渲染 |
| serde / serde_json | MIT OR Apache-2.0 | 序列化 |
| Tauri | MIT OR Apache-2.0 | 桌面壳 |
| rusqlite / SQLite | Public Domain | 场景存储（如启用） |

## 外部工具（不随包分发，按用户系统安装）

| 工具 | 许可 | 关系 |
|---|---|---|
| Blender | GPL-2.0-or-later | **独立进程桥接**（命令行调用），不链接、不分发 |
| 3ds Max / 3dsmaxbatch | Autodesk 专有 | 独立进程桥接；.max 转换走官方 FBX 导出通道 |
| ffmpeg | LGPL/GPL | 独立进程调用（视频装配），不链接、不分发 |

注：Autodesk FBX SDK 未使用（FBX 互转经 Blender 桥实现），无 FBX SDK 署名义务。
本清单随版本更新核对；如对许可有疑问请在分发前咨询法务。
