# Signs a Nightjar release's Windows files on the maintainer's PC with the maintainer's code
# signing certificate (Certum "Open Source Developer Ramazan Kara", whose key stays in Certum's
# cloud and is reached through SimplySign Desktop), and replaces them in the draft release:
#
#   .\scripts\sign-windows-release.ps1 -Tag v5.10.0
#
# Run it once the release workflow has finished and the draft holds both Windows zips and
# installers, before the release checks (defender-scan.ps1, the smoke and the stream). For x86_64
# and arm64 it signs nightjar.exe inside the zip, rebuilds the installer from that zip with
# packaging/windows/build-installer.ps1 -SignCommand (ISCC signs the installer and its uninstaller),
# then uploads the four files and SHA256SUMS-<version>.txt with their new hashes and corrects the
# release text's signing sentence. The release stays a draft. The rebuilt installer skips
# build-installer.ps1's -Smoke, which CI ran on the same files before signing.
#
# ISCC comes from -Iscc, $env:ISCC, a usual install folder, or else the Inno Setup version and
# SHA-256 that .github/workflows/release.yml pins, installed for the current user.
# -NoUpload signs into the work folder only (any release, also a published one), for a dry run.
# -From <folder> takes the two zips from that folder instead of the release and implies -NoUpload,
# leaving SHA256SUMS to whoever publishes: release.yml's sign-windows job uses it. -Smoke runs
# build-installer.ps1's install test on the signed x86_64 installer (CI runners only).
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$Repository = 'RamazanKara/nightjar',
    [string]$WorkDirectory = (Join-Path ([IO.Path]::GetTempPath()) "nightjar-sign-$Tag"),
    [string]$Subject = 'CN=Open Source Developer Ramazan Kara',
    [string]$TimestampUrl = 'http://time.certum.pl',
    [string]$Iscc = '',
    [string]$From,
    [switch]$Smoke,
    [switch]$NoUpload
)
$ErrorActionPreference = 'Stop'
$Version = $Tag -replace '^v', ''
$sumsName = "SHA256SUMS-$Version.txt"
$client = Split-Path $PSScriptRoot

function Get-Signtool {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $tool = Get-ChildItem -Path (Join-Path $kits '*\x64\signtool.exe') -File -ErrorAction SilentlyContinue |
        Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0' } } -Descending |
        Select-Object -First 1
    if (!$tool) { throw "signtool.exe was not found under $kits; the Windows SDK provides it" }
    $tool.FullName
}
function Get-SigningCertificate {
    $now = Get-Date
    $certificate = Get-ChildItem Cert:\CurrentUser\My | Where-Object {
        ($_.Subject -eq $Subject -or $_.Subject.StartsWith("$Subject,")) -and $_.HasPrivateKey -and
        $_.NotBefore -le $now -and $_.NotAfter -gt $now
    } | Sort-Object NotAfter -Descending | Select-Object -First 1
    if (!$certificate) { throw "No valid certificate '$Subject' with a key in CurrentUser\My: connect SimplySign Desktop (tray icon, Connect to SimplySign) and run again" }
    $certificate
}
function Assert-Signed([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Thumbprint -ne $certificate.Thumbprint -or !$signature.TimeStamperCertificate) {
        throw "$Path lacks a valid timestamped signature by $Subject ($($signature.Status))"
    }
}
function Get-InnoSetup {
    if ($Iscc) { return $Iscc }
    if ($env:ISCC) { return $env:ISCC }
    $found = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($found) { return $found }
    # The version and hash the release workflow pins.
    $workflow = Get-Content -Raw (Join-Path $client '.github\workflows\release.yml')
    $innoVersion = [regex]::Match($workflow, 'INNO_SETUP_VERSION: (\S+)').Groups[1].Value
    $innoSha = [regex]::Match($workflow, 'INNO_SETUP_SHA256: (\S+)').Groups[1].Value
    if (!$innoVersion -or !$innoSha) { throw 'release.yml pins no Inno Setup version' }
    $dir = Join-Path $env:LOCALAPPDATA "Programs\Inno Setup $innoVersion"
    if (Test-Path "$dir\ISCC.exe") { return "$dir\ISCC.exe" }
    $setup = Join-Path $WorkDirectory "innosetup-$innoVersion.exe"
    $innoTag = 'is-' + $innoVersion.Replace('.', '_')
    Invoke-WebRequest "https://github.com/jrsoftware/issrc/releases/download/$innoTag/innosetup-$innoVersion.exe" -OutFile $setup
    $sha = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($sha -ne $innoSha) { throw "innosetup-$innoVersion.exe is $sha, expected $innoSha" }
    $process = Start-Process $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/CURRENTUSER', '/NOICONS', "/DIR=`"$dir`"" -Wait -PassThru
    if ($process.ExitCode -ne 0 -or !(Test-Path "$dir\ISCC.exe")) { throw "Inno Setup did not install (exit $($process.ExitCode))" }
    "$dir\ISCC.exe"
}

