# rexplorer smoke test. Windows PowerShell 5.1. Prints one PASS/FAIL/SKIP line per check, always exits 0.
$ErrorActionPreference = 'Stop'
$Exe = Join-Path (Split-Path $PSScriptRoot -Parent) 'target\release\rexplorer.exe'
Add-Type -AssemblyName System.Windows.Forms, Microsoft.VisualBasic, UIAutomationClient, UIAutomationTypes
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class W {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string cls, string title);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
}
'@

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

Run-Check 'fallback' {
  $before = Get-Explorers
  $script:cur = Start-App 'shell:::{11111111-2222-3333-4444-555555555555}'
  Start-Sleep -Seconds 3; $script:cur.Refresh()
  $alive = -not $script:cur.HasExited
  $after = Get-Explorers
  $newExp = @($after.Keys | Where-Object { -not $before.ContainsKey($_) }).Count
  $dlg = [W]::FindWindow('#32770', 'rexplorer')
  if ($dlg -ne [IntPtr]::Zero) { $null = [W]::PostMessage($dlg, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) }
  Close-NewExplorers $before
  $d = "explorer window appeared=$($newExp -gt 0) (new=$newExp); rexplorer dialog appeared=$($dlg -ne [IntPtr]::Zero)"
  if ($alive) { @('PASS', "alive after 3 s; $d") } else { @('FAIL', "process exited; $d") }
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
exit 0
