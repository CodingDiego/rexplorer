# rexplorer user guide

rexplorer is a file manager for Windows. It opens folders in tabs, with a sidebar, a preview pane and the same
right-click menu as Windows Explorer. This guide is for people who use it. The README has the build notes and the
developer details. The release notes list what changed in each version; where this guide says "see the release
notes", a feature is still being finished.

## Install

rexplorer installs for your user only. It does not need an administrator and does not touch `HKLM`.

1. Get `rexplorer.exe` (or build it: `cargo build --release`).
2. Run it once to try it. Nothing is changed on your PC yet.
3. To install, run `rexplorer.exe --install`, or open **More > Install as default file manager...** in the app.
   You can also run `powershell -ExecutionPolicy Bypass -File scripts\install.ps1`.

Install copies the program to `%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe`. Windows may show a SmartScreen
warning the first time you run a file that is not signed. Choose to run it only if you trust where you got it.

Install first warns you that it replaces Explorer's folder windows and any running rexplorer, and quits with code 2
if you say No. If it fails halfway, a dialog offers **Roll back** (puts everything back) or **Keep changes**.

The install can also copy your Windows Explorer settings (hidden files, extensions, status bar, start folder and
a few more). You can do this any time with `rexplorer.exe --import-explorer`. It keeps a backup named
`settings.ini.bak-<date>` first. Add `--overwrite` to replace values you changed in rexplorer.

## Make it the default, and what that changes

After install, these open in rexplorer:

- Opening a folder or drive from the desktop, a shortcut or another program.
- Win+E, the taskbar's File Explorer icon and Start's File Explorer (they open at Home).
- Programs that run `explorer.exe` by its bare name, such as `explorer.exe /select,<file>`.
- Right-click on the desktop: rexplorer shows its own menu. This needs a small helper, `rexplorer --desktop`,
  that starts when you sign in.

These stay Windows Explorer's: the desktop itself, the taskbar, Start, other programs' Open and Save dialogs,
Explorer windows that were already open, and programs that start `C:\Windows\explorer.exe` by its full path
(many "Show in folder" buttons).

Everything install created is listed in `%LOCALAPPDATA%\Programs\rexplorer\install.txt`.

## Go back to Explorer

Three ways, from nicest to last resort.

1. In rexplorer: **More > Uninstall...**.
2. From a terminal or Win+R: `%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.
   If you still have the scripts: `powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -Uninstall`.
3. If rexplorer will not start at all, Windows Explorer is still there. Press Win+R, type
   `C:\Windows\explorer.exe` and press Enter to get an Explorer window. Then press Win+R again and run the
   command in step 2.

Uninstall shows a summary and removes the registry values that still name rexplorer, the Start shortcut, the
Startup shortcut for the desktop menu and the program copy. A value another program changed since is left alone.
Your settings in `%APPDATA%\rexplorer` stay, and an import from Explorer is not undone.

If rexplorer crashes, it restarts itself and restores your tabs. If it crashes again within 60 seconds, it opens
the folder in Windows Explorer instead, so a crash loop ends in Explorer.

## Keys

These are the defaults, taken from the key table in the app. Where a key is rebound, your setting wins.

### Move around

| Keys | Action |
|---|---|
| Enter, double-click | Open a folder, archive or file |
| Backspace, Alt+Up | Up one folder |
| Alt+Left, Alt+Right (mouse back and forward buttons) | Back, forward |
| F5, Ctrl+R | Refresh |
| Ctrl+L, Alt+D, F4 | Edit the address |
| Ctrl+F, Ctrl+E, F3 | Search box (Enter searches, Esc stops) |
| Ctrl+P | Palette: recent places, bookmarks and subfolders, fuzzy matched |
| Ctrl+D | Pin the selected folders (or this folder) to Home |
| F6, Shift+F6 (also Tab, Shift+Tab) | Next, previous pane |
| Ctrl+Shift+E | Open this folder in Windows Explorer |
| Esc | Stop a search, or cancel a cut |

### Tabs and windows

| Keys | Action |
|---|---|
| Ctrl+T, Ctrl+W | New tab, close tab |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| Ctrl+1 to Ctrl+8, Ctrl+9 | Tab number n, last tab |
| Ctrl+Enter, Ctrl+double-click, middle-click on a folder | Open in a background tab |
| Ctrl+N | New window |
| F11 | Full screen |

### Files

| Keys | Action |
|---|---|
| F2, Delete, Shift+Delete, Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+A, Alt+Enter | The usual verbs on the selection (Alt+Enter is Properties) |
| Ctrl+Shift+N | New folder, then rename it |
| Ctrl+Z, Ctrl+Y | Undo, redo |
| Ctrl+I | Invert the selection |
| Ctrl+Shift+C | Copy the selected paths as text |
| Ctrl+` | Terminal in this folder |
| Right-click, Shift+F10, Menu key | The full shell context menu (Shift adds extended verbs) |

