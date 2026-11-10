# Changelog

Every release, newest first. `.github/release-notes.md` holds the text of the release being prepared; at release
time its content is added here under the new version, including its Known gaps. Every user-visible change adds
a line under Unreleased. Sections up to 0.1.4 were rebuilt from git history and predate the Known gaps rule.

## Unreleased

Changes on `master` since 0.1.4. Several of the mouse-driven items are built and unit-tested but not yet tried with
a real mouse; `docs/EXPLORER-PARITY.md` marks them UNVERIFIED.

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

**Fixes.**
- Rename selects the name without its extension.
- A clipboard change only re-evaluates Paste.
- Pin and Unpin run in the background, one at a time.
- As-you-type search starts at 2 characters; Enter searches any text.
- A change notification no longer restarts an armed timer, so a folder written every few milliseconds still
  reloads about once a second.
- Refresh and sort keep the selection on the exact same names and keep the Shift+click anchor on the focused item.
- The volume index is built per drive outside the cache lock.

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
