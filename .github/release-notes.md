First public release: a Rust + Win32 file manager that replaces Windows Explorer's folder windows.

**Install.** Download the zip, extract it, and run `rexplorer.exe` to try it. `rexplorer.exe --install` makes it
the default file manager for your user; it needs no administrator rights. `--import-explorer` copies your
Explorer settings. Undo with `%LOCALAPPDATA%\Programs\rexplorer\rexplorer.exe --uninstall`.

**Highlights.** Tabs, Home page with Quick Access tiles, search as you type, Icons and Thumbnails views with real
image thumbnails, preview pane, RAR archives browsed in place, FTP connections, Folder options dialog with
Explorer-compatible settings and custom shortcuts, and fallback to the embedded Explorer view for anything it
cannot list. In the benchmark in the README it opens folders about 2x to 4x faster than Explorer's folder windows
in about a twentieth of the memory.

**Security.** This release includes the fixes from a whole-project review: RAR path traversal guard and libunrar
7.21, Mark-of-the-Web kept on extracted files, out-of-process preview handlers, FTP passwords never shown or saved,
and no elevated folder windows unless `--elevated` is passed. Report issues privately as described in
SECURITY.md.

The binary is built by GitHub Actions from the tagged commit; `SHA256SUMS.txt` lists the checksums. It is not
code-signed, so SmartScreen may warn on first run.
