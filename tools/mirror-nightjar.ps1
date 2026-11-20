# Publishes client/ to the nightjar repo, fast-forward only. The work is done
# by mirror_nightjar.py, an incremental `git subtree split` that starts from
# nightjar main (seconds instead of the 10+ minutes the full split takes).
# CI runs it after Nightjar's main checks pass (.github/workflows/nightjar-mirror.yml).
#   tools\mirror-nightjar.ps1 -DryRun    # split, check, `git push --dry-run`
#   tools\mirror-nightjar.ps1            # the real push
param(
    [string] $Remote = 'https://github.com/RamazanKara/nightjar.git',
    [string] $Branch = 'main',
    # The monorepo commit to publish. Default: origin/main after a fetch of
    # origin (never the local HEAD, which may be stale). The default does not check
    # CI: pass the commit whose main CI passed.
    [string] $Ref = '',
    [switch] $DryRun
)
$py = @("$PSScriptRoot\mirror_nightjar.py", '--repo', "$PSScriptRoot\..", '--remote', $Remote, '--branch', $Branch)
if ($Ref) { $py += @('--ref', $Ref) }
if ($DryRun) { $py += '--dry-run' }
& python @py
exit $LASTEXITCODE
