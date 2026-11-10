# rexplorer 1.x: requirements for a stable release

rexplorer is at 0.1.x: fast, and in daily use as the default file manager, but tested on one Windows 11 machine.
**1.0 is a promise that anyone can make rexplorer their default file manager on any supported Windows PC without
losing data, getting locked out of a feature they rely on, or having their settings broken by an upgrade.** This
file lists what that promise needs. Every item has a "done when" and its own issue; all of them are in the
[v1.0 milestone](https://github.com/CodingDiego/rexplorer/milestone/1) and tracked in
[#71](https://github.com/CodingDiego/rexplorer/issues/71).

Legend: **[blocker]** must be done for 1.0 · **[should]** expected for 1.0 unless explicitly deferred with a reason in
the release notes · **[later]** fine for 1.x.

A box is ticked only when the code or text that meets its "done when" was read in the tree. *Status 2026-11-10*
lines record the v1 release-candidate round (branch `ADD/v1-release-candidate`, uncommitted): what exists and what
is left. "Compiled only" means its tests build and were not run; [HANDOFF.md](HANDOFF.md) lists every check still
to run.

## 1. Legal and branding

- [ ] **[blocker] Own app icon.** ([#18](https://github.com/CodingDiego/rexplorer/issues/18)) `crates/app/app.ico` was Windows Explorer's icon, copied from `explorer.exe` by
  `scripts/extract-icon.py`.
  *Done when:* rexplorer ships its own icon, `app.ico` is replaced in history-forward commits, the extraction script
  is removed or local-only, and the README no longer says the window looks like Explorer's because of it.
  *Status 2026-11-10:* an original icon (`scripts/make-icon.py`, standard library only; 8 sizes in `app.ico`),
  `extract-icon.py` deleted, README updated. Left: commit it, the user approves the artwork, and
  `THIRD-PARTY-NOTICES.md`'s Assets line still calls the icon Explorer's.
- [x] **[blocker] Complete third-party notices.** ([#19](https://github.com/CodingDiego/rexplorer/issues/19)) *Done when:* `THIRD-PARTY-NOTICES.md` covers every bundled crate
  license (generated, for example with `cargo about`) and every asset in the binary.
  *Status 2026-11-10:* the 20 crates in the exe (x64 and arm64), UnRAR and Intel's Slicing-by-8 notice, an Assets
  section; the `notices` job (`.github/workflows/ci.yml:39`) fails when the table and `cargo tree` disagree. The
  icon line must change with #18.
- [ ] **[should] Trademark hygiene.** ([#20](https://github.com/CodingDiego/rexplorer/issues/20)) *Done when:* no UI text, docs, or store listing suggests rexplorer is Windows
  Explorer or a Microsoft product.
  *Status 2026-11-10:* README non-affiliation line, a VERSIONINFO resource naming rexplorer only (`app.rc:4-27`), a
  localized shortcut description. Left: the icon line in `THIRD-PARTY-NOTICES.md`; the winget locale manifest is
  the first store listing to review.

## 2. Accessibility

The custom-drawn controls answer `WM_GETOBJECT` with an MSAA tree (`crates/core/src/access.rs`), which UI
Automation clients read through Windows' bridge. None of it has been tried with Narrator or NVDA.

- [ ] **[blocker] UI Automation for every custom control.** ([#21](https://github.com/CodingDiego/rexplorer/issues/21)) *Done when:* each one exposes name, role, state and
  invoke/selection patterns, and Narrator can read and operate tabs, crumbs, every toolbar button, every sidebar row,
  Home tiles and rows, and the preview caption.
  *Status 2026-11-10:* the preview and details pane now has an accessible object (`preview_pane.rs:353`, `:635`):
  a pane named after the file, one text line per caption or details row, name-change events. Left: Narrator and
  NVDA by hand; the address and search Edits have no accessible name; while a text or handler preview shows, the
  pane lists no child windows (pure-MSAA clients may lose the Edit or the handler).
- [ ] **[blocker] Full keyboard reach.** ([#22](https://github.com/CodingDiego/rexplorer/issues/22)) *Done when:* every control is reachable with Tab/F6 and operable without a
  mouse, the sidebar included. The focus is always visible, and the README's keyboard section covers each control.
  *Status 2026-11-10:* F6 and Tab now also reach the navigation toolbar, the tools bar and the command bar
  (`toolbar.rs:579-620`, focus rectangle `:522`; `main.rs:566` `pane_accel` keeps F6 and Tab working from them).
  Left on the first shown crumb opens the "..." overflow menu. README keys updated. Left: a real keyboard run; the
  overflow opens the root's subfolder menu, not a list of the hidden crumbs (as by mouse).
- [ ] **[blocker] High contrast.** ([#23](https://github.com/CodingDiego/rexplorer/issues/23)) *Done when:* with a Windows contrast theme on, rexplorer uses system colors
  instead of its themes, and nothing is invisible.
  *Status 2026-11-10:* built (`theme.rs` `effective_theme`); never seen under a real contrast theme.
- [ ] **[should] Text scaling.** ([#24](https://github.com/CodingDiego/rexplorer/issues/24)) *Done when:* Windows "Text size" (Accessibility > Text size) up to 225% gives no clipped or
  overlapping text.
  *Status 2026-11-10:* fonts and text-holding metrics follow `TextScaleFactor` (`theme.rs:275-308`), live on
  `WM_SETTINGCHANGE`. Left: the visual check at 150/200/225%; fixed or user-set widths do not grow (ellipsis).
- [ ] **[should] An accessibility check in the smoke test.** ([#25](https://github.com/CodingDiego/rexplorer/issues/25)) *Done when:* an automated pass walks the UIA tree and fails on unnamed
  interactive elements.
  *Status 2026-11-10:* `scripts/smoke.ps1` check `accessibility` (`:1172`), with a commented exclusion list. Never
  run, so the exclusions are untuned.

## 3. Data safety and reliability

- [ ] **[blocker] No data-loss paths.** ([#26](https://github.com/CodingDiego/rexplorer/issues/26)) Delete, move, rename, paste with conflicts, drag and drop, archive extraction,
  and New > item all go through the shell, as today, and have tests. *Done when:* each operation has an automated
  check that the result matches Explorer's on the same input, including conflicts and cancel.
  *Status 2026-11-10:* `crates/core/tests/data_ops.rs` (copy and move with Replace and Keep both, recycle and undo,
  rename onto a taken name, cancel, New folder) compiled only. Extraction never replaces an existing file and never
  writes outside the target (`archive.rs:35`, `:124-150`). Left: run the tests; Skip as an answer, end-to-end paste
  and drop, New > text document.
- [ ] **[blocker] Undo and redo.** ([#27](https://github.com/CodingDiego/rexplorer/issues/27)) Explorer users rely on Ctrl+Z. *Done when:* Ctrl+Z undoes the last copy, move, rename,
  delete-to-Recycle-Bin and new item through the shell's undo (or an equivalent), and Ctrl+Y redoes.
  *Status 2026-11-10:* now also recorded: drops on the sidebar, crumbs, tabs and Home (`list.rs:545-572`), the
  shell's New submenu, and Delete and rename in `--desktop-icons` (`desktop_icons.rs:600`). Not recorded: pastes
  staged in %TEMP%, Keep-both copies, paste or drop on `--desktop-icons`. Needs a real-mouse run.
- [ ] **[blocker] Panic audit.** ([#28](https://github.com/CodingDiego/rexplorer/issues/28)) *Done when:* no `unwrap`/`expect` remains on any shell, filesystem, registry or
  window result outside tests, and every fallible path degrades: a beep, a message, or the embedded Explorer view.
  *Status 2026-11-10:* COM/OLE init, the menu verb string, USN parsing and the mutex locks no longer panic; a scan
  leaves only guarded sites. Left: no `clippy::unwrap_used` lint in CI, a failed COM init continues silently, no
  audit of slicing and `as usize` indexing on shell results.
- [ ] **[blocker] Soak test.** ([#29](https://github.com/CodingDiego/rexplorer/issues/29)) *Done when:* an 8-hour scripted session (navigate, switch tabs, search, preview,
  thumbnails, file operations) ends with GDI objects, USER objects, handles, threads and private bytes flat within
  a stated margin. The test runs before every release.
  *Status 2026-11-10:* `scripts/soak.ps1` (margin: end median ≤ baseline median × 1.05 + slack). Never run; the
  8-hour run needs an idle machine. It leaves out the app's Delete and Undo (both recycle).
- [ ] **[should] Crash reports users can send.** ([#30](https://github.com/CodingDiego/rexplorer/issues/30)) Crash recovery exists (`--recover`, a second crash opens Explorer).
  *Done when:* a crash also writes a minidump and a short text report under `%LOCALAPPDATA%\rexplorer\crashes`, and
  `SECURITY.md`/README say how to send them (no telemetry).
  *Status 2026-11-10:* minidump per crash, the newest 10 kept (`crash.rs:86-160`); a panic also writes a `.txt`
  with message and backtrace; the recovered window offers to open the folder (`main.rs:509`). README says how to
  send them. Left: `SECURITY.md` text; native (SEH) crashes get the dump only, no `.txt`.
- [x] **[should] Hang watchdog.** ([#31](https://github.com/CodingDiego/rexplorer/issues/31)) *Done when:* a UI thread blocked for more than N seconds is detected and logged
  with the operation it was running.
  *Status 2026-11-10:* `crates/app/src/watchdog.rs`: N = 5 s, `hang:` and `answered again` lines in
  `rexplorer.log`, operation labels for navigate and the context menu. Tests compiled only.

## 4. Compatibility

- [ ] **[blocker] Supported Windows versions stated and tested.** ([#32](https://github.com/CodingDiego/rexplorer/issues/32)) *Done when:* the README names the minimum version,
  and every release is smoke-tested on at least Windows 10 22H2 and the two newest Windows 11 releases. Today only
  Windows 11 10.0.26200 is tested. Home already falls back to This PC where it does not exist.
  *Status 2026-11-10:* README names Windows 10 22H2 and 11; the Home fallback is `nav.rs` `fallback_of` with a test.
  Left: smoke runs on Windows 10 22H2 and two Windows 11 releases, a tested-on table.
- [ ] **[blocker] Display scaling and multi-monitor.** ([#33](https://github.com/CodingDiego/rexplorer/issues/33)) *Done when:* everything is checked at 100/125/150/200/250% and on mixed-DPI multi-monitor
  setups (drag between monitors), in light and dark mode.
  *Status 2026-11-10:* the main window, Folder options and Connect rebuild their fonts on `WM_DPICHANGED`. Left:
  the whole matrix by hand.
- [ ] **[blocker] Every place Explorer users browse.** ([#34](https://github.com/CodingDiego/rexplorer/issues/34)) *Done when:* each of these is verified:
  - mapped drives and UNC shares, both reachable and unreachable (no hang);
  - OneDrive placeholders (online-only, always-keep, sync errors);
  - MTP phones, WSL (`\\wsl$`), Linux shares, BitLocker-locked drives, removable media and the Recycle Bin;
  - long paths over 260 characters.

  *Status 2026-11-10:* the exe declares `longPathAware` (`crates/app/app.manifest`), the folder picker has no
  260-character buffer, and navigation parses only on its worker. Every place above still needs real hardware.
- [ ] **[should] ARM64 build.** ([#35](https://github.com/CodingDiego/rexplorer/issues/35)) *Done when:* releases include a native arm64 zip that passes the smoke test on ARM
  hardware.
  *Status 2026-11-10:* the release job cross-compiles `rexplorer-<v>-arm64.zip` and CI runs clippy for arm64.
  Never built or run; needs ARM hardware.
- [ ] **[should] FTP verified or dropped.** ([#36](https://github.com/CodingDiego/rexplorer/issues/36)) The README calls it untested against a live server. *Done when:* it is tested against one, or removed
  from the docs.
  *Status 2026-11-10:* deferred: README "Not part of 1.0" says FTP is present but unverified, with the reason.

## 5. Localization

Since 2026-11-10 rexplorer's own strings come from a table (`crates/core/src/i18n.rs`): English built in, Spanish
shipped, `lang\<code>.ini` to add a language. Command line, install and register messages are translated too.
Spanish dialogs have not been checked for clipped text; #37 stays open until that check.

- [ ] **[blocker] Externalized strings.** ([#37](https://github.com/CodingDiego/rexplorer/issues/37)) *Done when:* every user-visible string comes from a resource table, and a
  missing translation falls back to English.
  *Status 2026-11-10:* two more strings moved (the desktop icons window name, the Start menu shortcut
  description). The sweep was grep-based. Left: the Spanish clipped-text check.
- [x] **[should] First translations.** ([#38](https://github.com/CodingDiego/rexplorer/issues/38)) *Done when:* Spanish ships in addition to English, and the process to add a
  language is documented in CONTRIBUTING.md.
  *Status 2026-11-10:* CONTRIBUTING.md "Translations". Spanish still wants a proofreader.
- [ ] **[should] Right-to-left layout.** ([#39](https://github.com/CodingDiego/rexplorer/issues/39)) *Done when:* the window mirrors correctly for Arabic and Hebrew UI languages.
  *Status 2026-11-10:* deferred: README "Not part of 1.0", with the reason.

## 6. Explorer parity for daily use

Each gap below is either closed, or explicitly accepted for 1.0 and listed under "Known gaps" in the release notes.
The row-by-row status of every Explorer behaviour is in [docs/EXPLORER-PARITY.md](docs/EXPLORER-PARITY.md).

- [ ] **[blocker] No BROKEN or REPORTED BROKEN rows in the parity checklist.** As of 2026-11-09: dragging files
  to other programs ([#75](https://github.com/CodingDiego/rexplorer/issues/75)) is broken; F2 rename and click-again
  rename were reported broken and pass when driven with window messages. *Done when:* each is proven with a real
  keyboard and mouse on the built exe, and the checklist row reads WORKS.
  *Status 2026-11-10 (#75):* cause still unknown. A refused or cancelled drag no longer shows an error box, and each
  drag that applied nothing is logged to `rexplorer.log` with the button state and duration
  (`dragdrop.rs:167`), to find the cause on the next real-mouse run.
- [ ] **[blocker] Daily-use rows of the parity checklist.** *Done when:* every MISSING or PARTIAL row in its
  sections 1 to 7 and 10 is WORKS, or accepted for 1.0 with a reason in the release notes.

- [ ] **[blocker] Desktop menu completeness** ([#40](https://github.com/CodingDiego/rexplorer/issues/40)). The desktop menu shipped in 0.1.3
  (`rexplorer --desktop`). *Done when:* right-dragging desktop icons works as in Explorer, and Shift+F10 and the Menu
  key on the desktop open rexplorer's menu.
- [ ] **[blocker] Indexed search and full search results.** ([#41](https://github.com/CodingDiego/rexplorer/issues/41)) *Done when:* unelevated searches use the Windows Search index where it covers the folder,
  with the walker as fallback (today a drive-wide fast search needs `--elevated` for the USN index). Results also get
  real icons, thumbnails, preview, drag, sort, rename and refresh after delete.
  *Status 2026-11-10:* `crates/core/src/wsearch.rs` queries the index for a covered root and walks the folders its
  rules leave out (`excluded_below`); anything else falls back to the walk. Left: thumbnails and preview for result
  rows, and a live run with the WSearch service on.
- [x] **[should] Search results context menu and Back** ([#42](https://github.com/CodingDiego/rexplorer/issues/42)) across results from different folders, and Back returns to the results.
  *Status 2026-11-10:* `menu.rs:72` `for_scattered`; `nav.rs:29-41` keeps the last 4 results per tab. Not run on
  the exe.
- [ ] **[should] Archive view.** ([#43](https://github.com/CodingDiego/rexplorer/issues/43)) *Done when:* archive members get icons, thumbnails and preview in all views;
  multi-volume and encrypted RAR are tested; zip either goes through the same view or the difference is documented.
  *Status 2026-11-10:* member icons, picture thumbnails and a preview up to 32 MB (`archive_view.rs:142-169`). Left:
  encrypted RAR (no password prompt) and multi-volume fixtures, which need WinRAR.
- [x] **[should] Themed embedded Explorer view** ([#44](https://github.com/CodingDiego/rexplorer/issues/44)) follows the theme, or the difference is documented.
  *Status:* documented in the README Known gaps (it keeps Windows' own colors).
- [x] **[should] Home page completeness.** ([#45](https://github.com/CodingDiego/rexplorer/issues/45)) *Done when:* the location-status column exists, sort works, and the undocumented
  `System.Home.Grouping` value is guarded (a missing value degrades quietly).
  *Status 2026-11-10:* Status column (`home.rs:161`), header and Sort-menu sorting (`home.rs:392`), `is_shared`
  guard (`home.rs:155`). Not run on the exe; the sort is not saved across restarts.
- [ ] **[should] Send to, Properties on multiple selections, file conflict dialogs and progress** ([#46](https://github.com/CodingDiego/rexplorer/issues/46)) behave exactly as in
  Explorer (all from the shell; verified, not reimplemented).
  *Status 2026-11-10:* a test opens one Properties sheet for three files (`menu.rs:395`); no app path silences the
  shell's conflict or progress UI. Left: the on-screen comparison with Explorer.
- [ ] **[later] Tab drag and drop.** ([#47](https://github.com/CodingDiego/rexplorer/issues/47)) Drag a tab to a new window, and drag files onto a tab.
  *Status 2026-11-10:* tear-out and drops on a tab are built; merging a tab into another window is deferred (README
  "Not part of 1.0").

## 7. Performance budgets (enforced, not just measured)

Measured on 2026-10-11 with the 0.1.4 work, on a 12-core laptop:
- Home ink about 0.4 s from process start; a 100k-file folder listed in about 0.4 s.
- Idle while focused: about 0.55% of one core. Minimized under file churn: about 0.
- Terminal launch: 0.4 ms on the UI thread.

- [ ] **[blocker] Budgets in CI.** ([#48](https://github.com/CodingDiego/rexplorer/issues/48)) *Done when:* `scripts/bench-vs-explorer.ps1` (or a headless variant) runs on every
  PR and fails the build when a budget is exceeded. Budgets: cold start to first ink ≤ 600 ms, 100k-file folder
  ≤ 600 ms, idle CPU focused ≤ 1% of a core, churn reloads ≤ 1/s, no UI-thread call over 100 ms in the scripted
  session.
  *Status 2026-11-10:* `scripts/budgets.ps1` checks start (to the first visible window), the 100k listing and idle
  CPU. Never run, not in CI. Left: first ink, churn reloads and UI-thread calls need hooks in the app.
- [x] **[should] UI-thread budget tooling.** ([#49](https://github.com/CodingDiego/rexplorer/issues/49)) *Done when:* a debug build logs every UI-thread message that takes over 50 ms,
  with its source.
  *Status 2026-11-10:* `watchdog::dispatch` (`main.rs:531`) logs `slow:` lines in debug builds. Nested modal loops
  count as one message.
- [ ] **[should] Backlog from issue #17** ([#17](https://github.com/CodingDiego/rexplorer/issues/17)) is closed or explicitly accepted.

## 8. Stability contract (semantic versioning)

From 1.0, breaking any of these needs 2.0:

- [x] **[blocker] Command line frozen.** ([#50](https://github.com/CodingDiego/rexplorer/issues/50)) `--select` (and `/select,`), `--profile`, `--new-tab`/`-t`, `--search`,
  `--register`, `--unregister`, `--install`, `--uninstall`, `--import-explorer`, `--overwrite`, `--recover`,
  `--elevated`, `-h`/`--help`, and the Explorer argument forms rexplorer accepts or forwards. *Done when:* they are
  documented as stable, with a test per flag (`cli.rs` already checks that usage lists all of them).
  *Status 2026-11-10:* README Command line lists the frozen set; `cli.rs` `each_flag_sets_exactly_its_field` (compiled
  on x64, not yet run). `--new-window`, `--desktop-icons`, `--desktop` and the new `--quiet` are not frozen.
- [ ] **[blocker] Settings format versioned.** ([#51](https://github.com/CodingDiego/rexplorer/issues/51)) `settings.ini`, `themes\*.ini`, `icons\<name>\`, `[keys]` overrides
  and `session.txt`. *Done when:* the files carry a format version, every older format is migrated on load (tested
  with fixtures from each 0.x release), and unknown keys are preserved, not dropped.
  *Status 2026-11-10:* `format=1` (`settings.rs:14`) in settings.ini, themes, session.txt and folders.txt, stamped
  on the first v1 start; unknown keys kept; one 0.1.x fixture per file (0.1.0 to 0.1.3 are byte-identical). README
  records that `icons\<name>\` has no version (the PNG layout is the format). Left: run the tests, and the user
  approves that icons decision.
- [x] **[blocker] Cross-version hand-off.** ([#52](https://github.com/CodingDiego/rexplorer/issues/52)) *Done when:* the `WM_COPYDATA` payload between a running instance and a new
  launch is versioned, so an upgraded exe never mis-hands off to an older running one (or the reverse).
  *Status 2026-11-10:* `v=1` first line (`cli.rs:243`), a receiver answers 0 for another version and the sender
  opens its own window (`instance.rs:95-106`). The live two-build check is still to do.
- [x] **[should] Deprecation policy** ([#53](https://github.com/CodingDiego/rexplorer/issues/53)) written in the README: how long a deprecated key or flag keeps working.

## 9. Testing and engineering

- [ ] **[blocker] Smoke test in CI.** ([#54](https://github.com/CodingDiego/rexplorer/issues/54)) Today `scripts/smoke.ps1` runs locally, and its keyboard, share and dnd checks
  SKIP without an interactive foreground. *Done when:* it runs in CI on a runner with an interactive desktop (or
  those checks are rewritten to drive the app through messages), with zero SKIPs on the release run.
  *Status 2026-11-10:* keyboard, address, hotkey and tab checks now drive the app with window messages; `-NoSkip`
  fails on any SKIP; scratch install dir and registry base for the whole run. Left: never run; no CI job; share
  and dnd still need an interactive desktop.
- [ ] **[blocker] Data-operation tests** ([#55](https://github.com/CodingDiego/rexplorer/issues/55)) from section 3, in CI.
  *Status 2026-11-10:* `crates/core/tests/data_ops.rs`, picked up by CI's `cargo test`. Never run.
- [ ] **[should] Clippy clean** ([#56](https://github.com/CodingDiego/rexplorer/issues/56)) with `-D warnings` in CI, plus a stated minimum supported Rust version, checked in CI.
  *Status 2026-11-10:* clippy with `-D warnings` was clean on the x64 host (`cargo check` and `cargo clippy` only; the tests were
  written but not run, and arm64 was never built) and runs in CI (`ci.yml:25-37`, pinned 1.99.1);
  `rust-version = "1.88"` (`Cargo.toml:7`), stated in the README. Left: a CI job on 1.88; 1.88 was never built here.
- [x] **[should] Fuzzing** ([#57](https://github.com/CodingDiego/rexplorer/issues/57)) of path handling, archive path splitting and `settings.ini`/theme parsing.
  *Status 2026-11-10:* deterministic `fuzz_` unit tests on a seeded generator (`crates/core/src/fuzz.rs`) for
  settings, themes, language files, archive paths, the command line and the session. No cargo-fuzz campaign.
- [ ] **[should] Reproducible releases.** ([#58](https://github.com/CodingDiego/rexplorer/issues/58)) Release binaries are built only by CI from the tag, as today, and the build
  embeds no local paths.
  *Status 2026-11-10:* the release job pins Rust, remaps paths, links with `/Brepro`, builds twice and fails on a
  hash difference or a local path in the exe (`release.yml:24-50`). Never run.

## 10. Security

The whole project was reviewed on 2026-10-10 and every finding was fixed. These keep it that way:

- [ ] **[blocker] Code-signed releases.** ([#59](https://github.com/CodingDiego/rexplorer/issues/59)) Today SmartScreen warns on first run. *Done when:* the exe and the zip are
  signed by CI (signing key in a protected environment) and the signature is checked in the release job.
  *Status:* needs a certificate (the user).
- [ ] **[blocker] Pre-1.0 security review** ([#60](https://github.com/CodingDiego/rexplorer/issues/60)) repeated on the 1.0 candidate, covering everything added since
  2026-10-10, the Desktop helper (a low-level mouse hook) above all.
- [x] **[should] Dependency hygiene.** ([#61](https://github.com/CodingDiego/rexplorer/issues/61)) `cargo audit` runs weekly (exists). *Done when:* the pinned `windows` crate
  and `unrar` updates follow a written cadence, and Dependabot PRs are reviewed within a stated time.
  *Status 2026-11-10:* CONTRIBUTING.md "Dependencies"; the owner should confirm the proposed cadences.
- [x] **[should] Advisory response time.** ([#62](https://github.com/CodingDiego/rexplorer/issues/62)) `SECURITY.md` states a response time for private advisories.

## 11. Distribution and updates

- [ ] **[blocker] winget package.** ([#63](https://github.com/CodingDiego/rexplorer/issues/63)) *Done when:* `winget install rexplorer` works, and `winget upgrade` picks up
  releases.
  *Status 2026-11-10:* draft manifests under `manifests/c/CodingDiego/rexplorer/0.2.0/` (id
  `CodingDiego.rexplorer`, placeholder hashes) and `--install --quiet`. Left: `--install` writes no Add/Remove
  Programs entry, so winget would not see the package as installed; the hashes; the winget-pkgs PR.
- [ ] **[blocker] Upgrade path.** ([#64](https://github.com/CodingDiego/rexplorer/issues/64)) *Done when:* installing a new version over a running one is tested: the old copy is
  moved aside, the next launch runs the new one, settings carry over, and nothing is left elevated.
  *Status 2026-11-10:* `install.rs:630` installs, updates over a mapped exe and uninstalls under a scratch base
  (compiled only). Left: settings carry-over and "nothing elevated" are argued, not tested; a real upgrade between
  two releases.
- [ ] **[blocker] Uninstall restores Explorer completely.** ([#65](https://github.com/CodingDiego/rexplorer/issues/65)) *Done when:* every registry key, App Paths entry, Start
  menu shortcut and Startup entry is removed. It is tested on a clean VM.
  *Status 2026-11-10:* uninstall now also removes `rexplorer.old` and the empty parent folders install created.
  Left: the clean-VM run.
- [x] **[should] In-app update notice.** ([#66](https://github.com/CodingDiego/rexplorer/issues/66)) *Done when:* there is an opt-in check of the GitHub releases feed, with no
  telemetry.
  *Status 2026-11-10:* `crates/core/src/update.rs`, off by default, at most one HTTPS GET a day, no redirects,
  1 MiB cap. Never run against the network.
- [x] **[should] Full changelog.** ([#67](https://github.com/CodingDiego/rexplorer/issues/67)) *Done when:* `CHANGELOG.md` keeps every release's notes, not only the latest in
  `.github/release-notes.md`.

## 12. Documentation

- [ ] **[blocker] User guide.** ([#68](https://github.com/CodingDiego/rexplorer/issues/68)) *Done when:* it covers install, make default, revert to Explorer, keys, settings
  reference, themes and icon themes, and troubleshooting ("a folder hangs", "go back to Explorer now").
  *Status 2026-11-10:* [docs/USER-GUIDE.md](docs/USER-GUIDE.md), linked from the README. Left: it predates this
  round (it says rexplorer keeps no log files; no `--quiet`, update check, text size or crash dumps); a cold read and
  the revert steps on a VM.
- [x] **[should] Architecture overview** ([#69](https://github.com/CodingDiego/rexplorer/issues/69)) for contributors: the core/app split, threads and apartments, and the
  timers and messages.
  *Status 2026-11-10:* [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Line numbers drift; `wsearch` and the
  watchdog thread came after it.
- [x] **[should] Known gaps in release notes** ([#70](https://github.com/CodingDiego/rexplorer/issues/70)) kept current in every release's notes.

## 13. Exit criteria for 1.0

- [ ] Every **[blocker]** above is done.
- [ ] Every **[should]** is done or deferred with a written reason.
- [ ] A public beta (1.0.0-rc.N) has been used by at least five people other than the author as their default
  file manager for four weeks.
- [ ] No open crash, hang or data-loss bug, and no data-loss bug reported during the beta.
- [ ] Performance budgets green on the release candidate; soak test green.

## Suggested path

| Release | Focus |
|---|---|
| 0.2 | Own icon and notices; UI Automation and keyboard reach; high contrast |
| 0.3 | Signing, winget, upgrade and uninstall tests; Windows 10 and the DPI matrix; smoke test in CI |
| 0.4 | Externalized strings and Spanish; Ctrl+Z; data-operation tests; panic audit |
| 0.5 | Desktop menu completeness; indexed search; search and archive parity; performance budgets in CI; soak test |
| 1.0.0-rc | Settings format version, migrations and contract frozen; security review; public beta |
| 1.0 | Exit criteria met |

The update check skips a running version with a pre-release suffix, so a `1.0.0-rc.N` build shows no update notice.
