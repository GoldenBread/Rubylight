# Parallel worktrees, one per topic: C:\src\rl-<topic> on branch <topic>, with
# its submodules. tools\dev.ps1 check builds in a warm slot of the shared target
# pool (tools/dev.py claim_target keeps a reused slot safe); `rm` still removes
# an old per-topic target dir C:\src\rl-target-<topic>.
#   tools\wt.ps1 new <topic> [-From origin/main]
#   tools\wt.ps1 rm <topic> [-Into origin/main] [-Force]
# `rm` refuses while the branch has commits that are not in -Into (patch
# equivalents count as merged, so rebased merges pass) or the worktree has
# uncommitted changes, unless -Force; it then deletes the branch and the
# target dir.
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet('new', 'rm')] [string] $Action,
    [Parameter(Mandatory, Position = 1)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]*$')] [string] $Topic,
    [string] $From = 'origin/main',
    [string] $Into = 'origin/main',
    [string] $Root = 'C:\src',
    [switch] $Force
)
$ErrorActionPreference = 'Continue'

function Fail([string] $Message) {
    [Console]::Error.WriteLine("wt.ps1: $Message")
    exit 1
}

function Invoke-Git {
    $out = & git @args
    if ($LASTEXITCODE) { Fail "git $($args -join ' ') failed with exit code $LASTEXITCODE" }
    $out
}

# The main checkout, also when this runs from a worktree.
$common = Invoke-Git -C $PSScriptRoot rev-parse --path-format=absolute --git-common-dir
$repo = Split-Path $common -Parent
$tree = Join-Path $Root "rl-$Topic"
$target = Join-Path $Root "rl-target-$Topic"

if ($Action -eq 'new') {
    if (Test-Path $tree) { Fail "$tree already exists" }
    & git -C $repo show-ref --verify --quiet "refs/heads/$Topic"
    if (-not $LASTEXITCODE) { Fail "branch $Topic already exists; remove it or pick another topic" }
    Invoke-Git -C $repo worktree add --quiet $tree -b $Topic $From | Out-Null
    Invoke-Git -C $tree submodule --quiet update --init --recursive | Out-Null
    "worktree $tree on $Topic from $From"
    exit 0
}

$listed = (Invoke-Git -C $repo worktree list --porcelain) -contains "worktree $($tree.Replace('\', '/'))"
& git -C $repo show-ref --verify --quiet "refs/heads/$Topic"
$branch = -not $LASTEXITCODE
if (-not $listed -and -not $branch -and -not (Test-Path $target)) { Fail "no worktree, branch or target dir for $Topic" }
if (-not $Force) {
    if ($branch) {
        $unmerged = @(Invoke-Git -C $repo cherry $Into $Topic | Where-Object { $_ -like '+*' })
        if ($unmerged.Count) {
            Fail "$Topic has $($unmerged.Count) commits not in $Into; merge them or pass -Force"
        }
    }
    if ($listed) {
        $dirty = @(Invoke-Git -C $tree status --porcelain --ignore-submodules=dirty)
        if ($dirty.Count) { Fail "$tree has $($dirty.Count) uncommitted changes; commit them or pass -Force" }
    }
}
if ($listed) {
    # Submodules make `worktree remove` insist on --force; the checks above guard it.
    Invoke-Git -C $repo worktree remove --force $tree | Out-Null
}
if ($branch) { Invoke-Git -C $repo branch -D $Topic | Out-Null }
if (Test-Path $target) { Remove-Item -Recurse -Force $target -ErrorAction Stop }
"removed $tree, branch $Topic and $target"
