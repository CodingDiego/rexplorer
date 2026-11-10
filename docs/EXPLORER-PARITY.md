# Explorer parity checklist

What Windows File Explorer does, and where rexplorer stands on each point. This file is the backlog: a row
changes status only when the behaviour was proven on the built exe.

Last full review: 2026-11-09, working tree on top of `9781cc2`. The review read the code. Only the rows whose
note gives a date were run on the exe.

2026-11-10, v1 release-candidate round (`ADD/v1-release-candidate`, uncommitted): rows it touched say "v1 round".
Nothing from that round was run on the exe or with a real keyboard or mouse, so no row moved to WORKS.

| Status | Meaning |
|---|---|
| **WORKS** | Proven on the exe (the note says how). |
| **BROKEN** | The code path exists, but it fails when used. A bug. |
| **REPORTED BROKEN, NOT REPRODUCED** | A user saw it fail; driving the exe with window messages passes. Needs a check with a real keyboard and mouse. |
| **UNVERIFIED** | The code path exists and nobody has proven it at runtime. Treat as suspect. |
| **PARTIAL** | Some of it exists; the note says what is absent. |
| **MISSING** | No code. |

"Shell" in a note means rexplorer hands the work to the Windows shell (a verb or a drop target), so the
dialogs and results are the shell's own.

## 1. Rename

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| F2 renames the focused item | **REPORTED BROKEN, NOT REPRODUCED** | `list.rs:595` -> `begin_rename` `list.rs:658` | 2026-11-09, fresh build: an F2 key message opens the edit, it stays open, and Enter renames the file on disk. Passed in a plain folder, in the grouped Downloads view and after starting on Home. Not yet pressed on a physical keyboard. |
| Click the name of a selected item again | **REPORTED BROKEN, NOT REPRODUCED** | `LVS_EDITLABELS` `ui.rs:119`, `list.rs:73` | 2026-11-09: two click messages on the name, more than the double-click time apart, open the edit. Not yet done with a physical mouse. Never starts with single-click-to-open on. |
| F2 on the Home page (tiles and recent files) | WORKS | `home.rs:639` | 2026-11-09: an F2 key message opens the edit on a tile and on a file row; Escape closes it. Key messages, not a physical keyboard. |
| Click the name again on the Home page | UNVERIFIED | `home.rs` button-up and rename timer | Built 2026-11-09. Needs a real mouse. |
| Rename from the context menu and the command bar | UNVERIFIED | `menu.rs:111`, `main.rs:806` | Same `begin_rename` as F2. |
| Rename starts with the name selected, not the extension | WORKS | `list.rs` `stem_len`, `home.rs:652` | 2026-11-09: F2 on `alpha.txt` selects 0..5, on `beta.tar.gz` 0..11, a name without a dot is selected whole. |
| Tab commits and renames the next item | WORKS | comctl32's own label edit | 2026-11-09: two Tab renames in a row on scratch files ended with the right names. Wraps from the last row to the first. Shift+Tab not checked. Not on Home. |
| Rename in search results and inside .rar | PARTIAL | `source.rs`, `list.rs` | Search results: built 2026-11-09, unverified. Archive members: no. |

