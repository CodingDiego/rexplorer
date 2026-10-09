# rexplorer

A Windows file manager in Rust that hosts the Windows shell namespace and renders it with plain Win32
controls. Fast on the common path, delegates everything else to the shell, and falls back to Windows
Explorer for anything it cannot do yet.

Layout:

- `crates/core`: headless shell layer (folders, items, menus, notifications). Tested against the real shell.
- `crates/app`: the Win32 window. No logic beyond plumbing.

```
cargo test -p rexplorer-core
cargo run --release
cargo run --release -p rexplorer-core --example bench -- <folder>
```

## Phase 0 result: enumeration through the shell is fast enough

Folder of 100,000 files in `%TEMP%\rexplorer-bench`, release build, warm cache, 2026-10-09:

| Step | Time |
|---|---|
| `IShellFolder::EnumObjects`, all 100k PIDLs | 130–150 ms |
| Display names for all 100k (the list only needs visible rows) | 46 ms |
| Attributes (folder flag) for all 100k | 31 ms |
| Sort with `CompareIDs` | 66–69 ms |
| Floor: .NET `Directory.GetFileSystemEntries` | 93 ms |
| PowerShell `Get-ChildItem` for scale | 2052 ms |

Decision: keep pure shell enumeration for every folder type. No filesystem fast path.

## Phase 1: preliminary daily driver

One window: address bar over a virtual details list. Starts at the profile's start folder (This PC), or at the
folders given as arguments. Everything comes from the shell: names, columns, icons, sorting, context menus, file operations,
rename, live updates.

| Keys | Action |
|---|---|
| Enter, double-click | Open folder in place, or run the file's default verb |
| Backspace, Alt+Up / Alt+Left / Alt+Right, F5 | Up, back, forward, refresh |
| Ctrl+L, Alt+D | Edit the address (path, `shell:` name or `::{CLSID}`) |
| F2, Delete, Shift+Delete, Ctrl+C/X/V, Ctrl+A, Alt+Enter | Shell verbs on the selection |
| Right-click | Full shell context menu (Shift adds the extended verbs) |
| Ctrl+Shift+E | Open the current folder in Windows Explorer |

A folder that exists but cannot be listed opens in Windows Explorer instead; a name that does not resolve
shows an error. Window placement is saved under `%APPDATA%\rexplorer`.

`scripts\smoke.ps1` drives the release exe through startup, the 100k-file folder (populated in about 290 ms
from process start), columns, live update, fallback and placement. Its keyboard checks need an interactive
desktop to bring the window to the foreground.

Known gaps: This PC shows drives only (no Desktop/Documents entries), no keyboard context menu, icons resolve
on the UI thread.

## Phase 2, step 1: chrome, tabs, settings, CLI and recovery

Layout, top to bottom: a tab strip; a nav row with Back / Forward / Up / Refresh, the breadcrumb address bar
and Terminal / View / Connect / More; the details list; a status bar (item and selection counts, profile).
The window uses the theme from the settings (dark title bar, Mica when the theme asks for it, themed list,
header, Edit and fonts that follow the monitor DPI). Minimum size 640x400.

| Keys | Action |
|---|---|
| Ctrl+T, Ctrl+W | New tab at the same folder, close tab (closing the last tab closes the window) |
| Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+1..8, Ctrl+9 | Next, previous, nth, last tab |
| Middle-click a folder | Open it in a background tab (middle-click a tab closes it) |
| Ctrl+P | Palette: recent folders, profile bookmarks, cloud roots and the current subfolders, fuzzy matched |
| Ctrl+` | Terminal (Windows Terminal, else cmd) in the current filesystem folder |
| Ctrl+L, Alt+D, click on the breadcrumb's empty area | Type an address over the breadcrumb |

Breadcrumb: click a segment to go there; its chevron lists the subfolders (for drive paths, UNC paths and This
PC). More menu: Open in Explorer, register / unregister as the folder handler, the settings folder, About.
View and Connect are placeholders until steps 2 and 3.

Command line (`rexplorer --help`): folders to open, `--select <path>` (or `/select,<path>`), `--profile`,
`--new-tab`, `--register` / `--unregister` (each asks first; per-user `HKCU\Software\Classes` keys only),
`--recover`. A second launch hands its folders to the running window as new tabs and exits.

Files under `%APPDATA%\rexplorer`: `settings.ini` and `themes\*.ini` (written with defaults on first run, then
yours to edit), `session.txt` (open tabs, saved within 2 s of each navigation and on exit), `recent.txt`
(palette history), `window.txt` (placement). A plain launch restores the session's tabs; a launch with
folders opens just those. After a crash the process relaunches itself with `--recover` and restores the session
(at most once per 10 s, so a crash loop stops).

`scripts\smoke.ps1` also checks the tab strip and Ctrl+T, the single-instance hand-off, first-run settings files
(in a scratch `APPDATA`), and that `--help` shows its dialog and exits.
