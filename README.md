# rexplorer

A Windows file manager in Rust that hosts the Windows shell namespace and renders it with plain Win32
controls. Fast on the common path, delegates everything else to the shell, and falls back to Windows
Explorer (embedded in the window) for folders it cannot list.

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

## Features (Phase 1 and Phase 2 through step 3)

Everything shown is a shell item: names, columns, icons, sorting, context menus, file operations, rename, live
updates and drag and drop all come from the shell. rexplorer draws; the shell does the work.

### Window

Top to bottom: a tab strip; a nav row (Back / Forward / Up / Refresh, the breadcrumb address bar, a 200 px
search box, Terminal / View / Connect / More); the folder tree, the list and the preview side by side; a status
bar (item and selection counts, search progress, profile). Both side panes have a 4 px splitter (drag to resize,
120 to 600 px). The window follows the theme (dark title bar, Mica when the theme asks for it, themed list,
header, Edits and fonts that follow the monitor DPI). Minimum size 640x400.

Breadcrumb: click a segment to go there, its chevron lists the subfolders (drive paths, UNC paths and This PC),
a click on the empty area turns it into an address Edit (a path, a `shell:` name, a `::{CLSID}` or a path inside
a .rar).

More menu: Open in Explorer, register / unregister as the folder handler, the settings folder, About, and
**Quick access**: the profile's bookmarks and the cloud-sync roots found on this machine (OneDrive, Google Drive,
Dropbox, iCloud Drive).

### What a tab can show

- **A shell folder**: filesystem, virtual (This PC, Recycle Bin, Control Panel...), zip files (the shell browses
  them as folders), network shares, `ftp://` URLs where the shell supports them.
