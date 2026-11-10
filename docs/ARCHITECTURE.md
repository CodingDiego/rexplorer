# Architecture

An overview for contributors. File and line references are to the tree as of 2026-11-10 and drift; search for the
names if a line has moved. For what the program does, see the README; for features still in flight, see the release
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

The UI thread is a single-threaded apartment with OLE: `ComInit::ole()` at `crates/app/src/main.rs:397`. The
`--desktop-icons` process does the same on its own UI thread (`desktop_icons.rs:280`), and so does the `--desktop`
menu process (`desktop.rs:90`). Worker threads use MTA unless a shell object they need insists on an STA.

Every thread the program starts (production code; test threads are left out):

| Thread | Spawned at | Apartment | What it does |
|---|---|---|---|
| UI | process main | STA + OLE, `main.rs:397` | Window, message loop, all controls, all timers |
| Folder load | `nav.rs:239` | MTA, `nav.rs:240` | Enumerates and sorts a folder; posts `WM_ENUM_DONE` |
| Search | `source.rs:570` | MTA, `source.rs:571` | Walks folders; posts `WM_SEARCH_BATCH` |
| Thumbnails/icons | `thumbs.rs:54` | MTA, `thumbs.rs:55` | One long-lived worker fed by a channel; posts `WM_THUMB_DONE` |
| Sidebar rows | `panes.rs:299` | MTA, `panes.rs:301` | Builds the sidebar rows; posts `WM_SIDEBAR_ROWS` |
| Quick access pin/unpin | `panes.rs:379` | STA, `panes.rs:381` | Runs the shell verb on the Quick access folder (serialized by a lock); posts `WM_QUICK_ACCESS_DONE` |
| Preview fetch | `preview_pane.rs:144` | MTA, `preview_pane.rs:148` | Loads text or an image for the preview pane; posts `PV_READY` |
| Image dimensions | `preview_pane.rs:672` | MTA, `preview_pane.rs:674` | Reads image size; posts `PV_DIMS` |
| Archive member for the preview | `panes.rs:657` | none | Extracts one archive member, then posts `WM_TIMER` (`TIMER_PREVIEW`) so the preview asks again |
| Home icons | `home.rs:680` | MTA, `home.rs:681` | Looks up Home page icons; posts to the Home control |
| Icon export | `views.rs:328` | MTA, `views.rs:329` | Exports system icons for the icons view; posts `WM_ICONS_EXPORTED` |
| Open in terminal | `ui.rs:909` | STA, `ui.rs:910` | `ShellExecute` can be slow; no result |
| Open archive entry | `source.rs:467` | STA, `source.rs:468` | Extracts one entry and opens it; errors go to `WM_WORKER_ERROR` |
| Extract all | `source.rs:406` | none | File I/O only; errors go to `WM_WORKER_ERROR` |
| Old install cleanup | `main.rs:378` | none | `install::cleanup_old` at low priority |
| Packaged menu lookup | `packaged.rs:172` | none | Warms the packaged context-menu registrations once |
| Packaged menu handlers | `packaged.rs:313` | MTA, `packaged.rs:314` | Asks each packaged handler for its menu items; answers return over a channel |
| Hang watchdog | `watchdog.rs:61` (`watchdog::spawn`, called from `main.rs:514`) | none | Posts a heartbeat message to the window each tick and logs a hang to `rexplorer.log` when the UI thread stops answering; ends when the window is gone |
| Update check | `main.rs:540` (`check_updates`) | none | Only with `check_updates=true`; background priority; posts `WM_UPDATE_FOUND` when a newer release exists |
| Mouse and keyboard hook | `desktop.rs:95` (`hook_thread`, `desktop.rs:106`) | none | `--desktop` mode only; low-level hooks with their own message loop |
| Crash test | `main.rs:389` | none | Only when `REXPLORER_CRASH_AFTER_MS` is set |

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
  `Box::from_raw` (see `main.rs:684-698`). The window procedure owns the box once the message is delivered.
- If `PostMessageW` fails (the window is gone), the worker takes the box back and drops it
  (`panes.rs:298-303`, `source.rs:563-570`). A message posted to a window that is destroyed before it is handled
  leaks its box; this is the pointer-in-message robustness gap listed under Known gaps in the README.
