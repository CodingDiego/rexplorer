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

## Features (Phase 1 and Phase 2 through step 7)

Everything shown is a shell item: names, columns, icons, sorting, context menus, file operations, rename, live
updates and drag and drop all come from the shell. rexplorer draws; the shell does the work.

### Look

![Light theme](docs/screenshots/light.png)
![Dark theme](docs/screenshots/dark.png)
![Home](docs/screenshots/home.png)

In the spirit of GNOME Files and the macOS Finder: one flat header bar, a sidebar of places, pill-shaped path and
controls, the list and the preview in rounded cards, no 1 px borders. The frame, tab strip, header, sidebar and
status bar share the theme's `background`; cards and fields use its `surface`; selections are `selection` pills.
The system title bar takes the `background` color, so it reads as part of the header.
The cards' corners (the list, the Home page, the preview) are painted antialiased in the frame color, with 4x4
samples per pixel, at 10 px scaled to the DPI; the scroll bars' corners too. The embedded Explorer view and the
command palette keep a plain GDI window region.

### Window

Top to bottom: a tab strip of pill-shaped tabs; a 44 px header bar (a Back / Forward segment, Up, Refresh, the
path bar, a 220 px search field, Terminal, Connect); a 44 px command bar (below); the places sidebar beside the
list card (or the Home page) and the preview card; a status bar (centered item and selection counts, search progress; the profile on the
right). The 8 px gaps between the left pane and the cards are splitters (drag to resize, 120 to 600 px). The window
follows the theme (title bar color, Mica when the theme asks for it, themed list, header, Edits and fonts that follow
the monitor DPI). Minimum size 640x400.

Sidebar, every row read from the shell with its localized name and icon:

- **Home** ("Inicio" on a Spanish Windows) on top, without a section title.
- **Favorites**: the profile's bookmarks, then the folders of Quick Access in the shell's order: the pinned known
  folders, the Recycle Bin, your own pins and the frequent folders. A place is listed once.
- **This PC**: the drives, connected MTP phones and other devices This PC shows (not the user folders it also
  lists).
- **Devices**: what the desktop root adds that is neither a file on the desktop nor a place shown elsewhere: Phone
  Link phones (`%USERPROFILE%\CrossDevice\...`) and Linux (WSL). Left out when empty.
- **Cloud**: the cloud-sync roots found on this machine (OneDrive, Google Drive, Dropbox, iCloud Drive); left out
  when there are none.
- **Network**.

Click a row to go there; the row of the current folder, or of its nearest ancestor, is highlighted. Right-click a
pinned Quick Access folder for **Unpin from Home**, a frequent (not pinned) one for **Pin to Home**, a bookmark for
**Remove bookmark** (its `bookmark.<label>` line leaves settings.ini). Pinned and frequent are told apart by the
shell's `System.Home.IsPinned` property. The rows are built on a worker thread, rebuilt every 5 s while the window is active and right after
a pin or unpin from rexplorer, so the sidebar fills in after the first listing. More > **Folder tree** shows the
shell's folder tree in the sidebar's place (choose it again for the sidebar).

Pins: **Ctrl+D**, **Pin to Home** at the top of the list's context menu, or More > **Pin to Home** run the shell's
own "pin to Home" verb on the selected folders, or on the current folder when nothing is selected. The pin lands
in Explorer's Quick Access too, and unpinning from either side shows in both.

Loading: a navigation shows where it goes at once. The title, tab, path bar and sidebar take the destination's
name, the status bar reads "Loading…", and when the folder takes longer than 120 ms the list shows eight pulsing
placeholder rows until it arrives (a slow Network folder, a sleeping drive). Fast folders never flash. A refresh
keeps the current items on screen. If the place cannot be opened, the previous one comes back with the error. A
search shows the same placeholders, with "Searching…", until its first result.

Path bar: click a crumb to go there (the current folder, the last crumb, refreshes), its chevron lists the
subfolders (drive paths, UNC paths and This PC), a click on the empty area turns it into an address Edit (a path,
a `shell:` name, a `::{CLSID}` or a path inside a .rar).

