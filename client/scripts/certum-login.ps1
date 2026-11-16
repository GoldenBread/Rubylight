# Logs a GitHub-hosted Windows runner in to Certum SimplySign, so that signtool can sign with
# the maintainer's code signing certificate ("Open Source Developer Ramazan Kara"), whose key
# stays in Certum's cloud. Used by Rubylight's rust-windows.yml and signing-check.yml and by
# Nightjar's release.yml.
#
#   $env:CERTUM_USERNAME = <the Certum account's e-mail>
#   $env:CERTUM_TOTP_SECRET = <the secret= value of the otpauth URI Certum issued>
#   .\certum-login.ps1                # installs SimplySign Desktop, logs in, waits for the certificate
#   .\certum-login.ps1 -SelfTest      # checks the TOTP code against RFC 6238's SHA-256 vector only
#
# SimplySign Desktop has no command-line login, so this starts it and types the e-mail address
# and the current six-digit code (TOTP, HMAC-SHA-256, 30 s) into its login window, as Certum's
# own app would be used by hand. It makes at most two attempts, each with a code that has at
# least 15 seconds left, so a slow runner cannot lock the account with wrong codes. Neither the
# secret nor a code is ever printed. The certificate's SHA-1 thumbprint goes to the step output
# `thumbprint` and to $env:GITHUB_ENV as CERTUM_THUMBPRINT.
param(
    [string]$MsiVersion = '9.4.4.92',
    [string]$MsiSha256 = '8ec420fc27798b86078b7bd02fe7152097e1b3005bab51820eaca8e57df84da3',
    [string]$Subject = 'CN=Open Source Developer Ramazan Kara',
    [int]$TimeoutSeconds = 120,
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'

function ConvertFrom-Base32([string]$Text) {
    $alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'
    $clean = ($Text -replace '[\s=-]', '').ToUpperInvariant()
    $bytes = [Collections.Generic.List[byte]]::new()
    $buffer = 0; $bits = 0
    foreach ($char in $clean.ToCharArray()) {
        $index = $alphabet.IndexOf($char)
        if ($index -lt 0) { throw 'The TOTP secret is not Base32' }
        $buffer = (($buffer -shl 5) -bor $index) -band 0xFFFF; $bits += 5
        if ($bits -ge 8) { $bits -= 8; $bytes.Add([byte](($buffer -shr $bits) -band 0xFF)) }
    }
    $bytes.ToArray()
}
function Get-Totp([byte[]]$Key, [long]$UnixTime, [int]$Digits = 6, [int]$Period = 30) {
    $counter = [BitConverter]::GetBytes([long][math]::Floor($UnixTime / $Period))
    if ([BitConverter]::IsLittleEndian) { [Array]::Reverse($counter) }
    $hmac = [Security.Cryptography.HMACSHA256]::new($Key)
    try { $hash = $hmac.ComputeHash($counter) } finally { $hmac.Dispose() }
    $offset = $hash[$hash.Length - 1] -band 0x0F
    $binary = (([int]$hash[$offset] -band 0x7F) -shl 24) -bor ([int]$hash[$offset + 1] -shl 16) -bor ([int]$hash[$offset + 2] -shl 8) -bor [int]$hash[$offset + 3]
    ($binary % [math]::Pow(10, $Digits)).ToString().PadLeft($Digits, '0')
}
# RFC 6238 appendix B, SHA-256: the 32-byte ASCII key "1234567890..." at T = 59 and 1111111109.
$vectorKey = [Text.Encoding]::ASCII.GetBytes('12345678901234567890123456789012')
foreach ($vector in @(@(59, '46119246'), @(1111111109, '68084774'))) {
    if ((Get-Totp $vectorKey $vector[0] 8) -ne $vector[1]) { throw "TOTP self-test failed at T=$($vector[0])" }
}
if ((ConvertFrom-Base32 'GEZDGNBVGY3TQOJQ' | ForEach-Object { [char]$_ }) -join '' -ne '1234567890') { throw 'Base32 self-test failed' }
Write-Host 'TOTP self-test passed'
if ($SelfTest) { return }

if (!$env:CERTUM_USERNAME -or !$env:CERTUM_TOTP_SECRET) { throw 'CERTUM_USERNAME and CERTUM_TOTP_SECRET must be set' }
$key = ConvertFrom-Base32 $env:CERTUM_TOTP_SECRET
if ($key.Length -lt 10) { throw 'The TOTP secret is too short' }

# SimplySign Desktop, pinned by version and SHA-256, signed by its publisher.
$app = 'C:\Program Files\Certum\SimplySign Desktop\SimplySignDesktop.exe'
if (!(Test-Path $app)) {
    $msi = Join-Path ([IO.Path]::GetTempPath()) "SimplySignDesktop-$MsiVersion.msi"
    Invoke-WebRequest "https://files.certum.eu/software/SimplySignDesktop/Windows/$MsiVersion/SimplySignDesktop-$MsiVersion-64-bit-en.msi" -OutFile $msi -MaximumRetryCount 3 -RetryIntervalSec 10
    $sha = (Get-FileHash $msi -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($sha -ne $MsiSha256) { throw "SimplySignDesktop-$MsiVersion.msi is $sha, expected $MsiSha256" }
    $signature = Get-AuthenticodeSignature $msi
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notlike 'CN=Asseco Data Systems S.A.,*') { throw "The SimplySign installer's signature is $($signature.Status)" }
    $process = Start-Process msiexec.exe -ArgumentList '/i', "`"$msi`"", '/qn', '/norestart' -Wait -PassThru
    if ($process.ExitCode -notin 0, 3010 -or !(Test-Path $app)) { throw "SimplySign Desktop did not install (msiexec exit $($process.ExitCode))" }
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CertumWindow {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
'@
$shell = New-Object -ComObject WScript.Shell
function Get-LoginWindow {
    Get-Process SimplySignDesktop -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle } | Select-Object -First 1
}
function Show-LoginWindow {
    $window = Get-LoginWindow
    if (!$window) { return $false }
    [void][CertumWindow]::ShowWindow($window.MainWindowHandle, 9)
    [void][CertumWindow]::SetForegroundWindow($window.MainWindowHandle)
    [void]$shell.AppActivate($window.Id)
    Start-Sleep -Milliseconds 500
    $true
}
function Send-Text([string]$Text) { $shell.SendKeys(($Text -replace '([+^%~(){}\[\]])', '{$1}')) }
function Get-Certificate {
    $now = Get-Date
    Get-ChildItem Cert:\CurrentUser\My | Where-Object {
        ($_.Subject -eq $Subject -or $_.Subject.StartsWith("$Subject,")) -and $_.HasPrivateKey -and $_.NotBefore -le $now -and $_.NotAfter -gt $now
    } | Sort-Object NotAfter -Descending | Select-Object -First 1
}

# The first start sets SimplySign up in the tray; starting it again opens its login window.
(New-Object -ComObject Shell.Application).MinimizeAll()
for ($start = 1; $start -le 6 -and !(Get-LoginWindow); $start++) {
    Start-Process $app
    Start-Sleep -Seconds 4
}
if (!(Get-LoginWindow)) { throw 'The SimplySign Desktop login window did not open' }

$certificate = $null
for ($attempt = 1; $attempt -le 2 -and !$certificate; $attempt++) {
    if (!(Get-LoginWindow)) {
        Start-Process $app
        Start-Sleep -Seconds 4
    }
    if (!(Show-LoginWindow)) { throw 'The SimplySign Desktop login window is gone' }
    # A code with at least 15 seconds left, made just before it is typed.
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    if (30 - $now % 30 -lt 15) { Start-Sleep -Seconds (30 - $now % 30 + 1) }
    [void](Show-LoginWindow)
    $shell.SendKeys('^a'); Send-Text $env:CERTUM_USERNAME
    $shell.SendKeys('{TAB}'); $shell.SendKeys('^a')
    Send-Text (Get-Totp $key ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()))
    $shell.SendKeys('{ENTER}')
    Write-Host "Login attempt $attempt sent"
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds / 2)
    while ((Get-Date) -lt $deadline -and !($certificate = Get-Certificate)) { Start-Sleep -Seconds 3 }
}
[Array]::Clear($key, 0, $key.Length)
if (!$certificate) { throw "No certificate '$Subject' with a key appeared after logging in to SimplySign" }
Write-Host "SimplySign connected: $($certificate.Subject.Split(',')[0]) ($($certificate.Thumbprint)), valid until $($certificate.NotAfter.ToString('yyyy-MM-dd'))"
if ($env:GITHUB_OUTPUT) { "thumbprint=$($certificate.Thumbprint)" >> $env:GITHUB_OUTPUT }
if ($env:GITHUB_ENV) { "CERTUM_THUMBPRINT=$($certificate.Thumbprint)" >> $env:GITHUB_ENV }
