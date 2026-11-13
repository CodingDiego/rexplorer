# rexplorer performance budgets (#48). Windows PowerShell 5.1. Times three things from outside the process and
# exits 1 when a median is over its budget, else 0:
#   start  process start to the window's first paint, on an empty folder                 (-StartMs, default 400)
#   list   an open window sent to %TEMP%\rexplorer-bench (100,000 empty files, made once if missing, shared with
#          smoke.ps1) until the load finishes and its list holds every file             (-ListMs, default 400)
#   idle   CPU time of the process over -IdleSeconds (default 10) once that listing settled, in % of one core
#                                                                                        (-IdlePct, default 0.5)
# The defaults are about twice the medians measured on 2026-11-11 (scripts\bench-results.md: start 185-195 ms,
# list 181-185 ms, idle 0.0 %; the CPU counter's 15.6 ms tick is 0.16 % of 10 s, so 0.5 % is three ticks).
#   -Exe <path>  the rexplorer.exe to test (default target\release\rexplorer.exe)
#   -Runs <n>    timed runs per budget after one warm-up (default 5)
#   -Focus       brings the window to the front for the idle sample (the budget is for a focused window); it takes
#                the foreground, so use it only on a machine nobody is typing on
# Both are read from window properties the app sets on its main window (instance.rs: rexplorer.painted = tick count
# of the first paint, rexplorer.loads = finished folder loads, rexplorer.loadtick = tick count of the last one), not
# from titles. Budgets are parameters: CI passes looser values, e.g. -StartMs 1500 -ListMs 1500 -IdlePct 2.
# The budgets were measured on a 12-core laptop; a shared CI runner is slower, so pass looser values there.
# Safety: the app runs on a scratch %APPDATA% and %LOCALAPPDATA%, scratch install folder and scratch registry base, is driven only by
# window messages to its own window and is killed by pid. No input is sent.
param([string]$Exe = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe'),
  [int]$Runs = 5, [double]$StartMs = 400, [double]$ListMs = 400, [double]$IdlePct = 0.5, [int]$IdleSeconds = 10, [switch]$Focus)
