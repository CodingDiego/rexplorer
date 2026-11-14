# Architecture

An overview for contributors. File and line references are to the tree as of 2026-11-14 and drift; search for the
names if a line has moved. For what the program does, see the README; for features still in flight, see the release
notes.

## Crates

Two crates in one Cargo workspace (`Cargo.toml`).

- `crates/core` (`rexplorer_core`): the headless layer. Shell and COM wrappers (`com.rs`, `folder.rs`, `item.rs`,
  `tree.rs`, `browser.rs`, `menu.rs`, `icons.rs`, `images.rs`, `preview.rs`), file operations and undo (`undo.rs`),
  search (`search.rs`, `usn.rs`, `fuzzy.rs`), settings and language (`settings.rs`, `i18n.rs`), install and
  registration (`install.rs`, `register.rs`), elevation (`elevation.rs`), crash handling (`crash.rs`), change
  notifications (`notify.rs`), drag and drop (`dragdrop.rs`) and the accessibility server (`access.rs`).
  `lib.rs` says "No window code lives here". In production code core registers no window class and has no window
  procedure, message loop or timer of its own. It still works on windows the app hands it, because the shell
  objects it wraps need one:
  - `tree.rs` hosts the shell's NamespaceTreeControl, `browser.rs` an `IExplorerBrowser` and `preview.rs` an
    `IPreviewHandler` (`handler.SetWindow`), each inside a window of the app.
  - `menu.rs` shows a shell context menu with `TrackPopupMenuEx` on the app's window (a modal menu loop) and
    answers the owner-drawn and submenu messages the app forwards to `ContextMenu::handle_message`.
  - `dragdrop.rs` registers app windows as drop targets (`RegisterDragDrop`) and runs `SHDoDragDrop`, a modal
    drag loop.
  - `notify.rs` registers an app window for shell change notifications (`ChangeWatch::register`,
    `SHChangeNotifyRegister`).
  - `access.rs` answers `WM_GETOBJECT` for a control (`Server::answer`), sends the control `WM_ACC_ITEMS`
    through `send_own`, and posts it `WM_ACC_INVOKE`.
- `crates/app` (`rexplorer`): the Win32 program. `main.rs` (entry, window class `RexplorerMain`, main window
  procedure, timers), `nav.rs` (tabs, navigation, history, session saving), `list.rs` and `views.rs` (the file
  list and its views), `thumbs.rs` (the thumbnail worker), `mailbox.rs` (worker results), `watchdog.rs` (hang
  watchdog and message timing), `source.rs` (search, archives, embedded browser views), `archive_view.rs`
  (archive listings and member extraction), `panes.rs` (sidebar, tree, preview panes and splitters), the custom
  controls below, `instance.rs` (single instance), `session.rs`, `desktop.rs` and `desktop_icons.rs` (the
  `--desktop` and `--desktop-icons` modes).

Rule of thumb: if it needs no window handle, it belongs in core and should be testable without a window. Core may
take a window handle from the app when a shell object needs one; it never owns a window class or a message loop.

## Threads and COM apartments

`com::ComInit` (`crates/core/src/com.rs:13`) initializes COM for the thread that makes it and uninitializes on drop.
Three flavours: `sta()` (`com.rs:21`), `mta()` (`com.rs:26`) and `ole()` (`com.rs:31`, STA plus OLE, needed for
drag and drop and the clipboard).

The UI thread is a single-threaded apartment with OLE: `ComInit::ole()` at `crates/app/src/main.rs:423`. The
`--desktop-icons` process does the same on its own UI thread (`desktop_icons.rs:279`), and so does the `--desktop`
menu process (`desktop.rs:152`). Worker threads use MTA unless a shell object they need insists on an STA. The
thumbnail and preview workers use an STA, because extracting a library item's thumbnail in an MTA crashes the
process (`thumbs.rs:71`, `preview_pane.rs:150`). The shell's FTP folder lists nothing from an MTA, so an `ftp://`
folder load runs on an STA of its own (`nav.rs:317-322`). Apartment-threaded folders (the Libraries root, zip folders)
answer `RPC_E_WRONG_THREAD` when the folder load's MTA object is used from the UI thread, so the UI thread rebinds
such a folder itself (`core/folder.rs` `ShellFolder::adopt`), and libraries and archive folders are sorted on the UI
thread (`nav.rs` `sorts_on_ui_thread`).

Every thread the program starts (production code; test threads are left out):

