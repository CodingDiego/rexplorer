# Explorer parity checklist

What Windows File Explorer does, and where rexplorer stands on each point. This file is the backlog: a row
changes status only when the behaviour was proven on the built exe.

**Totals, 2026-11-11 (after the final regression run): 118 rows. WORKS 61, WORKS (scratch base) 3, PARTIAL 10,
UNVERIFIED 44, BROKEN 0, MISSING 0.**

Last full review: 2026-11-09, working tree on top of `9781cc2`. The review read the code. Only the rows whose
note gives a date were run on the exe.

2026-11-10, v1 release-candidate round (`ADD/v1-release-candidate`): rows it touched say "v1 round". That round
was built without being run; rows still saying "v1 round" without a 2026-11-11 note remain unproven.

2026-11-11 live: rows that say so were driven on a copy of the release exe with real keyboard and mouse input
(`SendInput`, sent only while the foreground window and the window under the cursor were rexplorer's), on
scratch files under `%TEMP%` and a scratch profile. State was read back from the controls (list view and header
messages, MSAA), from files on disk and from screenshots of the window.

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
| F2 renames the focused item | WORKS | `list.rs:595` -> `begin_rename` `list.rs:658` | 2026-11-11 live: a real F2 on a clicked row opened the edit, typed text and Enter renamed the file on disk. Also passed earlier by key message in the grouped Downloads view and after starting on Home. Final regression, 2026-11-11, on the final build: real click on the name, then real F2, typed a non-ASCII name and Enter, 5 of 5 renamed on disk; the edit was still open 0.5 s after it appeared each time and selected the name without `.txt`. |
| Click the name of a selected item again | WORKS | `LVS_EDITLABELS` `ui.rs:119`, `list.rs:73` | 2026-11-11 live: a real click on the name of the selected row, after the double-click time, opened the edit; Enter renamed on disk. Never starts with single-click-to-open on. Final regression, 2026-11-11: two real clicks on the name 750 ms apart (double-click time 500 ms), typed name and Enter, 5 of 5 renamed on disk. |
| F2 on the Home page (tiles and recent files) | WORKS | `home.rs:639` | 2026-11-11 live: real F2 on a tile opens the edit; Escape closes it with no change. Final regression, 2026-11-11: real click then real F2 on the Downloads, Desktop and Documents tiles, 5 of 5 opened the edit with the tile's name, still open after 0.5 s, and Esc closed it (the user's real folders, so never committed). |
| Click the name again on the Home page | WORKS | `home.rs` button-up and rename timer | 2026-11-11 live: a real second click on a selected tile and on a selected recent-file row opened the edit. Final regression, 2026-11-11: two real clicks 750 ms apart on a tile, 5 of 5 opened the edit, still open after 0.5 s, Esc closed it. Recent-file rows were not in view that run. |
| Rename from the context menu and the command bar | UNVERIFIED | `menu.rs:111`, `main.rs:806` | Same `begin_rename` as F2. |
| Rename starts with the name selected, not the extension | WORKS | `list.rs` `stem_len`, `home.rs:652` | 2026-11-11 live: real F2 on `alpha.txt` selects 0..5, on `beta.tar.gz` 0..8 (`beta.tar`, as Explorer), a name without a dot is selected whole. |
| Tab commits and renames the next item | WORKS | comctl32's own label edit, `list.rs` `rename_item` | 2026-11-11 live: real Tab commits and opens the next row's edit, Shift+Tab the previous one, with the name-only selection each time. Fixed that day: after a Tab rename the row kept its old item, so Shift+Tab back edited the stale name. Not on Home. |
| Rename in search results and inside .rar | PARTIAL | `source.rs`, `list.rs` | Search results: built 2026-11-09, unverified. Archive members: not possible, by design. unrar only reads archives, and rexplorer does not rewrite them. 2026-11-11 live, by window messages: in a .rar, F2, the Rename command and a direct `LVM_EDITLABEL` all opened no edit box, and the archive's hash did not change. Unlike Explorer in a .zip, no message explains why. |

