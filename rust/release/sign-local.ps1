# Signs a Rubylight release on the maintainer's PC with the maintainer's code
# signing certificate (Certum "Open Source Developer Ramazan Kara", whose key
# stays in Certum's cloud and is reached through SimplySign Desktop), then
# replaces the draft release's Windows files and publishes it.
#
#   .\rust\release\sign-local.ps1 -Tag v4.8.0
#
# With the repository variable LOCAL_SIGNING = true, rust-windows.yml publishes a
# release as a draft; this script finishes it. Run it after the whole release run
# has finished (nvidia-assets copies the installer). It signs in the order the
# SignPath steps of rust-windows.yml use, through the same sign-artifacts.ps1
# phases: Rubylight's own binaries, then the display driver's catalog made from
# the signed driver DLL, then the installer that package.ps1 builds again from
# the signed package. The NVIDIA component is never signed.
#
# -NoUpload signs into the work folder only (any release, also a published one,
# or -From a folder that holds the release's zip and setup), for a dry run.
# -KeepDraft uploads without publishing.
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$Repository = 'RamazanKara/Rubylight',
    [string]$WorkDirectory = (Join-Path ([IO.Path]::GetTempPath()) "rubylight-sign-$Tag"),
    [string]$From,
    [string]$Subject = 'CN=Open Source Developer Ramazan Kara',
    [string]$TimestampUrl = 'http://time.certum.pl',
    [switch]$NoUpload,
    [switch]$KeepDraft
)
$ErrorActionPreference = 'Stop'
$Version = $Tag -replace '^v', ''
$zipName = "rubylight-$Version-windows-x64.zip"
$setupName = "rubylight-setup-$Version.exe"
$migrateName = "rubylight-nvidia-setup-$Version.exe"
$signArtifacts = Join-Path $PSScriptRoot 'sign-artifacts.ps1'

function Get-KitTool([string]$Name, [string]$Architecture) {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $tool = Get-ChildItem -Path (Join-Path $kits "*\$Architecture\$Name") -File -ErrorAction SilentlyContinue |
        Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0' } } -Descending |
        Select-Object -First 1
    if (!$tool) { throw "$Name was not found under $kits; the Windows SDK provides it" }
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
function Invoke-Sign([string[]]$Files) {
    if (!$Files) { throw 'Nothing to sign' }
    & $script:signtool sign /sha1 $script:certificate.Thumbprint /fd sha256 /tr $TimestampUrl /td sha256 /d Rubylight /du "https://github.com/$Repository" @Files | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
    foreach ($file in $Files) {
        & $script:signtool verify /pa $file | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "The signature of $file does not verify" }
    }
}
function Invoke-Phase([string]$Phase, [string]$SigningDirectory) {
    $arguments = @{ Phase = $Phase; TargetDirectory = $target; Version = $Version }
    if ($SigningDirectory) { $arguments.SigningDirectory = $SigningDirectory }
    & $signArtifacts @arguments
}
# The installer is the setup stub, the zip and a 16-byte footer (rust/setup/src/payload.rs);
# a signed one may end in alignment padding and the certificate table.
function Get-Payload([string]$Path) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $end = $bytes.Length
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    $optional = $pe + 24
    $directories = switch ([BitConverter]::ToUInt16($bytes, $optional)) { 0x10b { 96 } 0x20b { 112 } default { throw "$Path is not a PE image" } }
    $security = $optional + $directories + 4 * 8
    $certificate = [BitConverter]::ToUInt32($bytes, $security)
    if ($certificate) { $end = [int]$certificate }
    for ($padding = 0; $padding -le 7; $padding++) {
        $footer = $end - $padding - 16
        if ($footer -lt 0) { break }
        if ([Text.Encoding]::ASCII.GetString($bytes, $footer, 8) -eq 'BPSETUP1') {
            $length = [BitConverter]::ToUInt64($bytes, $footer + 8)
            $start = $footer - [int64]$length
            if ($start -le 0) { throw "$Path has a footer with a bad length" }
            return [pscustomobject]@{ Bytes = $bytes; StubLength = [int]$start; ZipLength = [int]$length }
        }
    }
    throw "$Path carries no Rubylight package"
}
function Get-Sha256([byte[]]$Bytes) {
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}

$script:signtool = Get-KitTool 'signtool.exe' 'x64'
$script:certificate = Get-SigningCertificate
Write-Host "Signing $Tag as $($certificate.Subject) ($($certificate.Thumbprint))"

if (Test-Path -LiteralPath $WorkDirectory) {
    # Only a folder an earlier run of this script made.
    if (!(Test-Path -LiteralPath (Join-Path $WorkDirectory 'download'))) { throw "$WorkDirectory exists and is not this script's work folder" }
    Remove-Item -LiteralPath $WorkDirectory -Recurse -Force
}
$download = Join-Path $WorkDirectory 'download'
$target = Join-Path $WorkDirectory 'target'
$out = Join-Path $WorkDirectory 'out'
New-Item -ItemType Directory -Path $download, $target, $out -Force | Out-Null

