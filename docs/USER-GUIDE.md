# rexplorer user guide

rexplorer is a file manager for Windows. It opens folders in tabs, with a sidebar, a preview pane and the same
right-click menu as Windows Explorer. This guide is for people who use it. The README has the build notes and the
developer details. Some features have not been checked by a person yet. The release notes list them under "Known
gaps": https://github.com/CodingDiego/rexplorer/releases

## Contents

- [Words used in this guide](#words-used-in-this-guide)
- [Install](#install)
- [Run a script](#run-a-script)
- [Make it the default, and what that changes](#make-it-the-default-and-what-that-changes)
- [Go back to Explorer](#go-back-to-explorer)
- [When rexplorer will not start](#when-rexplorer-will-not-start)
- [Keys](#keys)
- [Settings](#settings)
- [Themes and icon themes](#themes-and-icon-themes)
- [Languages](#languages)
- [Command line](#command-line)
- [Troubleshooting](#troubleshooting)

## Words used in this guide

- **Win+R**: hold the Windows key and press R. The Run box opens. Type or paste a command and press Enter. The Run
  box understands names such as `%LOCALAPPDATA%` and `%APPDATA%`.
- **Command bar**: the row of buttons in the rexplorer window that starts with **New**, **Cut**, **Copy** and
  **Paste**.
- **More**: the button with three dots in the command bar. Its tooltip says "More". "More > Folder options..."
  means: click More, then click Folder options... in the menu that opens.
- **Install folder**: `%LOCALAPPDATA%\Programs\rexplorer`. Installing copies `rexplorer.exe` there.
- **Settings folder**: `%APPDATA%\rexplorer`. Your settings live there. Paste it into Win+R to open it.
- **Default file manager**: the program Windows starts when you open a folder or a drive, or press Win+E. Out of
  the box that is Windows Explorer. rexplorer can take that place, and give it back.

## Install

rexplorer changes only your own Windows account. It never asks for administrator rights and does not touch `HKLM`.

### 1. Get rexplorer

Download the zip from https://github.com/CodingDiego/rexplorer/releases. Most PCs need
`rexplorer-<version>-x64.zip`. `rexplorer-<version>-arm64.zip` is for ARM PCs. It has not been tested on ARM
hardware yet.

Right-click the zip and choose **Extract All**. The extracted folder holds `rexplorer.exe`, `install.ps1`,
`master-install.ps1` and `restore-explorer.ps1`. Keep this folder. Installing does not copy the scripts, and
`restore-explorer.ps1` is your way back to Explorer if rexplorer ever stops starting.

If you build rexplorer from source, `cargo build --release` writes `target\release\rexplorer.exe`. The scripts are
in the `scripts` folder of the source. The README has the build notes.

### 2. Try it

Double-click `rexplorer.exe`. A rexplorer window opens. Nothing is installed, and Windows Explorer still opens your
folders. rexplorer creates its settings folder on the first start.

The file is not signed, so Windows may show a SmartScreen warning the first time. Run it only if you trust where you
got it. To run it, click **More info**, then **Run anyway**.

### 3. Install it as a program

In rexplorer, choose **More > Install rexplorer...**. A question lists what will change. Choose **Yes**.

Install copies the program to `%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe`. It adds rexplorer to the Start menu
and to Settings > Apps, where you can uninstall it. It does **not** change which program opens folders. Windows
Explorer still does, and you start rexplorer yourself from the Start menu.

How to tell it worked: a message lists what was done. Its first line starts with "Copied to". rexplorer is now in the
Start menu.

If you choose **No**, nothing changes. If the install fails partway, a dialog offers two choices. **Roll back** undoes
every change this install made. **Keep changes** keeps them, and you can still undo them later with `--uninstall`.

You can also install with the script `install.ps1` from the extracted folder (see [Run a script](#run-a-script)). It
runs the same install, then asks whether to copy your Windows Explorer settings into rexplorer. Press Enter for
yes, or type `n` for no.

To update to a newer version, install the new `rexplorer.exe` the same way. Windows that are already open keep
working until you close them.

### Copy your Windows Explorer settings

rexplorer can read these Windows Explorer settings and use them: hidden files, file extensions, the status bar,
preview handlers, check boxes, the start folder, recent files and frequent folders on Home, and single-click to open.
Explorer's settings are only read, never changed. Run this in Win+R at any time:

```
%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --import-explorer
```

A message lists what was copied, what was skipped and why. A value is copied only when you have not changed it in
rexplorer. To replace those values too, add `--overwrite`:

```
%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --import-explorer --overwrite
```

Before it changes anything, it saves a copy of your settings as `settings.ini.bak-<date>` in the settings folder.

## Run a script

The scripts `install.ps1`, `master-install.ps1` and `restore-explorer.ps1` run in PowerShell, which comes with
Windows. To run one:

1. Open the folder that holds the script.
2. Hold Shift, right-click the script and choose **Copy as path**.
3. Press Win+R, type `powershell` and press Enter. A PowerShell window opens.
4. In that window, type `powershell -ExecutionPolicy Bypass -File ` (with a space at the end), then press Ctrl+V to
   paste the path. Press Enter.

The line then looks like this, with your own folder in it:

```
powershell -ExecutionPolicy Bypass -File "C:\Users\you\Downloads\rexplorer-1.0.0-x64\restore-explorer.ps1"
```

The script runs and prints what it did in the same window. Close the window when you are done. To add an option such
as `-RemoveProgram`, type it after the path, with a space before it.

## Make it the default, and what that changes

Making rexplorer the default file manager is a separate step. You can undo it at any time. **Install rexplorer
first** (see [Install](#install)): only the installed copy is ever made the default. Without an install, rexplorer
says it is not installed and changes nothing.

Choose one way:

- In rexplorer: **More > Make rexplorer the default file manager...**. A question lists the registry keys it will
  change. Choose **Yes**.
- In Win+R:

  ```
  %LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --register
  ```

- Install and make it the default in one go: run `master-install.ps1` (see [Run a script](#run-a-script)). Before it
  changes anything, it saves the keys it is about to change to
  `%LOCALAPPDATA%\rexplorer\backup\explorer-keys-<date>.reg`. At the end it prints where that file is and how to go
  back.

How to tell it worked: a message says "rexplorer now opens folders." Press Win+E. A rexplorer window opens.

If another program already took over one of these entry points, rexplorer changes nothing and names the key in its
message.

After that, these open in rexplorer:

- Opening a folder or drive from the desktop, a shortcut or another program.
- Win+E, the taskbar's File Explorer icon and Start's File Explorer. They open at your start folder, which is Home
  unless you changed it.
- Programs that run `explorer.exe` by its bare name, such as `explorer.exe /select,<file>`.
- Right-click on the desktop: rexplorer shows its own menu. This needs a small helper, `rexplorer --desktop`. It
  starts now and each time you sign in.

These stay Windows Explorer's: the desktop itself, the taskbar, Start, other programs' Open and Save dialogs,
Explorer windows that were already open, and programs that start `C:\Windows\explorer.exe` by its full path. Many
"Show in folder" buttons do that.

Everything install created is listed in `%LOCALAPPDATA%\Programs\rexplorer\install.txt`. What making it the default
changed is listed in `takeover.txt` in the same folder.

## Go back to Explorer

To keep rexplorer as a program but have folders open in Windows Explorer again, choose one way:

- In rexplorer: **More > Give folders back to Windows Explorer...**. This entry takes the place of "Make rexplorer
  the default file manager..." while rexplorer is the default. Choose **Yes**.
- In Win+R:

  ```
  %LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --unregister
  ```

How to tell it worked: a message says "Windows Explorer opens folders again." Press Win+E. A Windows Explorer window
opens. If rexplorer still opens, sign out and back in. Explorer windows that were already open, and the taskbar, can
keep the old setting until then.

To remove rexplorer completely, choose one way. Each gives folders back to Explorer first.

- In rexplorer: **More > Uninstall...**. It asks first.
- Settings > Apps > rexplorer > Uninstall.
- `install.ps1` with the option `-Uninstall` (see [Run a script](#run-a-script)).
- In Win+R. It asks first; add `--quiet` to skip the question:

  ```
  %LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall
  ```

Giving folders back and uninstalling remove only the registry values that still name rexplorer. A value another
program changed since is left alone. Both also remove the desktop menu helper's Startup shortcut. Uninstall also
removes the Start menu shortcut, the Settings > Apps entry and the program copy. Your settings in `%APPDATA%\rexplorer`
stay, and an import from Explorer is not undone.

## When rexplorer will not start

Windows Explorer is still there, and you can put it back without rexplorer.

1. Press Win+R, type this and press Enter. A Windows Explorer window opens.

   ```
   C:\Windows\explorer.exe
   ```

2. In that window, go to the folder you extracted from the release zip. If you no longer have it, download the same
   zip again and extract it (see [Get rexplorer](#1-get-rexplorer)). You do not need to install it again.
3. Run `restore-explorer.ps1` from that folder (see [Run a script](#run-a-script)). In a source checkout it is
   `scripts\restore-explorer.ps1`.

How to tell it worked: the script prints "Windows Explorer opens folders, drives and Win+E again."

The script closes the rexplorer windows that run from the install folder. It removes every setting that makes
rexplorer open folders. It reads the install records when they are there, and otherwise uses its own list. It keeps
rexplorer installed as a program. It is safe to run more than once. Options, typed after the script's path:

- `-RemoveProgram` also removes the program. This happens anyway when `rexplorer.exe` is already missing.
- `-WhatIf` shows what the script would do and changes nothing.
- `-RestartShell` restarts `explorer.exe` at the end, so the running Explorer shell drops the old setting. It only
  does this when no Explorer window and no file copy is open. Close every Explorer window first, including the one
  from step 1. Without this option, sign out and back in instead.

After a master install you can also put the saved keys back exactly as they were. Press Win+R, type `cmd` and press
Enter. In the window that opens, type this line, with `<date>` replaced by the date in the backup file's name. To see
that name, paste `%LOCALAPPDATA%\rexplorer\backup` into Win+R.

```
reg import "%LOCALAPPDATA%\rexplorer\backup\explorer-keys-<date>.reg"
```

If rexplorer crashes, it restarts itself and restores your tabs. A tab that was still loading when it crashed
opens Home instead, so a folder that crashes rexplorer is not opened again. If it crashes again within 60 seconds,
it opens the folder in Windows Explorer instead. So a crash loop ends in Explorer.

## Keys

These are the defaults, taken from the key table in the app. Where you rebind a key, your setting wins.

### Move around

| Keys | Action |
|---|---|
| Enter, double-click | Open a folder, archive or file |
| Alt+Up | Up one folder |
| Alt+Left or Backspace, Alt+Right (mouse back and forward buttons) | Back, forward. Backspace goes Back as in Windows 11 Explorer. Up to 0.2.0 it went up one folder |
| F5, Ctrl+R | Refresh |
| Ctrl+L, Alt+D | Edit the address |
| F4 | Edit the address, with your recent places listed under it (newest first). Pick one to go there |
| Esc in the address | Close the suggestion list first. A second Esc leaves the address |
| Right-click Back or Forward, or hold it about half a second | History menu |
| Alt or F10 alone | Move to the command bar (as in Windows 11 Explorer, there is no menu bar). Press again to come back |
| Ctrl+F, Ctrl+E, F3 | Search box (Enter searches, Esc stops) |
| Ctrl+P | Palette: recent places, bookmarks and subfolders, fuzzy matched |
| Ctrl+D | Pin the selected folders (or this folder) to Home |
| F6, Shift+F6 (also Tab, Shift+Tab) | Next, previous pane, the three toolbars included. They also leave a text preview |
| Left, Right, Home, End, then Enter, Space or Down (in a toolbar) | Move between buttons, then press one or open its menu (Down on Back or Forward opens the history). Esc returns to the files |
| Down past the last tiles on Home, then Left, Right, Space or Enter | Reach and pick the Recent, Favorites and Shared pills |
| Ctrl+Shift+E | Open this folder in Windows Explorer |
| Esc | Stop a search, or cancel a cut |

Long folder names in the address bar: each parent folder's name is cut at about 160 pixels (more at a larger
display scale or text size) and ends with "...". The current folder gets the rest of the space. It is cut only when
it cannot fit even alone. Hover a cut name to see it in full, or click the empty part of the bar to see the whole
path as text.

The sidebar's Favorites lists your bookmarks, then the folders pinned to Home (each marked with a pin), then the
folders you use most. Right-click a pinned folder to unpin it. Right-click a frequent one to pin it or to remove it
from the list. Only folders are listed: a `.zip` you opened is a recent file, and Home shows those.

### Tabs and windows

| Keys | Action |
|---|---|
| Ctrl+T, Ctrl+W | New tab, close tab |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| Ctrl+1 to Ctrl+8, Ctrl+9 | Tab number n, last tab |
| Ctrl+Enter, Ctrl+double-click, middle-click on a folder | Open in a background tab |
| Ctrl+N | New window |
| Drag a tab | Reorder it. Drop it on another rexplorer window to move it there, anywhere else to open it in a new window |
| F11 | Full screen |

### Files

| Keys | Action |
|---|---|
| F2, Delete, Shift+Delete, Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+A, Alt+Enter | The usual verbs on the selection (Alt+Enter is Properties) |
| F2, or click the name of a selected item again | Rename (the name is selected without its extension). Tab and Shift+Tab rename the next or previous item |
| Ctrl+Shift+N | New folder, then rename it |
| Right-click the background, New | The shell's New submenu. A new text document or folder opens its rename. A new shortcut is only selected, because the shortcut wizard asks for its name |
| Ctrl+Shift+Enter | Run the selected program as administrator |
| Ctrl+Z, Ctrl+Y | Undo, redo: rename, new items, delete, paste (also "Keep both" copies) and drops. Undo only touches a file it can prove it made or moved. Otherwise it beeps. A file overwritten by Replace cannot be brought back |
| Ctrl+I | Invert the selection |
| Ctrl+Shift+C | Copy the selected paths as text |
| Ctrl+` | Terminal in this folder |
| Right-click, Shift+F10, Menu key | The full shell context menu (Shift adds extended verbs) |

### View and panes

| Keys | Action |
|---|---|
| Ctrl+B | Show or hide the sidebar (More > Folder tree shows or hides the folder tree beside it) |
| Alt+P | Show or hide the preview pane |
| Alt+Shift+P | Show or hide the details pane |
| Ctrl+Shift+1 to 8 | Extra large icons, large icons, medium icons, small icons, list, details, tiles, content |
| Ctrl+wheel | Step through the views |
| Ctrl+numpad Plus | Fit every column to its content |

The Thumbnails view is in the **View** menu of the command bar and has no default key.

### Change a key

Add a `[keys]` section to `settings.ini` in the settings folder, one line per command. Separate several chords with
commas:

```ini
[keys]
cmd.new_tab=Ctrl+K
cmd.terminal=Ctrl+Alt+T, Ctrl+`
```

Close every rexplorer window and start it again to use the new keys.

A line replaces all default keys of that command. For example, a line for `cmd.next_pane` also takes F6 and Tab away
from it. If you bind a chord that another command already uses, it is taken from that command. For example, `Ctrl+Y`
is Redo by default, so giving it to another command removes it from Redo.

Do not give a command a key that types or edits text, such as Space or Backspace without Ctrl or Alt. That key would
then also run the command while you type in the address bar.

Commands: `cmd.back`, `cmd.forward`, `cmd.up`, `cmd.refresh`, `cmd.address`, `cmd.address_history`,
`cmd.explorer`, `cmd.enter`,
`cmd.escape`, `cmd.properties`, `cmd.new_tab`, `cmd.close_tab`, `cmd.next_tab`, `cmd.prev_tab`, `cmd.palette`,
`cmd.terminal`, `cmd.toggle_sidebar`, `cmd.toggle_preview`, `cmd.toggle_details`, `cmd.search`, `cmd.pin`,
`cmd.view_details`, `cmd.view_icons`, `cmd.view_thumbnails`, `cmd.view_extra_large`, `cmd.view_medium`,
`cmd.view_small`, `cmd.view_list`, `cmd.view_tiles`, `cmd.view_content`, `cmd.new_folder`, `cmd.open_tab`,
`cmd.fullscreen`, `cmd.invert_selection`, `cmd.next_pane`, `cmd.prev_pane`, `cmd.new_window`, `cmd.undo`,
`cmd.redo`, `cmd.copy_path`, `cmd.fit_columns`, `cmd.paste_shortcut`, `cmd.run_as_admin`. `cmd.view_icons` is
Large icons. `cmd.paste_shortcut` has no default key.

A chord is any combination of `Ctrl`, `Alt` and `Shift` plus one key. The key can be a letter, digit or backquote
(these need Ctrl or Alt), `F1` to `F24`, `Left`, `Right`, `Up`, `Down`, `Enter`, `Esc`, `Tab`, `Space`,
`Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp` or `PageDown`. A line with no valid chord is ignored and the
command keeps its default keys. `Win` is not accepted.

## Settings

Most settings are in **More > Folder options...**, on the pages General, View and Search. Changes made there apply
at once.

Everything is also in `settings.ini` in the settings folder (`%APPDATA%\rexplorer`), a plain text file you can edit
with Notepad. **More > Settings folder** opens that folder in a new tab. Lines starting with `;` or `#` are comments.
A missing key takes its default. A value that cannot be read is ignored. Close every rexplorer window before you
edit the file. Your changes apply the next time rexplorer starts.

A profile is a named set of settings. You can keep several and pick one. `settings.ini` has one `[general]` section
and one `[profile <name>]` section per profile.

`[general]`:

| Key | Type | Default | Effect |
|---|---|---|---|
| `profile` | name | `default` | Which `[profile <name>]` is active. A name with no section uses the first profile. `--profile <name>` picks one for a single start. |
| `language` | `system`, `en`, `es` or a code with a `lang\<code>.ini` | `system` | Language of rexplorer's own text. Applies at the next start. |

`[profile <name>]`:

| Key | Type | Default | Effect |
|---|---|---|---|
| `start` | path, `shell:` name or `::{CLSID}` | Home (`shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}`) | Where a plain start and a new tab open. |
| `theme` | theme name, or `system` | `dark` | Theme file in `themes\` without `.ini`. `system` follows Windows' app mode (light or dark). |
| `view` | `details`, `small`, `medium`, `icons`, `thumbnails`, `extra-large`, `list`, `tiles` or `content` | `details` | View for a first run or `--profile`. Later runs use the saved session. |
| `show_tree` | true or false | `false` | Folder tree, right of the sidebar (alone when the sidebar is hidden). |
| `show_preview` | true or false | `false` | Preview pane. |
| `expand_to_folder` | true or false | not set | Whether the folder tree opens the way to the current folder. Not set: it follows Explorer's own "Expand to open folder" option. Off, the tree selects a folder only when it already shows it (otherwise nothing). On, the way is opened only to a folder on a fixed or RAM disk with no junction, symbolic link or mount point on the way, or to the desktop, This PC, Home, Quick access or the user's files. Network, removable and other virtual folders are never expanded automatically. |
| `tree_width` | number | `220` | Width of the sidebar and of the tree (each) in pixels at 96 DPI. Limited to 120 to 600. |
| `preview_width` | number | `360` | Preview pane width, same units and limits. |
| `show_hidden` | true or false | `false` | Show hidden files, folders and drives. |
| `show_extensions` | true or false | `true` | Show file extensions. |
| `single_click` | true or false | `false` | Open items with one click. Pointing selects. |
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
| `check_updates` | true or false | `false` | Opt-in. At most once a day, one HTTPS request to GitHub asks for the latest release. A newer one shows a notice. Nothing is downloaded or run. |
| `bookmark.<label>` | path | none | A bookmark: shown under Favorites, in the palette and in More > Quick access. |

`[keys]` is described in [Change a key](#change-a-key).

The app writes other files in the settings folder: `session.txt` (open tabs and pane state), `recent.txt` (palette
history), `window.txt` (window position), `folders.txt` (each folder's view, sort and columns), `update-check.txt`
(the last update check), and `last-folder.txt` and `crash-count.txt` (crash recovery). To reset that state, close
rexplorer and delete the file.

## Themes and icon themes

`theme=` in a profile names a file in `%APPDATA%\rexplorer\themes\`. `light.ini` and `dark.ini` are written on
first run. You can edit them or add your own. A name with no file uses the dark theme. Each file has one `[theme]`
section. Colors are `#RRGGBB`. Restart rexplorer to see your edits.

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
| `border` | color | background mixed 8% toward the text | Kept for themes. Nothing draws border lines now. |
| `icon_theme` | folder name | none | A folder under `%APPDATA%\rexplorer\icons\`. Only a name: a path is ignored. |
| `font` | font name | `Segoe UI Variable Text` | Font. |
| `font_size` | number | `10` | Size in points. Limited to 6 to 72. |

The built-in light theme uses `#F5F5F7` for the background, `#FFFFFF` for the surface, `#1D1D1F` for the text,
`#6E6E73` for the secondary text and `#0A84FF` for the accent.

An icon theme is a folder `%APPDATA%\rexplorer\icons\<name>\` with PNG files named by extension (`rs.png`,
`pdf.png`), plus `folder.png` and `file.png` for everything else. A folder with neither `folder.png` nor `file.png`
is ignored, and the Windows icons stay. rexplorer draws the theme's PNGs at small-icon size, for example in the
Details view. The folder `icons\default\` is written on first run with a `folder.png` and a `file.png`. With
`icon_theme=default`, rexplorer also exports Windows' own icons there in the background the first time. It never
overwrites a PNG, so you can replace any of them. The export writes `system-icons.txt` when it is done. Delete that
file to export again, for example after you install a program.

When Windows is in a contrast theme, rexplorer uses the system colors instead of the theme file. This was checked
by eye under one Windows 11 contrast theme.

Windows' **Settings > Accessibility > Text size** (up to 225%) grows rexplorer's text and the bars and rows that
hold it, live, without a restart. Fixed widths (the sidebar, the panes, the search box) do not grow, so long text
there ends in "...". Folder options and Connect pick up a new size the next time they open. This was checked by
eye at 225% on Windows 11.

## Languages

rexplorer speaks English and Spanish. With `language=system` it follows Windows. To choose, set `language=en` or
`language=es` under `[general]` and restart rexplorer.

To add or change a language, put a UTF-8 file `%APPDATA%\rexplorer\lang\<code>.ini` with `key=text` lines (under
1 MiB) in place and set `language=<code>`. The key names are in the English file of the source,
`crates/core/src/lang/en.ini`. Keys you leave out stay in English. A file with a `[meta]` section holding
`direction=rtl` lays the window out right to left. No such translation ships yet, and that layout has not been
checked on screen.

## Command line

Run these in Win+R with the full path of the installed copy, for example:

```
%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --help
```

`--help` shows this list in a window:

```
rexplorer [options] [folder...]
  --select <path>     open the parent folder with <path> selected (also /select,<path>)
  --profile <name>    use the named settings profile
  --new-tab, -t       open in a new tab of the running window
  --search <text>     start with a search for <text>
  --register          make rexplorer the default file manager (folders, drives, Win+E), recorded so it can be undone
  --unregister        give folders back to Windows Explorer (undo --register)
  --install           copy rexplorer to %LOCALAPPDATA%\Programs as a normal program (Start menu, Settings > Apps); Explorer still opens folders
  --uninstall         give folders back to Explorer, then undo --install
  --quiet             with --install, --uninstall, --register or --unregister: no dialogs, a failure rolls back; the exit code is the result (0 done, 1 failed)
  --import-explorer   copy Windows Explorer's settings into the active profile (backs up settings.ini)
  --overwrite         with --import-explorer: also replace values you changed in rexplorer
  --recover           restore the previous session after a crash
  --desktop           run the helper that shows rexplorer's context menu on the desktop
  --desktop-icons     show the desktop's icons in a rexplorer window (opt-in, primary monitor)
  --new-window        open a separate window (does not hand off; saves no session)
  --elevated          when started elevated, stay so (drive-wide indexed search) instead of restarting unelevated
  -h, --help          show this help
```

`--install`, `--uninstall`, `--register` and `--unregister` ask first. These four exit with 0 when done, 1 when failed and 2 when you answer No. With `--quiet` nothing is asked, so the result is the
exit code alone. `--import-explorer` exits with 0 when done and 1 when it could not write the settings.

Folders can be paths, `shell:` names, `::{CLSID}` strings, UNC paths or .rar paths. A file path opens its folder with
the file selected. Explorer-style arguments work too: `/select,<path>`, `/e,<path>`, `/root,<path>`, `/n` and
`/separate`. A second launch passes its folders to the window that is already running and exits. An unknown option
that starts with `-` shows an error and this list. Other arguments rexplorer does not understand (URLs such as
`ms-settings:display`, an unknown `/` switch) are handed to `C:\Windows\explorer.exe`.

## Troubleshooting

**I want Explorer back right now.** See [Go back to Explorer](#go-back-to-explorer). If rexplorer does not start,
see [When rexplorer will not start](#when-rexplorer-will-not-start): Win+R, `C:\Windows\explorer.exe`, then run
`restore-explorer.ps1`.

**A folder seems to hang.** While a folder loads, its name shows in the tab and the title bar, and the status bar
says "Loading…". After about 120 ms the list shows placeholder rows that pulse. The window is not frozen. Big
folders, network drives and slow shell extensions can take a while. Wait, or press Back, or open another folder. F5
tries again. If one path always hangs (for example a disconnected network share), open it in Windows Explorer with
Ctrl+Shift+E.

**The whole window stops responding.** Windows adds "(Not Responding)" to the title and the window ignores clicks.
After 5 seconds rexplorer writes a line about it to `%APPDATA%\rexplorer\rexplorer.log`. rexplorer does not end
itself. Wait a little. If it does not come back, end rexplorer in Task Manager (Ctrl+Shift+Esc), then start it again.
To reach the folder at once, use Windows Explorer: Win+R, `C:\Windows\explorer.exe`. Add the lines from
`rexplorer.log` to a GitHub issue.

**Double-clicking a folder does nothing.** Try Enter. A folder opened from another program goes to the rexplorer
window that is already open. When that window is busy, the new launch waits for it, up to about 13 seconds, and then
opens a window of its own. If no window appears at all, try Ctrl+Shift+E to open the folder in Explorer. If this
keeps happening, give folders back to Explorer (see [Go back to Explorer](#go-back-to-explorer)).

**Dragging files to some program does not work.** Drags into other programs and Explorer were proven, but not yet
into browsers, WhatsApp or VS Code ([#75](https://github.com/CodingDiego/rexplorer/issues/75)). A drag that did
nothing leaves a line in `%APPDATA%\rexplorer\rexplorer.log`. Add it to the issue. Copy and paste (Ctrl+C, Ctrl+V)
works.

**F2 or click-again rename does not work.** Both were proven with a real keyboard and mouse (see
[EXPLORER-PARITY.md](EXPLORER-PARITY.md)). If it fails for you, open an issue and say where. Right-click, Rename is
the fallback. Inside a .rar, rename is not possible (archives are read-only) and F2 says so.

**Undo beeps and does nothing.** Undo only touches a file it can prove it made or moved. If the file was changed,
replaced or re-created since, it is left alone on purpose.

**Search is slow or misses files.** Searches of non-indexed folders walk the disk. Turn off
**Include system folders** or **Include compressed files (ZIP, CAB...)** in More > Folder options... > Search. A
search of a whole drive started from rexplorer with `--elevated` uses the drive's index instead.

**rexplorer starts unelevated even though I ran it as administrator.** On purpose: Explorer never shows an
elevated folder window. To stay elevated, open Command Prompt with **Run as administrator** and run:

```
"%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe" --elevated
```

**My settings are broken.** Close rexplorer and edit or delete `%APPDATA%\rexplorer\settings.ini`. A fresh one is
written with defaults at the next start. Unreadable values are ignored anyway. Backups made by `--import-explorer`
are the `settings.ini.bak-<date>` files in the same folder.

**It crashed.** It restarts itself and restores your tabs (`--recover`), except a tab that was still loading, which
opens Home. The second crash within 60 seconds opens
the folder in Windows Explorer instead. After a crash, rexplorer offers to open the crash reports folder. Two things
are kept on your disk, and nothing is uploaded:

- `%APPDATA%\rexplorer\rexplorer.log` records hangs of the window, panics and failed drags. It starts again once it
  passes 64 KB.
- `%LOCALAPPDATA%\rexplorer\crashes` holds a crash dump (`.dmp`) for each crash and a short `.txt` report beside
  it. For a panic the report holds the message and stack, written at once. For any other crash it holds the
  exception, the address and the module, written when rexplorer next starts (usually right away, by the automatic
  restart). The newest 10 dumps are kept, each with its report. One empty `.seh` file in that folder while
  rexplorer runs is normal. If `%LOCALAPPDATA%` is not set, the temp folder is used instead.

To report a crash, attach what you are comfortable sharing from these files to a GitHub issue, or send a private
report as `SECURITY.md` explains. Read the `.txt` first. The `.dmp` can hold names of files you had open. Say what
you did before the crash and which version you ran. **More > About rexplorer** shows the version.
