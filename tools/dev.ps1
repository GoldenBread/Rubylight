# Developer loop helpers. `check` runs the smallest set of checks for the
# files changed against the merge base with -Base (default origin/main) plus
# uncommitted ones, prints one PASS/FAIL table and keeps the logs in files
# (C:\src\butterpollo-release\unified\dev-check\<worktree>\<run>\). The mapping lives
# in tools/dev.py. `ci` watches GitHub CI for a pushed wip/** branch (tools/ci.py);
# it pushes nothing.
#   tools\dev.ps1 check -DryRun           # the plan only
#   tools\dev.ps1 check -Base HEAD        # only uncommitted changes
#   tools\dev.ps1 ci wip/my-topic         # watch the runs its last push started
#   tools\dev.ps1 ci wip/my-topic -Dispatch host,android   # start them on the branch
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet('check', 'ci')] [string] $Command,
    [Parameter(Position = 1)] [string] $Branch,
    [string] $Base = 'origin/main',
    [string] $LogDir,
    [string] $TargetDir,
    [switch] $DryRun,
    [switch] $KeepGoing,
    [switch] $FreshDependencies,
    [string] $LockDirectory,
    [string] $Pool,
    [ValidateRange(0, 86400)][int] $LockTimeout = 1800,
    [string] $Dispatch,
    [switch] $NoWait
)
if ($Command -eq 'ci') {
    if (-not $Branch) { $Branch = (git -C $PSScriptRoot rev-parse --abbrev-ref HEAD).Trim() }
    $py = @("$PSScriptRoot\ci.py", $Branch)
    if ($Dispatch) { $py += @('--dispatch', $Dispatch) }
    if ($NoWait) { $py += '--no-wait' }
    & python @py
    exit $LASTEXITCODE
}
$py = @("$PSScriptRoot\dev.py", $Command, '--base', $Base)
if ($LogDir) { $py += @('--log-dir', $LogDir) }
if ($TargetDir) { $py += @('--target-dir', $TargetDir) }
if ($DryRun) { $py += '--dry-run' }
if ($KeepGoing) { $py += '--keep-going' }
if ($FreshDependencies) { $py += '--fresh-dependencies' }
if ($LockDirectory) { $py += @('--lock-directory', $LockDirectory) }
if ($Pool) { $py += @('--pool', $Pool) }
$py += @('--lock-timeout', $LockTimeout)
& python @py
exit $LASTEXITCODE
