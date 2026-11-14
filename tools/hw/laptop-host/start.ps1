# Laptop side of the laptop-host rig, run over ssh (session 0, Windows PowerShell 5.1) by
# tools/hw/laptop-host.ps1 on the PC: takes the laptop lock, then starts host.ps1 in the console
# session through a one-time Interactive scheduled task (as rust/release/smoke.ps1 does), wrapped in
# the laptop's build lock so heavy builds there wait while the rig is up.
#
#   start.ps1 -Owner <run> [-Seconds N] [-Motion <cadence>] [-WindowCapture]   # start the rig
#   start.ps1 -Owner <run> -Swap ...       # first swap in <Package>.new-<run>, under the lock
#   start.ps1 -Owner <run> -Stop           # stop this run's rig, free this run's lock
#   start.ps1 [-Owner <run>] -Info         # probe size, package users, host version, rig-ours
#   start.ps1 -Owner <run> -ClearNew       # remove this run's <Package>.new-<run>
#
# The lock is C:\src\butterpollo-release\LAPTOP-LOCK ("<Owner> <time>", created atomically; the
# PC-host smoke test rust/release/smoke.ps1 takes the same one). The old C:\tests\LAP-LOCK counts
# as held while it exists. Only a lock whose text starts with -Owner is ever removed: host.ps1
# removes it when the host ends, -Stop when it stops the rig. -Stop touches only the rig this
# owner started (<Root>\owner); the PC passes an owner per run. With -Swap, the folder the PC has
# just copied for this run (<Package>.new-<Owner>) is swapped in by rename while the lock is held,
# so a failed copy never leaves the firewall-approved folder half-written, and a failed rename
# puts the old folder back.
# Refuses when nobody or another user is at the console, when the laptop's installed host has a
# session or its service runs but does not answer, or (host.ps1, in the console session) when the
# console user was active in the last -IdleSeconds by rust/release/user_idle.py in -LaptopRepo, or
# that cannot be measured. -NoQuiet runs host.ps1 without the build lock.
# Prints host-log-mark=<size> <base64 of the first 256 bytes> of the test host's log just before
# the host starts, so the PC searches only what this run wrote.
# One line starting "laptop-host:" says the outcome (ssh does not always pass the exit code on).
# Exit codes: 0 up, 2 the host or probe ended early (or did not start), 4 lock held, the laptop is
# building, or its package is in use, 5 refused (not idle, wrong user, installed host in use).
param(
    [string]$Root = 'C:\tests\laptop-host',
    [string]$Package = 'C:\Users\ramaz\rc4\pkg',
    [int]$Port = 48123,
    [string]$Bind = '192.168.4.12',
    [int]$Seconds = 1500,
    [string]$Motion = 'jitter:8.333,16.667,8.333,25,16.667',
    [switch]$WindowCapture,
    [int]$IdleSeconds = 120,
    [string]$Owner = 'laptop-host',
    [string]$LaptopRepo = 'C:\src\Rubylight',
    [int]$BuildWaitSeconds = 120,   # how long the task waits for the laptop's BUILD-LOCK
    [switch]$NoQuiet,
    [switch]$Swap,
    [switch]$Stop,
    [switch]$Info,
    [switch]$ClearNew
)
$ErrorActionPreference = 'Stop'
$Package = $Package.TrimEnd('\')
$staging = "$Package.new-$Owner"
$lockDir = 'C:\src\butterpollo-release'
$lock = "$lockDir\LAPTOP-LOCK"
$oldLock = 'C:\tests\LAP-LOCK'
$name = 'rubylight-laptop-host'

function Say([string]$m) { Write-Host "laptop-host: $m" }
function Test-OurLock { (Test-Path $lock) -and ((Get-Content $lock -Raw) -like "$Owner *") }
function Remove-OurLock { if (Test-OurLock) { Remove-Item $lock -Force } }
function Test-OurRig { (Test-Path "$Root\owner") -and ((Get-Content "$Root\owner" -Raw).Trim() -eq $Owner) }
# After host.ps1 has written done, build_lock.py still removes its BUILD-LOCK lease: let the task
# end by itself before stopping it, or the lease would stay behind.
function Wait-TaskEnd([int]$Limit = 20) {
    foreach ($i in 1..($Limit * 2)) {
        if ("$((Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue).State)" -ne 'Running') { return }
        Start-Sleep -Milliseconds 500
    }
}
function Remove-Task {
    # host.ps1 frees the lock as it ends: a run that took it since then owns the task and the files.
    if (-not (Test-OurRig)) { Write-Host 'the rig is another run''s by now: leaving its task alone'; return }
    # The stop file first: a host.ps1 still waiting for the build lock then starts nothing. The rig
    # does not rely on Stop-ScheduledTask ending the task's child processes.
    Set-Content "$Root\stop" 'stop'
    Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
}
# The processes host.ps1 recorded in <Root>\pids ("<role> <id> <start time ticks>"), still running.
function Get-RigProcesses {
    if (-not (Test-Path "$Root\pids")) { return @() }
    @(foreach ($row in Get-Content "$Root\pids") {
        $role, $id, $ticks = $row -split ' '
        $p = Get-Process -Id ([int]$id) -ErrorAction SilentlyContinue
        if ($p -and $p.StartTime.Ticks -eq [long]$ticks) { $p }
    })
}
function Get-PackageUsers {
    $prefix = "$Package\"
    @(Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) } |
        ForEach-Object { "$($_.Name) ($($_.Id))" })
}
function Get-HostVersion {
    $exe = "$Package\butterpollo.exe"
    if (-not (Test-Path $exe)) { return "none: $exe is missing" }
    $text = ''
    try { $text = ((& $exe --version 2>$null) -join ' ').Trim() } catch { }
    if (-not $text) { $text = "file version $((Get-Item $exe).VersionInfo.FileVersion)" }
    $text
}
function Get-RigTail { if (Test-Path "$Root\rig.log") { Get-Content "$Root\rig.log" -Tail 5 } }

