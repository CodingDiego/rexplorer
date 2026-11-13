# Architecture

An overview for contributors. File and line references are to the tree as of 2026-11-10 and drift (the 2026-11-13
round moved many lines in `main.rs`; its own additions are named, not numbered); search for the names if a line has
moved. For what the program does, see the README; for features still in flight, see the release
notes.

## Crates

Two crates in one Cargo workspace (`Cargo.toml`).

- `crates/core` (`rexplorer_core`): the headless layer. Shell and COM wrappers (`com.rs`, `folder.rs`, `item.rs`,
  `tree.rs`, `browser.rs`, `menu.rs`, `icons.rs`, `images.rs`, `preview.rs`), file operations and undo (`undo.rs`),
  search (`search.rs`, `usn.rs`, `fuzzy.rs`), settings and language (`settings.rs`, `i18n.rs`), install and
  registration (`install.rs`, `register.rs`), elevation (`elevation.rs`), crash handling (`crash.rs`), change
  notifications (`notify.rs`), drag and drop (`dragdrop.rs`) and the accessibility server (`access.rs`).
  `lib.rs` says "no window code lives here": core has no main window, message loop or timers. `notify.rs` and `access.rs`
  take a window handle from the app.
- `crates/app` (`rexplorer`): the Win32 program. `main.rs` (entry, window class `RexplorerMain`, main window
  procedure, timers), `nav.rs` (tabs, navigation, history, session saving), `list.rs` and `views.rs` (the file
  list and its views), `source.rs` (search, archives, embedded browser views), `panes.rs` (sidebar, tree, preview
  panes and splitters), the custom controls below, `instance.rs` (single instance), `desktop.rs` and
  `desktop_icons.rs` (the `--desktop` and `--desktop-icons` modes).

Rule of thumb: if it needs no window handle, it belongs in core and should be testable without a window.

## Threads and COM apartments

`com::ComInit` (`crates/core/src/com.rs:13`) initializes COM for the thread that makes it and uninitializes on drop.
Three flavours: `sta()` (`com.rs:21`), `mta()` (`com.rs:26`) and `ole()` (`com.rs:31`, STA plus OLE, needed for
drag and drop and the clipboard).

The UI thread is a single-threaded apartment with OLE: `ComInit::ole()` at `crates/app/src/main.rs:406`. The
`--desktop-icons` process does the same on its own UI thread (`desktop_icons.rs:279`), and so does the `--desktop`
menu process (`desktop.rs:90`). Worker threads use MTA unless a shell object they need insists on an STA. The
thumbnail and preview workers use an STA since 1.0.0-rc.1: extracting a library item's thumbnail in an MTA crashed
the process. Apartment-threaded folders (the Libraries root, zip folders) answer `RPC_E_WRONG_THREAD` when the
folder load's MTA object is used from the UI thread, so the UI thread rebinds such a folder itself
(`core/folder.rs` `ShellFolder::adopt`), and libraries and archive folders are sorted on the UI thread
(`nav.rs` `sorts_on_ui_thread`).

Every thread the program starts (production code; test threads are left out):

