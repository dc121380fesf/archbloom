//! Web 查看器导出：IR 场景 → 自包含 HTML（内嵌几何 + 手写 WebGL2 查看器，零外部请求）。
//! 审阅面：轨道相机 / 物体树（可见性）/ 线框 / 事实面。几何与 review_triangles 同源（世界坐标烘焙）。
//! v1 口径：几何内嵌 JSON（大场景 HTML 体积见说明书）；离线打开即用。

use archcore_ir::{ObjectKind, Scene};

/// 坐标紧凑化（4 位小数，米制精度足够审阅）
fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

/// 场景 → 自包含审阅 HTML
pub fn export_web(scene: &Scene, title: &str) -> Result<String, String> {
    let (tris, _tex) = crate::review_triangles(scene);
    // 颜色：材质 base_color 缺省中灰
    let mut objs: Vec<serde_json::Value> = Vec::new();
    let mut mesh_meta: Vec<(String, usize, usize)> = Vec::new(); // (id, tri_start, tri_count)
    for o in &scene.objects {
        let color: [f64; 3] = match &o.kind {
            ObjectKind::Mesh { material_slots, .. } => material_slots
                .first()
                .and_then(|m| scene.materials.iter().find(|x| x.id == *m))
                .map(|m| [m.base_color[0], m.base_color[1], m.base_color[2]])
                .unwrap_or([0.72, 0.72, 0.72]),
            _ => [0.72, 0.72, 0.72],
        };
        objs.push(serde_json::json!({
            "id": o.id,
            "name": o.name,
            "kind": match &o.kind {
                ObjectKind::Mesh { .. } => "mesh",
                ObjectKind::Group => "group",
                ObjectKind::Light { .. } => "light",
                ObjectKind::Camera { .. } => "camera",
                ObjectKind::Instance { .. } => "instance",
            },
            "parent": o.parent,
            "color": [r4(color[0]), r4(color[1]), r4(color[2])],
        }));
    }
    // 三角缓冲（按对象分段，供可见性开关按段绘制）
    let mut positions: Vec<f64> = Vec::with_capacity(tris.len() * 9);
    let mut normals: Vec<f64> = Vec::with_capacity(tris.len() * 9);
    let mut colors: Vec<f64> = Vec::with_capacity(tris.len() * 9);
    for t in &tris {
        let e1 = [
            t.p[1][0] - t.p[0][0],
            t.p[1][1] - t.p[0][1],
            t.p[1][2] - t.p[0][2],
        ];
        let e2 = [
            t.p[2][0] - t.p[0][0],
            t.p[2][1] - t.p[0][1],
            t.p[2][2] - t.p[0][2],
        ];
        let mut n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        n = [n[0] / l, n[1] / l, n[2] / l];
        for p in &t.p {
            positions.extend([r4(p[0]), r4(p[1]), r4(p[2])]);
            normals.extend([r4(n[0]), r4(n[1]), r4(n[2])]);
            colors.extend([r4(t.color[0]), r4(t.color[1]), r4(t.color[2])]);
        }
    }
    // 网格对象的三角分段（collect_tris 顺序=对象顺序内的面序，按对象几何面数切段）
    {
        let mut cursor = 0usize;
        for o in &scene.objects {
            if let ObjectKind::Mesh { geometry, .. } = &o.kind {
                let n_tris = geometry.indices.len() / 3;
                mesh_meta.push((o.id.clone(), cursor, n_tris));
                cursor += n_tris;
            }
        }
    }
    let facts = archcore_ops::geo_facts(scene);
    let data = serde_json::json!({
        "name": scene.name,
        "title": title,
        "units": format!("{:?}", scene.units),
        "up_axis": format!("{:?}", scene.up_axis),
        "facts": {
            "objects": facts.object_count,
            "meshes": facts.mesh_count,
            "triangles": facts.triangle_count,
            "materials": facts.material_count,
            "hierarchy_hash": facts.hierarchy_hash,
            "geometry_hash": facts.geometry_hash,
        },
        "objects": objs,
        "segments": mesh_meta,
        "positions": positions,
        "normals": normals,
        "colors": colors,
    });
    let html = TEMPLATE
        .replace("__TITLE__", &html_escape(title))
        .replace("__SCENE_JSON__", &data.to_string());
    Ok(html)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

const TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>__TITLE__ — Archbloom 查看器</title>
<style>
html,body{margin:0;height:100%;background:#14161a;color:#d7dade;font:13px/1.5 system-ui,"Microsoft YaHei",sans-serif}
header{position:fixed;top:0;left:0;right:0;height:44px;display:flex;align-items:center;gap:14px;padding:0 16px;background:#1b1e24;border-bottom:1px solid #2a2e36;z-index:2}
header b{font-size:14px}
header .sp{flex:1}
button{background:#262b33;color:#d7dade;border:1px solid #343a44;border-radius:6px;padding:5px 12px;cursor:pointer;font:inherit}
button:hover{background:#2f3540}
button.on{background:#2d4a6b;border-color:#3c6898}
aside{position:fixed;top:44px;left:0;bottom:0;width:230px;overflow:auto;background:#181b20;border-right:1px solid #2a2e36;padding:10px}
.tree{list-style:none;margin:0;padding:0}
.tree li{padding:2px 0 2px 10px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.tree .kind{color:#7b8494;font-size:11px;margin-left:6px}
main{position:fixed;top:44px;left:230px;right:0;bottom:0}
canvas{width:100%;height:100%;display:block;cursor:grab}
canvas:active{cursor:grabbing}
#hud{position:absolute;right:12px;bottom:12px;background:rgba(20,22,26,.82);border:1px solid #2a2e36;border-radius:8px;padding:8px 12px;font-size:12px;line-height:1.7}
#hud b{color:#8fc1ff}
</style>
</head>
<body>
<header>
  <b>__TITLE__</b>
  <span id="meta"></span>
  <span class="sp"></span>
  <button id="btn-wire">线框</button>
  <button id="btn-fit">适配视图</button>
</header>
<aside><div style="color:#7b8494;margin-bottom:6px">物体树（点击切换可见性）</div><ul class="tree" id="tree"></ul></aside>
<main>
  <canvas id="view"></canvas>
  <div id="hud"></div>
</main>
<script id="scene-data" type="application/json">__SCENE_JSON__</script>
<script>
"use strict";
const S = JSON.parse(document.getElementById("scene-data").textContent);
// ---------- 矩阵 ----------
function perspective(fovy, aspect, near, far){
  const f = 1/Math.tan(fovy/2), nf = 1/(near-far);
  return new Float32Array([f/aspect,0,0,0, 0,f,0,0, 0,0,(far+near)*nf,-1, 0,0,2*far*near*nf,0]);
}
function lookAt(e,c,u){
  let z=[e[0]-c[0],e[1]-c[1],e[2]-c[2]];let zl=Math.hypot(...z);z=z.map(v=>v/zl);
  let x=[u[1]*z[2]-u[2]*z[1],u[2]*z[0]-u[0]*z[2],u[0]*z[1]-u[1]*z[0]];let xl=Math.hypot(...x)||1;x=x.map(v=>v/xl);
  const y=[z[1]*x[2]-z[2]*x[1],z[2]*x[0]-z[0]*x[2],z[0]*x[1]-z[1]*x[0]];
  return new Float32Array([x[0],y[0],z[0],0, x[1],y[1],z[1],0, x[2],y[2],z[2],0,
    -(x[0]*e[0]+x[1]*e[1]+x[2]*e[2]), -(y[0]*e[0]+y[1]*e[1]+y[2]*e[2]), -(z[0]*e[0]+z[1]*e[1]+z[2]*e[2]),1]);
}
function mul(a,b){const o=new Float32Array(16);for(let c=0;c<4;c++)for(let r=0;r<4;r++){let s=0;for(let k=0;k<4;k++)s+=a[k*4+r]*b[c*4+k];o[c*4+r]=s;}return o;}
// ---------- WebGL ----------
const cv = document.getElementById("view");
const gl = cv.getContext("webgl2", {antialias:true});
if(!gl){document.body.innerHTML="<p style='padding:20px'>浏览器不支持 WebGL2。</p>";}
const VS=`#version 300 es
in vec3 aPos;in vec3 aNor;in vec3 aCol;uniform mat4 uMVP;out vec3 vNor;out vec3 vCol;
void main(){gl_Position=uMVP*vec4(aPos,1.0);vNor=aNor;vCol=aCol;}`;
const FS=`#version 300 es
precision highp float;in vec3 vNor;in vec3 vCol;out vec4 o;
void main(){float l=0.32+0.68*abs(dot(normalize(vNor),normalize(vec3(0.4,0.35,1.0))));o=vec4(vCol*l,1.0);}`;
function sh(t,s){const h=gl.createShader(t);gl.shaderSource(h,s);gl.compileShader(h);return h;}
const prog=gl.createProgram();gl.attachShader(prog,sh(gl.VERTEX_SHADER,VS));gl.attachShader(prog,sh(gl.FRAGMENT_SHADER,FS));gl.linkProgram(prog);
const vao=gl.createVertexArray();gl.bindVertexArray(vao);
function buf(data,attr,size){const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array(data),gl.STATIC_DRAW);const loc=gl.getAttribLocation(prog,attr);gl.enableVertexAttribArray(loc);gl.vertexAttribPointer(loc,size,gl.FLOAT,false,0,0);}
buf(S.positions,"aPos",3);buf(S.normals,"aNor",3);buf(S.colors,"aCol",3);
const idx=new Uint32Array(S.positions.length/3);for(let i=0;i<idx.length;i++)idx[i]=i;
const ibo=gl.createBuffer();gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,ibo);gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,idx,gl.STATIC_DRAW);
// 线框边索引
const wire=new Uint32Array(idx.length*2);for(let t=0;t<idx.length;t+=3){wire[t*2]=idx[t];wire[t*2+1]=idx[t+1];wire[t*2+2]=idx[t+1];wire[t*2+3]=idx[t+2];wire[t*2+4]=idx[t+2];wire[t*2+5]=idx[t];}
const wbo=gl.createBuffer();gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,wbo);gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,wire,gl.STATIC_DRAW);
gl.enable(gl.DEPTH_TEST);
// ---------- 相机（轨道） ----------
const P=S.positions;let ctr=[0,0,0];let rad=10;
if(P.length){let lo=[1e30,1e30,1e30],hi=[-1e30,-1e30,-1e30];for(let i=0;i<P.length;i+=3)for(let k=0;k<3;k++){lo[k]=Math.min(lo[k],P[i+k]);hi[k]=Math.max(hi[k],P[i+k]);}
 ctr=[(lo[0]+hi[0])/2,(lo[1]+hi[1])/2,(lo[2]+hi[2])/2];rad=Math.max(2,Math.hypot(hi[0]-lo[0],hi[1]-lo[1],hi[2]-lo[2])/2*1.6);}
let yaw=-2.2,pitch=0.45,dist=rad,tgt=ctr.slice();
function eye(){return [tgt[0]+dist*Math.cos(pitch)*Math.cos(yaw),tgt[1]+dist*Math.cos(pitch)*Math.sin(yaw),tgt[2]+dist*Math.sin(pitch)];}
function resize(){const r=cv.getBoundingClientRect();cv.width=r.width*devicePixelRatio;cv.height=r.height*devicePixelRatio;gl.viewport(0,0,cv.width,cv.height);}
window.addEventListener("resize",resize);
// ---------- 交互 ----------
let drag=null;
cv.addEventListener("pointerdown",e=>{drag={x:e.clientX,y:e.clientY,btn:e.button,shift:e.shiftKey};cv.setPointerCapture(e.pointerId);});
cv.addEventListener("pointermove",e=>{if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;drag.x=e.clientX;drag.y=e.clientY;
 if(drag.btn===2||drag.shift){const s=dist*0.0016;const cy=Math.cos(yaw),sy=Math.sin(yaw);tgt[0]+=(-dx*cy - dy*sy)*s;tgt[1]+=(-dx*sy + dy*cy)*s;}
 else{yaw-=dx*0.006;pitch=Math.max(-1.5,Math.min(1.5,pitch+dy*0.006));}});
cv.addEventListener("pointerup",()=>drag=null);
cv.addEventListener("contextmenu",e=>e.preventDefault());
cv.addEventListener("wheel",e=>{e.preventDefault();dist=Math.max(rad*0.02,dist*Math.exp(e.deltaY*0.001));},{passive:false});
// ---------- 物体树 ----------
const vis={};S.objects.forEach(o=>vis[o.id]=true);
const byParent={};S.objects.forEach(o=>{const k=o.parent||"";(byParent[k]=byParent[k]||[]).push(o);});
function branch(pid,ul){(byParent[pid]||[]).forEach(o=>{const li=document.createElement("li");
 const cb=document.createElement("input");cb.type="checkbox";cb.checked=true;cb.addEventListener("change",()=>{vis[o.id]=cb.checked;});
 li.appendChild(cb);const sp=document.createElement("span");sp.textContent=" "+o.name;const kd=document.createElement("span");kd.className="kind";kd.textContent=o.kind;sp.appendChild(kd);li.appendChild(sp);
 ul.appendChild(li);if(byParent[o.id]){const sub=document.createElement("ul");sub.className="tree";sub.style.paddingLeft="14px";li.appendChild(sub);branch(o.id,sub);}});}
branch("",document.getElementById("tree"));
// ---------- HUD ----------
document.getElementById("meta").textContent=S.name+" · 单位 "+S.units+" · "+S.up_axis;
document.getElementById("hud").innerHTML="物体 <b>"+S.facts.objects+"</b> · 网格 <b>"+S.facts.meshes+"</b> · 三角 <b>"+S.facts.triangles+
 "</b><br>层级哈希 <b>"+S.facts.hierarchy_hash.toString(16)+"</b> · 几何哈希 <b>"+S.facts.geometry_hash.toString(16)+"</b>";
// ---------- 绘制 ----------
let wireMode=false;
document.getElementById("btn-wire").addEventListener("click",e=>{wireMode=!wireMode;e.target.classList.toggle("on",wireMode);});
document.getElementById("btn-fit").addEventListener("click",()=>{tgt=ctr.slice();dist=rad;});
function draw(){
  resize();
  gl.clearColor(0.09,0.10,0.12,1);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);
  const proj=perspective(0.9,cv.width/cv.height,Math.max(0.01,dist*0.01),dist*20);
  const view=lookAt(eye(),tgt,[0,0,1]);
  gl.useProgram(prog);gl.uniformMatrix4fv(gl.getUniformLocation(prog,"uMVP"),false,mul(proj,view));
  gl.bindVertexArray(vao);
  // 按对象分段绘制（可见性）
  for(const seg of S.segments){
    const obj=S.objects.find(o=>o.id===seg[0]);
    if(obj&&!vis[seg[0]])continue;
    gl.drawArrays(gl.TRIANGLES,seg[1]*3,seg[2]*3);
  }
  if(wireMode){gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,wbo);gl.drawElements(gl.LINES,wire.length,gl.UNSIGNED_INT,0);gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,ibo);}
  requestAnimationFrame(draw);
}
draw();
</script>
</body>
</html>
"#;
