Draft: read these notes, then delete this line and the empty line under it. The release job refuses to publish while it is here.

rexplorer 1.0.0 is the first stable release of the 1.x line. It replaces Windows Explorer's folder windows; the desktop, the taskbar and Start stay Explorer's. The changes are listed in full in `CHANGELOG.md` under 1.0.0, 1.0.0-rc.2 and 1.0.0-rc.1.

**If you use 0.2.0, update.** 0.2.0 can rename the wrong file. In a window with Back history, each character typed
in the rename box sends the window Back, and Enter then renames whatever item sits at the same position in the
folder now shown. This is fixed since 1.0.0-rc.1: typing no longer navigates, and a rename always applies to the
item it was opened on (22 of 22 live renames after the fix).

**Download.** Two zips: `rexplorer-<version>-x64.zip` and `rexplorer-<version>-arm64.zip`. The arm64 build is
cross-compiled and has not been tested on ARM hardware yet. `SHA256SUMS.txt` lists both zips and both exes, the exes
with their folder (`rexplorer-<version>-x64/rexplorer.exe`); check a download with `Get-FileHash` against it. The
published files also carry a build provenance attestation.

**Install.** `rexplorer.exe --install` installs the program for your user (Explorer still opens folders),
`--register` makes it the default file manager, `--unregister` gives folders back, `--uninstall` removes it and now
asks first. The zip's `install.ps1`, `master-install.ps1` and `restore-explorer.ps1` do the same from PowerShell. To
upgrade, run the new `rexplorer.exe --install`. `docs/USER-GUIDE.md` walks through each step.

**New since 1.0.0-rc.2.**
- rexplorer has an icon.
- The sidebar's Favorites lists your pinned folders first, each with a pin, then the folders you use most, as
  Explorer does. Right-click a pinned folder to unpin it, or a frequent one to pin it or remove it.
- The sidebar has a This PC row. After you delete the selection, the row that takes its place is selected.
- `ftp://` folders list in rexplorer's own view, with their subfolders and Up (browsing only).
- The folder tree follows Explorer's "Expand to open folder" option.
- For screen readers: Folder options' check boxes and radio buttons say what they are and whether they are on, the
  menu buttons say they are collapsed menus, and the palette's search box has a name.

**Fixes.**
- With rexplorer as the file manager, Win+E opened Windows Explorer on some Windows 11 builds (seen on 26300). The
  desktop helper, which `--register` starts at sign-in, now takes the key.
- A multi-volume `.rar` could crash rexplorer while it was listed or extracted.
- Folder options, Connect and the archive password prompt stay inside the screen at every Windows text size and
  display scale, and scroll when their content is larger. The list's small icons and a shortcut's arrow follow the
  display scale when it changes while rexplorer runs.
- After a crash, the automatic restart opens Home in place of a tab that was still loading, so a folder that
  crashes rexplorer cannot do so again at every start. Signing out saves the open tabs and the window's place.
- A thumbnail is drawn again when its picture changes on disk. Closing the window during an extraction lets the
  extraction finish.
- A tab torn out and released just beside another rexplorer window opens a window of its own.
- The taskbar jump list opens a drive root (`C:\`) again.
- `--desktop-icons` showed folders the desktop never shows (Downloads, Documents, your own folder).

**Security.** An audit of the whole program came before this release; `SECURITY.md` lists every finding. The main
ones, all fixed here:
- A `.rar` member named like a network path could replace a file in the destination folder.
- Files extracted before a failure, or from an archive on an Internet-zone share, missed the Mark-of-the-Web.
- An `ftp://` password showed in the palette, in titles and in the name handed to Explorer.
- "Open in Terminal" on a folder with `;` in its name ran a second command.
- An uninstall record could remove folders that were not rexplorer's.
- A program that starts `explorer.exe /select,<path>` without quotes could be made to pass `--uninstall --quiet` by
  a file named that way.
- Another program in your session could crash rexplorer with a forged window message. rexplorer's own messages are
  now accepted only when rexplorer sent them, and none carries a pointer.
- Every GitHub Action the release uses is pinned to a commit, and the job that builds the files cannot write to the
  repository.

**Known gaps.** The full checklist is `docs/EXPLORER-PARITY.md`: 123 behaviours, 109 proven on the exe (WORKS) and 3
more on a scratch install base, 5 built but not proven (UNVERIFIED), 6 PARTIAL.
- Ctrl+Z after a cancelled folder copy leaves the partial folder (undo never deletes what it cannot prove it made).
- Undo does not restore a file overwritten by Replace, does not undo a merged folder, and gives no undo for New,
  rename or shortcuts on a share that reports no file ids.
- A drag out to another program once went undelivered (1 of 4 tries) and was not reproduced. A drag that does
  nothing leaves a line in `rexplorer.log`.
- With rexplorer as the file manager, Win+E through the desktop helper was proven on a test profile, not yet on a
  registered install of this build.
- `ftp://` is for browsing: copying to or from an FTP folder in rexplorer's window was not proven.
- The desktop icons view (`--desktop-icons`, opt-in) shows the desktop and follows it; its menu, rename, delete and
  drops were never run. Run as administrator and Empty Recycle Bin were never invoked in testing.
- Not yet tried by a person: Narrator or NVDA, ARM64 hardware, Windows 10, an install on a clean machine, MTP
  phones, OneDrive, and a move between two monitors of different scale.
- The exe is not code-signed, so SmartScreen may warn (#59). The winget submission is pending (#63).

**Reporting problems.** Open an issue at https://github.com/CodingDiego/rexplorer/issues with the steps, what you
expected and what happened, your Windows version, and `rexplorer.log` from `%APPDATA%\rexplorer` if it has lines
from that time (it holds no file paths). After a crash, `%LOCALAPPDATA%\rexplorer\crashes` holds a `.dmp` and a `.txt`; attach them only
if you are comfortable sharing them (`SECURITY.md` says what they contain). Report security problems privately, as
`SECURITY.md` describes.

**Roadmap.** `VERSION-1-X.md` lists what the 1.x line still needs; each item has an issue (#71).

The binary is built by GitHub Actions from the tagged commit, twice, and the job fails if the two builds differ.