- Stale results are dropped, not applied. Each folder load, search, thumbnail batch and listing carries a
  generation number; the worker checks an atomic against it and stops early, and the handler ignores a result with
  an old generation (`nav.rs:203-210`, `thumbs.rs:53-56`, `home.rs:627-630`).
- `thumbs.rs` also drains older `WM_THUMB_DONE` messages with `PeekMessageW` (`views.rs:622`) so a fast scroll does
  not queue a long backlog.

## Messages

Custom messages are `WM_APP + n`. The main window's:

| Message | Value | Sender | Payload (pointer owner) |
|---|---|---|---|
| `WM_ENUM_DONE` | `WM_APP+1` (`main.rs:72`) | Folder load worker | `Box<EnumDone>` (UI thread frees) |
| `WM_SHELL_CHANGE` | `WM_APP+2` (`main.rs:73`) | Shell, via `SHChangeNotifyRegister` | Shell-owned notification; handled in `nav.rs:978` |
| `WM_TREE_SELECT` | `WM_APP+3` (`panes.rs:20`) | Tree control | `Box<Pidl>` |
| `WM_SEARCH_BATCH` | `WM_APP+4` (`source.rs:25`) | Search worker | `Box<SearchBatch>` |
| `WM_BROWSER_NAV` | `WM_APP+5` (`source.rs:27`) | Embedded browser view | `Box<Pidl>` |
| `WM_WORKER_ERROR` | `WM_APP+6` (`source.rs:29`) | Any worker | `Box<String>` |
| `WM_THUMB_DONE` | `WM_APP+7` (`views.rs:16`) | Thumbnail worker | See `views::on_thumb_done` (`views.rs:588`) |
| `WM_SIDEBAR_ROWS` | `WM_APP+8` (`panes.rs:22`) | Sidebar worker | `Box<SidebarRows>` |
| `WM_QUICK_ACCESS_DONE` | `WM_APP+9` (`panes.rs:24`) | Quick access worker | `wparam` is success; no pointer |
| `WM_ICONS_EXPORTED` | `WM_APP+10` (`views.rs:18`) | Icon export worker | none |

