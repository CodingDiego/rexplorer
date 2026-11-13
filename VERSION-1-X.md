# rexplorer 1.x: requirements for a stable release

rexplorer's newest release is 1.0.0-rc.2 (2026-11-13), the second release candidate, published as a pre-release; the newest
stable release is 0.2.0 (2026-11-12). It is fast, and in daily use as the default file manager, but tested on one
Windows 11 machine.
**1.0 is a promise that anyone can make rexplorer their default file manager on any supported Windows PC without
losing data, getting locked out of a feature they rely on, or having their settings broken by an upgrade.** This
file lists what that promise needs. Every item has a "done when" and its own issue; all of them are in the
[v1.0 milestone](https://github.com/CodingDiego/rexplorer/milestone/1) and tracked in
[#71](https://github.com/CodingDiego/rexplorer/issues/71).

Legend: **[blocker]** must be done for 1.0 · **[should]** expected for 1.0 unless explicitly deferred with a reason in
the release notes · **[later]** fine for 1.x.

A box is ticked only when the code or text that meets its "done when" was read in the tree. *Status 2026-11-10*
lines record what the v1 release-candidate round (branch `ADD/v1-release-candidate`) built; *Status 2026-11-11*
lines record what was then run: the whole test suite (233 app, 214 core and 7 data-operation tests, three runs in a
row), clippy, `scripts/smoke.ps1` (0 FAIL, 2 SKIP that need `-UserState`), and live checks on a copy of the exe,
many with real keyboard and mouse input. [docs/EXPLORER-PARITY.md](docs/EXPLORER-PARITY.md) has the evidence per
behaviour. *Status 2026-11-12* lines record what the v0.2.0 release run and the CI runs on pull request #82 settled.
*Status 2026-11-13* lines record the v1 remaining-work round (branch `ADD/v1-remaining-work`, uncommitted): built,
`cargo check` and `cargo clippy -D warnings` clean, pure-logic unit tests run by name; window, COM, registry and
Recycle Bin tests compiled only (CI runs them); nothing run on the exe at that point. *Live 2026-11-12/13* lines
record what was then proven on copies of the release exe with real keyboard and mouse input: a test of the 0.2.0
build on 2026-11-12 (it reported nine problems, the rename data-loss bug among them), the fixes for eight of them
proven the same night (the ninth was not reproduced), and this round's features on 2026-11-13. The full release gate (release build, tests, clippy with
`-D warnings`) passed after each batch's last code change. `HANDOFF.md` (the maintainer's local hand-over notes,
kept out of git) lists what is left; the parity checklist's notes say what each row rests on.

### Blockers at a glance (2026-11-13, after the live batches)

| Done (code, CI or the release meets every clause) | Only a person can finish (hardware, account, Windows setting, judgment, certificate, store submission, a live run) | Still needs code or agent work |
|---|---|---|
| #19 notices, #28 panic audit, #40 desktop menu, #41 indexed search and results (proven live 2026-11-11 and 2026-11-13), #50 command line, #51 settings format, #52 hand-off, #55 data-operation tests in CI, #26 automated checks for every data operation (2026-11-13), #43 multi-volume .rar fixtures and tests (2026-11-13), no BROKEN parity rows | #18 icon art approval, #21 Narrator and NVDA, #22 the focus rectangle under a contrast theme, #23 contrast theme, #27 undo of Home tile drops and in `--desktop` / `--desktop-icons` (test account), #29 the 8-hour soak, #32 Windows 10 and two Windows 11 releases, #33 DPI and monitors, #34 places (OneDrive, MTP, WSL, drives), #37 proofreader and large Text size, #54 `-NoSkip -UserState` run on a test account, #59 signing certificate, #63 winget submission, #64 a real upgrade on a VM, #65 uninstall on a clean VM, #68 cold read and the revert steps on a VM | #27 drops onto the folder tree are not undoable, #48 churn-reload and 100 ms UI-call budgets, #60 the security review of this round's diff and of the desktop helper's hooks, daily-use parity rows (the drag image from Home) |

## 1. Legal and branding

