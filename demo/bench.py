# -*- coding: utf-8 -*-
"""评测基准 v0 全量驱动 —— G2 验收数据生成器。
每任务三件套判据：几何事实断言（自动）/ 视觉确认（产出文件存在，人审待确认）/ 往返保真（自动对比）。
"""
import json
import os
import subprocess

EXE = r"D:\Word\archcore\target\debug\archcore-run.exe"
CWD = r"D:\Word\archcore"
OUT = r"D:\Word\arch0-spike\bench"
os.makedirs(OUT, exist_ok=True)

BATAK = r"D:\Word\arch0-spike\samples\max\opengameart_batak-house_keiffer_CC0\Batak House.max"
ARMCHAIR = r"D:\Word\arch0-spike\samples\fbx\polyhaven_ArmChair_01_2k\polyhaven_ArmChair_01_2k.fbx"
BRICKS = r"D:/Word/arch0-spike/samples/textures/Bricks105_2K-JPG_Color.jpg"
WOOD = r"D:/Word/arch0-spike/samples/textures/Wood096_2K-JPG_Color.jpg"

BOX_IDX = [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3]

def box(name, x0, y0, z0, x1, y1, z1, **extra):
    p = [[x0,y0,z0],[x1,y0,z0],[x1,y1,z0],[x0,y1,z0],[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1]]
    a = {"name": name, "positions": p, "indices": BOX_IDX}
    a.update(extra)
    return {"op": "mesh.create", "args": a}

def quad(name, pts, uvs, **extra):
    a = {"name": name, "positions": pts, "indices": [0,2,1,0,3,2], "uv": uvs}
    a.update(extra)
    return {"op": "mesh.create", "args": a}

def parse_report(out):
    lines = out.splitlines()
    for i, ln in enumerate(lines):
        if ln.strip() == "{":
            return json.loads("\n".join(lines[i:]))
    return {"ok": False, "parse_error": out[:300]}

def run(ops, tag):
    path = os.path.join(OUT, f"task_{tag}.json")
    json.dump({"ops": ops}, open(path, "w", encoding="utf-8"), ensure_ascii=False)
    r = subprocess.run([EXE, path], capture_output=True, text=True, encoding="utf-8",
                       errors="replace", cwd=CWD)
    return parse_report(r.stdout)

def load_scene_json(path):
    return json.load(open(path, encoding="utf-8"))

def facts(rep):
    reps = rep.get("reports") or []
    return reps[-1]["facts_after"] if reps else {}

results = []

def judge(tid, name, checks, evidence):
    verdict = "PASS" if all(c[1] for c in checks) else "FAIL"
    results.append({"id": tid, "name": name, "verdict": verdict,
                    "checks": checks, "evidence": evidence})

# ---------- T1 墙开门洞 ----------
r = run([
    box("Wall", 0,0,0, 3,0.2,2.8),
    box("DoorCutter", 1.05,-0.1,0, 1.95,0.3,2.1),
    {"op":"mesh.boolean","args":{"id_a":"obj-1","id_b":"obj-2","op":"difference","name":"Wall_门洞"}},
    {"op":"render.preview","args":{"path": OUT+"/t1.png","width":480,"height":360}},
    {"op":"io.save_scene","args":{"path": OUT+"/t1.json"}},
], "t1")
sc = load_scene_json(OUT + "/t1.json")
names = [o["name"] for o in sc["objects"]]
tri = facts(r).get("triangle_count", 0)
judge("T1", "墙 3m×2.8m 开 0.9×2.1m 门洞", [
    ("布尔差集成功", r.get("ok") is True),
    ("门洞对象存在", "Wall_门洞" in names),
    ("几何复杂度上升(>12 三角)", tri > 12),
], f"triangle_count={tri}")

# ---------- T2 房间体块 ----------
ops = [{"op":"object.create_group","args":{"name":"Room"}}]
walls = [("Floor",0,0,0,4,4,0.1),("Ceiling",0,0,2.7,4,4,2.8),
         ("Wall_N",0,3.9,0.1,4,4,2.7),("Wall_S",0,0,0.1,4,0.1,2.7),
         ("Wall_E",3.9,0,0.1,4,3.9,2.7),("Wall_W",0,0,0.1,0.1,3.9,2.7)]
for i,(n,a,b,c,d,e,f) in enumerate(walls):
    ops.append(box(n,a,b,c,d,e,f))
