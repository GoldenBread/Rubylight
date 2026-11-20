# Laptop side of the laptop-host rig (Windows PowerShell 5.1, run in the console session by the
# Interactive scheduled task start.ps1 registers): motion_probe at a game's cadence on the laptop's
# own panel, captured by an isolated Rubylight test host (the firewall-approved package in
# -Package) on -Bind:-Port. Runs until <Root>\stop appears or -Seconds pass. Refuses to start
# anything when the console user was at the laptop less than -IdleSeconds ago, or when that cannot
# be measured: the reason goes to rig.log and to the done file ("refused: ..."), which start.ps1
# turns into its exit 5. The measure is rust/release/user_idle.py (-UserIdle, run by -Python), not
# Windows' own idle time, which a stream or an install resets; the rig marks its own time with it
# (begin/end), so its stream does not count as the user's input for the next run.
# When the host ends, the laptop lock (-Lock) is removed if its text still starts with -Owner.
# Files in -Root: ready (the host and probe run), done (ended; "done" or "refused: ..."), stop
# (written by start.ps1 -Stop), pids ("<role> <id> <start time ticks>", for start.ps1 -Stop),
# rig.log. The profile in <Root>\profile is kept between runs so the client's pairing survives.
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
    [string]$Lock = 'C:\src\butterpollo-release\LAPTOP-LOCK',
    [string]$Python = 'python',
    [string]$UserIdle = 'C:\src\Rubylight\rust\release\user_idle.py'
)
$ErrorActionPreference = 'Stop'
$prof = "$Root\profile"
function Log([string]$m) { Add-Content "$Root\rig.log" "$(Get-Date -Format s) $m" }
# start.ps1 clears stop before the task starts. A stop file here means -Stop came while the build
# lock kept this script waiting: start nothing, or the host would run with nobody left to stop it.
Remove-Item "$Root\done", "$Root\ready" -ErrorAction SilentlyContinue
# Only a lock that is still this run's: another run may hold it by now.
function Remove-OurLock {
    if ((Test-Path $Lock) -and ((Get-Content $Lock -Raw) -like "$Owner *")) { Remove-Item $Lock -Force; Log 'laptop lock released' }
}
function Stop-Early([string]$Why) { Log $Why; Set-Content "$Root\done" $Why; Remove-OurLock; exit 0 }
if (Test-Path "$Root\stop") { Stop-Early 'done: stopped before the start' }
# start.ps1 may have lost the lock while this task waited (a -Stop after a dropped ssh call).
if (-not ((Test-Path $Lock) -and ((Get-Content $Lock -Raw) -like "$Owner *"))) { Stop-Early "refused: $Lock is no longer this run's ($Owner)" }
# Session 0 has no console user: an idle time measured there means nothing.
if ((Get-Process -Id $PID).SessionId -eq 0) { Stop-Early 'refused: host.ps1 runs in session 0, not in the console session' }

# The console user's idle time, measured here in the console session (over ssh, in session 0,
# it would mean nothing). Anything but two numbers refuses.
# Python's stderr (a warning) goes to user_idle.err: under 'Stop', Windows PowerShell would throw on
# its first line. Only the exit code and stdout count.
function Invoke-UserIdle([string[]]$PyArgs) {
    $ErrorActionPreference = 'Continue'
    $global:LASTEXITCODE = -1
    $text = ''
    try { $text = ((& $Python @PyArgs 2> "$Root\user_idle.err") | ForEach-Object { "$_" }) -join ' ' } catch { $text = $_.Exception.Message }
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = $text.Trim() }
}
$idleDir = Split-Path $UserIdle
$measured = Invoke-UserIdle @('-c', "import sys; sys.path.insert(0, r'$idleDir'); import user_idle; a, r = user_idle.idle(); print('%.1f %.1f' % (a, r))")
if ($measured.Code -ne 0 -or $measured.Text -notmatch '^([0-9.]+) ([0-9.]+)$') {
    Stop-Early "refused: the laptop user's idle time could not be measured ($UserIdle, exit $($measured.Code)): '$($measured.Text)', see $Root\user_idle.err"
}
$away = [double]$Matches[1]; $reported = [double]$Matches[2]
if ($away -lt $IdleSeconds) {
    Stop-Early "refused: the laptop's console user was active $([math]::Round($away)) s ago (needs $IdleSeconds s idle)"
}
# Mark the rig's own time: the input its stream causes is then not taken for the user's.
$idleMark = (Invoke-UserIdle @($UserIdle, 'begin')).Text
if ($idleMark -notmatch '^[0-9.]+ [0-9.]+$') { Log "user_idle.py begin failed ($idleMark): the next run may see this run's input as the user's"; $idleMark = $null }