| Thread | Spawned at | Apartment | What it does |
|---|---|---|---|
| UI | process main | STA + OLE, `main.rs:406` | Window, message loop, all controls, all timers |
| Folder load | `nav.rs` (`navigate`) | MTA | Enumerates and sorts a folder (libraries and archive folders arrive unsorted and are sorted on the UI thread); posts `WM_ENUM_DONE` |
| Search | `source.rs:634` | MTA, `source.rs:635` | Queries the Windows Search index (`core::wsearch`) where it covers the root, walks the rest; posts `WM_SEARCH_BATCH` |
| Thumbnails/icons | `thumbs.rs` (`start`) | STA | One long-lived worker fed by a channel; posts `WM_THUMB_DONE` |
| Sidebar rows | `panes.rs:299` | MTA, `panes.rs:301` | Builds the sidebar rows; posts `WM_SIDEBAR_ROWS` |
| Quick access pin/unpin | `panes.rs:379` | STA, `panes.rs:381` | Runs the shell verb on the Quick access folder (serialized by a lock); posts `WM_QUICK_ACCESS_DONE` |
| Preview fetch | `preview_pane.rs` (`fetch`) | STA | Loads text or an image for the preview pane; posts `PV_READY` |
| Image dimensions | `preview_pane.rs:692` | MTA, `preview_pane.rs:694` | Reads image size; posts `PV_DIMS` |
| Archive member for the preview | `panes.rs:684` | none | Extracts one archive member, then posts `WM_TIMER` (`TIMER_PREVIEW`) so the preview asks again |
| Home icons | `home.rs:680` | MTA, `home.rs:681` | Looks up Home page icons; posts to the Home control |
| Icon export | `views.rs:378` | MTA, `views.rs:379` | Exports system icons for the icons view; posts `WM_ICONS_EXPORTED` |
| Open in terminal | `ui.rs:975` | STA, `ui.rs:976` | `ShellExecute` can be slow; no result |
| Breadcrumb dropdown | `ui.rs:651` (`crumb_dropdown`) | MTA, `ui.rs:653` | Lists a crumb's subfolders in the background; posts `WM_CRUMB_ITEMS` |
| Archive job | `source.rs:422` (`run_job`) | STA, `source.rs:423` | Opens a member with its app, or extracts everything; a missing or wrong password posts `WM_ARCHIVE_PASSWORD`, other errors `WM_WORKER_ERROR` |
| Old install cleanup | `main.rs:387` | none | `install::cleanup_old` at low priority |
| Packaged menu lookup | `packaged.rs:172` | none | Warms the packaged context-menu registrations once |
| Packaged menu handlers | `packaged.rs:313` | MTA, `packaged.rs:314` | Asks each packaged handler for its menu items; answers return over a channel |
| Hang watchdog | `watchdog.rs:61` (`watchdog::spawn`, called from `main.rs:524`) | none | Posts a heartbeat message to the window each tick and logs a hang to `rexplorer.log` when the UI thread stops answering; ends when the window is gone |
| Update check | `main.rs:550` (`check_updates`) | none | Only with `check_updates=true`; background priority; posts `WM_UPDATE_FOUND` when a newer release exists |
| Mouse and keyboard hook | `desktop.rs:95` (`hook_thread`, `desktop.rs:106`) | none | `--desktop` mode only; low-level hooks with their own message loop |
| Jump list | `main.rs:570` (`refresh_jump_list`) | STA, `main.rs:572` | Rewrites the taskbar jump list after a navigation, one at a time |
| Crash test | `main.rs:398` | none | Only when `REXPLORER_CRASH_AFTER_MS` is set |
| Tab hand-off | `nav.rs` `tear_tab` (2026-11-13) | none | A tab dropped on another rexplorer window: `instance::send` with the tab's place; holds only handles and owned strings; a refused tab's place goes into a queue in `nav.rs` and `WM_TAB_RETURN` is posted with no pointer |

Other helper windows run on the UI thread, for example the in-place name editor and the palette; they are not
separate threads. `notify.rs` has no thread of its own: the shell delivers change notifications to the window
registered by `ChangeWatch::register` (`notify.rs:23`) as a window message on the UI thread. The accessibility
server in `access.rs` is called by screen readers from other threads; COM proxies those calls onto the UI thread
(see the test at `access.rs:618`), and the server only reads the control by sending it a message.

Workers named "background priority" call `ui::background_priority` (`ui.rs:51`) so they do not compete with typing.

## How worker results return to the UI thread

A worker never touches a control. It does the slow work, then returns the result with `PostMessageW` to the main
window (the window handle is passed to the worker as an integer, since `HWND` is not `Send`).

- The result is boxed and the raw pointer goes in `lparam`: `Box::into_raw`, then the window procedure does
  `Box::from_raw` (see `main.rs:770-778`). The window procedure owns the box once the message is delivered.
- If `PostMessageW` fails (the window is gone), the worker takes the box back and drops it
  (`panes.rs:304-306`, `source.rs:638`). A message posted to a window that is destroyed before it is handled
  leaks its box; this is the pointer-in-message robustness gap listed under Known gaps in the README.
