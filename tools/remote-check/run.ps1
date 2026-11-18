# Runs in the console user's session (started by start.ps1's scheduled task):
# checks out origin/<branch> in a detached worktree and runs tools\dev.ps1
# check -KeepGoing there. The log ends with LOGS=<run folder parent> and
# EXIT=<code>, which tools\remote-check.ps1 waits for. ASCII only.
param(
    [Parameter(Mandatory)][string]$Branch,
    [Parameter(Mandatory)][string]$Name,
    [string]$Repo = 'C:\src\Rubylight',
    [string]$Work = 'C:\src\rl-check'
)
$ErrorActionPreference = 'Continue'
$tree = Join-Path $Work $Name
$log = Join-Path $Work "$Name.log"
$logs = Join-Path $Work "logs\$Name"
$code = 1
try {
    "remote check of $Branch on $env:COMPUTERNAME as $env:USERNAME in session $((Get-Process -Id $PID).SessionId)" | Out-File $log -Encoding utf8
    # MSYS2 last: its python must not shadow the user's (the tools suites need PyYAML);
    # rl-env.ps1 and tools/dev.py put MSYS2 first for the Cargo stages themselves.
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH;C:\msys64\ucrt64\bin"
    git -C $Repo fetch -q origin 2>&1 | Out-File -Append $log -Encoding utf8
    if (Test-Path -LiteralPath $tree) { git -C $Repo worktree remove --force $tree 2>&1 | Out-File -Append $log -Encoding utf8 }
    git -C $Repo worktree prune 2>&1 | Out-File -Append $log -Encoding utf8
    git -C $Repo worktree add -q --detach $tree "origin/$Branch" 2>&1 | Out-File -Append $log -Encoding utf8
    if ($LASTEXITCODE) { throw "could not check out origin/$Branch" }
    "checked out $(git -C $tree rev-parse HEAD)" | Out-File -Append $log -Encoding utf8
    git -C $tree submodule update --init --recursive -q 2>&1 | Out-File -Append $log -Encoding utf8
    & pwsh -NoProfile -ExecutionPolicy Bypass -File (Join-Path $tree 'tools\dev.ps1') check -KeepGoing -LogDir $logs *>> $log
    $code = $LASTEXITCODE
} catch {
    "remote check failed: $_" | Out-File -Append $log -Encoding utf8
} finally {
    "LOGS=$logs" | Out-File -Append $log -Encoding utf8
    "EXIT=$code" | Out-File -Append $log -Encoding utf8
}