for i in range(2, 8):
    ops.append({"op":"object.set_parent","args":{"id":f"obj-{i}","parent":"obj-1"}})
ops.append({"op":"io.save_scene","args":{"path": OUT+"/t2.json"}})
r = run(ops, "t2")
sc = load_scene_json(OUT + "/t2.json")
parents_ok = sum(1 for o in sc["objects"] if o.get("parent") == "obj-1") == 6
judge("T2", "房间体块（四墙+楼板+天花，命名成组）", [
    ("执行成功", r.get("ok") is True),
    ("对象数 7", len(sc["objects"]) == 7),
    ("六件挂 Room 组", parents_ok),
    ("命名齐全", all(w[0] in [o["name"] for o in sc["objects"]] for w in walls)),
], f"objects={len(sc['objects'])}")

# ---------- T3 窗洞阵列 ----------
ops = [box("Wall", 0,0,0, 8,0.2,2.4)]
centers = [0.75, 2.25, 3.75, 5.25, 6.75]
for i, c in enumerate(centers):
    ops.append(box(f"Cut_{i+1}", c-0.6,-0.1,0.5, c+0.6,0.3,2.0))
prev = "obj-1"
changed_hashes = 0
# 链式差集在单脚本里 id 可预测：obj-7..obj-11
for i in range(5):
    cutter = f"obj-{i+2}"
    result = f"obj-{i+7}"
    ops.append({"op":"mesh.boolean","args":{"id_a":prev,"id_b":cutter,"op":"difference","name":f"Wall_窗{i+1}" if i==4 else f"Wall_step{i+1}"}})
    prev = result
ops.append({"op":"render.preview","args":{"path": OUT+"/t3.png","width":480,"height":360}})
ops.append({"op":"io.save_scene","args":{"path": OUT+"/t3.json"}})
r = run(ops, "t3")
sc = load_scene_json(OUT + "/t3.json")
bool_reps = [x for x in (r.get("reports") or []) if x.get("op_id") == "mesh.boolean"]
hash_changed = sum(1 for x in bool_reps if x["facts_before"]["geometry_hash"] != x["facts_after"]["geometry_hash"])
final_tri = facts(r).get("triangle_count", 0)
judge("T3", "窗洞阵列（5×1.2×1.5m，间距 1.5m）", [
    ("五次差集全成功", r.get("ok") is True and len(bool_reps) == 5),
    ("每次切割几何都变化", hash_changed == 5),
    ("最终几何复杂(>30 三角)", final_tri > 30),
], f"bool_steps={len(bool_reps)}, hash_changed={hash_changed}, triangle_count={final_tri}")

# ---------- T4 PBR 材质 ----------
r = run([
    {"op":"material.create","args":{"name":"MAT_Bricks","base_color":[0.75,0.75,0.75,1]}},
    {"op":"material.create","args":{"name":"MAT_Wood","base_color":[0.8,0.8,0.8,1]}},
    {"op":"material.set_texture","args":{"material":"mat-0","slot":"base_color","uri":BRICKS}},
    {"op":"material.set_texture","args":{"material":"mat-1","slot":"base_color","uri":WOOD}},
    quad("Floor", [[0,0,0],[3,0,0],[3,3,0],[0,3,0]], [[0,0],[3,0],[3,3],[0,3]], material="mat-1"),
    quad("Wall", [[0,0,0],[3,0,0],[3,0,1.8],[0,0,1.8]], [[0,0],[3,0],[3,1.8],[0,1.8]], material="mat-0"),
    {"op":"render.preview","args":{"path": OUT+"/t4.png","width":480,"height":360}},
    {"op":"render.high_quality","args":{"path": OUT+"/t4_hq.png","width":640,"height":480,"spp":32}},
    {"op":"io.save_scene","args":{"path": OUT+"/t4.json"}},
], "t4")
f4 = facts(r)
judge("T4", "PBR 材质（Bricks/Wood 贴图）", [
    ("执行成功", r.get("ok") is True),
    ("材质 2 件", f4.get("material_count") == 2),
    ("贴图资产 2 件", f4.get("asset_count") == 2),
    ("评审图+HQ 图产出", os.path.exists(OUT+"/t4.png") and os.path.exists(OUT+"/t4_hq.png")),
], f"materials={f4.get('material_count')}, assets={f4.get('asset_count')}")

