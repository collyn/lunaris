# Lunaris agent installer for Windows (x64), made by the Lunaris server.
#
# In an elevated PowerShell ("Run as administrator"):
#   $env:LUNARIS_INSTALL_TOKEN='TOKEN'; [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; & ([scriptblock]::Create((irm https://SERVER/install/agent.ps1))) [-Name NAME]
# or saved as a file:
#   $env:LUNARIS_INSTALL_TOKEN='TOKEN'; powershell -ExecutionPolicy Bypass -File .\agent.ps1 [-Name NAME]
# (-Token TOKEN works too, but then the token shows in the process list.)
#
# Installs the Lunaris agent of this server's version from GitHub Releases
# (silent installer: the command-line agent, which has no window, and the
# approval helper, a tray that asks the person at the computer before anyone
# connects without the password), joins it to the server with the install
# token (used once, then the agent signs in with its own key) and runs it as
# the "Lunaris Agent" Windows service (LunarisAgentService), which starts at
# boot.
#
# The installer runs only when its SHA-256 is the expected one: written in by
# the server after it checked the signature of the release's SHA256SUMS
# (Ed25519, the Lunaris release key), or else its line in SHA256SUMS fetched
# here from GitHub over HTTPS (Windows PowerShell has no Ed25519 to check the
# signature itself). Anything else stops the installation.
param(
    [string]$Token = $env:LUNARIS_INSTALL_TOKEN,
    [string]$Name = ''
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# A certificate check turned off to fetch this script from a server with a
# self-signed certificate is turned on again for the downloads below.
[System.Net.ServicePointManager]::ServerCertificateValidationCallback = $null
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$ServerUrl = '@@SERVER_URL@@'
$Version = '@@VERSION@@'
$CertSha256 = '@@CERT_SHA256@@'
# SHA-256 of the installer, from the release's signed SHA256SUMS as the
# server checked it (empty: from SHA256SUMS below).
$PinnedInstaller = '@@SHA256_INSTALLER@@'
$ReleaseBase = "https://github.com/collyn/lunaris/releases/download/v$Version"
$AppName = 'Lunaris Agent'

function Fail([string]$Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    throw $Message
}

# Quotes one argument for a Windows command line.
function Quote-Arg([string]$Value) {
    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

# Runs the agent (a console program: Start-Process waits for it and catches
# its output) and returns the exit code.
function Invoke-Agent([string]$Exe, [string[]]$Arguments, [string]$Dir) {
    $out = Join-Path $Dir 'agent-out.txt'
    $err = Join-Path $Dir 'agent-err.txt'
    $p = Start-Process -FilePath $Exe -ArgumentList ($Arguments -join ' ') -Wait -PassThru -NoNewWindow `
        -RedirectStandardOutput $out -RedirectStandardError $err
    foreach ($f in @($out, $err)) {
        if (Test-Path $f) {
            Get-Content $f | ForEach-Object { Write-Host "   $_" }
            Remove-Item $f -Force -ErrorAction SilentlyContinue
        }
    }
    return $p.ExitCode
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Fail 'run this in PowerShell started with "Run as administrator" (the agent runs as a service to control every window)'
}
if (-not [Environment]::Is64BitOperatingSystem) { Fail 'only 64-bit Windows is supported' }
if (-not $Token) { Fail 'missing install token: set $env:LUNARIS_INSTALL_TOKEN (copy the command from the server''s web page)' }
if ($Token -notmatch '^lit_[A-Za-z0-9_-]{16,128}$') { Fail 'this is not an install token (they start with lit_)' }
if ($Name.Length -gt 128) { Fail '-Name is longer than 128 characters' }

$tmp = Join-Path $env:TEMP ('lunaris-agent-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $installer = Join-Path $tmp 'lunaris-agent-installer-windows.exe'
    Write-Host "==> Downloading $ReleaseBase/lunaris-agent-installer-windows.exe"
    try {
        Invoke-WebRequest -Uri "$ReleaseBase/lunaris-agent-installer-windows.exe" -OutFile $installer -UseBasicParsing
    } catch {
        Fail "cannot download the installer (is version $Version released?): $($_.Exception.Message)"
    }

    $installerName = 'lunaris-agent-installer-windows.exe'
    $expected = $PinnedInstaller
    if ($expected) {
        Write-Host '==> Checking the installer against the release checksum the server verified'
    } else {
        Write-Host "==> Checking the installer against $ReleaseBase/SHA256SUMS"
        $sumsFile = Join-Path $tmp 'SHA256SUMS'
        try {
            Invoke-WebRequest -Uri "$ReleaseBase/SHA256SUMS" -OutFile $sumsFile -UseBasicParsing
        } catch {
            Fail "cannot download SHA256SUMS, so the installer can't be checked: nothing is installed ($($_.Exception.Message))"
        }
        $lines = @(Get-Content -LiteralPath $sumsFile)
        if ($lines.Count -eq 0 -or $lines[0] -ne "# lunaris $Version") {
            Fail "SHA256SUMS is not the list of release ${Version}: nothing is installed"
        }
        foreach ($line in $lines) {
            if ($line -match '^([0-9a-fA-F]{64}) [ *](.+)$' -and $Matches[2] -eq $installerName) { $expected = $Matches[1]; break }
        }
        if (-not $expected) { Fail "$installerName is not in SHA256SUMS of release ${Version}: nothing is installed" }
        Write-Host '   (the signature of SHA256SUMS is not checked here: Windows PowerShell cannot check Ed25519; the list comes from GitHub over HTTPS)'
    }
    $actual = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    if ($actual -ne $expected) {
        Fail "the installer does not match its checksum (expected $expected, got $actual): nothing is installed"
    }
    Write-Host '   checksum OK'

    Write-Host "==> Installing $AppName $Version"
    $p = Start-Process -FilePath $installer -ArgumentList '/S' -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "the installer failed (exit code $($p.ExitCode))" }

    # Where it went: the installer (32-bit) records it under WOW6432Node.
    $dir = $null
    foreach ($key in @("HKLM:\SOFTWARE\WOW6432Node\$AppName", "HKLM:\SOFTWARE\$AppName")) {
        $item = Get-ItemProperty -Path $key -Name InstallDir -ErrorAction SilentlyContinue
        if ($item -and $item.InstallDir) { $dir = $item.InstallDir; break }
    }
    if (-not $dir) { $dir = Join-Path $env:ProgramFiles $AppName }
    $exe = Join-Path $dir 'lunaris-agent.exe'
    if (-not (Test-Path $exe)) { Fail "$exe is missing after the install" }

    # The config the service uses (agent/src/win_service.rs): --system.
    $enroll = @('--system', '--enroll', '--server', (Quote-Arg $ServerUrl))
    if ($Name) { $enroll += @('--name', (Quote-Arg $Name)) }
    if ($CertSha256) { $enroll += @('--server-cert-sha256', $CertSha256) }

    Write-Host "==> Joining this computer to $ServerUrl"
    # The token goes in the environment, not on the agent's command line.
    $env:LUNARIS_INSTALL_TOKEN = $Token
    try {
        $code = Invoke-Agent $exe $enroll $tmp
    } finally {
        Remove-Item Env:\LUNARIS_INSTALL_TOKEN -ErrorAction SilentlyContinue
    }
    if ($code -ne 0) { Fail "the agent could not be set up (exit code $code)" }

    # The installer installed and started the service; this restarts it with
    # the new config (and installs it again if it was missing).
    Write-Host '==> Starting the Lunaris Agent service'
    $code = Invoke-Agent $exe @('--install-service') $tmp
    if ($code -ne 0) { Fail "the service could not be installed (exit code $code)" }
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host "$AppName $Version is installed and joined to $ServerUrl."
Write-Host '  * It runs as the "Lunaris Agent" service and starts at boot; the computer'
Write-Host '    appears on the server within a few seconds.'
Write-Host "  * Settings and log: $env:ProgramData\Lunaris (administrators only);"
Write-Host "    state: & '$exe' --system status"
