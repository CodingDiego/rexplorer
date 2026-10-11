Lower CPU use and quicker responses: rexplorer no longer reloads a folder on every change notification, preloads
less, and keeps slow shell work off the window's thread.

**Install.** Download the zip, extract it, and run `rexplorer.exe` to try it. `rexplorer.exe --install` makes it
the default file manager for your user; it needs no administrator rights. Already installed? Run the new
`rexplorer.exe --install` to replace the old copy. Undo with
`%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.

**CPU and latency.** Measured on the developer's machine against 0.1.1.
- Change notifications (#11): a folder whose files keep changing reloads at most once a second, and not at all while
  the window is minimized; restoring it catches up once. A folder under constant writes costs about a third of the
  CPU it did, and about none while minimized. Idle CPU is about 7x lower.
- Sidebar (#12): it is rebuilt every 30 seconds while shown instead of every 5, and right away when a drive is added
  or removed.
- Hover preview (#9): only text files and light images (png, jpg, gif, bmp, webp, ico) are loaded ahead when the
  pointer rests on them, only once it has stopped moving for the hover delay, and only while rexplorer is the
  active window. PDFs and other heavy types are previewed only when selected.
- Preview pane (#10): showing the same file again keeps its preview handler instead of starting a new one.
- Home (#13): it is built faster at startup, and its icons repaint in batches.
- First launch (#14): Windows' system icons are exported only when the theme uses them (`icon_theme=default`).
  First-launch CPU went from about 4.5 s to 0.5 s.
- Icons view (#15): heavy types (PDF, video, documents) show icons instead of rendered thumbnails; pictures keep
  thumbnails, and only pictures are loaded ahead of the scroll.
- Open terminal (#16): it starts in the background, so the window no longer waits for it.
- From the #17 backlog:
  - Sorting runs on the load thread. Cell text and icons are cached, so repaints and type-to-select no longer ask
    the shell again.
  - Switching tabs reloads a folder only if it changed while the tab was in the background.
  - Thumbnails of rows scrolled past are skipped, and thumbnails are scaled off the window's thread.
  - Copy, Cut, Paste, Delete and Properties no longer load every shell extension (Copy: about 350 ms down to 80 ms).
  - A navigation that is superseded stops early. Search stops at its result cap, and the elevated index search can
    be cancelled.
  - The breadcrumb chevron no longer lists network folders, the hidden folder tree is not kept in sync, and the
    text preview reads up to 64 KB.
  - A second launch waits for a busy first window instead of opening its own window after 3 s.

**Fixes.**
- Sorting keeps the selection on the same files. Before, it stayed on the same row positions, so a Delete right
  after a sort could act on a file you had not picked.
- Sorting in the Icons and Thumbnails views no longer leaves thumbnails on the wrong files.
- Open terminal finds Windows Terminal installed outside the Store (scoop, the zip) on PATH, and never runs a
  `wt.exe` found through the browsed folder.

**Roadmap.** `VERSION-1-X.md` lists what a stable 1.0 needs; each item has an issue in the v1.0 milestone (#71).

The binary is built by GitHub Actions from the tagged commit; `SHA256SUMS.txt` lists the checksums. It is not
code-signed, so SmartScreen may warn on first run.