- Stale results are dropped, not applied. Each folder load, search, thumbnail batch and listing carries a
  generation number; the worker checks an atomic against it and stops early, and the handler ignores a result with
  an old generation (`nav.rs:202-207` and `:242`, `thumbs.rs:113-124`, `home.rs:672-683`).
- `thumbs.rs` also drains older `WM_THUMB_DONE` messages with `PeekMessageW` (`views.rs:703`) so a fast scroll does
  not queue a long backlog.

## Messages

Custom messages are `WM_APP + n`. The main window's:

| Message | Value | Sender | Payload (pointer owner) |
|---|---|---|---|
| `WM_ENUM_DONE` | `WM_APP+1` (`main.rs:73`) | Folder load worker | `Box<EnumDone>` (UI thread frees) |
| `WM_SHELL_CHANGE` | `WM_APP+2` (`main.rs:74`) | Shell, via `SHChangeNotifyRegister` | Shell-owned notification; handled in `nav.rs:1084` |
| `WM_TREE_SELECT` | `WM_APP+3` (`panes.rs:20`) | Tree control | `Box<Pidl>` |
| `WM_SEARCH_BATCH` | `WM_APP+4` (`source.rs:27`) | Search worker | `Box<SearchBatch>` |
| `WM_BROWSER_NAV` | `WM_APP+5` (`source.rs:29`) | Embedded browser view | `Box<Pidl>` |
| `WM_WORKER_ERROR` | `WM_APP+6` (`source.rs:31`) | Any worker | `Box<String>` |
| `WM_THUMB_DONE` | `WM_APP+7` (`views.rs:18`) | Thumbnail worker | See `views::on_thumb_done` (`views.rs:670`) |
| `WM_SIDEBAR_ROWS` | `WM_APP+8` (`panes.rs:22`) | Sidebar worker | `Box<SidebarRows>` |
| `WM_QUICK_ACCESS_DONE` | `WM_APP+9` (`panes.rs:24`) | Quick access worker | `wparam` is success; no pointer |
| `WM_ICONS_EXPORTED` | `WM_APP+10` (`views.rs:20`) | Icon export worker | none |
| `WM_UPDATE_FOUND` | `WM_APP+11` (`main.rs:76`) | Update check worker | `Box<String>`, the newer version |
| `WM_ARCHIVE_PASSWORD` | `WM_APP+12` (`source.rs:417`) | Archive job worker | `Box<(PathBuf, bool, ArchiveJob)>`; the UI thread asks and runs the job again |
| `WM_CRUMB_ITEMS` | `WM_APP+13` (`main.rs:78`) | Breadcrumb dropdown worker | `Box<CrumbItems>` |
| `WM_TAB_RETURN` | `WM_APP+14` (`nav.rs`) | Tab hand-off worker | none; the places of refused tabs wait in a queue (`nav.rs` `take_returned`), and the UI thread drains it and reopens them |
| `WM_HEARTBEAT` | `WM_APP+31` (`watchdog.rs:16`) | Hang watchdog | none; the UI thread answers by stamping the time |