if ($Info) {
    $probe = Get-Item "$Root\motion_probe.exe" -ErrorAction SilentlyContinue
    Write-Host "probe-length=$(if ($probe) { $probe.Length } else { 0 })"
    Write-Host "package-users=$((Get-PackageUsers) -join ', ')"
    Write-Host "version=$(Get-HostVersion)"
    $ours = if (-not (Test-OurRig)) { 'no: the rig is not this run''s' }
        elseif (-not (Test-OurLock)) { 'no: the lock is not this run''s' }
        elseif (-not (Test-Path "$Root\ready") -or (Test-Path "$Root\done")) { 'no: the host is not running' }
        else { 'yes' }
    Write-Host "rig-ours=$ours"
    exit 0
}

if ($ClearNew) {
    # Only this run's staging folder: another run may be copying into its own.
    if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
    Say 'cleared'
    exit 0
}

if ($Stop) {
    # Another run may have the laptop by now: touch nothing that is not ours.
    if (-not (Test-OurRig)) {
        Remove-OurLock
        Say "not ours: the rig is $(if (Test-Path "$Root\owner") { (Get-Content "$Root\owner" -Raw).Trim() } else { 'not started' }), the lock $(if (Test-Path $lock) { Get-Content $lock -Raw } else { 'free' })"
        exit 0
    }
    Set-Content "$Root\stop" 'stop'
    foreach ($i in 1..30) { if (Test-Path "$Root\done") { break }; Start-Sleep -Seconds 1 }
    Wait-TaskEnd
    Remove-Task
    # host.ps1 did not end by itself: end what it started, by id and start time.
    $left = if (Test-OurRig) { Get-RigProcesses } else { @() }
    foreach ($p in $left) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    Remove-OurLock
    Get-RigTail
    if ($left.Count) { Say "stopped (host.ps1 did not end; ended $(($left | ForEach-Object { "$($_.Name) $($_.Id)" }) -join ', '))" }
    else { Say 'stopped' }
    exit 0
}

