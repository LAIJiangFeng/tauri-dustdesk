$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$repoRoot = Resolve-Path (Join-Path $root "..")
$nodeDir = Get-ChildItem -LiteralPath (Join-Path $repoRoot ".tools") -Directory -Filter "node-v22*" |
    Sort-Object Name -Descending |
    Select-Object -First 1

if ($null -eq $nodeDir) {
    throw "Node 22 portable runtime was not found under .tools. Run scripts/install-node-portable.ps1 first."
}

$env:PATH = "$($nodeDir.FullName);$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:CARGO_BUILD_JOBS = "1"
Push-Location $root
try {
    & (Join-Path $nodeDir.FullName "npm.cmd") run build
    & (Join-Path $nodeDir.FullName "npm.cmd") run tauri build
}
finally {
    Pop-Location
}