| Thread | Spawned at | Apartment | What it does |
|---|---|---|---|
| UI | process main | STA + OLE, `main.rs:423` | Window, message loop, all controls, all timers |
| Folder load | `nav.rs:321` (`navigate`) | MTA, or STA for an `ftp://` place (`nav.rs:322`) | Enumerates and sorts a folder (libraries and archive folders arrive unsorted and are sorted on the UI thread); an `ftp://` listing is parked (`ShellFolder::park`) before its apartment ends and rebound on the UI thread (`ShellFolder::rebind`); queues the result in `nav::ENUM_DONE` and posts `WM_ENUM_DONE` |
| Search | `source.rs:722` | MTA, `source.rs:723` | Queries the Windows Search index (`core::wsearch`) where it covers the root, walks the rest; queues batches in `source::SEARCH_BATCH` and posts `WM_SEARCH_BATCH` |
| Thumbnails/icons | `thumbs.rs:70` (`start`) | STA, `thumbs.rs:73` | One long-lived worker fed by a channel; queues bitmaps in `thumbs::DONE` and posts `WM_THUMB_DONE` |
| Sidebar rows | `panes.rs:310` (`refresh_sidebar`) | MTA, `panes.rs:312` | Builds the sidebar rows and their icons (`sidebar_rows`); queues them in `panes::SIDEBAR_ROWS` and posts `WM_SIDEBAR_ROWS` |
| Quick access pin/unpin | `panes.rs:394` (`quick_access_verb`) | STA, `panes.rs:396` | Runs the shell verb on the Quick access folder (serialized by a lock); posts `WM_QUICK_ACCESS_DONE` |
| Preview fetch | `preview_pane.rs:146` (`fetch`) | STA, `preview_pane.rs:152` | Loads text or an image for the preview pane; queues the path in `READY` and posts `PV_READY` |
| Image dimensions | `preview_pane.rs:703` | MTA, `preview_pane.rs:705` | Reads image size; queues it in `DIMS` and posts `PV_DIMS` |
| Archive member for the preview | `panes.rs:716` (`member_subject`) | none | Extracts one archive member, then posts `WM_TIMER` (`TIMER_PREVIEW`) so the preview asks again |
| Home icons | `home.rs:706` | MTA, `home.rs:707` | Looks up Home page icons; queues them in the Home control's `ICONS` and posts it `HM_ICON` |
| Icon export | `views.rs:446` (`start_icon_export`) | MTA, `views.rs:447` | Exports system icons for the icons view; posts `WM_ICONS_EXPORTED` |
| Open in terminal | `ui.rs:1031` (`open_terminal`) | STA, `ui.rs:1032` | `ShellExecute` can be slow; no result |
| Breadcrumb dropdown | `ui.rs:691` (`crumb_dropdown`) | MTA, `ui.rs:693` | Lists a crumb's subfolders in the background; queues them in `ui::CRUMB_ITEMS` and posts `WM_CRUMB_ITEMS` |
| Archive job | `source.rs:495` (`run_job`) | STA, `source.rs:497` | Opens a member with its app, or extracts everything; a missing or wrong password goes to `source::ARCHIVE_PASSWORD` with `WM_ARCHIVE_PASSWORD`, other errors to `source::WORKER_ERROR` with `WM_WORKER_ERROR`; counted in `source::JOBS` |
| Old install cleanup | `main.rs:404` | none | `install::cleanup_old` at background priority |
| Packaged menu lookup | `packaged.rs:172` | none | Warms the packaged context-menu registrations once |
| Packaged menu handlers | `packaged.rs:313` | MTA, `packaged.rs:314` | Asks each packaged handler for its menu items; answers return over a channel |
| Hang watchdog | `watchdog.rs:104` (`watchdog::spawn`, called from `main.rs:542`) | none | Posts `WM_HEARTBEAT` to the window every second and logs a hang to `rexplorer.log` when the UI thread stops answering; ends when the window is gone |
| Update check | `main.rs:577` (`check_updates`) | none | Only with `check_updates=true`; background priority; queues the version in `UPDATE_FOUND` and posts `WM_UPDATE_FOUND` when a newer release exists |
| Mouse and keyboard hook | `desktop.rs:157` (`hook_thread`, `desktop.rs:168`) | none | `--desktop` mode only; low-level hooks with their own message loop |
| Jump list | `main.rs:597` (`refresh_jump_list`) | STA, `main.rs:599` | Rewrites the taskbar jump list after a navigation, one at a time |
| Crash test | `main.rs:415` | none | Only when `REXPLORER_CRASH_AFTER_MS` is set |
| Tab hand-off | `nav.rs:1614` (`tear_tab`) | none | A tab dropped on another rexplorer window: `instance::send` with the tab's place; holds only handles and owned strings; a refused tab's place goes into the `RETURNED` queue in `nav.rs` and `WM_TAB_RETURN` is posted with no pointer |

