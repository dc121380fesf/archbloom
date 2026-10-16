# 场景 IR v0 —— 规范（schema 单一来源）

> 阶段一第一块砖。本文件与 `schema/scene_ir.v0.schema.json`、`crates/ir` 的 Rust 类型同源；
> 变更纪律：改 schema 必须三处同步 + 版本号递增 + 迁移说明。
> 设计教训来源：稳定 ID（agent 寻址）、显式单位/轴向（FBX 0.0254 鬼影）、shading_model（PBR 往返退化）。

## 设计原则

1. **场景图 + 稳定 ID**：每个对象有不可变 `id`（跨编辑存续、不复用），agent 与 op 一律按 id 寻址；
2. **显式物理语义**：`units`、`up_axis` 是场景级必填字段，禁止隐式默认（转换链规范化据此执行）；
3. **版本化**：`schema_version` 必填，读入端拒绝未知大版本，小版本走迁移函数；
4. **数据与视图分离**：IR 只存事实（几何/材质/变换），不存渲染器私有状态；
5. **可断言**：所有几何事实（面数/体积/尺寸）可从 IR 确定性导出，供验证回路做断言。

## 顶层结构

| 字段 | 类型 | 说明 |
|---|---|---|
| schema_version | string | "0.1" 起步 |
| units | enum | mm / cm / m / in / ft（必填） |
| up_axis | enum | y_up / z_up（必填） |
| name | string | 场景名 |
| objects | Object[] | 场景图节点 |
| materials | Material[] | 材质库（对象按 id 引用） |
| assets | AssetRef[] | 外部资产引用（贴图/嵌套模型） |
| meta | map | 来源/作者/备注（不参与几何断言） |

## Object（判别联合 by `kind`）

公共字段：`id`、`name`、`kind`、`parent`（id 或 null）、`transform`（T/R/S，R 用四元数避免万向锁）、`visible`、`meta`。

- **mesh**：`geometry`（positions/indices/uv_sets/normals）+ `material_slots[]`
- **group**：纯层级容器
- **instance**：`source`（引用 mesh/组的 id）+ 实例变换——资产复用（软装库的基础）
- **light**：`light_kind`（area/point/spot/sun）、`color`、`intensity`、`size`/`cone`（类型相关）
- **camera**：`lens_mm`、`sensor_mm`、`clip`

## Material

`id`、`name`、**`shading_model`（pbr_metallic_roughness / unknown，必填）**、`base_color`、`metallic`、`roughness`、`opacity`、`textures`（base_color/normal/roughness/metallic/ao，各为 AssetRef）。
往返纪律：经第三方工具后 `shading_model` 退化必须置 `unknown` 并由验证层报出，禁止静默沿用旧参数。

## AssetRef

`id`、`uri`（相对路径优先）、`kind`（texture/model/audio…）、`license`、`source`。资产授权信息进 IR——版权链随模型走。

## 几何事实断言接口（验证回路的锚点）

由 IR 确定性计算：对象计数、命名/层级树哈希、每网格顶点/面数、包围盒、UV 集完整性、材质绑定完整性。**任何 op 执行后，期望断言不成立必须显式报错——静默 no-op 是最高级缺陷。**
