# The laptop as host: an isolated Rubylight test host on the laptop (Radeon 780M) captures
# motion_probe on the laptop's own panel and streams to Nightjar on this PC, borderless fullscreen
# with VRR, so a client's cadence can be judged on a real VRR panel (loopback on one PC cannot show
# it) with a second AMD GPU as the host. The laptop side lives in tools/hw/laptop-host/ (start.ps1
# over ssh, host.ps1 in the laptop's console session) and is copied to -LaptopRoot on every run.
#
#   pwsh tools\hw\laptop-host.ps1 -Out D:\runs\lap1
#   pwsh tools\hw\laptop-host.ps1 -Out D:\runs\lap2 -ClientB C:\tests\nightjar\b\nightjar.exe -ExtraB @{ present_at_host_times = $true }
#   pwsh tools\hw\laptop-host.ps1 -Out D:\runs\lap3 -Package C:\src\butterpollo-release\<version>\release\butterpollo-rust-release
#
# Per codec, client/scripts/pyrowave-loopback.ps1 -RemoteHost streams from the laptop into
# <Out>\<codec> (one timing CSV per leg, summary.txt) and client/scripts/timing_breakdown.py writes
# <Out>\<codec>\breakdown.txt. The laptop's host log and rig log are copied to
# <Out>\laptop-butterpollo.log and <Out>\laptop-rig.log. The part of the host log written since the
# host started (start.ps1's size-and-head mark; a rotated log adds butterpollo.log.1) is searched
# for an AMD encoder hang after every failed leg and at the end; a log that cannot be copied or
# placed is reported as unknown, never as clean.
#
# -Package replaces the laptop's package folder (-LaptopPackage, the one its firewall allows
# butterpollo.exe from) before the run: copied to <LaptopPackage>.new-<run>, a folder of this run
# alone, then swapped in by rename while the laptop lock is held; refused while anything runs from
# that folder. The profile in <LaptopRoot>\profile is kept between runs, so the pairing with
# -ClientProfile survives.
#
# Refuses first when this PC's user is not idle (rust/release/user_idle.py: no input for 10 minutes,
# as tools/hw/run.py's idle precondition), since Nightjar takes this PC's monitor. On the laptop it
# takes C:\src\butterpollo-release\LAPTOP-LOCK (shared with the PC-host smoke test,
# rust/release/smoke.ps1; waits up to -WaitMinutes while another run holds it, never breaks it;
# released by the laptop side when its host ends) and, for as long as the host runs, the laptop's
# BUILD-LOCK (tools/build_lock.py), so heavy builds there wait; -NoQuiet leaves the build lock
# alone. It refuses when nobody or another user is at the laptop's console, when that user was
# active in the last -IdleSeconds (user_idle.py on the laptop), or when the laptop's installed host
# has a session or does not answer while its service runs. On this PC pyrowave-loopback.ps1 takes
# GPU-LOCK itself and refuses while this PC's installed host has a session.
# Exit codes: 0 done, 2 a leg failed (or the laptop rig did not start, or the hang search could not
# read the laptop's log), 3 AMD encoder hang suspected (stop and investigate, never retry), 4 this
# PC's user or the laptop is busy, locked, building or not idle (or this PC's installed host has a
# session).
#Requires -Version 7
param(
    [string]$Laptop = 'ramaz@192.168.4.12',
    [string]$LaptopAddress = '192.168.4.12',
    [int]$Port = 48123,
    [string]$LaptopRoot = 'C:\tests\laptop-host',
    [string]$LaptopPackage = 'C:\Users\ramaz\rc4\pkg',
    [string]$Package = '',
    [string]$LaptopRepo = 'C:\src\Rubylight',
    [string]$MotionProbe = 'C:\src\butterpollo-release\fixtures\motion_probe.exe',
    [string]$Motion = 'jitter:8.333,16.667,8.333,25,16.667',
    [switch]$WindowCapture,
    [string]$ClientA = 'C:\tests\nightjar\a\nightjar.exe',
    [string]$ClientB = '',
    [string]$ClientProfile = 'C:\tests\nj-laptop-host-client',
    [string[]]$Codecs = @('PyroWave'),
    [int]$Seconds = 45,
    [int]$Fps = 120,
    [int]$PyroWaveKbps = 150000,
    [int]$VideoKbps = 30000,
    [string]$Order,   # pyrowave-loopback's default: ABBA with -ClientB, else A
    [hashtable]$ExtraA = @{},   # settings.json keys over the default borderless VRR window, for the A legs
    [hashtable]$ExtraB = @{},   # and for the B legs
    [switch]$NoVrrCheck,   # NIGHTJAR_VRR_CHECK=0: keep the chosen pacing on a display without VRR
    [int]$IdleSeconds = 120,   # how long the laptop's console user must have been away
    [int]$WaitMinutes = 30,
    [switch]$NoQuiet,
    [Parameter(Mandatory)][string]$Out,
    [string]$Owner = 'laptop-host'   # lock text prefix; this run adds its process id
)
$ErrorActionPreference = 'Stop'
# Git's ssh has no keys here.
$ssh = 'C:\Windows\System32\OpenSSH\ssh.exe'
$scp = 'C:\Windows\System32\OpenSSH\scp.exe'
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$remoteRoot = $LaptopRoot -replace '\\', '/'
$remotePackage = $LaptopPackage.TrimEnd('\') -replace '\\', '/'
# This PC's idle rule, as tools/hw/run.py's idle precondition (IDLE_SECONDS).
$pcIdleSeconds = 600
New-Item -ItemType Directory -Force $Out | Out-Null
# Per run: start.ps1 -Stop and -ClearNew touch only what carries this exact owner.
$runOwner = "$Owner-$PID"

# The laptop's shell is Windows PowerShell 5.1, which mangles script text such as $_ inside an ssh
# command: only start.ps1 runs there, with each value single-quoted (the laptop's PowerShell parses
# the line first; an unquoted comma would split -Motion).
function Invoke-Start([string[]]$Arguments) {
    $line = "powershell -NoProfile -ExecutionPolicy Bypass -File $LaptopRoot\start.ps1 " + ($Arguments -join ' ')
    $text = (& $ssh $Laptop $line 2>&1 | ForEach-Object { "$_" }) -join "`n"
    $code = $LASTEXITCODE
    # ssh does not always pass the script's exit code on: its "laptop-host:" line says what happened.
    $outcome = if ($text -match '(?m)^laptop-host: (.*)$') { $Matches[1].Trim() } else { '' }
    [pscustomobject]@{ Code = $code; Text = $text; Outcome = $outcome }
}
function Copy-FromLaptop([string]$Path, [string]$To) {
    Remove-Item $To -ErrorAction SilentlyContinue
    & $scp -q "${Laptop}:$remoteRoot/$Path" $To 2>$null | Out-Host
    if ($LASTEXITCODE -or -not (Test-Path $To)) { Write-Host "could not copy $Path from the laptop"; return $false }
    $true
}
# The bytes of the laptop's host log written since $Mark (size, head), or $null when they cannot be
# placed: a log that cannot be copied, or rotated more than once since the mark.
function Get-RunLog($Mark) {
    if (-not (Copy-FromLaptop 'profile/logs/butterpollo.log' "$Out\laptop-butterpollo.log")) { return $null }
    $data = [IO.File]::ReadAllBytes("$Out\laptop-butterpollo.log")
    $follows = { param($bytes) $bytes.Length -ge $Mark.Size -and [Linq.Enumerable]::SequenceEqual([byte[]]$bytes[0..($Mark.Head.Length - 1)], [byte[]]$Mark.Head) }
    if ($Mark.Head.Length -eq 0 -or (& $follows $data)) {
        return [Text.Encoding]::UTF8.GetString($data, [int]$Mark.Size, $data.Length - [int]$Mark.Size)
    }
    # Rotated since the mark: the start of this run is in butterpollo.log.1.
    if (-not (Copy-FromLaptop 'profile/logs/butterpollo.log.1' "$Out\laptop-butterpollo.log.1")) { return $null }
    $older = [IO.File]::ReadAllBytes("$Out\laptop-butterpollo.log.1")
    if (-not (& $follows $older)) { Write-Host 'the laptop host log rotated more than once during this run'; return $null }
    [Text.Encoding]::UTF8.GetString($older, [int]$Mark.Size, $older.Length - [int]$Mark.Size) + [Text.Encoding]::UTF8.GetString($data)
}
# 'hang', 'clean' or 'unknown' for this run's part of the laptop host log.
function Test-LaptopHang($Mark) {
    if (-not $Mark) { Write-Host 'laptop host log: no mark from start.ps1, hang state unknown'; return 'unknown' }
    $text = Get-RunLog $Mark
    if ($null -eq $text) { Write-Host 'laptop host log: could not be read, hang state unknown'; return 'unknown' }
    $hangs = @($text -split "`n" | Where-Object { $_ -match '(?i)gpu hang|encoder hang|device removed' }).Count
    Write-Host "laptop host log: $hangs hang lines in this run"
    if ($hangs) { 'hang' } else { 'clean' }
}

# $null when this PC's user has been away long enough, else why not (user_idle.py; stderr is not parsed).
function Test-PcIdle {
    $idleLine = ((& python -c "import sys; sys.path.insert(0, r'$repo\rust\release'); import user_idle; a, r = user_idle.idle(); print('%.0f %s' % (a, user_idle.describe(a, r)))" 2>$null) -join ' ').Trim()
    if ($LASTEXITCODE -or $idleLine -notmatch '^(\d+) (.*)$') { return "this PC's idle time could not be measured: '$idleLine' (exit $LASTEXITCODE)" }
    if ([int]$Matches[1] -lt $pcIdleSeconds) { return "this PC's user is not idle ($($Matches[2]); needs $($pcIdleSeconds / 60) min)" }
    $null
}

function Invoke-Rig {
    $rigTaken = $false; $rigUp = $false; $code = 0; $mark = $null
    $ownerArgs = @("-Package '$LaptopPackage'", "-Owner '$runOwner'")
    try {
        # --- This PC: Nightjar takes its monitor -------------------------------------------
        $busy = Test-PcIdle
        if ($busy) { Write-Host "${busy}: not starting"; return 4 }

        # --- Deploy: the repo is the source of the laptop's scripts ---------------------------
        & $ssh $Laptop "New-Item -ItemType Directory -Force $LaptopRoot | Out-Null" | Out-Host
        if ($LASTEXITCODE) { Write-Host "the laptop ($Laptop) is not reachable over ssh"; return 2 }
        & $scp -q "$PSScriptRoot\laptop-host\start.ps1" "$PSScriptRoot\laptop-host\host.ps1" "${Laptop}:$remoteRoot/" | Out-Host
        if ($LASTEXITCODE) { Write-Host 'copying start.ps1 and host.ps1 to the laptop failed'; return 2 }
        $probeInfo = Invoke-Start @("-Root '$LaptopRoot'", "-Package '$LaptopPackage'", '-Info')
        if ($probeInfo.Text -notmatch '(?m)^probe-length=(\d+)') { Write-Host $probeInfo.Text; Write-Host 'start.ps1 -Info did not answer'; return 2 }
        $remoteProbe = [long]$Matches[1]
        if (-not (Test-Path $MotionProbe)) { Write-Host "motion_probe not found at $MotionProbe"; return 2 }
        if ($remoteProbe -ne (Get-Item $MotionProbe).Length) {
            Write-Host "copying motion_probe.exe to the laptop"
            & $scp -q $MotionProbe "${Laptop}:$remoteRoot/motion_probe.exe" | Out-Host
            if ($LASTEXITCODE) { Write-Host 'copying motion_probe.exe failed'; return 2 }
        }

        # --- Package: copied into this run's own folder, swapped in by start.ps1 under the lock -
        if ($Package) {
            $Package = $Package.TrimEnd('\')
            if (-not (Test-Path "$Package\butterpollo.exe")) { Write-Host "no butterpollo.exe in $Package"; return 2 }
            if ($probeInfo.Text -match '(?m)^package-users=(.+)$') { Write-Host "refused: $($Matches[1].Trim()) run from $LaptopPackage on the laptop"; return 4 }
            $cleared = Invoke-Start ($ownerArgs + '-ClearNew')
            if ($cleared.Outcome -ne 'cleared') { Write-Host $cleared.Text; return 2 }
            Write-Host "copying $Package to $LaptopPackage.new-$runOwner on the laptop"
            & $scp -q -r $Package "${Laptop}:$remotePackage.new-$runOwner" | Out-Host
            if ($LASTEXITCODE) {
                Invoke-Start ($ownerArgs + '-ClearNew') | Out-Null
                Write-Host 'copying the package failed; the laptop package is unchanged'; return 2
            }
        } elseif ($probeInfo.Text -match '(?m)^version=(.+)$') {
            Write-Host "laptop host: $($Matches[1].Trim())"
        }

        # --- Start the laptop rig -------------------------------------------------------------
        $legs = [math]::Max(1, $(if ($Order) { $Order.Length } elseif ($ClientB) { 4 } else { 1 }))
        # motion_probe runs for at most an hour, and the rig with it: the legs must fit, and the rest
        # of the hour covers pyrowave-loopback.ps1's wait for this PC's GPU-LOCK.
        $rigSeconds = 3600
        $needed = $Codecs.Count * ($legs * ($Seconds + 60) + 180) + 300
        if ($needed -gt $rigSeconds) { Write-Host "these legs need about $needed s and one rig runs for $rigSeconds s: use fewer codecs or legs"; return 2 }
        $startArgs = @("-Root '$LaptopRoot'", "-Port $Port", "-Bind '$LaptopAddress'", "-Seconds $rigSeconds", "-Motion '$Motion'",
            "-IdleSeconds $IdleSeconds", "-LaptopRepo '$LaptopRepo'") + $ownerArgs
        if ($WindowCapture) { $startArgs += '-WindowCapture' }
        if ($NoQuiet) { $startArgs += '-NoQuiet' }
        if ($Package) { $startArgs += '-Swap' }
        $deadline = (Get-Date).AddMinutes($WaitMinutes); $said = $false
        # From here the laptop may hold this run's lock (an interrupted start, a dropped ssh):
        # finally runs -Stop, which touches nothing that is not this run's.
        $rigTaken = $true
        while ($true) {
            $start = Invoke-Start $startArgs
            if ($start.Outcome -like 'lock held*') {
                if (-not $said) { Write-Host "waiting for the laptop: $($start.Outcome)"; $said = $true }
                if ((Get-Date) -gt $deadline) { Write-Host "the laptop lock is still held after $WaitMinutes minutes"; return 4 }
                Start-Sleep -Seconds 60
                continue
            }
            Write-Host $start.Text
            if ($start.Text -match '(?m)^host-log-mark=(\d+) ?(\S*)') {
                $mark = [pscustomobject]@{ Size = [long]$Matches[1]; Head = [Convert]::FromBase64String($Matches[2]) }
            }
            if ($start.Outcome -like 'up*') { $rigUp = $true; break }
            if ($start.Outcome -like 'building*') { Write-Host "the laptop is building: $($start.Outcome -replace '^building: ', '')"; return 4 }
            if ($start.Outcome -match '^(refused|package in use)') { return 4 }
            Write-Host "the laptop rig did not start (exit $($start.Code))"
            return 2
        }
        Write-Host "laptop rig up at $(Get-Date -Format T)"

        # --- The desktop leg: Nightjar on this PC ---------------------------------------------
        # Without a VRR panel the display check would turn both arms into V-Sync: keep the chosen pacing.
        if ($NoVrrCheck) { $env:NIGHTJAR_VRR_CHECK = '0' }
        $base = @{ window_mode = 'Borderless'; vrr = $true }
        $a = $base.Clone(); foreach ($k in $ExtraA.Keys) { $a[$k] = $ExtraA[$k] }
        $b = $base.Clone(); foreach ($k in $ExtraB.Keys) { $b[$k] = $ExtraB[$k] }
        foreach ($codec in $Codecs) {
            $codecOut = Join-Path $Out $codec
            $leg = @{
                ClientA = $ClientA; Codec = $codec; Fps = $Fps; Seconds = $Seconds
                Bitrates = @($(if ($codec -eq 'PyroWave') { $PyroWaveKbps } else { $VideoKbps }))
                DecoderA = 'Gpu'; DecoderB = 'Gpu'; RemoteHost = $LaptopAddress; Port = $Port
                ClientProfile = $ClientProfile; ExtraA = $a; ExtraB = $b; Out = $codecOut; Metric = 'receive_to_present_ms'
            }
            if ($ClientB) { $leg.ClientB = $ClientB }
            if ($Order) { $leg.Order = $Order }
            # The waits before a leg (the laptop lock, GPU-LOCK, earlier legs) can last an hour.
            $busy = Test-PcIdle
            if ($busy) { Write-Host "${busy}: stopping before the $codec leg"; return 4 }
            # 4: the rig must still be this run's (it may have outlived its -Seconds and another run
            # started its host on the same port and profile).
            $check = Invoke-Start @("-Root '$LaptopRoot'", "-Package '$LaptopPackage'", "-Owner '$runOwner'", '-Info')
            if ($check.Text -notmatch '(?m)^rig-ours=yes') {
                Write-Host "the laptop rig is no longer this run's ($(if ($check.Text -match '(?m)^rig-ours=(.*)$') { $Matches[1] } else { 'no answer' })): stopping before the $codec leg"
                return 2
            }
            $global:LASTEXITCODE = 0
            try {
                & "$repo\client\scripts\pyrowave-loopback.ps1" @leg | Out-Host
                $legCode = $LASTEXITCODE
            } catch {
                Write-Host "$codec leg stopped: $($_.Exception.Message)"
                $legCode = 2
            }
            Write-Host "$codec exit $legCode"
            if ($legCode -eq 3) { Write-Host 'AMD encoder hang suspected: stopping'; return 3 }
            if ($legCode -eq 4) { Write-Host "this PC is not idle (pyrowave-loopback.ps1 exit 4)"; return 4 }
            if ($legCode -ne 0) {
                # The next codec would stream from the same encoder: look for a hang first.
                $state = Test-LaptopHang $mark
                if ($state -eq 'hang') { $script:hangFound = $true; return 3 }
                if ($state -eq 'unknown') { Write-Host 'not going on without knowing whether the laptop encoder hung'; return 2 }
                $code = 2
                continue
            }
            python "$repo\client\scripts\timing_breakdown.py" $codecOut --fps $Fps *> "$codecOut\breakdown.txt"
        }
        return $code
    } catch {
        Write-Host "stopped: $($_.Exception.Message)"
        return 2
    } finally {
        # An upload that was never swapped in (lock wait over, refused, building) is not left beside
        # the firewall-approved folder; after a swap there is nothing to clear.
        if ($Package) { Invoke-Start ($ownerArgs + '-ClearNew') | Out-Null }
        if ($rigTaken) {
            $stopped = Invoke-Start @("-Root '$LaptopRoot'", "-Owner '$runOwner'", '-Stop')
            Write-Host $stopped.Text
            if ($stopped.Outcome -notmatch '^(stopped|not ours)') {
                Write-Host "start.ps1 -Stop did not answer: check C:\src\butterpollo-release\LAPTOP-LOCK on the laptop (text '$runOwner ...')"
            }
            [void](Copy-FromLaptop 'rig.log' "$Out\laptop-rig.log")
            if (-not $NoQuiet) { [void](Copy-FromLaptop 'buildlock.log' "$Out\laptop-buildlock.log") }
            if ($mark) {
                $state = Test-LaptopHang $mark
                if ($state -eq 'hang') { $script:hangFound = $true }
                if ($state -eq 'unknown') { $script:hangUnknown = $true }
            } elseif ($rigUp) {
                Write-Host 'laptop host log: no mark from start.ps1, hang state unknown'
                $script:hangUnknown = $true
            }
        }
    }
}

$script:hangFound = $false; $script:hangUnknown = $false
# Commands print to Out-Host; should anything else reach the pipeline, the code is still the last number.
$result = @(Invoke-Rig | Where-Object { $_ -is [int] })[-1]
if ($script:hangFound) { Write-Host "AMD encoder hang suspected on the laptop: see $Out\laptop-butterpollo.log. Do not retry"; exit 3 }
if ($script:hangUnknown -and $result -eq 0) { Write-Host 'the laptop host log could not be searched for a hang: not reporting this run as done'; exit 2 }
exit [int]$result
