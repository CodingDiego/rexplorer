# rexplorer soak test (#29). Windows PowerShell 5.1. Runs one rexplorer for -Minutes, looping navigation, tabs,
# search, the preview pane, thumbnails and file operations in a scratch tree, and samples the process's GDI objects,
# USER objects, handles, threads and private bytes into a CSV. Exits 1 when the process died, a resource grew past
# the margin, or fewer than -MinIterations rounds ran, else 0. It prints how many of each operation it did.
# The release run is one hour (`-Minutes 60`), before every release: a leak grows with the number of operations,
# not with the clock, and an hour is about 400 rounds here (6.8 a minute on 2026-11-11), each round 3 navigations,
# 2 tab switches plus a new and a closed tab, 1 search, 6 previewed rows, 1 thumbnails view, a copy and a paste and
# 60 file changes (20 creates, 20 renames, 20 deletes).
#   -Exe <path>          the rexplorer.exe to test (default target\release\rexplorer.exe)
#   -Minutes <n>         run length (default 10, a harness check; the release run is 60)
#   -SampleSeconds <n>   time between samples (default 30)
#   -MinIterations <n>   rounds the run must reach (default 5 a minute: 300 for the hour); a stuck run fails
#   -Csv <path>          where the samples go (default %TEMP%\rexplorer-soak-<pid>.csv, kept)
# Margin: the first 5 minutes (at most a fifth of the run) are warm-up and not judged. Of the judged samples, the
# baseline is the median of the first fifth and the end value the median of the last fifth; a resource fails when
# end > baseline * 1.05 + slack, with slack GDI 50, USER 50, handles 200, threads 10, private bytes 50 MB.
# GDI also gets one object per paste made between the baseline and the end samples (the app's Paste goes through the shell's copy engine, which leaks
# about 0.7 to 1.1 GDI objects per copy in any process: 8 hours would end near +2,300 objects; see
# scripts/bench-results.md, run 3: 102 pastes, GDI 259 -> 330, over the plain margin's limit of 322 by 8, and a plain
# PowerShell process doing IFileOperation::CopyItem 150 times went 38 -> 203). A leak elsewhere still fails.
# Safety: the app runs on a scratch %APPDATA% and %LOCALAPPDATA%, scratch install folder and scratch registry base, and is driven only
# by window messages to its own window (no keys, no mouse, no foreground needed). Files are created and deleted by
# this script inside its own temp tree, never through the Recycle Bin: the app's Delete and Undo recycle, so they
# are not used. Copy and Paste go through the clipboard, which is restored at the end; leave the clipboard alone
# while it runs.
param([string]$Exe = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe'),
  [double]$Minutes = 10, [int]$SampleSeconds = 30, [int]$MinIterations = -1, [string]$Csv = (Join-Path $env:TEMP "rexplorer-soak-$PID.csv"))
if ($MinIterations -lt 0) { $MinIterations = [int]($Minutes * 5) }
$ErrorActionPreference = 'Stop'
if (-not (Test-Path $Exe)) { Write-Output "FAIL exe missing: $Exe"; exit 1 }
$Exe = (Resolve-Path $Exe).Path
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class S {
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string cls, string t);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  // GR_GDIOBJECTS 0, GR_USEROBJECTS 1.
  [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr process, uint flags);
  public static string Title(IntPtr h) { var sb = new StringBuilder(512); GetWindowText(h, sb, 512); return sb.ToString(); }
}
'@

