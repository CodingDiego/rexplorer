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
- The installer and uninstaller (`--install`, `--uninstall`, `scripts/install.ps1`) changing or removing
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
- Dependencies are checked with `cargo audit` in CI.
