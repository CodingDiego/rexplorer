# rexplorer 1.x: requirements for a stable release

rexplorer is at 0.1.x: fast, and in daily use as the default file manager, but tested on one Windows 11 machine.
**1.0 is a promise that anyone can make rexplorer their default file manager on any supported Windows PC without
losing data, getting locked out of a feature they rely on, or having their settings broken by an upgrade.** This
file lists what that promise needs. Every item has a "done when" and its own issue; all of them are in the
[v1.0 milestone](https://github.com/CodingDiego/rexplorer/milestone/1) and tracked in
[#71](https://github.com/CodingDiego/rexplorer/issues/71).

Legend: **[blocker]** must be done for 1.0 · **[should]** expected for 1.0 unless explicitly deferred with a reason in
the release notes · **[later]** fine for 1.x.

## 1. Legal and branding

- [ ] **[blocker] Own app icon.** ([#18](https://github.com/CodingDiego/rexplorer/issues/18)) `crates/app/app.ico` is Windows Explorer's icon, copied from `explorer.exe` by
  `scripts/extract-icon.py`. It is Microsoft's artwork, and the README says it is for personal use, yet it is in the public repo and
  in every release binary.
  *Done when:* rexplorer ships its own icon, `app.ico` is replaced in history-forward commits, the extraction script
  is removed or local-only, and the README no longer says the window looks like Explorer's because of it.
- [ ] **[blocker] Complete third-party notices.** ([#19](https://github.com/CodingDiego/rexplorer/issues/19)) *Done when:* `THIRD-PARTY-NOTICES.md` covers every bundled crate
  license (generated, for example with `cargo about`) and every asset in the binary.
- [ ] **[should] Trademark hygiene.** ([#20](https://github.com/CodingDiego/rexplorer/issues/20)) *Done when:* no UI text, docs, or store listing suggests rexplorer is Windows
  Explorer or a Microsoft product.

## 2. Accessibility

The custom-drawn controls have no accessibility layer: no `WM_GETOBJECT` or UI Automation provider exists in
`crates/app` for the tab strip, the breadcrumb, the toolbar and command bar, the sidebar, Home or the preview pane.
The README also lists the sidebar as mouse-only.

- [ ] **[blocker] UI Automation for every custom control.** ([#21](https://github.com/CodingDiego/rexplorer/issues/21)) *Done when:* each one exposes name, role, state and
  invoke/selection patterns, and Narrator can read and operate tabs, crumbs, every toolbar button, every sidebar row,
  Home tiles and rows, and the preview caption.
- [ ] **[blocker] Full keyboard reach.** ([#22](https://github.com/CodingDiego/rexplorer/issues/22)) *Done when:* every control is reachable with Tab/F6 and operable without a
  mouse, the sidebar included. The focus is always visible, and the README's keyboard section covers each control.
- [ ] **[blocker] High contrast.** ([#23](https://github.com/CodingDiego/rexplorer/issues/23)) *Done when:* with a Windows contrast theme on, rexplorer uses system colors
  instead of its themes, and nothing is invisible.
- [ ] **[should] Text scaling.** ([#24](https://github.com/CodingDiego/rexplorer/issues/24)) *Done when:* Windows "Text size" (Accessibility > Text size) up to 225% gives no clipped or
  overlapping text.
- [ ] **[should] An accessibility check in the smoke test.** ([#25](https://github.com/CodingDiego/rexplorer/issues/25)) *Done when:* an automated pass walks the UIA tree and fails on unnamed
  interactive elements.

## 3. Data safety and reliability

- [ ] **[blocker] No data-loss paths.** ([#26](https://github.com/CodingDiego/rexplorer/issues/26)) Delete, move, rename, paste with conflicts, drag and drop, archive extraction,
  and New > item all go through the shell, as today, and have tests. *Done when:* each operation has an automated
  check that the result matches Explorer's on the same input, including conflicts and cancel.
- [ ] **[blocker] Undo and redo.** ([#27](https://github.com/CodingDiego/rexplorer/issues/27)) Explorer users rely on Ctrl+Z. *Done when:* Ctrl+Z undoes the last copy, move, rename,
  delete-to-Recycle-Bin and new item through the shell's undo (or an equivalent), and Ctrl+Y redoes.
- [ ] **[blocker] Panic audit.** ([#28](https://github.com/CodingDiego/rexplorer/issues/28)) *Done when:* no `unwrap`/`expect` remains on any shell, filesystem, registry or
  window result outside tests, and every fallible path degrades: a beep, a message, or the embedded Explorer view.
- [ ] **[blocker] Soak test.** ([#29](https://github.com/CodingDiego/rexplorer/issues/29)) *Done when:* an 8-hour scripted session (navigate, switch tabs, search, preview,
  thumbnails, file operations) ends with GDI objects, USER objects, handles, threads and private bytes flat within
  a stated margin. The test runs before every release.
- [ ] **[should] Crash reports users can send.** ([#30](https://github.com/CodingDiego/rexplorer/issues/30)) Crash recovery exists (`--recover`, a second crash opens Explorer).
  *Done when:* a crash also writes a minidump and a short text report under `%LOCALAPPDATA%\rexplorer\crashes`, and
  `SECURITY.md`/README say how to send them (no telemetry).
- [ ] **[should] Hang watchdog.** ([#31](https://github.com/CodingDiego/rexplorer/issues/31)) *Done when:* a UI thread blocked for more than N seconds is detected and logged
  with the operation it was running.

## 4. Compatibility

- [ ] **[blocker] Supported Windows versions stated and tested.** ([#32](https://github.com/CodingDiego/rexplorer/issues/32)) *Done when:* the README names the minimum version,
  and every release is smoke-tested on at least Windows 10 22H2 and the two newest Windows 11 releases. Today only
  Windows 11 10.0.26200 is tested. Home already falls back to This PC where it does not exist.
- [ ] **[blocker] Display scaling and multi-monitor.** ([#33](https://github.com/CodingDiego/rexplorer/issues/33)) *Done when:* everything is checked at 100/125/150/200/250% and on mixed-DPI multi-monitor
  setups (drag between monitors), in light and dark mode.
- [ ] **[blocker] Every place Explorer users browse.** ([#34](https://github.com/CodingDiego/rexplorer/issues/34)) *Done when:* each of these is verified:
  - mapped drives and UNC shares, both reachable and unreachable (no hang);
  - OneDrive placeholders (online-only, always-keep, sync errors);
  - MTP phones, WSL (`\\wsl$`), Linux shares, BitLocker-locked drives, removable media and the Recycle Bin;
  - long paths over 260 characters.
- [ ] **[should] ARM64 build.** ([#35](https://github.com/CodingDiego/rexplorer/issues/35)) *Done when:* releases include a native arm64 zip that passes the smoke test on ARM
  hardware.
- [ ] **[should] FTP verified or dropped.** ([#36](https://github.com/CodingDiego/rexplorer/issues/36)) The README calls it untested against a live server. *Done when:* it is tested against one, or removed
  from the docs.

## 5. Localization

rexplorer's own strings are English-only and in the code. Sizes, dates and item names already come from the shell
in the user's language, so a Spanish Windows shows "Nueva carpeta" next to English menus.

- [ ] **[blocker] Externalized strings.** ([#37](https://github.com/CodingDiego/rexplorer/issues/37)) *Done when:* every user-visible string comes from a resource table, and a
  missing translation falls back to English.
- [ ] **[should] First translations.** ([#38](https://github.com/CodingDiego/rexplorer/issues/38)) *Done when:* Spanish ships in addition to English, and the process to add a
  language is documented in CONTRIBUTING.md.
- [ ] **[should] Right-to-left layout.** ([#39](https://github.com/CodingDiego/rexplorer/issues/39)) *Done when:* the window mirrors correctly for Arabic and Hebrew UI languages.

## 6. Explorer parity for daily use

Each gap below is either closed, or explicitly accepted for 1.0 and listed under "Known gaps" in the release notes.

- [ ] **[blocker] Desktop menu completeness** ([#40](https://github.com/CodingDiego/rexplorer/issues/40)). The desktop menu shipped in 0.1.3
  (`rexplorer --desktop`). *Done when:* right-dragging desktop icons works as in Explorer, and Shift+F10 and the Menu
  key on the desktop open rexplorer's menu.
- [ ] **[blocker] Indexed search and full search results.** ([#41](https://github.com/CodingDiego/rexplorer/issues/41)) *Done when:* unelevated searches use the Windows Search index where it covers the folder,
  with the walker as fallback (today a drive-wide fast search needs `--elevated` for the USN index). Results also get
  real icons, thumbnails, preview, drag, sort, rename and refresh after delete.
- [ ] **[should] Search results context menu and Back** ([#42](https://github.com/CodingDiego/rexplorer/issues/42)) across results from different folders, and Back returns to the results.
- [ ] **[should] Archive view.** ([#43](https://github.com/CodingDiego/rexplorer/issues/43)) *Done when:* archive members get icons, thumbnails and preview in all views;
  multi-volume and encrypted RAR are tested; zip either goes through the same view or the difference is documented.
- [ ] **[should] Themed embedded Explorer view** ([#44](https://github.com/CodingDiego/rexplorer/issues/44)) follows the theme, or the difference is documented.
- [ ] **[should] Home page completeness.** ([#45](https://github.com/CodingDiego/rexplorer/issues/45)) *Done when:* the location-status column exists, sort works, and the undocumented
  `System.Home.Grouping` value is guarded (a missing value degrades quietly).
- [ ] **[should] Send to, Properties on multiple selections, file conflict dialogs and progress** ([#46](https://github.com/CodingDiego/rexplorer/issues/46)) behave exactly as in
  Explorer (all from the shell; verified, not reimplemented).
- [ ] **[later] Tab drag and drop.** ([#47](https://github.com/CodingDiego/rexplorer/issues/47)) Drag a tab to a new window, and drag files onto a tab.

## 7. Performance budgets (enforced, not just measured)

Measured on 2026-10-11 with the 0.1.4 work, on a 12-core laptop:
- Home ink about 0.4 s from process start; a 100k-file folder listed in about 0.4 s.
- Idle while focused: about 0.55% of one core. Minimized under file churn: about 0.
- Terminal launch: 0.4 ms on the UI thread.

- [ ] **[blocker] Budgets in CI.** ([#48](https://github.com/CodingDiego/rexplorer/issues/48)) *Done when:* `scripts/bench-vs-explorer.ps1` (or a headless variant) runs on every
  PR and fails the build when a budget is exceeded. Budgets: cold start to first ink ≤ 600 ms, 100k-file folder
  ≤ 600 ms, idle CPU focused ≤ 1% of a core, churn reloads ≤ 1/s, no UI-thread call over 100 ms in the scripted
  session.
- [ ] **[should] UI-thread budget tooling.** ([#49](https://github.com/CodingDiego/rexplorer/issues/49)) *Done when:* a debug build logs every UI-thread message that takes over 50 ms,
  with its source.
- [ ] **[should] Backlog from issue #17** ([#17](https://github.com/CodingDiego/rexplorer/issues/17)) is closed or explicitly accepted.

## 8. Stability contract (semantic versioning)

From 1.0, breaking any of these needs 2.0:

- [ ] **[blocker] Command line frozen.** ([#50](https://github.com/CodingDiego/rexplorer/issues/50)) `--select` (and `/select,`), `--profile`, `--new-tab`/`-t`, `--search`,
  `--register`, `--unregister`, `--install`, `--uninstall`, `--import-explorer`, `--overwrite`, `--recover`,
  `--elevated`, `-h`/`--help`, and the Explorer argument forms rexplorer accepts or forwards. *Done when:* they are
  documented as stable, with a test per flag (`cli.rs` already checks that usage lists all of them).
- [ ] **[blocker] Settings format versioned.** ([#51](https://github.com/CodingDiego/rexplorer/issues/51)) `settings.ini`, `themes\*.ini`, `icons\<name>\`, `[keys]` overrides
  and `session.txt`. *Done when:* the files carry a format version, every older format is migrated on load (tested
  with fixtures from each 0.x release), and unknown keys are preserved, not dropped.
- [ ] **[blocker] Cross-version hand-off.** ([#52](https://github.com/CodingDiego/rexplorer/issues/52)) *Done when:* the `WM_COPYDATA` payload between a running instance and a new
  launch is versioned, so an upgraded exe never mis-hands off to an older running one (or the reverse).
- [ ] **[should] Deprecation policy** ([#53](https://github.com/CodingDiego/rexplorer/issues/53)) written in the README: how long a deprecated key or flag keeps working.

## 9. Testing and engineering

- [ ] **[blocker] Smoke test in CI.** ([#54](https://github.com/CodingDiego/rexplorer/issues/54)) Today `scripts/smoke.ps1` runs locally, and its keyboard, share and dnd checks
  SKIP without an interactive foreground. *Done when:* it runs in CI on a runner with an interactive desktop (or
  those checks are rewritten to drive the app through messages), with zero SKIPs on the release run.
- [ ] **[blocker] Data-operation tests** ([#55](https://github.com/CodingDiego/rexplorer/issues/55)) from section 3, in CI.
- [ ] **[should] Clippy clean** ([#56](https://github.com/CodingDiego/rexplorer/issues/56)) with `-D warnings` in CI, plus a stated minimum supported Rust version, checked in CI.
- [ ] **[should] Fuzzing** ([#57](https://github.com/CodingDiego/rexplorer/issues/57)) of path handling, archive path splitting and `settings.ini`/theme parsing.
- [ ] **[should] Reproducible releases.** ([#58](https://github.com/CodingDiego/rexplorer/issues/58)) Release binaries are built only by CI from the tag, as today, and the build
  embeds no local paths.

## 10. Security

The whole project was reviewed on 2026-10-10 and every finding was fixed. These keep it that way:

- [ ] **[blocker] Code-signed releases.** ([#59](https://github.com/CodingDiego/rexplorer/issues/59)) Today SmartScreen warns on first run. *Done when:* the exe and the zip are
  signed by CI (signing key in a protected environment) and the signature is checked in the release job.
- [ ] **[blocker] Pre-1.0 security review** ([#60](https://github.com/CodingDiego/rexplorer/issues/60)) repeated on the 1.0 candidate, covering everything added since
  2026-10-10, the Desktop helper (a low-level mouse hook) above all.
- [ ] **[should] Dependency hygiene.** ([#61](https://github.com/CodingDiego/rexplorer/issues/61)) `cargo audit` runs weekly (exists). *Done when:* the pinned `windows` crate
  and `unrar` updates follow a written cadence, and Dependabot PRs are reviewed within a stated time.
- [ ] **[should] Advisory response time.** ([#62](https://github.com/CodingDiego/rexplorer/issues/62)) `SECURITY.md` states a response time for private advisories.

## 11. Distribution and updates

- [ ] **[blocker] winget package.** ([#63](https://github.com/CodingDiego/rexplorer/issues/63)) *Done when:* `winget install rexplorer` works, and `winget upgrade` picks up
  releases.
- [ ] **[blocker] Upgrade path.** ([#64](https://github.com/CodingDiego/rexplorer/issues/64)) *Done when:* installing a new version over a running one is tested: the old copy is
  moved aside, the next launch runs the new one, settings carry over, and nothing is left elevated.
- [ ] **[blocker] Uninstall restores Explorer completely.** ([#65](https://github.com/CodingDiego/rexplorer/issues/65)) *Done when:* every registry key, App Paths entry, Start
  menu shortcut and Startup entry is removed. It is tested on a clean VM.
- [ ] **[should] In-app update notice.** ([#66](https://github.com/CodingDiego/rexplorer/issues/66)) *Done when:* there is an opt-in check of the GitHub releases feed, with no
  telemetry.
- [ ] **[should] Full changelog.** ([#67](https://github.com/CodingDiego/rexplorer/issues/67)) *Done when:* `CHANGELOG.md` keeps every release's notes, not only the latest in
  `.github/release-notes.md`.

## 12. Documentation

- [ ] **[blocker] User guide.** ([#68](https://github.com/CodingDiego/rexplorer/issues/68)) *Done when:* it covers install, make default, revert to Explorer, keys, settings
  reference, themes and icon themes, and troubleshooting ("a folder hangs", "go back to Explorer now").
- [ ] **[should] Architecture overview** ([#69](https://github.com/CodingDiego/rexplorer/issues/69)) for contributors: the core/app split, threads and apartments, and the
  timers and messages.
- [ ] **[should] Known gaps in release notes** ([#70](https://github.com/CodingDiego/rexplorer/issues/70)) kept current in every release's notes.

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