Others, owned by one control: `PV_READY` `WM_APP+0x51` and `PV_DIMS` `WM_APP+0x52` (preview pane,
`preview_pane.rs:31-34`); `WM_DESKTOP_MENU`, `WM_REPLAY`, `WM_DESKTOP_KEYMENU` `WM_APP+0x60..0x62` (`desktop.rs`);
`WM_DESK_CHANGE` `WM_APP+0x70` (`desktop_icons.rs:46`); and the accessibility pair `WM_ACC_ITEMS` `WM_APP+0x3F00`
(sent; `lparam` points to a `Tree` on the sender's stack that the control fills) and `WM_ACC_INVOKE`
`WM_APP+0x3F01` (posted; `wparam` is the item index) in `access.rs:24-26`.

`WM_COPYDATA` carries the single-instance hand-off (see below). Controls tell their parent about user actions
with `WM_NOTIFY` and a small struct (`NmTab`, `NmSidebar`, and so on); the parent reads it during the call.

`WM_COMMAND` runs a command only when it comes from a menu or an accelerator, or is a plain click on one of
rexplorer's toolbars (`main.rs` `is_command`). Child controls also send `WM_COMMAND` notifications: the list's
rename edit has control id 1, the same number as the Back command, and before 1.0.0-rc.1 each character typed in
it ran Back. A rename commit follows the item the edit was opened on, not its row (`list.rs`).

## Timers

All timers are `SetTimer` on the main window, handled in the main window procedure
(`main.rs:811-860`). They exist so work is coalesced and the UI thread does not do it inline.

| Timer | Id | Interval | Purpose |
|---|---|---|---|
| `TIMER_REFRESH` | 1 | 150 to 500 ms (`list.rs`, `nav.rs`, `commands.rs`) | Reload a listing after a change, debounced |
| `TIMER_STATUS` | 2 | 100 ms (`list.rs:53`) | Update the status bar after a selection change |
| `TIMER_SESSION` | 3 | 2000 ms (`nav.rs:1572`) | Save the session after a change |
| `TIMER_PREVIEW` | 4 | 150 ms (`panes.rs:570`) | Load the preview of the selected item |
| `TIMER_TREE` | 5 | 1 ms (`nav.rs:655`) | Sync the folder tree selection after the message loop turns |
| `TIMER_SIDEBAR` | 6 | 1 s, or 30 s when fresh (`panes.rs:323`, `main.rs:684,849`) | Rebuild sidebar rows (drives, pins) |
| `TIMER_SKELETON` | 7 | after a delay (`nav.rs:1345`, `source.rs:627`) | A slow navigation shows the loading skeleton |
| `TIMER_PULSE` | 8 | `PULSE_MS` (`nav.rs:1373`) | Alternate the skeleton's shade |
| `TIMER_SEARCH` | 9 | `profile.search_delay_ms` (`main.rs:711`) | Typing in the search box has paused: start the search |
| `TIMER_HOVER` | 10 | `profile.hover_preview_ms` (`panes.rs:581`) | The pointer rested on an item: prefetch its preview |
| `TIMER_CHANGE` | 11 | debounced (`main.rs:898`) | A shell change notification arrived: reload at most once a second |
| `TIMER_COLUMNS` | 12 | 100 ms (`list.rs:998,1393`) | A column was resized or dragged: remember the layout |

Controls and the desktop-icons window keep a few small timers of their own (for example `ICON_TIMER` and
`RENAME_TIMER` in `home.rs`, `MARQUEE_TIMER` in `list.rs`, `HOVER_TIMER` in `tabs.rs`, `HOLD_TIMER` (press and
hold on Back / Forward, 500 ms) in `toolbar.rs`, `TIMER_RELOAD` in `desktop_icons.rs`).

## Custom controls

Owner-drawn windows with their own window classes. Each registers its class once and talks to its parent with
`WM_NOTIFY`; each answers the accessibility server through `access.rs`.

`RexTabs` (`tabs.rs`), `RexToolbar` (`toolbar.rs`), `RexCrumbs` (`breadcrumb.rs`, the address bar), `RexSidebar`
(`sidebar.rs`), `RexStatus` (`statusbar.rs`), `RexHome` (`home.rs`, the Home page), `RexPreview`
(`preview_pane.rs`), `RexPalette` (`palette.rs`, the command palette), `RexConnect` (`connect.rs`, the connect
dialog) and `RexOptList` inside `RexFolderOptions` (`folder_options.rs`). The file list is a system list-view
(`ICC_LISTVIEW_CLASSES`, `main.rs:407`) that `list.rs` draws and subclasses. Colors and metrics come from
`theme.rs`; text comes from `i18n.rs`.

## Settings, session and language

- Settings: `%APPDATA%\rexplorer\settings.ini` (profiles) and `themes\<name>.ini`, read and written by
  `crates/core/src/settings.rs` (`settings.rs:445` `dir` for the folder, `Settings::load` at `:466`, `save` at `:522`).
  When `APPDATA` is unset it falls back to the temp folder.
- Session: `%APPDATA%\rexplorer\session.txt` (`crates/app/src/session.rs:209`): open tabs, folder views, pane
  visibility and widths. It is saved from the UI thread by `TIMER_SESSION` and read at start or after
  a crash (`--recover`; `crash.rs`).
- Language: `i18n.rs` with the bundled `lang` folder; a `lang\<code>.ini` file in the settings folder overrides it.

## Other mechanisms

- Single instance: `instance.rs`. A named mutex and a window property decide the group; a new launch finds the
  existing `RexplorerMain` window and hands over its arguments with `WM_COPYDATA` (`instance::send`,
  `instance.rs:88`) using `SendMessageTimeoutW`, so a busy first window does not hang the second.
- Elevation: `elevation.rs`. An elevated launch restarts itself under the desktop shell's token and exits, because
  an elevated window would not receive `WM_COPYDATA` from normal launches (UIPI).
- Install and registration: `install.rs`, `register.rs`. Everything is under HKCU. `--install` only installs the
  program (exe, Start menu shortcut, Apps and Features entry) and records it in `install.txt`; `--register`
  (`take_over`) makes rexplorer the default file manager and records that in `takeover.txt`. Each change is journalled
  (a value's prior type and bytes, keys, folders, files) before it is made, so a failure rolls back, and
  `--unregister` / `--uninstall` revert exactly what was recorded (uninstall gives back first).
  `scripts\restore-explorer.ps1` reverts the same records without the exe, with `known_keys()` as its fixed list.
- Crash handling: `crash.rs` installs a handler that relaunches with `--recover`. A native (SEH) crash writes a
  fixed-size `.seh` record without touching the heap (`write_record`); the next start converts finished records into
  `.txt` reports (`convert_seh_records`) before pruning old dumps.
- Outside-observable signals: the main window carries the properties `rexplorer.painted` (tick count of the first
  paint, kept), `rexplorer.loads` (finished folder loads) and `rexplorer.loadtick` (tick of the last load), set by
  `instance::signal_painted` / `signal_loaded`. `scripts\budgets.ps1` reads them; keep them when changing paint or
  load code.
- Undo (`core/undo.rs`): every record carries what proves the item is ours: a file id (volume serial and file
  index, read right after the operation) for rename, new items and moves; for a paste or drop (`Op::Transfer`), the
  destination names taken before the shell ran and each source's size and time. An undo that cannot prove it does
  nothing and beeps.
- Right-to-left layout: `i18n::is_rtl` (a language file's `[meta]` `direction=rtl`) and `crates/app/src/rtl.rs`.
  Anything new that paints into a memory DC calls `rtl::match_dc` after selecting the bitmap; anything that draws
  words passes its DrawText flags through `rtl::text`; a menu anchored on a screen rectangle uses `rtl::menu_x`. The
  helpers must not run before `i18n::init` in `main`, or the table freezes to English.

## Rules that keep the UI thread free

1. Anything that can touch the disk, the network or a shell extension runs on a worker: folder enumeration, icons,
   thumbnails, previews, search, archives, `ShellExecute`, packaged menu handlers. The UI thread only paints and
   applies results.
2. Workers return results with `PostMessageW`, never `SendMessageW`. A sent message would block the worker until
   the UI thread is free and can deadlock with a worker waiting on the UI. The one `SendMessageW` into a control
   (`WM_ACC_ITEMS`) is read-only and is called by COM on the UI thread.
3. Workers never touch windows or controls and never hold UI state. They get plain copies (paths, ids, the window
   handle as an integer, an `Arc` to an atomic generation).
4. Every result carries a generation. Newer work cancels older work by bumping it; workers check often and stop,
   and handlers drop results that no longer match.
5. Use a timer to coalesce bursts (typing, resizing, shell notifications, selection changes) instead of doing the
   work on every message.
6. Pick the apartment on purpose. MTA for workers that only need free-threaded objects; STA where a shell object
   requires it (the Quick access verb, `ShellExecute`, archive opening). Shell objects made on one thread are not
   passed to another; PIDLs are (`com.rs:105`).
7. A worker that does non-urgent work (sidebar, speculative hover previews, cleanup) lowers its priority with
   `background_priority`.
8. The UI thread's first use of the system image list happens before the workers start (`main.rs:409`), because
   a worker's first call racing the UI thread's own comes back empty.
9. Modal loops and long calls in a message handler are avoided: accessibility invoke is posted, not sent
   (`access.rs:368`), so a menu or dialog cannot block a screen reader's call.
