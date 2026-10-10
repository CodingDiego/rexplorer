Bug fixes from using rexplorer as the daily file manager, so it behaves more like Explorer.

**Install.** Download the zip, extract it, and run `rexplorer.exe` to try it. `rexplorer.exe --install` makes it
the default file manager for your user; it needs no administrator rights. Already installed? Run the new
`rexplorer.exe --install` to replace the old copy. Undo with
`%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.

**Fixes.**
- Box selection (#5): in Details view, a drag that starts on a row's Date, Type or Size column draws a selection
  rectangle, as in Explorer. A drag from a name still drags the item.
- Delete on Home's cloud files (#3): OneDrive recent files on Home that cannot be deleted from there no longer
  fail with an error; Delete beeps and the command bar's Delete is disabled.
- New tab (#6): Ctrl+T and the tab strip's + open the start place instead of a copy of the current folder.
- Launch (#4): a plain launch opens the start place, as Explorer does. View and pane widths still come from the
  last session, and a restart after a crash still restores the tabs.
- Shift+F10 and the Menu key open the context menu in the file list, and on Home under the focused item.
- "Always underline" in single-click mode underlines item names only, as in Explorer.
- Edits to `settings.ini` (Folder options, `--import-explorer`, bookmarks) keep the file's line endings and
  spacing.

The binary is built by GitHub Actions from the tagged commit; `SHA256SUMS.txt` lists the checksums. It is not
code-signed, so SmartScreen may warn on first run.