$signtool = Get-Signtool
$certificate = Get-SigningCertificate
Write-Host "Signing Nightjar $Tag as $($certificate.Subject) ($($certificate.Thumbprint))"
if ($From) { $NoUpload = $true }
$arches = 'x86_64', 'arm64'
$files = foreach ($arch in $arches) { "nightjar-desktop-$Version-windows-$arch.zip"; "nightjar-desktop-$Version-windows-$arch-setup.exe" }
if (!$From) {
    $release = gh release view $Tag -R $Repository --json isDraft,assets,body | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "No release $Tag in $Repository" }
    if (!$release.isDraft -and !$NoUpload) { throw "$Tag is published already; this script only signs draft releases (-NoUpload for a dry run)" }
    $assets = @($release.assets.name)
    foreach ($name in @($files) + $sumsName) { if ($assets -notcontains $name) { throw "$Tag has no $name yet; wait for the release workflow" } }
}

if (Test-Path -LiteralPath $WorkDirectory) {
    # Only a folder an earlier run of this script made.
    if (!(Test-Path -LiteralPath (Join-Path $WorkDirectory 'download'))) { throw "$WorkDirectory exists and is not this script's work folder" }
    Remove-Item -LiteralPath $WorkDirectory -Recurse -Force
}
$download = Join-Path $WorkDirectory 'download'
$out = Join-Path $WorkDirectory 'out'
New-Item -ItemType Directory -Path $download, $out -Force | Out-Null
$isccPath = Get-InnoSetup
if ($From) {
    foreach ($arch in $arches) { Copy-Item -LiteralPath (Join-Path $From "nightjar-desktop-$Version-windows-$arch.zip") -Destination $download }
} else {
    gh release download $Tag -R $Repository -D $download --clobber @(foreach ($arch in $arches) { '-p'; "nightjar-desktop-$Version-windows-$arch.zip" }) -p $sumsName
    if ($LASTEXITCODE -ne 0) { throw "Downloading $Tag's Windows files failed" }
}

# ISCC runs the sign tool as one command line with $f for the file: put signtool on PATH so
# the command needs no quoted path.
$env:PATH = "$(Split-Path $signtool);$env:PATH"
$signCommand = "signtool.exe sign /sha1 $($certificate.Thumbprint) /fd sha256 /tr $TimestampUrl /td sha256 /d Nightjar /du https://github.com/$Repository `$f"
foreach ($arch in $arches) {
    $zipName = "nightjar-desktop-$Version-windows-$arch.zip"
    $stage = Join-Path $WorkDirectory "stage-$arch"
    Expand-Archive -LiteralPath (Join-Path $download $zipName) -DestinationPath $stage
    $exe = Join-Path $stage 'nightjar.exe'
    if (!(Test-Path -LiteralPath $exe)) { throw "$zipName has no nightjar.exe at its top" }
    if ((Get-AuthenticodeSignature -LiteralPath $exe).Status -ne 'NotSigned') { throw "nightjar.exe in $zipName is signed already" }
    & $signtool sign /sha1 $certificate.Thumbprint /fd sha256 /tr $TimestampUrl /td sha256 /d Nightjar /du "https://github.com/$Repository" $exe | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
    Assert-Signed $exe
    # As package-windows.ps1 packs it: the files at the zip's top.
    $zip = Join-Path $out $zipName
    Compress-Archive -Path "$stage\*" -DestinationPath $zip -CompressionLevel Optimal
    $smokeThis = $Smoke -and $arch -eq 'x86_64'
    & (Join-Path $client 'protocol\desktop\packaging\windows\build-installer.ps1') -Zip $zip -Out $out -Iscc $isccPath -SignCommand $signCommand -Smoke:$smokeThis | Out-Host
    $setup = Join-Path $out "nightjar-desktop-$Version-windows-$arch-setup.exe"
    Assert-Signed $setup
}

if ($From) {
    Get-ChildItem -LiteralPath $out -File | ForEach-Object { Write-Host "Signed: $($_.FullName)" }
    return
}
# SHA256SUMS keeps its other lines and order; the Windows files get their new hashes.
$sums = Join-Path $download $sumsName
$lines = foreach ($line in (Get-Content -LiteralPath $sums)) {
    $name = ($line -split '  ', 2)[1]
    if ($files -contains $name) { "$((Get-FileHash -LiteralPath (Join-Path $out $name) -Algorithm SHA256).Hash.ToLowerInvariant())  $name" } else { $line }
}
foreach ($name in $files) { if (!($lines -match "  $([regex]::Escape($name))$")) { throw "$sumsName does not list $name" } }
[IO.File]::WriteAllText((Join-Path $out $sumsName), (($lines -join "`n") + "`n"))

Get-ChildItem -LiteralPath $out -File | ForEach-Object { Write-Host "Signed: $($_.FullName)" }
if ($NoUpload) { return }
gh release upload $Tag -R $Repository --clobber @(Get-ChildItem -LiteralPath $out -File | ForEach-Object FullName)
if ($LASTEXITCODE -ne 0) { throw 'Uploading the signed files failed' }
# The release workflow's text says no desktop package is signed (release.yml).
$unsigned = 'the desktop packages (Windows installer and zip; '
if ($release.body.Contains($unsigned)) {
    $body = $release.body.Replace($unsigned, 'the Windows installer and nightjar.exe are signed by Ramazan Kara (Certum open-source code signing certificate); the other desktop packages (')
    $notes = Join-Path $WorkDirectory 'body.md'
    [IO.File]::WriteAllText($notes, $body)
    gh release edit $Tag -R $Repository --notes-file $notes
    if ($LASTEXITCODE -ne 0) { throw 'Correcting the release text failed' }
} else {
    Write-Warning "The release text has no '$unsigned'; check what it says about signing"
}
Write-Host "Uploaded; $Tag stays a draft"
