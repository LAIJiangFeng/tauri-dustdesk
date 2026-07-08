param(
    [int] $WaitSeconds = 90,
    [switch] $NoWait
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$rootPath = $root.Path
$targetRoot = Join-Path $rootPath "src-tauri\target"
$logDir = Join-Path $rootPath "logs"
$runStamp = Get-Date -Format "yyyyMMdd-HHmmss"
$stdoutLog = Join-Path $logDir "tauri-dev.$runStamp.stdout.log"
$stderrLog = Join-Path $logDir "tauri-dev.$runStamp.stderr.log"

function Write-Step {
    param([string] $Message)
    Write-Host "==> $Message"
}

function Escape-SingleQuoted {
    param([string] $Value)
    return $Value.Replace("'", "''")
}

function Get-NpmCommand {
    $repoRoot = Split-Path $rootPath -Parent
    $portableTools = Join-Path $repoRoot ".tools"
    $nodeDir = Get-ChildItem -LiteralPath $portableTools -Directory -Filter "node-v22*" -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Select-Object -First 1

    if ($null -ne $nodeDir) {
        $env:PATH = "$($nodeDir.FullName);$env:USERPROFILE\.cargo\bin;$env:PATH"
        return Join-Path $nodeDir.FullName "npm.cmd"
    }

    $npm = Get-Command npm.cmd -ErrorAction SilentlyContinue
    if ($null -eq $npm) {
        $npm = Get-Command npm -ErrorAction SilentlyContinue
    }

    if ($null -eq $npm) {
        throw "npm was not found. Install Node.js or place a node-v22 portable runtime under ..\.tools."
    }

    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    return $npm.Source
}

function Test-PathStartsWith {
    param(
        [string] $Path,
        [string] $Prefix
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $false
    }

    return $Path.StartsWith($Prefix, [System.StringComparison]::OrdinalIgnoreCase)
}

function Stop-OldDesktopDev {
    Write-Step "Stopping old DeskNest dev processes"

    $ids = New-Object "System.Collections.Generic.HashSet[int]"
    $processes = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)

    foreach ($process in $processes) {
        if ($process.ProcessId -eq $PID) {
            continue
        }

        $name = [string] $process.Name
        $commandLine = [string] $process.CommandLine
        $executablePath = [string] $process.ExecutablePath

        if ($name -eq "dustdesk-tauri.exe" -and (Test-PathStartsWith $executablePath $targetRoot)) {
            [void] $ids.Add([int] $process.ProcessId)
            continue
        }

        if ($name -in @("node.exe", "cargo.exe", "rustc.exe") -and $commandLine.IndexOf($rootPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            [void] $ids.Add([int] $process.ProcessId)
            continue
        }

        if ($name -in @("powershell.exe", "pwsh.exe") -and $commandLine.IndexOf($rootPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and $commandLine -match "tauri\s+dev") {
            [void] $ids.Add([int] $process.ProcessId)
        }
    }

    do {
        $addedChild = $false
        foreach ($process in $processes) {
            if ($process.ProcessId -eq $PID) {
                continue
            }

            if ($ids.Contains([int] $process.ParentProcessId) -and -not $ids.Contains([int] $process.ProcessId)) {
                [void] $ids.Add([int] $process.ProcessId)
                $addedChild = $true
            }
        }
    } while ($addedChild)

    foreach ($id in ($ids | Sort-Object -Descending)) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }

    if ($ids.Count -gt 0) {
        Start-Sleep -Milliseconds 800
    }

    Write-Step "Stopped $($ids.Count) old process(es)"
}

function Show-DesktopWindow {
    $process = Get-Process dustdesk-tauri -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowHandle -ne 0 } |
        Sort-Object StartTime -Descending |
        Select-Object -First 1

    if ($null -eq $process) {
        return $false
    }

    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class WindowTools {
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
'@ -ErrorAction SilentlyContinue

    [WindowTools]::ShowWindow($process.MainWindowHandle, 9) | Out-Null
    [WindowTools]::SetForegroundWindow($process.MainWindowHandle) | Out-Null
    Write-Step "Desktop window is running, PID $($process.Id)"
    return $true
}

New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$npm = Get-NpmCommand
$env:CARGO_BUILD_JOBS = "1"

Stop-OldDesktopDev

$escapedRoot = Escape-SingleQuoted $rootPath
$escapedNpm = Escape-SingleQuoted $npm
$devCommand = "Set-Location -LiteralPath '$escapedRoot'; & '$escapedNpm' run tauri dev"

Write-Step "Starting Tauri dev shell"
$runner = Start-Process -FilePath powershell.exe `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", $devCommand) `
    -RedirectStandardOutput $stdoutLog `
    -RedirectStandardError $stderrLog `
    -WindowStyle Hidden `
    -PassThru

Write-Step "Dev runner PID $($runner.Id)"
Write-Step "Logs: $stdoutLog"
Write-Step "Logs: $stderrLog"

if ($NoWait) {
    exit 0
}

$deadline = (Get-Date).AddSeconds($WaitSeconds)
while ((Get-Date) -lt $deadline) {
    if (Show-DesktopWindow) {
        exit 0
    }

    if ($runner.HasExited) {
        Write-Host "Dev runner exited early. Last stderr lines:"
        if (Test-Path -LiteralPath $stderrLog) {
            Get-Content -LiteralPath $stderrLog -Tail 80
        }
        exit 1
    }

    Start-Sleep -Seconds 1
}

Write-Host "Timed out waiting for desktop window. Dev server may still be compiling."
Write-Host "Check $stderrLog"
exit 1
