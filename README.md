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

One window: address bar over a virtual details list. Starts at This PC, or at the folder given as the first
argument. Everything comes from the shell: names, columns, icons, sorting, context menus, file operations,
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
on the UI thread, light theme only.
