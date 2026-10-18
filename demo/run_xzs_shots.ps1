# 新中式别墅渲染运行器：每张图独立进程（规避 wgpu TDR 累积），失败自动重试一次
$ErrorActionPreference = "Continue"
Remove-Item env:HTTP_PROXY,env:HTTPS_PROXY,env:WS_PROXY,env:WSS_PROXY -EA SilentlyContinue
$exe = "D:\word\archcore\target\debug\archbloom-run.exe"
$shots = Get-Content "D:\Word\archcore\demo\xzs_shots\_manifest.txt" | Where-Object { $_ -ne "" }
$log = @()
foreach ($s in $shots) {
    $name = [IO.Path]::GetFileNameWithoutExtension($s)
    $ok = $false
    foreach ($try in 1..2) {
        $out = & $exe $s 2>&1 | Out-String
        if ($LASTEXITCODE -eq 0) { $ok = $true; break }
        Write-Host "[$name] try $try failed: $($out.Substring(0, [Math]::Min(160, $out.Length)))"
        Start-Sleep -Seconds 3
    }
    $log += "{0}: {1}" -f $name, $(if ($ok) { "OK" } else { "FAIL" })
    Write-Host $log[-1]
}
$log | Set-Content "D:\Word\deliverables_xzs\_render_log.txt"
"DONE"
