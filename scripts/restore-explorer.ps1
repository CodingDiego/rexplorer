# Puts Windows Explorer back as the program that opens folders, drives and Win+E, WITHOUT needing a working
# rexplorer.exe (use it when rexplorer will not start, or after a broken install). Windows PowerShell 5.1, run as
# you, unelevated: everything rexplorer writes is under HKCU and %LOCALAPPDATA%.
#
#   powershell -ExecutionPolicy Bypass -File restore-explorer.ps1 [-RemoveProgram] [-RestartShell] [-Quiet] [-WhatIf]
#
# What it does, in order (safe to run twice, and when nothing is installed):
#   1. Closes the rexplorer processes that run from the install folder (and no other process).
#   2. Reads the install records (<install dir>\takeover.txt and install.txt) when present and reverts what they list;
#      then, records or not, goes through the fixed list of keys rexplorer can write ($KnownKeys below, the same list
#      as known_keys() in crates/core/src/install.rs) and removes only the values that point at rexplorer, plus the
#      desktop helper's Startup shortcut.
#   3. -RemoveProgram (or when rexplorer.exe is already gone): also the exe, the Start menu shortcut and the Apps and
#      Features entry; -RemoveProgram on the real base also the taskbar jump list. Without it rexplorer stays
#      installed as a normal program.
#   4. The Explorer shell that is running now may still hold the old registration. -RestartShell (off by default)
#      restarts explorer.exe, but only when no Explorer window and no file copy is open.
# -InstallDir / -RegBase / -ProgramsDir / -StartupDir point it at test locations (REXPLORER_INSTALL_DIR,
# REXPLORER_REG_BASE, REXPLORER_PROGRAMS_DIR, REXPLORER_STARTUP_DIR); a test base leaves the real Start menu and
# Startup folders alone unless both folder overrides are given, like the exe.
# Exit codes: 0 Explorer is back (or there was nothing to do), 1 something failed or another program's value was
# left in place (listed), 2 -RestartShell was refused because a window is open (everything else was done).
[CmdletBinding(SupportsShouldProcess = $true)]
param([string]$InstallDir, [string]$RegBase, [string]$ProgramsDir, [string]$StartupDir,
  [switch]$RemoveProgram, [switch]$RestartShell, [switch]$Quiet)
$ErrorActionPreference = 'Stop'
$script:Cmdlet = $PSCmdlet

# The keys rexplorer can write, relative to HKCU\<RegBase>. Keep in step with known_keys() in install.rs (the test
# install::tests::restore_script_knows_every_key checks that every one of them is here).
$KnownKeys = @(
  'Classes\Folder\shell\open\command',
  'Classes\Directory\shell\open\command',
  'Classes\Drive\shell\open\command',
  'Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open\command',
  'Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command',
  'Microsoft\Windows\CurrentVersion\App Paths\explorer.exe'
)
$ArpKey = 'Microsoft\Windows\CurrentVersion\Uninstall\rexplorer'
$ArpNames = @('DisplayName', 'DisplayVersion', 'Publisher', 'InstallLocation', 'DisplayIcon', 'UninstallString', 'NoModify', 'NoRepair')
# The only files it deletes; anything else in the install folder stays.
$OurFiles = @('rexplorer.exe', 'rexplorer.old', 'install.txt', 'takeover.txt', 'rexplorer.lnk', 'rexplorer desktop menu.lnk')

$code = 0
$script:left = @()
# Write-Host, not Write-Output: Remove-IfEmpty returns a value, and output written on the way would join it.
function Say($text) { if (-not $Quiet -or $text -match '^(Left|Failed)') { Write-Host $text } }
function Change($target, $action, [scriptblock]$do) {
  if ($script:Cmdlet.ShouldProcess($target, $action)) {
    try { & $do; Say "$action`: $target" } catch { $script:left += "Failed to $action $target`: $($_.Exception.Message)" }
  }
}