- **A .rar archive**: Enter on a .rar file, or type a path to it or inside it (`C:\x\a.rar\docs`). Columns
  Name, Date modified, Type, Size; directories first. Enter on a directory enters it, Backspace / Alt+Up goes up
  (at the archive's root: back to its folder), Enter on a file extracts it to
  `%TEMP%\rexplorer-archive\<hash>\` (cached per archive and modification time) and opens it with its default
  app. Right-click: **Extract all to...** (folder picker) or **Extract here** (the archive's folder). Archives
  are read-only: no rename, delete or drop. The archive is listed on a worker thread.
- **Search results**: type in the search box and press Enter (Ctrl+F focuses it). The search walks the current
  folder and every subfolder on a worker thread (archives, links and hidden `C:\$...` folders are skipped);
  results stream into the list as each folder finishes, with columns Name, Folder, Type, Size. The tab and
  window title read `Search: <text>`. Escape stops the search and keeps what it found; F5 runs it again;
  navigating anywhere stops it. When rexplorer runs elevated and the folder is a drive root, the drive's NTFS
  index (read once per drive from the MFT, then kept for the session) answers instead, up to 5000 results.
  Enter on a result opens it (folders and .rar files in place); the context menu, Delete and Ctrl+C / Ctrl+X
  work on results that share one folder.
- **The embedded Explorer view**: a folder that exists but cannot be listed (a phone, an odd namespace
  extension) opens in an Explorer view hosted in place of the list. The breadcrumb, title and address follow
  it; navigating inside it to a folder the list can show brings the list back, as does going anywhere else.
  Ctrl+Shift+E still opens real Explorer. A name that does not resolve shows an error.

### Keys

| Keys | Action |
|---|---|
| Enter, double-click | Open folder, archive or archive directory in place; open a file with its default verb |
| Backspace, Alt+Up | Up (folder, archive directory, archive's folder, or the searched folder) |
| Alt+Left / Alt+Right, mouse back / forward buttons | Back, forward |
| F5, Ctrl+R | Refresh (a search runs again) |
| Ctrl+L, Alt+D | Edit the address over the breadcrumb |
| Ctrl+F | Search box (Enter searches, Escape stops and returns to the list) |
| Escape (list) | Stop a running search |
| F2, Delete, Shift+Delete, Ctrl+C/X/V, Ctrl+A, Alt+Enter | Shell verbs on the selection (F2 on shell items only) |
| Right-click | Full shell context menu (Shift adds the extended verbs); archives get the extract menu |
| Ctrl+Shift+E | Open the current folder in Windows Explorer (an archive: its folder) |
| Ctrl+T, Ctrl+W | New tab at the same place, close tab (closing the last tab closes the window) |
| Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+1..8, Ctrl+9 | Next, previous, nth, last tab |
| Middle-click a folder | Open it in a background tab (middle-click a tab closes it) |
| Ctrl+P | Palette: recent places, bookmarks, cloud roots and the current subfolders, fuzzy matched |
| Ctrl+` | Terminal (Windows Terminal, else cmd) in the current filesystem folder |
| Ctrl+B | Show / hide the folder tree |
| Alt+P | Show / hide the preview pane (text in a monospace Edit, preview handlers in place, else a thumbnail) |
| Ctrl+Shift+1 / 2 / 3, View button | Details / icons (48 px) / thumbnails (128 px); the button cycles |
| Connect button | FTP dialog (host, port, user, password, path); the `ftp://` URL opens in a new tab |

Shortcuts work while the tree, the search box or the embedded Explorer view has the focus, except Enter and
Escape, which those keep.

Icon and thumbnail images render on a worker thread and land as they finish; the last 512 are cached. Drag items
out of the list to anywhere that accepts files, or drop files onto a folder item or the list background.

### Command line

```
rexplorer [options] [folder...]
  --select <path>     open the parent folder with <path> selected (also /select,<path>)
  --profile <name>    use the named settings profile
  --new-tab, -t       open in a new tab of the running window
  --search <text>     search the (first) folder opened for <text>
  --register          open folders with rexplorer (per-user HKCU\Software\Classes keys; asks first)
  --unregister        undo --register
  --recover           restore the previous session after a crash
  -h, --help          show this help
```

Folders may be paths, `shell:` names, `::{CLSID}` strings or .rar paths. `/e`, `/n`, `/root,<path>` are accepted
as Explorer does. A second launch hands its folders (and `--search`) to the running window and exits. A plain
launch restores the session; a launch with folders opens just those. After a crash the process relaunches itself
with `--recover` (at most once per 10 s, so a crash loop stops).

### Files under `%APPDATA%\rexplorer`

`settings.ini` and `themes\*.ini` are written with defaults on first run, then yours to edit. Lines starting
with `;` or `#` are comments.

```ini
[general]
profile=default                  ; the active profile

[profile default]
start=shell:MyComputerFolder     ; where a first run (or a launch without a session) starts
theme=dark                       ; a file name in themes\ without .ini
view=details                     ; details | icons | thumbnails (the View button writes it back here)
show_tree=true
show_preview=false
tree_width=240                   ; 96-DPI pixels
preview_width=360
bookmark.Desktop=shell:Desktop   ; bookmark.<label>=<target>: palette and More > Quick access
bookmark.Downloads=shell:Downloads
```

`themes\<name>.ini` holds one `[theme]` section; missing keys fall back to the built-in dark or light theme
chosen by `dark`:

```ini
[theme]
dark=true
mica=true
background=#202020               ; colors are #RRGGBB
text=#FFFFFF
secondary_text=#A0A0A0
accent=#4CC2FF
selection=#3A3A3A
border=#333333
icon_theme=default               ; optional: a folder under icons\
font=Segoe UI Variable Text
font_size=9                      ; points
```

Icon themes: `icons\<name>\` holds PNGs named by extension (`rs.png`, `pdf.png`...) plus `folder.png` and
`file.png` (the fallback for unknown extensions); the details view draws them, at the small-icon size, instead of
the system icons, for folders, archive members and search results alike. A starter set is written to
`icons\default` on first run.

Written by the app: `session.txt` (`tab=` lines, `active=`, `view=`, `tree=` / `preview=` as 0 or 1, the pane
widths; saved within 2 s of each change and on exit; a search tab is saved as its folder), `recent.txt` (palette
history), `window.txt` (placement). A first run, or `--profile`, takes view and panes from the profile; later
runs from the session.

The exe carries a Common Controls 6 manifest (`crates/app/build.rs`): icon views and the themed controls need it.

### Smoke test

`scripts\smoke.ps1` drives the release exe: startup, the 100k-file folder (`%TEMP%\rexplorer-bench`, populated
in about 300 ms from process start), columns, keyboard navigation, live update, the not-found fallback (error,
no Explorer window), the Explorer hotkey, tabs, the single-instance hand-off, first-run settings, `--help`,
placement, the tree, the preview, the views, a RAR5 archive (open, enter a directory, go back up), a search in
the 100k folder (typed into the search box) and `--search` from the command line. Checks that write settings
run on a scratch `APPDATA`. The keyboard checks need an interactive desktop that lets the window come to the
foreground; they report SKIP otherwise.

### Known gaps

- Search results and archive members show placeholders in the icons and thumbnails views, have no preview,
  cannot be dragged, sorted by column or renamed; search results are not refreshed after a delete.
- The context menu on search results needs every selected result in one folder.
- Back from a folder opened out of search results returns to the searched folder, not to the results.
- Search result icons come from the extension (no per-file icons such as an exe's own).
- Only .rar archives get the archive view (zip is the shell's); multi-volume and encrypted .rar are untested.
- The embedded Explorer view keeps Windows' own colors, not the theme.
- `ftp://` browsing depends on the shell's FTP folder, which recent Windows builds no longer ship; without it the
  URL shows the not-found error.
- No keyboard context menu; small icons of shell folders resolve on the UI thread.
