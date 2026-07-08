$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$repoRoot = Resolve-Path (Join-Path $root "..")
$nodeDir = Get-ChildItem -LiteralPath (Join-Path $repoRoot ".tools") -Directory -Filter "node-v22*" |
    Sort-Object Name -Descending |
    Select-Object -First 1

if ($null -eq $nodeDir) {
    throw "Node 22 portable runtime was not found under .tools. Run ..\scripts\install-node-portable.ps1 first."
}

$env:PATH = "$($nodeDir.FullName);$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:CARGO_BUILD_JOBS = "1"

$debugExe = Join-Path $root "src-tauri\target\debug\dustdesk-tauri.exe"
$runningDebugProcesses = Get-CimInstance Win32_Process -Filter "name = 'dustdesk-tauri.exe'" |
    Where-Object { $_.ExecutablePath -eq $debugExe }

foreach ($process in $runningDebugProcesses) {
    Stop-Process -Id $process.ProcessId -Force
}

if ($runningDebugProcesses) {
    Start-Sleep -Milliseconds 500
}

Push-Location $root
try {
    & (Join-Path $nodeDir.FullName "npm.cmd") run tauri build -- --debug
}
finally {
    Pop-Location
}
