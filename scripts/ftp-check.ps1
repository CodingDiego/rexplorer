# Live FTP check (#36): a copy of the exe on a scratch profile against scripts\ftp-server.py, a read-only server on
# 127.0.0.1. It drives the window with messages and MSAA only: no real keyboard or mouse input, no clipboard.
# Checks: the root lists, a subfolder opens with Enter, Up returns, a refused login says so, the shell's copy engine
# copies a file out, and the password is never in the title, the address box, a message or any file of the profile.
# Needs Python 3 (-Python, default `python` on PATH). Exits 1 on any FAIL.
param([string]$Exe = (Join-Path $PSScriptRoot '..\target\release\rexplorer.exe'), [int]$Port = 52121, [string]$Python = 'python')
$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies Accessibility @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text; using Accessibility;
public static class F {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string c, string t);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] static extern IntPtr GetText(IntPtr h, uint m, IntPtr n, StringBuilder s);
  // WM_GETTEXT: an edit box of another process answers this, not GetWindowText.
  public static string EditText(IntPtr h) { var s = new StringBuilder(4096); GetText(h, 0x0D, (IntPtr)4096, s); return s.ToString(); }
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  delegate bool Cb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(Cb cb, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr p, Cb cb, IntPtr l);
  [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h, uint id, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out object o);
  public static string Text(IntPtr h) { var s = new StringBuilder(4096); GetWindowText(h, s, 4096); return s.ToString(); }
  // A visible top-level window of the process other than `main`: a message box or a login dialog.
  public static IntPtr Popup(uint pid, IntPtr main) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && h != main && IsWindowVisible(h)) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  // The window's own text and every child's.
  public static string AllText(IntPtr h) {
    var sb = new StringBuilder(Text(h));
    EnumChildWindows(h, (c, l) => { sb.Append(" | ").Append(Text(c)); return true; }, IntPtr.Zero);
    return sb.ToString();
  }
  static IAccessible Acc(IntPtr h) { Guid g = new Guid("618736E0-3C3D-11CF-810C-00AA00389B71"); object o; return AccessibleObjectFromWindow(h, 0xFFFFFFFC, ref g, out o) == 0 ? o as IAccessible : null; }
  public static string[] Names(IntPtr list) {
    var a = Acc(list); var r = new List<string>(); if (a == null) return r.ToArray();
    for (int i = 1; i <= a.accChildCount; i++) { try { r.Add(a.get_accName(i) ?? ""); } catch { r.Add(""); } }
    return r.ToArray();
  }
  // Focuses and selects the row named `name` (SELFLAG_TAKEFOCUS | SELFLAG_TAKESELECTION).
  public static bool Select(IntPtr list, string name) {
    var a = Acc(list); if (a == null) return false;
    for (int i = 1; i <= a.accChildCount; i++) { try { if (a.get_accName(i) == name) { a.accSelect(3, i); return true; } } catch { } }
    return false;
  }
}
'@
$Exe = (Resolve-Path $Exe).Path
$Secret = 'pa55-SECRET-w0rd'
$Wrong = 'wr0ng-SECRET-pa55'
$User = 'tester'
$Work = Join-Path $env:TEMP "rexplorer-ftp-test-$PID"
$Root = Join-Path $Work 'root'
New-Item -ItemType Directory -Path (Join-Path $Root 'docs'), (Join-Path $Work 'profile\rexplorer'), (Join-Path $Work 'local'), (Join-Path $Work 'out') -Force | Out-Null
Set-Content -Path (Join-Path $Root 'hello.txt') -Value ('hello from the ftp root ' * 40) -NoNewline
Set-Content -Path (Join-Path $Root 'docs\inner.txt') -Value 'inner' -NoNewline
Set-Content -Path (Join-Path $Root 'docs\second.txt') -Value 'second' -NoNewline
Set-Content -Path (Join-Path $Work 'profile\rexplorer\settings.ini') -Value "[general]`r`nlanguage=en`r`n"
$ExeCopy = Join-Path $Work 'rexplorer.exe'
Copy-Item $Exe $ExeCopy
$ServerLog = Join-Path $Work 'server.log'
$results = New-Object System.Collections.Generic.List[string]
function Check($ok, $name, $detail) { $results.Add(('{0} {1}: {2}' -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $name, $detail)) }
function Wait-Until([scriptblock]$cond, $ms) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
  [bool](& $cond)
}
$saved = @{}
foreach ($n in 'APPDATA', 'LOCALAPPDATA', 'REXPLORER_INSTALL_DIR', 'REXPLORER_REG_BASE') { $saved[$n] = [Environment]::GetEnvironmentVariable($n) }
$server = $null; $app = $null
try {
  $server = Start-Process -FilePath $Python -ArgumentList @('-I', "`"$PSScriptRoot\ftp-server.py`"", "`"$Root`"", $Port, $User, $Secret, "`"$ServerLog`"") -PassThru -WindowStyle Hidden
  if (-not (Wait-Until { Test-Path $ServerLog } 8000)) { throw "the FTP server did not start ($Python)" }
  # A scratch profile, and scratch install and registry bases in case a launch ever reaches them.
  $env:APPDATA = Join-Path $Work 'profile'; $env:LOCALAPPDATA = Join-Path $Work 'local'
  $env:REXPLORER_INSTALL_DIR = Join-Path $Work 'install'; $env:REXPLORER_REG_BASE = "Software\rexplorer-test\ftp$PID"
  $app = Start-Process -FilePath $ExeCopy -ArgumentList '"C:\Windows\Web"' -PassThru
  $null = $app.Handle
  $null = Wait-Until { $app.Refresh(); $app.MainWindowHandle -ne [IntPtr]::Zero } 8000
  $main = $app.MainWindowHandle
  if ($main -eq [IntPtr]::Zero) { throw 'rexplorer showed no window' }
  Start-Sleep -Milliseconds 1200
  $list = [F]::FindWindowEx($main, [IntPtr]::Zero, 'SysListView32', $null)
  $addr = [F]::GetDlgItem($main, 100)
  function Send-Command($cmd) { $null = [F]::PostMessage($main, 0x111, [IntPtr]$cmd, [IntPtr]::Zero) }
  # Ctrl+L, the text into the address box, Enter: as the commands they send.
  function Go($path) {
    Send-Command 5; Start-Sleep -Milliseconds 300
    $null = [F]::SendText($addr, 0x0C, [IntPtr]::Zero, $path)
    Send-Command 7
  }
  function Get-Names { @([F]::Names($list)) }
  function Shown { 'title=[{0}]' -f [F]::Text($main) }

  $url = "ftp://${User}:$Secret@127.0.0.1:$Port/"
  Go $url
  $listed = Wait-Until { (Get-Names) -contains 'hello.txt' } 60000
  Check ($listed -and ((Get-Names) -contains 'docs')) 'the root lists' ('items: ' + ((Get-Names) -join ', ') + '; ' + (Shown))
  Check (-not (Shown).Contains($Secret)) 'root: the title has no password' (Shown)
  Send-Command 5; Start-Sleep -Milliseconds 400
  $typed = [F]::EditText($addr)
  Check ($typed -like 'ftp://*127.0.0.1*' -and -not $typed.Contains($Secret)) 'the address box shows the place without the password' $typed
  $null = [F]::PostMessage($addr, 0x100, [IntPtr]0x1B, [IntPtr]::Zero); Start-Sleep -Milliseconds 300

  $opened = [F]::Select($list, 'docs')
  Start-Sleep -Milliseconds 300
  if ($opened) { Send-Command 7 }
  $inDocs = Wait-Until { (Get-Names) -contains 'inner.txt' } 20000
  Check ($inDocs -and $opened) 'a subfolder opens with Enter' ('items: ' + ((Get-Names) -join ', ') + '; ' + (Shown))
  Check (-not (Shown).Contains($Secret)) 'subfolder: the title has no password' (Shown)
  Send-Command 3
  Check (Wait-Until { (Get-Names) -contains 'hello.txt' } 20000) 'Up returns to the root' (((Get-Names) -join ', ') + '; ' + (Shown))

  # A refused login: a message that names the place without the password, and the tab stays where it was.
  # Another user name: with the user it is logged in as, the shell can answer from the connection it holds and
  # never ask the server (seen once in seven runs: no login in the server's log, and the folder opened).
  Go "ftp://not-$User`:$Wrong@127.0.0.1:$Port/docs/"
  $box = [IntPtr]::Zero
  $null = Wait-Until { $script:box = [F]::Popup([uint32]$app.Id, $main); $script:box -ne [IntPtr]::Zero } 30000
  $said = if ($box -ne [IntPtr]::Zero) { [F]::AllText($box) } else { '' }
  $refused = @(Get-Content $ServerLog | Where-Object { $_ -match "login REFUSED for not-$User" }).Count -gt 0
  Check ($box -ne [IntPtr]::Zero -and $refused -and $said -match '127\.0\.0\.1' -and -not $said.Contains($Wrong)) 'a refused login says so without the password' "$said; refused in the server's log: $refused"
  if ($box -ne [IntPtr]::Zero) { $null = [F]::PostMessage($box, 0x10, [IntPtr]::Zero, [IntPtr]::Zero); Start-Sleep -Milliseconds 500 }
  Check ((Get-Names) -contains 'hello.txt' -and -not (Shown).Contains($Wrong)) 'after it the tab still shows the root' (((Get-Names) -join ', ') + '; ' + (Shown))

  Go 'C:\Windows\Web'
  Check (Wait-Until { [F]::Text($main) -like 'Web*' } 8000) 'leaves FTP for a local folder' (Shown)

  # Copying out is the shell's copy engine whoever asks; asked from this process, since the app's Copy needs the clipboard.
  try {
    $shell = New-Object -ComObject Shell.Application
    $out = Join-Path $Work 'out'
    $item = $shell.NameSpace($url).Items() | Where-Object { $_.Name -like 'hello*' } | Select-Object -First 1
    $shell.NameSpace($out).CopyHere($item, 0x614)
    $copy = Join-Path $out 'hello.txt'
    $null = Wait-Until { (Test-Path $copy) -and ((Get-ChildItem -LiteralPath $copy).Length -gt 0) } 15000
    $len = if (Test-Path $copy) { (Get-ChildItem -LiteralPath $copy).Length } else { -1 }
    $same = (Test-Path $copy) -and ((Get-FileHash $copy).Hash -eq (Get-FileHash (Join-Path $Root 'hello.txt')).Hash)
    Check $same 'the shell copies a file out' "bytes written: $len of $((Get-ChildItem (Join-Path $Root 'hello.txt')).Length)"
  } catch { Check $false 'the shell copies a file out' $_.Exception.Message }

  # Closed normally, so the session and recent files are written; then no file may hold either password.
  $null = [F]::PostMessage($main, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
  $null = $app.WaitForExit(8000)
  $files = @(Get-ChildItem -Path (Join-Path $Work 'profile'), (Join-Path $Work 'local') -Recurse -File)
  $hits = @($files | Where-Object { $bytes = [IO.File]::ReadAllBytes($_.FullName); $text = [Text.Encoding]::UTF8.GetString($bytes) + [Text.Encoding]::Unicode.GetString($bytes); $text.Contains($Secret) -or $text.Contains($Wrong) })
  Check ($hits.Count -eq 0) 'no file of the profile holds a password' ("$($files.Count) files; hits: " + (($hits | ForEach-Object { $_.Name }) -join ', '))
  $recent = Join-Path $Work 'profile\rexplorer\recent.txt'
  $kept = if (Test-Path $recent) { (Get-Content $recent) -join ' | ' } else { '' }
  Check ($kept -match "ftp://$User@127\.0\.0\.1:$Port/docs/") 'the recent list keeps the place, without the password' $kept
  $logins = @(Get-Content $ServerLog | Where-Object { $_ -match 'login (ok|REFUSED)' } | ForEach-Object { ($_ -split 'login ')[1] })
  $results.Add('INFO server logins: ' + ($logins -join '; '))
} finally {
  foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
  if ($app -and -not $app.HasExited) { $app.Kill() }
  # With its children: `python` can be a launcher (py, a venv) whose interpreter is the process that listens.
  # Through cmd: under ErrorActionPreference Stop, PowerShell 5.1 throws on a native command's stderr line.
  if ($server -and -not $server.HasExited) { & cmd.exe /c "taskkill /T /F /PID $($server.Id) >nul 2>&1" }
  $results | ForEach-Object { $_ }
  Start-Sleep -Milliseconds 300
  Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item "HKCU:\Software\rexplorer-test\ftp$PID" -Recurse -Force -ErrorAction SilentlyContinue
}
if ($results -match '^FAIL') { exit 1 }
