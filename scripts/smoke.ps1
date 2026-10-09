# rexplorer smoke test. Windows PowerShell 5.1. Prints one PASS/FAIL/SKIP line per check, always exits 0.
$ErrorActionPreference = 'Stop'
$Exe = Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe'
Add-Type -AssemblyName System.Windows.Forms, Microsoft.VisualBasic, UIAutomationClient, UIAutomationTypes
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class W {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string cls, string title);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h, string name);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  // First descendant of `parent` whose window class contains `cls`.
  public static IntPtr FindDescendant(IntPtr parent, string cls) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(parent, (h, l) => {
      var sb = new StringBuilder(128); GetClassName(h, sb, 128);
      if (sb.ToString().Contains(cls)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static int Left(IntPtr h) { RECT r; GetWindowRect(h, out r); return r.L; }
  // The visible direct child Edit of `parent` (the search box; the address Edit stays hidden until Ctrl+L).
  public static IntPtr VisibleEdit(IntPtr parent) {
    IntPtr h = IntPtr.Zero;
    while ((h = FindWindowEx(parent, h, "Edit", null)) != IntPtr.Zero) { if (IsWindowVisible(h)) return h; }
    return IntPtr.Zero;
  }
  // The RAR5 fixture of crates/core/src/archive.rs tests: one file docs/a.txt holding "hello".
  static uint Crc32(byte[] d) {
    uint c = 0xFFFFFFFF;
    foreach (byte b in d) { c ^= b; for (int k = 0; k < 8; k++) c = (c & 1) != 0 ? (c >> 1) ^ 0xEDB88320 : c >> 1; }
    return ~c;
  }
  static void Vint(List<byte> o, ulong v) {
    while (true) { byte b = (byte)(v & 0x7F); v >>= 7; if (v == 0) { o.Add(b); return; } o.Add((byte)(b | 0x80)); }
  }
  static byte[] Header(byte[] body) {
    var after = new List<byte>(); Vint(after, (ulong)body.Length); after.AddRange(body);
    var h = new List<byte>(BitConverter.GetBytes(Crc32(after.ToArray()))); h.AddRange(after); return h.ToArray();
  }
  public static byte[] Rar5() {
    byte[] name = Encoding.ASCII.GetBytes("docs/a.txt");
    var f = new List<byte> { 2, 0x02 };
    foreach (ulong v in new ulong[] { 5, 0, 5, 0x20, 0, 0, (ulong)name.Length }) Vint(f, v);
    f.AddRange(name);
    var rar = new List<byte> { 0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00 };
    rar.AddRange(Header(new byte[] { 1, 0, 0 }));
    rar.AddRange(Header(f.ToArray()));
    rar.AddRange(Encoding.ASCII.GetBytes("hello"));
    rar.AddRange(Header(new byte[] { 5, 0, 0 }));
    return rar.ToArray();
  }
  // First top-level window of process `pid` with window class `cls`.
  public static IntPtr FindOfProcess(uint pid, string cls) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      var sb = new StringBuilder(64); GetClassName(h, sb, 64);
      if (p == pid && sb.ToString() == cls) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
'@

# Window rects in physical pixels, like the app's own layout.
$null = [W]::SetProcessDPIAware()
function Out-Check($status, $name, $detail) { Write-Output ("{0} {1} {2}" -f $status, $name, ($detail -replace '\s+', ' ')) }
function Start-App($arg) {
  if ($arg) { $p = Start-Process -FilePath $Exe -ArgumentList ('"{0}"' -f $arg) -PassThru } else { $p = Start-Process -FilePath $Exe -PassThru }
  $null = $p.Handle  # cache handle so ExitCode works after exit
  $p
}
function Get-Main($p) {
  for ($i = 0; $i -lt 100; $i++) {
    $p.Refresh(); if ($p.HasExited) { return [IntPtr]::Zero }
    if ($p.MainWindowHandle -ne [IntPtr]::Zero) { return $p.MainWindowHandle }
    Start-Sleep -Milliseconds 50
  }
  [IntPtr]::Zero
}
function Get-Title($h) {
  $n = [W]::GetWindowTextLength($h); $sb = New-Object System.Text.StringBuilder ($n + 2)
  $null = [W]::GetWindowText($h, $sb, $sb.Capacity); $sb.ToString()
}
function Get-List($m) { [W]::FindWindowEx($m, [IntPtr]::Zero, 'SysListView32', $null) }
function Get-Count($m) {
  $l = Get-List $m; if ($l -eq [IntPtr]::Zero) { return -1 }
  [int][W]::SendMessage($l, 0x1004, [IntPtr]::Zero, [IntPtr]::Zero)
}
function Wait-Until([scriptblock]$cond, $ms) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 50 }
  [bool](& $cond)
}
function Set-Foreground($m) {
  for ($i = 0; $i -lt 3; $i++) {
    try { [Microsoft.VisualBasic.Interaction]::AppActivate($script:cur.Id) } catch {}
    $null = [W]::SetForegroundWindow($m); Start-Sleep -Milliseconds 300
    if ([W]::GetForegroundWindow() -eq $m) { return $true }
  }
  $false
}
function Stop-App($p) { if ($p) { try { if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit(2000) | Out-Null } } catch {} } }
function Get-Explorers { $r = @{}; foreach ($w in (New-Object -ComObject Shell.Application).Windows()) { try { $r[[int64]$w.HWND] = $w } catch {} }; $r }
function Close-NewExplorers($before) { foreach ($k in (Get-Explorers).GetEnumerator()) { if (-not $before.ContainsKey($k.Key)) { try { $k.Value.Quit() } catch {} } } }