# --- Lock ------------------------------------------------------------------------------------
if (Test-Path $oldLock) { Say "lock held: $oldLock ($(Get-Content $oldLock -Raw))"; exit 4 }
New-Item -ItemType Directory -Force $lockDir | Out-Null
try {
    $stream = [IO.File]::Open($lock, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    $writer = New-Object IO.StreamWriter($stream)
    $writer.Write("$Owner $(Get-Date -Format s)")
    $writer.Dispose()
} catch [IO.IOException] {
    Say "lock held: $(Get-Content $lock -Raw -ErrorAction SilentlyContinue)"
    exit 4
}
function Quit([int]$code, [string]$m) { Remove-OurLock; Say $m; exit $code }
# host.ps1 frees the lock just before its task ends: a task still running is the last run's.
if ("$((Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue).State)" -eq 'Running') {
    Quit 4 "lock held: the last run's task $name is still running"
}

# --- Refusals --------------------------------------------------------------------------------
$console = (Get-CimInstance Win32_ComputerSystem).UserName
if (-not $console) { Quit 5 'refused: nobody is signed in at the console' }
if (($console -split '\\')[-1] -ne $env:USERNAME) { Quit 5 "refused: the console session is ${console}'s, not $env:USERNAME's" }
$serverInfo = $null
try {
    $serverInfo = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 'http://127.0.0.1:47989/serverinfo').Content
} catch {
    # A running installed host that does not answer may be mid-stream or hung: not idle. Without
    # the service (or with it stopped) there is no installed host to disturb.
    $service = Get-Service ApolloService -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped') {
        Quit 5 "refused: the laptop's installed host (service $($service.Status)) does not answer on 47989"
    }
    Write-Host "no installed host service running on the laptop: treating it as free"
}
if ($serverInfo) {
    foreach ($key in 'RustHostSessionCount', 'RustHostPendingSessionCount') {
        if ($serverInfo -notmatch "<$key>0</$key>") { Quit 5 "refused: the laptop's installed host reports $key not 0" }
    }
}

# --- Package swap, while the lock is ours ----------------------------------------------------
$leaf = Split-Path $Package -Leaf
# A swap that stopped half-way and could not put the old folder back left only <Package>.old.
if (-not (Test-Path $Package) -and (Test-Path "$Package.old")) {
    Rename-Item "$Package.old" $leaf
    Write-Host "put $Package.old back as $Package"
}
# Staging folders of runs killed between their copy and their swap: <leaf>.new-<owner> only,
# older than a day; never .old, never the package.
Get-ChildItem (Split-Path $Package) -Directory -Filter "$leaf.new-*" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like "$leaf.new-?*" -and $_.FullName -ne $staging -and $_.CreationTime -lt (Get-Date).AddDays(-1) } |
    ForEach-Object {
        try { Remove-Item $_.FullName -Recurse -Force; Write-Host "removed the stale staging folder $($_.FullName)" }
        catch { Write-Host "could not remove the stale staging folder $($_.FullName): $($_.Exception.Message)" }
    }
if ($Swap) {
    if (-not (Test-Path "$staging\butterpollo.exe")) { Quit 2 "failed: -Swap but $staging has no butterpollo.exe" }
    $users = Get-PackageUsers
    if ($users.Count) { Quit 4 "package in use: $($users -join ', ') run from $Package" }
    try {
        if (Test-Path "$Package.old") { Remove-Item "$Package.old" -Recurse -Force }
        if (Test-Path $Package) { Rename-Item $Package "$leaf.old" }
        Rename-Item $staging $leaf
    } catch {
        $why = $_.Exception.Message
        if (-not (Test-Path $Package) -and (Test-Path "$Package.old")) {
            try { Rename-Item "$Package.old" $leaf; $why += '; the old package is back' }
            catch { $why += "; putting $Package.old back failed too: $($_.Exception.Message)" }
        }
        Quit 2 "failed: the package swap stopped: $why"
    }
    # The swap is done: a leftover .old is only disk space, removed by the next swap.
    try { if (Test-Path "$Package.old") { Remove-Item "$Package.old" -Recurse -Force } }
    catch { Write-Host "could not remove $Package.old yet: $($_.Exception.Message)" }
    Write-Host "swapped in the new package at $Package"
}
Write-Host "host version: $(Get-HostVersion)"
if (-not (Test-Path "$Package\butterpollo.exe")) { Quit 2 "failed: $Package\butterpollo.exe is missing" }
if (-not (Test-Path "$Root\host.ps1")) { Quit 2 "failed: $Root\host.ps1 is missing" }
if (-not (Test-Path "$Root\motion_probe.exe")) { Quit 2 "failed: $Root\motion_probe.exe is missing" }

# --- The task --------------------------------------------------------------------------------
$ps = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
# Python measures the console user's idle time (user_idle.py) and holds the build lock. The Store's
# python.exe stub in WindowsApps only opens the Store.
$python = Get-Command python -All -ErrorAction SilentlyContinue | Where-Object { $_.Source -notlike '*\WindowsApps\*' } | Select-Object -First 1
if (-not $python) { Quit 2 'failed: python is not on PATH (user_idle.py and the build lock need it)' }
$userIdle = "$LaptopRepo\rust\release\user_idle.py"
if (-not (Test-Path $userIdle)) { Quit 2 "failed: $userIdle is missing" }
$hostArgs = "-Root '$Root' -Package '$Package' -Port $Port -Bind '$Bind' -Seconds $Seconds -Motion '$Motion' -IdleSeconds $IdleSeconds" +
    " -Owner '$Owner' -Lock '$lock' -Python '$($python.Source)' -UserIdle '$userIdle'" +
    $(if ($WindowCapture) { ' -WindowCapture' } else { '' })