Other helper windows run on the UI thread, for example the in-place name editor and the palette; they are not
separate threads. `notify.rs` has no thread of its own. The shell delivers change notifications to the window
registered by `ChangeWatch::register` (`notify.rs:20`) as a window message on the UI thread. The accessibility
server in `access.rs` is called by screen readers from other threads. COM proxies those calls onto the UI thread
(see the test at `access.rs:672`), and the server only reads the control by sending it a message.

Workers named "background priority" call `ui::background_priority` (`ui.rs:51`) so they do not compete with typing.

### The `App` state and re-entrancy

`App` is one `Box` made with `Box::into_raw` in `main` (`main.rs:466`) and never freed. The window procedure reaches
it through the `*mut App` kept in `GWLP_USERDATA`. Only the UI thread touches it, and there is no re-entrancy guard.
Many shell calls pump messages while they run. Among them are context menus and their `InvokeCommand`, shell
details and icons, dialogs, password prompts, and drag and drop. A handler inside one of them can be entered again,
and a reload, a search batch or a tab change can replace or drop the listing it was reading. The code stays safe by
working from copies and finding its row again afterwards:

- `source::row_text` and `source::small_icon` copy the row's item and the folder's PIDL, make the shell calls with
  no borrow of the listing held, and keep the result only when `source::same_row` says the active tab still shows
  that folder and row `i` still holds that item (`same_pidl` compares the PIDL bytes).
- A rename commit finds its row with `list::edit_row`. That is the row it was opened on when it still holds the
  same parsing name, else wherever that name is now, else none.
- A delete in search results takes `source::search_picks` (row and parsing name) before the shell runs and
  `source::prune_search` finds each row again by name afterwards.
- `nav::on_enum_done` finds its tab again with `find_tab` after a password prompt.
- `list::show_menu` returns at once while `App::active_menu` holds a menu. A previous menu's command may still be
  running inside `InvokeCommand`, and its menu object must outlive it.

## How worker results return to the UI thread

A worker never touches a control. It does the slow work, then hands the result to the window that asked for it (the
window handle is passed to the worker as an integer, since `HWND` is not `Send`).

- No posted message carries a pointer, because any program on the desktop can post one. The payload goes into a
  `mailbox::Mailbox` (a mutex-guarded queue per message, keyed by window), and the message is posted with
  `lParam` 0. The window procedure takes one payload per message with `Mailbox::take(hwnd)` (`main.rs:808-860`).
  A forged message can at most deliver early what a worker already queued.
- `Mailbox::post` queues the payload before it posts, so the message never arrives before its payload. When
  `PostMessageW` fails (the window is gone or its queue is full) it removes the payload again and returns
  `Err(value)`, so the worker can free it (`panes.rs:315-319` destroys the sidebar icons, `source.rs:512` shows the
  error with no owner). `post_while` also gives the payload back when a condition is false once it is queued; the
  thumbnail and Home icon workers use it with their generation check.
- `WM_DESTROY` calls `forget` on every mailbox of the main window (`main.rs:976-985`), and `views::destroy` deletes
  the bitmaps still queued (`thumbs::forget`), so nothing queued for a destroyed window is kept.
- Messages with no payload (`WM_QUICK_ACCESS_DONE`, `WM_ICONS_EXPORTED`, `WM_TAB_RETURN`, `WM_HEARTBEAT`) are
  posted with `PostMessageW` directly; `WM_QUICK_ACCESS_DONE` carries its success in `wParam`.
- Stale results are dropped, not applied. Each folder load, search, thumbnail request, Home icon batch and
  breadcrumb listing carries a generation number. The worker checks an atomic against it and stops early, and the
  handler ignores a result with an old generation (`nav.rs:323` and `:337`, `nav::on_enum_done` at `nav.rs:500`,
  `source::on_search_batch`, `thumbs.rs:93` and `:115`, `home.rs:709` and `:715`, `ui::on_crumb_items`).

