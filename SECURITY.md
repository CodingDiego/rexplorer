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

## Known limitations (1.0.0-rc.1)

- **Forgeable window messages.** Several of rexplorer's internal window messages still carry a memory pointer in
  their parameters (two were changed in this release to carry none). Another program running in the same session,
  as the same user and at the same or a higher integrity level, can post such a message with a bad pointer and
  crash rexplorer. Windows blocks these messages from lower-integrity programs. The impact is a crash (lost tabs
  until the automatic recovery start), not code running with more rights: a program that can send the message can
  already read and change everything the user can.
- **Undo checks a file's identity once.** Before an undo moves, renames or recycles something, rexplorer checks that
  the item is still the one it recorded (its file id, or for a paste or drop a free name of the shell's form with the
  source's size and time). It does not check again while the shell's confirmation dialog is open. If another
  program swaps the file in that window, undo can act on the swapped file. The shell's own operations still apply
  (a delete goes to the Recycle Bin, where it can be restored), and only files the user can already change are
  reachable.

## Sending a crash report

Nothing is uploaded automatically, ever. A crash leaves its files in `%LOCALAPPDATA%\rexplorer\crashes` (the temp
folder if that variable is not set):

- `rexplorer-<time>-<pid>.dmp`, a minidump, for every crash.
- `rexplorer-<time>-<pid>.txt`, a short text report. A panic writes it at once, with the message and a backtrace. A
  native crash (an access violation, for example) writes a small `.seh` record at the moment of the crash, and the
  next start, usually the automatic `--recover` one, turns it into the `.txt`: the version, the exception code and
  its name, the address, the module and offset when known, the thread and the UTC time.

The newest 10 dumps are kept; each one's `.txt` goes with it. While rexplorer runs, one empty `.seh` file sits in
that folder; that is normal and it is removed at the next start. After a crash the recovered window offers to open
the folder.

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
- **2026-11-13** (the v1 remaining-work round, not yet committed: undo of pastes and drops, multi-volume .rar,
  native crash reports, tab hand-off between windows, right-to-left layout, fuzzing): its security review runs before
  the round is committed, and this entry will name its findings. The desktop helper's low-level mouse and keyboard
  hooks (`--desktop`) get an explicit pass then, because no record above names them. Hardening already in the round:
  undo of a copied folder checks the folder's file id, the tab hand-off message carries no pointer, the Home drag
  image is not built inside a cross-thread call, and a crash record of the wrong size is deleted unread. The two
  limitations above remain.
