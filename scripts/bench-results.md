Machine: 12th Gen Intel(R) Core(TM) i5-12450H, 12 logical cores, 32 GB RAM, Windows 10.0.26200; Explorer 10.0.26100.8875 (WinBuild.160101.0800); rexplorer.exe built 2026-10-10 00:55 (1636 KB); run 2026-10-10, 7 runs per cell after 1 warm-up, median (min-max).

| Metric | rexplorer | Windows Explorer | rexplorer / Explorer |
|---|---|---|---|
| First window visible, 200-file folder (ms) | 153 (141-168) | 337 (316-782) | 0.45x |
| Open a 200-file folder (ms) | 226 (214-240) | 899 (866-954) | 0.25x |
| Open a 1,000-file folder (ms) | 243 (214-266) | 873 (858-927) | 0.28x |
| Open a 20,000-file folder (ms) | 421 (419-452) | 925 (783-947) | 0.45x |
| Navigate 200 -> 5000 files in the open window (ms) | 65 (64-70) | 166 (160-179) | 0.39x |
| Private working set after opening 200 files (MB) | 5.3 (5.1-5.3) | 100.8 (100.3-101.4) | 0.05x |
| Private working set after opening 1,000 files (MB) | 5.4 (5.4-5.5) | 100.8 (100.4-101.1) | 0.05x |
| Private working set after opening 20,000 files (MB) | 8.3 (7.9-8.6) | 105.9 (105.4-106.5) | 0.08x |

Explorer windows hosted by a new explorer.exe process (memory = that whole process): 21 of 21 measured opens; the rest are the growth of an existing process.

## Budgets and smoke timings, 2026-11-11

Same machine (Windows 10.0.26300 by then); rexplorer.exe release build of the `ADD/v1-release-candidate` working tree
on top of `0d6344f` (2642 KB), run as a copy with a scratch `%APPDATA%` / `%LOCALAPPDATA%`, while the user had
Edge, Discord and WhatsApp open. `scripts\budgets.ps1`, four runs (5, then three with `-Runs 7 -Focus`), median
of each run and the range of all timed samples:

| Budget | Medians of the four runs | All samples | Budget set |
|---|---|---|---|
| Start to the first visible window, empty folder (ms) | 195, 189, 185, 185 | 173-243 | 400 (was 600) |
| 100,000-file folder listed in an open window (ms) | 185, 184, 181, 182 | 176-223 | 400 (was 600) |
| Idle CPU over 10 s with that folder shown (% of a core) | 0.0, 0.0, 0.0, 0.0 | 0.0 | 0.5 (was 1; one 15.6 ms CPU tick is 0.16 %) |

`scripts\smoke.ps1` timings in the same session (two full runs): perf, process start until all 100,000 rows are
listed, 523 and 511 ms (limit 2500, left for slower machines and the Windows 10 VM); handoff, second launch to the
new tab's title, 50 and 37 ms (limit 300).

## Soak, 2026-11-11 (15 minutes each, `-SampleSeconds 30`; the 8-hour run is still to do)

| Run | Build | Iterations / waits missed | GDI | USER | Handles | Threads | Private MB | Verdict |
|---|---|---|---|---|---|---|---|---|
| 1 | before the fixes below | 67 / 67 | 421 -> 839 | 76 -> 75 | 860 -> 849 | 50 -> 144 | 34.7 -> 43.4 | FAIL gdi, threads |
| 2 | Copy/Cut/Paste without the shell verbs, address-bar fix | 67 / 67 | 249 -> 296 | 75 -> 76 | 685 -> 686 | 12 -> 13 | 31.5 -> 35.4 | PASS |
| 3 | + preview-pane crash fix, soak harness fixes | 102 / 0 | 259 -> 330 | 73 -> 73 | 701 -> 700 | 13 -> 12 | 24.8 -> 27.9 | FAIL gdi (limit 322) |

What each run showed and what was done:
- Threads (run 1): +1 per Copy and +1 per Paste, all started in `WorkfoldersShell.dll` (the Work Folders context-menu
  handler, registered for `*`, `Directory` and `Directory\Background`), which the shell's copy and paste verbs load
  with every menu. Copy and Cut now put the folder's data object on the clipboard directly and Paste uses the
  default menu without extension handlers; 5 copies, cuts or pastes then add no thread.
- Waits missed (runs 1-2): one per round. The soak's tab step closed the folder's tab and left Home, where its
  search found nothing; and entering the searched folder's path in the address bar on search results searched
  again instead of opening the folder (an app bug, fixed). Run 3 misses none.
- Crash (found while splitting the iteration into steps): preview pane on, a row selected on Home, 0xC000041D;
  the minidump, symbolized with a line-table build, pointed at `preview_pane::paint` handing DrawTextW an empty
  string (pointer 0x2). Fixed; smoke `home-preview` guards it.
- GDI (runs 2-3): about +0.7 per round, linear, all from Paste (Copy, navigation, tabs, search, preview and the
  context-menu build each add none, measured per step over 8-15 repeats). The shell's copy engine does the same
  outside rexplorer: a plain Windows PowerShell process with one WinForms window ran `IFileOperation::CopyItem` +
  `PerformOperations` 150 times and went from 38 to 203 GDI objects (about 1.1 per copy, with or without an owner
  window). So run 3 fails its GDI margin by 8 objects for a reason outside rexplorer (Windows 10.0.26300); an 8-hour
  run at this paste rate (about 3,300 pastes) would end near +2,300 GDI objects. Not fixed: rexplorer would have to
  stop using the shell's copy engine.