# Runs one check: $body returns @(status, detail). Process in $script:cur is always killed.
function Run-Check($name, [scriptblock]$body) {
  $script:cur = $null
  try { $r = & $body; Out-Check $r[0] $name $r[1] }
  catch { Out-Check 'FAIL' $name ("exception: " + $_.Exception.Message) }
  finally { Stop-App $script:cur }
}
function Launch($arg) {
  $script:cur = Start-App $arg
  $script:main = Get-Main $script:cur
  $script:main -ne [IntPtr]::Zero
}

# Launches with APPDATA pointed at a fresh scratch folder: a first run on profile defaults, real settings untouched.
function Launch-Fresh($arg) {
  $script:fake = Join-Path $env:TEMP "rexplorer-smoke-appdata-$PID"
  Remove-Item $script:fake -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $script:fake -Force | Out-Null
  $appdata = $env:APPDATA
  try { $env:APPDATA = $script:fake; Launch $arg } finally { $env:APPDATA = $appdata }
}
function Remove-Fresh { Stop-App $script:cur; Start-Sleep -Milliseconds 200; Remove-Item $script:fake -Recurse -Force -ErrorAction SilentlyContinue }
# Like Launch-Fresh, with a raw argument string (several arguments).
function Launch-FreshArgs($argline) {
  $script:fake = Join-Path $env:TEMP "rexplorer-smoke-appdata-$PID"
  Remove-Item $script:fake -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $script:fake -Force | Out-Null
  $appdata = $env:APPDATA
  try {
    $env:APPDATA = $script:fake
    $script:cur = Start-Process -FilePath $Exe -ArgumentList $argline -PassThru; $null = $script:cur.Handle
    $script:main = Get-Main $script:cur
  } finally { $env:APPDATA = $appdata }
  $script:main -ne [IntPtr]::Zero
}
# Keys when the window can be activated, else the same command as WM_COMMAND. Returns how it was sent.
function Send-KeysOrCommand($keys, $cmd) {
  if (Set-Foreground $script:main) { [System.Windows.Forms.SendKeys]::SendWait($keys); 'keys' }
  else { $null = [W]::PostMessage($script:main, 0x111, [IntPtr]$cmd, [IntPtr]::Zero); 'WM_COMMAND' }
}

if (-not (Test-Path $Exe)) { Out-Check 'FAIL' 'startup' "exe missing: $Exe"; exit 0 }