### View and panes

| Keys | Action |
|---|---|
| Ctrl+B | Show or hide the sidebar |
| Alt+P | Show or hide the preview pane |
| Alt+Shift+P | Show or hide the details pane |
| Ctrl+Shift+1 to 8 | Extra large icons, large icons, medium icons, small icons, list, details, tiles, content |
| Ctrl+wheel | Step through the views |
| Ctrl+numpad Plus | Fit every column to its content |

The Thumbnails view is in the View menu and has no default key.

### Change a key

Add a `[keys]` section to `settings.ini`, one line per command. Several chords are separated by commas:

```ini
[keys]
cmd.new_tab=Ctrl+K
cmd.back=Alt+Left, Backspace
```

If you bind a chord that another command already uses, it is taken from that command. For example, `Ctrl+Y` is
Redo by default, so giving it to another command removes it from Redo.

Commands: `cmd.back`, `cmd.forward`, `cmd.up`, `cmd.refresh`, `cmd.address`, `cmd.explorer`, `cmd.enter`,
`cmd.escape`, `cmd.properties`, `cmd.new_tab`, `cmd.close_tab`, `cmd.next_tab`, `cmd.prev_tab`, `cmd.palette`,
`cmd.terminal`, `cmd.toggle_sidebar`, `cmd.toggle_preview`, `cmd.toggle_details`, `cmd.search`, `cmd.pin`,
`cmd.view_details`, `cmd.view_icons`, `cmd.view_thumbnails`, `cmd.view_extra_large`, `cmd.view_medium`,
`cmd.view_small`, `cmd.view_list`, `cmd.view_tiles`, `cmd.view_content`, `cmd.new_folder`, `cmd.open_tab`,
`cmd.fullscreen`, `cmd.invert_selection`, `cmd.next_pane`, `cmd.prev_pane`, `cmd.new_window`, `cmd.undo`,
`cmd.redo`, `cmd.copy_path`, `cmd.fit_columns`.

A chord is any of `Ctrl`, `Alt`, `Shift` plus one key. The key can be a letter, digit or backquote (these need Ctrl
or Alt), `F1` to `F24`, `Left`, `Right`, `Up`, `Down`, `Enter`, `Esc`, `Tab`, `Space`, `Backspace`, `Delete`,
`Insert`, `Home`, `End`, `PageUp` or `PageDown`. A line with no valid chord is ignored and the command keeps its
default keys. `Win` is not accepted.

## Settings

Most settings are in **More > Folder options...** (pages General, View and Search). Everything is also in
`%APPDATA%\rexplorer\settings.ini`, a plain text file you can edit. **More > Settings folder** opens it. Lines
starting with `;` or `#` are comments. A missing key takes its default. A value that cannot be read is ignored.

`[general]`:

| Key | Type | Default | Effect |
|---|---|---|---|
| `profile` | name | `default` | Which `[profile <name>]` is active. `--profile <name>` picks one for a single start. |
| `language` | `system`, `en`, `es` or a code with a `lang\<code>.ini` | `system` | Language of rexplorer's own text. Applies at the next start. |

`[profile <name>]`:

