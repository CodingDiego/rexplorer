# rexplorer smoke test. Windows PowerShell 5.1. Prints one PASS/FAIL/SKIP line per check, always exits 0.
# -Screenshots also writes docs\screenshots\dark.png, light.png and home.png (rendered with PrintWindow).
# -Only <regex> runs just the checks whose names match.
param([switch]$Screenshots, [string]$Only)
$ErrorActionPreference = 'Stop'
$Exe = Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing, Microsoft.VisualBasic, UIAutomationClient, UIAutomationTypes
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
  [DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int l, int t, int r, int b);
  [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr h, IntPtr rgn);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  // The direct child of `parent` with window class `cls` that answers SendMessage(msg, w) with >= 0.
  public static IntPtr ChildAnswering(IntPtr parent, string cls, uint msg, int w) {
    IntPtr h = IntPtr.Zero;
    while ((h = FindWindowEx(parent, h, cls, null)) != IntPtr.Zero) { if ((long)SendMessage(h, msg, (IntPtr)w, IntPtr.Zero) >= 0) return h; }
    return IntPtr.Zero;
  }
  // Region type of a window's region: 0 none (ERROR), 1 NULLREGION, 2 SIMPLEREGION, 3 COMPLEXREGION (rounded).
  public static int RegionType(IntPtr h) { IntPtr r = CreateRectRgn(0, 0, 0, 0); int t = GetWindowRgn(h, r); DeleteObject(r); return t; }
  public static int Width(IntPtr h) { RECT r; GetWindowRect(h, out r); return r.R - r.L; }
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
  [DllImport("shell32.dll", CharSet=CharSet.Unicode)] public static extern void SHAddToRecentDocs(uint flags, string path);
  [DllImport("dwmapi.dll", EntryPoint="DwmGetWindowAttribute")] static extern int DwmGetInt(IntPtr h, int attr, out int v, int size);
  // DWMWA_CLOAKED: the shell keeps hidden app frames "visible" but cloaked.
  public static bool Cloaked(IntPtr h) { int v; return DwmGetInt(h, 14, out v, 4) == 0 && v != 0; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MOUSEINPUT mi; }  // 40 bytes on x64, as SendInput checks
  [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
  static void Mouse(uint flags, int x, int y) {
    // Absolute coordinates over the virtual desktop, 0..65535.
    int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77), vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
    var i = new INPUT { type = 0, mi = new MOUSEINPUT { dx = (int)((x - vx) * 65535L / (vw - 1)), dy = (int)((y - vy) * 65535L / (vh - 1)), flags = flags | 0x8000 | 0x4000 | 0x0001 } };
    SendInput(1, new[] { i }, Marshal.SizeOf(typeof(INPUT)));
  }
  // A real mouse drag (SendInput): press at (x1, y1), `steps` moves over `ms` to (x2, y2), release.
  public static void Drag(int x1, int y1, int x2, int y2, int steps, int ms) {
    Mouse(0, x1, y1); System.Threading.Thread.Sleep(50);
    Mouse(0x0002, x1, y1);
    for (int k = 1; k <= steps; k++) { System.Threading.Thread.Sleep(ms / steps); Mouse(0, x1 + (x2 - x1) * k / steps, y1 + (y2 - y1) * k / steps); }
    System.Threading.Thread.Sleep(100);
    Mouse(0x0004, x2, y2);
  }
  [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
  [DllImport("kernel32.dll")] static extern IntPtr VirtualAllocEx(IntPtr p, IntPtr a, UIntPtr size, uint type, uint protect);
  [DllImport("kernel32.dll")] static extern bool VirtualFreeEx(IntPtr p, IntPtr a, UIntPtr size, uint type);
  [DllImport("kernel32.dll")] static extern bool WriteProcessMemory(IntPtr p, IntPtr a, int[] buf, UIntPtr n, out UIntPtr done);
  [DllImport("kernel32.dll")] static extern bool ReadProcessMemory(IntPtr p, IntPtr a, int[] buf, UIntPtr n, out UIntPtr done);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  // Row i's bounds in screen pixels: LVM_GETITEMRECT (LVIR_BOUNDS) with the RECT in the list's own process.
  public static RECT ItemRect(IntPtr list, int i) {
    uint pid; GetWindowThreadProcessId(list, out pid);
    IntPtr p = OpenProcess(0x0008 | 0x0010 | 0x0020, false, pid);
    IntPtr mem = VirtualAllocEx(p, IntPtr.Zero, (UIntPtr)16, 0x3000, 0x04);
    int[] buf = { 0, 0, 0, 0 }; UIntPtr n;
    WriteProcessMemory(p, mem, buf, (UIntPtr)16, out n);
    SendMessage(list, 0x100E, (IntPtr)i, mem);
    ReadProcessMemory(p, mem, buf, (UIntPtr)16, out n);
    VirtualFreeEx(p, mem, UIntPtr.Zero, 0x8000); CloseHandle(p);
    var a = new POINT { X = buf[0], Y = buf[1] }; ClientToScreen(list, ref a);
    return new RECT { L = a.X, T = a.Y, R = a.X + buf[2] - buf[0], B = a.Y + buf[3] - buf[1] };
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
# Every launch runs on a scratch %APPDATA%: the user's settings and window.txt stay untouched, and the single-instance
# hand-off (one instance per settings folder) never reaches a rexplorer the user has open.
$RealAppData = $env:APPDATA
$env:APPDATA = Join-Path $env:TEMP "rexplorer-smoke-profile-$PID"
New-Item -ItemType Directory -Path $env:APPDATA -Force | Out-Null
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
# PrintWindow of `h` (the window itself, nothing else on screen) as a Bitmap; the caller disposes it.
function Get-WindowBitmap($h) {
  $r = New-Object W+RECT; $null = [W]::GetWindowRect($h, [ref]$r)
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc(); $null = [W]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc); $g.Dispose()  # PW_RENDERFULLCONTENT
  $bmp
}
# `child`'s window rect relative to `parent`'s window rect.
function Get-RelativeRect($child, $parent) {
  $c = New-Object W+RECT; $p = New-Object W+RECT
  $null = [W]::GetWindowRect($child, [ref]$c); $null = [W]::GetWindowRect($parent, [ref]$p)
  @{ L = $c.L - $p.L; T = $c.T - $p.T; R = $c.R - $p.L; B = $c.B - $p.T }
}
function Stop-App($p) { if ($p) { try { if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit(2000) | Out-Null } } catch {} } }
function Get-Explorers { $r = @{}; foreach ($w in (New-Object -ComObject Shell.Application).Windows()) { try { $r[[int64]$w.HWND] = $w } catch {} }; $r }
function Close-NewExplorers($before) { foreach ($k in (Get-Explorers).GetEnumerator()) { if (-not $before.ContainsKey($k.Key)) { try { $k.Value.Quit() } catch {} } } }

