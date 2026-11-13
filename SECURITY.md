# Security policy

rexplorer replaces Windows Explorer's folder windows, so it opens whatever is on your disks, USB sticks, archives
and network shares. Bugs that let such content run code, write outside where it should, leak credentials or
crash the window are security issues, and reports are welcome.

## Supported versions

Only the latest commit on `master` is supported. There are no release branches yet; fixes land on `master`.

## Reporting a vulnerability

Please report privately, not in a public issue:

- Use GitHub's **Report a vulnerability** button on the repository's
  [Security tab](https://github.com/CodingDiego/rexplorer/security/advisories/new).

Include the Windows version, the rexplorer commit, the steps or a sample file that triggers it, and what happens.
A minimal crafted file (archive, shortcut, folder name) is the most useful thing you can attach.

You can expect an acknowledgement within 7 days and a fix or a plan within 30 days for confirmed issues. Credit
goes in the fix's commit message and the advisory unless you prefer otherwise.

## Scope

In scope:

- Content-driven issues: archives, file and folder names, shortcuts, previews, thumbnails, network locations.
- The installer and uninstaller (`--install`, `--uninstall`, `--register`, `--unregister`, `scripts/install.ps1`,
  `scripts/master-install.ps1`, `scripts/restore-explorer.ps1`) changing or removing
  registry keys or files they did not create.
- Credentials (for example FTP passwords) shown on screen or written to disk.
- Anything that makes rexplorer run with more rights than the user who started it.

Out of scope:

- Changes made by code already running as the same user (it can edit HKCU and `%APPDATA%` directly anyway).
- Vulnerabilities in Windows itself, shell extensions or preview handlers from other vendors (report those to
  their vendors), unless rexplorer makes them reachable where Windows Explorer would not.
- RAR library bugs: report them to [RARLAB](https://www.rarlab.com/); tell us too so the bundled version can be
  updated.

## How rexplorer limits exposure

- Archive entries that would extract outside the destination (`..`) are skipped; extracted files keep the
  archive's Mark-of-the-Web.
- Preview handlers run out of process (`prevhost.exe`) and are not used for files from the Internet zone.
- An elevated start restarts unelevated unless `--elevated` is passed.
- Everything rexplorer installs is per-user (HKCU, `%LOCALAPPDATA%`); nothing needs administrator rights.
- Dependencies are checked with `cargo audit` in CI every week; failures are triaged within 7 days (see
  CONTRIBUTING.md, Dependencies).
- No telemetry: crash reports stay on your PC (next section).
- The update check is off by default (Folder options > General). When on, it makes at most one HTTPS GET a day to
  `api.github.com` for the latest release, follows no redirects, reads at most 1 MiB, downloads nothing and sends
  nothing beyond the request itself.

## Known limitations (after the 2026-11-13 review)

- **The list view's own notifications.** No message rexplorer's own code sends or posts carries a pointer or a
  handle that it trusts from the sender any more: worker results (thumbnails and Home icons included) wait in the
  window's own queue and their message carries nothing, and the accessibility request (`WM_ACC_ITEMS`) and the custom
  controls' notifications (`WM_NOTIFY` with rexplorer's own structures) are read only when rexplorer itself sent them
  on that thread. Two messages from the system are read through their pointer. The shell's drag image request
  (`ShellGetDragImage`) is answered by Home once per drag, only from Home's own thread and only during the call
  that starts a drag from Home; at any other time, or a second time, it is ignored. And what every Win32 program
  has: the notifications the system's list view sends the main window (`WM_NOTIFY` from comctl32) are read through
  their pointer, so another program running in the same
  session, as the same user and at the same or a higher integrity level, can crash rexplorer with a forged one.
  Windows blocks such messages from lower-integrity programs, and a program at the same level could already inject
  code: a robustness gap, not a privilege boundary.
- **Undo's identity check leaves a short window.** Before an undo moves, renames or recycles something, rexplorer
  checks that the item is still the one it recorded (its file id, or for a paste or drop a free name of the shell's
  form with the source's size and time), and checks the file id again when the shell is about to act on each item
  (the shell's Pre callbacks); a move or rename back also requires the old name still free there, so the shell's
  Replace or Skip dialog, during which a swap would go unseen, is never reached. What remains is the moment between
  the shell's Pre callback and its own move, rename or delete, and the Recycle Bin confirmation: with "Display delete
  confirmation" turned on, whether the shell shows it before or after its Pre callback was not checked (the test
  machine has it off, and turning it on is a Windows setting the tests must not change). The shell's own operations
  still apply (a delete goes to the Recycle Bin, where it can be restored), and only files the user can already
  change are reachable.
- **The desktop helper trusts the shell's process.** It acts only on a desktop list owned by the process of the
  shell's desktop window (`GetShellWindow`). A program that can run code inside explorer.exe is beyond this check,
  as it is beyond any other.

## Sending a crash report

Nothing is uploaded automatically, ever. A crash leaves its files in `%LOCALAPPDATA%\rexplorer\crashes` (the temp
folder if that variable is not set):

- `rexplorer-<time>-<pid>.dmp`, a minidump, for every crash.
- `rexplorer-<time>-<pid>.txt`, a short text report. A panic writes it at once, with the message and a backtrace. A
  native crash (an access violation, for example) writes a small `.seh` record at the moment of the crash, and the
  next start, usually the automatic `--recover` one, turns it into the `.txt`: the version, the exception code and
  its name, the address, the module and offset when known, the thread and the UTC time.

The newest 10 dumps are kept; each one's `.txt` goes with it. While rexplorer runs, one empty `.seh` file sits in
that folder; that is normal, a clean exit removes it, and one left by a run that was killed is removed at the next
start. A crash in the last moments of exit, after that file is removed, leaves its minidump without the text record.
After a crash the recovered window offers to open the folder.

To report a crash, attach the `.txt` and the matching `.dmp` to a GitHub issue, or, if you would rather not make
them public, to a private advisory (the **Report a vulnerability** button above). Say what you were doing and which
version you ran. A dump holds part of the process memory, which can include names and paths of files you had open:
look before you share it, and send only the `.txt` if you prefer.

## Security reviews of the 1.0 candidate line

- **2026-10-10:** the whole project was reviewed; every finding was fixed.
- **2026-11-10** (pull request #81, Explorer parity, accessibility, Spanish, the desktop icons view): a security
  review of the whole diff in each round of work; the findings were fixed in that pull request.
- **2026-11-10 to 2026-11-12** (pull request #82, released as 0.2.0): four security review passes over the whole
  diff (it holds the new install model and take-over journal, the restore script, archive passwords and drag-out and
  the update check). Every finding of medium severity or higher was fixed before the merge: archive overwrite through
  renamed entries, archive link members (symbolic links and junctions), the smoke test's drag guard, and the order of
  the crash-recovery steps. Also fixed then: names in the long-path rename, and the jump list under the test install
  overrides.
- **2026-11-13** (the v1 remaining-work round, released as 1.0.0-rc.1 and rc.2: undo of pastes and drops,
  multi-volume .rar, native crash reports, tab hand-off between windows, right-to-left layout, fuzzing). Hardening
  in the round: undo of a copied folder checks the folder's file id, the tab hand-off message carries no pointer, the
  Home drag image is not built inside a cross-thread call, a crash record of the wrong size is deleted unread, worker
  results carry no pointer, and undo checks each item's file id again when the shell acts on it. A review at the
  rc.2 tag then gave the desktop helper's low-level hooks (`--desktop`) and the install their explicit pass
  ([#60](https://github.com/CodingDiego/rexplorer/issues/60)) and found, all fixed after rc.2:
  - medium: the helper decided "this is the desktop" from window class names alone, so a program's look-alike list
    could make it build the menu for, and act on, the real desktop's icons at the look-alike's indices; and its
    selection reads had no timeout, so a list that never answered froze the helper while its hooks kept taking every
    desktop right-click. It now requires the list to belong to the shell's process and reads its selection first,
    with a one-second timeout (no menu on a timeout), and drops a click it could not get to within 2 seconds. The
    calls into Explorer after that first answer (finding the desktop's view, building and running the menu) still
    have no timeout, so an Explorer that hangs between the read and those calls can still hold the helper.
  - low: the helper's Delete compared only the number of selected icons with its menu's; `--register` registered the
    exe it ran from (from a folder other accounts can write to, one of them could replace the exe that then runs at
    sign-in); a right-drag released while the press was being handed back could leave the right button down; a menu
    key whose release the helper missed was swallowed once in any window; uninstall deleted any file, and any
    registry value or empty key, its records named; `install.ps1` in a release folder fell back to building; `restore-explorer.ps1 -RestartShell` started
    `explorer.exe` by bare name (App Paths may still name rexplorer); the jump list quoted a drive root as `C:"`.
  - a further read of the search syntax, settings and session files found one more: a tab name with a line break
    (from an FTP server) could add lines to the session file, such as a `tab=` that opens a network path at the next
    start. Fixed.
  - the leftovers of [#84](https://github.com/CodingDiego/rexplorer/issues/84): the accessibility request, the custom
    controls' notifications and the bitmap results trusted a pointer or handle from the sender (now ours only, or
    queued); with a conflict dialog on screen the undo re-check missed a swap made while the dialog waited (a test
    showed it; undo now never reaches that dialog); items the shell cancelled after a refused one lost their undo
    entry; the thumbnail worker's apartment did not pump between requests.
  The limitations above are what remains.
