# rexplorer vs Windows Explorer folder windows, same event for both: the clock stops when the folder's rows are on
# screen (rexplorer: its list count, which is set together with the rows; Explorer: ink in its view's row slots, since
# its status bar count lags the rows). Windows PowerShell 5.1.
# Prints a markdown table and writes it to scripts\bench-results.md. Runs serially; each case is 1 warm-up + -Runs measured.
#   -Runs <n>      measured runs per case (default 7)
#   -ShotDir <d>   also saves one PrintWindow capture per program and case (the first measured run, taken once the
#                  folder is fully listed) to check the window reached the right folder
#   -KeepFiles     keep the generated folders under -Work
# Touches only what it starts: rexplorer.exe runs on a scratch %APPDATA% (never joins your instance, never writes
# the real one) and is killed by pid; Explorer windows opened here are closed with WM_CLOSE by handle. The
# explorer.exe that is your desktop shell is never touched.
param([int]$Runs = 7, [string]$ShotDir, [switch]$KeepFiles, [string]$Work = (Join-Path $env:TEMP 'rexplorer-bench-vs-explorer'))
$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
$Root = Split-Path $PSScriptRoot -Parent
$Exe = Join-Path $Root 'target\release\rexplorer.exe'
$ExplorerExe = Join-Path $env:SystemRoot 'explorer.exe'
$Out = Join-Path $PSScriptRoot 'bench-results.md'
if (-not (Test-Path $Exe)) { throw "build first: cargo build --release ($Exe)" }
Add-Type -AssemblyName System.Drawing, UIAutomationClient, UIAutomationTypes
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class B {
  public delegate bool EP(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EP f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string cls, string t);
  [DllImport("user32.dll")] static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern int GetDlgCtrlID(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  // Visible top-level windows of class `cls` (any process).
  public static long[] ByClass(string cls) {
    var r = new List<long>();
    EnumWindows((h, l) => { var sb = new StringBuilder(64); GetClassName(h, sb, 64); if (sb.ToString() == cls && IsWindowVisible(h)) r.Add((long)h); return true; }, IntPtr.Zero);
    return r.ToArray();
  }
  // First visible top-level window of process `pid`.
  public static long OfProcess(uint pid) {
    long found = 0;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) { found = (long)h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static uint Pid(long h) { uint p; GetWindowThreadProcessId((IntPtr)h, out p); return p; }
  public static bool Alive(long h) { return IsWindow((IntPtr)h); }
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr p, EP f, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
  // First descendant window of `root` with class `cls`, 0 if none.
  public static long Descendant(long root, string cls) {
    long found = 0;
    EnumChildWindows((IntPtr)root, (h, l) => { var sb = new StringBuilder(64); GetClassName(h, sb, 64); if (sb.ToString() == cls) { found = (long)h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static int Dpi(long h) { return (int)GetDpiForWindow((IntPtr)h); }
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  // Pixels unlike the dominant colour (text, icons) in a band of `view`, read from a PrintWindow capture of `top`:
  // `skip` px below the view's top, `height` tall, from `left` px in to 40 % of the view's width. Both rects are read
  // right before the capture, since the window may still be moving to its saved size.
  public static int Ink(long top, long view, int left, int skip, int height) {
    RECT w, v; GetWindowRect((IntPtr)view, out v); GetWindowRect((IntPtr)top, out w);
    // While Explorer still lays the window out, the view's rect sits at the client area's top although its pixels
    // are drawn below the chrome; a band taken then would read the column header.
    POINT o = new POINT(); ClientToScreen((IntPtr)top, ref o);
    if (v.T <= o.Y) return -1;
    int l = Math.Max(v.L + left, w.L), t = Math.Max(v.T + skip, w.T), r = Math.Min(v.L + (v.R - v.L) * 2 / 5, w.R), b = Math.Min(v.T + skip + height, w.B);
    if (r <= l || b <= t) return -1;
    using (var bmp = new System.Drawing.Bitmap(w.R - w.L, w.B - w.T))
    using (var g = System.Drawing.Graphics.FromImage(bmp)) {
      IntPtr dc = g.GetHdc(); PrintWindow((IntPtr)top, dc, 2); g.ReleaseHdc(dc);
      int bw = r - l, bh = b - t;
      var data = bmp.LockBits(new System.Drawing.Rectangle(l - w.L, t - w.T, bw, bh), System.Drawing.Imaging.ImageLockMode.ReadOnly, System.Drawing.Imaging.PixelFormat.Format32bppArgb);
      var px = new int[bw * bh];
      for (int y = 0; y < bh; y++) Marshal.Copy((IntPtr)((long)data.Scan0 + y * data.Stride), px, y * bw, bw);
      bmp.UnlockBits(data);
      var hist = new Dictionary<int, int>();
      foreach (int c in px) { int n; hist.TryGetValue(c & 0xFFFFFF, out n); hist[c & 0xFFFFFF] = n + 1; }
      int bg = 0, best = -1; foreach (var kv in hist) if (kv.Value > best) { best = kv.Value; bg = kv.Key; }
      int br = (bg >> 16) & 255, bgc = (bg >> 8) & 255, bb = bg & 255, ink = 0;
      foreach (int c in px) if (Math.Abs(((c >> 16) & 255) - br) + Math.Abs(((c >> 8) & 255) - bgc) + Math.Abs((c & 255) - bb) > 120) ink++;
      return ink;
    }
  }
  public static long Child(long p, string cls) { return (long)FindWindowEx((IntPtr)p, IntPtr.Zero, cls, null); }
  // Direct child of `p` with class `cls` and control id `id`.
  public static long ChildById(long p, string cls, int id) {
    IntPtr h = IntPtr.Zero;
    while ((h = FindWindowEx((IntPtr)p, h, cls, null)) != IntPtr.Zero) { if (GetDlgCtrlID(h) == id) return (long)h; }
    return 0;
  }
  // LVM_GETITEMCOUNT
  public static int Count(long list) { return (int)SendMessage((IntPtr)list, 0x1004, IntPtr.Zero, IntPtr.Zero); }
  public static void SetText(long h, string t) { SendText((IntPtr)h, 0x000C, IntPtr.Zero, t); }
  public static void Command(long h, int id) { PostMessage((IntPtr)h, 0x0111, (IntPtr)(id | (1 << 16)), IntPtr.Zero); }
  public static void Close(long h) { PostMessage((IntPtr)h, 0x0010, IntPtr.Zero, IntPtr.Zero); }
  // PW_RENDERFULLCONTENT capture of the window itself.
  public static void Shot(long h, string path) {
    RECT r; GetWindowRect((IntPtr)h, out r);
    using (var bmp = new System.Drawing.Bitmap(r.R - r.L, r.B - r.T))
    using (var g = System.Drawing.Graphics.FromImage(bmp)) {
      IntPtr dc = g.GetHdc(); PrintWindow((IntPtr)h, dc, 2); g.ReleaseHdc(dc);
      bmp.Save(path, System.Drawing.Imaging.ImageFormat.Png);
    }
  }
}
'@

# Window rects in physical pixels, like the PrintWindow bitmaps the row bands are read from.
[void][B]::SetProcessDPIAware()

# ---- helpers ----------------------------------------------------------------------------------------
function New-Files($dir, $n) {
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  1..$n | ForEach-Object { [IO.File]::WriteAllText("$dir\file$_.txt", 'x') }
}
# Private working set in MB of every process named `name` ("explorer"), by pid.
function Get-Pws($name) {
  $r = @{}
  foreach ($p in (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "Name LIKE '$name%'")) { $r[[int]$p.IDProcess] = $p.WorkingSetPrivate / 1MB }
  $r
}
$cGroup = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::AutomationIdProperty), 'System.StatusBarViewItemCount'
$cValue = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::AutomationIdProperty), 'PropertyValue'
# The item count Explorer's status bar shows ("5.000 elementos"): digits of the leading number, so no shell text is
# compared. The UIA element is looked up once per window and then read directly (a read is a few ms).
function Read-ExplorerCount($st, $hwnd) {
  try {
    if (-not $st.el) {
      $g = [Windows.Automation.AutomationElement]::FromHandle([IntPtr]$hwnd).FindFirst('Descendants', $cGroup)
      if ($g) { $st.el = $g.FindFirst('Descendants', $cValue) }
    }
    if ($st.el -and $st.el.Current.Name -match '^[\d.,\s ]+') { return [int]($matches[0] -replace '\D', '') }
  } catch { $st.el = $null }
  -1
}
function Wait-Explorer($hwnd, $expected, $sw, $limitMs) {
  $st = @{ el = $null }
  while ($sw.ElapsedMilliseconds -lt $limitMs) { if ((Read-ExplorerCount $st $hwnd) -eq $expected) { return $sw.Elapsed.TotalMilliseconds } }
  -1
}
# Explorer's rows on screen: ink in the row slots of the folder view (SHELLDLL_DefView) from 110 px below its top,
# which is below the column header, right of the navigation pane and left of the centred "Working on it..." text.
# Explorer creates a new view window per folder, so `notView` (the previous one) is skipped when navigating.
# Returns ms and the view found.
function Wait-ExplorerRows($hwnd, $sw, $limitMs, $notView) {
  $dpi = [B]::Dpi($hwnd); $left = [int](50 * $dpi / 96); $skip = [int](110 * $dpi / 96); $height = $skip
  while ($sw.ElapsedMilliseconds -lt $limitMs) {
    $view = [B]::Descendant($hwnd, 'SHELLDLL_DefView')
    if ($view -eq 0 -or $view -eq $notView) { continue }
    if ([B]::Ink($hwnd, $view, $left, $skip, $height) -gt 150) { return @{ ms = $sw.Elapsed.TotalMilliseconds; view = $view } }
  }
  @{ ms = -1; view = $notView }
}
function Save-Shot($hwnd, $name) { if ($ShotDir) { New-Item -ItemType Directory -Path $ShotDir -Force | Out-Null; [B]::Shot($hwnd, (Join-Path $ShotDir "$name.png")) } }

$RexAppData = Join-Path $env:TEMP "rexplorer-bench-appdata-$PID"
function Start-Rex($folder) {
  $psi = New-Object Diagnostics.ProcessStartInfo $Exe
  $psi.Arguments = '"' + $folder + '"'; $psi.UseShellExecute = $false
  $psi.EnvironmentVariables['APPDATA'] = $RexAppData
  $psi
}
# One rexplorer open: process start -> visible top-level window whose list holds `expected` rows. Returns ms and MB.
function Measure-RexOpen($folder, $expected, $shot) {
  $psi = Start-Rex $folder
  $sw = [Diagnostics.Stopwatch]::StartNew(); $p = [Diagnostics.Process]::Start($psi)
  try {
    $main = 0; $list = 0; $ms = -1; $win = -1
    while ($sw.ElapsedMilliseconds -lt 30000) {
      if ($main -eq 0) { $main = [B]::OfProcess($p.Id); if ($main -ne 0) { $win = $sw.Elapsed.TotalMilliseconds } }
      elseif ($list -eq 0) { $list = [B]::Child($main, 'SysListView32') }
      elseif ([B]::Count($list) -eq $expected) { $ms = $sw.Elapsed.TotalMilliseconds; break }
    }
    if ($ms -ge 0 -and $shot) { Save-Shot $main $shot }
    Start-Sleep -Milliseconds 1500
    $mb = -1; if ($ms -ge 0) { $mb = (Get-Pws 'rexplorer')[$p.Id] }
    @{ ms = $ms; mb = $mb; win = $win }
  } finally { try { $p.Kill(); $p.WaitForExit(3000) | Out-Null } catch {} }
}
# One Explorer open: explorer.exe <folder> -> a new visible CabinetWClass window with rows on screen (then its status
# bar must reach `expected` items, untimed).
# Memory is the private working set of the explorer.exe that owns the window. When that process is new (here every
# folder window gets its own /factory host) it is the whole process; when it existed before the open (a window
# hosted by the desktop process) it is the growth over the open. `fresh` says which.
function Measure-ExplorerOpen($folder, $expected, $shot) {
  $pre = Get-Pws 'explorer'; $known = [B]::ByClass('CabinetWClass')
  $psi = New-Object Diagnostics.ProcessStartInfo $ExplorerExe
  $psi.Arguments = '"' + $folder + '"'; $psi.UseShellExecute = $false
  $sw = [Diagnostics.Stopwatch]::StartNew(); $null = [Diagnostics.Process]::Start($psi)
  $h = 0
  while ($h -eq 0 -and $sw.ElapsedMilliseconds -lt 30000) { foreach ($w in [B]::ByClass('CabinetWClass')) { if ($known -notcontains $w) { $h = $w; break } } }
  if ($h -eq 0) { return @{ ms = -1; mb = -1; hwnd = 0; win = -1; fresh = $false } }
  $win = $sw.Elapsed.TotalMilliseconds
  $ms = (Wait-ExplorerRows $h $sw 30000 0).ms
  # The status bar count arrives after the rows; it is not timed, it confirms the window shows the whole folder.
  if ($ms -ge 0 -and (Wait-Explorer $h $expected ([Diagnostics.Stopwatch]::StartNew()) 30000) -lt 0) { $ms = -1 }
  if ($ms -ge 0 -and $shot) { Save-Shot $h $shot }
  Start-Sleep -Milliseconds 1500
  $mb = -1; $owner = [int][B]::Pid($h); $fresh = -not $pre.ContainsKey($owner)
  if ($ms -ge 0) { $post = Get-Pws 'explorer'; $base = 0; if (-not $fresh) { $base = $pre[$owner] }; $mb = $post[$owner] - $base }
  @{ ms = $ms; mb = $mb; hwnd = $h; win = $win; fresh = $fresh }
}
function Close-Explorer($h) {
  if ($h -eq 0) { return }
  [B]::Close($h); $sw = [Diagnostics.Stopwatch]::StartNew()
  while ([B]::Alive($h) -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 50 }
}

function Stat($v) {
  $v = @($v | Where-Object { $_ -ge 0 } | Sort-Object)
  if ($v.Count -eq 0) { return @{ n = 0 } }
  $m = if ($v.Count % 2) { $v[($v.Count - 1) / 2] } else { ($v[$v.Count / 2 - 1] + $v[$v.Count / 2]) / 2 }
  @{ n = $v.Count; med = $m; min = $v[0]; max = $v[-1] }
}
function Cell($s, $fmt) { if ($s.n -eq 0) { 'failed' } else { '{0} ({1}-{2})' -f ($s.med.ToString($fmt)), ($s.min.ToString($fmt)), ($s.max.ToString($fmt)) } }
function Ratio($a, $b) { if ($a.n -eq 0 -or $b.n -eq 0 -or $b.med -le 0) { '-' } else { '{0:N2}x' -f ($a.med / $b.med) } }

# ---- run --------------------------------------------------------------------------------------------
$cases = @(
  @{ key = 'small'; dir = 'small-200'; n = 200 },
  @{ key = 'mid'; dir = 'files-1000'; n = 1000 },
  @{ key = 'big'; dir = 'files-20000'; n = 20000 })
$navN = 5000
$res = @{}
$expFresh = 0; $expOpens = 0
$openWindows = New-Object Collections.ArrayList
try {
  New-Item -ItemType Directory -Path $Work -Force | Out-Null
  New-Item -ItemType Directory -Path $RexAppData -Force | Out-Null
  foreach ($c in $cases) { Write-Host "creating $($c.n) files ..."; New-Files (Join-Path $Work $c.dir) $c.n }
  Write-Host "creating $navN files ..."; New-Files (Join-Path $Work "files-$navN") $navN

  # rexplorer's first run writes settings and the starter icons; let that finish so the measured runs see a
  # settled profile, as a user's would.
  Write-Host 'rexplorer first run on the scratch profile ...'
  $marker = Join-Path $RexAppData 'rexplorer\icons\default\folder.png'
  $first = [Diagnostics.Process]::Start((Start-Rex (Join-Path $Work $cases[0].dir)))
  try { for ($i = 0; $i -lt 20 -and -not (Test-Path $marker); $i++) { Start-Sleep -Milliseconds 500 } } finally { try { $first.Kill(); $first.WaitForExit(3000) | Out-Null } catch {} }
  Start-Sleep -Milliseconds 700

  foreach ($c in $cases) {
    $folder = Join-Path $Work $c.dir
    $r = @{ rex = @(); rexMb = @(); exp = @(); expMb = @(); rexWin = @(); expWin = @() }
    Write-Host "open $($c.n): warm-up + $Runs runs"
    for ($i = 0; $i -le $Runs; $i++) {
      $shot = $null; if ($i -eq 1) { $shot = "open-$($c.key)" }
      $shotRex = $null; $shotExp = $null; if ($shot) { $shotRex = "$shot-rexplorer"; $shotExp = "$shot-explorer" }
      $a = Measure-RexOpen $folder $c.n $shotRex
      Start-Sleep -Milliseconds 700
      $b = Measure-ExplorerOpen $folder $c.n $shotExp
      if ($b.hwnd) { $null = $openWindows.Add($b.hwnd); Close-Explorer $b.hwnd }
      Start-Sleep -Milliseconds 700
      if ($i -eq 0) { continue }
      $r.rexWin += $a.win; $r.expWin += $b.win; $r.rex += $a.ms; $r.rexMb += $a.mb; $r.exp += $b.ms; $r.expMb += $b.mb
      $expOpens++; if ($b.fresh) { $expFresh++ }
      Write-Host ("  run {0}: rexplorer {1:N0} ms {2:N1} MB | Explorer {3:N0} ms {4:N1} MB (new process: {5})" -f $i, $a.ms, $a.mb, $b.ms, $b.mb, $b.fresh)
    }
    $res[$c.key] = $r
  }

  # Navigation: one window per program, opened on the 200-file folder, then sent to the 5,000-file folder.
  Write-Host "navigate 200 -> $navN : warm-up + $Runs runs"
  $small = Join-Path $Work $cases[0].dir; $target = Join-Path $Work "files-$navN"
  $nav = @{ rex = @(); exp = @() }
  # rexplorer: Ctrl+L (command 5) shows the address Edit, the text is set synchronously, the clock starts at the
  # Enter command (7) and stops when the list holds the destination's rows.
  $p = [Diagnostics.Process]::Start((Start-Rex $small))
  try {
    $main = 0; $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($main -eq 0 -and $sw.ElapsedMilliseconds -lt 10000) { $main = [B]::OfProcess($p.Id) }
    if ($main -eq 0) { throw 'no rexplorer window for the navigation case' }
    $list = [B]::Child($main, 'SysListView32')
    # Only the address Edit (ID_ADDR = 100): text in the search box (109) would also start a search.
    $addr = [B]::ChildById($main, 'Edit', 100)
    if ($addr -eq 0) { throw 'no address Edit (id 100) in the rexplorer window' }
    function Go-Rex($path, $expected) {
      [B]::Command($main, 5); Start-Sleep -Milliseconds 300
      [B]::SetText($addr, $path)
      $sw = [Diagnostics.Stopwatch]::StartNew(); [B]::Command($main, 7)
      while ([B]::Count($list) -ne $expected) { if ($sw.ElapsedMilliseconds -gt 30000) { return -1 } }
      $sw.Elapsed.TotalMilliseconds
    }
    $sw = [Diagnostics.Stopwatch]::StartNew(); while ([B]::Count($list) -ne $cases[0].n -and $sw.ElapsedMilliseconds -lt 10000) { }
    for ($i = 0; $i -le $Runs; $i++) {
      $ms = Go-Rex $target $navN
      if ($i -eq 1) { Save-Shot $main 'nav-rexplorer' }
      $null = Go-Rex $small $cases[0].n; Start-Sleep -Milliseconds 700
      if ($i -gt 0) { $nav.rex += $ms; Write-Host ("  run {0}: rexplorer {1:N0} ms" -f $i, $ms) }
    }
  } finally { try { $p.Kill(); $p.WaitForExit(3000) | Out-Null } catch {} }
  Start-Sleep -Milliseconds 700

  # Explorer: Navigate2 on the Shell.Application window with that handle.
  $known = [B]::ByClass('CabinetWClass')
  $null = [Diagnostics.Process]::Start((New-Object Diagnostics.ProcessStartInfo $ExplorerExe -Property @{ Arguments = '"' + $small + '"'; UseShellExecute = $false }))
  $h = 0; $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($h -eq 0 -and $sw.ElapsedMilliseconds -lt 10000) { foreach ($w in [B]::ByClass('CabinetWClass')) { if ($known -notcontains $w) { $h = $w; break } } }
  if ($h -eq 0) { throw 'no Explorer window for the navigation case' }
  $null = $openWindows.Add($h)
  try {
    $win = $null; $shell = New-Object -ComObject Shell.Application
    for ($t = 0; $t -lt 100 -and -not $win; $t++) { foreach ($w in $shell.Windows()) { try { if ([int64]$w.HWND -eq $h) { $win = $w } } catch {} }; if (-not $win) { Start-Sleep -Milliseconds 50 } }
    if (-not $win) { throw 'Shell.Application does not list the new window' }
    function Go-Explorer($path, $expected) {
      $sw = [Diagnostics.Stopwatch]::StartNew(); $win.Navigate2($path)
      $rows = Wait-ExplorerRows $h $sw 30000 $script:expView; $script:expView = $rows.view
      if ($rows.ms -ge 0 -and (Wait-Explorer $h $expected ([Diagnostics.Stopwatch]::StartNew()) 30000) -lt 0) { return -1 }
      $rows.ms
    }
    $null = Wait-Explorer $h $cases[0].n ([Diagnostics.Stopwatch]::StartNew()) 10000
    $script:expView = [B]::Descendant($h, 'SHELLDLL_DefView')
    for ($i = 0; $i -le $Runs; $i++) {
      $ms = Go-Explorer $target $navN
      if ($i -eq 1) { Save-Shot $h 'nav-explorer' }
      $null = Go-Explorer $small $cases[0].n; Start-Sleep -Milliseconds 700
      if ($i -gt 0) { $nav.exp += $ms; Write-Host ("  run {0}: Explorer {1:N0} ms" -f $i, $ms) }
    }
  } finally { Close-Explorer $h }
  $res['nav'] = $nav
}
finally {
  foreach ($h in $openWindows) { if ([B]::Alive($h)) { Close-Explorer $h } }
  Start-Sleep -Milliseconds 300
  Remove-Item $RexAppData -Recurse -Force -ErrorAction SilentlyContinue
  # Only the folders this run fills: -Work may be a folder the caller already uses.
  if (-not $KeepFiles) {
    foreach ($d in ($cases | ForEach-Object { $_.dir }) + "files-$navN") { Remove-Item -LiteralPath (Join-Path $Work $d) -Recurse -Force -ErrorAction SilentlyContinue }
    if (-not (Get-ChildItem -LiteralPath $Work -Force -ErrorAction SilentlyContinue)) { Remove-Item -LiteralPath $Work -Force -ErrorAction SilentlyContinue }
  }
}

# ---- report -----------------------------------------------------------------------------------------
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os = Get-CimInstance Win32_OperatingSystem
$ram = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)
$expVer = (Get-Item $ExplorerExe).VersionInfo.FileVersion
$rexInfo = Get-Item $Exe
$rows = @(
  @('First window visible, 200-file folder (ms)', $res.small.rexWin, $res.small.expWin, 'N0'),
  @('Open a 200-file folder (ms)', $res.small.rex, $res.small.exp, 'N0'),
  @('Open a 1,000-file folder (ms)', $res.mid.rex, $res.mid.exp, 'N0'),
  @('Open a 20,000-file folder (ms)', $res.big.rex, $res.big.exp, 'N0'),
  @("Navigate 200 -> $navN files in the open window (ms)", $res.nav.rex, $res.nav.exp, 'N0'),
  @('Private working set after opening 200 files (MB)', $res.small.rexMb, $res.small.expMb, 'N1'),
  @('Private working set after opening 1,000 files (MB)', $res.mid.rexMb, $res.mid.expMb, 'N1'),
  @('Private working set after opening 20,000 files (MB)', $res.big.rexMb, $res.big.expMb, 'N1'))
$md = New-Object Collections.ArrayList
$null = $md.Add("Machine: $($cpu.Name.Trim()), $($cpu.NumberOfLogicalProcessors) logical cores, $ram GB RAM, Windows $($os.Version); Explorer $expVer; rexplorer.exe built $($rexInfo.LastWriteTime.ToString('yyyy-MM-dd HH:mm')) ($([math]::Round($rexInfo.Length / 1KB)) KB); run $(Get-Date -Format 'yyyy-MM-dd'), $Runs runs per cell after 1 warm-up, median (min-max).")
$null = $md.Add('')
$null = $md.Add('| Metric | rexplorer | Windows Explorer | rexplorer / Explorer |')
$null = $md.Add('|---|---|---|---|')
foreach ($r in $rows) {
  $a = Stat $r[1]; $b = Stat $r[2]
  $null = $md.Add(('| {0} | {1} | {2} | {3} |' -f $r[0], (Cell $a $r[3]), (Cell $b $r[3]), (Ratio $a $b)))
}
$null = $md.Add('')
$null = $md.Add("Explorer windows hosted by a new explorer.exe process (memory = that whole process): $expFresh of $expOpens measured opens; the rest are the growth of an existing process.")
$md | Set-Content -Path $Out -Encoding UTF8
$md