New-Item -ItemType Directory -Force "$prof\record" | Out-Null
Add-Type -AssemblyName System.Windows.Forms
$display = [System.Windows.Forms.Screen]::PrimaryScreen.DeviceName
Log "display $display motion $Motion idle $([math]::Round($away)) s (Windows: $([math]::Round($reported)) s)"
@("port = $Port", 'encoder = amf', 'capture = wgc',
  'virtual_display_layout = extended', 'virtual_display_mode = disabled',
  'dd_configuration_option = disabled', 'dd_resolution_option = disabled',
  'dd_refresh_rate_option = disabled', 'dd_hdr_option = disabled', 'dd_always_restore_from_golden = false',
  'frame_limiter_enable = false', 'stream_audio = false', 'audio_sink_capture_only = true', 'upnp = false',
  'enable_discovery = false', 'vulkan_hdr_layer = false', 'system_tray = false', 'updates = off',
  'pyrowave = true', 'lan_encryption_mode = 2') | Set-Content "$prof\sunshine.conf"
'{"apps": [{"name": "Desktop", "cmd": "", "virtual-display": false}]}' | Set-Content "$prof\apps.json"
if (-not (Test-Path "$prof\sunshine_state.json")) {
    $salt = -join ((1..8) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })
    $sha = [Security.Cryptography.SHA256]::Create()
    $hash = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes('rust-smoke-only' + $salt))
    [array]::Reverse($hash)
    (@{ username = 'test'; salt = $salt; password = ([BitConverter]::ToString($hash) -replace '-', '') } | ConvertTo-Json -Compress) |
        Set-Content "$prof\sunshine_state.json"
}
$probe = $null; $hostProc = $null
try {
    $probe = Start-Process "$Root\motion_probe.exe" -PassThru -WindowStyle Hidden -RedirectStandardOutput "$Root\motion.log" `
        -RedirectStandardError "$Root\motion.err.log" -ArgumentList $display, ([math]::Min($Seconds, 3600)), "$Root\motion.json", $Motion
    Start-Sleep -Seconds 3
    # motion_probe takes 1..3600 seconds; the rig ends with it.
    if ($probe.HasExited) { throw "motion_probe exited at once: $(Get-Content "$Root\motion.err.log" -TotalCount 1 -ErrorAction SilentlyContinue)" }
    if ($WindowCapture) {
        # Only the probe's own presents make frames: monitor capture also composed for other
        # desktop changes, about 16 extra frames a second at other times.
        Add-Type -Namespace Probe -Name Native -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern System.IntPtr FindWindowW(string cls, string title);'
        $hwnd = [Probe.Native]::FindWindowW('ButterpolloMotionProbe', [NullString]::Value)
        if ($hwnd -eq [IntPtr]::Zero) { throw 'motion_probe window not found' }
        Add-Content "$prof\sunshine.conf" "output_name = window:$($hwnd.ToInt64())@$display"
        Log "window capture window:$($hwnd.ToInt64())@$display"
    }
    $hostProc = Start-Process "$Package\butterpollo.exe" -PassThru -WindowStyle Hidden -WorkingDirectory $prof `
        -RedirectStandardOutput "$prof\host.stdout.log" -RedirectStandardError "$prof\host.stderr.log" `
        -ArgumentList '--config-dir', $prof, '--assets', "`"$Package\assets\web`"", '--port', "$Port", '--bind', $Bind, '--no-tray'
    Log "probe $($probe.Id) host $($hostProc.Id)"
    @("probe $($probe.Id) $($probe.StartTime.Ticks)", "host $($hostProc.Id) $($hostProc.StartTime.Ticks)") | Set-Content "$Root\pids"
    Set-Content "$Root\ready" 'ready'
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline -and -not (Test-Path "$Root\stop")) {
        if ($hostProc.HasExited) { Log "host exited $($hostProc.ExitCode)"; break }
        if ($probe.HasExited) { Log "probe exited $($probe.ExitCode)"; break }
        Start-Sleep -Seconds 1
    }
} catch {
    Log "error: $($_.Exception.Message)"
} finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force }
    if ($probe -and -not $probe.HasExited) { Stop-Process -Id $probe.Id -Force }
    Log 'stopped'
    if ($idleMark) {
        $startAt, $before = $idleMark -split ' '
        $ended = Invoke-UserIdle @($UserIdle, 'end', $startAt, $before)
        if ($ended.Code -ne 0) { Log "user_idle.py end failed (exit $($ended.Code)): $($ended.Text)" }
    }
    Set-Content "$Root\done" 'done'
    Remove-OurLock
}