$assets = @()
if ($From) {
    foreach ($name in $zipName, $setupName) { Copy-Item -LiteralPath (Join-Path $From $name) -Destination $download }
} else {
    $release = gh release view $Tag -R $Repository --json isDraft,assets | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "No release $Tag in $Repository" }
    if (!$release.isDraft -and !$NoUpload) { throw "$Tag is published already; this script only finishes draft releases (-NoUpload for a dry run)" }
    $assets = @($release.assets.name)
    $running = gh run list -R $Repository --workflow rust-windows.yml --branch $Tag --json status,url | ConvertFrom-Json |
        Where-Object { $_.status -ne 'completed' }
    if ($running -and !$NoUpload) { throw "The release run is still going ($($running[0].url)); run this when it has finished" }
    gh release download $Tag -R $Repository -p $zipName -p $setupName -D $download --clobber
    if ($LASTEXITCODE -ne 0) { throw "Downloading $Tag's Windows files failed" }
}

# The unsigned stub package.ps1 needs comes out of CI's installer, whose zip must be
# the release's zip byte for byte.
$payload = Get-Payload (Join-Path $download $setupName)
$zipBytes = [IO.File]::ReadAllBytes((Join-Path $download $zipName))
$embedded = [byte[]]::new($payload.ZipLength)
[Array]::Copy($payload.Bytes, $payload.StubLength, $embedded, 0, $payload.ZipLength)
if ((Get-Sha256 $embedded) -ne (Get-Sha256 $zipBytes)) { throw "$setupName does not carry $zipName" }
$stub = Join-Path $WorkDirectory 'butterpollo-setup.exe'
$stubBytes = [byte[]]::new($payload.StubLength)
[Array]::Copy($payload.Bytes, 0, $stubBytes, 0, $payload.StubLength)
[IO.File]::WriteAllBytes($stub, $stubBytes)
if ((Get-AuthenticodeSignature -LiteralPath $stub).Status -ne 'NotSigned') { throw "$setupName is signed already" }

Expand-Archive -LiteralPath (Join-Path $download $zipName) -DestinationPath $target
$distribution = Join-Path $target 'butterpollo-rust-release'
if (!(Test-Path -LiteralPath $distribution)) { throw "$zipName has no butterpollo-rust-release folder" }
Invoke-Phase 'CheckMetadata'

$binaries = Join-Path $WorkDirectory 'binaries'
Invoke-Phase 'StageBinaries' $binaries
Invoke-Sign @(Get-ChildItem -LiteralPath $binaries -Recurse -File | ForEach-Object FullName)
Invoke-Phase 'ApplyBinaries' $binaries

$driver = Join-Path $WorkDirectory 'driver'
Invoke-Phase 'StageDriver' $driver
Invoke-Sign @(Get-ChildItem -LiteralPath $driver -File -Filter '*.cat' | ForEach-Object FullName)
Invoke-Phase 'ApplyDriver' $driver

& (Join-Path $PSScriptRoot 'package.ps1') -Distribution $distribution -TargetDirectory $target -Stub $stub -Version $Version | Out-Host
$installer = Join-Path $WorkDirectory 'installer'
New-Item -ItemType Directory -Path $installer -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $target "butterpollo-setup-$Version.exe") -Destination $installer
Invoke-Sign @(Join-Path $installer "butterpollo-setup-$Version.exe")
Invoke-Phase 'ApplyInstaller' $installer

# The signed installer must still carry the signed zip.
$signedSetup = Join-Path $target "butterpollo-setup-$Version.exe"
$signedZip = Join-Path $target 'butterpollo-rust-release.zip'
$payload = Get-Payload $signedSetup
$embedded = [byte[]]::new($payload.ZipLength)
[Array]::Copy($payload.Bytes, $payload.StubLength, $embedded, 0, $payload.ZipLength)
if ((Get-Sha256 $embedded) -ne (Get-Sha256 ([IO.File]::ReadAllBytes($signedZip)))) { throw 'The signed installer does not carry the signed zip' }

Copy-Item -LiteralPath $signedZip -Destination (Join-Path $out $zipName)
Copy-Item -LiteralPath $signedSetup -Destination (Join-Path $out $setupName)
# SHA256SUMS lists the zip and the installer, as the publish job writes it (`sha256sum *`).
$sums = foreach ($name in $zipName, $setupName) {
    "$((Get-FileHash -LiteralPath (Join-Path $out $name) -Algorithm SHA256).Hash.ToLowerInvariant())  $name"
}
[IO.File]::WriteAllText((Join-Path $out 'SHA256SUMS'), (($sums -join "`n") + "`n"))
# nvidia-assets attached the unsigned installer under this name for hosts of the former NVIDIA build.
if ($assets -contains $migrateName) { Copy-Item -LiteralPath $signedSetup -Destination (Join-Path $out $migrateName) }

Get-ChildItem -LiteralPath $out | ForEach-Object { Write-Host "Signed: $($_.FullName)" }
if ($NoUpload) { return }
gh release upload $Tag -R $Repository --clobber @(Get-ChildItem -LiteralPath $out -File | ForEach-Object FullName)
if ($LASTEXITCODE -ne 0) { throw 'Uploading the signed files failed' }
if ($KeepDraft) { Write-Host "Uploaded; $Tag stays a draft"; return }
$flags = if ($Version.Contains('-')) { @('--prerelease') } else { @('--latest') }
gh release edit $Tag -R $Repository --draft=false @flags
if ($LASTEXITCODE -ne 0) { throw "Publishing $Tag failed" }
Write-Host "Published https://github.com/$Repository/releases/tag/$Tag"