if ($NoQuiet) {
    $command = "& '$Root\host.ps1' $hostArgs"
    $wait = 20
} else {
    $buildLock = "$LaptopRepo\tools\build_lock.py"
    if (-not (Test-Path $buildLock)) { Quit 2 "failed: $buildLock is missing (-NoQuiet runs without the build lock)" }
    $command = "& '$($python.Source)' '$buildLock' --timeout $BuildWaitSeconds --label '$Owner' '--' '$ps' -NoProfile -ExecutionPolicy Bypass -File '$Root\host.ps1' $hostArgs *> '$Root\buildlock.log'"
    $wait = $BuildWaitSeconds + 60
}
Remove-Item "$Root\stop", "$Root\done", "$Root\ready", "$Root\pids", "$Root\buildlock.log" -ErrorAction SilentlyContinue
Set-Content "$Root\owner" $Owner
Add-Content "$Root\rig.log" "$(Get-Date -Format s) --- start by $Owner"
# Where the test host's log ends now: what comes after it is this run's.
$hostLog = "$Root\profile\logs\butterpollo.log"
if (Test-Path $hostLog) {
    $file = New-Object IO.FileStream($hostLog, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    try {
        $head = New-Object byte[] ([math]::Min(256, $file.Length))
        [void]$file.Read($head, 0, $head.Length)
        Write-Host "host-log-mark=$($file.Length) $([Convert]::ToBase64String($head))"
    } finally { $file.Dispose() }
} else {
    Write-Host 'host-log-mark=0 '
}
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
$action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\conhost.exe" -WorkingDirectory $Root `
    -Argument "--headless `"$ps`" -NoProfile -ExecutionPolicy Bypass -EncodedCommand $encoded"
$principal = New-ScheduledTaskPrincipal -UserId $console -LogonType Interactive
# A laptop on battery would leave the task queued with the default settings.
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 2)
Register-ScheduledTask -TaskName $name -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $name

# Read beside the writer; a log that cannot be read is said, not taken as empty.
function Read-BuildLog {
    $path = "$Root\buildlock.log"
    if (-not (Test-Path $path)) { return '' }
    try {
        $file = New-Object IO.FileStream($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
        try { (New-Object IO.StreamReader($file)).ReadToEnd() } finally { $file.Dispose() }
    } catch {
        $script:buildLogError = $_.Exception.Message
        ''
    }
}
$script:buildLogError = $null
$started = Get-Date
while ($true) {
    if ((Test-Path "$Root\ready") -or (Test-Path "$Root\done")) { break }
    $buildText = Read-BuildLog
    if ($buildText -match 'BUILD-LOCK busy') { break }
    $elapsed = ((Get-Date) - $started).TotalSeconds
    $state = (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue).State
    if ($elapsed -gt 10 -and "$state" -ne 'Running' -and "$state" -ne 'Queued') { break }
    if ($elapsed -gt $wait) { break }
    Start-Sleep -Milliseconds 500
}
if (Test-Path "$Root\ready") { Start-Sleep -Seconds 5 }

if (Test-Path "$Root\done") {
    $done = (Get-Content "$Root\done" -Raw).Trim()
    if ($done -like 'refused:*') { Wait-TaskEnd; Remove-Task; Quit 5 $done }
    Get-RigTail
    Say 'failed: the host or probe ended early'
    exit 2
}
if (Test-Path "$Root\ready") {
    Get-RigTail
    Say "up on ${Bind}:$Port"
    exit 0
}
$buildText = Read-BuildLog
if ($buildText -match 'BUILD-LOCK (?:waiting|busy)[^:]*: (.*)') {
    Remove-Task
    Quit 4 "building: $($Matches[1].Trim())"
}
Remove-Task
Get-RigTail
if ($buildText) { Write-Host $buildText.Trim() }
if ($script:buildLogError) { Write-Host "$Root\buildlock.log could not be read: $($script:buildLogError)" }
Quit 2 'failed: the host did not start'
