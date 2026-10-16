# Archbloom 外部依赖检测（Windows）
# 运行：powershell -File check_deps.ps1
$ErrorActionPreference = "Continue"
Write-Host "=== Archbloom 依赖检测 ===" -ForegroundColor Cyan

function Test-Tool($name, $cmd, $hint) {
    $found = Get-Command $cmd -ErrorAction SilentlyContinue
    if ($found) {
        Write-Host "[OK]   $name : $($found.Source)" -ForegroundColor Green
    } else {
        Write-Host "[缺失] $name —— $hint" -ForegroundColor Yellow
    }
}

Test-Tool "Blender（FBX 桥）" "blender" "安装 Blender 4.x 并加入 PATH；或在调用时传绝对路径"
Test-Tool "ffmpeg（视频装配）" "ffmpeg" "winget install Gyan.FFmpeg；缺失时帧序列仍产出，可手动装配"
Test-Tool "3dsmaxbatch（.max 导入）" "3dsmaxbatch" "安装 3ds Max 并将 3dsmaxbatch.exe 加入 PATH"

# 常见安装路径兜底探测
$candidates = @(
    "D:\New Folder\blender.exe",
    "C:\Program Files\Blender Foundation\Blender*\blender.exe",
    "D:\3D MAX\3ds Max 2024\3dsmaxbatch.exe",
    "C:\Program Files\Autodesk\3ds Max*\3dsmaxbatch.exe"
)
foreach ($p in $candidates) {
    $hit = Get-Item $p -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($hit) { Write-Host "[发现] 常见路径：$($hit.FullName)" -ForegroundColor DarkGreen }
}

# GPU 探测
$gpu = Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name
Write-Host "=== 显卡 ===" -ForegroundColor Cyan
$gpu | ForEach-Object { Write-Host "  $_" }
Write-Host "（渲染加速需 Vulkan/DX12 兼容显卡；无 GPU 时 CPU 渲染可用）" -ForegroundColor DarkGray
