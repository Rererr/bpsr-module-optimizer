# bpsr-module-optimizer launcher.
# Non-admin (default): dump mode (pre-loads owned_modules.json).
# -Live: self-elevate (UAC) and run with WinDivert live capture, logging stderr to a file.
param([switch]$Live)

$exe = "C:\Users\PCuser\develop\bpsr-module-optimizer\src-tauri\target\release\bpsr-module-optimizer.exe"
$log = "$env:TEMP\bpsr-mod-opt.log"
$out = "$env:TEMP\bpsr-mod-opt.out"

if (-not (Test-Path $exe)) {
    Write-Host "exe not found. Build first: npx tauri build --no-bundle"
    return
}

if ($Live) {
    $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)
    if (-not $isAdmin) {
        Write-Host "Elevating for live capture (approve UAC)..."
        Start-Process -FilePath "powershell.exe" -Verb RunAs -ArgumentList @(
            "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $PSCommandPath, "-Live"
        )
        return
    }
    # Elevated: launch the app capturing stderr (env_logger) to a log file.
    if (Test-Path $log) { Remove-Item $log -Force }
    Write-Host "Live capture. Log: $log"
    Start-Process -FilePath $exe -RedirectStandardError $log -RedirectStandardOutput $out
    return
}

Start-Process -FilePath $exe
Write-Host "Launched (dump mode). Use -Live for admin live capture."