Others, owned by one control: `PV_READY` `WM_APP+0x51` and `PV_DIMS` `WM_APP+0x52` (preview pane,
`preview_pane.rs:29-32`); `WM_DESKTOP_MENU`, `WM_REPLAY`, `WM_DESKTOP_KEYMENU` `WM_APP+0x60..0x62` (`desktop.rs`);
`WM_DESK_CHANGE` `WM_APP+0x70` (`desktop_icons.rs:47`); and the accessibility pair `WM_ACC_ITEMS` `WM_APP+0x3F00`
(sent; `lparam` points to a `Tree` on the sender's stack that the control fills) and `WM_ACC_INVOKE`
`WM_APP+0x3F01` (posted; `wparam` is the item index) in `access.rs:24-26`.

`WM_COPYDATA` carries the single-instance hand-off (see below). Controls tell their parent about user actions
with `WM_NOTIFY` and a small struct (`NmTab`, `NmSidebar`, and so on); the parent reads it during the call.

## Timers

All timers are `SetTimer` on the main window, handled in the main window procedure
(`main.rs:720-760`). They exist so work is coalesced and the UI thread does not do it inline.

| Timer | Id | Interval | Purpose |
|---|---|---|---|
| `TIMER_REFRESH` | 1 | 150 to 500 ms (`list.rs`, `nav.rs`, `commands.rs`) | Reload a listing after a change, debounced |
| `TIMER_STATUS` | 2 | 100 ms (`list.rs:53`) | Update the status bar after a selection change |
| `TIMER_SESSION` | 3 | 2000 ms (`nav.rs:1393`) | Save the session after a change |
| `TIMER_PREVIEW` | 4 | 150 ms (`panes.rs:561`) | Load the preview of the selected item |
| `TIMER_TREE` | 5 | 1 ms (`nav.rs:550`) | Sync the folder tree selection after the message loop turns |
| `TIMER_SIDEBAR` | 6 | 1 s, or 30 s when fresh (`panes.rs:322`, `main.rs:607,756`) | Rebuild sidebar rows (drives, pins) |
| `TIMER_SKELETON` | 7 | after a delay (`nav.rs:1181`, `source.rs:556`) | A slow navigation shows the loading skeleton |
| `TIMER_PULSE` | 8 | `PULSE_MS` (`nav.rs:1209`) | Alternate the skeleton's shade |
| `TIMER_SEARCH` | 9 | `profile.search_delay_ms` (`main.rs:634`) | Typing in the search box has paused: start the search |
| `TIMER_HOVER` | 10 | `profile.hover_preview_ms` (`panes.rs:572`) | The pointer rested on an item: prefetch its preview |
| `TIMER_CHANGE` | 11 | debounced (`main.rs:810`) | A shell change notification arrived: reload at most once a second |
| `TIMER_COLUMNS` | 12 | 100 ms (`list.rs:866,1192`) | A column was resized or dragged: remember the layout |

Controls and the desktop-icons window keep a few small timers of their own (for example `ICON_TIMER` and
`RENAME_TIMER` in `home.rs`, `MARQUEE_TIMER` in `list.rs`, `HOVER_TIMER` in `tabs.rs`, `TIMER_RELOAD` in
`desktop_icons.rs`).

## Custom controls

Owner-drawn windows with their own window classes. Each registers its class once and talks to its parent with
`WM_NOTIFY`; each answers the accessibility server through `access.rs`.

`RexTabs` (`tabs.rs`), `RexToolbar` (`toolbar.rs`), `RexCrumbs` (`breadcrumb.rs`, the address bar), `RexSidebar`
(`sidebar.rs`), `RexStatus` (`statusbar.rs`), `RexHome` (`home.rs`, the Home page), `RexPreview`
(`preview_pane.rs`), `RexPalette` (`palette.rs`, the command palette), `RexConnect` (`connect.rs`, the connect
dialog) and `RexOptList` inside `RexFolderOptions` (`folder_options.rs`). The file list is a system list-view
(`ICC_LISTVIEW_CLASSES`, `main.rs:386`) that `list.rs` draws and subclasses. Colors and metrics come from
`theme.rs`; text comes from `i18n.rs`.

## Settings, session and language

- Settings: `%APPDATA%\rexplorer\settings.ini` (profiles) and `themes\<name>.ini`, read and written by
  `crates/core/src/settings.rs` (`settings.rs:359` for the folder, `Settings::load` at `:381`, `save` at `:427`).
  When `APPDATA` is unset it falls back to the temp folder.
- Session: `%APPDATA%\rexplorer\session.txt` (`crates/app/src/session.rs:163`): open tabs, folder views, pane
  visibility and widths. It is saved from the UI thread by `TIMER_SESSION` and read at start or after
  a crash (`--recover`; `crash.rs`).
- Language: `i18n.rs` with the bundled `lang` folder; a `lang\<code>.ini` file in the settings folder overrides it.

## Other mechanisms

- Single instance: `instance.rs`. A named mutex and a window property decide the group; a new launch finds the
  existing `RexplorerMain` window and hands over its arguments with `WM_COPYDATA` (`instance::send`,
  `instance.rs:88`) using `SendMessageTimeoutW`, so a busy first window does not hang the second.
- Elevation: `elevation.rs`. An elevated launch restarts itself under the desktop shell's token and exits, because
  an elevated window would not receive `WM_COPYDATA` from normal launches (UIPI).
- Install and registration: `install.rs`, `register.rs`. Everything is under HKCU. Each change is journalled to
  `install.txt` before it is made, so a failed install rolls back and `--uninstall` reverts exactly what was
  recorded.
- Crash handling: `crash.rs` installs a handler that relaunches with `--recover`.

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
8. The UI thread's first use of the system image list happens before the workers start (`main.rs:388-391`), because
   a worker's first call racing the UI thread's own comes back empty.
9. Modal loops and long calls in a message handler are avoided: accessibility invoke is posted, not sent
   (`access.rs:356`), so a menu or dialog cannot block a screen reader's call.
