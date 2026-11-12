# Master installer: installs rexplorer (install.ps1, the standalone install) and then makes it the default file manager
# in place of Windows Explorer (rexplorer --register: folders, drives, Win+E, the taskbar's File Explorer, programs that
# start explorer.exe by name, and the desktop's context menu). Windows PowerShell 5.1, run as you, unelevated.
# Before the take-over it saves every HKCU key it is about to change to a dated .reg file under
# %LOCALAPPDATA%\rexplorer\backup and prints how to go back:
#   - rexplorer --unregister, or More > Give folders back to Windows Explorer (rexplorer keeps working as a program);
#   - restore-explorer.ps1 (next to this script), which needs no working rexplorer.exe;
#   - reg import <backup>.reg, which puts those keys back exactly as they were.
# -Exe, -ImportExplorer, -NoImport, -Quiet, -InstallDir, -RegBase, -ProgramsDir and -StartupDir are passed to
# install.ps1 (the last four are test overrides). -WhatIf shows what would run and changes nothing.
# Exit codes: 0 done, 1 failed (a failed take-over is rolled back; the program stays installed), 2 cancelled.
[CmdletBinding(SupportsShouldProcess = $true)]
param([string]$Exe, [string]$InstallDir, [string]$RegBase, [string]$ProgramsDir, [string]$StartupDir,
  [switch]$ImportExplorer, [switch]$NoImport, [switch]$Quiet)
$ErrorActionPreference = 'Stop'

# The keys `rexplorer --register` writes, relative to HKCU\<RegBase>: the take-over half of $KnownKeys in
# restore-explorer.ps1 (known_keys() in crates/core/src/install.rs, without the Apps and Features entry).
$TakeoverKeys = @(
  'Classes\Folder\shell\open\command',
  'Classes\Directory\shell\open\command',
  'Classes\Drive\shell\open\command',
  'Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open\command',
  'Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command',
  'Microsoft\Windows\CurrentVersion\App Paths\explorer.exe'
)

$code = 0
try {
  if (-not $InstallDir) { $InstallDir = $env:REXPLORER_INSTALL_DIR }
  if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\rexplorer' }
  if (-not $RegBase) { $RegBase = $env:REXPLORER_REG_BASE }
  if (-not $RegBase) { $RegBase = 'Software' }
  $RegBase = $RegBase.TrimEnd('\')
  if ($RegBase -notmatch '^Software(\\[^\\]+)*$') { throw "RegBase must be Software or a key under it: $RegBase" }
  $restore = Join-Path $PSScriptRoot 'restore-explorer.ps1'
  $app = Join-Path $InstallDir 'rexplorer.exe'

  # 1. The program (install.ps1 shares this window and returns its exit code).
  $pass = @{ InstallDir = $InstallDir; RegBase = $RegBase }
  foreach ($n in 'Exe', 'ProgramsDir', 'StartupDir') { if ($PSBoundParameters[$n]) { $pass[$n] = $PSBoundParameters[$n] } }
  foreach ($n in 'ImportExplorer', 'NoImport', 'Quiet') { if ($PSBoundParameters[$n]) { $pass[$n] = $true } }
  if ($WhatIfPreference) { $pass['WhatIf'] = $true }
  & (Join-Path $PSScriptRoot 'install.ps1') @pass
  if ($LASTEXITCODE) { Write-Host "The install did not finish (exit code $LASTEXITCODE); Windows Explorer was not replaced."; exit $LASTEXITCODE }

  # 2. The backup of every key the take-over is about to change: a key that exists is exported as it is, a missing
  # one becomes a [-key] line, so importing the file deletes what --register created.
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $backup = Join-Path $env:LOCALAPPDATA "rexplorer\backup\explorer-keys-$stamp.reg"
  if ($PSCmdlet.ShouldProcess($backup, "Back up $($TakeoverKeys.Count) keys under HKCU\$RegBase")) {
    $null = New-Item -ItemType Directory -Force -Path (Split-Path $backup)
    $out = @('Windows Registry Editor Version 5.00', '',
      "; rexplorer master-install backup, $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss'): the keys rexplorer --register changes.",
      "; Put them back with: reg import `"$backup`"   (or run restore-explorer.ps1, which also removes the desktop helper).", '')
    $tmp = Join-Path ([IO.Path]::GetTempPath()) "rexplorer-backup-$PID.reg"
    try {
      foreach ($rel in $TakeoverKeys) {
        $key = "HKEY_CURRENT_USER\$RegBase\$rel"
        if (Test-Path -LiteralPath "Registry::$key") {
          $null = & reg.exe export $key $tmp /y 2>&1
          if ($LASTEXITCODE) { throw "reg export $key failed (exit code $LASTEXITCODE); nothing was changed." }
          # reg export writes UTF-16 with its own header line: keep the key sections.
          $out += @(Get-Content -LiteralPath $tmp -Encoding Unicode | Select-Object -Skip 1) + ''
        } else {
          $out += @("[-$key]", '')
        }
      }
    } finally { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
    Set-Content -LiteralPath $backup -Value $out -Encoding Unicode
    Write-Host "Backup of the Explorer keys: $backup"
  }

  # 3. The take-over, by the installed copy (journaled in <install dir>\takeover.txt).
  if ($PSCmdlet.ShouldProcess($app, 'rexplorer --register (make it the default file manager)')) {
    # The exe reads these from its environment; set here too, not left to install.ps1, so --register can never
    # reach a different base than the install did.
    $env:REXPLORER_INSTALL_DIR = $InstallDir; $env:REXPLORER_REG_BASE = $RegBase
    if ($ProgramsDir) { $env:REXPLORER_PROGRAMS_DIR = $ProgramsDir }
    if ($StartupDir) { $env:REXPLORER_STARTUP_DIR = $StartupDir }
    $regArgs = @('--register') + @(if ($Quiet) { '--quiet' })
    $p = Start-Process -FilePath $app -ArgumentList $regArgs -Wait -PassThru
    if ($p.ExitCode -eq 2) { Write-Host 'Cancelled: rexplorer is installed, but Windows Explorer still opens folders.' -ForegroundColor Yellow; exit 2 }
    if ($p.ExitCode) { throw "rexplorer --register failed (exit code $($p.ExitCode)); what it had changed was rolled back. rexplorer stays installed; Windows Explorer still opens folders." }
  }

  if ($WhatIfPreference) { exit 0 }
  Write-Host "`nrexplorer is installed in $InstallDir and is now the default file manager (folders, drives, Win+E)."
  Write-Host 'To go back to Windows Explorer:'
  Write-Host "  - in rexplorer: More > Give folders back to Windows Explorer, or `"$app`" --unregister"
  Write-Host "  - if rexplorer will not start: powershell -ExecutionPolicy Bypass -File `"$restore`""
  Write-Host "  - or put the saved keys back: reg import `"$backup`""
  Write-Host '  Then sign out and back in, so the running Explorer shell drops the old setting.'
} catch {
  Write-Host $_.Exception.Message -ForegroundColor Red
  $code = 1
}
exit $code