# ---- scratch tree and profile -----------------------------------------------------------------------
$Root = Join-Path $env:TEMP "rexplorer-soak-$PID"
$Docs = Join-Path $Root 'docs'; $Pics = Join-Path $Root 'pics'; $Work = Join-Path $Root 'work'
foreach ($d in $Docs, $Pics, $Work, "$Root\appdata", "$Root\local", "$Root\install") { New-Item -ItemType Directory -Path $d -Force | Out-Null }
for ($i = 0; $i -lt 200; $i++) { [IO.File]::WriteAllText((Join-Path $Docs ('file{0:D3}.txt' -f $i)), "soak line $i") }
# Pictures, so the thumbnails view has thumbnails to make.
for ($i = 0; $i -lt 40; $i++) {
  $bmp = New-Object System.Drawing.Bitmap 256, 256
  $g = [System.Drawing.Graphics]::FromImage($bmp); $g.Clear([System.Drawing.Color]::FromArgb(255, ($i * 6) % 256, ($i * 37) % 256, ($i * 91) % 256)); $g.Dispose()
  $bmp.Save((Join-Path $Pics ('pic{0:D2}.png' -f $i)), [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
}
$Keep = Join-Path $Work 'keep.txt'; [IO.File]::WriteAllText($Keep, 'copied and pasted by the soak test')

$psi = New-Object Diagnostics.ProcessStartInfo $Exe
$psi.Arguments = '"' + $Docs + '"'; $psi.UseShellExecute = $false
$psi.EnvironmentVariables['APPDATA'] = "$Root\appdata"
$psi.EnvironmentVariables['LOCALAPPDATA'] = "$Root\local"
$psi.EnvironmentVariables['REXPLORER_INSTALL_DIR'] = "$Root\install\app"
$psi.EnvironmentVariables['REXPLORER_REG_BASE'] = "Software\rexplorer-test\soak-$PID"

# ---- driving: window messages only ------------------------------------------------------------------
function Alive { $script:p.Refresh(); if ($script:p.HasExited) { throw "rexplorer exited (code $($script:p.ExitCode))" } }
function Cmd($id) { $null = [S]::PostMessage($script:main, 0x111, [IntPtr]$id, [IntPtr]::Zero) }
function List { [S]::FindWindowEx($script:main, [IntPtr]::Zero, 'SysListView32', [NullString]::Value) }
function Count { [int][S]::SendMessage((List), 0x1004, [IntPtr]::Zero, [IntPtr]::Zero) }
function Key($h, $vk) { $null = [S]::PostMessage($h, 0x100, [IntPtr]$vk, [IntPtr]::Zero) }
# Waits for $cond; a miss is counted, not fatal (the soak judges resources, the smoke test judges behavior).
function Wait-Until([scriptblock]$cond, $ms, $what) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.ElapsedMilliseconds -lt $ms) { Alive; if (& $cond) { return }; Start-Sleep -Milliseconds 50 }
  $script:misses++; $script:missedBy[$what]++
}
# Ctrl+L, the path into the address Edit (ID_ADDR 100), Enter (CMD_ADDRESS 5, CMD_ENTER 7).
function Go($path, $n) {
  $script:ops.navigations++
  Cmd 5; Start-Sleep -Milliseconds 200
  $null = [S]::SendText([S]::GetDlgItem($script:main, 100), 0x0C, [IntPtr]::Zero, $path)
  Cmd 7
  Wait-Until { (Count) -eq $n } 10000 "open $(Split-Path $path -Leaf)"
}
function Iteration {
  # Navigate, then the preview pane (CMD_TOGGLE_PREVIEW 24) over a few selected rows (Home, Down).
  Go $Docs 200
  Cmd 24; Key (List) 0x24
  for ($k = 0; $k -lt 5; $k++) { Start-Sleep -Milliseconds 300; Key (List) 0x28 }
  Start-Sleep -Milliseconds 300; Cmd 24; $script:ops.previews += 6
  # Thumbnails (CMD_VIEW1 + 2), back to Details (CMD_VIEW1).
  Go $Pics 40
  Cmd 27; Start-Sleep -Milliseconds 1500; Cmd 25; $script:ops.thumbnails++
  # Tabs: new (10), next (12) twice, close (11) the new one, so the pictures tab stays (closing it instead left the
  # window on Home, where the search below found nothing every round).
  Cmd 10; Start-Sleep -Milliseconds 500; Cmd 12; Start-Sleep -Milliseconds 300; Cmd 12; Start-Sleep -Milliseconds 300; Cmd 11; Start-Sleep -Milliseconds 300
  $script:ops.tabSwitches += 2; $script:ops.tabsOpenedClosed++
  # Search the pictures from the search box (ID_SEARCH 109): pic10 to pic19.
  $box = [S]::GetDlgItem($script:main, 109); $t0 = [S]::Title($script:main)
  $null = [S]::SendText($box, 0x0C, [IntPtr]::Zero, 'pic1'); Key $box 0x0D; $script:ops.searches++
  Wait-Until { (Count) -ge 1 -and ([S]::Title($script:main)) -ne $t0 } 5000 'search results'
  Start-Sleep -Milliseconds 500
  # File operations: the app's Copy (43) and Paste (44) of keep.txt, then this script's own churn in the folder.
  Go $Work 1
  # Only in the work folder: after a missed navigation, Copy and Paste would act on whatever folder is showing.
  if ((Count) -ge 1 -and [S]::Title($script:main) -eq "$(Split-Path $Work -Leaf) - rexplorer") {
    Key (List) 0x24; Start-Sleep -Milliseconds 200; Cmd 43; Start-Sleep -Milliseconds 300; Cmd 44; $script:copies++; $script:ops.fileOps += 2
    Wait-Until { (Count) -ge 2 } 5000 'paste'
  } else { $script:misses++; $script:missedBy['not in the work folder']++ }
  $tag = $script:iterations
  for ($k = 0; $k -lt 20; $k++) { [IO.File]::WriteAllText((Join-Path $Work "churn-$tag-$k.tmp"), 'x') }
  for ($k = 0; $k -lt 20; $k++) { [IO.File]::Move((Join-Path $Work "churn-$tag-$k.tmp"), (Join-Path $Work "churn-$tag-$k.txt")) }
  # A file the app still holds open is left for the next round.
  # By name: on a hosted runner %TEMP% is a short path (RUNNER~1) and FullName comes back long, so a full-path
  # comparison deleted keep.txt in the first round and every later round waited on an empty folder.
  $gone = @(Get-ChildItem -LiteralPath $Work -File | Where-Object { $_.Name -ne (Split-Path $Keep -Leaf) })
  $gone | Remove-Item -Force -ErrorAction SilentlyContinue
  $script:ops.fileOps += 40 + $gone.Count
  Wait-Until { (Count) -eq 1 } 5000 'churn removed'
}
function Sample {
  $script:p.Refresh()
  [pscustomobject]@{
    minute    = [math]::Round($script:clock.Elapsed.TotalMinutes, 2)
    gdi       = [S]::GetGuiResources($script:p.Handle, 0)
    user      = [S]::GetGuiResources($script:p.Handle, 1)
    handles   = $script:p.HandleCount
    threads   = $script:p.Threads.Count
    privateMB = [math]::Round($script:p.PrivateMemorySize64 / 1MB, 1)
    copies    = $script:copies
  }
}
function Median($v) { $s = @($v | Sort-Object); if ($s.Count % 2) { $s[($s.Count - 1) / 2] } else { ($s[$s.Count / 2 - 1] + $s[$s.Count / 2]) / 2 } }
# The clipboard's formats as a detached copy (Windows PowerShell runs STA, as the clipboard needs).
# shortcut: formats .NET cannot read back (some private ones) are lost; fine for a test session's clipboard.
function Save-Clipboard {
  $d = New-Object System.Windows.Forms.DataObject
  try {
    $src = [System.Windows.Forms.Clipboard]::GetDataObject()
    if ($src) { foreach ($f in $src.GetFormats($false)) { try { $v = $src.GetData($f, $false); if ($null -ne $v) { $d.SetData($f, $false, $v) } } catch {} } }
  } catch {}
  $d
}
function Restore-Clipboard($d) {
  try { if (@($d.GetFormats()).Count) { [System.Windows.Forms.Clipboard]::SetDataObject($d, $true) } else { [System.Windows.Forms.Clipboard]::Clear() } } catch {}
}

