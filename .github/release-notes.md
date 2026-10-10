rexplorer's context menu on the desktop: right-click the desktop and get the same menu as in a rexplorer folder.

**Install.** Download the zip, extract it, and run `rexplorer.exe` to try it. `rexplorer.exe --install` makes it
the default file manager for your user; it needs no administrator rights. Already installed? Run the new
`rexplorer.exe --install` to replace the old copy. Undo with
`%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.

**Changes.**
- Desktop context menu: a small helper (`rexplorer --desktop`, one per sign-in) shows rexplorer's menu when you
  right-click the desktop. On icons it has the shell's verbs for the selection; on the background it has View,
  Sort by, Refresh and Paste plus the shell's background verbs. Explorer still draws the desktop, the taskbar and
  Start. Install adds a Startup shortcut so the helper runs at every sign-in, and starts it right away.
- Install and uninstall close a running helper before they replace or remove the exe, and start it again
  afterwards. If an install fails, Roll back also removes or restores the Start menu and Startup shortcuts.
- Not covered yet: right-dragging desktop icons (the press goes to the menu), and Shift+F10 or the Menu key on the
  desktop, which still open Explorer's menu.

The binary is built by GitHub Actions from the tagged commit; `SHA256SUMS.txt` lists the checksums. It is not
code-signed, so SmartScreen may warn on first run.
