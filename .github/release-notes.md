rexplorer 1.0.0-rc.2 is the second release candidate for 1.0, published as a pre-release: the update notice and winget stay on 0.2.0. The changes are listed in full in `CHANGELOG.md` under 1.0.0-rc.2 and 1.0.0-rc.1.

**Since 1.0.0-rc.1.**
- Drops onto the folder tree can be undone and redone, like drops on the file list.
- Undo checks a file's identity again when the shell is about to act on it, and cancels the undo if the file was
  swapped.
- Results from background work (folder loads, search, the sidebar, the preview pane and others) no longer travel
  as pointers in window messages, so another program can no longer crash rexplorer by forging one of them.
- Two known gaps listed for 1.0.0-rc.1 were not bugs: clicking a selected name again does select the name for
  renaming, and Ctrl+C in Windows Explorer does replace a clipboard rexplorer set.

**If you use 0.2.0, update.** 0.2.0 can rename the wrong file. In a window with Back history, each character typed
in the rename box sends the window Back, and Enter then renames whatever item sits at the same position in the
folder now shown. This release candidate fixes it: typing no longer navigates, and a rename always applies to the
item it was opened on (22 of 22 live renames after the fix).

**Download.** Two zips: `rexplorer-<version>-x64.zip` and `rexplorer-<version>-arm64.zip`. The arm64 build is
cross-compiled and has not been tested on ARM hardware yet. `SHA256SUMS.txt` lists both zips and both exes, the exes
with their folder (`rexplorer-<version>-x64/rexplorer.exe`); check a download with `Get-FileHash` against it.

**Install.** Unchanged since 0.2.0: `rexplorer.exe --install` installs the program for your user (Explorer still
opens folders), `--register` makes it the default file manager, `--unregister` gives folders back, `--uninstall`
removes it. The zip's `install.ps1`, `master-install.ps1` and `restore-explorer.ps1` do the same from PowerShell. To
upgrade, run the new `rexplorer.exe --install`.

**New since 0.2.0.**
- Backspace goes Back, as in Windows 11 Explorer (it went up before; Alt+Up still does).
- Long folder names in the address bar are cut with "..." and show their full name in a tooltip; the current folder
  keeps the most room.
- The folder tree sits next to the sidebar instead of replacing it. Ctrl+B shows or hides only the sidebar; More >
  Folder tree shows or hides the tree.
- F4 opens the address bar with your recent places listed under it. Ctrl+L and Alt+D open it without the list. Esc
  closes the suggestions first, then the bar.
- Undo (Ctrl+Z) and redo (Ctrl+Y) also cover "Keep both" copies, pastes where you chose Skip, drops on Home, items
  made with the context menu's New submenu, paste and drop in `--desktop-icons`, and Delete in the desktop menu
  helper. Undo only touches a file it can prove it made or moved.
- Alt or F10 alone focuses the command bar. Holding Back or Forward opens the history menu. Home's Recent,
  Favorites and Shared pills work from the keyboard. Tab and F6 leave the preview pane.
- Drop a tab on another rexplorer window to move it there.
- Renaming inside a .rar says why it cannot be done. Multi-volume .rar archives open from their first part.
- New > Shortcut selects the new shortcut without opening a rename, because the wizard names it.
- Native crashes also get a short text report beside the minidump. Right-to-left layout for languages that declare
  it (no right-to-left translation ships yet).

**Fixes.**
- The rename data-loss bug above.
- Libraries and .zip folders showed rows without names, library rows did not open, and Ctrl+C on a zip member did
  nothing. Opening Libraries in an icon view could crash rexplorer.
- The address text you were typing was replaced whenever the folder refreshed.
- A rename box stayed open after its file was deleted elsewhere. The Recycle Bin's Name column showed full paths,
  and its items got packaged-app menu entries. A click on a sidebar row left the keyboard in the file list.
- Every undo of a delete left an orphan record in the Recycle Bin; a drop that did nothing could be undone; search
  results lost their sort on F5; the folder tree did not follow the current folder.
- New from the context menu on the top-level Desktop could open the rename on another item. A library's own rows
  could still show without names. Opening a Quick Access folder from the folder tree added two Back steps. A trimmed
  crumb's tooltip could show the previous folder's name.
- Undo no longer sends a moved folder to the Recycle Bin when something re-created its old path. A copy is undone
  only while its source is unchanged, and a copied empty folder is never undone.
- The update notice understands pre-release versions, so this candidate will announce 1.0.0.

**Known gaps.** The full checklist is `docs/EXPLORER-PARITY.md`: 123 behaviours, 103 proven on the exe (WORKS) and 3
more on a scratch install base, 13 built but not proven (UNVERIFIED), 4 PARTIAL.
- Ctrl+Z after a cancelled folder copy leaves the partial folder (undo never deletes what it cannot prove it
  made). After Delete in search results no row is selected. No sidebar row opens This PC itself.
- Tearing out a Home tab once landed in the other rexplorer window (1 of 2 tries, not reproduced).
- When undo refuses a swapped file it also cancels the files after it in the same undo.
- Not run for this candidate: the 15-minute soak, the fuzz targets, and multi-volume .rar (no test archive; two
  tests are skipped).
- Not yet tried by a person: Narrator or NVDA, a Windows contrast theme, Text size above 100% and other display
  scales (including the address bar trimming), ARM64 hardware, Windows 10, a clean-machine install, MTP phones,
  OneDrive, WhatsApp as a drop target, and Libraries in an icon view before their thumbnails are cached.
- Undo does not restore a file overwritten by Replace, does not undo a merged folder, and gives no undo for New,
  rename or shortcuts on a share that reports no file ids.
- The accessibility request of rexplorer's own controls and their notifications still carry a pointer, so another
  program in your session can crash rexplorer with a forged one. See `SECURITY.md`.
- Run as administrator was never invoked in testing. FTP is deferred (#36). The exe is not code-signed, so
  SmartScreen may warn (#59). The winget submission is pending (#63). There is no app icon yet (#18).

**Reporting problems.** Open an issue at https://github.com/CodingDiego/rexplorer/issues with the steps, what you
expected and what happened, your Windows version, and `rexplorer.log` from `%APPDATA%\rexplorer` if it has lines
from that time (it holds no file paths). After a crash, `%LOCALAPPDATA%\rexplorer\crashes` holds a `.dmp` and a `.txt`; attach them only
if you are comfortable sharing them (`SECURITY.md` says what they contain). Report security problems privately, as
`SECURITY.md` describes.

**Roadmap.** `VERSION-1-X.md` lists what a stable 1.0 needs; each item has an issue in the v1.0 milestone (#71).

The binary is built by GitHub Actions from the tagged commit, twice, and the job fails if the two builds differ.