# ---- run --------------------------------------------------------------------------------------------
$clip = Save-Clipboard
$script:misses = 0; $script:missedBy = @{}; $script:iterations = 0; $script:copies = 0; $samples = New-Object Collections.ArrayList; $died = $null
$script:ops = [ordered]@{ navigations = 0; tabSwitches = 0; tabsOpenedClosed = 0; searches = 0; previews = 0; thumbnails = 0; fileOps = 0 }
$script:p = $null
try {
  $script:p = [Diagnostics.Process]::Start($psi); $null = $script:p.Handle
  $script:main = [IntPtr]::Zero
  for ($i = 0; $i -lt 200 -and $script:main -eq [IntPtr]::Zero; $i++) { Alive; $script:main = $script:p.MainWindowHandle; Start-Sleep -Milliseconds 50 }
  if ($script:main -eq [IntPtr]::Zero) { throw 'no main window within 10 s' }
  Wait-Until { (Count) -eq 200 } 10000 'first listing'
  $script:clock = [Diagnostics.Stopwatch]::StartNew(); $next = 0
  while ($script:clock.Elapsed.TotalMinutes -lt $Minutes) {
    Iteration; $script:iterations++
    if ($script:clock.Elapsed.TotalSeconds -ge $next) {
      $s = Sample; $null = $samples.Add($s); $next += $SampleSeconds
      Write-Host ("{0,7:N2} min  GDI {1,5}  USER {2,5}  handles {3,6}  threads {4,4}  private {5,7:N1} MB  ({6} iterations, {7} waits missed)" -f $s.minute, $s.gdi, $s.user, $s.handles, $s.threads, $s.privateMB, $iterations, $script:misses)
    }
  }
  $null = $samples.Add((Sample))
} catch { $died = $_.Exception.Message }
finally {
  if ($script:p) { try { if (-not $script:p.HasExited) { $script:p.Kill(); $null = $script:p.WaitForExit(3000) } } catch {} }
  Restore-Clipboard $clip
  Start-Sleep -Milliseconds 300
  Remove-Item $Root -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath "HKCU:\Software\rexplorer-test\soak-$PID" -Recurse -Force -ErrorAction SilentlyContinue
  if ($samples.Count) { $samples | Export-Csv -Path $Csv -NoTypeInformation -Encoding UTF8 }
}

