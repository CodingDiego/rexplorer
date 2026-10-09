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
