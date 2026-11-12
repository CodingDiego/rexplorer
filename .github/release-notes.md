rexplorer 0.2.0, the v1 release candidate. The changes since 0.1.4 are listed in full in `CHANGELOG.md`.

**Download.** Two zips: `rexplorer-<version>-x64.zip` and `rexplorer-<version>-arm64.zip`. The arm64 build is
cross-compiled and has not been tested on ARM hardware yet. `SHA256SUMS.txt` lists both zips and both exes, the exes
with their folder (`rexplorer-<version>-x64/rexplorer.exe`); check a download with `Get-FileHash` against it.

**Install (changed).** Installing and making rexplorer the default file manager are now two separate steps, so an
install can no longer take over your folders by itself. Extract the zip and run `rexplorer.exe` to try it.
`rexplorer.exe --install` installs it as a normal program for your user (Start menu, Settings > Apps, no
administrator rights); Windows Explorer still opens folders. `rexplorer.exe --register` (or More > Make rexplorer the
default file manager) then makes it the default, journaled so `--unregister` (More > Give folders back to Windows
Explorer) and uninstall undo it exactly. The scripts `install.ps1` (program only) and `master-install.ps1` (program,
then the default, after saving the keys it changes to a `.reg` backup) do the same from PowerShell, and
`restore-explorer.ps1` puts Windows Explorer back without needing a working rexplorer.exe. Already installed by an
earlier build? Run the new `rexplorer.exe --install` to replace the copy; folders keep opening in rexplorer until you
give them back, and uninstall still reverts everything the old install did. Uninstall from Settings > Apps or with
`%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.

**Added.**
- Keyboard: F6 and Tab now reach the navigation toolbar, the tools bar and the command bar; arrows, Enter, Space,
  Down and Esc work inside them. Left on the first crumb opens the breadcrumb's "..." menu.
- Screen readers: the preview and details pane has an accessible name and reads its lines; the desktop icons
  window is named "Desktop".
- Text size: Windows' Accessibility > Text size (up to 225%) grows rexplorer's text and the bars and rows around
  it, live.
- Search: unelevated searches use the Windows Search index where it covers the folder, and walk what it leaves
  out. The context menu works on results from several folders, and Back returns to the results.
- Home: a Status column (cloud state) and sorting by column.
- Archives (.rar): member icons in every view, picture thumbnails, and a preview of pictures and text; a password
  prompt for encrypted archives; members can be dragged out to other programs.
- Undo now also covers drops on the sidebar, crumbs, tabs and Home, the context menu's New submenu, and Delete and
  rename in `--desktop-icons`. A new item from the context menu starts its rename at once.
- An opt-in update check (Folder options > General, off by default): at most once a day, no telemetry, nothing
  downloaded.
- Crash reports stay on your PC in `%LOCALAPPDATA%\rexplorer\crashes` (a minidump, and a text report for a panic).
  A UI thread blocked for 5 s is logged to `%APPDATA%\rexplorer\rexplorer.log`.
- rexplorer's own icon and a version resource that names rexplorer only.
- Paths over 260 characters, where Windows' LongPathsEnabled policy is on.
- Taskbar: rexplorer's windows group on one button with a jump list (Frequent and Recent folders, New window),
  installed or not.
- Paste shortcut in the background menu, and Run as administrator (Ctrl+Shift+Enter) on programs.
- Breadcrumb dropdowns on every crumb, This PC, Network and shares included, listed in the background; the folder
  icon on the left lists the desktop's places, Control Panel and Libraries among them.
- Marks for cloud files that are online-only or always kept on this device.

**Fixes.**
- Extract here / Extract all to never replace a file already in the target, and never write outside it.
- A drag the target refused or you cancelled no longer shows an error box.
- A second launch of another version no longer loses the folder it was asked to open: the hand-off is versioned.
- A failed COM start no longer crashes the app.
- Uninstall also removes `rexplorer.old` and the empty folders install created.

**Settings format.** Settings files now carry `format=1`. The first start adds that one line to `settings.ini` and
each theme and changes nothing else; unknown keys are kept. Files from 0.1.x load as they are.

**Known gaps.** From the Known gaps in the README; the full checklist is `docs/EXPLORER-PARITY.md`, where 61
behaviours were proven on this build (WORKS) and 3 more on a scratch install base, 44 are built and unit-tested but not yet tried on the exe
(UNVERIFIED) and 10 are PARTIAL.
- Not yet tried by a person: Narrator or NVDA, a Windows contrast theme, Text size above 100% and display scales
  other than this machine's, two monitors, ARM64 hardware, Windows 10, and the taskbar jump list on the taskbar
  itself (it was checked in its file).
- Cloud marks were never seen on real OneDrive placeholders, and show only in Details and List. Run as
  administrator was never invoked in testing.
- MTP phones open in an embedded Explorer view; rename in search results is unverified.
- The exe is not code-signed, so SmartScreen may warn.
- A few internal window messages carry a pointer; another program at the same integrity level can crash
  rexplorer with a bad value. This is a robustness gap, not a privilege boundary.
- The breadcrumb's hidden crumbs are not keyboard stops.
- Sorting, renaming and deleting in search results are untested.
- Archive members cannot be sorted by column or renamed; an encrypted .rar's password is passed in the ANSI code
  page, multi-volume .rar is untested, and only .rar gets the archive view.
- Dragging files into browsers, WhatsApp and VS Code was not tried (#75); drags into other programs work.
- Undo does not cover pastes staged in %TEMP%, Keep-both copies, or paste and drop on `--desktop-icons`.
- Fixed widths (sidebar, panes, search field) do not grow with Text size.
- Not part of 1.0: merging a tab into another window, right-to-left layout, and FTP is present but unverified.

**Roadmap.** `VERSION-1-X.md` lists what a stable 1.0 needs; each item has an issue in the v1.0 milestone (#71).

The binary is built by GitHub Actions from the tagged commit, twice, and the job fails if the two builds differ.
It is not code-signed, so SmartScreen may warn on first run.
