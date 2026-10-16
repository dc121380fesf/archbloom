import json
import subprocess

EXE = r"D:\Word\archcore\target\debug\archcore-run.exe"
CWD = r"D:\Word\archcore"

def parse_report(out):
    lines = out.splitlines()
    for i, ln in enumerate(lines):
        if ln.strip() == "{":
            return json.loads("\n".join(lines[i:]))
    raise RuntimeError("未找到 JSON 报告; 输出开头: " + out[:200])

def run(script):
    r = subprocess.run(
        [EXE, script],
        capture_output=True, text=True, encoding="utf-8", errors="replace", cwd=CWD,
    )
    report = parse_report(r.stdout)
    return report, r.returncode, r.stderr[-300:]

a, ra, ea = run(r"demo\rt_export.json")
b, rb, eb = run(r"demo\rt_import.json")
print("导出 run exit:", ra, "| 导入 run exit:", rb)
fa = a["reports"][-1]["facts_after"]
fb = b["reports"][0]["facts_after"]
print("导出侧:", fa)
print("导入侧:", fb)
print("对象数一致:", fa["object_count"] == fb["object_count"])
print("三角形数一致:", fa["triangle_count"] == fb["triangle_count"])
print("几何哈希一致:", fa["geometry_hash"] == fb["geometry_hash"], "(f32 往返舍入可致不同)")
print("导入对象 ids:", b["reports"][0]["changed"])
if ra or rb:
    print("stderr 尾巴:", ea, eb)
