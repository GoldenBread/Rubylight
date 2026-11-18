# Runs on the remote machine over ssh (Windows PowerShell 5.1, session 0,
# elevated): refuses while the machine is busy, else starts run.ps1 through an
# Interactive scheduled task in the console user's session, where host tests
# that start programs work. Called by tools\remote-check.ps1; ASCII only.
# Exit codes: 0 started, 4 busy, 5 nobody (or another user) at the console.
param(
    [Parameter(Mandatory)][string]$Branch,
    [Parameter(Mandatory)][string]$Name,
    [string]$Repo = 'C:\src\Rubylight',
    [string]$Work = 'C:\src\rl-check',
    [string]$Locks = 'C:\src\butterpollo-release',
    [string]$Pool = 'C:\src\rl-target-pool',
    [string]$User = 'ramaz',
    [int]$Hours = 3
)
$ErrorActionPreference = 'Stop'

function Test-Held([string]$Path) {
    # A lane is a file some process holds open without sharing (tools/build_lock.py).
    try { [IO.File]::Open($Path, 'Open', 'ReadWrite', 'None').Dispose(); return $false } catch { return $true }
}

$busy = @()
foreach ($lock in 'BUILD-LOCK', 'LAPTOP-LOCK', 'PRIORITY') {
    $path = Join-Path $Locks $lock
    if (-not (Test-Path -LiteralPath $path)) { continue }
    $text = Get-Content -LiteralPath $path -Raw -ErrorAction SilentlyContinue
    # A release ticket counts only while its process lives.
    if ($lock -eq 'PRIORITY' -and $text -match 'pid=(\d+)' -and -not (Get-Process -Id ([int]$Matches[1]) -ErrorAction SilentlyContinue)) { continue }
    $busy += "$lock`: $text"
}
foreach ($dir in (Join-Path $Locks 'lanes'), $Pool) {
    if (Test-Path -LiteralPath $dir) {
        foreach ($file in Get-ChildItem -LiteralPath $dir -Filter *.lock) {
            if (Test-Held $file.FullName) { $busy += "lane $($file.FullName)" }
        }
    }
}
foreach ($task in Get-ScheduledTask -TaskName 'rl-gate-*', 'rl-remote-check-*' -ErrorAction SilentlyContinue) {
    if ($task.State -eq 'Running') { $busy += "task $($task.TaskName) is running" }
}
if ($busy.Count) {
    'BUSY'
    $busy
    exit 4
}

$console = (Get-CimInstance Win32_ComputerSystem).UserName
if (-not $console -or ($console -split '\\')[-1] -ne $User) {
    "NO SESSION: the console user is '$console', not $User; an Interactive task needs $User signed in"
    exit 5
}

New-Item -ItemType Directory -Force -Path $Work | Out-Null
$log = Join-Path $Work "$Name.log"
Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
$taskName = "rl-remote-check-$Name"
$run = Join-Path $PSScriptRoot 'run.ps1'
$arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$run`" -Branch `"$Branch`" -Name `"$Name`" -Repo `"$Repo`" -Work `"$Work`""
$action = New-ScheduledTaskAction -Execute 'pwsh.exe' -Argument $arguments
$principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours $Hours) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
"STARTED $taskName"
exit 0