# Runs one check: $body returns @(status, detail). Process in $script:cur is always killed.
function Run-Check($name, [scriptblock]$body) {
  if ($Only -and $name -notmatch $Only) { return }
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
# Sidebar rows: WM_APP+0x41 puts row i's target in the sidebar's window text (-1 past the last row), WM_APP+0x40
# answers row i's client top. Rows fill in from a worker thread.
# [NullString]: PowerShell passes $null to a string parameter as "", which only matches windows without text.
function Get-Sidebar { [W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexSidebar', [NullString]::Value) }
function Get-RowTargets($sb) {
  $out = @()
  for ($i = 0; $i -lt 200; $i++) { if ([int][W]::SendMessage($sb, 0x8041, [IntPtr]$i, [IntPtr]::Zero) -lt 0) { break }; $out += (Get-Title $sb) }
  , $out
}
# Index of the first row whose target is $target (case-insensitive), waiting up to $ms for it; -1 if none.
function Find-Row($sb, $target, $ms) {
  $script:rowIndex = -1
  $null = Wait-Until { $t = Get-RowTargets $sb; for ($i = 0; $i -lt $t.Count; $i++) { if ($t[$i] -eq $target) { $script:rowIndex = $i; return $true } }; $false } $ms
  $script:rowIndex
}
# Posts a click (or, with -Right, a right-click) in the middle of sidebar row $i.
function Click-Row($sb, $i, [switch]$Right) {
  $top = [int][W]::SendMessage($sb, 0x8040, [IntPtr]$i, [IntPtr]::Zero)
  $at = [IntPtr]((($top + 10) -shl 16) -bor [int]([W]::Width($sb) / 2))
  if ($Right) { $null = [W]::PostMessage($sb, 0x205, [IntPtr]::Zero, $at) }
  else { $null = [W]::PostMessage($sb, 0x201, [IntPtr]1, $at); $null = [W]::PostMessage($sb, 0x202, [IntPtr]0, $at) }
  $top
}
function Get-Status { Get-Title ([W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexStatus', [NullString]::Value)) }
$QuickAccess = 'shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}'
function Get-QuickAccess { @((New-Object -ComObject Shell.Application).NameSpace($QuickAccess).Items() | ForEach-Object { $_.Path }) }

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
    # While the new tab loads its title is the typed path; once loaded, the shell's name for it.
    $changed = Wait-Until { $n = Get-Title $script:main; $n -ne $orig -and $n -ne 'C:\Users - rexplorer' -and $n -ne 'rexplorer' } 3000
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
    # More > Folder tree (command 30) shows the tree instead of the sidebar.
    $null = [W]::PostMessage($script:main, 0x111, [IntPtr]30, [IntPtr]::Zero)
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
    $d = "More > Folder tree showed it; splitter drag moved the list $moved px; Ctrl+B ($how) hid it=$hidden; list drop target registered=$drop"
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

Run-Check 'look' {
  try {
    if (-not (Launch-Fresh 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
    $sb = [W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexSidebar', $null)
    if ($sb -eq [IntPtr]::Zero -or -not [W]::IsWindowVisible($sb)) { return @('FAIL', "RexSidebar present=$($sb -ne [IntPtr]::Zero), visible=false") }
    # The list card's corners are painted in the frame color (no window region): its corner pixels match the frame
    # beside the card, the card's inside differs.
    $lr = Get-RelativeRect (Get-List $script:main) $script:main
    $bmp = Get-WindowBitmap $script:main
    $px = { param($x, $y) '{0:X6}' -f ($bmp.GetPixel($x, $y).ToArgb() -band 0xFFFFFF) }
    $frame = & $px ($lr.L - 3) ($lr.T + 40); $tl = & $px $lr.L $lr.T; $br = & $px ($lr.R - 1) ($lr.B - 1); $inside = & $px ($lr.L + 40) ($lr.T + 40)
    $bmp.Dispose()
    $rgn = "corners TL=$tl BR=$br frame=$frame inside=$inside"
    $round = $tl -eq $frame -and $br -eq $frame -and $inside -ne $frame
    # Favorites come from Quick Access, which pins Downloads by default.
    $dl = (New-Object -ComObject Shell.Application).NameSpace('shell:Downloads').Self
    $i = Find-Row $sb $dl.Path 5000
    if ($i -lt 0) { return @('FAIL', "list $rgn; no sidebar row for $($dl.Path) after 5 s") }
    $want = $dl.Name
    $top = Click-Row $sb $i
    $went = Wait-Until { (Get-Title $script:main) -eq "$want - rexplorer" } 3000
    $t = Get-Title $script:main
    $how = Send-KeysOrCommand '^b' 23
    $hidden = Wait-Until { -not [W]::IsWindowVisible($sb) } 2000
    $d = "RexSidebar shown; list $rgn; click on Downloads row $i (y=$top) -> '$t' (want '$want'); Ctrl+B ($how) hid the sidebar=$hidden"
    if ($round -and $went -and $hidden) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'home' {
  $homeId = '::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}'
  try {
    # A first run without folders starts at Home (its name is localized: ask the shell).
    if (-not (Launch-Fresh $null)) { return @('FAIL', 'no main window within 5 s') }
    $want = (New-Object -ComObject Shell.Application).NameSpace("shell:$homeId").Self.Name
    $ok = Wait-Until { (Get-Title $script:main) -eq "$want - rexplorer" -and (Get-Count $script:main) -gt 0 } 3000
    $t = Get-Title $script:main; $c = Get-Count $script:main
    $sb = Get-Sidebar
    $null = Wait-Until { [int][W]::SendMessage($sb, 0x8041, [IntPtr]0, [IntPtr]::Zero) -ge 0 } 5000
    $first = Get-Title $sb
    $d = "title='$t' (want '$want - rexplorer') count=$c; first sidebar row target='$first'"
    if ($ok -and $first -eq $homeId) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'sidebar-shell' {
  $bin = '::{645FF040-5081-101B-9F08-00AA002F954E}'
  try {
    if (-not (Launch-Fresh 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    $sb = Get-Sidebar
    $c = Find-Row $sb 'C:\' 5000
    $t = Get-RowTargets $sb
    $b = [array]::IndexOf([string[]]$t, $bin)
    # Favorites (Quick Access) come before This PC.
    $d = "Recycle Bin row=$b, This PC C:\ row=$c, $($t.Count) rows"
    if ($b -gt 0 -and $c -gt $b) { @('PASS', $d) } else { @('FAIL', "$d; targets: $($t -join ' | ')") }
  } finally { Remove-Fresh }
}

Run-Check 'loading' {
  $red = '::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}'
  try {
    if (-not (Launch-Fresh 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
    $sb = Get-Sidebar
    $i = Find-Row $sb $red 5000
    if ($i -lt 0) { return @('FAIL', 'no Network row in the sidebar after 5 s') }
    $want = (New-Object -ComObject Shell.Application).NameSpace("shell:$red").Self.Name
    $null = Click-Row $sb $i
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $seen = $false; $t = ''; $st = ''
    while ($sw.ElapsedMilliseconds -lt 150) {
      $t = Get-Title $script:main; $st = Get-Status
      if ($t -eq "$want - rexplorer" -and $st -like '*Loading*') { $seen = $true; break }
      Start-Sleep -Milliseconds 5
    }
    $ms = $sw.ElapsedMilliseconds
    # Past 120 ms the list is emptied for the skeleton while the status still says Loading.
    $skeleton = Wait-Until { (Get-Count $script:main) -eq 0 -and (Get-Status) -like '*Loading*' } 600
    $d = "click on Network row $i -> title='$t' status='$st' after $ms ms; list emptied for the skeleton=$skeleton"
    if ($seen) { @('PASS', $d) }
    elseif ($t -eq "$want - rexplorer") { @('SKIP', "enumeration finished first: $d") }
    else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'pin' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-pin-$PID"
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  try {
    if (-not (Launch-FreshArgs "--select `"$dir`"")) { return @('FAIL', 'no main window within 5 s') }
    $selected = Wait-Until { [int][W]::SendMessage((Get-List $script:main), 0x1032, [IntPtr]::Zero, [IntPtr]::Zero) -ge 1 } 4000
    if (-not $selected) { return @('FAIL', "--select did not select $dir") }
    $how = Send-KeysOrCommand '^d' 40
    $pinned = Wait-Until { (Get-QuickAccess) -contains $dir } 4000
    if (-not $pinned) { return @('FAIL', "Ctrl+D ($how): $dir not in Quick Access") }
    # Unpin through the sidebar: right-click the folder's Favorites row, pick the popup's only entry.
    $sb = Get-Sidebar
    $i = Find-Row $sb $dir 4000
    if ($i -lt 0) { return @('FAIL', "Ctrl+D ($how) pinned it, but no sidebar row for it after 4 s") }
    $null = Click-Row $sb $i -Right
    $script:menu = [IntPtr]::Zero
    $null = Wait-Until { $script:menu = [W]::FindOfProcess([uint32]$script:cur.Id, '#32768'); $script:menu -ne [IntPtr]::Zero } 2000
    if ($script:menu -eq [IntPtr]::Zero) { return @('FAIL', "pinned; right-click on row $i showed no popup menu") }
    $null = [W]::PostMessage($script:menu, 0x100, [IntPtr]0x28, [IntPtr]::Zero)
    $null = [W]::PostMessage($script:menu, 0x100, [IntPtr]0x0D, [IntPtr]::Zero)
    $gone = Wait-Until { -not ((Get-QuickAccess) -contains $dir) } 4000
    $d = "Ctrl+D ($how) pinned $dir (in Quick Access, sidebar row $i); its Unpin from Home removed it=$gone"
    if ($gone) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally {
    # Leave Quick Access as found, whatever happened above.
    foreach ($it in (New-Object -ComObject Shell.Application).NameSpace($QuickAccess).Items()) { if ($it.Path -eq $dir) { try { $it.InvokeVerb('unpinfromhome') } catch {} } }
    Remove-Fresh; Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
  }
}

# The command bar is the RexToolbar that has a Cut button (command 42). WM_APP+0x44 answers a button's state
# (-1 none, 1 enabled, +2 checked), WM_APP+0x45 its center x.
function Get-CmdBar { [W]::ChildAnswering($script:main, 'RexToolbar', 0x8044, 42) }
function Get-ButtonState($bar, $cmd) { [int][W]::SendMessage($bar, 0x8044, [IntPtr]$cmd, [IntPtr]::Zero) }
function Click-Button($bar, $cmd) {
  $x = [int][W]::SendMessage($bar, 0x8045, [IntPtr]$cmd, [IntPtr]::Zero)
  $r = New-Object W+RECT; $null = [W]::GetClientRect($bar, [ref]$r)
  $at = [IntPtr]((([int]($r.B / 2)) -shl 16) -bor $x)
  $null = [W]::PostMessage($bar, 0x201, [IntPtr]1, $at); $null = [W]::PostMessage($bar, 0x202, [IntPtr]0, $at)
}

Run-Check 'commandbar' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-cmd-$PID"
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  Set-Content -Path (Join-Path $dir 'file000000.txt') -Value 'x'
  try {
    if (-not (Launch-Fresh $dir)) { return @('FAIL', 'no main window within 5 s') }
    if (-not (Wait-Until { (Get-Count $script:main) -eq 1 } 4000)) { return @('FAIL', "count=$(Get-Count $script:main)") }
    $bar = Get-CmdBar
    if ($bar -eq [IntPtr]::Zero) { return @('FAIL', 'no RexToolbar with a Cut button') }
    $cut0 = Get-ButtonState $bar 42
    # Home selects file000000.txt; the buttons follow the selection.
    $null = [W]::PostMessage((Get-List $script:main), 0x100, [IntPtr]0x24, [IntPtr]::Zero)
    $on = Wait-Until { @(42, 43, 45, 46 | Where-Object { ((Get-ButtonState $bar $_) -band 1) -eq 0 }).Count -eq 0 } 2000
    Click-Button $bar 43
    $paste = Wait-Until { ((Get-ButtonState $bar 44) -band 1) -eq 1 } 3000
    Click-Button $bar 44
    $copied = Wait-Until { (Get-Count $script:main) -eq 2 } 3000
    $d = "Cut with no selection=$cut0 (0 = disabled); after Home Cut/Copy/Rename/Delete enabled=$on; Copy click -> Paste enabled=$paste; Paste click -> count $(Get-Count $script:main) (want 2)"
    if ($cut0 -eq 0 -and $on -and $paste -and $copied) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh; Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Run-Check 'home-page' {
  try {
    if (-not (Launch-Fresh $null)) { return @('FAIL', 'no main window within 5 s') }
    $script:homePage = [IntPtr]::Zero
    $shown = Wait-Until { $script:homePage = [W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexHome', [NullString]::Value); $script:homePage -ne [IntPtr]::Zero -and [W]::IsWindowVisible($script:homePage) } 4000
    if (-not $shown) { return @('FAIL', "RexHome present=$($script:homePage -ne [IntPtr]::Zero), not visible after 4 s") }
    # WM_APP+0x52: tiles << 16 | rows.
    $n = [int64][W]::SendMessage($script:homePage, 0x8052, [IntPtr]::Zero, [IntPtr]::Zero)
    $tiles = $n -shr 16; $rows = $n -band 0xFFFF
    $listHidden = -not [W]::IsWindowVisible((Get-List $script:main))
    $sb = Get-Sidebar
    $i = Find-Row $sb 'C:\' 5000
    if ($i -lt 0) { return @('FAIL', "tiles=$tiles rows=$rows; no C:\ sidebar row") }
    $null = Click-Row $sb $i
    $back = Wait-Until { -not [W]::IsWindowVisible($script:homePage) -and [W]::IsWindowVisible((Get-List $script:main)) -and (Get-Count $script:main) -gt 0 } 4000
    $d = "RexHome visible with tiles=$tiles rows=$rows, ListView hidden=$listHidden; C:\ row -> list back, RexHome hidden=$back"
    if ($tiles -gt 0 -and $rows -ge 0 -and $listHidden -and $back) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'view-menu' {
  try {
    if (-not (Launch-Fresh 'C:\Windows')) { return @('FAIL', 'no main window within 5 s') }
    Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
    $bar = Get-CmdBar
    if ($bar -eq [IntPtr]::Zero) { return @('FAIL', 'no command bar') }
    Click-Button $bar 48
    $script:menu = [IntPtr]::Zero
    $open = Wait-Until { $script:menu = [W]::FindOfProcess([uint32]$script:cur.Id, '#32768'); $script:menu -ne [IntPtr]::Zero -and [W]::IsWindowVisible($script:menu) } 2000
    if (-not $open) { return @('FAIL', 'View click showed no popup menu') }
    $null = [W]::PostMessage($script:menu, 0x100, [IntPtr]0x1B, [IntPtr]::Zero)
    $closed = Wait-Until { $m = [W]::FindOfProcess([uint32]$script:cur.Id, '#32768'); $m -eq [IntPtr]::Zero -or -not [W]::IsWindowVisible($m) } 2000
    $alive = -not $script:cur.HasExited
    $d = "View click -> popup menu; Escape closed it=$closed; alive=$alive"
    if ($closed -and $alive) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

# Home's own hooks: WM_APP+0x53 selects the entry named by RexHome's window text (display or parsing name) and
# answers its position (-1 none); WM_APP+0x54 answers a rect packed as four int16 (left, top, right, bottom).
function Get-HomePage { [W]::FindWindowEx($script:main, [IntPtr]::Zero, 'RexHome', [NullString]::Value) }
function Select-HomeNamed($hp, $name) { $null = [W]::SendText($hp, 0x000C, [IntPtr]::Zero, $name); [int][W]::SendMessage($hp, 0x8053, [IntPtr]::Zero, [IntPtr]::Zero) }
function Get-HomeRect($hp, $i) {
  $v = [int64][W]::SendMessage($hp, 0x8054, [IntPtr]$i, [IntPtr]::Zero)
  if ($v -le 0) { return $v }
  $w = { param($k) $x = ($v -shr (16 * $k)) -band 0xFFFF; if ($x -ge 0x8000) { $x - 0x10000 } else { $x } }
  @{ L = (& $w 0); T = (& $w 1); R = (& $w 2); B = (& $w 3) }
}

Run-Check 'home-open' {
  # A recent file Home lists, opened with Enter. Its extension is registered for this check only (HKCU) with an open
  # command that writes a marker, so nothing visible opens. Under %LOCALAPPDATA%: Recent ignores %TEMP%.
  $dir = Join-Path $env:LOCALAPPDATA 'rexplorer-smoke'
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  $file = Join-Path $dir "home-open-$PID.rexsmoke"; $marker = Join-Path $dir "home-open-$PID.opened"
  Set-Content -Path $file -Value 'x'; Remove-Item $marker -ErrorAction SilentlyContinue
  $cls = 'HKCU:\Software\Classes'
  New-Item -Path "$cls\.rexsmoke" -Value 'rexplorer.smokefile' -Force | Out-Null
  New-Item -Path "$cls\rexplorer.smokefile\shell\open\command" -Value ('cmd.exe /c type nul > "' + $marker + '"') -Force | Out-Null
  $recent = Join-Path ([Environment]::GetFolderPath('Recent')) ("home-open-$PID.rexsmoke.lnk")
  try {
    [W]::SHAddToRecentDocs(3, $file)  # SHARD_PATHW
    if (-not (Launch-Fresh $null)) { return @('FAIL', 'no main window within 5 s') }
    $hp = [IntPtr]::Zero
    $null = Wait-Until { $script:hp = Get-HomePage; $script:hp -ne [IntPtr]::Zero -and [W]::IsWindowVisible($script:hp) } 4000
    $hp = $script:hp
    if ($hp -eq [IntPtr]::Zero) { return @('FAIL', 'no RexHome') }
    $script:at = -1
    $null = Wait-Until { $script:at = Select-HomeNamed $hp $file; $script:at -ge 0 } 5000
    if ($script:at -lt 0) { return @('FAIL', "Home does not list the recent file $file after 5 s") }
    $null = [W]::PostMessage($hp, 0x100, [IntPtr]0x0D, [IntPtr]::Zero)
    $opened = Wait-Until { Test-Path $marker } 5000
    $d = "recent $file is Home entry $($script:at); Enter opened it with its registered app=$opened"
    if ($opened) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally {
    Remove-Fresh
    # Off Home's list (its own "remove" verb), then the shortcut, the association and the files.
    try { foreach ($it in (New-Object -ComObject Shell.Application).NameSpace('shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}').Items()) { if ($it.Path -eq $file) { $it.InvokeVerb('remove') } } } catch {}
    # Windows also adds the file's folder to Recent.
    Remove-Item $recent, (Join-Path ([Environment]::GetFolderPath('Recent')) 'rexplorer-smoke.lnk') -ErrorAction SilentlyContinue
    Remove-Item "$cls\.rexsmoke", "$cls\rexplorer.smokefile" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $file, $marker -ErrorAction SilentlyContinue
    if (-not (Get-ChildItem $dir -Force)) { Remove-Item $dir -ErrorAction SilentlyContinue }
  }
}

Run-Check 'remove-bookmark' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-bookmark-$PID"
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  try {
    # A scratch profile with one bookmark, removed through the sidebar's popup.
    $script:fake = Join-Path $env:TEMP "rexplorer-smoke-appdata-$PID"
    Remove-Item $script:fake -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path (Join-Path $script:fake 'rexplorer') -Force | Out-Null
    $ini = Join-Path $script:fake 'rexplorer\settings.ini'
    Set-Content -Path $ini -Value "[general]`r`nprofile=default`r`n[profile default]`r`nbookmark.Repo=$dir`r`n"
    $appdata = $env:APPDATA
    try { $env:APPDATA = $script:fake; $up = Launch 'C:\Windows' } finally { $env:APPDATA = $appdata }
    if (-not $up) { return @('FAIL', 'no main window within 5 s') }
    $sb = Get-Sidebar
    $i = Find-Row $sb $dir 5000
    if ($i -lt 0) { return @('FAIL', "no sidebar row for the bookmark $dir") }
    $null = Click-Row $sb $i -Right
    $script:menu = [IntPtr]::Zero
    $null = Wait-Until { $script:menu = [W]::FindOfProcess([uint32]$script:cur.Id, '#32768'); $script:menu -ne [IntPtr]::Zero } 2000
    if ($script:menu -eq [IntPtr]::Zero) { return @('FAIL', "right-click on row $i showed no popup menu") }
    $null = [W]::PostMessage($script:menu, 0x100, [IntPtr]0x28, [IntPtr]::Zero)
    $null = [W]::PostMessage($script:menu, 0x100, [IntPtr]0x0D, [IntPtr]::Zero)
    $rowGone = Wait-Until { (Get-RowTargets $sb) -notcontains $dir } 3000
    $lineGone = -not (Select-String -Path $ini -Pattern '^bookmark\.Repo=' -Quiet)
    $d = "bookmark row $i; Remove bookmark -> row gone=$rowGone, settings.ini line gone=$lineGone"
    if ($rowGone -and $lineGone) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh; Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Run-Check 'share' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-share-$PID"
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  $file = Join-Path $dir 'share-me.txt'; Set-Content -Path $file -Value 'x'
  $sheet = [IntPtr]::Zero
  try {
    if (-not (Launch-FreshArgs "--select `"$file`"")) { return @('FAIL', 'no main window within 5 s') }
    $bar = Get-CmdBar
    $off = Get-ButtonState $bar 49
    $on = Wait-Until { ((Get-ButtonState $bar 49) -band 1) -eq 1 } 4000
    if (-not $on) { return @('FAIL', "Share state=$(Get-ButtonState $bar 49) with $file selected") }
    # The share sheet only opens over the foreground window.
    if (-not (Set-Foreground $script:main)) { return @('SKIP', 'could not activate') }
    # The sheet is a top-level window created by the click that takes the foreground (owned by the shell; its title
    # is localized, e.g. "Compartir"). Windows that existed before do not count.
    $before = @{}; $null = [W]::EnumWindows({ param($h, $l) $before[[int64]$h] = 1; $true }, [IntPtr]::Zero)
    Click-Button $bar 49
    $script:sheet = [IntPtr]::Zero
    $null = Wait-Until { $f = [W]::GetForegroundWindow(); if ($f -ne $script:main -and -not $before.ContainsKey([int64]$f) -and -not [W]::Cloaked($f)) { $script:sheet = $f }; $script:sheet -ne [IntPtr]::Zero } 4000
    $sheet = $script:sheet
    if ($sheet -eq [IntPtr]::Zero) {
      if ($before.ContainsKey([int64][W]::GetForegroundWindow())) { return @('SKIP', 'focus moved to another window during the check') }
      return @('FAIL', 'Share click: no new window came to the foreground within 4 s')
    }
    $cls = New-Object System.Text.StringBuilder 128; $null = [W]::GetClassName($sheet, $cls, 128)
    [uint32]$owner = 0; $null = [W]::GetWindowThreadProcessId($sheet, [ref]$owner)
    $title = "$(Get-Title $sheet)' class $cls of $((Get-Process -Id $owner -ErrorAction SilentlyContinue).ProcessName)"
    [System.Windows.Forms.SendKeys]::SendWait('{ESC}')
    $closed = Wait-Until { [W]::GetForegroundWindow() -ne $sheet } 2000
    @('PASS', "Share enabled with a file selected; click -> share sheet '$title' in the foreground; Escape closed it=$closed")
  } finally {
    if ($sheet -ne [IntPtr]::Zero -and [W]::GetForegroundWindow() -eq $sheet) { [System.Windows.Forms.SendKeys]::SendWait('{ESC}') }
    Remove-Fresh; Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
  }
}

Run-Check 'access-denied' {
  $sv = 'C:\System Volume Information'
  try {
    if (-not (Launch-Fresh $sv)) { return @('FAIL', 'no main window within 5 s') }
    $ok = Wait-Until { (Get-Status) -like 'Access denied:*' } 3000
    $d = "status='$(Get-Status)' count=$(Get-Count $script:main)"
    if ($ok) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Remove-Fresh }
}

Run-Check 'crash-recovery' {
  $script:rec = $null
  try {
    $env:REXPLORER_CRASH_AFTER_MS = '1500'
    try { $up = Launch-Fresh 'C:\Windows' } finally { Remove-Item Env:REXPLORER_CRASH_AFTER_MS -ErrorAction SilentlyContinue }
    if (-not $up) { return @('FAIL', 'no main window within 5 s') }
    $first = $script:cur.Id
    # The crash handler relaunches the exe with --recover on the folder it was showing.
    $null = Wait-Until {
      $p = Get-CimInstance Win32_Process -Filter "Name='rexplorer.exe'" | Where-Object { $_.ProcessId -ne $first -and $_.CommandLine -like '*--recover*' } | Select-Object -First 1
      if ($p) { $script:rec = Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue }
      $null -ne $script:rec
    } 6000
    if (-not $script:rec) { return @('FAIL', "no rexplorer.exe --recover within 6 s; first exited=$($script:cur.HasExited)") }
    $script:recMain = [IntPtr]::Zero
    $titled = Wait-Until { $script:recMain = [W]::FindOfProcess([uint32]$script:rec.Id, 'RexplorerMain'); $script:recMain -ne [IntPtr]::Zero -and (Get-Title $script:recMain) -eq 'Windows - rexplorer' } 4000
    $t = if ($script:recMain -ne [IntPtr]::Zero) { Get-Title $script:recMain } else { '' }
    $d = "crashed pid $first exited=$($script:cur.HasExited); recovery pid $($script:rec.Id) titled '$t'"
    if ($titled) { @('PASS', $d) } else { @('FAIL', $d) }
  } finally { Stop-App $script:rec; Remove-Fresh }
}

Run-Check 'dnd' {
  $dir = Join-Path $env:TEMP "rexplorer-smoke-dnd-$PID"
  New-Item -ItemType Directory -Path (Join-Path $dir 'target') -Force | Out-Null
  $file = Join-Path $dir 'drag-me.txt'; $moved = Join-Path $dir 'target\drag-me.txt'
  Set-Content -Path $file -Value 'x'
  $cursor = New-Object W+POINT; $null = [W]::GetCursorPos([ref]$cursor)
  try {
    if (-not (Launch-Fresh $dir)) { return @('FAIL', 'no main window within 5 s') }
    if (-not (Wait-Until { (Get-Count $script:main) -eq 2 } 4000)) { return @('FAIL', "count=$(Get-Count $script:main)") }
    if (-not (Set-Foreground $script:main)) { return @('SKIP', 'could not activate') }
    # Row rects with LVM_GETITEMRECT through a buffer in the app's process (UI Automation sees no rows in this
    # list). Folders sort first: row 0 is the folder "target", row 1 the file.
    $list = Get-List $script:main
    $dst = [W]::ItemRect($list, 0); $src = [W]::ItemRect($list, 1)
    if ($src.B -le $src.T -or $dst.B -le $dst.T) { return @('FAIL', 'LVM_GETITEMRECT gave no row rects') }
    $x1 = $src.L + 40; $y1 = [int](($src.T + $src.B) / 2); $x2 = $dst.L + 40; $y2 = [int](($dst.T + $dst.B) / 2)
    [W]::Drag($x1, $y1, $x2, $y2, 8, 400)
    $ok = Wait-Until { (Test-Path $moved) -and -not (Test-Path $file) } 3000
    $at = New-Object W+POINT; $null = [W]::GetCursorPos([ref]$at)
    $d = "SendInput drag of drag-me.txt ($x1,$y1) onto the folder row target ($x2,$y2): moved into it=$ok (cursor ended at $($at.X),$($at.Y), selected rows $([int][W]::SendMessage($list, 0x1032, [IntPtr]::Zero, [IntPtr]::Zero)), foreground ours=$([W]::GetForegroundWindow() -eq $script:main))"
    if ($ok) { @('PASS', $d) }
    elseif ([W]::GetForegroundWindow() -ne $script:main) { @('SKIP', "focus lost during the drag; $d") }
    else { @('FAIL', $d) }
  } finally {
    $null = [W]::SetCursorPos($cursor.X, $cursor.Y)
    Remove-Fresh
    if (Test-Path $moved) { Move-Item $moved $file -Force -ErrorAction SilentlyContinue }
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
  }
}

# Window screenshots for the README: dark on C:\Windows (details, a row selected), light on the wallpapers with
# the preview pane, dark on Home. PrintWindow renders the window itself, so nothing else on screen is captured.
# Pixelates a rect of `bmp`: down to 1/12 and back up, nearest neighbor.
function Hide-Rect($bmp, $x, $y, $w, $h) {
  $x = [Math]::Max(0, $x); $y = [Math]::Max(0, $y); $w = [Math]::Min($w, $bmp.Width - $x); $h = [Math]::Min($h, $bmp.Height - $y)
  if ($w -le 0 -or $h -le 0) { return }
  $small = New-Object System.Drawing.Bitmap ([Math]::Max(1, [int]($w / 12))), ([Math]::Max(1, [int]($h / 12)))
  $g = [System.Drawing.Graphics]::FromImage($small); $g.InterpolationMode = 'NearestNeighbor'
  $g.DrawImage($bmp, (New-Object System.Drawing.Rectangle 0, 0, $small.Width, $small.Height), (New-Object System.Drawing.Rectangle $x, $y, $w, $h), 'Pixel'); $g.Dispose()
  $g = [System.Drawing.Graphics]::FromImage($bmp); $g.InterpolationMode = 'NearestNeighbor'; $g.PixelOffsetMode = 'Half'
  $g.DrawImage($small, (New-Object System.Drawing.Rectangle $x, $y, $w, $h), (New-Object System.Drawing.Rectangle 0, 0, $small.Width, $small.Height), 'Pixel'); $g.Dispose()
  $small.Dispose()
}
# Home's recent rows and the tile subtitles that are paths (not "Stored locally"), pixelated in `bmp`.
function Hide-HomeText($bmp) {
  $hp = Get-HomePage
  if ($hp -eq [IntPtr]::Zero -or -not [W]::IsWindowVisible($hp)) { return 0 }
  $o = Get-RelativeRect $hp $script:main
  $n = 0
  $rects = @(Get-HomeRect $hp 0xFFFF)
  for ($i = 0; $i -lt 200; $i++) { $r = Get-HomeRect $hp $i; if ($r -is [hashtable]) { $rects += $r } elseif ($r -lt 0) { break } }
  foreach ($r in $rects) { if ($r -is [hashtable]) { Hide-Rect $bmp ($o.L + $r.L) ($o.T + $r.T) ($r.R - $r.L) ($r.B - $r.T); $n++ } }
  $n
}
function Save-Shot($theme, $folder, $preview, $file, $place) {
  $script:fake = Join-Path $env:TEMP "rexplorer-smoke-appdata-$PID"
  Remove-Item $script:fake -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path (Join-Path $script:fake 'rexplorer') -Force | Out-Null
  $ini = "[general]`r`nprofile=default`r`n[profile default]`r`ntheme=$theme`r`nshow_preview=$preview`r`n"
  Set-Content -Path (Join-Path $script:fake 'rexplorer\settings.ini') -Value $ini
  if ($place) { Set-Content -Path (Join-Path $script:fake 'rexplorer\window.txt') -Value $place }
  $appdata = $env:APPDATA
  try { $env:APPDATA = $script:fake; $null = Launch $folder } finally { $env:APPDATA = $appdata }
  Wait-Until { (Get-Count $script:main) -ge 1 } 3000 | Out-Null
  $null = Set-Foreground $script:main
  $list = Get-List $script:main
  foreach ($k in 0x24, 0x28, 0x28) { $null = [W]::PostMessage($list, 0x100, [IntPtr]$k, [IntPtr]::Zero) }
  Start-Sleep -Milliseconds 2000
  $bmp = Get-WindowBitmap $script:main
  $hidden = Hide-HomeText $bmp
  $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
  $size = "$($bmp.Width)x$($bmp.Height)"; $bmp.Dispose()
  @('PASS', "$file ($size; $hidden areas pixelated)")
}
if ($Screenshots) {
  $shots = Join-Path (Split-Path $PSScriptRoot -Parent) 'docs\screenshots'
  New-Item -ItemType Directory -Path $shots -Force | Out-Null
  Run-Check 'screenshot-dark' { try { Save-Shot 'dark' 'C:\Windows' 'false' (Join-Path $shots 'dark.png') } finally { Remove-Fresh } }
  Run-Check 'screenshot-light' { try { Save-Shot 'light' 'C:\Windows\Web\Wallpaper\Windows' 'true' (Join-Path $shots 'light.png') } finally { Remove-Fresh } }
  Run-Check 'screenshot-home' { try { Save-Shot 'dark' $null 'false' (Join-Path $shots 'home.png') '1 40 40 1480 1040' } finally { Remove-Fresh } }
}
$scratch = $env:APPDATA; $env:APPDATA = $RealAppData
Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue
exit 0