# ---------- T5 资产导入编组（两阶段） ----------
r1 = run([
    {"op":"object.create_group","args":{"name":"Props"}},
    {"op":"io.import_fbx","args":{"path": ARMCHAIR}},
    {"op":"io.save_scene","args":{"path": OUT+"/t5a.json"}},
], "t5a")
imp_rep = next((x for x in (r1.get("reports") or []) if x.get("op_id") == "io.import_fbx"), None)
imported = imp_rep["changed"] if imp_rep else []
ops2 = [{"op":"io.load_scene","args":{"path": OUT+"/t5a.json"}}]
for iid in imported:
    ops2.append({"op":"object.set_parent","args":{"id":iid,"parent":"obj-1"}})
if imported:
    ops2.append({"op":"object.rename","args":{"id":imported[0],"name":"扶手椅"}})
ops2.append({"op":"io.save_scene","args":{"path": OUT+"/t5.json"}})
r2 = run(ops2, "t5b")
sc = load_scene_json(OUT + "/t5.json")
by_id = {o["id"]: o for o in sc["objects"]}
grouped = all(by_id[i].get("parent") == "obj-1" for i in imported if i in by_id)
renamed = any(o["name"] == "扶手椅" for o in sc["objects"])
judge("T5", "导入扶手椅 FBX，摆位/命名/编组", [
    ("FBX 导入成功", r1.get("ok") is True and len(imported) > 0),
    ("全部编入 Props 组", grouped and len(imported) > 0),
    ("重命名生效", renamed),
], f"imported={len(imported)} objects")

# ---------- T6 灯光相机出图 ----------
r = run([
    box("Floor", 0,0,0, 4,4,0.1),
    box("Wall", 0,3.9,0.1, 4,4,2.6),
    {"op":"light.create","args":{"name":"KeyLight","light_kind":"area","intensity":200,"size":2,"translation":[2,1,3.5]}},
    {"op":"camera.create","args":{"name":"Shot","lens_mm":35,"translation":[5,-4,3]}},
    {"op":"render.high_quality","args":{"path": OUT+"/t6.png","width":640,"height":480,"spp":32}},
    {"op":"io.save_scene","args":{"path": OUT+"/t6.json"}},
], "t6")
sc = load_scene_json(OUT + "/t6.json")
has_lc = any(o["kind"] == "light" for o in sc["objects"]) and any(o["kind"] == "camera" for o in sc["objects"])
judge("T6", "面光 + 相机 + 出图", [
    ("执行成功", r.get("ok") is True),
    ("灯光/相机对象在场", has_lc),
    ("出图产出", os.path.exists(OUT+"/t6.png")),
], "构图待人审（IR 相机朝向为固定朝向，非取景对准）")

# ---------- T7 整层复制 ----------
r1 = run([
    {"op":"object.create_group","args":{"name":"Floor_A"}},
    box("Slab", 0,0,0, 4,4,0.15),
    box("Wall", 0,3.85,0.15, 4,4,2.8),
    {"op":"object.set_parent","args":{"id":"obj-2","parent":"obj-1"}},
    {"op":"object.set_parent","args":{"id":"obj-3","parent":"obj-1"}},
    {"op":"io.save_scene","args":{"path": OUT+"/t7a.json"}},
], "t7a")
r2 = run([
    {"op":"io.load_scene","args":{"path": OUT+"/t7a.json"}},
    {"op":"object.duplicate","args":{"id":"obj-1","name":"Floor_B","translate":[0,0,3]}},
    {"op":"object.duplicate","args":{"id":"obj-1","name":"Floor_C","translate":[0,0,6]}},
    {"op":"io.save_scene","args":{"path": OUT+"/t7.json"}},
], "t7b")
sc = load_scene_json(OUT + "/t7.json")
groups = [o for o in sc["objects"] if o["name"] in ("Floor_A","Floor_B","Floor_C")]
z_ok = sorted(round(g["transform"]["translation"][2],3) for g in groups) == [0.0,3.0,6.0]
kids_ok = all(sum(1 for o in sc["objects"] if o.get("parent") == g["id"]) == 2 for g in groups)
judge("T7", "整层复制三层（层级与命名规律）", [
    ("两阶段执行成功", r1.get("ok") is True and r2.get("ok") is True),
    ("对象数 9", len(sc["objects"]) == 9),
    ("每层 2 个子件", kids_ok),
    ("偏移 0/3/6", z_ok),
], f"objects={len(sc['objects'])}")

