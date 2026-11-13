# Standalone installer: installs rexplorer as a normal program (or with -Uninstall removes it). Windows PowerShell 5.1.
# It copies rexplorer.exe to %LOCALAPPDATA%\Programs\rexplorer, adds a Start menu shortcut and an entry in
# Settings > Apps, and changes NO folder, drive or Explorer key: Windows Explorer still opens folders and rexplorer is
# a program you start yourself. To make rexplorer the default file manager use master-install.ps1 (or
# rexplorer --register, or More > Make rexplorer the default file manager); restore-explorer.ps1 goes back.
# Runs as you, unelevated: rexplorer needs no administrator rights (everything is HKCU and %LOCALAPPDATA%), and an
# elevated window would only run user-writable files with rights they should not have.
# After an install it offers to import your current Windows Explorer settings (rexplorer --import-explorer, which
# only reads them): -ImportExplorer says yes, -NoImport says no, neither asks (default yes); with -Quiet it never asks
# (no import unless -ImportExplorer) and the exe shows no dialog.
# -Exe <path> defaults to rexplorer.exe next to this script (a release zip), else target\release\rexplorer.exe, built
# with cargo build --release when missing. In a release zip (LICENSE next to this script) a missing exe is an error.
# -InstallDir / -RegBase / -ProgramsDir / -StartupDir carry REXPLORER_INSTALL_DIR / REXPLORER_REG_BASE /
# REXPLORER_PROGRAMS_DIR / REXPLORER_STARTUP_DIR (tests). -WhatIf shows what would run and changes nothing.
# Exit codes: 0 done, 1 failed, 2 cancelled (nothing changed).
[CmdletBinding(SupportsShouldProcess = $true)]
param([string]$Exe, [switch]$Uninstall, [string]$InstallDir, [string]$RegBase, [string]$ProgramsDir, [string]$StartupDir,
  [switch]$ImportExplorer, [switch]$NoImport, [switch]$Quiet)
$ErrorActionPreference = 'Stop'

$flag = if ($Uninstall) { '--uninstall' } else { '--install' }
$code = 0
try {
  if (-not $InstallDir) { $InstallDir = $env:REXPLORER_INSTALL_DIR }
  if (-not $RegBase) { $RegBase = $env:REXPLORER_REG_BASE }

  $dir = if ($InstallDir) { $InstallDir } else { Join-Path $env:LOCALAPPDATA 'Programs\rexplorer' }
  $app = Join-Path $dir 'rexplorer.exe'
  # Next to the script in a release zip; the repo's release build otherwise.
  if (-not $Exe) { $Exe = Join-Path $PSScriptRoot 'rexplorer.exe' }
  # A release zip has LICENSE next to this script and no source tree above it: nothing to fall back to or build.
  $releaseZip = Test-Path -LiteralPath (Join-Path $PSScriptRoot 'LICENSE')
  if (-not $releaseZip -and -not (Test-Path -LiteralPath $Exe)) { $Exe = Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe' }
  # Uninstalling needs no build: the installed copy can do it.
  if ($Uninstall -and -not (Test-Path -LiteralPath $Exe) -and (Test-Path -LiteralPath $app)) { $Exe = $app }
  if ($releaseZip -and -not (Test-Path -LiteralPath $Exe)) {
    throw "Not found: $Exe. This is a release folder, so nothing is built: extract the whole release zip again (rexplorer.exe belongs next to this script), or pass -Exe <path to rexplorer.exe>."
  }
  if (-not (Test-Path -LiteralPath $Exe) -and -not $Uninstall -and $PSCmdlet.ShouldProcess('cargo build --release', 'Build rexplorer.exe')) {
    Write-Host 'rexplorer.exe not found, building it with cargo build --release...'
    Push-Location (Split-Path $PSScriptRoot -Parent)
    try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build --release failed.' } } finally { Pop-Location }
  }
  if (-not (Test-Path -LiteralPath $Exe)) {
    if ($WhatIfPreference) { Write-Host "What if: would run `"$Exe`" $flag"; exit 0 }
    throw "Not found: $Exe"
  }
  $Exe = (Resolve-Path -LiteralPath $Exe).Path

  if ($InstallDir) { $env:REXPLORER_INSTALL_DIR = $InstallDir }
  if ($RegBase) { $env:REXPLORER_REG_BASE = $RegBase }
  if ($ProgramsDir) { $env:REXPLORER_PROGRAMS_DIR = $ProgramsDir }
  if ($StartupDir) { $env:REXPLORER_STARTUP_DIR = $StartupDir }
  $exeArgs = @($flag) + @(if ($Quiet) { '--quiet' })
  if (-not $PSCmdlet.ShouldProcess("$dir (registry base HKCU\$(if ($RegBase) { $RegBase } else { 'Software' }))", "rexplorer $($exeArgs -join ' ')")) { exit 0 }
  # The exe asks first (install only, unless --quiet), shows its own result box (unless --quiet) and exits 0, 2 when
  # cancelled, or 1 on failure.
  $p = Start-Process -FilePath $Exe -ArgumentList $exeArgs -Wait -PassThru
  if ($p.ExitCode -eq 2) {
    Write-Host 'Install cancelled; nothing was changed.' -ForegroundColor Yellow
    exit 2
  }
  if ($p.ExitCode) { throw "rexplorer $flag failed (exit code $($p.ExitCode)); $(if ($Quiet) { 'a failed install was rolled back' } else { 'see the message box it showed' })." }

  if ($Uninstall) {
    Write-Host "`nrexplorer is uninstalled; folders, Win+E and the taskbar open Windows Explorer."
    Write-Host "Your settings in $env:APPDATA\rexplorer stay, including anything imported from Explorer (an import is not undone)."
    Write-Host "To install again: powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`""
  } else {
    $import = $ImportExplorer
    if (-not $ImportExplorer -and -not $NoImport -and -not $Quiet) {
      $answer = Read-Host 'Import your current Windows Explorer settings (hidden files, extensions, start folder...) into rexplorer? [Y/n]'
      $import = $answer -notmatch '^\s*n'
    }
    if ($import -and $PSCmdlet.ShouldProcess($app, 'rexplorer --import-explorer')) {
      $q = Start-Process -FilePath $app -ArgumentList '--import-explorer' -Wait -PassThru
      if ($q.ExitCode) { Write-Host "The import failed (exit code $($q.ExitCode)); nothing was changed. Retry: & `"$app`" --import-explorer" -ForegroundColor Yellow }
    }
    $master = Join-Path $PSScriptRoot 'master-install.ps1'
    Write-Host "`nrexplorer is installed in $dir as a normal program. Windows Explorer still opens folders."
    Write-Host '  Start it:        the Start menu shortcut "rexplorer", or run it from the folder above.'
    Write-Host "  Make it default: powershell -ExecutionPolicy Bypass -File `"$master`"    (or `"$app`" --register, or More > Make rexplorer the default file manager)"
    Write-Host "  Uninstall:       Settings > Apps, powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Uninstall, or `"$app`" --uninstall"
    Write-Host "  Import later:    & `"$app`" --import-explorer [--overwrite]    (keeps a settings.ini.bak-<date> backup)"
  }
} catch {
  Write-Host $_.Exception.Message -ForegroundColor Red
  $code = 1
}
exit $code
