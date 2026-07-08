$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$exe = Join-Path $root "src-tauri\target\debug\dustdesk-tauri.exe"

if (-not (Test-Path -LiteralPath $exe)) {
    throw "Desktop exe was not found. Run scripts\build-desktop-debug.ps1 first."
}

Get-Process -Name "dustdesk-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe)