The thumbnail worker (`thumbs::start`) is one STA thread fed by a channel. It pumps messages while it waits, and
each request posts it a `WM_NULL` wake-up. It skips a request whose generation is older than the latest
(`Thumbs::invalidate` on navigation) or whose item index is outside the window the list last set
(`Thumbs::set_window`, the rows on screen plus a lookahead). It fits the bitmap to the requested size and draws the
shell's overlay on it, then queues a `thumbs::Done` with `post_while`, which gives the bitmap back for deletion when
the generation moved on meanwhile. `views::on_thumb_done` takes one payload per `WM_THUMB_DONE`. It deletes a bitmap
whose listing token (the high 32 bits of its id) is not `views.token`, which `views::reset` bumps for every new
listing or view. Otherwise it adds the bitmap to the image list when the row's slot still waits for it
(`REQUESTED`), keeps it in the image cache, and redraws that row. Dropping `Thumbs` bumps its generation and closes
the channel without joining the worker, so a worker inside a slow shell call never stalls the UI thread.

## Messages

Custom messages are `WM_APP + n`. The main window's:

| Message | Value | Sender | Payload |
|---|---|---|---|
| `WM_ENUM_DONE` | `WM_APP+1` (`main.rs:78`) | Folder load worker | `Box<EnumDone>` in `nav::ENUM_DONE` |
| `WM_SHELL_CHANGE` | `WM_APP+2` (`main.rs:79`) | Shell, via `SHChangeNotifyRegister` | Shell-owned notification, read by `ChangeWatch::acknowledge` in `main.rs:882` |
| `WM_TREE_SELECT` | `WM_APP+3` (`panes.rs:20`) | Tree control callback | `Box<Pidl>` in `panes::TREE_SELECT` |
| `WM_SEARCH_BATCH` | `WM_APP+4` (`source.rs:28`) | Search worker | `Box<SearchBatch>` in `source::SEARCH_BATCH` |
| `WM_BROWSER_NAV` | `WM_APP+5` (`source.rs:31`) | Embedded browser view callback | `Box<Pidl>` in `source::BROWSER_NAV` |
| `WM_WORKER_ERROR` | `WM_APP+6` (`source.rs:34`) | Archive job worker | `String` in `source::WORKER_ERROR` |
| `WM_THUMB_DONE` | `WM_APP+7` (`views.rs:18`) | Thumbnail worker | `thumbs::Done` in `thumbs::DONE`; see `views::on_thumb_done` (`views.rs:762`) |
| `WM_SIDEBAR_ROWS` | `WM_APP+8` (`panes.rs:23`) | Sidebar worker | `Box<SidebarRows>` in `panes::SIDEBAR_ROWS` |
| `WM_QUICK_ACCESS_DONE` | `WM_APP+9` (`panes.rs:26`) | Quick access worker | `wParam` is success; no payload |
| `WM_ICONS_EXPORTED` | `WM_APP+10` (`views.rs:20`) | Icon export worker | none |
| `WM_UPDATE_FOUND` | `WM_APP+11` (`main.rs:81`) | Update check worker | `String`, the newer version, in `UPDATE_FOUND` |
| `WM_ARCHIVE_PASSWORD` | `WM_APP+12` (`source.rs:482`) | Archive job worker | `Box<(PathBuf, bool, ArchiveJob)>` in `source::ARCHIVE_PASSWORD`; the UI thread asks and runs the job again |
| `WM_CRUMB_ITEMS` | `WM_APP+13` (`main.rs:84`) | Breadcrumb dropdown worker | `CrumbItems` in `ui::CRUMB_ITEMS` |
| `WM_TAB_RETURN` | `WM_APP+14` (`nav.rs:1588`) | Tab hand-off worker | none; the places of refused tabs wait in a queue (`nav.rs` `take_returned`), and the UI thread drains it and reopens them |
| `WM_HEARTBEAT` | `WM_APP+31` (`watchdog.rs:18`) | Hang watchdog | none; the UI thread answers with `watchdog::beat` |

