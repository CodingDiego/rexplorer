# Changelog

Every release, newest first. `.github/release-notes.md` holds the text of the release being prepared; at release
time its content is added here under the new version, including its Known gaps. Every user-visible change adds
a line under Unreleased. Sections up to 0.1.4 were rebuilt from git history and predate the Known gaps rule.

## Unreleased

Nothing yet.

## 0.2.0 - 2026-11-11

Changes since 0.1.4. The release notes of this version (`.github/release-notes.md` at the tag) carry its Known gaps;
`docs/EXPLORER-PARITY.md` marks what was proven on the exe (WORKS) and what is built but not yet tried (UNVERIFIED).

**Changed.**
- `--install` is a standalone install: the program, a Start menu shortcut and a Settings > Apps entry, and no folder,
  drive, App Paths or Startup change. Making rexplorer the default file manager is `--register` (More > Make
  rexplorer the default file manager), journaled in `takeover.txt`; `--unregister` (More > Give folders back to
  Windows Explorer) and uninstall revert it. `--register` and `--unregister` take `--quiet` and exit 0, 1 or 2, and
  honour the test registry base.
- `scripts\install.ps1` installs the program only; new `scripts\master-install.ps1` installs and makes rexplorer the
  default after a `.reg` backup of the keys it changes; new `scripts\restore-explorer.ps1` puts Windows Explorer back
  without a working rexplorer.exe. All three take `-Quiet` and `-WhatIf`.

**Added.**
- Undo and redo (Ctrl+Z, Ctrl+Y) of rename, new items, delete to the Recycle Bin, paste and drops on the list.
- Drag and drop: the shell's drag image, right-button drag, drops on the sidebar, breadcrumb, tabs and Home tiles,
  drag from Home and from search results, cut items shown faded.
- Tabs: drag to reorder, tab menu, tear out to a new window, restore tabs.
- Views: four icon sizes, List, Tiles and Content, Ctrl+wheel through all views, column chooser, group by any
  column, and each folder's view, sort, group and columns remembered (`folders.txt`).
- Keys: Ctrl+Shift+N, Ctrl+N, Ctrl+Enter, F3 / Ctrl+E, F4, F6, F11, Ctrl+I, Ctrl+Shift+C, Ctrl+numpad Plus, Tab and
  Shift+Tab between panes, Esc to cancel a cut. The sidebar, tab strip and Home page work from the keyboard.
- Details pane (Alt+Shift+P), address autocomplete, back and forward history menus, icon overlays, `theme=system`,
  Libraries and Recycle Bin rows, search results with icons, selected size in the status bar.
- High contrast: with a Windows contrast theme on, every custom surface uses the system colors.
- Screen readers: the custom controls expose MSAA.
- `--desktop-icons`: an opt-in view of the desktop's icons in a rexplorer window.
- Spanish, chosen by Windows' language or `language=es`.
- Desktop menu: right-drag on desktop icons, and Shift+F10 or the Menu key on the desktop.
- Taskbar: one AppUserModelID (`CodingDiego.rexplorer`) for the process, its windows and the installed Start
  shortcut, and a jump list with Frequent and Recent folders and a New window task.
- Paste shortcut in the background menu; Run as administrator (and Ctrl+Shift+Enter) on programs.
- Breadcrumb dropdowns for every shell folder (This PC, Network, shares, Control Panel), listed in the background,
  and the folder icon's dropdown of the desktop's places, which reaches Control Panel and Libraries.
- Cloud file marks from the file attributes: online-only and always kept on this device.
- cargo-fuzz targets for the settings, theme, language, command-line and archive-name parsers in `fuzz/`.
- Search filters in Explorer's syntax, with English and Spanish keys: `kind:`, `date:`, `size:`, `ext:` and quoted
  phrases, for the walk, the Windows Search index and the volume index alike.
- Encrypted .rar: a password prompt (kept in memory only while the archive is shown, never written to disk).
- Archive members can be dragged out to other programs (extracted to a per-process temp folder first).
- `%ENV%` names in the address bar.

**Fixes.**
- Rename selects the name without its extension.
- A clipboard change only re-evaluates Paste.
- Pin and Unpin run in the background, one at a time.
- As-you-type search starts at 2 characters; Enter searches any text.
- A change notification no longer restarts an armed timer, so a folder written every few milliseconds still
  reloads about once a second.
- Refresh and sort keep the selection on the exact same names and keep the Shift+click anchor on the focused item.
- The volume index is built per drive outside the cache lock.
- Back while a folder is still loading returns to the folder you were on; it used to do nothing.
- On search results, entering the searched folder's path in the address bar opens that folder instead of searching
  again.
- The preview pane no longer crashes the window when the selected entry has no detail line (seen on Home).
- Folder options and Connect make their own font: a text size change while they are open no longer leaves them
  with the main window's deleted one.
- Copy, Cut and Paste no longer leave a thread behind each time (the Work Folders context-menu handler on Windows 11
  started one per use; a 15-minute soak grew from 50 to 144 threads).
- Rename: after Tab renamed one row and moved on, Shift+Tab back edited the old name.
- Undo: Ctrl+Z after a paste from a folder row's menu, after any paste from a folder under %TEMP%, and after
  Create shortcuts here did nothing.
