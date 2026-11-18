# Runs tools\dev.ps1 check for a pushed branch on another Windows machine (the
# laptop lane) and copies its summary back. Pushes nothing.
#   tools\remote-check.ps1 -Host laptop -Branch wip/my-topic [-Wait]
# Over ssh the remote shell runs in session 0, elevated, where host tests that
# start programs fail; so tools\remote-check\start.ps1 starts the check through
# an Interactive scheduled task in the console user's session
# (tools\remote-check\run.ps1: a detached worktree of origin/<branch> under
# C:\src\rl-check, then dev.ps1 check -KeepGoing). The scripts are copied with
# scp, not passed inline: the remote side is Windows PowerShell 5.1.
# It refuses (exit 4) while the remote machine's BUILD-LOCK, LAPTOP-LOCK, a live
# release PRIORITY, a stage lane or another remote check is busy; -Wait polls
# until it is free instead. Exit 5: nobody (or another user) at the console.
# Otherwise the exit code is the remote check's. The run folder (summary.json and
# one log per check) lands in -Out, by default
# C:\src\butterpollo-release\unified\dev-check\<host>-<branch>\.
param(
    [Alias('Host')][string]$Machine = 'laptop',
    [Parameter(Mandatory)][string]$Branch,
    [string]$Ssh,
    [string]$RemoteRepo = 'C:\src\Rubylight',
    [string]$RemoteWork = 'C:\src\rl-check',
    [string]$Out,
    [int]$TimeoutMinutes = 180,
    [switch]$Wait
)
$ErrorActionPreference = 'Stop'
$machines = @{ laptop = 'ramaz@192.168.4.12' }
if (-not $Ssh) {
    if (-not $machines.ContainsKey($Machine)) { throw "unknown host $Machine; pass -Ssh user@address" }
    $Ssh = $machines[$Machine]
}
$Branch = $Branch -replace '^(refs/heads/|origin/)', ''
$name = $Branch -replace '[^A-Za-z0-9-]', '-'
if (-not $Out) { $Out = "C:\src\butterpollo-release\unified\dev-check\$Machine-$name" }
$sshOptions = @('-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15')
# Windows' OpenSSH, which has the keys; from Git Bash a bare ssh would be Git's.
$openSsh = Join-Path $env:SystemRoot 'System32\OpenSSH'
$sshExe = if (Test-Path -LiteralPath "$openSsh\ssh.exe") { "$openSsh\ssh.exe" } else { 'ssh' }
$scpExe = if (Test-Path -LiteralPath "$openSsh\scp.exe") { "$openSsh\scp.exe" } else { 'scp' }
function Invoke-Remote([string]$Command) {
    $output = & $sshExe @sshOptions $Ssh $Command 2>&1
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = ($output | Out-String).TrimEnd() }
}

$head = (git ls-remote origin "refs/heads/$Branch") -split '\s+' | Select-Object -First 1
if (-not $head) { Write-Host "origin has no branch $Branch; push it first (this tool pushes nothing)."; exit 2 }
Write-Host "$Branch at $($head.Substring(0, 9)) on origin; checking on $Machine ($Ssh)"

$remote = "$RemoteWork\remote"
$made = Invoke-Remote "New-Item -ItemType Directory -Force -Path '$remote' | Out-Null"
if ($made.Code) { throw "ssh to $Ssh failed: $($made.Text)" }
foreach ($file in 'start.ps1', 'run.ps1') {
    & $scpExe @sshOptions -q (Join-Path $PSScriptRoot "remote-check\$file") "${Ssh}:$($remote.Replace('\', '/'))/$file"
    if ($LASTEXITCODE) { throw "scp of $file to $Ssh failed" }
}

$deadline = (Get-Date).AddMinutes($TimeoutMinutes)
while ($true) {
    $start = Invoke-Remote "powershell -NoProfile -ExecutionPolicy Bypass -File '$remote\start.ps1' -Branch '$Branch' -Name '$name' -Repo '$RemoteRepo' -Work '$RemoteWork'"
    if ($start.Code -eq 0) { Write-Host $start.Text; break }
    if ($start.Code -ne 4 -or -not $Wait -or (Get-Date) -gt $deadline) {
        Write-Host $start.Text
        exit $(if ($start.Code -in 4, 5) { $start.Code } else { 1 })
    }
    Write-Host "$Machine is busy; trying again in 60 s: $($start.Text -replace '\s+', ' ')"
    Start-Sleep -Seconds 60
}

$log = "$RemoteWork\$name.log"
$shown = 0
$text = ''
while ($true) {
    Start-Sleep -Seconds 20
    $read = Invoke-Remote "Get-Content -LiteralPath '$log' -ErrorAction SilentlyContinue"
    $lines = @($read.Text -split "`r?`n")
    for ($i = $shown; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^(-- |remote check|checked out|.*waiting:)') { Write-Host $lines[$i] }
    }
    $shown = [math]::Max($shown, $lines.Count)
    if ($read.Text -match '(?m)^EXIT=') { $text = $read.Text; break }
    # The task stays registered while it runs, so start.ps1 refuses a second check meanwhile.
    if ((Get-Date) -gt $deadline) { Write-Host "no result after $TimeoutMinutes minutes; the task rl-remote-check-$name keeps running on $Machine"; exit 3 }
}
Invoke-Remote "Unregister-ScheduledTask -TaskName 'rl-remote-check-$name' -Confirm:`$false -ErrorAction SilentlyContinue" | Out-Null

$code = if ($text -match '(?m)^EXIT=(-?\d+)') { [int]$Matches[1] } else { 1 }
# This run's folder, from dev.py's own Logs: line (earlier runs sit beside it).
$run = if ($text -match '(?m)^Logs: (.+)$') { $Matches[1].Trim() } else { $null }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
Set-Content -LiteralPath (Join-Path $Out 'remote.log') -Value $text -Encoding utf8
$copied = $null
if ($run -and (Invoke-Remote "if (Test-Path -LiteralPath '$run') { 'yes' }").Text -eq 'yes') {
    & $scpExe @sshOptions -q -r "${Ssh}:$($run.Replace('\', '/'))" $Out
    if ($LASTEXITCODE) { Write-Host "could not copy $run from $Machine" } else { $copied = Join-Path $Out (Split-Path -Leaf $run) }
}
# The check's own table, as printed on the remote machine.
$table = $text -split "`r?`n" | Where-Object { $_ -match '^(Nothing to check|No check for|check |\S+\s+(PASS|FAIL|SKIP|NOT RUN)\b|Logs: )' }
$table | ForEach-Object { Write-Host $_ }
Write-Host "Result on ${Machine}: exit $code; copied to $(if ($copied) { $copied } else { $Out })"
exit $code