## 2. Drag and drop

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Drag files out to another program | **BROKEN** | `list.rs:334` `begin_drag`, `dragdrop.rs:54` | [#75](https://github.com/CodingDiego/rexplorer/issues/75): intermittent, target often never gets DragEnter. v1 round: cause still unknown; each drag that applied nothing is logged to `%APPDATA%\rexplorer\rexplorer.log` (outcome, HRESULT, button state, duration; `dragdrop.rs` `drag_note`) for the next real-mouse run. |
| Drag onto a folder row of the same list | UNVERIFIED | `list.rs:350` `register_drop` | Shell drop target. |
| Drop files from another program into the list | UNVERIFIED | `dragdrop.rs:81` `DropForwarder` | List only. |
| Drop onto the Home page, sidebar rows, breadcrumb crumbs, tabs | UNVERIFIED | `list.rs` `register_folder_drops` | Built 2026-11-09; hit tests are unit-tested. Hovering 0.7 s on a tab switches to it. Needs a real drag. v1 round: these drops are recorded for Ctrl+Z (`list.rs` `register_folder_drop`). |
| Drop onto the folder tree | UNVERIFIED | `tree.rs:60` | Whatever the shell tree control does by itself. |
| Drag from Home tiles and rows | UNVERIFIED | `home.rs` `HN_DRAG` | Built 2026-11-09; the notification is unit-tested. Needs a real mouse. |
| No error box after a drop the target refused or the user cancelled | UNVERIFIED | `list.rs` `begin_drag` | 2026-11-09: the box now shows only when the drag data cannot be built. v1 round: also when the drag loop itself fails; a refused or cancelled drag shows none. |
| Drag from search results and archive members | PARTIAL | `dragdrop.rs` `data_object_of_paths` | Search results: built 2026-11-09, data object tested headless, drag unverified. Archive members: no. |
| Right-button drag with the Copy / Move / Shortcut menu | UNVERIFIED | `LVN_BEGINRDRAG`, `SHDoDragDrop` | Built 2026-11-09. Needs a real mouse. |
| Drag image (the translucent icons and count) | UNVERIFIED | `SHDoDragDrop`, `IDropTargetHelper` | Built 2026-11-09. No image for drags from Home. |
| Drag a tab out to a new window, drag files onto a tab | UNVERIFIED | `tabs.rs` `TN_TEAR` | Built 2026-11-09; the tear notification is tested with window messages, the second process was not. [#47](https://github.com/CodingDiego/rexplorer/issues/47) |

## 3. Clipboard and undo

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Ctrl+C, Ctrl+X | UNVERIFIED | `list.rs:597-598` | Shell verbs. |
| Ctrl+V | UNVERIFIED | `list.rs:599` | Shell verb on the folder. |
| Cut items look faded | UNVERIFIED | `dragdrop.rs` `cut_paths`, `LVIS_CUT` | Built 2026-11-09. Not run: it needs the real clipboard. Not on Home. |
| Paste shortcut | PARTIAL | `menu.rs:90` | Only as the shell's own menu entry. |
| Ctrl+Z undo, Ctrl+Y redo | PARTIAL | `core/undo.rs` | 2026-11-09: rename, undo, redo, undo proven on the exe by command. Recorded: rename, New folder and New items, Delete to the Recycle Bin (key, command bar, right-click), Ctrl+V, drops on the list. v1 round, unverified: also drops on the sidebar, crumbs, tabs and Home, the shell's New submenu, and Delete and rename in `--desktop-icons` (its own Ctrl+Z / Ctrl+Y). Not recorded: pastes from %TEMP%, Keep-both copies, paste or drop on `--desktop-icons`. [#27](https://github.com/CodingDiego/rexplorer/issues/27) |

## 4. Delete and new

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Delete to the Recycle Bin, with the shell's confirmation | UNVERIFIED | `core/undo.rs` `recycle` | `IFileOperation` since 2026-11-09, so it can be undone. |
| Shift+Delete deletes permanently | UNVERIFIED | `menu.rs:274` | Relies on `CMIC_MASK_SHIFT_DOWN`. |
| New folder from the command bar, then rename | UNVERIFIED | `commands.rs:141`, `:177` | The rename step uses the broken path above. |
| Ctrl+Shift+N | WORKS | `commands.rs` `new_folder` | 2026-11-09: the command creates "Nueva carpeta" on disk and opens its rename. The key chord itself was not pressed. |
| New > text document, shortcut and so on | UNVERIFIED | `commands.rs:145`, `menu.rs` `Picked::New` | Shell's New submenu. v1 round: the background menu's New starts an in-place rename of the new item and Ctrl+Z removes it. New > Shortcut also starts the rename while its wizard is open. |

## 5. Selection

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Ctrl+A, Shift+click, Ctrl+click | UNVERIFIED | `list.rs:600` | Native ListView. |
| Rubber-band selection | UNVERIFIED | `list.rs:828-860` | Custom mouse handling; a suspect for the rename and drag bugs. |
| Type a name to jump to it | UNVERIFIED | `list.rs:95` | |
| Check boxes | UNVERIFIED | `ui.rs:159` | Off by default. |
| Invert selection | WORKS | `list.rs` `invert_selection` | 2026-11-09: Ctrl+I's command turns 1 of 3 selected into 2 of 3 and back. Chord not pressed. |
| Selection size in the status bar | WORKS | `ui.rs` `update_status` | 2026-11-09: "3 items, 1 selected, 5 B". Up to 500 selected items, not on network folders. |

## 6. Navigation

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Alt+Left / Right / Up, Backspace, mouse back / forward | UNVERIFIED | `main.rs:160-162`, `:560` | |
| Ctrl+L / Alt+D, typed paths, `shell:` names | UNVERIFIED | `main.rs:164`, `nav.rs:221` | |
| `%ENV%` names in the address | UNVERIFIED | | Depends on the shell's parser. |
| Address autocomplete | UNVERIFIED | `edit.rs`, `SHAutoComplete` | Built 2026-11-09. Escape leaves the bar at once instead of closing the dropdown first. |
| Breadcrumb dropdowns | PARTIAL | `ui.rs:524` | Not for remote or virtual crumbs. v1 round, unverified: Left on the first shown crumb opens the "..." overflow menu, the same one a click opens. |
| Back / forward history dropdown | UNVERIFIED | `nav.rs` | Built 2026-11-09: right-click Back or Forward. Press-and-hold: no. |
| F5 | UNVERIFIED | `main.rs:163` | |
| F4 focuses the address | WORKS | `main.rs` ACCELS | 2026-11-09: an F4 key message moves the focus to the address edit. No dropdown list as in Explorer. |
| F6 / Tab between panes | WORKS | `ui.rs` `cycle_focus` | 2026-11-10: F6 and Tab key messages go list, breadcrumb, address, search, sidebar, tab strip, list. On the breadcrumb Left then Enter went to the parent folder; its dropdown on Down was not opened. Shift+F6 and Shift+Tab not pressed. Tab in a rename box still renames the next item. |
| F6 / Tab reach the toolbars; keys inside a toolbar | UNVERIFIED | `toolbar.rs`, `ui.rs` `panes_in_order`, `main.rs` `pane_accel` | v1 round: navigation toolbar, tools bar and command bar join the cycle after the tab strip. Left / Right / Home / End move, Enter or Space press, Down or Alt+Down opens New / Sort / View / More, Esc returns to the view. Focus rectangle drawn. [#22](https://github.com/CodingDiego/rexplorer/issues/22) |
| Ctrl+N new window | WORKS | `instance.rs` `open_new_window`, `--new-window` | 2026-11-09: the command opened a second process on the same folder; closing it changed no settings file. That window saves no tabs, placement or recents. The chord itself was not pressed. |
| Ctrl+Enter opens in a new tab | UNVERIFIED | `main.rs` `CMD_OPEN_TAB`, `nav.rs` `activate_or_tab` | The command runs without error, the new tab was not confirmed. Ctrl+double-click built 2026-11-09: opens a background tab (Explorer opens a window). |

## 7. Views and columns

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Details | UNVERIFIED | `views.rs:268` | |
| Icons in four sizes | UNVERIFIED | `views.rs` | Extra large, Large, Medium and Small built 2026-11-09. |
| List, Tiles, Content | WORKS | `views.rs` | 2026-11-10: all nine views switch on the exe by command, items intact, F2 selects the name only. Content is a wide tile with three lines, not Explorer's layout. List shows no groups (the control cannot). |
| Ctrl+wheel changes icon size | UNVERIFIED | `views.rs` | All nine views in Explorer's order since 2026-11-10; a real wheel was not turned. |
| Click a column to sort | UNVERIFIED | `list.rs:132` | |
| Choose columns | UNVERIFIED | `folder.rs`, `commands.rs` `column_menu` | Built 2026-11-09: right-click the header. No "More..." and no "Size column to fit". |
| Column widths and order remembered | UNVERIFIED | `session.rs` `folders.txt` | Built 2026-11-09: kept per folder on disk. |
| Group by | UNVERIFIED | `nav.rs`, `commands.rs` | Built 2026-11-09: any shown column. A text column on a very large folder can pause the window. |
| Each folder remembers its view | UNVERIFIED | `session.rs` `folders.txt` | Built 2026-11-09: view, sort, group and columns, kept across restarts. |
| Ctrl+Shift+1..8 | UNVERIFIED | `main.rs` `ACCELS` | Explorer's meaning since 2026-11-10: extra large, large, medium, small, list, details, tiles, content. Thumbnails has no default chord. The chords were not pressed. |

## 8. Search

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Ctrl+F search box, results view | UNVERIFIED | `source.rs:459` | |
| Ctrl+E, F3 | WORKS | `main.rs` ACCELS | 2026-11-09: an F3 key message moves the focus to the search box. Ctrl+E shares the command; chord not pressed. |
| Filters (`kind:`, `date:`, size) | MISSING | | |
| Indexed search without elevation | UNVERIFIED | `core/wsearch.rs`, `source.rs` `start_search` | v1 round: a root the Windows Search index covers is queried through it, and the folders its rules leave out (AppData in a profile) are walked; otherwise the walk as before. Index rows match the real file name (extension included). Never run with WSearch on. [#41](https://github.com/CodingDiego/rexplorer/issues/41) |
| Results: icons, thumbnails, preview, drag, sort, rename, refresh | PARTIAL | `source.rs`, `search.rs` | Icons, sort, delete, rename and drag built 2026-11-09, unverified. Thumbnails, preview and refresh: no. [#41](https://github.com/CodingDiego/rexplorer/issues/41) |
| Results: context menu across folders, Back returns to them | UNVERIFIED | `menu.rs` `for_scattered`, `nav.rs` `Kept` | v1 round: one menu for results from several folders; Back restores the results with selection, scroll and sort. The last 4 per tab are kept. [#42](https://github.com/CodingDiego/rexplorer/issues/42) |

## 9. Context menus and opening

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Item menu, background menu, Shift for extended verbs | UNVERIFIED | `list.rs:402`, `menu.rs:59-86` | |
| Open with, Send to | UNVERIFIED | `menu.rs:293` | v1 round: a test gets one menu with Properties for three files and opens the sheet. Send to not tested. [#46](https://github.com/CodingDiego/rexplorer/issues/46) |
| Windows 11 app entries (Open in Terminal, Open with Code) | UNVERIFIED | `packaged.rs:295` | Uncommitted; skipped over 16 items. |
| Shift+F10, Menu key | UNVERIFIED | `list.rs`, `sidebar.rs`, `tabs.rs` | Also in the sidebar and the tab strip since 2026-11-10. |
| Properties, Alt+Enter | UNVERIFIED | `main.rs:169` | |
| Enter / double-click opens | UNVERIFIED | `source.rs:397` | |
| Run as administrator | PARTIAL | | Only through the shell menu. |
| Open in Terminal | UNVERIFIED | `instance.rs:117` | Ctrl+` |

## 10. Sidebar, tree and tabs

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Quick access, This PC, drives, Network | UNVERIFIED | `panes.rs`, `sidebar.rs` | Keyboard since 2026-11-10: arrows, Home, End, PgUp, PgDn, Enter, Esc, type-ahead, Shift+F10 ([#22](https://github.com/CodingDiego/rexplorer/issues/22)). |
| Folder tree that follows the current folder | PARTIAL | `tree.rs:60`, `panes.rs:98` | Off by default; replaces the sidebar. |
| Libraries | UNVERIFIED | `panes.rs` | Built 2026-11-09: a Libraries row, and Recycle Bin under Favorites. |
| Ctrl+T, Ctrl+W, Ctrl+Tab, Ctrl+1..9 | UNVERIFIED | `main.rs:171-174` | |
| Drag tabs to reorder, duplicate a tab | UNVERIFIED | `tabs.rs`, `nav.rs` `tab_menu` | Built 2026-11-09: drag to reorder and a tab menu (duplicate, close others, close to the right). Tested with window messages. |
| Tabs restored at start | UNVERIFIED | `settings.rs` `restore_tabs` | Built 2026-11-09: an option in Folder options, off by default. Always after a crash. |

## 11. Panes, icons and live updates

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Preview pane, Alt+P | UNVERIFIED | `panes.rs:180` | |
| Home: location status column, sort | UNVERIFIED | `home.rs` `STATUS`, `next_sort` | v1 round: a Status column (cloud state, empty for local files), sort by header click or the Sort menu, for the session only. [#45](https://github.com/CodingDiego/rexplorer/issues/45) |
| Details pane, Alt+Shift+P | WORKS | `panes.rs` `toggle_right`, `preview_pane.rs` | 2026-11-10: on the exe by command, right text for none, one and several items; swaps with the preview pane. The chord was not pressed; network and cloud items untested. |
| Item count in the status bar | UNVERIFIED | `ui.rs:837` | |
| Thumbnails and file-type icons | UNVERIFIED | `images.rs:33`, `source.rs:319` | |
| Icon overlays (shortcut arrow, OneDrive state) | UNVERIFIED | `item.rs` `overlay_slot` | Built 2026-11-09. Asked on the window thread for rows scrolled into view. |
| The list updates when files change | UNVERIFIED | `nav.rs:466` | A suspect for cancelling an open rename. |

## 12. File operations and special places

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Progress, conflict and elevation dialogs | UNVERIFIED | `menu.rs:263` | The shell's; no `IFileOperation` of our own. v1 round: `core/tests/data_ops.rs` checks copy and move conflicts (Replace, Keep both), cancel and recycle against the shell's engine, compiled only; no app path silences the shell's UI. [#26](https://github.com/CodingDiego/rexplorer/issues/26) |
| Recycle Bin restore and empty | UNVERIFIED | | Shell verbs. |
| Network and UNC | UNVERIFIED | `ui.rs:51` | v1 round: resolving an unreachable share's address touches no network on the window thread (test compiled only). [#34](https://github.com/CodingDiego/rexplorer/issues/34) |
| Paths over 260 characters | UNVERIFIED | `app.manifest` `longPathAware` | v1 round: declared; needs the LongPathsEnabled policy on. A core test of a long folder may fail, since the test exe has no such manifest. [#34](https://github.com/CodingDiego/rexplorer/issues/34) |
| MTP phones | PARTIAL | `nav.rs:236` | Falls back to an embedded Explorer view. |
| Zip as a folder | UNVERIFIED | `source.rs:405` | |
| .rar members: icons, thumbnails, preview | UNVERIFIED | `archive_view.rs`, `thumbs.rs`, `panes.rs` | v1 round: type icons for every member, picture thumbnails and a preview up to 32 MB, extracted to `%TEMP%\rexplorer-show-<pid>`. Extract here / Extract all never replace a file already there. Encrypted .rar: no password prompt. [#43](https://github.com/CodingDiego/rexplorer/issues/43) |
| FTP | UNVERIFIED | `connect.rs` | [#36](https://github.com/CodingDiego/rexplorer/issues/36) |
| Control Panel, Libraries | PARTIAL | | Typed `shell:` names only. |
| OneDrive placeholders | PARTIAL | `connect.rs:87` | No state icons. |

## 13. System integration, accessibility and options

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Default folder handler, Win+E, `/select`, `/e` | UNVERIFIED | `register.rs:148`, `cli.rs` | Not installed since 2026-10-18. |
| Desktop icons view | UNVERIFIED | `desktop_icons.rs` | Built 2026-11-10, never run: opt-in `--desktop-icons`, a bottom window on the primary monitor's work area, positions in `desktop-icons.txt`. `--desktop` still only replaces the desktop's menu. v1 round: its own Ctrl+Z / Ctrl+Y for Delete and rename; its window is named "Desktop" for screen readers. |
| Jump list, AppUserModelID | MISSING | | |
| F11 full screen | WORKS | `ui.rs` `toggle_fullscreen` | 2026-11-09: the command makes the window cover the monitor and a second one restores the exact placement. |
| Follow the system light / dark setting | UNVERIFIED | `settings.rs` | Built 2026-11-09: `theme=system`. |
| Per-monitor DPI | UNVERIFIED | `main.rs:310`, `:653` | v1 round: Folder options and Connect also rebuild their fonts when moved to another monitor. [#33](https://github.com/CodingDiego/rexplorer/issues/33) |
| Windows Text size | UNVERIFIED | `theme.rs` `text_scale` | v1 round: fonts and text-holding metrics follow Accessibility > Text size (100-225%), live. Fixed widths do not grow. [#24](https://github.com/CodingDiego/rexplorer/issues/24) |
| High contrast | UNVERIFIED | `theme.rs` `effective_theme` | Built 2026-11-10: system colors, outlines for hover and selection, live switch. Never seen under a real contrast theme. [#23](https://github.com/CodingDiego/rexplorer/issues/23) |
| Screen reader support for the custom controls | UNVERIFIED | `core/access.rs` | Built 2026-11-10: MSAA for Home, tabs, breadcrumb, sidebar, toolbars, palette and status bar, tested in-process; the preview and details pane since the v1 round. The address and search boxes have no accessible name. Not tried with Narrator or NVDA. [#21](https://github.com/CodingDiego/rexplorer/issues/21) |
| Show hidden files, show extensions | UNVERIFIED | `folder_options.rs:663`, `:1087` | |
| Copy as path, Ctrl+Shift+C | UNVERIFIED | `list.rs` `copy_as_path` | Built 2026-11-10. Not run: it writes the real clipboard. Refuses names with quotes or line breaks (archive folders). |
| Size all columns to fit, Ctrl+numpad Plus | UNVERIFIED | `list.rs` `fit_columns` | Built 2026-11-10; widths grew on the exe by command. |
| Esc cancels a cut | UNVERIFIED | `list.rs` `cancel_cut` | Built 2026-11-10. Not run: real clipboard. |
| Keyboard in the tab strip and on Home | UNVERIFIED | `tabs.rs`, `home.rs` | Built 2026-11-10: Left, Right, Home, End on tabs; Shift and Ctrl with arrows, Space and type-ahead on Home. |
| Language | UNVERIFIED | `core/i18n.rs`, `lang/es.ini` | Built 2026-11-10: English built in, Spanish shipped, `language=` in `[general]` (default: Windows' language), `lang\<code>.ini` overrides, restart to apply. `--help`, argument errors and the install, uninstall and register messages follow it too. Status bar seen in Spanish on the exe; dialogs not checked for clipping. |

## Order of work

1. Check with a real keyboard and mouse: F2, click-again rename and dragging to other programs, in a folder and
   on Home. Then every UNVERIFIED row built on 2026-11-09 and 2026-11-10.
2. Try `--desktop-icons`, a Windows contrast theme, and Narrator.
3. The v1 round's rows: toolbar keys, indexed search, results Back, Home sort, archive thumbnails, Text size,
   then read `rexplorer.log` after each failed drag to another program.
4. Undo for the cases not recorded yet.
5. Search filters, thumbnails and preview for search results, rename and drag for archive members.

Deliberate differences from Explorer: Backspace goes up, Ctrl+D pins, Ctrl+Shift+E opens Explorer, F4 has no
dropdown, and F10 opens no menu bar.