- The first click on a Date or Size header sorts newest or largest first, as Explorer, and headers show a hover
  fill (#76, #79).
- Home: a right-button press never started a drag.
- Down on the toolbar's More button did nothing.
- The item menu listed rexplorer's pin entry and the shell's own.
- Properties on search results from several folders did nothing; it opens one sheet for all of them.
- An indexed search in a moved Documents folder found nothing.
- Paths over 260 characters: rename failed (the shell's rename stops there) and Delete failed without asking;
  Delete there now asks to delete permanently, since the Recycle Bin cannot take such paths.
- The address box, search box, file list and text preview had no accessible name.
- A crash relaunch split an argument that ended in a backslash.
- Extraction counted a link entry that unrar refused without an error as extracted.
- Spanish: four Folder options labels were clipped, and the details pane's values overlapped long labels.

**Known gaps.** As in the release notes: 61 behaviours of `docs/EXPLORER-PARITY.md` were proven on this build (and
3 more on a scratch install base), 44 are built and unit-tested but not yet tried on the exe, 10 are partial.
- Not yet tried by a person: Narrator or NVDA, a Windows contrast theme, Text size above 100% and other display
  scales, two monitors, ARM64 hardware, Windows 10, and the jump list on the taskbar itself (checked in its file).
- Cloud marks were never seen on real OneDrive placeholders, and show only in Details and List. Run as
  administrator was never invoked in testing.
- MTP phones open in an embedded Explorer view; rename in search results is unverified.
- The exe is not code-signed, so SmartScreen may warn.

## 0.1.4 - 2026-10-11

Lower CPU use and quicker responses: rexplorer no longer reloads a folder on every change notification, preloads
less, and keeps slow shell work off the window's thread.

**CPU and latency.** Measured on the developer's machine against 0.1.1.
- Change notifications (#11): a folder whose files keep changing reloads at most once a second, and not at all while
  the window is minimized. Idle CPU is about 7x lower.
- Sidebar (#12): rebuilt every 30 seconds while shown instead of every 5, and right away when a drive is added or
  removed.
- Hover preview (#9): only text files and light images are loaded ahead, only once the pointer has stopped, and
  only while rexplorer is the active window.
- Preview pane (#10): showing the same file again keeps its preview handler.
- Home (#13): built faster at startup, icons repaint in batches.
- First launch (#14): Windows' system icons are exported only when the theme uses them. First-launch CPU went from
  about 4.5 s to 0.5 s.
- Icons view (#15): heavy types show icons instead of rendered thumbnails.
- Open terminal (#16): starts in the background.
- From the #17 backlog: sorting on the load thread with cached cell text and icons; tab switches reload only
  changed folders; fewer and cheaper thumbnails; Copy, Cut, Paste, Delete and Properties no longer load every
  shell extension; superseded navigations and searches stop early; a second launch waits for a busy first window.

**Fixes.**
- Sorting keeps the selection on the same files, and no longer leaves thumbnails on the wrong files in the Icons
  and Thumbnails views.
- Open terminal finds Windows Terminal installed outside the Store on PATH, and never runs a `wt.exe` found through
  the browsed folder.

## 0.1.3 - 2026-10-10

rexplorer's context menu on the desktop.

- A small helper (`rexplorer --desktop`, one per sign-in) shows rexplorer's menu when you right-click the desktop:
  the shell's verbs on icons, and View, Sort by, Refresh and Paste plus the shell's background verbs on the
  background. Explorer still draws the desktop, the taskbar and Start. Install adds a Startup shortcut for it.
- Install and uninstall close a running helper before they replace or remove the exe, and start it again afterwards.
  Roll back also removes or restores the Start menu and Startup shortcuts.
- Not covered in this release: right-dragging desktop icons, and Shift+F10 or the Menu key on the desktop.

## 0.1.2 - 2026-10-10

A safer installer: it warns before it changes anything, and a failed install can be rolled back.

- `--install` always asks first. The warning says it replaces Windows Explorer for folders, drives, Win+E and the
  taskbar's File Explorer. No is the default; answering No exits with code 2 and changes nothing.
- If the install fails partway, a dialog offers Roll back (the default) or Keep changes. Roll back restores the
  registry values, the previous exe, `install.txt` and the Start menu shortcut.
- An update keeps the previous exe aside until the install succeeds.

## 0.1.1 - 2026-10-10

Bug fixes from using rexplorer as the daily file manager.

- Box selection (#5): in Details view, a drag from a row's Date, Type or Size column draws a selection rectangle.
- Delete on Home's cloud files (#3): no error; Delete beeps and the command bar's Delete is disabled.
- New tab (#6): Ctrl+T and the tab strip's + open the start place.
- Launch (#4): a plain launch opens the start place, as Explorer does. View and pane widths still come from the
  last session.
- Shift+F10 and the Menu key open the context menu in the file list, and on Home under the focused item.
- "Always underline" in single-click mode underlines item names only.
- Edits to `settings.ini` keep the file's line endings and spacing.

## 0.1.0 - 2026-10-10

First public release: a Rust and Win32 file manager that replaces Windows Explorer's folder windows.

- Tabs, Home page with Quick Access tiles, search as you type, Icons and Thumbnails views with real image
  thumbnails, preview pane, RAR archives browsed in place, FTP connections, Folder options dialog with
  Explorer-compatible settings and custom shortcuts, and a fallback to the embedded Explorer view for anything it
  cannot list.
- `--install` makes it the default file manager for your user without administrator rights; `--import-explorer`
  copies your Explorer settings; `--uninstall` undoes the install.
- Security fixes from a whole-project review: RAR path traversal guard and libunrar 7.21, Mark-of-the-Web kept on
  extracted files, out-of-process preview handlers, FTP passwords never shown or saved, and no elevated folder
  windows unless `--elevated` is passed.
- Builds come from GitHub Actions; `SHA256SUMS.txt` lists the checksums. The exe is not code-signed.