Others, owned by one control: `PV_READY` `WM_APP+0x51` and `PV_DIMS` `WM_APP+0x52` (preview pane, payloads in its
`READY` and `DIMS` mailboxes, `preview_pane.rs:31-36`); `HM_ICON` `WM_APP+0x50` (Home control, payloads in `ICONS`,
`home.rs:63-65`); `WM_DESKTOP_MENU`, `WM_REPLAY`, `WM_DESKTOP_KEYMENU`, `WM_DESKTOP_WIN_E` `WM_APP+0x60..0x63`
(`desktop.rs:43-49`); `WM_DESK_CHANGE` `WM_APP+0x70` (`desktop_icons.rs:46`); and the accessibility pair
`WM_ACC_ITEMS` `WM_APP+0x3F00` (sent; `lParam` points to a `Tree` on the sender's stack that the control fills) and
`WM_ACC_INVOKE` `WM_APP+0x3F01` (posted; `wParam` is the item index) in `access.rs:24-26`.

Two kinds of private message still carry a pointer in `lParam`. They are `WM_ACC_ITEMS` and the `WM_NOTIFY` a
rexplorer control sends its parent. Both are sent with `access::send_own` (`access.rs:80`), which records the
window, message id and `lParam` in a thread-local while its `SendMessageW` runs. A handler reads the pointer only when
`access::own_send(hwnd, msg, lparam)` (`access.rs:93`) matches all three. A send to a window of the same thread is a
direct call, so the mark is still set there; a message posted or sent by another thread or program is not marked.
Every control's `WM_ACC_ITEMS` handler is guarded this way, and the main window's `on_notify` (`main.rs:1002`) reads
a control's structure (`NmTab`, `NmSidebar`, `NmHome` and so on) only after `own_send`. A `WM_NOTIFY` that fails the
check is passed to `list::on_notify` when it comes from the list (comctl32's own notifications), and ignored
otherwise.

`WM_COPYDATA` carries the single-instance hand-off (see below).

`WM_COMMAND` runs a command only when it comes from a menu or an accelerator, or is a plain click on one of
rexplorer's toolbars (`main.rs` `is_command`). Child controls also send `WM_COMMAND` notifications: the list's
rename edit has control id 1, the same number as the Back command, and before 1.0.0-rc.1 each character typed in
it ran Back. A rename commit follows the item the edit was opened on, not its row (`list::edit_row`).

## Timers

All timers are `SetTimer` on the main window, handled in the main window procedure
(`main.rs:892-935`), which kills each timer as it fires. They exist so work is coalesced and the UI thread does not
do it inline.

| Timer | Id | Interval | Purpose |
|---|---|---|---|
| `TIMER_REFRESH` | 1 | 150, 300 or 500 ms (`list.rs`, `commands.rs:209`, `nav.rs:1226`) | Reload a listing after a change, debounced; `nav::refresh` waits 500 ms more while a rename edit is open |
| `TIMER_STATUS` | 2 | 100 ms (`list.rs:53`) | Update the status bar after a selection change |
| `TIMER_SESSION` | 3 | 2000 ms (`nav.rs:1793`) | Save the session when `nav::session_changed` finds a save less than 2 s ago |
| `TIMER_PREVIEW` | 4 | 150 ms (`panes.rs:602`) | Load the preview of the selected item; the archive member worker also posts it as a `WM_TIMER` (`panes.rs:721`) |
| `TIMER_TREE` | 5 | 1 ms (`nav.rs:809`) | Create the folder tree (`panes::ensure_tree`) once the first listing is on screen |
| `TIMER_SIDEBAR` | 6 | 1 ms when the rows are over 5 s old on activation, else 30 s (`main.rs:720`, `:931`); 1 s after a pin or unpin (`panes.rs:332`) | Rebuild sidebar rows (drives, pins) while the window is active |
| `TIMER_SKELETON` | 7 | `SKELETON_DELAY` (`nav.rs:1521`), 120 ms for a search (`source.rs:715`) | A slow navigation or a search with no results yet shows the loading skeleton |
| `TIMER_PULSE` | 8 | `PULSE_MS` (`nav.rs:1549`) | Alternate the skeleton's shade |
| `TIMER_SEARCH` | 9 | `profile.search_delay_ms` (`main.rs:747`) | Typing in the search box has paused: start the search |
| `TIMER_HOVER` | 10 | `profile.hover_preview_ms` (`panes.rs:613`) | The pointer rested on an item. Prefetch its preview, only while the window is in front |
| `TIMER_CHANGE` | 11 | 150 ms after the first notification (`main.rs:888`, `arm_change` at `main.rs:996`) | A shell change notification arrived. Reload at most once a second (`change_gate`), and again 500 ms later while a change reload is still running |
| `TIMER_COLUMNS` | 12 | 100 ms (`list.rs:1030`, `:1483`) | A column was resized or dragged: remember the layout |
| `TIMER_TREE_SYNC` | 13 | 1 ms (`nav.rs:769`) | Select the folder just shown in the folder tree (`panes::sync_tree`) after the listing has painted, so navigation never waits for the tree |

Controls and the desktop-icons window keep a few small timers of their own (for example `ICON_TIMER` and
`RENAME_TIMER` in `home.rs`, `MARQUEE_TIMER` in `list.rs`, `HOVER_TIMER` in `tabs.rs`, `HOLD_TIMER` (press and
hold on Back / Forward) in `toolbar.rs`, `TIMER_RELOAD` in `desktop_icons.rs`).

## Custom controls

Owner-drawn windows with their own window classes. Each registers its class once. Those that tell their parent
about user actions with `WM_NOTIFY` (`RexTabs`, `RexCrumbs`, `RexSidebar`, `RexHome`, `RexPalette`) send it through
`access::send_own`. The tabs, toolbar, crumbs, sidebar, status bar, Home, preview and palette answer the
accessibility server through `access.rs` (`WM_ACC_ITEMS`).

`RexTabs` (`tabs.rs`), `RexToolbar` (`toolbar.rs`), `RexCrumbs` (`breadcrumb.rs`, the address bar), `RexSidebar`
(`sidebar.rs`), `RexStatus` (`statusbar.rs`), `RexHome` (`home.rs`, the Home page), `RexPreview`
(`preview_pane.rs`), `RexPalette` (`palette.rs`, the command palette), `RexConnect` (`connect.rs`, the connect
dialog) and `RexOptList` inside `RexFolderOptions` (`folder_options.rs`). The file list is a system list-view
(`ICC_LISTVIEW_CLASSES`, `main.rs:424`) that `list.rs` draws and subclasses. Colors and metrics come from
`theme.rs`; text comes from `i18n.rs`.

## Settings, session and language

- Settings: `%APPDATA%\rexplorer\settings.ini` (profiles) and `themes\<name>.ini`, read and written by
  `crates/core/src/settings.rs` (`settings.rs:455` `dir` for the folder, `Settings::load` at `:490`, `save` at
  `:546`). When `APPDATA` is unset it falls back to the temp folder. `Settings::ensure_files` creates missing
  default files with `create_new` and rewrites an existing one only to migrate it (a stamped `format=`, or a
  built-in theme still holding an earlier version's defaults). The app changes single keys of a profile section
  with `views::save_profile_key`, `remove_profile_key` and `try_save_profile_keys`.
- Profile files are written with `settings::write_atomic` (`settings.rs:461`). The text goes to a temp file beside
  the target (named with the process id), which is then renamed over it, so a crash or a full disk leaves the old
  file. A target that is a symbolic link is written through with a plain write, so the link stays. Its callers are
  `Settings::save_to`, `Settings::ensure_files` (migrations), the `views.rs` key writers, `session::save`
  (`session.txt` and `folders.txt`), `save_placement` (`window.txt`, `main.rs:1239`) and `Recent::save`
  (`recent.txt`, `fuzzy.rs:88`).
- Session: `%APPDATA%\rexplorer\session.txt` (`crates/app/src/session.rs:224`) holds the open tabs (passwords
  removed by `connect::redact`, a name with a line break left out), the active tab, the view, pane visibility and
  widths.
  Folder views (columns, sort, view per folder) go to `folders.txt` beside it (`session::folders_path`). A change
  calls `nav::session_changed`, which saves at once or, when a save happened less than 2 s ago, arms
  `TIMER_SESSION`. `WM_DESTROY` and `WM_ENDSESSION` (sign-out and shutdown, which end the process without a
  `WM_DESTROY`) save the window placement and the session (`main.rs:949-960`). A `--new-window` window
  (`App::secondary`) saves neither. The session is read at start, and on `--recover` after a crash (see Crash
  handling below).
- Language: `i18n.rs` with the bundled `lang` folder; a `lang\<code>.ini` file in the settings folder overrides it.

## Other mechanisms

- Single instance: `instance.rs`. `instance::claim` creates the named mutex
  `Local\rexplorer-single-instance-<group>`, where `group` is a hash of the lowercased settings folder
  (`settings::dir`). A launch with another `%APPDATA%` therefore runs beside the user's window. `instance::mark`
  tags the main window with the `RexplorerGroup` property, and `find_main_window` looks for a `RexplorerMain`
  window of the same group. A launch that does not get the mutex hands its arguments to that window with
  `WM_COPYDATA` (`instance::send`, `instance.rs:171`, tagged with `COPYDATA_MAGIC`). It waits with
  `SendMessageTimeoutW` and `SMTO_ABORTIFHUNG` for 3 s; when that times out and the window is not hung
  (`IsHungAppWindow`), it waits up to 10 s more with `SMTO_NORMAL`, so a second launch waits at most 13 s. The
  receiver answers 1 when it took the hand-off; any other outcome opens a window of the launch's own.
  `--new-window` takes no mutex, and `--recover` never hands off.
- Elevation: `elevation.rs`. An elevated launch restarts itself under the desktop shell's token and exits, because
  an elevated window would not receive `WM_COPYDATA` from normal launches (UIPI). `--elevated` keeps the token,
  except for `--desktop-icons` (`main.rs:369`).
- Install and registration: `install.rs`, `register.rs`. Everything is under HKCU. `--install` only installs the
  program (exe, Start menu shortcut, Apps and Features entry) and records it in `install.txt`; `--register`
  (`take_over`) makes rexplorer the default file manager and records that in `takeover.txt`. Each change is journalled
  (a value's prior type and bytes, keys, folders, files) before it is made, so a failure rolls back, and
  `--unregister` / `--uninstall` revert exactly what was recorded (uninstall gives back first).
  `scripts\restore-explorer.ps1` reverts the same records without the exe, with `known_keys()` as its fixed list.
- Crash handling, per process:
  - The main window process calls `crash::install(&["--recover"])` (`main.rs:410`). It turns finished `.seh`
    records into `.txt` reports (`convert_seh_records`), prunes old dumps, names this run's dump, opens this run's
    `.seh` file, and installs an unhandled-exception filter and a panic hook. On a crash (`on_crash`) it first
    relaunches with `--recover`, then writes the fixed-size `.seh` record without touching the heap
    (`write_record`) and a minidump to `%LOCALAPPDATA%\rexplorer\crashes`. A panic also writes a text report beside
    the dump. At a clean exit `crash::finish` deletes the empty `.seh` record.
  - Before that, `main` sets a panic hook that logs the panic to `rexplorer.log` (`crash::log`) and aborts
    (`main.rs:373`). `crash::install` replaces it, so it stays the hook of the `--desktop` and `--desktop-icons`
    helpers and of a `--new-window` window, which have no crash relaunch.
  - A `--recover` start counts itself first (`crash::record_recovery`, `crash-count.txt`). The second one within
    `CRASH_GUARD_SECS` (60 s) opens the active saved tab's folder in Windows Explorer instead of running. A start
    without `--recover` or `--new-window` resets the count. After a recovery with dumps on disk, the window offers
    to open the crashes folder.
  - `nav::note_loading` records the places tabs are loading, one per line with passwords removed, through
    `crash::save_state` (`last-folder.txt`), each time that list changes. On `--recover`, `open_startup_tabs` calls
    `nav::skip_crashed`, and `session::recover_tabs` opens Home instead of a saved tab whose place was loading when
    the app crashed.
- Exit: after the message loop, `main` drops the instance mutex, calls `crash::finish`, then `source::wait_for_jobs`
  (`main.rs:567-570`). That waits until `source::JOBS`, the count of running archive jobs, is 0, so an extraction is
  not cut short. The window is already destroyed, so nothing shows meanwhile; an error from a job that ends then is
  shown in a message box with no owner (`source.rs:511-514`).
- Folder change watch: `nav::show_source` registers `ChangeWatch::register(hwnd, &l.folder, WM_SHELL_CHANGE)` for
  a shell listing each time it is shown, after its load has landed, dropping the old registration first
  (`nav.rs:754-760`). `WM_SHELL_CHANGE` reads the changed items with `ChangeWatch::acknowledge`, drops their cached
  images (`views::forget_changed`), marks the tabs they concern (`nav::mark_changed`), and arms `TIMER_CHANGE`
  when the active tab is one. A change that arrives while the window is minimized reloads once when it is
  restored.
- Hang watchdog: `watchdog.rs`. The watchdog thread posts `WM_HEARTBEAT` every second, and the window answers with
  `watchdog::beat`, which counts in `BEATS`. When the count has not moved for `HANG_AFTER` (5 s) the thread logs
  one line to `rexplorer.log` naming the current `watchdog::op` label and the message being dispatched, and one
  line when the thread answers again. A step counts as at most two ticks, so a system sleep is no hang. A handler
  names a slow operation with the guard `watchdog::op("...")` (for example "navigate", "context menu"). The main
  loop runs every dispatch and accelerator through `watchdog::timed`, which times it. `watchdog::modal`, called on
  `WM_ENTERSIZEMOVE`, `WM_ENTERMENULOOP` and `WM_ENTERIDLE`, ends the timed span where a modal loop begins. Modal
  loops dispatch the heartbeat too, so an open menu or dialog is not a hang.
- Outside-observable signals: the main window carries the properties `rexplorer.painted` (tick count of the first
  paint, kept), `rexplorer.loads` (finished folder loads) and `rexplorer.loadtick` (tick of the last load), set by
  `instance::signal_painted` / `signal_loaded`, and `rexplorer.slowest` / `rexplorer.slowestmsg` (the slowest
  UI-thread message so far, in ms, and which one), set by `instance::signal_slowest` from `watchdog::timed`.
  `scripts\budgets.ps1` reads them; keep them when changing paint, load or message loop code.
- Sidebar: `panes::refresh_sidebar` builds the rows on a worker (`panes::sidebar_rows`), one build at a time.
  Favorites lists the profile's bookmarks, then Quick Access's pinned folders, then its frequent folders, in the
  shell's order and names, then the Recycle Bin unless Quick Access pinned it. Quick Access's recent files are left
  out. `source::named_folder`
  keeps only items that are folders and not streams, since the shell calls a `.zip` a folder too. A row that Quick
  Access has pinned carries a pin mark (`sidebar::Row::pinned`). Right-click (`panes::sidebar_menu`) offers "Unpin from
  Home" on a pinned folder, "Pin to Home" and "Remove from Home" on a frequent one, and "Remove bookmark" on a
  bookmark; the Quick Access verbs run on a worker (`quick_access_verb`).
- Archives: `archive_view::open` lists an archive and records whether it is solid (`archive::is_solid`). A member
  thumbnail is extracted on the thumbnail worker (`Thumbs::request_member`, under `archive_view::show_dir`) only
  for a picture within `archive_view::MEMBER_MAX` (32 MiB, `showable`) and only while the folder view stays within
  `archive_view::VIEW_MAX` (256 MiB, `thumbnail_fits`, counted in `views.member_bytes` and reset with each
  listing). A solid archive never extracts for a thumbnail. Every other member gets its type's icon.
- The `--desktop` helper: `desktop.rs`. One runs per session (the mutex `Local\rexplorer-desktop`), with a hidden
  `RexDesktop` window. Its hook thread installs a low-level mouse hook (`WH_MOUSE_LL`), which takes a right-click
  on Explorer's desktop icon list and replays the press to Explorer when it turns into a drag, and a low-level
  keyboard hook (`WH_KEYBOARD_LL`), which takes Shift+F10 and the Menu key while the desktop list has the focus.
  The keyboard hook also takes Win+E with no other modifier (`is_win_e`) in any window and posts
  `WM_DESKTOP_WIN_E`; `open_file_manager` then starts rexplorer with no argument, which opens the start folder or
  hands off to the open window. The hooks only classify and post; requests older than 2 s are dropped (`stale`).
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
   applies results. Known exceptions still read on the UI thread:
   - A details-view small icon and a cell's text (`source::small_icon`, marked `shortcut:`, and
     `source::row_text`).
   - Sorting and property reads of libraries and archive folders (`nav.rs` `sorts_on_ui_thread`, marked
     `shortcut:`).
   - Building the folder tree (`panes::ensure_tree`, deferred by `TIMER_TREE`).
   - The preview subject's size and type (`panes::subject_of`).
   - The paste plan of a context menu, read only when the clipboard holds files (`list::show_menu`).
   - Small local checks: `std::path::Path::exists` in `source::prune_search`, `archive_view::shown` in
     `views::request`.
2. Workers return results with a mailbox and `PostMessageW`, never `SendMessageW`. A sent message would block the
   worker until the UI thread is free and can deadlock with a worker waiting on the UI. No posted message carries
   a pointer. The only sends that carry one (`WM_ACC_ITEMS`, a control's `WM_NOTIFY`) go through
   `access::send_own` on the UI thread and are read only after `access::own_send`.
3. Workers never touch windows or controls and never hold UI state. They get plain copies (paths, ids, the window
   handle as an integer, an `Arc` to an atomic generation).
4. Every result carries a generation. Newer work cancels older work by bumping it; workers check often and stop,
   and handlers drop results that no longer match.
5. Use a timer to coalesce bursts (typing, resizing, shell notifications, selection changes) instead of doing the
   work on every message.
6. Pick the apartment on purpose. MTA for workers that only need free-threaded objects; STA where a shell object
   requires it (the Quick access verb, `ShellExecute`, archive opening, thumbnails, an `ftp://` load). Shell
   objects made on one thread are not passed to another; PIDLs are (`com.rs:134`). A listing made on an STA that
   ends with its thread is parked first (`ShellFolder::park`) and rebound where it lands.
7. A worker that does non-urgent work (sidebar, speculative hover previews, cleanup) lowers its priority with
   `background_priority`.
8. The UI thread's first use of the system image list happens before the workers start (`main.rs:428`), because
   a worker's first call racing the UI thread's own comes back empty.
9. Modal loops and long calls in a message handler are avoided: accessibility invoke is posted, not sent
   (`access.rs:393`), so a menu or dialog cannot block a screen reader's call.
10. A handler that calls into the shell assumes the calls pump messages. It works from copies and finds its row
    again afterwards (see The `App` state and re-entrancy).