try {
  if (-not $InstallDir) { $InstallDir = $env:REXPLORER_INSTALL_DIR }
  if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\rexplorer' }
  if (-not $RegBase) { $RegBase = $env:REXPLORER_REG_BASE }
  if (-not $RegBase) { $RegBase = 'Software' }
  if (-not $ProgramsDir) { $ProgramsDir = $env:REXPLORER_PROGRAMS_DIR }
  if (-not $StartupDir) { $StartupDir = $env:REXPLORER_STARTUP_DIR }
  $RegBase = $RegBase.TrimEnd('\')
  if ($RegBase -notmatch '^Software(\\[^\\]+)*$' -or $RegBase -match '\\\.\.?(\\|$)') { throw "RegBase must be Software or a key under it: $RegBase" }
  $real = $RegBase -eq 'Software'
  # Same rule as install.rs shortcut_dirs(): both overrides, or the real folders on the real base, or none.
  if (-not ($ProgramsDir -and $StartupDir)) {
    if ($real) {
      if (-not $ProgramsDir) { $ProgramsDir = [Environment]::GetFolderPath('Programs') }
      if (-not $StartupDir) { $StartupDir = [Environment]::GetFolderPath('Startup') }
    } else { $ProgramsDir = $null; $StartupDir = $null }
  }
  $InstallDir = $InstallDir.TrimEnd('\')
  $exe = Join-Path $InstallDir 'rexplorer.exe'
  $removeProgram = $RemoveProgram -or -not (Test-Path -LiteralPath $exe)
  $HKCU = [Microsoft.Win32.Registry]::CurrentUser
  $baseLen = $RegBase.Length

  function Test-Ours($v) { ($v -is [string]) -and $v.ToLowerInvariant().Contains('rexplorer') }
  function Test-Mine($key) { $key.StartsWith("$RegBase\", [StringComparison]::OrdinalIgnoreCase) }
  # A record is a file in a user-writable folder; like install.rs revert(), it may name only the keys rexplorer
  # writes ($KnownKeys and the Apps and Features entry), with -Parents also an empty key on the way to one.
  $ourKeys = @($KnownKeys + $ArpKey | ForEach-Object { "$RegBase\$_" })
  function Test-OurKey($key, [switch]$Parents) {
    [bool]($ourKeys | Where-Object { $_ -eq $key -or ($Parents -and $_.StartsWith("$key\", [StringComparison]::OrdinalIgnoreCase)) })
  }
  # The folders install and take-over write files to.
  $ourDirs = @($InstallDir, $ProgramsDir, $StartupDir | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') })
  function Remove-IfEmpty($sub) {
    $k = $HKCU.OpenSubKey($sub)
    if (-not $k) { return $false }
    $empty = $k.SubKeyCount -eq 0 -and $k.ValueCount -eq 0
    $k.Close()
    if ($empty) { Change "HKCU\$sub" 'Delete empty key' { $HKCU.DeleteSubKey($sub, $false) } }
    $empty
  }
  # The key and the parents it leaves empty, up to (not including) RegBase, like register.rs unregister().
  function Remove-EmptyUp($sub) {
    while ($sub.Length -gt $baseLen -and (Remove-IfEmpty $sub)) { $sub = $sub.Substring(0, $sub.LastIndexOf('\')) }
  }
  # register.rs remove_if_ours(): a key whose default value names rexplorer loses that value, DelegateExecute and
  # Path; a key holding anything else is another program's and stays.
  function Clear-Command($sub) {
    $k = $HKCU.OpenSubKey($sub, $true)
    if (-not $k) { return }
    try {
      if (Test-Ours ($k.GetValue('', $null, 'DoNotExpandEnvironmentNames'))) {
        foreach ($n in '', 'DelegateExecute', 'Path') {
          if ($k.GetValueNames() -contains $n) { Change "HKCU\$sub\$(if ($n) { $n } else { '(default)' })" 'Delete value' { $k.DeleteValue($n, $false) } }
        }
      } elseif ($k.ValueCount -gt 0 -or $k.SubKeyCount -gt 0) {
        $script:left += "Left HKCU\$sub`: another program's value"
      }
    } finally { $k.Close() }
    Remove-EmptyUp $sub
  }
  function Remove-Arp($sub) {
    $k = $HKCU.OpenSubKey($sub, $true)
    if (-not $k) { return }
    try {
      if ((Test-Ours $k.GetValue('UninstallString')) -or (Test-Ours $k.GetValue('DisplayIcon'))) {
        foreach ($n in $ArpNames) { if ($k.GetValueNames() -contains $n) { Change "HKCU\$sub\$n" 'Delete value' { $k.DeleteValue($n, $false) } } }
      } else { $script:left += "Left HKCU\$sub`: it does not name rexplorer" }
    } finally { $k.Close() }
    Remove-EmptyUp $sub
  }
  function Remove-OurFile($path) {
    if ($path -and (Test-Path -LiteralPath $path -PathType Leaf) -and ($OurFiles -contains (Split-Path $path -Leaf)) -and
      ($ourDirs -contains (Split-Path $path -Parent).TrimEnd('\'))) {
      Change $path 'Delete file' { Remove-Item -LiteralPath $path -Force }
    }
  }
  # The shell names an app's jump list file after the CRC-64 (reflected polynomial 0x92C64265D32139A4, initial value
  # all ones, no zero padding) of its upper-case AppUserModelID in UTF-16; rexplorer's is jumplist.rs APP_ID.
  function Get-JumpListName($appId) {
    $poly = [uint64]::Parse('92C64265D32139A4', 'AllowHexSpecifier'); $c = [uint64]::MaxValue
    foreach ($b in [Text.Encoding]::Unicode.GetBytes($appId.ToUpperInvariant())) {
      $c = $c -bxor [uint64]$b
      for ($k = 0; $k -lt 8; $k++) { if ($c -band [uint64]1) { $c = ($c -shr 1) -bxor $poly } else { $c = $c -shr 1 } }
    }
    '{0:x}.customDestinations-ms' -f $c
  }
  function Read-Record($name) {
    $f = Join-Path $InstallDir $name
    if (Test-Path -LiteralPath $f) { @(Get-Content -LiteralPath $f | Where-Object { $_ }) } else { @() }
  }
  # One record's registry and file lines; `program`: also its named values and files (install.txt's).
  function Undo-Record($lines) {
    foreach ($l in $lines) {
      $kind, $rest = $l -split ' ', 2
      switch ($kind) {
        'value' { if (Test-OurKey $rest) { Clear-Command $rest } }
        'named' {
          $i = $rest.LastIndexOf('|')
          if ($i -gt 0 -and (Test-OurKey $rest.Substring(0, $i))) {
            $sub = $rest.Substring(0, $i); $n = $rest.Substring($i + 1)
            $k = $HKCU.OpenSubKey($sub, $true)
            if ($k) { try { if ($k.GetValueNames() -contains $n) { Change "HKCU\$sub\$n" 'Delete value' { $k.DeleteValue($n, $false) } } } finally { $k.Close() } }
          }
        }
        'file' { Remove-OurFile $rest }
      }
    }
    $lines | Where-Object { $_ -like 'key *' } | ForEach-Object { $_.Substring(4) } | Where-Object { (Test-Mine $_) -and (Test-OurKey $_ -Parents) } |
      Sort-Object Length -Descending | ForEach-Object { $null = Remove-IfEmpty $_ }
  }

  # 1. Our processes only: rexplorer.exe running from the install folder (its helper and windows).
  Get-Process -Name rexplorer -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith("$InstallDir\", [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object { $p = $_; Change "rexplorer (pid $($p.Id), $($p.Path))" 'Close' { $p.CloseMainWindow() | Out-Null; if (-not $p.WaitForExit(3000)) { $p.Kill(); $null = $p.WaitForExit(3000) } } }

  # 2. Folders back to Explorer: the take-over record, an older install's record (it held these lines too), then the
  # fixed list for anything a lost or unjournaled record missed.
  $takeover = Read-Record 'takeover.txt'
  $install = Read-Record 'install.txt'
  Undo-Record $takeover
  Undo-Record ($install | Where-Object { $_ -like 'value *' -or $_ -like 'file *desktop menu.lnk' })
  foreach ($rel in $KnownKeys) { Clear-Command "$RegBase\$rel" }
  if ($StartupDir) { Remove-OurFile (Join-Path $StartupDir 'rexplorer desktop menu.lnk') }
  Remove-OurFile (Join-Path $InstallDir 'takeover.txt')

  # 3. The program itself.
  if ($removeProgram) {
    Undo-Record $install
    Remove-Arp "$RegBase\$ArpKey"
    if ($ProgramsDir) { Remove-OurFile (Join-Path $ProgramsDir 'rexplorer.lnk') }
    foreach ($f in 'rexplorer.exe', 'rexplorer.old', 'install.txt') { Remove-OurFile (Join-Path $InstallDir $f) }
    if ($RemoveProgram -and $real) {
      $jl = Join-Path ([Environment]::GetFolderPath('Recent')) "CustomDestinations\$(Get-JumpListName 'CodingDiego.rexplorer')"
      if (Test-Path -LiteralPath $jl -PathType Leaf) { Change $jl 'Delete file' { Remove-Item -LiteralPath $jl -Force } }
    }
    # The folders install created (deepest first in the record), then the install folder, each only when empty.
    $dirs = @($takeover + $install | Where-Object { $_ -like 'dir *' } | ForEach-Object { $_.Substring(4) }) + $InstallDir
    foreach ($d in $dirs) {
      if ((Test-Path -LiteralPath $d -PathType Container) -and -not (Get-ChildItem -LiteralPath $d -Force | Select-Object -First 1)) {
        Change $d 'Delete empty folder' { Remove-Item -LiteralPath $d -Force }
      }
    }
  } else {
    # A record of what a take-over without an install created: its folders, when empty.
    foreach ($d in @($takeover | Where-Object { $_ -like 'dir *' } | ForEach-Object { $_.Substring(4) })) {
      if ((Test-Path -LiteralPath $d -PathType Container) -and -not (Get-ChildItem -LiteralPath $d -Force | Select-Object -First 1)) {
        Change $d 'Delete empty folder' { Remove-Item -LiteralPath $d -Force }
      }
    }
  }

  foreach ($l in $script:left) { Write-Host $l }
  if ($script:left | Where-Object { $_ -like 'Failed*' -or $_ -like 'Left*' }) { $code = 1 }
  Say ''
  Say 'Windows Explorer opens folders, drives and Win+E again.'
  if (-not $removeProgram -and (Test-Path -LiteralPath $exe)) { Say "rexplorer is still installed as a program in $InstallDir; remove it too with -RemoveProgram." }

  # 4. The running shell.
  if ($RestartShell) {
    if (-not $real) { Say 'Not restarting explorer.exe: -RegBase points at a test key, the shell never read it.' }
    else {
      $windows = @((New-Object -ComObject Shell.Application).Windows()).Count
      Add-Type -Namespace RexRestore -Name W -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern System.IntPtr FindWindowW(string c, string t);'
      $copying = [RexRestore.W]::FindWindowW('OperationStatusWindow', $null) -ne [IntPtr]::Zero
      if ($windows -gt 0 -or $copying) {
        Write-Host "Not restarting explorer.exe: $windows Explorer window(s) open, file copy in progress=$copying. Close them and run this again with -RestartShell, or sign out and back in."
        if ($code -eq 0) { $code = 2 }
      } elseif ($script:Cmdlet.ShouldProcess('explorer.exe', 'Restart the Windows shell')) {
        Get-Process -Name explorer -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Seconds 3
        # By full path: a bare name goes through App Paths (which may still name rexplorer) and the PATH.
        if (-not (Get-Process -Name explorer -ErrorAction SilentlyContinue)) { Start-Process (Join-Path $env:WINDIR 'explorer.exe') }
        Say 'explorer.exe restarted.'
      }
    }
  } else {
    Say 'The Explorer shell that is running now may still hold the old registration until you sign out and back in.'
    Say "To restart it now instead: powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" -RestartShell (it refuses while an Explorer window or a file copy is open)."
  }
} catch {
  Write-Host "Failed: $($_.Exception.Message)"
  $code = 1
}
exit $code