Command bar, as in Windows 11 Explorer: **New** v (Folder, which is created and then renamed in place, then the
shell's own New submenu: shortcut, text document and so on) | **Cut**, **Copy**, **Paste**, **Rename**, **Share**, **Delete**
| **Sort** v (the listing's columns, the active one checked, then Ascending / Descending), **View** v (Details,
Icons, Thumbnails, then Sidebar, Folder tree and Preview pane, all with checks) | **More** (...), and on the right
a **Preview** toggle. The buttons are enabled from the current state and update on a selection change, a tab
switch, the end of a load and a clipboard change: Cut and Rename need a shell folder (Rename exactly one item),
Copy and Delete also work on search results, Paste needs files on the clipboard, New and Sort are off on Home,
Share needs a selection in a shell folder or on Home. Share opens the Windows share sheet over the window for the
selected files (through `IDataTransferManagerInterop`; the shell's own "Windows.ModernShare" verb refuses a
desktop window); folders cannot be shared, as in Explorer, and a selection without files beeps.
Delete with Shift deletes permanently, as in the list. Cut, Copy, Paste, Rename and Delete run the shell's own
verbs. The old list / grid view segment is gone: the views are in View v and on Ctrl+Shift+1/2/3.

Home page: Home and Quick Access open a page of their own in the list's place, drawn like Explorer's: a "Quick
access" title, a responsive grid of 300x72 folder tiles (40 px icon, name, the parent folder relative to the
profile, a pin glyph on pinned folders only), three pills and a table of files (icon, name with its folder
below, Date modified, Type, Size, and Activity when the shell has any, e.g. "X edited this" on cloud files; column
titles from the shell, so localized), newest access first. The pills filter the table: **Recent** (the recent
files), **Favorites** (files added to Favorites) and **Shared** (files shared with you, the shell's "shared"
group); each tab keeps its own pill (not saved). Empty tables read "No recent files", "No favorites yet" or
"Nothing shared with you". A file the shell lists in two groups (recent and a favorite) is one row with both. It scrolls (wheel and
scroll bar) and follows the theme and the DPI. Hover, click (Ctrl+click toggles), double-click or Enter opens;
right-click shows the shell's context menu, with **Unpin from Home** on pinned tiles and **Pin to Home** on
frequent ones (the shell's own entries, such as add to or remove from Favorites, refresh the page); Up / Down / Home / End move,
Ctrl+A selects the rows, Delete deletes, F5 refreshes, F2 renames in place. The status bar, the preview pane and
the command bar follow its selection. While Home loads,
the list's placeholder rows show as for any folder.

More menu: Folder tree, Pin to Home, Open in Explorer, register / unregister as the folder handler, the settings
folder, About, and **Quick access**: the profile's bookmarks and the cloud-sync roots.

### What a tab can show

- **A shell folder**: filesystem, virtual (This PC, Recycle Bin, Control Panel...), zip files (the shell browses
  them as folders), network shares, `ftp://` URLs where the shell supports them. A filesystem folder the shell
  lists as empty but Windows refuses to read (`C:\System Volume Information`) says so in the status bar: "Access
  denied: <the error> (Ctrl+Shift+E opens it in Explorer)".
- **Home**, where a first run starts: the shell's own Home folder, so the pinned folders (above, as folders sort
  first) and the recent files, with the shell's columns. Enter on a recent file opens it with its default verb; Up
  goes to This PC.
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
| Ctrl+D | Pin the selected folders (or the current folder) to Home |
| Right-click a sidebar row | Unpin from Home (Quick Access folders), Remove bookmark (bookmarks) |
| Ctrl+` | Terminal (Windows Terminal, else cmd) in the current filesystem folder |
| Ctrl+B | Show / hide the sidebar (hides the folder tree when that is showing instead) |
| Alt+P | Show / hide the preview pane (text in a monospace Edit, preview handlers in place, else a thumbnail) |
| Ctrl+Shift+1 / 2 / 3, View menu | Details / icons (96 px) / thumbnails (128 px) |
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
as Explorer does. A second launch hands its folders (and `--search`) to the running window and exits; one
instance runs per settings folder, so a launch with another `%APPDATA%` (the smoke test) runs on its own. A plain
launch restores the session; a launch with folders opens just those. After a crash the process relaunches itself
with `--recover` (at most once per 10 s, so a crash loop stops).

### Files under `%APPDATA%\rexplorer`

`settings.ini` and `themes\*.ini` are written with defaults on first run, then yours to edit. A new profile has
no bookmarks (Quick Access already brings the known folders); one written by an earlier version keeps its start
folder and its Desktop / Downloads / Documents bookmarks (right-click them in the sidebar to remove them). Where
Home does not exist (older Windows), the default start is This PC. Lines starting with `;` or `#` are comments.