## 2. Drag and drop

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Drag files out to another program | WORKS | `list.rs` `begin_drag`, `dragdrop.rs` `drag` | 2026-11-11 live, real mouse: 60 drags out of the list, none lost. Into a separate WinForms OLE target that logs what it gets (DragEnter every time, CF_HDROP with the dragged path, the shell's formats and DragImageBits), into a classic `DragAcceptFiles` / WM_DROPFILES window, into Explorer's file list and onto an Explorer folder row (moves). Details and Large icons, selected or not, from the name or another cell, 5 ms and 80 ms mouse steps. [#75](https://github.com/CodingDiego/rexplorer/issues/75) did not reproduce, so its cause is still unknown. Browsers, WhatsApp and VS Code were not tried (they are the user's apps). A drag that applies nothing is logged to `rexplorer.log`; a target's optimized move no longer counts as nothing (`performed_effect`). Final regression, 2026-11-11, on the final build: 5 of 5 real drags from a folder into a WinForms OLE target delivered CF_HDROP with the dragged path, the target's copy matched the file and the source stayed; `rexplorer.log` stayed empty. |
| Drag onto a folder row of the same list | WORKS | `list.rs` `register_drop` | 2026-11-11 live: a right-drag from a row onto the `rxsub` row, then Copy here, Move here and Create shortcuts here. Each did the operation, and Ctrl+Z took it back. Explorer files dropped on the row moved into it (3 of 3). |
| Drop files from another program into the list | WORKS | `dragdrop.rs` `DropForwarder` | 2026-11-11 live, real mouse from a real Explorer window: a move within the volume (3 of 3), a copy with Ctrl (3 of 3), onto a folder row (3 of 3), and a right-drag with Move here (3 of 3). Ctrl+Z put each back and Ctrl+Y redid it. Explorer misses every other drag of a file the user did not click first after a move. It does this into its own folders too, so it is not rexplorer's doing. |
| Drop onto the Home page, sidebar rows, breadcrumb crumbs, tabs | WORKS | `list.rs` `register_folder_drops` | 2026-11-11 live, real mouse from Explorer, 3 drops each: onto a sidebar bookmark row, onto a crumb, and onto a second tab (resting on the tab switched to it). Each drop moved the file, Ctrl+Z moved it back and Ctrl+Y redid it. Onto a Home tile, with the right button only: the shell's Copy / Move / Shortcut menu opened (3 of 3) and was cancelled, so nothing was put into the user's real folders. |
| Drop onto the folder tree | UNVERIFIED | `tree.rs:60` | Whatever the shell tree control does by itself. |
| Drag from Home tiles and rows | PARTIAL | `home.rs` `HN_DRAG` | 2026-11-11 live, tiles: a left drag to the OLE target delivered CF_HDROP of the tile's folder (3 of 3; nothing was copied). A right drag onto Explorer's list opened Explorer's drop menu (3 of 3, cancelled). Fixed that day: a right-button press on Home never started a drag. Final regression, 2026-11-11: 5 of 5 real left drags of the Downloads, Desktop and Documents tiles into a WinForms OLE target delivered CF_HDROP of the tile's folder (the target answered Copy and copied nothing; every folder stayed). Recent-file rows were not dragged. |
| No error box after a drop the target refused or the user cancelled | WORKS | `list.rs` `begin_drag` | 2026-11-11 live: Esc during a drag out (3 of 3) and Cancel in a right-drag menu showed no box and moved nothing; the log says "drag no effect". |
| Drag from search results and archive members | WORKS | `dragdrop.rs` `data_object_of_paths`, `archive_view.rs` `extract_for_drag` | Search results: 2026-11-11 live, three results from two folders were dragged one by one into Explorer, and each moved. The results list keeps showing the moved files. Archive members: 2026-11-11 live, real mouse with the guard checked at the press, along the path and at the release. `pic.png` was dragged from a .rar onto a WinForms OLE target. The target saw only Copy allowed and CF_HDROP of the member, extracted to `%TEMP%\rexplorer-show-<pid>`. The drop's effect was Copy, and the copy matched the member byte for byte (658 bytes). A folder member hands over its extracted folder; this is covered by a unit test only. An encrypted member asks for the password first (`source.rs` `with_password`), which was not dragged live. |
| Right-button drag with the Copy / Move / Shortcut menu | WORKS | `LVN_BEGINRDRAG`, `SHDoDragDrop` | 2026-11-11 live: rexplorer to rexplorer (all three choices), Explorer to rexplorer (Move, 3 of 3), rexplorer to Explorer (Copy), each picked with a real click on the menu. Fixed that day: Ctrl+Z after Create shortcuts here did nothing; the new `.lnk` now goes to the Recycle Bin (`list.rs` `record_drop`). |
| Drag image (the translucent icons and count) | WORKS | `SHDoDragDrop`, `IDropTargetHelper` | 2026-11-11 live: screenshots taken mid-drag showed the icon and name with the move glyph over Explorer for a search result, and the folder icon for a drag from a Home tile. |
| Drag a tab out to a new window, drag files onto a tab | UNVERIFIED | `tabs.rs` `TN_TEAR` | Built 2026-11-09; the tear notification is tested with window messages, the second process was not. [#47](https://github.com/CodingDiego/rexplorer/issues/47) |

## 3. Clipboard and undo

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Ctrl+C, Ctrl+X | WORKS | `list.rs` `to_clipboard`, `dragdrop.rs` `to_clipboard` | Shell verbs. 2026-11-11 live: real chords, then Ctrl+V in another folder, copied and moved the file. With Explorer, both ways: Ctrl+C and Ctrl+X in rexplorer, then Ctrl+V in a real Explorer window, copied and moved the file. Later that day (scripts batch): in a file system folder Copy and Cut now put the folder's data object on the clipboard themselves (Preferred DropEffect 5 / 2, as Explorer) and Paste uses the default menu without handlers, because the shell verbs loaded the Work Folders handler, which left one thread per use (soak: 50 to 144 threads in 15 min). Re-proven by command (WM_COMMAND, not chords): Cut then Paste moved the file, Copy then Paste copied it, smoke `commandbar` PASS, no thread growth over 5 copies, cuts or pastes. The exchange with a real Explorer window was not re-run on this build. |
| Ctrl+V | WORKS | `list.rs` `verb_on_background` | Shell verb on the folder (since the scripts batch, 2026-11-11, from the default menu without extension handlers in a file system folder; see the row above). 2026-11-11 live, as above. With Explorer: Ctrl+C and Ctrl+X in Explorer, then Ctrl+V in rexplorer, copied and moved the file; Ctrl+Z and Ctrl+Y undid and redid both. Paste from the background menu works too, and so does Paste from a folder row's menu (into that folder). Fixed that day: Ctrl+Z after a paste from a folder row's menu did nothing, because the paste was recorded for the current folder (`list.rs` `show_menu`). |
| Cut items look faded | WORKS | `dragdrop.rs` `cut_paths`, `LVIS_CUT` | 2026-11-11 live, real clipboard (saved and restored): after a real Ctrl+X the row reports LVIS_CUT and the other rows do not. A file cut in a real Explorer window shows faded in rexplorer too. Not on Home. |
| Paste shortcut | WORKS | `list.rs` `paste_shortcut`, `dragdrop.rs` `paste_link` | 2026-11-11 live (window messages to a copy of the exe): the background menu shows one "Paste shortcut", enabled only with files on the clipboard. With a bare file list (`Set-Clipboard -Path`) and with rexplorer's own Copy of a folder it made `a.txt.lnk` and `sub.lnk` pointing at them (the shell names them; this PC's Explorer `link` setting drops " - Shortcut"), and Ctrl+Z sent each to the Recycle Bin. |
| Ctrl+Z undo, Ctrl+Y redo | PARTIAL | `core/undo.rs` | 2026-11-11 live: real Ctrl+Z then Ctrl+Y on rename, New folder, Delete (to the Recycle Bin and back), Ctrl+C / Ctrl+V copy and Ctrl+X / Ctrl+V move each restored and redid the disk state (clipboard saved and restored around the run). Fixed that day: every paste from a folder under %TEMP% was skipped; now only archive extractions there are (`list.rs` `extracted_for_transfer`). Recorded: rename, New folder and New items, Delete to the Recycle Bin (key, command bar, right-click), Ctrl+V, drops on the list. 2026-11-11 live: real-mouse drops from Explorer onto the list, a folder row, a sidebar row, a crumb and a tab, Explorer's copy and cut pasted in rexplorer, menu pastes, and right-drag copy, move and shortcut were each undone and redone on disk. A drag out of rexplorer (the drop is Explorer's) is left alone by rexplorer's Ctrl+Z, which undoes its own newest entry instead. Shortcuts made by a drop are recorded as the new `.lnk` files. v1 round, unverified: drops on Home, the shell's New submenu, and Delete and rename in `--desktop-icons` (its own Ctrl+Z / Ctrl+Y). Not recorded: Keep-both copies, paste or drop on `--desktop-icons`. [#27](https://github.com/CodingDiego/rexplorer/issues/27) |

## 4. Delete and new

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Delete to the Recycle Bin, with the shell's confirmation | WORKS | `core/undo.rs` `recycle` | `IFileOperation` since 2026-11-09, so it can be undone. 2026-11-11 live: real Delete sent the file to the Recycle Bin and Ctrl+Z brought it back. |
| Shift+Delete deletes permanently | UNVERIFIED | `menu.rs:274` | Relies on `CMIC_MASK_SHIFT_DOWN`. |
| New folder from the command bar, then rename | WORKS | `commands.rs:141`, `:177` | 2026-11-11 live: real Ctrl+Shift+N made the folder with its name edit open; typing and Enter renamed it; Ctrl+Z removed it. |
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
| Alt+Left / Right / Up, Backspace, mouse back / forward | UNVERIFIED | `main.rs:160-162`, `:560` | 2026-11-11 live: real Alt+Left went back, also while a toolbar had the focus. The others not pressed. 2026-11-11 scripts batch: Back sent while an Up was still loading did nothing (the history entry is pushed when the load lands; smoke `keyboard-nav` failed); now Back cancels that load and stays on the folder, proven by the commands (WM_COMMAND, not keys) in smoke and in a title-timeline probe. Forward to the cancelled place is not offered. |
| Ctrl+L / Alt+D, typed paths, `shell:` names | WORKS | `main.rs:164`, `nav.rs:221` | 2026-11-11 live: real Ctrl+L, a typed `shell:Downloads` and Enter opened Downloads. |
| `%ENV%` names in the address | WORKS | `nav.rs` `expand_env` | 2026-11-11 live: typed `%TEMP%` and Enter opened the temp folder. Fixed that day: the shell's parser does not expand them, so the address now does; unknown names stay as typed. |
| Address autocomplete | WORKS | `edit.rs`, `SHAutoComplete` | 2026-11-11 live: typing a partial path showed the dropdown. Escape leaves the bar at once instead of closing the dropdown first (kept). |
| Breadcrumb dropdowns | WORKS | `ui.rs` `crumb_dropdown` | Listed on a worker for every crumb with a shell folder; the folder icon opens the desktop's places (Explorer's root chevron). 2026-11-11 live (window messages): the icon listed Home, Gallery, This PC, Libraries, Network, Control Panel, Recycle Bin and the desktop's folders; This PC's crumb listed its folders and C:; on `\\localhost\C$\Users\Public` the Users crumb listed its folders over SMB; the window answered WM_NULL at once while each listed. An empty folder (Network here) shows no menu. Earlier 2026-11-11 live: on a focused crumb real Down opened its folder dropdown, and Left on the first shown crumb opened the "..." overflow menu. |
| Back / forward history dropdown | WORKS | `nav.rs` | 2026-11-11 live: right-click Back showed the history and picking an entry went there; the Forward menu listed the folders ahead. Press-and-hold: no. |
| F5 | UNVERIFIED | `main.rs:163` | |
| F4 focuses the address | WORKS | `main.rs` ACCELS | 2026-11-09: an F4 key message moves the focus to the address edit. No dropdown list as in Explorer. |
| F6 / Tab between panes | WORKS | `ui.rs` `cycle_focus` | 2026-11-11 live: real F6 and Tab go list, breadcrumb, address, search, sidebar, tab strip, navigation toolbar, tools bar, command bar, list; Shift+F6 and Shift+Tab walk it backwards. No trap. Tab in a rename box still renames the next item. |
| F6 / Tab reach the toolbars; keys inside a toolbar | WORKS | `toolbar.rs`, `ui.rs` `panes_in_order`, `main.rs` `pane_accel` | 2026-11-11 live: real Left / Right / Home / End move; Down, Alt+Down, Enter and Space open Sort; Down opens View, New and More; Esc returns to the view; Ctrl+L and Alt+Left still work from a toolbar. Focus rectangle seen in screenshots. Fixed that day: Down did nothing on More (it is a plain button that opens a menu; `toolbar.rs` `set_menu`). [#22](https://github.com/CodingDiego/rexplorer/issues/22) |
| Ctrl+N new window | WORKS | `instance.rs` `open_new_window`, `--new-window` | 2026-11-09: the command opened a second process on the same folder; closing it changed no settings file. That window saves no tabs, placement or recents. The chord itself was not pressed. |
| Ctrl+Enter opens in a new tab | UNVERIFIED | `main.rs` `CMD_OPEN_TAB`, `nav.rs` `activate_or_tab` | The command runs without error, the new tab was not confirmed. Ctrl+double-click built 2026-11-09: opens a background tab (Explorer opens a window). |

## 7. Views and columns

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Details | WORKS | `views.rs:268` | 2026-11-11 live: real Ctrl+Shift+6. |
| Icons in four sizes | WORKS | `views.rs` | 2026-11-11 live: real Ctrl+Shift+1..4 switch the list view to each, item count intact. |
| List, Tiles, Content | WORKS | `views.rs` | 2026-11-10: all nine views switch on the exe by command, items intact, F2 selects the name only. Content is a wide tile with three lines, not Explorer's layout. List shows no groups (the control cannot). |
| Ctrl+wheel changes icon size | UNVERIFIED | `views.rs` | All nine views in Explorer's order since 2026-11-10; a real wheel was not turned. |
| Click a column to sort | WORKS | `list.rs:132` | 2026-11-11 live: real header clicks; the first click sorts Size and Date descending and Type ascending, as Explorer; a second click flips it; the header arrow follows (HDM_GETITEM). Hover fill on a header item seen on a screen capture. Fixed that day: header items had no hover fill. [#76](https://github.com/CodingDiego/rexplorer/issues/76) |
| Choose columns | UNVERIFIED | `folder.rs`, `commands.rs` `column_menu` | Built 2026-11-09: right-click the header. No "More..." and no "Size column to fit". |
| Column widths and order remembered | UNVERIFIED | `session.rs` `folders.txt` | Built 2026-11-09: kept per folder on disk. |
| Group by | WORKS | `nav.rs`, `commands.rs` | 2026-11-11 live: Group by > Date modified turned groups on (3 groups), "(None)" turned them off, and the background menu's Sort by / Group by show the same state. Downloads opens grouped by date, newest first (6 groups). A text column on a very large folder can pause the window. |
| Each folder remembers its view | WORKS | `session.rs` `folders.txt` | 2026-11-11 live: view, sort and group set in one folder came back after navigating away and back, and after a restart (`folders.txt` line written). |
| Ctrl+Shift+1..8 | WORKS | `main.rs` `ACCELS` | 2026-11-11 live: real chords give extra large, large, medium, small, list, details, tiles, content, each with the item count intact. Thumbnails has no default chord (View menu only, checked by command). |

## 8. Search

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Ctrl+F search box, results view | UNVERIFIED | `source.rs:459` | |
| Ctrl+E, F3 | WORKS | `main.rs` ACCELS | 2026-11-09: an F3 key message moves the focus to the search box. Ctrl+E shares the command; chord not pressed. |
| Filters (`kind:`, `date:`, size) | WORKS | `core/query.rs`, `search.rs` `hit`, `wsearch.rs`, `usn.rs` | Explorer's syntax, English and Spanish keys and values: `kind:`/`tipo:` (folder, picture, document, music, video), `date:`/`fecha:` (today, yesterday, this or last week, month or year, `2025`, `2025-03`, `2025-03-14`, `<`, `>`, `a..b`), `size:`/`tamaño:` (empty to gigantic, `>10MB`, `a..b`), `ext:`, plain words and quoted phrases. One parser serves the walk, the index and the USN volume index. A filter it does not understand becomes a plain word. 2026-11-11 live, by window messages on scratch files: all 14 queries returned exactly the expected files. They included `kind:picture`, `tipo:imagen trip`, `date:today ext:txt`, `date:<2025`, `size:empty`, `size:>1MB`, `tamaño:mediano`, `date:this week kind:music`, `fecha:hoy tipo:carpeta`, and `size:banana`, which found nothing. [#41](https://github.com/CodingDiego/rexplorer/issues/41) |
| Indexed search without elevation | WORKS | `core/wsearch.rs`, `source.rs` `start_search` | A root the Windows Search index covers is queried through it. The folders its rules leave out (AppData in a profile) are walked; any other root uses the walk as before. Index rows match the real file name (extension included). 2026-11-11 live, WSearch on, by window messages: in an indexed profile folder, the exe's result counts for `ext:pdf`, `kind:picture`, `kind:folder`, `size:>1MB` and `date:2026 kind:document` equalled a file system walk of the same folder. A core probe showed that the index itself answered there. Fixed that day: in a moved Documents known folder the index returned its rows under the old path, all of them were dropped, and the search found nothing. Now rows outside the root make the search walk the root instead (`wsearch.rs` strays), which matched the walk for every query. Speed was not measured. [#41](https://github.com/CodingDiego/rexplorer/issues/41) |
| Results: icons, thumbnails, preview, drag, sort, rename, refresh | PARTIAL | `source.rs`, `search.rs`, `views.rs`, `panes.rs` | 2026-11-11 live, by window messages: the Thumbnails view showed picture thumbnails for results. The preview pane showed a result's picture and a result's text. The details pane showed one result (name, type, size, dates) and two ("2 items selected", total size). Refresh (F5's command) picked up a file added after the search (2 to 3 results). All were checked on screenshots and in the list's item count. Drag: see the drag row. Sort, delete and rename were built 2026-11-09 and remain unverified. [#41](https://github.com/CodingDiego/rexplorer/issues/41) |
| Results: context menu across folders, Back returns to them | WORKS | `menu.rs` `for_scattered`, `nav.rs` `Kept` | Scripts batch 2026-11-11: the searched folder's own path entered in the address bar searched again instead of opening the folder (soak missed a wait every round); fixed, proven by command in the soak steps (8 of 8 left the results). 2026-11-11 live: right-click on results from two folders showed one menu; its Properties and Alt+Enter opened one sheet for all of them ("Varias carpetas"); opening a result's folder then Back restored the results. Fixed that day: Properties on such a selection did nothing (now `SHMultiFileProperties`). The last 4 per tab are kept. [#42](https://github.com/CodingDiego/rexplorer/issues/42) |

## 9. Context menus and opening

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Item menu, background menu, Shift for extended verbs | PARTIAL | `list.rs:402`, `menu.rs:59-86` | 2026-11-11 live: real right-click on one item, on two items and on the background each opened the right menu. Fixed that day: the item menu listed both our pin entry and the shell's own. Shift for extended verbs not tried. |
| Open with, Send to | UNVERIFIED | `menu.rs:293` | v1 round: a test gets one menu with Properties for three files and opens the sheet. Send to not tested. [#46](https://github.com/CodingDiego/rexplorer/issues/46) |
| Windows 11 app entries (Open in Terminal, Open with Code) | UNVERIFIED | `packaged.rs:295` | Uncommitted; skipped over 16 items. |
| Shift+F10, Menu key | WORKS | `list.rs`, `sidebar.rs`, `tabs.rs` | 2026-11-11 live in the list: real Shift+F10 and the Menu key open the item menu. Sidebar and tab strip not pressed. |
| Properties, Alt+Enter | WORKS | `main.rs:169` | 2026-11-11 live: real Alt+Enter opened the sheet, also for search results from several folders. |
| Enter / double-click opens | UNVERIFIED | `source.rs:397` | |
| Run as administrator | UNVERIFIED | `list.rs` `run_as_admin` | Our entry (shield, Ctrl+Shift+Enter) on a selected .exe, .bat, .cmd, .com, .msi or .msc runs the shell's runas verb; the shell's own entry is hidden. 2026-11-11 live: tool.bat's menu showed it once, a folder's not at all. Not invoked: the consent prompt is the user's to answer. |
| Open in Terminal | UNVERIFIED | `instance.rs:117` | Ctrl+` |

## 10. Sidebar, tree and tabs

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Quick access, This PC, drives, Network | UNVERIFIED | `panes.rs`, `sidebar.rs` | Keyboard since 2026-11-10: arrows, Home, End, PgUp, PgDn, Enter, Esc, type-ahead, Shift+F10 ([#22](https://github.com/CodingDiego/rexplorer/issues/22)). |
| Folder tree that follows the current folder | PARTIAL | `tree.rs:60`, `panes.rs:98` | Off by default; replaces the sidebar. |
| Libraries | UNVERIFIED | `panes.rs` | Built 2026-11-09: a Libraries row, and Recycle Bin under Favorites. |
| Ctrl+T, Ctrl+W, Ctrl+Tab, Ctrl+1..9 | WORKS | `main.rs:171-174` | 2026-11-11 live: real Ctrl+T, Ctrl+1, Ctrl+Tab, Ctrl+Shift+Tab and Ctrl+W; the new-tab and close-tab commands by message too. |
| Drag tabs to reorder, duplicate a tab | WORKS | `tabs.rs`, `nav.rs` `tab_menu` | 2026-11-11 live: a real mouse drag moved a tab to the other end; Duplicate from the tab menu added a tab on the same folder. Close others and close to the right not tried. |
| Tabs restored at start | UNVERIFIED | `settings.rs` `restore_tabs` | Built 2026-11-09: an option in Folder options, off by default. Always after a crash. 2026-11-11 live: after `REXPLORER_CRASH_AFTER_MS` the process died with 0xC0000005, a minidump landed in `%LOCALAPPDATA%\rexplorer\crashes`, and it restarted with `--recover`; answering Yes on its notice opened the crashes folder in a tab. Restored tabs not compared. Later that day, after the relaunch quoting fix (an argument ending in a backslash was split wrongly; `crash::tests::command_line_round_trips_through_argv`), a crash from a folder with a space in its name relaunched once. The child's command line, read from WMI and split by `CommandLineToArgvW`, was exactly the exe and `--recover`, and its window showed that folder. Smoke `crash-recovery` PASS in two full runs that day: the crashed process exited and its `--recover` child showed the same folder ("Windows") within 6 s. [#30](https://github.com/CodingDiego/rexplorer/issues/30) |

## 11. Panes, icons and live updates

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Preview pane, Alt+P | WORKS | `panes.rs:180` | 2026-11-11 live: real Alt+P showed a text file's text and a JPEG's picture with name, size and type; a second Alt+P hid it. Network and cloud items untested. Scripts batch 2026-11-11: with the pane on, a row selected on Home crashed the window (0xC000041D, an empty detail line handed to DrawTextW as the dangling pointer 0x2; found by the soak, located from the minidump with a line-table build); fixed, and the new smoke `home-preview` FAILs on the old exe and PASSes on the fixed one. |
| Home: location status column, sort | UNVERIFIED | `home.rs` `STATUS`, `next_sort` | v1 round: a Status column (cloud state, empty for local files), sort by header click or the Sort menu, for the session only. [#45](https://github.com/CodingDiego/rexplorer/issues/45) |
| Details pane, Alt+Shift+P | WORKS | `panes.rs` `toggle_right`, `preview_pane.rs` | 2026-11-11 live: real Alt+Shift+P shows Size and Date modified for the selection and swaps with the preview pane; pressed again it hides. Fixed that day: values started at a fixed offset and overlapped long Spanish labels. Network and cloud items untested. |
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
| Paths over 260 characters | UNVERIFIED | `app.manifest` `longPathAware` | v1 round: declared; needs the LongPathsEnabled policy on. 2026-11-11: the shell's own rename (SetNameOf) failed over 260 characters even with the manifest (0x800700CE); `item.rs` now renames such paths with MoveFileExW (no replace). `source::tests::lists_and_renames_in_a_folder_over_260_characters` passes in the app's test exe (which carries the manifest). 2026-11-11 live, with the policy on: a folder whose path is 304 characters lists, and real F2 renames in it. Delete there asks to delete permanently (the Recycle Bin cannot take such paths): No keeps the file, Yes removes it. Fixed that day: Delete failed with no question. [#34](https://github.com/CodingDiego/rexplorer/issues/34) |
| MTP phones | PARTIAL | `nav.rs:236` | Falls back to an embedded Explorer view. |
| Zip as a folder | UNVERIFIED | `source.rs:405` | |
| .rar members: icons, thumbnails, preview | UNVERIFIED | `archive_view.rs`, `thumbs.rs`, `panes.rs` | v1 round: type icons for every member, picture thumbnails and a preview up to 32 MB, extracted to `%TEMP%\rexplorer-show-<pid>`. Extract here / Extract all never replace a file already there. 2026-11-11 live: the preview pane showed the text of a member of a header-encrypted .rar once its password was given. Icons and thumbnails were not checked that day. [#43](https://github.com/CodingDiego/rexplorer/issues/43) |
| Encrypted .rar: password prompt | WORKS | `source.rs` `ask_password` / `run_job` / `with_password`, `nav.rs`, `core/archive.rs` `Password` | A themed dialog (`connect.rs` `form`) with a masked field. The password is kept in memory only while a tab shows the archive, zeroed when dropped, and never written to disk. 2026-11-11 live, by window messages, on RAR5 archives built by `scripts/make-rar-fixtures.ps1`. Header-encrypted archive: opening it asked "Enter the password for enc-headers.rar". A wrong password asked again with "Wrong password ... Try again". Cancel left the folder with no error box. The right password (non-ASCII) listed `docs\hidden.txt`. Member-encrypted archive: it listed without asking, and Extract here asked. After a wrong password nothing at all was written (the first encrypted member is test-decrypted first). The right one extracted both members with the right content. Process memory held 1 UTF-16 copy while the archive was shown and 0 after leaving it (ACP: 0 throughout). No file under the scratch profile or temp folder held either password. Limit: unrar takes the password in the ANSI code page, so characters outside it cannot be typed for such archives. [#43](https://github.com/CodingDiego/rexplorer/issues/43) |
| Archive safety: links, device names, paths that climb out | WORKS | `core/archive.rs` `escapes`, `extract`, `extract_all` | Member names with a `..` step (also `.. ` and `... `), a DOS device name (`CON`, `COM1.jpg`, `CONIN$`, `lpt3` as a folder), a trailing dot or space, or a `:` are not listed, previewed or extracted. A link entry (symbolic link or junction) is never left on disk: unrar refuses the unsafe ones, and the ones it makes are removed at once. Each refused entry is counted in the error. 2026-11-11 live: Extract here on a crafted .rar with nine such members (five names and four links) plus `ok.txt` said "9 file(s) were not extracted ..." and left only `ok.txt` next to the archive. No link, no `evil` file anywhere, and the decoy `..\outside.txt` was untouched. Previewing a link member showed nothing and left no link in the show folder. Fixed that day: a link unrar refused without an error was not counted. Unit tests on the same kinds of archive: `archive::tests`. |
| FTP | UNVERIFIED | `connect.rs` | [#36](https://github.com/CodingDiego/rexplorer/issues/36) |
| Control Panel, Libraries | WORKS | `ui.rs` `crumb_dropdown` | 2026-11-11 live: picking Control Panel, then Libraries, from the breadcrumb icon's dropdown opened "Panel de control" and "Bibliotecas" (window titles). |
| OneDrive placeholders | PARTIAL | `item.rs` `cloud_state`, `views.rs` `cloud_mark` | Without a sync engine overlay, list rows get Explorer's marks from the file attributes: a blue cloud (online-only) or a green check (always keep on this device). 2026-11-11 live: a file with FILE_ATTRIBUTE_PINNED showed the green check in Details (screenshot); FILE_ATTRIBUTE_OFFLINE got the shell's own overlay instead. Real OneDrive placeholders untested: OneDrive is not running on this PC and its folder has none. Icon views and Home show no mark. |

## 13. System integration, accessibility and options

| Explorer behaviour | Status | Evidence | Note |
|---|---|---|---|
| Default folder handler, Win+E, `/select`, `/e` | UNVERIFIED | `register.rs:148`, `install.rs` `take_over`, `cli.rs` | Not installed since 2026-10-18. 2026-11-11: now a separate step (`--register`), journaled in `takeover.txt`; its keys were written and removed on a scratch base (rows below), but no real folder has been opened through them. |
| Standalone install: exe, Start menu shortcut, Apps and Features entry, no association | WORKS (scratch base) | `install.rs` `install`, `scripts\install.ps1` | 2026-11-11, on the exe with `REXPLORER_INSTALL_DIR` / `REXPLORER_REG_BASE` / shortcut-folder overrides: `install.ps1 -Quiet` left the copy, `rexplorer.lnk` and `Uninstall\rexplorer` (UninstallString `"<dir>\rexplorer.exe" --uninstall --quiet`, NoModify / NoRepair REG_DWORD 1) and no Classes, App Paths or Startup entry; `-Uninstall`, and the UninstallString run by itself, each left a `reg export` identical to before. smoke `install-scratch` PASS with both question boxes answered by message. Smoke now also checks the Start menu and Startup shortcuts in scratch folders, and `install-scripts` runs `master-install.ps1 -Quiet` then `restore-explorer.ps1 -RemoveProgram -Quiet`; both PASS 2026-11-11. The real Settings > Apps page not looked at. |
| Make default / give back (`--register`, `--unregister`, More menu) | WORKS (scratch base) | `install.rs` `take_over` / `give_back`, `ui.rs` `register_flow` | 2026-11-11: `--register --quiet` by the installed copy wrote the five command keys, App Paths and the Startup shortcut; `--unregister --quiet` then `--uninstall --quiet` left the scratch base as before. More menu read from the live popup (WM_COMMAND, MN_GETHMENU): "Make rexplorer the default file manager..." before, "Give folders back to Windows Explorer..." after a scratch `--register`, back again after `--unregister`. |
| Master install with backup, restore Explorer without rexplorer | WORKS (scratch base) | `scripts\master-install.ps1`, `scripts\restore-explorer.ps1` | 2026-11-11: master install wrote a dated `.reg` with six `[-key]` sections and printed it and the way back; `restore-explorer.ps1` closed a rexplorer window running from the install folder, removed the take-over and kept the program, ran twice cleanly, `-RemoveProgram` and the broken case (exe and both records deleted first) each left a `reg export` identical to before; `-WhatIf` changed nothing. `-RestartShell` not exercised (it refuses on a test base). |
| Desktop icons view | UNVERIFIED | `desktop_icons.rs` | Built 2026-11-10, never run: opt-in `--desktop-icons`, a bottom window on the primary monitor's work area, positions in `desktop-icons.txt`. `--desktop` still only replaces the desktop's menu. v1 round: its own Ctrl+Z / Ctrl+Y for Delete and rename; its window is named "Desktop" for screen readers. |
| Jump list, AppUserModelID | WORKS | `jumplist.rs`, `main.rs` `refresh_jump_list` | `CodingDiego.rexplorer` for the process, its windows and the installed Start shortcut; the jump list has Frequent (Quick Access's frequent folders), Recent (rexplorer's) and a New window task. 2026-11-11 live: the window's System.AppUserModel.ID read back from outside; after a navigation the AppID's `.customDestinations-ms` held both categories, the data folder and `--new-window`; a scratch `--install --quiet` made a Start shortcut carrying the same ID, the installed copy rewrote the list with its own path, `--uninstall --quiet` removed it. Not seen on the taskbar itself (right-clicking it is Explorer's window). |
| F11 full screen | WORKS | `ui.rs` `toggle_fullscreen` | 2026-11-09: the command makes the window cover the monitor and a second one restores the exact placement. |
| Follow the system light / dark setting | UNVERIFIED | `settings.rs` | Built 2026-11-09: `theme=system`. |
| Per-monitor DPI | UNVERIFIED | `main.rs:310`, `:653` | v1 round: Folder options and Connect also rebuild their fonts when moved to another monitor. [#33](https://github.com/CodingDiego/rexplorer/issues/33) |
| Windows Text size | UNVERIFIED | `theme.rs` `text_scale` | v1 round: fonts and text-holding metrics follow Accessibility > Text size (100-225%), live. Fixed widths do not grow. Folder options and Connect make their own font, so the main window replacing its font while they are open leaves them a live one; 2026-11-11 live: both dialogs' controls answered WM_GETFONT with a font other than the main window's. [#24](https://github.com/CodingDiego/rexplorer/issues/24) |
| High contrast | UNVERIFIED | `theme.rs` `effective_theme` | Built 2026-11-10: system colors, outlines for hover and selection, live switch. Never seen under a real contrast theme. [#23](https://github.com/CodingDiego/rexplorer/issues/23) |
| Screen reader support for the custom controls | PARTIAL | `core/access.rs` | MSAA for Home, tabs, breadcrumb, sidebar, toolbars, palette, status bar, preview and details pane. 2026-11-11 live: an MSAA walk of every visible child window on Home and on a folder with each pane open found no unnamed focusable or interactive object; smoke `accessibility` now does this walk (it read an empty tree through managed UIA before). Fixed that day: the address box, search box, file list and text preview had no name (now "Address", "Search", "Items View", "Preview"). The check was shown to fail: a throwaway build answering "" for each custom control's first child made smoke `accessibility` FAIL with 12 unnamed objects (tabs, toolbars, crumbs, sidebar, Home). Not tried with Narrator or NVDA. [#21](https://github.com/CodingDiego/rexplorer/issues/21) [#25](https://github.com/CodingDiego/rexplorer/issues/25) |
| Show hidden files, show extensions | UNVERIFIED | `folder_options.rs:663`, `:1087` | |
| Copy as path, Ctrl+Shift+C | WORKS | `list.rs` `copy_as_path` | 2026-11-11 live: a real Ctrl+Shift+C on a row put its full path in quotes on the clipboard (the clipboard was saved first and restored after). Refuses names with quotes or line breaks (archive folders). |
| Size all columns to fit, Ctrl+numpad Plus | UNVERIFIED | `list.rs` `fit_columns` | Built 2026-11-10; widths grew on the exe by command. |
| Esc cancels a cut | WORKS | `list.rs` `cancel_cut` | 2026-11-11 live: after a real Ctrl+X, a real Esc removed the faded state and emptied the clipboard. The file stayed. |
| Keyboard in the tab strip and on Home | PARTIAL | `tabs.rs`, `home.rs` | Built 2026-11-10: Left, Right, Home, End on tabs; Shift and Ctrl with arrows, Space and type-ahead on Home. 2026-11-11 live: real arrow keys on a focused tab strip moved between tabs (focus rectangle seen). Home keys not pressed. |
| Language | WORKS | `core/i18n.rs`, `lang/es.ini` | English built in, Spanish shipped, `language=` in `[general]` (default: Windows' language), `lang\<code>.ini` overrides, restart to apply. 2026-11-11 live with `language=es`: menus, command bar, status bar, Folder options, palette and question boxes are in Spanish, checked on screenshots. Fixed that day: four Folder options labels were clipped and the pin entries said "Anclar a Inicio" (Windows' own words for Pin to Start); the shorter texts fit. |
| Settings file format and old profiles | WORKS | `core/settings.rs` `loaded_format` | 2026-11-11 live on a 0.1.x profile: the first start only adds `format=1` to `settings.ini` and stamps the theme file; comments, CRLF and unknown keys stay; a second start changes nothing; a Folder options change written back keeps the unknown keys; `session.txt` is rewritten in the new format. [#51](https://github.com/CodingDiego/rexplorer/issues/51) |
| One instance per profile; hand-off version | WORKS | `instance.rs` | 2026-11-11 live: a second copy started on another folder handed off and exited 0 in either start order, adding a tab to the first; `WM_COPYDATA` with an unknown protocol version (`v=2`) is refused, `v=1` and none are taken. [#52](https://github.com/CodingDiego/rexplorer/issues/52) |
| Hang log | WORKS | `watchdog.rs` | 2026-11-11 live: with the window thread suspended 7 s, `rexplorer.log` gained "UI thread blocked 5 s" and then "answered again after 6 s"; a modal About box held 11 s logged nothing. [#31](https://github.com/CodingDiego/rexplorer/issues/31) |
| Update notice | WORKS | `core/update.rs` | 2026-11-11 live: off, no request and no stamp; on, one real request stamped the current version and showed nothing; a fresh stamp of a newer version was not fetched again and showed "rexplorer 9.9.9 is out" in the status bar and a More menu entry. [#66](https://github.com/CodingDiego/rexplorer/issues/66) |

## Order of work

1. Done 2026-11-11 (5 of 5 each, final build): F2, click-again rename and dragging to another program, in a folder
   and on Home. Still to try: dragging into a browser, WhatsApp and VS Code (#75), Home's recent-file rows. Then
   every UNVERIFIED row built on 2026-11-09 and 2026-11-10.
2. Try `--desktop-icons`, a Windows contrast theme, and Narrator.
3. The v1 round's rows: toolbar keys, indexed search, results Back, Home sort, archive thumbnails, Text size,
   then read `rexplorer.log` after each failed drag to another program.
4. Undo for the cases not recorded yet.
5. Sort and rename in search results, thumbnails of archive members, a message when rename in a .rar is refused.

Deliberate differences from Explorer: Backspace goes up, Ctrl+D pins, Ctrl+Shift+E opens Explorer, F4 has no
dropdown, and F10 opens no menu bar.