Run-Check 'startup' {
  if (-not (Launch $null)) { return @('FAIL', 'no main window within 5 s') }
  Start-Sleep -Seconds 3; $script:cur.Refresh()
  $t = Get-Title $script:main; $c = Get-Count $script:main
  if ($script:cur.HasExited) { return @('FAIL', 'process exited') }
  if ($t.EndsWith(' - rexplorer') -and $c -ge 1) { @('PASS', "title='$t' count=$c") } else { @('FAIL', "title='$t' count=$c") }
}

Run-Check 'perf' {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $script:cur = Start-App (Join-Path $env:TEMP 'rexplorer-bench')
  $last = -1; $main = [IntPtr]::Zero
  while ($sw.ElapsedMilliseconds -lt 10000) {
    if ($main -eq [IntPtr]::Zero) { $script:cur.Refresh(); $main = $script:cur.MainWindowHandle }
    else { $last = Get-Count $main; if ($last -eq 100000) { break } }
    Start-Sleep -Milliseconds 50
  }
  $ms = $sw.ElapsedMilliseconds
  if ($last -ne 100000) { return @('FAIL', "count never reached 100000, last=$last") }
  if ($ms -lt 2500) { @('PASS', "${ms} ms") } else { @('FAIL', "${ms} ms (limit 2500)") }
}

Run-Check 'columns' {
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  $ok = Wait-Until { (Get-Count $script:main) -ge 1 } 3000
  $hdr = [W]::FindWindowEx((Get-List $script:main), [IntPtr]::Zero, 'SysHeader32', $null)
  $h = if ($hdr -eq [IntPtr]::Zero) { -1 } else { [int][W]::SendMessage($hdr, 0x1200, [IntPtr]::Zero, [IntPtr]::Zero) }
  $c = Get-Count $script:main
  if ($ok -and $h -ge 4) { @('PASS', "columns=$h count=$c") } else { @('FAIL', "columns=$h count=$c") }
}

Run-Check 'keyboard-nav' {
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
  $orig = Get-Title $script:main
  if (-not (Set-Foreground $script:main)) { return @('SKIP', 'could not activate') }
  [System.Windows.Forms.SendKeys]::SendWait('{BACKSPACE}')
  $a = Wait-Until { (Get-Title $script:main) -ne $orig } 2000
  $up = Get-Title $script:main
  [System.Windows.Forms.SendKeys]::SendWait('%{LEFT}')
  $b = Wait-Until { (Get-Title $script:main) -eq $orig } 2000
  if ($a -and $b) { @('PASS', "'$orig' -> '$up' -> back") }
  elseif ([W]::GetForegroundWindow() -ne $script:main) { @('SKIP', 'could not activate (focus lost)') }
  else { @('FAIL', "A(up)=$a B(back)=$b orig='$orig' now='$(Get-Title $script:main)'") }
}

Run-Check 'address-nav' {
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
  $orig = Get-Title $script:main
  if (-not (Set-Foreground $script:main)) { return @('SKIP', 'could not activate') }
  # The address Edit stays hidden behind the breadcrumb until Ctrl+L.
  [System.Windows.Forms.SendKeys]::SendWait('^l'); Start-Sleep -Milliseconds 300
  $root = [Windows.Automation.AutomationElement]::FromHandle($script:main)
  # rexplorer's Edit is exposed to UIA as ControlType.Pane, so match by class name instead
  $cond = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ClassNameProperty), 'Edit'
  $ed = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $cond)
  if (-not $ed) { return @('FAIL', 'Edit control not found via UIA') }
  $vp = $null
  if (-not $ed.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$vp)) { return @('SKIP', 'Edit exposes no UIA ValuePattern, cannot set text via UIA') }
  $ed.SetFocus()
  $vp.SetValue('C:\Users')
  [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
  $ok = Wait-Until { (Get-Title $script:main) -ne $orig -and (Get-Count $script:main) -ge 1 } 2000
  $t = Get-Title $script:main; $c = Get-Count $script:main
  if ($ok) { @('PASS', "'$orig' -> '$t' count=$c") }
  elseif ([W]::GetForegroundWindow() -ne $script:main) { @('SKIP', 'could not activate (focus lost)') }
  else { @('FAIL', "title='$t' count=$c") }
}