$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
if (-not (Test-Path $Exe)) { Write-Output "FAIL exe missing: $Exe"; exit 1 }
$Exe = (Resolve-Path $Exe).Path
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class B {
  public delegate bool EP(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EP f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string cls, string t);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr GetProp(IntPtr h, string name);
  public static long Prop(IntPtr h, string name) { return GetProp(h, name).ToInt64(); }
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  // First visible top-level window of process `pid`.
  public static IntPtr OfProcess(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
'@

$Scratch = Join-Path $env:TEMP "rexplorer-budgets-$PID"
$Empty = Join-Path $Scratch 'empty'
New-Item -ItemType Directory -Path $Empty, "$Scratch\appdata", "$Scratch\local" -Force | Out-Null
$Big = Join-Path $env:TEMP 'rexplorer-bench'
if (-not (Test-Path $Big)) {
  Write-Host 'creating 100,000 files ...'
  New-Item -ItemType Directory -Path $Big -Force | Out-Null
  for ($i = 0; $i -lt 100000; $i++) { [IO.File]::WriteAllText((Join-Path $Big ('file{0:D6}.txt' -f $i)), '') }
}
$BigN = @([IO.Directory]::EnumerateFileSystemEntries($Big)).Count

function Start-Rex($folder) {
  $psi = New-Object Diagnostics.ProcessStartInfo $Exe
  $psi.Arguments = '"' + $folder + '"'; $psi.UseShellExecute = $false
  $psi.EnvironmentVariables['APPDATA'] = "$Scratch\appdata"
  $psi.EnvironmentVariables['LOCALAPPDATA'] = "$Scratch\local"
  $psi.EnvironmentVariables['REXPLORER_INSTALL_DIR'] = "$Scratch\install\app"
  $psi.EnvironmentVariables['REXPLORER_REG_BASE'] = "Software\rexplorer-test\budgets-$PID"
  $psi
}
function Stop-Rex($p) { try { if (-not $p.HasExited) { $p.Kill(); $null = $p.WaitForExit(3000) } } catch {} }
function Median($v) { $s = @($v | Sort-Object); if ($s.Count -eq 0) { return -1 }; if ($s.Count % 2) { $s[($s.Count - 1) / 2] } else { ($s[$s.Count / 2 - 1] + $s[$s.Count / 2]) / 2 } }
$results = @()
function Judge($name, $values, $budget, $unit) {
  $m = Median $values
  $ok = $m -ge 0 -and $m -le $budget
  $script:results += $ok
  Write-Output ("{0} {1}: median {2:N1} {3} (budget {4} {3}; runs {5})" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $name, $m, $unit, $budget, (($values | ForEach-Object { '{0:N0}' -f $_ }) -join ', '))
}

try {
  # start: busy-polls, like bench-vs-explorer.ps1, so the poll interval adds nothing to the time.
  $start = @()
  for ($i = 0; $i -le $Runs; $i++) {
    $psi = Start-Rex $Empty
    $sw = [Diagnostics.Stopwatch]::StartNew(); $p = [Diagnostics.Process]::Start($psi)
    try {
      $ms = -1
      while ($sw.ElapsedMilliseconds -lt 10000) {
        $w = [B]::OfProcess([uint32]$p.Id)
        if ($w -ne [IntPtr]::Zero -and [B]::Prop($w, 'rexplorer.painted') -ne 0) { $ms = $sw.Elapsed.TotalMilliseconds; break }
      }
    } finally { Stop-Rex $p }
    if ($i -gt 0 -and $ms -ge 0) { $start += $ms }
    Start-Sleep -Milliseconds 700
  }
  Judge 'start' $start $StartMs 'ms'

  # list: one window, sent to the big folder and back to the empty one; the clock runs from Enter (CMD_ENTER 7) in
  # the address Edit (ID_ADDR 100, shown by CMD_ADDRESS 5) until rexplorer.loads goes up and LVM_GETITEMCOUNT = every file.
  $p = [Diagnostics.Process]::Start((Start-Rex $Empty))
  try {
    $main = [IntPtr]::Zero; $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($main -eq [IntPtr]::Zero -and $sw.ElapsedMilliseconds -lt 10000) { $main = [B]::OfProcess([uint32]$p.Id) }
    if ($main -eq [IntPtr]::Zero) { throw 'no rexplorer window within 10 s' }
    Start-Sleep -Milliseconds 1000
    $list = [B]::FindWindowEx($main, [IntPtr]::Zero, 'SysListView32', [NullString]::Value)
    function Go($path, $n) {
      $null = [B]::PostMessage($main, 0x111, [IntPtr]5, [IntPtr]::Zero); Start-Sleep -Milliseconds 300
      $null = [B]::SendText([B]::GetDlgItem($main, 100), 0x0C, [IntPtr]::Zero, $path)
      $loads = [B]::Prop($main, 'rexplorer.loads')
      $sw = [Diagnostics.Stopwatch]::StartNew(); $null = [B]::PostMessage($main, 0x111, [IntPtr]7, [IntPtr]::Zero)
      while ([B]::Prop($main, 'rexplorer.loads') -le $loads -or [int][B]::SendMessage($list, 0x1004, [IntPtr]::Zero, [IntPtr]::Zero) -ne $n) {
        if ($sw.ElapsedMilliseconds -gt 30000) { return -1 }
      }
      $sw.Elapsed.TotalMilliseconds
    }
    $list100k = @()
    for ($i = 0; $i -le $Runs; $i++) {
      $ms = Go $Big $BigN
      if ($i -gt 0 -and $ms -ge 0) { $list100k += $ms }
      $null = Go $Empty 0; Start-Sleep -Milliseconds 700
    }
    Judge "list ($BigN files)" $list100k $ListMs 'ms'

    # idle: the big folder on screen, nothing happening.
    $null = Go $Big $BigN
    if ($Focus) { $null = [B]::SetForegroundWindow($main) }
    Start-Sleep -Seconds 2
    $p.Refresh(); $cpu0 = $p.TotalProcessorTime; $sw = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Seconds $IdleSeconds
    $p.Refresh(); $pct = ($p.TotalProcessorTime - $cpu0).TotalMilliseconds / $sw.Elapsed.TotalMilliseconds * 100
    Write-Host "idle sample: foreground ours=$([B]::GetForegroundWindow() -eq $main)"
    Judge 'idle CPU' @($pct) $IdlePct '% of a core'
  } finally { Stop-Rex $p }
} catch { Write-Output "FAIL budgets: $($_.Exception.Message)"; $results += $false }
finally {
  Start-Sleep -Milliseconds 300
  Remove-Item $Scratch -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath "HKCU:\Software\rexplorer-test\budgets-$PID" -Recurse -Force -ErrorAction SilentlyContinue
}
if ($results -contains $false -or $results.Count -lt 3) { exit 1 }
exit 0
