import json

d = json.load(open(r"D:\Word\archcore\demo\massing.json", encoding="utf-8"))
for op in d["ops"]:
    if op["op"] == "render.preview":
        op["op"] = "render.high_quality"
        op["args"] = {
            "path": "D:/Word/arch0-spike/hq_massing.png",
            "width": 800,
            "height": 600,
            "spp": 64,
        }
json.dump(
    d,
    open(r"D:\Word\archcore\demo\massing_hq.json", "w", encoding="utf-8"),
    ensure_ascii=False,
    indent=1,
)
print("massing_hq.json ok")