| Key | Type | Default | Effect |
|---|---|---|---|
| `start` | path, `shell:` name or `::{CLSID}` | Home (`shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}`) | Where a plain start and a new tab open. |
| `theme` | theme name, or `system` | `dark` | Theme file in `themes\` without `.ini`. `system` follows Windows' app mode (light or dark). |
| `view` | `details`, `small`, `medium`, `icons`, `thumbnails`, `extra-large`, `list`, `tiles` or `content` | `details` | View for a first run or `--profile`; later runs use the saved session. |
| `show_tree` | true or false | `false` | Folder tree instead of the sidebar. |
| `show_preview` | true or false | `false` | Preview pane. |
| `tree_width` | number | `220` | Sidebar or tree width in pixels at 96 DPI. |
| `preview_width` | number | `360` | Preview pane width, same units. |
| `show_hidden` | true or false | `false` | Show hidden files, folders and drives. |
| `show_extensions` | true or false | `true` | Show file extensions. |
| `single_click` | true or false | `false` | Open items with one click; pointing selects. |
| `restore_tabs` | true or false | `false` | Reopen the last session's tabs at a plain start. |
| `search_delay_ms` | number | `250` | Wait after you stop typing before a search starts. The dialog accepts 50 to 5000. |
| `hover_preview_ms` | number | `2000` | Wait before a hover preview. Values are limited to 200 to 10000. |
| `underline_titles` | `hover` or `always` | `hover` | With single-click on, when item names are underlined. |
| `show_recent` | true or false | `true` | Recent files on Home. |
| `show_frequent` | true or false | `true` | Frequent folders on Home. |
| `show_status` | true or false | `true` | Status bar. |
| `preview_handlers` | true or false | `true` | Use Windows preview handlers in the preview pane. |
| `item_checkboxes` | true or false | `false` | Check boxes for selecting items. |
| `search_system` | true or false | `true` | Searches of folders that are not indexed include system folders. |
| `search_archives` | true or false | `false` | Those searches also look inside ZIP, CAB and similar files. |
| `check_updates` | true or false | `false` | Opt-in. At most once a day, one HTTPS request to GitHub asks for the latest release; a newer one shows a notice. Nothing is downloaded or run. |
| `bookmark.<label>` | path | none | A bookmark: shown under Favorites, in the palette and in More > Quick access. |

`[keys]` is described above.

Other files in `%APPDATA%\rexplorer` are written by the app: `session.txt` (open tabs and pane state),
`recent.txt` (palette history), `window.txt` (window position), `folders.txt` (each folder's view, sort and
columns) and `last-folder.txt` and `crash-count.txt` (crash recovery). You can delete them to reset that state.

## Themes and icon themes

`theme=` in a profile names a file in `%APPDATA%\rexplorer\themes\`. `light.ini` and `dark.ini` are written on
first run, and you can edit them or add your own. Each file has one `[theme]` section. Colors are `#RRGGBB`.

| Key | Type | Default (dark) | Effect |
|---|---|---|---|
| `dark` | true or false | `true` | Which built-in theme missing keys fall back to. |
| `mica` | true or false | `true` | Mica backdrop where Windows supports it. |
| `background` | color | `#1E1E1E` | Window frame, tabs, header, sidebar, status bar. |
| `surface` | color | `#262626` | Cards and fields: the list, preview, path bar and search field. |
| `text` | color | `#F2F2F7` | Main text. |
| `secondary_text` | color | `#98989D` | Section and column titles, status bar. |
| `accent` | color | `#0A84FF` | Accent color. |
| `selection` | color | accent mixed 18% into the surface | Selected items. |
| `border` | color | background mixed 8% toward the text | Kept for themes; nothing draws border lines now. |
| `icon_theme` | folder name | none | A folder under `icons\`. |
| `font` | font name | `Segoe UI Variable Text` | Font. |
| `font_size` | number | `10` | Size in points. |

The built-in light theme uses `#F5F5F7` for the background, `#FFFFFF` for the surface, `#1D1D1F` for the text,
`#6E6E73` for the secondary text and `#0A84FF` for the accent.