- [ ] **[blocker] Own app icon.** ([#18](https://github.com/CodingDiego/rexplorer/issues/18)) `crates/app/app.ico` was Windows Explorer's icon, copied from `explorer.exe` by
  `scripts/extract-icon.py`.
  *Done when:* rexplorer ships its own icon, `app.ico` is replaced in history-forward commits, the extraction script
  is removed or local-only, and the README no longer says the window looks like Explorer's because of it.
  *Status 2026-11-10:* an original icon (`scripts/make-icon.py`, standard library only; 8 sizes in `app.ico`),
  `extract-icon.py` deleted, README updated. Left: commit it, the user approves the artwork, and
  `THIRD-PARTY-NOTICES.md`'s Assets line still calls the icon Explorer's.
  *Status 2026-11-11:* `THIRD-PARTY-NOTICES.md` Assets now names the icon as rexplorer's own. Left: the user approves the artwork.
  *Left 2026-11-13:* the user approves the artwork (v0.2.0 shipped with it).
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
  *Status 2026-11-11:* the icon line is fixed. Left: a person reviews the winget locale text.
  *Left 2026-11-13:* the user reads `manifests/c/CodingDiego/rexplorer/0.2.0/CodingDiego.rexplorer.locale.en-US.yaml`
  before the winget submission.

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
  *Status 2026-11-11:* the address box, search box, file list and text preview now have names; smoke `accessibility` walks every
  visible child window on Home and on a folder with the preview pane and finds no unnamed focusable or interactive
  object. Left: Narrator and NVDA (a person, with a speaker); the preview's child windows.
  *Status 2026-11-13:* the Home pills are exposed as tabs with focus events (`home.rs` `announce`, `acc_tree`), built,
  not run. Left: the user runs Narrator (and NVDA) through every pane; the preview's child windows (code).
- [ ] **[blocker] Full keyboard reach.** ([#22](https://github.com/CodingDiego/rexplorer/issues/22)) *Done when:* every control is reachable with Tab/F6 and operable without a
  mouse, the sidebar included. The focus is always visible, and the README's keyboard section covers each control.
  *Status 2026-11-10:* F6 and Tab now also reach the navigation toolbar, the tools bar and the command bar
  (`toolbar.rs:579-620`, focus rectangle `:522`; `main.rs:566` `pane_accel` keeps F6 and Tab working from them).
  Left on the first shown crumb opens the "..." overflow menu. README keys updated. Left: a real keyboard run; the
  overflow opens the root's subfolder menu, not a list of the hidden crumbs (as by mouse).
  *Status 2026-11-11:* real F6, Tab and their Shift forms walk every pane and the three toolbars with no trap; arrows, Home, End,
  Enter, Space, Down, Alt+Down and Esc work in the toolbars (Down on More fixed); the focus rectangle shows; arrows
  move on the tab strip. Left: the sidebar and Home keys pressed for real, the focus rectangle under a contrast theme.
  *Status 2026-11-13:* built, not run: the Home pills are keyboard stops (`home.rs:462` `to_pills`, `:471`
  `from_pills`, `:755` `on_pill`; Enter reaches Home on a pill, `main.rs:544`); Tab and F6 leave the preview pane,
  and only they are routed from it (`main.rs:618` `pane_accel`); F4 opens the address with its history
  (`main.rs:240`, `ui.rs:483`); Alt or F10 alone focuses the command bar (`main.rs:780`, `ui.rs:410`, `:415`); Esc
  closes the address suggestions first (`main.rs:542`, `edit.rs:19`); Down or a hold on Back / Forward opens the
  history (`toolbar.rs:251`, `:261`). The README keyboard section covers each control. The tab strip's "+" and close
  buttons are no stops (Ctrl+T, Ctrl+W do the same). Left: a real keyboard run through every pane on the PC (an agent
  with the input guard, or the user), and the focus rectangle under a contrast theme (the user).
  *Live 2026-11-12/13:* real keys in the sidebar after F6 (Home, End, PgDn, arrows, type-ahead, Enter, Esc), on the
  tab strip (Home, Right, End, Esc), on the Home pills (Up, Right, Space), Alt and F10 alone, F4, Esc in the address
  suggestions and Down or a hold on Back each worked; a click on a sidebar row now moves the focus there. Left: the
  focus rectangle under a contrast theme (the user).
- [ ] **[blocker] High contrast.** ([#23](https://github.com/CodingDiego/rexplorer/issues/23)) *Done when:* with a Windows contrast theme on, rexplorer uses system colors
  instead of its themes, and nothing is invisible.
  *Status 2026-11-10:* built (`theme.rs` `effective_theme`); never seen under a real contrast theme.
  *Status 2026-11-11:* unchanged. Only a person can switch Windows to a contrast theme.
  *Left 2026-11-13:* the user turns on a contrast theme and checks every surface.
- [ ] **[should] Text scaling.** ([#24](https://github.com/CodingDiego/rexplorer/issues/24)) *Done when:* Windows "Text size" (Accessibility > Text size) up to 225% gives no clipped or
  overlapping text.
  *Status 2026-11-10:* fonts and text-holding metrics follow `TextScaleFactor` (`theme.rs:275-308`), live on
  `WM_SETTINGCHANGE`. Left: the visual check at 150/200/225%; fixed or user-set widths do not grow (ellipsis).
  *Status 2026-11-11:* not checked by eye: Windows' Text size is the user's setting. Folder options and Connect keep a live font
  of their own while the main window replaces its font (checked by WM_GETFONT).
  *Left 2026-11-13:* the user sets Text size to 150, 200 and 225% and looks for clipped text.
- [x] **[should] An accessibility check in the smoke test.** ([#25](https://github.com/CodingDiego/rexplorer/issues/25)) *Done when:* an automated pass walks the UIA tree and fails on unnamed
  interactive elements.
  *Status 2026-11-10:* `scripts/smoke.ps1` check `accessibility` (`:1172`), with a commented exclusion list. Never
  run, so the exclusions are untuned.
  *Status 2026-11-11:* ticked. The check walks the MSAA tree (the managed UIA client read an empty tree) and passes on this
  build; a throwaway build with the custom controls' names blanked made it fail with 12 unnamed objects.

## 3. Data safety and reliability

- [ ] **[blocker] No data-loss paths.** ([#26](https://github.com/CodingDiego/rexplorer/issues/26)) Delete, move, rename, paste with conflicts, drag and drop, archive extraction,
  and New > item all go through the shell, as today, and have tests. *Done when:* each operation has an automated
  check that the result matches Explorer's on the same input, including conflicts and cancel.
  *Status 2026-11-10:* `crates/core/tests/data_ops.rs` (copy and move with Replace and Keep both, recycle and undo,
  rename onto a taken name, cancel, New folder) compiled only. Extraction never replaces an existing file and never
  writes outside the target (`archive.rs:35`, `:124-150`). Left: run the tests; Skip as an answer, end-to-end paste
  and drop, New > text document.
  *Status 2026-11-11:* `data_ops.rs` runs and passes (7 tests, three suite runs in a row; the rename test answers the
  shell's real conflict question). Extract here next to an older extraction kept the old files and reported the
  skipped ones; paste, cut and drop with Explorer were driven live. Left: Skip as an answer, New > text document,
  and a Replace / Keep both comparison with Explorer side by side.
  *Status 2026-11-12:* CI's `cargo test` ran `data_ops` on the hosted runner for pull request #82 (green).
  *Left 2026-11-13:* code: automated checks for Skip as the conflict answer and for New > text document (an agent);
  the user compares a Replace / Keep both run with Explorer side by side.
  *Live 2026-11-12/13:* the 0.2.0 build had a data-loss path: typing in a rename box with Back history went Back and
  Enter renamed the wrong file. Fixed in 1.0.0-rc.1 and proven live (22 of 22 renames). The shell's conflict dialog
  (Skip, Replace, keep both), its progress dialog and Cancel, Shift+Delete and New > Text document were driven live
  and left the expected files. Left: the automated Skip and New checks above.
  *Status 2026-11-13, later:* the automated checks are written and pass (11 tests in `data_ops.rs`, three suite
  runs): Skip pressed in the shell's own conflict dialog through its accessibility tree (copy and move,
  `paste_with_skip_in_the_shells_dialog_keeps_both_sides_and_does_the_rest`), the dialog's Replace against the
  `FOF_NO_UI` flag the other checks stand in with (`the_dialogs_replace_matches_the_no_ui_flag`), a copy and a move
  dropped on a folder's own drop target with and without a conflict (`a_drop_on_a_folder_does_what_a_paste_does`),
  New > Text Document twice against the engine's names (`new_text_document_names_match_the_engine`) and a cancelled
  delete, to the bin and for good (in `cancel_leaves_the_source_and_no_partial_destination`). Each operation and
  its check: delete `delete_to_the_bin_comes_back_and_permanent_does_not_use_it`; move
  `move_onto_an_existing_name_replace_and_keep_both`; rename `rename_onto_an_existing_name_loses_nothing`; paste with
  conflicts `copy_onto_an_existing_name_replace`, `copy_onto_an_existing_name_keep_both` and the two dialog tests;
  drag and drop `a_drop_on_a_folder_does_what_a_paste_does`; archive extraction (rexplorer's own, for .rar; zip is
  the shell's copy engine) `archive.rs` `extract_all_never_replaces_or_escapes`, `extract_one_never_escapes`,
  `multi_volume_middle_or_missing_volume_fails_clearly`; New > item `new_folder_names_match_the_engine`,
  `new_text_document_names_match_the_engine`; cancel `cancel_leaves_the_source_and_no_partial_destination`. Not
  automated: Keep both has no button of its own in Windows 11's dialog (it sits behind Compare, a second dialog
  with check boxes), so it stays the `FOF_RENAMEONCOLLISION` stand-in; it was driven live on 2026-11-13.
- [ ] **[blocker] Undo and redo.** ([#27](https://github.com/CodingDiego/rexplorer/issues/27)) Explorer users rely on Ctrl+Z. *Done when:* Ctrl+Z undoes the last copy, move, rename,
  delete-to-Recycle-Bin and new item through the shell's undo (or an equivalent), and Ctrl+Y redoes.
  *Status 2026-11-10:* now also recorded: drops on the sidebar, crumbs, tabs and Home (`list.rs:545-572`), the
  shell's New submenu, and Delete and rename in `--desktop-icons` (`desktop_icons.rs:600`). Not recorded: pastes
  staged in %TEMP%, Keep-both copies, paste or drop on `--desktop-icons`. Needs a real-mouse run.
  *Status 2026-11-11:* real-mouse and real-key runs undid and redid rename, New folder, Delete, copy and move pastes (Explorer's
  too), menu pastes, drops on the list, a folder row, a sidebar row, a crumb and a tab, and right-drag copy, move and
  shortcut. Fixed: paste from a folder row's menu, pastes from %TEMP%, Create shortcuts here. Left: drops on Home, the
  New submenu, `--desktop-icons` (never run from a session) and Keep-both copies.
  *Status 2026-11-13:* built, not run, for every case left: Keep-both copies and Skip (`Op::Transfer`,
  `crates/core/src/undo.rs:42`, `plan_transfer` `:153`, `could_be` `:172`, `holds` `:181`, `landed` `:193`), Home tile
  drops (`list.rs:594` `plan_drop`), New submenu items (`commands.rs:217` `Op::created`), paste and drop in
  `--desktop-icons` (`desktop_icons.rs:528` `plan_paste`, `:533` `paste_with`), and Delete in the `--desktop` helper
  done inside Explorer (`core/desktop.rs:158`). Every undo now refuses an item it cannot prove it made or moved (file
  id, or a free name of the shell's form holding the source's size and time). The bin's undelete runs synchronously
  (`menu.rs:316`, `:391`). Six pure undo tests run and pass. Left: the live checks in HANDOFF.md, on the PC; the
  `--desktop-icons` and `--desktop` ones on a test account (the user).
  *Live 2026-11-13:* rename, New folder, New > Text document, Delete, paste with Skip and Keep both (copy and cut),
  drag-move onto a folder row, right-drag Copy here and a drop on a tab were each undone and redone on disk; a copy
  swapped by another program and a re-made folder were left alone; a dismissed right-drag menu recorded nothing.
  Fixed that day: every undo of a delete left an orphan `$I` record in the bin. Left: drops onto the folder tree are
  not undoable (code); Home tile drops and the `--desktop` / `--desktop-icons` paths (the user, a test account); a
  cancelled folder copy keeps its partial folder on Ctrl+Z, by design.
- [x] **[blocker] Panic audit.** ([#28](https://github.com/CodingDiego/rexplorer/issues/28)) *Done when:* no `unwrap`/`expect` remains on any shell, filesystem, registry or
  window result outside tests, and every fallible path degrades: a beep, a message, or the embedded Explorer view.
  *Status 2026-11-10:* COM/OLE init, the menu verb string, USN parsing and the mutex locks no longer panic; a scan
  leaves only guarded sites. Left: no `clippy::unwrap_used` lint in CI, a failed COM init continues silently, no
  audit of slicing and `as usize` indexing on shell results.
  *Status 2026-11-13:* ticked. `#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]` at
  `crates/core/src/lib.rs:3` and `crates/app/src/main.rs:7` (the whole workspace), enforced by both clippy steps in CI;
  the remaining sites now degrade (a missing menu takes the error path, a failed settings backup reports the import
  as failed). Not covered by the lint: indexing and arithmetic panics, and `build.rs`.
- [ ] **[blocker] Soak test.** ([#29](https://github.com/CodingDiego/rexplorer/issues/29)) *Done when:* an 8-hour scripted session (navigate, switch tabs, search, preview,
  thumbnails, file operations) ends with GDI objects, USER objects, handles, threads and private bytes flat within
  a stated margin. The test runs before every release.
  *Status 2026-11-10:* `scripts/soak.ps1` (margin: end median ≤ baseline median × 1.05 + slack). Never run; the
  8-hour run needs an idle machine. It leaves out the app's Delete and Undo (both recycle).
  *Status 2026-11-11:* three 15-minute runs (`scripts/bench-results.md`) found a thread leak per Copy, Cut and Paste and two app
  bugs, all fixed; GDI still grows about 0.7 per paste inside the Windows copy engine. Left: the 8-hour run on an idle
  machine (the user), and a decision on that GDI growth.
  *Status 2026-11-13:* `soak.ps1` allows GDI one object per paste made between the baseline and end windows only
  (a `copies` column in the CSV), so a leak elsewhere still fails; parse-checked, not run. A weekly and manual
  30-minute soak job (`.github/workflows/fuzz-soak.yml`, never run yet) gives a signal between releases. Left: the
  user's 8-hour run (`scripts\soak.ps1 -Minutes 480`) on an idle machine before each release.
  *2026-11-13:* the 15-minute soak was not run for 1.0.0-rc.1.
- [x] **[should] Crash reports users can send.** ([#30](https://github.com/CodingDiego/rexplorer/issues/30)) Crash recovery exists (`--recover`, a second crash opens Explorer).
  *Done when:* a crash also writes a minidump and a short text report under `%LOCALAPPDATA%\rexplorer\crashes`, and
  `SECURITY.md`/README say how to send them (no telemetry).
  *Status 2026-11-10:* minidump per crash, the newest 10 kept (`crash.rs:86-160`); a panic also writes a `.txt`
  with message and backtrace; the recovered window offers to open the folder (`main.rs:509`). README says how to
  send them. Left: `SECURITY.md` text; native (SEH) crashes get the dump only, no `.txt`.
  *Status 2026-11-11:* a live crash left a minidump and relaunched with `--recover` on the same folder (also from a folder whose
  name has a space; smoke `crash-recovery` PASS); Yes on the notice opened the crashes folder. `SECURITY.md` now says
  where reports live and how to send them. Left: SEH crashes still get no `.txt`.
  *Status 2026-11-13:* ticked on code and docs, not yet seen on a real native crash. A native crash writes a small
  `.seh` record next to its dump (`crates/core/src/crash.rs:281` `write_record`, no heap); the next start, usually
  the automatic `--recover` one, turns it into `rexplorer-<time>-<pid>.txt` with version, exception code and name,
  address, module and offset, thread and UTC time (`:261` `convert_seh_records`, `:240` `seh_report`). Two pure tests
  run and pass. SECURITY.md and the README say where the files are and how to send them. Live check in HANDOFF.md.
  *Live 2026-11-13:* an induced access violation (`REXPLORER_CRASH_AFTER_MS`) left a `.dmp` and a `.txt` (version,
  0xC0000005, module and offset, thread, UTC time), started `--recover`, and its notice was answered.
- [x] **[should] Hang watchdog.** ([#31](https://github.com/CodingDiego/rexplorer/issues/31)) *Done when:* a UI thread blocked for more than N seconds is detected and logged
  with the operation it was running.
  *Status 2026-11-10:* `crates/app/src/watchdog.rs`: N = 5 s, `hang:` and `answered again` lines in
  `rexplorer.log`, operation labels for navigate and the context menu. Tests compiled only.
  *Status 2026-11-11:* with the UI thread suspended 7 s, `rexplorer.log` got the hang line and the "answered again" line; an
  11 s modal box logged nothing. The tests pass.

## 4. Compatibility

- [ ] **[blocker] Supported Windows versions stated and tested.** ([#32](https://github.com/CodingDiego/rexplorer/issues/32)) *Done when:* the README names the minimum version,
  and every release is smoke-tested on at least Windows 10 22H2 and the two newest Windows 11 releases. Today only
  Windows 11 10.0.26200 is tested. Home already falls back to This PC where it does not exist.
  *Status 2026-11-10:* README names Windows 10 22H2 and 11; the Home fallback is `nav.rs` `fallback_of` with a test.
  Left: smoke runs on Windows 10 22H2 and two Windows 11 releases, a tested-on table.
  *Status 2026-11-11:* smoke ran on this Windows 11 machine only. Left: Windows 10 22H2 and two Windows 11 releases (VMs).
  *Status 2026-11-12:* CI's hosted smoke subset passes on the GitHub `windows-latest` image. Left: the user runs
  `smoke.ps1` on Windows 10 22H2 and two Windows 11 releases (VMs) and adds a tested-on table.
- [ ] **[blocker] Display scaling and multi-monitor.** ([#33](https://github.com/CodingDiego/rexplorer/issues/33)) *Done when:* everything is checked at 100/125/150/200/250% and on mixed-DPI multi-monitor
  setups (drag between monitors), in light and dark mode.
  *Status 2026-11-10:* the main window, Folder options and Connect rebuild their fonts on `WM_DPICHANGED`. Left:
  the whole matrix by hand.
  *Status 2026-11-11:* unchanged; display scale and monitors are the user's settings and hardware.
  *Left 2026-11-13:* the user walks the scale matrix, light and dark, and drags the window between mixed-DPI monitors.
- [ ] **[blocker] Every place Explorer users browse.** ([#34](https://github.com/CodingDiego/rexplorer/issues/34)) *Done when:* each of these is verified:
  - mapped drives and UNC shares, both reachable and unreachable (no hang);
  - OneDrive placeholders (online-only, always-keep, sync errors);
  - MTP phones, WSL (`\\wsl$`), Linux shares, BitLocker-locked drives, removable media and the Recycle Bin;
  - long paths over 260 characters.

  *Status 2026-11-10:* the exe declares `longPathAware` (`crates/app/app.manifest`), the folder picker has no
  260-character buffer, and navigation parses only on its worker. Every place above still needs real hardware.
  *Status 2026-11-11:* with LongPathsEnabled on, a 304-character folder lists, real F2 renames in it (rename uses MoveFileExW
  there) and Delete asks to delete permanently; breadcrumb dropdowns list a share over SMB without blocking the
  window. Left: mapped drives, OneDrive, MTP, WSL, BitLocker and removable media.
  *Left 2026-11-13:* the user, with the hardware and accounts: mapped drives (reachable and not), OneDrive
  placeholders, an MTP phone, `\\wsl$`, a Samba share, a BitLocker-locked drive, a USB stick, the Recycle Bin.
  *Live 2026-11-12/13:* `\\localhost` and `\\localhost\C$` listed; an unreachable share kept the window answering
  and failed after 41 s with the shell's error; a 371-character folder renamed and deleted with real keys; the
  Recycle Bin restored 2 of 2 files and now shows file names. Left: the hardware and accounts listed above (the
  user); Empty Recycle Bin was never invoked.
- [ ] **[should] ARM64 build.** ([#35](https://github.com/CodingDiego/rexplorer/issues/35)) *Done when:* releases include a native arm64 zip that passes the smoke test on ARM
  hardware.
  *Status 2026-11-10:* the release job cross-compiles `rexplorer-<v>-arm64.zip` and CI runs clippy for arm64.
  Never built or run; needs ARM hardware.
  *Status 2026-11-11:* not built here (no aarch64 target installed). Left: the first release run and ARM hardware.
  *Status 2026-11-12:* the build half is settled: the v0.2.0 release job (run 43329502825, every step green) built
  `rexplorer-0.2.0-arm64.zip` twice with identical hashes and published it, and CI's arm64 clippy step passes. It has
  never run on ARM. Left: the user (or anyone with an ARM PC) runs the smoke test on ARM hardware.
- [ ] **[should] FTP verified or dropped.** ([#36](https://github.com/CodingDiego/rexplorer/issues/36)) The README calls it untested against a live server. *Done when:* it is tested against one, or removed
  from the docs.
  *Status 2026-11-10:* deferred: README "Not part of 1.0" says FTP is present but unverified, with the reason.
  *Left 2026-11-13:* deferred with a written reason; a test against a live FTP server (the user's choice of server)
  would close it, or removing it from the docs.

## 5. Localization

Since 2026-11-10 rexplorer's own strings come from a table (`crates/core/src/i18n.rs`): English built in, Spanish
shipped, `lang\<code>.ini` to add a language. Command line, install and register messages are translated too.
Spanish dialogs have not been checked for clipped text; #37 stays open until that check.

- [ ] **[blocker] Externalized strings.** ([#37](https://github.com/CodingDiego/rexplorer/issues/37)) *Done when:* every user-visible string comes from a resource table, and a
  missing translation falls back to English.
  *Status 2026-11-10:* two more strings moved (the desktop icons window name, the Start menu shortcut
  description). The sweep was grep-based. Left: the Spanish clipped-text check.
  *Status 2026-11-11:* Spanish checked on screenshots at 100%: four clipped Folder options labels and the details pane's overlap
  fixed; the i18n parity tests pass. Left: a proofreader, and the check at a larger Text size.
  *Status 2026-11-13:* the new texts of this round (`err.archive_readonly`, `err.rar_middle_volume`,
  `err.rar_volume_missing`) are in both `en.ini` and `es.ini` (`:93`, `:146`, `:147`); `menu.more.places_sidebar` is
  gone from both. Left: a Spanish proofreader and the Spanish dialogs at a larger Text size (the user).
- [x] **[should] First translations.** ([#38](https://github.com/CodingDiego/rexplorer/issues/38)) *Done when:* Spanish ships in addition to English, and the process to add a
  language is documented in CONTRIBUTING.md.
  *Status 2026-11-10:* CONTRIBUTING.md "Translations". Spanish still wants a proofreader.
- [ ] **[should] Right-to-left layout.** ([#39](https://github.com/CodingDiego/rexplorer/issues/39)) *Done when:* the window mirrors correctly for Arabic and Hebrew UI languages.
  *Status 2026-11-10:* deferred: README "Not part of 1.0", with the reason.
  *Status 2026-11-13:* the mechanism is built, not seen on screen. A language file with `[meta]` / `direction=rtl`
  turns it on (`crates/core/src/i18n.rs:22` `direction`, `:174` `is_rtl`); `crates/app/src/rtl.rs` gates every
  mirroring, reading-order, glyph, key and menu-anchor change, so English and Spanish are unchanged. Test language
  `crates/core/tests/lang/xr.ini` (not shipped). No Arabic or Hebrew translation ships. Left: the user looks at the
  window with `xr.ini` (steps in HANDOFF.md); a fluent reader for a real translation.
  *Live 2026-11-13:* with `xr.ini`, every pane, the context menu, a message box and the rename edit were mirrored and
  read correctly on screenshots; menus opening rightwards was fixed that day. Left: a real Arabic or Hebrew
  translation and a fluent reader.

## 6. Explorer parity for daily use

Each gap below is either closed, or explicitly accepted for 1.0 and listed under "Known gaps" in the release notes.
The row-by-row status of every Explorer behaviour is in [docs/EXPLORER-PARITY.md](docs/EXPLORER-PARITY.md).

- [x] **[blocker] No BROKEN or REPORTED BROKEN rows in the parity checklist.** As of 2026-11-09: dragging files
  to other programs ([#75](https://github.com/CodingDiego/rexplorer/issues/75)) is broken; F2 rename and click-again
  rename were reported broken and pass when driven with window messages. *Done when:* each is proven with a real
  keyboard and mouse on the built exe, and the checklist row reads WORKS.
  *Status 2026-11-10 (#75):* cause still unknown. A refused or cancelled drag no longer shows an error box, and each
  drag that applied nothing is logged to `rexplorer.log` with the button state and duration
  (`dragdrop.rs:167`), to find the cause on the next real-mouse run.
  *Status 2026-11-11:* ticked: the checklist has no BROKEN or REPORTED BROKEN row. On the final build, with real
  input guarded to rexplorer's own window: F2 rename 5 of 5 and click-again rename 5 of 5 in a folder (renamed on
  disk) and on Home (edit opened, stayed, Esc); dragging out to another program 5 of 5 from a folder and 5 of 5 from
  Home tiles. #75 did not reproduce on OLE, WM_DROPFILES and Explorer targets; browsers, WhatsApp and VS Code (the
  user's apps) were not tried, so #75 stays open until the user drags into one of them.
  *Live 2026-11-12/13:* a test of the 0.2.0 build found three BROKEN rows (rename, which could rename the wrong file;
  Libraries; zip folders). All three were fixed and proven live the same night, so the checklist again has no BROKEN
  row. #75: drags into Edge (5 of 5 each way) and into VS Code (5 of 5) delivered the file. Left: WhatsApp (the user).
- [ ] **[blocker] Daily-use rows of the parity checklist.** *Done when:* every MISSING or PARTIAL row in its
  sections 1 to 7 and 10 is WORKS, or accepted for 1.0 with a reason in the release notes.
  *Status 2026-11-13:* sections 1 to 7 and 10 hold no MISSING or PARTIAL row any more, but only because this round
  built the missing parts of five of them (Rename in .rar, Drag from Home, Ctrl+Z, Folder tree, Keyboard on Home),
  which are now UNVERIFIED. Left: those live checks (HANDOFF.md, an agent with the input guard or the user); then
  tick.
  *Live 2026-11-12/13:* Rename in .rar, Ctrl+Z, the folder tree and the keyboard on Home are WORKS. Left: the drag
  image from Home (UNVERIFIED), and "Quick access, This PC, drives, Network" is PARTIAL: the sidebar has no row that
  opens This PC itself. Build that row or accept the gap in the release notes, then tick.
  *2026-11-13, later:* the sidebar has a This PC row (`panes.rs` `sidebar_rows`); a posted click and posted keys
  opened This PC, and the row is WORKS. Left: the drag image from Home (UNVERIFIED), then tick.

- [x] **[blocker] Desktop menu completeness** ([#40](https://github.com/CodingDiego/rexplorer/issues/40)). The desktop menu shipped in 0.1.3
  (`rexplorer --desktop`). *Done when:* right-dragging desktop icons works as in Explorer, and Shift+F10 and the Menu
  key on the desktop open rexplorer's menu.
  *Status 2026-11-13:* ticked: built in #73 (`crates/app/src/desktop.rs`, the mouse hook's right-button press and
  the keyboard hook for Shift+F10 and the Menu key) and the issue was closed on 2026-10-11. The helper is never run
  from a working session; the check on a test account is in HANDOFF.md.
- [x] **[blocker] Indexed search and full search results.** ([#41](https://github.com/CodingDiego/rexplorer/issues/41)) *Done when:* unelevated searches use the Windows Search index where it covers the folder,
  with the walker as fallback (today a drive-wide fast search needs `--elevated` for the USN index). Results also get
  real icons, thumbnails, preview, drag, sort, rename and refresh after delete.
  *Status 2026-11-10:* `crates/core/src/wsearch.rs` queries the index for a covered root and walks the folders its
  rules leave out (`excluded_below`); anything else falls back to the walk. Left: thumbnails and preview for result
  rows, and a live run with the WSearch service on.
  *Status 2026-11-11:* live with WSearch on: result counts in an indexed folder equal a walk for five queries; the filters
  (`kind:`, `date:`, `size:`, `ext:`, Spanish keys) return exactly the expected files; thumbnails, preview and
  details pane work for results; a moved Documents folder falls back to the walk. Left: sort, rename and delete in
  results.
  *Status 2026-11-13:* no code change was needed: sort by every column (`source.rs` `cells_cmp`, `sort_rows`,
  `sort_search`), rename in place (`list.rs` `rename_item`), delete prune (`source.rs` `remove_indices`,
  `prune_search`) and refresh exist; the sort test now covers the Type column. Left: sort, rename, delete and F5 in
  results on the PC (an agent with the input guard, or the user).
  *Live 2026-11-13:* ticked. Real header clicks sorted results by each column both ways; F2 renamed a result and
  Ctrl+Z undid it; Delete removed its row; F5 picked up a new file and kept the sort (fixed that day). Open, minor:
  after a Delete no row is selected (Explorer selects the next one).
  *2026-11-13, later:* fixed: after Delete the row that takes the deleted one's place is selected and focused, the
  last row when the last went (`list.rs` `row_after_delete`, `select_after_delete`; search results in
  `source.rs` `prune_search`, folders on the reload in `nav.rs` `on_enum_done`). Proven by posted Delete keys, 3 of 3 in
  search results and 3 of 3 in a folder, including deleting the last row twice.
  *2026-11-13, after review:* in a folder only the reload after the user's own delete selects the next row: every
  delete of the selection goes through `list.rs` `verb_on_selection`, which arms the tab's `nav.rs` `Deleted`
  (folder, deleted names, the row after the focused one) and disarms it when nothing went (a cancelled
  confirmation); the reload of that folder within 10 s consumes it. Any other reload with the selection gone (another
  program, an editor's save, Ctrl+Z, a move out) selects nothing, and Home and Quick Access never do. Unit tested;
  not rerun live.
- [x] **[should] Search results context menu and Back** ([#42](https://github.com/CodingDiego/rexplorer/issues/42)) across results from different folders, and Back returns to the results.
  *Status 2026-11-10:* `menu.rs:72` `for_scattered`; `nav.rs:29-41` keeps the last 4 results per tab. Not run on
  the exe.
  *Status 2026-11-11:* proven live: one menu and one Properties sheet for results from two folders; Back returns to the results.
- [ ] **[should] Archive view.** ([#43](https://github.com/CodingDiego/rexplorer/issues/43)) *Done when:* archive members get icons, thumbnails and preview in all views;
  multi-volume and encrypted RAR are tested; zip either goes through the same view or the difference is documented.
  *Status 2026-11-10:* member icons, picture thumbnails and a preview up to 32 MB (`archive_view.rs:142-169`). Left:
  encrypted RAR (no password prompt) and multi-volume fixtures, which need WinRAR.
  *Status 2026-11-11:* a password prompt for encrypted RAR5 (headers or members), proven live on fixtures built by
  `scripts/make-rar-fixtures.ps1`; members drag out to other programs (live); crafted names and links are refused
  (live). Left: thumbnails checked by eye, multi-volume archives.
  *Status 2026-11-13:* multi-volume sets built, not run: they open from the first part; a middle part and a missing
  part give clear messages (`crates/core/src/archive.rs:112` `first_volume`, `:99` `rar_err`); a member cut off by a
  missing part is removed (`:117` `drop_partial`). `only_the_first_volume_opens` passes; the two set tests are
  `#[ignore]` until a fixture exists. In a .rar, F2 now says archives are read-only. Left: the user installs WinRAR
  and makes `crates/core/tests/fixtures/multivol` (`rar a -v1k -ep set.rar ...`), then the ignored tests and the
  live checks run; member thumbnails by eye (the user). Zip stays the shell's, as documented.
  *Live 2026-11-12/13:* member type icons, a picture member's thumbnail in Large icons and the preview of a picture
  and a text member were seen on screenshots; F2 on a member shows the read-only message (English and Spanish).
  Zip folders, broken in 0.2.0 (no names), now list, open members and copy them out. Left: multi-volume sets (needs
  WinRAR for the fixture).
  *2026-11-13, later:* WinRAR 7.23 made `crates/core/tests/fixtures/multivol/set.part{1,2,3}.rar` (own content:
  seeded random bytes and a line of text); the two set tests run everywhere and pass. In the app (posted messages,
  copy of the release exe): part 1 lists the three members and Extract here writes them whole; a middle part and a
  set with a part missing show their messages and write nothing. Every clause of the done-when now holds.
- [x] **[should] Themed embedded Explorer view** ([#44](https://github.com/CodingDiego/rexplorer/issues/44)) follows the theme, or the difference is documented.
  *Status:* documented in the README Known gaps (it keeps Windows' own colors).
- [x] **[should] Home page completeness.** ([#45](https://github.com/CodingDiego/rexplorer/issues/45)) *Done when:* the location-status column exists, sort works, and the undocumented
  `System.Home.Grouping` value is guarded (a missing value degrades quietly).
  *Status 2026-11-10:* Status column (`home.rs:161`), header and Sort-menu sorting (`home.rs:392`), `is_shared`
  guard (`home.rs:155`). Not run on the exe; the sort is not saved across restarts.
  *Status 2026-11-11:* not driven live (Home lists the user's real places); smoke `home-page` PASS.
  *Note 2026-11-13:* the parity row "Home: location status column, sort" is still UNVERIFIED; a look at the Status
  column and a header click on Home is in HANDOFF.md.
  *Live 2026-11-12:* the Status column showed (empty for local files), and real header clicks and the Sort menu
  sorted the recent rows; the parity row is WORKS.
- [ ] **[should] Send to, Properties on multiple selections, file conflict dialogs and progress** ([#46](https://github.com/CodingDiego/rexplorer/issues/46)) behave exactly as in
  Explorer (all from the shell; verified, not reimplemented).
  *Status 2026-11-10:* a test opens one Properties sheet for three files (`menu.rs:395`); no app path silences the
  shell's conflict or progress UI. Left: the on-screen comparison with Explorer.
  *Status 2026-11-11:* Properties on three files and on results from several folders opens one sheet (live). Left: Send to, and
  the on-screen comparison with Explorer.
  *Left 2026-11-13:* the user compares Send to, a conflict dialog and the progress dialog with Explorer's on screen.
  *Live 2026-11-12:* Send to > Compressed folder made the zip; the shell's conflict dialog (Skip, Replace, keep both)
  and progress dialog with Cancel ran in rexplorer's process and left the expected files. One difference: after
  Send to, Explorer selects the new zip and starts a rename, rexplorer leaves the source selected. Left: decide on
  that difference; the elevation dialog (the user, a UAC prompt).
- [ ] **[later] Tab drag and drop.** ([#47](https://github.com/CodingDiego/rexplorer/issues/47)) Drag a tab to a new window, and drag files onto a tab.
  *Status 2026-11-10:* tear-out and drops on a tab are built; merging a tab into another window is deferred (README
  "Not part of 1.0").
  *Status 2026-11-13:* merging is built, not run: a tab released over another rexplorer window moves there
  (`nav.rs:1424` `tear_tab`, `instance.rs:119` `other_main_window`, `:129` `tab_payload`, the hand-off on a worker
  thread; a refusal reopens the tab, `main.rs:784`). Left: two windows on the PC, the live check in HANDOFF.md.
  *Live 2026-11-13:* a tab dragged onto a second window moved there; a folder tab and a Home tab released elsewhere
  each opened a new window; a file dropped on a tab moved and was undone. Open: once a torn Home tab landed in the
  other window instead (1 of 2, not reproduced).
  *2026-11-13, later (built, not run live):* WindowFromPoint answers a top-level window for its invisible resize
  border too (about 8 px around the visible edge), so a release just beside the other window counted as over it.
  A release now moves the tab only inside that window's DWM frame bounds, and never onto a minimized or cloaked
  window (`instance.rs` `release_lands_on`, `lands_on`; two unit tests). Left: the six live tear-outs (two windows
  overlapping, side by side, one behind a third program), an agent with the input guard while the PC is idle.

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
  *Status 2026-11-11:* measured on this machine: PASS at 400 ms start, 400 ms for 100k files and 0.5% idle
  (`scripts/bench-results.md`). Left: budgets for a CI runner, and the in-app hooks.
  *Status 2026-11-13:* the app now marks its first paint and each finished load as properties on its main window
  (`instance.rs:78-80` `rexplorer.painted`, `rexplorer.loads`, `rexplorer.loadtick`; set from `ui.rs:446` and
  `nav.rs:537`), and `budgets.ps1` times start to first paint and waits until the list holds every file. CI's
  `build-test` job runs it on every pull request with hosted values (`ci.yml:32-33`: 2000 ms, 2000 ms, 3 %), a
  tripwire at about ten times the laptop medians; not run yet. Left (code, an agent): the churn-reload (≤ 1/s) and
  100 ms UI-call budgets are not measured; after the first CI run, tighten the hosted values to about 3x the runner's
  medians. The local budget and smoke scripts were not run for 1.0.0-rc.1; CI runs both.
- [x] **[should] UI-thread budget tooling.** ([#49](https://github.com/CodingDiego/rexplorer/issues/49)) *Done when:* a debug build logs every UI-thread message that takes over 50 ms,
  with its source.
  *Status 2026-11-10:* `watchdog::dispatch` (`main.rs:531`) logs `slow:` lines in debug builds. Nested modal loops
  count as one message.
  *Status 2026-11-13:* checked: every piece is `cfg(debug_assertions)` (`watchdog.rs:19`, `:48`, `:51`), the pump
  calls it (`main.rs:550`). A debug-build run on a big folder is in HANDOFF.md.
- [ ] **[should] Backlog from issue #17** ([#17](https://github.com/CodingDiego/rexplorer/issues/17)) is closed or explicitly accepted.
  *Left 2026-11-13:* the user decides which #17 items are accepted for 1.0.

## 8. Stability contract (semantic versioning)

From 1.0, breaking any of these needs 2.0:

- [x] **[blocker] Command line frozen.** ([#50](https://github.com/CodingDiego/rexplorer/issues/50)) `--select` (and `/select,`), `--profile`, `--new-tab`/`-t`, `--search`,
  `--register`, `--unregister`, `--install`, `--uninstall`, `--import-explorer`, `--overwrite`, `--recover`,
  `--elevated`, `-h`/`--help`, and the Explorer argument forms rexplorer accepts or forwards. *Done when:* they are
  documented as stable, with a test per flag (`cli.rs` already checks that usage lists all of them).
  *Status 2026-11-10:* README Command line lists the frozen set; `cli.rs` `each_flag_sets_exactly_its_field` (compiled
  on x64, not yet run). `--new-window`, `--desktop-icons`, `--desktop` and the new `--quiet` are not frozen.
  *Status 2026-11-11:* the test runs and passes.
- [ ] **[blocker] Settings format versioned.** ([#51](https://github.com/CodingDiego/rexplorer/issues/51)) `settings.ini`, `themes\*.ini`, `icons\<name>\`, `[keys]` overrides
  and `session.txt`. *Done when:* the files carry a format version, every older format is migrated on load (tested
  with fixtures from each 0.x release), and unknown keys are preserved, not dropped.
  *Status 2026-11-10:* `format=1` (`settings.rs:14`) in settings.ini, themes, session.txt and folders.txt, stamped
  on the first v1 start; unknown keys kept; one 0.1.x fixture per file (0.1.0 to 0.1.3 are byte-identical). README
  records that `icons\<name>\` has no version (the PNG layout is the format). Left: run the tests, and the user
  approves that icons decision.
  *Status 2026-11-11:* live on a 0.1.x profile: only `format=1` added; comments, CRLF and unknown keys kept; a second start
  changes nothing. The tests pass. Left: the user approves the icons decision.
- [x] **[blocker] Cross-version hand-off.** ([#52](https://github.com/CodingDiego/rexplorer/issues/52)) *Done when:* the `WM_COPYDATA` payload between a running instance and a new
  launch is versioned, so an upgraded exe never mis-hands off to an older running one (or the reverse).
  *Status 2026-11-10:* `v=1` first line (`cli.rs:243`), a receiver answers 0 for another version and the sender
  opens its own window (`instance.rs:95-106`). The live two-build check is still to do.
  *Status 2026-11-11:* live: a second copy handed off in either start order; `v=2` is refused, `v=1` and none are taken.
- [x] **[should] Deprecation policy** ([#53](https://github.com/CodingDiego/rexplorer/issues/53)) written in the README: how long a deprecated key or flag keeps working.

## 9. Testing and engineering

- [ ] **[blocker] Smoke test in CI.** ([#54](https://github.com/CodingDiego/rexplorer/issues/54)) Today `scripts/smoke.ps1` runs locally, and its keyboard, share and dnd checks
  SKIP without an interactive foreground. *Done when:* it runs in CI on a runner with an interactive desktop (or
  those checks are rewritten to drive the app through messages), with zero SKIPs on the release run.
  *Status 2026-11-10:* keyboard, address, hotkey and tab checks now drive the app with window messages; `-NoSkip`
  fails on any SKIP; scratch install dir and registry base for the whole run. Left: never run; no CI job; share
  and dnd still need an interactive desktop.
  *Status 2026-11-11:* full local run 0 FAIL, 2 SKIP (pin and home-open need `-UserState`); CI `build-test` runs `-Hosted`.
  Left: the first run on a hosted runner; `-NoSkip -UserState` on a test account.
  *Status 2026-11-12:* the hosted subset runs in CI on every pull request and on master; it passed on pull request
  #82 (run 43327795680, step "Smoke test (hosted subset)") and on the merge to master. Left: the release run with
  zero SKIPs, `smoke.ps1 -NoSkip -UserState` on an interactive desktop under a test account (the user), because
  share, dnd, the Explorer-shell checks and the timings need a desktop a hosted runner does not give.
- [x] **[blocker] Data-operation tests** ([#55](https://github.com/CodingDiego/rexplorer/issues/55)) from section 3, in CI.
  *Status 2026-11-10:* `crates/core/tests/data_ops.rs`, picked up by CI's `cargo test`. Never run.
  *Status 2026-11-11:* 7 of 7 pass locally, three runs in a row. Left: the first CI run.
  *Status 2026-11-12:* ticked: CI's `cargo test --release --locked` (which runs `data_ops`) passed on pull request
  #82 and on master, and the release job ran it again before building v0.2.0.
- [ ] **[should] Clippy clean** ([#56](https://github.com/CodingDiego/rexplorer/issues/56)) with `-D warnings` in CI, plus a stated minimum supported Rust version, checked in CI.
  *Status 2026-11-10:* clippy with `-D warnings` was clean on the x64 host (`cargo check` and `cargo clippy` only; the tests were
  written but not run, and arm64 was never built) and runs in CI (`ci.yml:25-37`, pinned 1.99.1);
  `rust-version = "1.88"` (`Cargo.toml:7`), stated in the README. Left: a CI job on 1.88; 1.88 was never built here.
  *Status 2026-11-11:* clippy clean with `-D warnings` on x64. Left: a 1.88 CI job and a 1.88 build.
  *Status 2026-11-13:* CI job `msrv` (`ci.yml:37-50`) reads `rust-version` from `Cargo.toml` and builds
  `--all-targets` with exactly that toolchain; clippy x64 and arm64 passed on pull request #82. Left: the `msrv` job's
  first run, on the next pull request (CI); if a locked dependency needs a newer Rust, raise `rust-version` (the user
  decides).
- [x] **[should] Fuzzing** ([#57](https://github.com/CodingDiego/rexplorer/issues/57)) of path handling, archive path splitting and `settings.ini`/theme parsing.
  *Status 2026-11-10:* deterministic `fuzz_` unit tests on a seeded generator (`crates/core/src/fuzz.rs`) for
  settings, themes, language files, archive paths, the command line and the session. No cargo-fuzz campaign.
  *Status 2026-11-11:* cargo-fuzz targets in `fuzz/` (outside the workspace); not run (needs nightly).
  *Status 2026-11-13:* seven targets (three new: search query, update response, session and folders files), seed
  corpora in `fuzz/corpus/`, `fuzz/README.md`, hostile-input tests (1 MB lines, deep nesting, NUL, lone surrogates,
  numeric overflow; `crates/core/src/fuzz.rs:80`, `crates/app/src/session.rs:483`), and a weekly and manual CI job
  that fuzzes every target three minutes (`.github/workflows/fuzz-soak.yml`). No parser panicked. Not run with
  cargo-fuzz yet: the first scheduled or manual run of "Fuzz and soak" does that. For 1.0.0-rc.1 the job pins the
  version of the fuzz tool it installs (being finished, not run); the targets were not run for this release.
- [x] **[should] Reproducible releases.** ([#58](https://github.com/CodingDiego/rexplorer/issues/58)) Release binaries are built only by CI from the tag, as today, and the build
  embeds no local paths.
  *Status 2026-11-10:* the release job pins Rust, remaps paths, links with `/Brepro`, builds twice and fails on a
  hash difference or a local path in the exe (`release.yml:24-50`). Never run.
  *Status 2026-11-11:* not run (needs a tag).
  *Status 2026-11-12:* ticked: v0.2.0 was built only by the release job from the tag (run 43329502825): tag matches
  the crate version, tests, two clean builds of x64 and arm64 with equal hashes and no local path in either exe, both
  zips and `SHA256SUMS.txt`, every step green. A rebuild on a later runner image can still differ (the MSVC linker
  and the libunrar C++ compiler come from the image), as `release.yml` says.

## 10. Security

The whole project was reviewed on 2026-10-10 and every finding was fixed. These keep it that way:

- [ ] **[blocker] Code-signed releases.** ([#59](https://github.com/CodingDiego/rexplorer/issues/59)) Today SmartScreen warns on first run. *Done when:* the exe and the zip are
  signed by CI (signing key in a protected environment) and the signature is checked in the release job.
  *Status:* needs a certificate (the user).
  *Left 2026-11-13:* the user gets a code-signing certificate; then an agent adds the signing and the check to
  `release.yml` with the key in a protected environment.
- [ ] **[blocker] Pre-1.0 security review** ([#60](https://github.com/CodingDiego/rexplorer/issues/60)) repeated on the 1.0 candidate, covering everything added since
  2026-10-10, the Desktop helper (a low-level mouse hook) above all.
  *Status 2026-11-13:* the 0.2.0 diff (pull request #82, 2026-11-10 to 2026-11-12) had four security review passes;
  every finding of medium severity or higher was fixed (dated record in SECURITY.md). Left (an agent): the security
  review of this round's uncommitted diff before it is committed, and an explicit pass over `crates/app/src/desktop.rs`
  (the low-level mouse and keyboard hooks), which the record does not name as reviewed.
  *2026-11-13:* this round changed one internal message to carry no pointer, kept the Home drag image out of
  cross-thread calls, deletes an oversized crash record unread, and checks a copied folder's file id before undo.
  SECURITY.md lists the two limitations still open (forgeable window messages, and no re-check of a file's identity
  while the shell's confirmation dialog is open).
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
  *Status 2026-11-11:* `--install` now writes the Settings > Apps entry (checked on a scratch base: UninstallString, NoModify,
  NoRepair). Left: the hashes, the winget-pkgs PR, a clean-VM install and upgrade (the user).
  *Status 2026-11-13:* the manifests carry the real v0.2.0 asset URLs and SHA-256 values (checked against
  `gh release view v0.2.0`), schema 1.6.0, and `RelativeFilePath` `rexplorer-0.2.0-<arch>\rexplorer.exe` as
  `release.yml` packs it (not checked against the zip itself). Left: the user runs `winget validate` and a
  `winget install --manifest` on a VM, submits the pull request to microsoft/winget-pkgs, and after the merge checks
  `winget install CodingDiego.rexplorer` and `winget upgrade` with the next release.
- [ ] **[blocker] Upgrade path.** ([#64](https://github.com/CodingDiego/rexplorer/issues/64)) *Done when:* installing a new version over a running one is tested: the old copy is
  moved aside, the next launch runs the new one, settings carry over, and nothing is left elevated.
  *Status 2026-11-10:* `install.rs:630` installs, updates over a mapped exe and uninstalls under a scratch base
  (compiled only). Left: settings carry-over and "nothing elevated" are argued, not tested; a real upgrade between
  two releases.
  *Status 2026-11-11:* the update-over-a-running-copy test passes; scratch `--install --quiet` then `--uninstall --quiet` leave
  the registry export as before. Left: a real upgrade between two releases.
  *Status 2026-11-13:* two tests, compiled, for CI (they use a scratch HKCU base, COM shortcuts and a mapped image):
  `a_successful_update_over_a_running_exe_moves_it_to_old` now checks that the user files stay and that the next
  start removes `rexplorer.old` (`install.rs:830`, `remove_old_in` `:620`);
  `a_0_1_4_install_upgrades_unregisters_and_uninstalls_to_the_original_base` (`:856`) upgrades a hand-built 0.1.4
  install, unregisters and uninstalls to the original registry. Nothing elevated holds by construction (HKCU only). Left: their first CI run, and the user
  upgrades a real 0.1.4 (or 0.2.0) install on a VM and compares `reg export HKCU\Software` with a snapshot.
  *Live 2026-11-13, scratch base and folders:* the exe's `--install --quiet` over a hand-built 0.1.4 install kept
  every old record line and added the Uninstall entry; `--unregister --quiet` and `--uninstall --quiet` then left
  nothing behind. Left: a real upgrade on a VM (the user).
- [ ] **[blocker] Uninstall restores Explorer completely.** ([#65](https://github.com/CodingDiego/rexplorer/issues/65)) *Done when:* every registry key, App Paths entry, Start
  menu shortcut and Startup entry is removed. It is tested on a clean VM.
  *Status 2026-11-10:* uninstall now also removes `rexplorer.old` and the empty parent folders install created.
  Left: the clean-VM run.
  *Status 2026-11-11:* on a scratch base, uninstall, the UninstallString run alone and `restore-explorer.ps1 -RemoveProgram`
  each left a `reg export` identical to before. Left: the clean-VM run.
  *Left 2026-11-13:* the user runs `--install`, `--register`, `--uninstall` on a clean Windows 11 VM and confirms with
  `reg query` that the folder command keys, App Paths and the Uninstall entry are gone, that `shell:programs` and
  `shell:startup` hold no rexplorer shortcut, and that `restore-explorer.ps1` reports nothing to do.
  *Live 2026-11-13, scratch base and folders:* after an upgrade from a hand-built 0.1.4 install, `--uninstall
  --quiet` left no key, no install folder and no shortcut. Left: the clean-VM run (the user).
- [x] **[should] In-app update notice.** ([#66](https://github.com/CodingDiego/rexplorer/issues/66)) *Done when:* there is an opt-in check of the GitHub releases feed, with no
  telemetry.
  *Status 2026-11-10:* `crates/core/src/update.rs`, off by default, at most one HTTPS GET a day, no redirects,
  1 MiB cap. Never run against the network.
  *Status 2026-11-11:* live: off makes no request; on makes one real request; a newer version shows the status bar notice and
  the More entry.
- [x] **[should] Full changelog.** ([#67](https://github.com/CodingDiego/rexplorer/issues/67)) *Done when:* `CHANGELOG.md` keeps every release's notes, not only the latest in
  `.github/release-notes.md`.

## 12. Documentation

- [ ] **[blocker] User guide.** ([#68](https://github.com/CodingDiego/rexplorer/issues/68)) *Done when:* it covers install, make default, revert to Explorer, keys, settings
  reference, themes and icon themes, and troubleshooting ("a folder hangs", "go back to Explorer now").
  *Status 2026-11-10:* [docs/USER-GUIDE.md](docs/USER-GUIDE.md), linked from the README. Left: it predates this
  round (it says rexplorer keeps no log files; no `--quiet`, update check, text size or crash dumps); a cold read and
  the revert steps on a VM.
  *Status 2026-11-11:* the guide now covers logs, crash dumps, `--quiet`, the update check, Text size, toolbar keys and
  rename. Left: a cold read and the revert steps on a VM.
  *Status 2026-11-13:* the guide covers this round's keys (F4, Alt / F10, Esc in the address, the Home pills,
  press-and-hold, Ctrl+B and the folder tree), undo's new cases and crash `.txt` reports. Left: a cold read by
  someone new to rexplorer, and the revert steps followed on a VM (the user).
- [x] **[should] Architecture overview** ([#69](https://github.com/CodingDiego/rexplorer/issues/69)) for contributors: the core/app split, threads and apartments, and the
  timers and messages.
  *Status 2026-11-10:* [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Line numbers drift; `wsearch` and the
  watchdog thread came after it.
  *Status 2026-11-11:* line references checked against the code; the thread table now lists the index search, archive job,
  breadcrumb, jump list and watchdog threads, and the message table the newer messages.
- [x] **[should] Known gaps in release notes** ([#70](https://github.com/CodingDiego/rexplorer/issues/70)) kept current in every release's notes.

## 13. Exit criteria for 1.0

- [ ] Every **[blocker]** above is done.
- [ ] Every **[should]** is done or deferred with a written reason.
- [ ] A public beta (1.0.0-rc.N) has been used by at least five people other than the author as their default
  file manager for four weeks.
- [ ] No open crash, hang or data-loss bug, and no data-loss bug reported during the beta.
- [ ] Performance budgets green on the release candidate; soak test green.

*Status 2026-11-13:* none of the exit criteria is met yet. The blockers that remain are in the table at the top.
1.0.0-rc.1 is the first release candidate, so the public beta can start with it. A data-loss bug (rename) was found
in 0.2.0 on 2026-11-12 and is fixed in 1.0.0-rc.1; the soak was not run for it.

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