```ini
[general]
profile=default                  ; the active profile

[profile default]
start=shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}   ; where a first run (or a launch without a session) starts: Home
theme=dark                       ; a file name in themes\ without .ini
view=details                     ; details | icons | thumbnails (the view segment writes it back here)
show_tree=false                  ; the folder tree instead of the sidebar
show_preview=false
tree_width=220                   ; sidebar (or tree) width, 96-DPI pixels
preview_width=360
bookmark.Repos=C:\src           ; bookmark.<label>=<target>: sidebar Favorites, palette, More > Quick access
```

`themes\<name>.ini` holds one `[theme]` section; missing keys fall back to the built-in dark or light theme
chosen by `dark` (the built-in dark theme shown):

```ini
[theme]
dark=true
mica=true                        ; Mica backdrop where Windows supports it
background=#1E1E1E               ; colors are #RRGGBB; the frame: tabs, header, sidebar, status bar, title bar
surface=#262626                  ; cards and fields: the list, the preview, the path bar, the search field
text=#F2F2F7
secondary_text=#98989D           ; section titles, column titles, the status bar, chevrons
accent=#0A84FF
selection=#21374D                ; selection pills (the built-ins mix 18% accent into the surface)
border=#2F2F2F                   ; kept for themes; nothing draws border lines any more
icon_theme=default               ; optional: a folder under icons\
font=Segoe UI Variable Text
font_size=10                     ; points
```

The built-in light theme is `background=#F5F5F7`, `surface=#FFFFFF`, `text=#1D1D1F`, `secondary_text=#6E6E73`,
`accent=#0A84FF`, `selection=#D3E9FF`. A theme file without `surface` but with `background` uses the background for
the cards (flat, as before). A `light.ini` / `dark.ini` still holding an earlier version's unedited defaults is
rewritten with the current ones at startup; edited files are left alone.

