Draft for the next release (the v1 release-candidate work). Not tagged yet; the 0.1.4 notes are in `CHANGELOG.md`.

**Download.** Two zips: `rexplorer-<version>-x64.zip` and `rexplorer-<version>-arm64.zip`. The arm64 build is
cross-compiled and has not been tested on ARM hardware yet. `SHA256SUMS.txt` lists both zips and both exes, the exes
with their folder (`rexplorer-<version>-x64/rexplorer.exe`); check a download with `Get-FileHash` against it.

**Install.** Extract the zip and run `rexplorer.exe` to try it. `rexplorer.exe --install` makes it the default file
manager for your user, with no administrator rights; `--install --quiet` does it without a dialog. Already
installed? Run the new `rexplorer.exe --install` to replace the old copy. Undo with
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
- Archives (.rar): member icons in every view, picture thumbnails, and a preview of pictures and text.
- Undo now also covers drops on the sidebar, crumbs, tabs and Home, the context menu's New submenu, and Delete and
  rename in `--desktop-icons`. A new item from the context menu starts its rename at once.
- An opt-in update check (Folder options > General, off by default): at most once a day, no telemetry, nothing
  downloaded.
- Crash reports stay on your PC in `%LOCALAPPDATA%\rexplorer\crashes` (a minidump, and a text report for a panic).
  A UI thread blocked for 5 s is logged to `%APPDATA%\rexplorer\rexplorer.log`.
- rexplorer's own icon and a version resource that names rexplorer only.
- Paths over 260 characters, where Windows' LongPathsEnabled policy is on.

**Fixes.**
- Extract here / Extract all to never replace a file already in the target, and never write outside it.
- A drag the target refused or you cancelled no longer shows an error box.
- A second launch of another version no longer loses the folder it was asked to open: the hand-off is versioned.
- A failed COM start no longer crashes the app.
- Uninstall also removes `rexplorer.old` and the empty folders install created.

**Settings format.** Settings files now carry `format=1`. The first start adds that one line to `settings.ini` and
each theme and changes nothing else; unknown keys are kept. Files from 0.1.x load as they are.

**Known gaps.** Copied from the Known gaps in the README at release time; the full checklist is
`docs/EXPLORER-PARITY.md`.
- Dragging files to other programs is unreliable (#75). Each failed drag is now logged to find the cause.
- Most mouse- and keyboard-driven features added since 0.1.4, and everything in this release, are UNVERIFIED: built
  and unit-tested, not tried with a real mouse, keyboard or screen reader.
- The exe is not code-signed, so SmartScreen may warn.
- A few internal window messages carry a pointer; another program at the same integrity level can crash
  rexplorer with a bad value. This is a robustness gap, not a privilege boundary.
- The address and search boxes have no accessible name. The breadcrumb's hidden crumbs are not keyboard stops.
- Search results have no thumbnails or preview.
- Archive members cannot be dragged, sorted by column or renamed; encrypted .rar has no password prompt,
  multi-volume .rar is untested, and only .rar gets the archive view.
- Undo does not cover pastes staged in %TEMP%, Keep-both copies, or paste and drop on `--desktop-icons`.
- Fixed widths (sidebar, panes, search field) do not grow with Text size.
- Not part of 1.0: merging a tab into another window, right-to-left layout, and FTP is present but unverified.

**Roadmap.** `VERSION-1-X.md` lists what a stable 1.0 needs; each item has an issue in the v1.0 milestone (#71).

The binary is built by GitHub Actions from the tagged commit, twice, and the job fails if the two builds differ.
It is not code-signed, so SmartScreen may warn on first run.
