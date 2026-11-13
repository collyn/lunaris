# Starts a packaged Qt application with nothing but Windows on PATH and none
# of the build machine's Qt visible, and fails when it dies at load (a missing
# DLL: 0xC0000135) or cannot load its QML (a missing module leaves the process
# running without a window, so the error output is checked too).
#
# The real "windows" platform plugin is used: "offscreen" is not deployed, and
# asking for it makes Qt show a message box and wait, which looks like success.
#
# usage: smoke-qml.ps1 -Exe <path> [-ExeArgs <args>] [-Seconds 10]
# Works in Windows PowerShell 5.1 and PowerShell 7.
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [string[]]$ExeArgs = @(),
  [int]$Seconds = 10
)
$ErrorActionPreference = 'Stop'

$err = [System.IO.Path]::GetTempFileName()
$out = [System.IO.Path]::GetTempFileName()
# The child inherits this process' environment; put it back afterwards.
$environment = @{
  PATH = 'C:\Windows\System32;C:\Windows'
  QT_FORCE_STDERR_LOGGING = '1'
  LUNARIS_ALLOW_SECOND_INSTANCE = '1'
  QML2_IMPORT_PATH = $null; QML_IMPORT_PATH = $null; QT_PLUGIN_PATH = $null; QT_ROOT_DIR = $null; Qt6_DIR = $null
}
$saved = @{}
foreach ($name in $environment.Keys) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
  [Environment]::SetEnvironmentVariable($name, $environment[$name], 'Process')
}
try {
  $start = @{ FilePath = $Exe; PassThru = $true; RedirectStandardError = $err; RedirectStandardOutput = $out }
  if ($ExeArgs.Count -gt 0) { $start.ArgumentList = $ExeArgs }
  $p = Start-Process @start
} finally {
  foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
Start-Sleep -Seconds $Seconds
$exited = $p.HasExited
if (-not $exited) { Stop-Process -Id $p.Id -Force }
Start-Sleep -Milliseconds 300
$text = (Get-Content $err -Raw -ErrorAction SilentlyContinue) + (Get-Content $out -Raw -ErrorAction SilentlyContinue)
Remove-Item $err, $out -Force -ErrorAction SilentlyContinue

if ($exited) { Write-Host $text; throw "$Exe exited early with code $($p.ExitCode)" }
if ($text -match 'is not installed|failed to load component|is not a type|Cannot load library|Could not (load|find) the Qt platform plugin|no Qt platform plugin') {
  Write-Host $text
  throw "$Exe did not load its QML or a plugin: see the output above"
}
"$Exe ran for $Seconds s and loaded its QML"