An icon theme is a folder `icons\<name>\` with PNG files named by extension (`rs.png`, `pdf.png`), plus
`folder.png` and `file.png` for everything else. With `icon_theme=default`, rexplorer also exports Windows' own
icons there in the background the first time; it never overwrites a PNG, so you can replace any of them.

When Windows is in a high contrast theme, rexplorer uses the system colors instead of the theme file. This has not
been tested under a contrast theme yet; see the release notes.

## Languages

rexplorer speaks English and Spanish. With `language=system` it follows Windows. Set `language=en` or
`language=es` under `[general]` and restart to choose. To add or change a language, put a UTF-8 file
`%APPDATA%\rexplorer\lang\<code>.ini` with `key=text` lines (under 1 MiB) in place and set `language=<code>`.
Keys you leave out stay in English.

## Command line

```
rexplorer [options] [folder...]
  --select <path>     open the parent folder with <path> selected (also /select,<path>)
  --profile <name>    use the named settings profile
  --new-tab, -t       open in a new tab of the running window
  --new-window        open a second window (it saves no tabs, placement or recents)
  --desktop-icons     show the desktop's icons in a rexplorer window (opt-in, primary monitor)
  --search <text>     search the (first) folder opened for <text>
  --register          open folders and Win+E with this exe (per user; asks first)
  --unregister        undo --register
  --install           copy rexplorer to %LOCALAPPDATA%\Programs and make it the default file manager
  --uninstall         undo --install
  --quiet             with --install or --uninstall: no dialogs; a failed install rolls back
  --import-explorer   copy Windows Explorer's settings into the active profile (backs up settings.ini)
  --overwrite         with --import-explorer: also replace values you changed in rexplorer
  --recover           restore the previous session after a crash
  --desktop           run the helper that shows rexplorer's menu on the desktop
  --elevated          when started elevated, stay so instead of restarting unelevated
  -h, --help          show this help
```

`--install` and `--uninstall` exit with 0 when done, 1 when failed and 2 when cancelled at the confirmation. With
`--quiet` nothing is asked, so the result is the exit code alone.

Folders can be paths, `shell:` names, `::{CLSID}` strings, UNC paths or .rar paths. Explorer-style arguments work
too: `/select,<path>`, `/e,<path>`, `/root,<path>`, `/n` and `/separate`. A second launch passes its folders to
the window that is already running and exits. Arguments rexplorer does not understand (URLs such as
`ms-settings:display`, an unknown `/` switch) are handed to `C:\Windows\explorer.exe`.

## Troubleshooting

**I want Explorer back right now.** See "Go back to Explorer" above. If nothing starts, Win+R, then
`C:\Windows\explorer.exe`.

**A folder seems to hang.** After about 120 ms a folder that is still loading shows a placeholder with its name
and "Loading..." in the status bar, so the window is not frozen. Big folders, network drives and slow shell
extensions can take a while. Wait, or press Back or open another folder. F5 tries again. If one path always hangs
(for example a disconnected network share), open it in Windows Explorer with Ctrl+Shift+E.

**Double-clicking a folder does nothing.** Try Enter. If a folder opened from another program does nothing,
rexplorer may not be reaching the running window. Check that the app is not stuck, then try Ctrl+Shift+E to open
the folder in Explorer. If this keeps happening, uninstall (see above).

**Dragging files to other programs does not work.** This is a known problem; see the release notes and
`docs/EXPLORER-PARITY.md`. Copy and paste (Ctrl+C, Ctrl+V) works.

**F2 or click-again rename does not work.** Reported but not reproduced; see `docs/EXPLORER-PARITY.md`. Right-click,
Rename is the fallback.

**Search is slow or misses files.** Searches of non-indexed folders walk the disk. Turn off
**Include system folders** or **Include compressed files** in Folder options > Search. A search of a whole
drive started from `rexplorer --elevated` uses the drive's index instead.

**rexplorer starts unelevated even though I ran it as administrator.** On purpose: Explorer never shows an
elevated folder window. Use `rexplorer --elevated` from an administrator terminal to stay elevated.

**My settings are broken.** Close rexplorer and edit or delete `%APPDATA%\rexplorer\settings.ini`; a fresh one is
written with defaults. Unreadable values are ignored anyway. Backups made by `--import-explorer` are the
`settings.ini.bak-<date>` files in the same folder.

**It crashed.** It restarts itself and restores your tabs (`--recover`). The second crash within 60 seconds opens
the folder in Windows Explorer instead. Two things are kept on your disk, and nothing is uploaded:

- `%APPDATA%\rexplorer\rexplorer.log` records hangs of the window, panics and failed drags. It starts again once it
  passes 64 KB.
- `%LOCALAPPDATA%\rexplorer\crashes` holds a crash dump (`.dmp`) for each crash, plus a `.txt` report with the
  message and stack for a panic. The newest 10 dumps are kept. If that variable is not set, the temp folder is used.

To report a crash, attach what you are comfortable sharing from these files and say what you did before it and
which version you ran.