# ---- verdict ----------------------------------------------------------------------------------------
if ($died) { Write-Output "FAIL soak: $died after $iterations iterations; samples in $Csv"; exit 1 }
$warm = [math]::Min(5, $Minutes / 5)
$judged = @($samples | Where-Object { $_.minute -ge $warm })
if ($judged.Count -lt 5) { Write-Output "FAIL soak: only $($judged.Count) samples after the $warm min warm-up; lower -SampleSeconds or raise -Minutes"; exit 1 }
$k = [math]::Max(1, [int][math]::Floor($judged.Count / 5))
$slack = [ordered]@{ gdi = 50; user = 50; handles = 200; threads = 10; privateMB = 50 }
$first = $judged[0..($k - 1)]; $last = $judged[($judged.Count - $k)..($judged.Count - 1)]
# Only the pastes between the baseline and the end samples: the warm-up's are already in the baseline.
$allow = @{ gdi = [int]((Median ($last | ForEach-Object { $_.copies })) - (Median ($first | ForEach-Object { $_.copies }))) }
$failed = $false
foreach ($m in $slack.Keys) {
  $base = Median ($first | ForEach-Object { $_.$m })
  $end = Median ($last | ForEach-Object { $_.$m })
  $limit = $base * 1.05 + $slack[$m] + [int]$allow[$m]
  $status = if ($end -gt $limit) { $failed = $true; 'FAIL' } else { 'PASS' }
  Write-Output ("{0} {1}: {2} -> {3} (limit {4:N1}{5})" -f $status, $m, $base, $end, $limit, $(if ($allow[$m]) { ", incl. $($allow[$m]) for the pastes" }))
}
Write-Output "$iterations iterations in $Minutes min, $($script:misses) waits missed$(if ($script:misses) { ' (' + (($script:missedBy.GetEnumerator() | ForEach-Object { "$($_.Key) x$($_.Value)" }) -join ', ') + ')' }); samples in $Csv"
Write-Output ("operations: {0}" -f (($script:ops.GetEnumerator() | ForEach-Object { "$($_.Key) $($_.Value)" }) -join ', '))
$s0 = $samples[0]; $s1 = $samples[$samples.Count - 1]
Write-Output ("first and last sample: GDI {0} -> {1}, USER {2} -> {3}, handles {4} -> {5}, threads {6} -> {7}, private {8} -> {9} MB" -f $s0.gdi, $s1.gdi, $s0.user, $s1.user, $s0.handles, $s1.handles, $s0.threads, $s1.threads, $s0.privateMB, $s1.privateMB)
if ($iterations -lt $MinIterations) { Write-Output "FAIL soak: $iterations iterations, fewer than the stated $MinIterations"; $failed = $true }
if ($failed) { exit 1 }
exit 0