Run-Check 'live-update' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-$PID"
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  try {
    if (-not (Launch $dir)) { return @('FAIL', 'no main window within 5 s') }
    Start-Sleep -Seconds 2; $c0 = Get-Count $script:main
    if ($c0 -ne 0) { return @('FAIL', "initial count=$c0, expected 0") }
    Set-Content -Path (Join-Path $dir 'a.txt') -Value 'x'
    $ok = Wait-Until { (Get-Count $script:main) -eq 1 } 2000
    $c1 = Get-Count $script:main
    if ($ok) { @('PASS', "count 0 -> $c1") } else { @('FAIL', "count after create=$c1") }
  } finally { Stop-App $script:cur; Start-Sleep -Milliseconds 200; Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Run-Check 'fallback-embedded' {
  # A name that does not resolve: an error box and no Explorer window (folders that exist but cannot be
  # listed open in the embedded Explorer view instead of explorer.exe).
  $before = Get-Explorers
  try {
    if (-not (Launch-Fresh 'shell:::{11111111-2222-3333-4444-555555555555}')) { return @('FAIL', 'no main window within 5 s') }
    Start-Sleep -Seconds 3; $script:cur.Refresh()
    $alive = -not $script:cur.HasExited
    $newExp = @((Get-Explorers).Keys | Where-Object { -not $before.ContainsKey($_) }).Count
    $dlg = [W]::FindOfProcess([uint32]$script:cur.Id, '#32770')
    if ($dlg -ne [IntPtr]::Zero) { $null = [W]::PostMessage($dlg, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) }
    $d = "explorer windows new=$newExp; rexplorer dialog appeared=$($dlg -ne [IntPtr]::Zero)"
    if ($alive -and $newExp -eq 0) { @('PASS', "alive after 3 s; $d") } else { @('FAIL', "alive=$alive; $d") }
  } finally { Close-NewExplorers $before; Remove-Fresh }
}

Run-Check 'explorer-hotkey' {
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
  if (-not (Set-Foreground $script:main)) { return @('SKIP', 'could not activate') }
  $before = Get-Explorers
  try {
    [System.Windows.Forms.SendKeys]::SendWait('^+e')
    $ok = Wait-Until { (Get-Explorers).Count -gt $before.Count } 4000
    $n = (Get-Explorers).Count
  } finally { Close-NewExplorers $before }
  if ($ok) { @('PASS', "explorer windows $($before.Count) -> $n") }
  elseif ([W]::GetForegroundWindow() -ne $script:main) { @('SKIP', 'could not activate (focus lost)') }
  else { @('FAIL', "explorer windows stayed at $($before.Count)") }
}

Run-Check 'tabs' {
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
  $orig = Get-Title $script:main; $c0 = Get-Count $script:main
  $strip = [W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexTabs', $null)
  if ($strip -eq [IntPtr]::Zero) { return @('FAIL', 'no RexTabs child window') }
  if (-not (Set-Foreground $script:main)) { return @('SKIP', 'could not activate') }
  [System.Windows.Forms.SendKeys]::SendWait('^t')
  Start-Sleep -Milliseconds 300
  $ok = Wait-Until { (Get-Count $script:main) -eq $c0 } 2000
  $t = Get-Title $script:main; $c = Get-Count $script:main
  $alive = -not $script:cur.HasExited
  if ($alive -and $ok -and $t -eq $orig) { @('PASS', "RexTabs present; after Ctrl+T title='$t' count=$c") }
  elseif ([W]::GetForegroundWindow() -ne $script:main) { @('SKIP', 'could not activate (focus lost)') }
  else { @('FAIL', "alive=$alive title '$orig' -> '$t' count $c0 -> $c") }
}

Run-Check 'single-instance' {
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
  $orig = Get-Title $script:main
  $second = Start-App 'C:\Users'
  try {
    $exited = $second.WaitForExit(3000)
    $script:cur.Refresh(); $alive = -not $script:cur.HasExited
    # 'rexplorer' alone is the new tab while it loads.
    $changed = Wait-Until { $n = Get-Title $script:main; $n -ne $orig -and $n -ne 'rexplorer' } 3000
    $t = Get-Title $script:main
    if ($exited -and $alive -and $changed) { @('PASS', "second exited, first alive, title '$orig' -> '$t' (new tab)") }
    else { @('FAIL', "second exited=$exited first alive=$alive title '$orig' -> '$t'") }
  } finally { Stop-App $second }
}

Run-Check 'theme' {
  # A first run: point APPDATA at an empty folder so the real settings are untouched.
  $appdata = $env:APPDATA
  $fake = Join-Path $env:TEMP "rexplorer-smoke-appdata-$PID"
  Remove-Item $fake -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $fake -Force | Out-Null
  $dir = Join-Path $fake 'rexplorer'
  try {
    $env:APPDATA = $fake
    if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    $env:APPDATA = $appdata
    Start-Sleep -Seconds 1
    $missing = @('settings.ini', 'themes\dark.ini', 'themes\light.ini', 'icons\default\folder.png') | Where-Object { -not (Test-Path (Join-Path $dir $_)) }
    $null = [W]::PostMessage($script:main, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $script:cur.WaitForExit(3000)) { return @('FAIL', 'did not exit within 3 s of WM_CLOSE') }
    $win = Test-Path (Join-Path $dir 'window.txt'); $ses = Test-Path (Join-Path $dir 'session.txt')
    if (@($missing).Count -eq 0 -and $win -and $ses) { @('PASS', 'first run wrote settings.ini, themes\dark.ini, themes\light.ini, icons\default\*.png; exit wrote window.txt, session.txt') }
    else { @('FAIL', "missing=$($missing -join ',') window.txt=$win session.txt=$ses") }
  } finally { $env:APPDATA = $appdata; Stop-App $script:cur; Remove-Item $fake -Recurse -Force -ErrorAction SilentlyContinue }
}

Run-Check 'cli-help' {
  $script:cur = Start-Process -FilePath $Exe -ArgumentList '--help' -PassThru
  $null = $script:cur.Handle
  $script:dlg = [IntPtr]::Zero
  Wait-Until { $script:dlg = [W]::FindOfProcess([uint32]$script:cur.Id, '#32770'); $script:dlg -ne [IntPtr]::Zero } 3000 | Out-Null
  if ($script:dlg -eq [IntPtr]::Zero) { return @('FAIL', 'no #32770 dialog within 3 s') }
  $title = Get-Title $script:dlg
  $null = [W]::PostMessage($script:dlg, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
  if ($script:cur.WaitForExit(3000)) { @('PASS', "dialog '$title' shown; process exited after it closed") }
  else { @('FAIL', "dialog '$title' shown; process still running 3 s after closing it") }
}

Run-Check 'placement' {
  $f = Join-Path $env:APPDATA 'rexplorer\window.txt'
  $prev = if (Test-Path $f) { (Get-Item $f).LastWriteTimeUtc } else { [datetime]::MinValue }
  if (-not (Launch 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
  Start-Sleep -Seconds 2
  $null = [W]::PostMessage($script:main, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
  if (-not $script:cur.WaitForExit(3000)) { return @('FAIL', 'did not exit within 3 s of WM_CLOSE') }
  $code = $script:cur.ExitCode
  if (-not (Test-Path $f)) { return @('FAIL', "exit=$code, window.txt missing") }
  $raw = (Get-Content $f -Raw).Trim()
  $tok = @($raw -split '[\s,;]+' | Where-Object { $_ })
  $fresh = (Get-Item $f).LastWriteTimeUtc -gt $prev
  $good = ($tok.Count -eq 5) -and (@($tok | Where-Object { $_ -notmatch '^-?\d+$' }).Count -eq 0)
  if ($code -eq 0 -and $good -and $fresh) { @('PASS', "exit=0 window.txt='$raw'") }
  else { @('FAIL', "exit=$code ints=$($tok.Count) wellformed=$good freshly written=$fresh content='$raw'") }
}
Run-Check 'tree' {
  try {
    if (-not (Launch-Fresh 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
    # The control creates its inner SysTreeView32 a moment after itself.
    $found = Wait-Until { $script:nstc = [W]::FindDescendant($script:main, 'NamespaceTreeControl'); $script:nstc -ne [IntPtr]::Zero -and [W]::FindDescendant($script:main, 'SysTreeView32') -ne [IntPtr]::Zero } 3000
    $tv = [W]::FindDescendant($script:main, 'SysTreeView32')
    if (-not $found -or $tv -eq [IntPtr]::Zero) { return @('FAIL', "NamespaceTreeControl found=$found SysTreeView32=$($tv -ne [IntPtr]::Zero)") }
    # Drag the splitter right of the tree 100 px with posted mouse messages (client x = screen x - tree's left).
    $list = Get-List $script:main
    # The tree is created once the first listing is on screen, then laid out.
    Wait-Until { ([W]::Left($list) - [W]::Left($script:nstc)) -gt 100 } 2000 | Out-Null
    $x0 = [W]::Left($list) - [W]::Left($script:nstc); $y = 200
    $lp = { param($x) [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF)) }
    $null = [W]::PostMessage($script:main, 0x201, [IntPtr]1, (& $lp ($x0 - 2)))
    $null = [W]::PostMessage($script:main, 0x200, [IntPtr]1, (& $lp ($x0 + 98)))
    $null = [W]::PostMessage($script:main, 0x202, [IntPtr]0, (& $lp ($x0 + 98)))
    Start-Sleep -Milliseconds 300
    $moved = ([W]::Left($list) - [W]::Left($script:nstc)) - $x0
    $how = Send-KeysOrCommand '^b' 23
    $hidden = Wait-Until { -not [W]::IsWindowVisible($script:nstc) } 2000
    $drop = [W]::GetProp($list, 'OleDropTargetInterface') -ne [IntPtr]::Zero
    $d = "tree present; splitter drag moved the list $moved px; Ctrl+B ($how) hid it=$hidden; list drop target registered=$drop"
    if ($moved -ge 90 -and $moved -le 115 -and $hidden -and $drop) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'preview' {
  $bench = (Join-Path $env:TEMP 'rexplorer-bench')
  $first = Join-Path $bench 'file000000.txt'
  # The bench files are empty; the first one gets a line so the preview has something to show.
  if ((Get-Item $first).Length -eq 0) { Set-Content -Path $first -Value 'rexplorer preview smoke' }
  try {
    if (-not (Launch-Fresh $bench)) { return @('FAIL', 'no main window within 5 s') }
    if (-not (Wait-Until { (Get-Count $script:main) -eq 100000 } 5000)) { return @('FAIL', "count=$(Get-Count $script:main)") }
    $how = Send-KeysOrCommand '%p' 24
    $pane = [W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexPreview', $null)
    $shown = Wait-Until { $pane -ne [IntPtr]::Zero -and [W]::IsWindowVisible($pane) } 2000
    if (-not $shown) { return @('FAIL', "Alt+P ($how): RexPreview present=$($pane -ne [IntPtr]::Zero) visible=$shown") }
    # Home selects file000000.txt; the pane updates 150 ms after the selection settles.
    $null = [W]::PostMessage((Get-List $script:main), 0x100, [IntPtr]0x24, [IntPtr]::Zero)
    $edit = [W]::FindWindowEx($pane, [IntPtr]::Zero, 'Edit', $null)
    $ok = Wait-Until { [int][W]::SendMessage($edit, 0x0E, [IntPtr]::Zero, [IntPtr]::Zero) -gt 0 } 3000
    $n = [int][W]::SendMessage($edit, 0x0E, [IntPtr]::Zero, [IntPtr]::Zero)
    if ($ok) { @('PASS', "Alt+P ($how) showed RexPreview; file000000.txt previewed, Edit holds $n chars") }
    else { @('FAIL', "Alt+P ($how) showed RexPreview; Edit text length=$n after selecting file000000.txt") }
  } finally { Remove-Fresh }
}

Run-Check 'views' {
  try {
    if (-not (Launch-Fresh 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
    $list = Get-List $script:main
    $view = { [int][W]::SendMessage($list, 0x108F, [IntPtr]::Zero, [IntPtr]::Zero) }  # LVM_GETVIEW
    $v0 = & $view
    $how = Send-KeysOrCommand '^+2' 26
    $icon = Wait-Until { (& $view) -eq 0 } 2000
    $ini = Get-Content (Join-Path $script:fake 'rexplorer\settings.ini') -Raw
    $saved = $ini -match '(?m)^view=icons\s*$'
    $null = Send-KeysOrCommand '^+1' 25
    $details = Wait-Until { (& $view) -eq 1 } 2000
    $d = "LVM_GETVIEW $v0 -> Ctrl+Shift+2 ($how) icon=$icon (profile view=icons saved=$saved) -> Ctrl+Shift+1 details=$details"
    if ($v0 -eq 1 -and $icon -and $saved -and $details) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'archive' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-rar-$PID"
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  $rar = Join-Path $dir 'smoke.rar'
  [IO.File]::WriteAllBytes($rar, [W]::Rar5())
  try {
    if (-not (Launch-Fresh $rar)) { return @('FAIL', 'no main window within 5 s') }
    $ok = Wait-Until { (Get-Title $script:main) -like '*smoke.rar*' -and (Get-Count $script:main) -eq 1 } 4000
    $t0 = Get-Title $script:main; $c0 = Get-Count $script:main
    if (-not $ok) { return @('FAIL', "title='$t0' count=$c0") }
    # Home + Enter opens the archive's 'docs' directory; Backspace returns to the archive root.
    $list = Get-List $script:main
    $null = [W]::PostMessage($list, 0x100, [IntPtr]0x24, [IntPtr]::Zero)
    $null = [W]::PostMessage($list, 0x100, [IntPtr]0x0D, [IntPtr]::Zero)
    $in = Wait-Until { (Get-Title $script:main) -like 'docs*' -and (Get-Count $script:main) -eq 1 } 2000
    $t1 = Get-Title $script:main
    $null = [W]::PostMessage($list, 0x100, [IntPtr]0x08, [IntPtr]::Zero)
    $out = Wait-Until { (Get-Title $script:main) -like '*smoke.rar*' } 2000
    $d = "'$t0' count=$c0; Enter -> '$t1' ($in); Backspace -> '$(Get-Title $script:main)' ($out)"
    if ($in -and $out) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh; Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Run-Check 'search' {
  $bench = (Join-Path $env:TEMP 'rexplorer-bench')
  try {
    if (-not (Launch-Fresh $bench)) { return @('FAIL', 'no main window within 5 s') }
    if (-not (Wait-Until { (Get-Count $script:main) -eq 100000 } 5000)) { return @('FAIL', "count=$(Get-Count $script:main)") }
    $box = [W]::VisibleEdit($script:main)
    if ($box -eq [IntPtr]::Zero) { return @('FAIL', 'no visible search Edit') }
    $null = [W]::SendText($box, 0x0C, [IntPtr]::Zero, 'file00012')
    $null = [W]::PostMessage($box, 0x100, [IntPtr]0x0D, [IntPtr]::Zero)
    Start-Sleep -Seconds 3
    $t = Get-Title $script:main; $c = Get-Count $script:main
    $d = "WM_SETTEXT 'file00012' + Enter: title='$t' count=$c"
    if ($t.StartsWith('Search') -and $c -ge 1 -and $c -le 100000) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'search-cli' {
  $bench = (Join-Path $env:TEMP 'rexplorer-bench')
  try {
    if (-not (Launch-FreshArgs "--search file00012 `"$bench`"")) { return @('FAIL', 'no main window within 5 s') }
    $ok = Wait-Until { (Get-Title $script:main).StartsWith('Search') -and (Get-Count $script:main) -ge 1 } 5000
    $d = "--search file00012: title='$(Get-Title $script:main)' count=$(Get-Count $script:main)"
    if ($ok) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}
exit 0
