# Installs (or with -Uninstall removes) rexplorer as the default file manager. Windows PowerShell 5.1.
# Runs as you, unelevated: rexplorer needs no administrator rights (everything is HKCU and %LOCALAPPDATA%), and an
# elevated window would only run user-writable files with rights they should not have.
# After an install it offers to import your current Windows Explorer settings (rexplorer --import-explorer, which
# only reads them): -ImportExplorer says yes, -NoImport says no, neither asks (default yes).
# -Exe <path> defaults to target\release\rexplorer.exe, built with cargo build --release when missing.
# -InstallDir / -RegBase carry REXPLORER_INSTALL_DIR / REXPLORER_REG_BASE (tests).
param([string]$Exe, [switch]$Uninstall, [string]$InstallDir, [string]$RegBase, [switch]$ImportExplorer, [switch]$NoImport)
$ErrorActionPreference = 'Stop'

$flag = if ($Uninstall) { '--uninstall' } else { '--install' }
$code = 0
try {
    if (-not $InstallDir) { $InstallDir = $env:REXPLORER_INSTALL_DIR }
    if (-not $RegBase) { $RegBase = $env:REXPLORER_REG_BASE }

    $dir = if ($InstallDir) { $InstallDir } else { Join-Path $env:LOCALAPPDATA 'Programs\rexplorer' }
    $app = Join-Path $dir 'rexplorer.exe'
    if (-not $Exe) { $Exe = Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe' }
    # Uninstalling needs no build: the installed copy can do it.
    if ($Uninstall -and -not (Test-Path -LiteralPath $Exe) -and (Test-Path -LiteralPath $app)) { $Exe = $app }
    if (-not (Test-Path -LiteralPath $Exe) -and -not $Uninstall) {
        Write-Host 'rexplorer.exe not found, building it with cargo build --release...'
        Push-Location (Split-Path $PSScriptRoot -Parent)
        try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build --release failed.' } } finally { Pop-Location }
    }
    if (-not (Test-Path -LiteralPath $Exe)) { throw "Not found: $Exe" }
    $Exe = (Resolve-Path -LiteralPath $Exe).Path

    if ($InstallDir) { $env:REXPLORER_INSTALL_DIR = $InstallDir }
    if ($RegBase) { $env:REXPLORER_REG_BASE = $RegBase }
    # The exe shows its own result box and exits 0 or non-zero.
    $p = Start-Process -FilePath $Exe -ArgumentList $flag -Wait -PassThru
    if ($p.ExitCode) { throw "rexplorer $flag failed (exit code $($p.ExitCode)); see the message box it showed." }

    if ($Uninstall) {
        Write-Host "`nrexplorer is uninstalled: folders, Win+E and the taskbar open Windows Explorer again."
        Write-Host "Your settings in $env:APPDATA\rexplorer stay, including anything imported from Explorer (an import is not undone)."
        Write-Host "To install again: powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`""
    } else {
        $import = $ImportExplorer
        if (-not $ImportExplorer -and -not $NoImport) {
            $answer = Read-Host 'Import your current Windows Explorer settings (hidden files, extensions, start folder...) into rexplorer? [Y/n]'
            $import = $answer -notmatch '^\s*n'
        }
        if ($import) {
            $q = Start-Process -FilePath $app -ArgumentList '--import-explorer' -Wait -PassThru
            if ($q.ExitCode) { Write-Host "The import failed (exit code $($q.ExitCode)); nothing was changed. Retry: & `"$app`" --import-explorer" -ForegroundColor Yellow }
        }
        Write-Host "`nrexplorer is installed in $dir"
        Write-Host '  Start it:        Win+E, the File Explorer icon on the taskbar, or open any folder; also the Start menu shortcut "rexplorer".'
        Write-Host "  Uninstall:       powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Uninstall    (or `"$app`" --uninstall)"
        Write-Host "  Import later:    & `"$app`" --import-explorer [--overwrite]    (keeps a settings.ini.bak-<date> backup)"
    }
} catch {
    Write-Host $_.Exception.Message -ForegroundColor Red
    $code = 1
}
exit $code
