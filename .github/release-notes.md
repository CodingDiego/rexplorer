A safer installer: it warns before it changes anything, and a failed install can be rolled back.

**Install.** Download the zip, extract it, and run `rexplorer.exe` to try it. `rexplorer.exe --install` makes it
the default file manager for your user; it needs no administrator rights. Already installed? Run the new
`rexplorer.exe --install` to replace the old copy. Undo with
`%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.

**Changes.**
- `--install` now always asks first. The warning says it replaces Windows Explorer for folders, drives, Win+E and
  the taskbar's File Explorer, and that a running copy of rexplorer is replaced (its open windows keep working
  until you close them). No is the default. Answering No exits with code 2 and changes nothing;
  `scripts\install.ps1` reports it as cancelled.
- If the install fails partway, a dialog offers **Roll back** (the default) or **Keep changes**. Roll back
  restores everything as it was before: registry values, the previous rexplorer.exe, install.txt and the Start
  menu shortcut. Keep changes leaves the partial install for `--uninstall` to undo later.
- An update no longer overwrites the previous exe in place: it is kept aside until the install succeeds, so a
  failed update can go back to it.

The binary is built by GitHub Actions from the tagged commit; `SHA256SUMS.txt` lists the checksums. It is not
code-signed, so SmartScreen may warn on first run.