# ---------- T8 .max 真件转换 ----------
r = run([
    {"op":"io.import_max","args":{"path": BATAK}},
    {"op":"io.save_scene","args":{"path": OUT+"/t8.json"}},
], "t8")
f8 = facts(r)
judge("T8", "Batak House.max → IR 真件转换（3dsmaxbatch 全链）", [
    ("转换+导入成功", r.get("ok") is True),
    ("对象数 >0", f8.get("object_count", 0) > 0),
    ("三角形 >0", f8.get("triangle_count", 0) > 0),
], f"objects={f8.get('object_count')}, triangles={f8.get('triangle_count')}, materials={f8.get('material_count')}")

# ---------- T9 FBX 往返保真 ----------
r1 = run([
    box("BoxA", 0,0,0, 1,1,1, color=[0.85,0.35,0.3]),
    box("BoxB", 1.4,0,0, 2.4,1,0.6, color=[0.3,0.5,0.85]),
    {"op":"io.export_fbx","args":{"path": OUT+"/t9.fbx"}},
    {"op":"io.save_scene","args":{"path": OUT+"/t9a.json"}},
], "t9a")
r2 = run([
    {"op":"io.import_fbx","args":{"path": OUT+"/t9.fbx"}},
    {"op":"io.save_scene","args":{"path": OUT+"/t9b.json"}},
], "t9b")
fa, fb = facts(r1), facts(r2)
judge("T9", "FBX 往返保真（导出→回读逐通道）", [
    ("双向执行成功", r1.get("ok") is True and r2.get("ok") is True),
    ("对象数一致", fa.get("object_count") == fb.get("object_count") and fa.get("object_count") == 2),
    ("三角形数一致", fa.get("triangle_count") == fb.get("triangle_count")),
    ("层级哈希一致(命名/父子)", fa.get("hierarchy_hash") == fb.get("hierarchy_hash")),
], f"objects {fa.get('object_count')}→{fb.get('object_count')}, tris {fa.get('triangle_count')}→{fb.get('triangle_count')}, hierarchy {'一致' if fa.get('hierarchy_hash')==fb.get('hierarchy_hash') else '不一致'}")

# ---------- T10 修错任务（一票否决项） ----------
r = run([
    box("Wall", 0,0,0, 3,0.2,2.4),
    box("Cutter", 9,9,9, 10,10,10),
    {"op":"mesh.boolean","args":{"id_a":"obj-1","id_b":"obj-2","op":"difference"}},
], "t10")
err = r.get("error", {})
judge("T10", "修错任务：切除打空必须显式报错（一票否决）", [
    ("整体拒绝执行", r.get("ok") is False),
    ("失败步指名", r.get("failed_step") == 2),
    ("结构化错误码", err.get("code") == "precondition"),
], f"error={err}")

# ---------- 汇总 ----------
lines = ["# G2 评测结果 —— 评测基准 v0 全量", "",
         "> 机器裁决（几何事实/产出文件/往返对比自动判定）；标注「人审待确认」的为视觉件。",
         "", "| # | 任务 | 判定 | 判据明细 | 证据 |", "|---|---|---|---|---|"]
for t in results:
    detail = "<br>".join(f"{c[0]}: {'✅' if c[1] else '❌'}" for c in t["checks"])
    lines.append(f"| {t['id']} | {t['name']} | **{t['verdict']}** | {detail} | {t['evidence']} |")
passed = sum(1 for t in results if t["verdict"] == "PASS")
lines += ["", f"## 总分：{passed}/{len(results)} 通过",
          "", "### 已知债务", "- `next_id` 在删除后可能复用 id（max+1 策略）——与稳定 ID 不复用的宪法冲突，需改为单调计数器（已记账）。",
          "- T6 的 IR 相机朝向为对象变换驱动，未做取景对准（视口/取景器是后续项）。",
          "- 评测基准任务判据 v0 口径，v1 起纳入回归集。"]
open(os.path.join(OUT, "G2评测结果.md"), "w", encoding="utf-8").write("\n".join(lines))
print(f"G2 评测完成：{passed}/{len(results)} 通过")
for t in results:
    print(f"  {t['id']} {t['verdict']}  {t['name']}")