Icon themes: `icons\<name>\` holds PNGs named by extension (`rs.png`, `pdf.png`...) plus `folder.png` and
`file.png` (the fallback for unknown extensions); the details view draws them, at the small-icon size, instead of
the system icons, for folders, archive members and search results alike. A starter set is written to
`icons\default` on first run.

Written by the app: `session.txt` (`tab=` lines, `active=`, `view=`, `tree=` / `sidebar=` / `preview=` as 0 or 1,
the pane widths; saved within 2 s of each change and on exit; a search tab is saved as its folder), `recent.txt`
(palette history), `window.txt` (placement). A first run, or `--profile`, takes view and panes from the profile;
later runs from the session. A session written before the sidebar existed (no `sidebar=`) opens with the sidebar.

The exe carries a Common Controls 6 manifest (`crates/app/build.rs`): icon views and the themed controls need it.
It also carries Windows Explorer's own icon, so the taskbar and the title bar look like Explorer's: `scripts/extract-icon.py` copies the
first icon group of `C:\Windows\explorer.exe` into `crates/app/app.ico` (Microsoft's artwork, for personal use), and `build.rs` compiles
`app.rc` with the newest Windows SDK `rc.exe` it finds and links the result. Without the SDK the build warns and skips the icon.

### Smoke test

`scripts\smoke.ps1` drives the release exe: startup, the 100k-file folder (`%TEMP%\rexplorer-bench`, populated
in about 300 ms from process start), columns, keyboard navigation, live update, the not-found fallback (error,
no Explorer window), the Explorer hotkey, tabs, the single-instance hand-off, first-run settings, `--help`,
placement, the tree (opened from More > Folder tree), the preview, the views, a RAR5 archive (open, enter a
directory, go back up), a search in the 100k folder (typed into the search box), `--search` from the command line,
the look (the sidebar is there, the list card's corner pixels are the frame color and its inside is not, a click
on the Downloads row navigates, Ctrl+B hides the sidebar), Home (a first run without folders shows it, and it is the sidebar's first row), the
shell-built sidebar (a Recycle Bin row in Favorites, a C:\ row under This PC), the loading state (a click on
Network shows its name and "Loading…" within 150 ms, then the emptied list), and pins (Ctrl+D on a temp folder puts
it in Quick Access, its sidebar row's Unpin from Home takes it out; the test always unpins, so Quick Access is left
as found), the command bar (Cut is disabled with no selection; after Home in a temp folder Cut, Copy, Rename and
Delete are enabled; a Copy click enables Paste; a Paste click adds the copy), the Home page (a first run shows
RexHome with tiles and rows and the list hidden; a click on the C:\ row brings the list back) and the View menu
(a View click opens a popup menu that Escape closes), home-open (a file added to Recent with `SHAddToRecentDocs`
under `%LOCALAPPDATA%\rexplorer-smoke`, with an extension registered in HKCU for the check only, is found on Home
and Enter opens it; afterwards it is removed from Home, the Recent shortcut, the keys and the files are deleted),
remove-bookmark (a scratch profile's bookmark row, removed through its popup, leaves the sidebar and settings.ini),
share (Share is enabled with a file selected and its click brings the share sheet to the foreground; Escape
closes it), access-denied (`C:\System Volume Information` shows "Access denied: ..." within 3 s), crash-recovery
(`REXPLORER_CRASH_AFTER_MS`, read only for this, raises an access violation; within 6 s a `--recover` process
shows the same folder) and dnd (a real `SendInput` drag of a file row onto a folder row moves the file; the file
is moved back whatever happens). `-Only <regex>` runs the matching checks. The whole run uses a scratch `APPDATA`,
so the user's settings, window placement and open rexplorer window are never touched. The keyboard, share and dnd
checks need an interactive desktop that lets the window come to the foreground; they report SKIP otherwise.
`smoke.ps1 -Screenshots` also writes `docs\screenshots\dark.png`, `light.png` and `home.png` (rendered with
`PrintWindow`, so nothing else on the screen is captured). home.png shows the machine's own Quick Access; the file
rows and the tile subtitles that are paths are pixelated (1/12 and back, nearest neighbor), at rects Home reports
through WM_APP+0x54.

### Known gaps

- Search results and archive members show placeholders in the icons and thumbnails views, have no preview,
  cannot be dragged, sorted by column or renamed; search results are not refreshed after a delete.
- The context menu on search results needs every selected result in one folder.
- Back from a folder opened out of search results returns to the searched folder, not to the results.
- Search result icons come from the extension (no per-file icons such as an exe's own).
- Only .rar archives get the archive view (zip is the shell's); multi-volume and encrypted .rar are untested.
- The embedded Explorer view keeps Windows' own colors, not the theme.
- Home page: no location-status column; Sort is disabled on Home (tiles keep the shell's order, files are
  newest-accessed first); tile subtitles are computed from the path, not read from the shell. The Shared pill
  relies on the shell's undocumented `System.Home.Grouping` value 5. Home keeps listing a recent file for a while
  after the file and its Recent shortcut are deleted (the shell caches it).
- Pin to Home on a frequent folder and the pinned / frequent split were tried on one machine only; a folder that
  is both frequent and pinned through another tool shows as pinned.
- The sidebar is mouse-only (no keyboard focus). The embedded Explorer view and the command palette keep GDI
  window regions, so their corners are not antialiased. A scroll bar redrawn by the system on hover can show a
  square corner until the next paint.
- Share skips selected rows without a local file (on Home, files shared with you live only in the cloud); with
  none left it beeps.
- `ftp://` browsing goes through the shell's FTP folder (present on Windows 11 25H2, untested against a live
  server); where it is missing the URL shows the not-found error.
- No keyboard context menu; small icons of shell folders resolve on the UI thread.
