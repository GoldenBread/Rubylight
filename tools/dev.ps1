# Developer loop helpers. `check` runs the smallest set of checks for the
# files changed against the merge base with -Base (default origin/main) plus
# uncommitted ones, prints one PASS/FAIL table and keeps the logs in files
# (C:\src\butterpollo-release\unified\dev-check\<worktree>\<run>\). The mapping lives
# in tools/dev.py. `ci` watches GitHub CI for a pushed wip/** branch (tools/ci.py);
# it pushes nothing.
#   tools\dev.ps1 check -DryRun           # the plan only
#   tools\dev.ps1 check -Fast             # fast static + check-only loop
#   tools\dev.ps1 check -Base HEAD        # only uncommitted changes
#   tools\dev.ps1 ci wip/my-topic         # watch the runs its last push started
#   tools\dev.ps1 ci wip/my-topic -Dispatch host,android   # start them on the branch
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet('check', 'ci')] [string] $Command,
    [Parameter(Position = 1)] [string] $Branch,
    [string] $Base = 'origin/main',
    [string] $LogDir,
    [string] $TargetDir,
    [string[]] $Paths,
    [switch] $DryRun,
    [switch] $Fast,
    [switch] $KeepGoing,
    [switch] $FreshDependencies,
    [string] $LockDirectory,
    [string] $Pool,
    [ValidateRange(0, 86400)][int] $LockTimeout = 1800,
    [string] $Dispatch,
    [switch] $NoWait
)
function Resolve-RubylightPython {
    $pyLauncher = Get-Command py -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($pyLauncher) {
        try {
            $exe = @(& $pyLauncher.Source -3.13 -c "import sys; print(sys.executable)" 2>$null | ForEach-Object { $_.Trim() } | Where-Object { $_ }) | Select-Object -First 1
            if ($LASTEXITCODE -eq 0 -and $exe -and $exe -notmatch '[\\/]msys64[\\/]' -and (Test-Path -LiteralPath $exe)) {
                return $exe
            }
        } catch {
        }
        $global:LASTEXITCODE = 0
    }
    if ($env:LOCALAPPDATA) {
        $candidate = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python313\python.exe'
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    foreach ($cmd in (Get-Command python -CommandType Application -All -ErrorAction SilentlyContinue)) {
        if ($cmd.Source -and $cmd.Source -notmatch '[\\/]msys64[\\/]' -and $cmd.Source -notmatch '[\\/]Microsoft[\\/]WindowsApps[\\/]') {
            return $cmd.Source
        }
    }
    return 'python'
}

$python = Resolve-RubylightPython

if ($Command -eq 'ci') {
    if (-not $Branch) { $Branch = (git -C $PSScriptRoot rev-parse --abbrev-ref HEAD).Trim() }
    $py = @("$PSScriptRoot\ci.py", $Branch)
    if ($Dispatch) { $py += @('--dispatch', $Dispatch) }
    if ($NoWait) { $py += '--no-wait' }
    & $python @py
    exit $LASTEXITCODE
}
$py = @("$PSScriptRoot\dev.py", $Command, '--base', $Base)
if ($LogDir) { $py += @('--log-dir', $LogDir) }
if ($TargetDir) { $py += @('--target-dir', $TargetDir) }
if ($Paths) {
    $Paths = @($Paths | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
    if ($Paths) { $py += @('--paths') + $Paths }
}
if ($DryRun) { $py += '--dry-run' }
if ($Fast) { $py += '--fast' }
if ($KeepGoing) { $py += '--keep-going' }
if ($FreshDependencies) { $py += '--fresh-dependencies' }
if ($LockDirectory) { $py += @('--lock-directory', $LockDirectory) }
if ($Pool) { $py += @('--pool', $Pool) }
$py += @('--lock-timeout', $LockTimeout)
& $python @py
exit $LASTEXITCODE
