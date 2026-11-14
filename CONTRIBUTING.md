# Contributing

Issues and pull requests are welcome. Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Build and test

Windows 10 or 11, a stable Rust toolchain (MSVC target), and the Windows SDK if you want the app icon embedded.

```
cargo build --release
cargo test --release
```

The core crate's tests run against the real shell. Tests that touch the registry use a throwaway key under
`HKCU\Software\rexplorer-test-<pid>-...` and remove it; keep new tests that way.

`scripts\smoke.ps1` drives the real `target\release\rexplorer.exe` (about a minute, three the first time while it
makes its 100,000-file folder; `-Only <regex>` for a subset). It runs every instance on a scratch `%APPDATA%` and
`%LOCALAPPDATA%`, so it does not touch your settings. CI runs it with `-Hosted` (see the README), then
`scripts\budgets.ps1` with hosted values.

CI also builds with the minimum supported Rust (`rust-version` in `Cargo.toml`, the `msrv` job), so do not use an
API newer than that version; raise `rust-version` on purpose if you must. Code outside tests may not call
`unwrap()` or `expect()` (a crate-level clippy deny in both crates): handle the error, or degrade with a beep or a
message.

Fuzzing: [fuzz/README.md](fuzz/README.md) has the targets and commands (nightly and `cargo-fuzz`). The weekly
"Fuzz and soak" workflow runs every target with `--sanitizer none`; reproduce a crash it uploads with the same
flag. A crash becomes a test in `crates/core/src/fuzz.rs` (or the session test) before the parser is fixed.

## Rules for changes

- Never run `--install`, `--uninstall`, `--register`, `--unregister`, `--import-explorer` or the install scripts
  (`install.ps1`, `master-install.ps1`, `restore-explorer.ps1`) against your real profile while testing; set
  `REXPLORER_INSTALL_DIR`, `REXPLORER_REG_BASE=Software\rexplorer-test\<tag>`, `REXPLORER_PROGRAMS_DIR`,
  `REXPLORER_STARTUP_DIR` and a scratch `APPDATA` / `LOCALAPPDATA` in the environment (the scripts take the same as
  `-InstallDir`, `-RegBase`, `-ProgramsDir`, `-StartupDir`). Never pass `-RestartShell` while testing.
- Installing (`--install`, `install.rs` `install`) and taking over Explorer (`--register`, `take_over`) stay
  separate. A key rexplorer can write is added to `known_keys()` in `install.rs` and to `$KnownKeys` in
  `scripts\restore-explorer.ps1` together; `install::tests::restore_script_knows_every_key` checks it.
- Keep `cargo build --release` free of warnings and `cargo test --release` green.
- Match the surrounding code: small diffs, comments only for a why the code cannot show.
- The `windows` crate is pinned on purpose (`Cargo.toml`); upgrade it in its own change.
- Every user-visible change adds a line under Unreleased in [CHANGELOG.md](CHANGELOG.md).

## Translations

English and Spanish are built in. Text lives in `crates/core/src/lang/<code>.ini`, one `key=text` per line.

1. Copy `crates/core/src/lang/en.ini` to `<code>.ini` (`fr.ini`, `de.ini`...). Translate the text after `=`; never
   change a key. A key you leave out falls back to English.
2. Keep every `{placeholder}` (`{n}`, `{a}`...) exactly as written; you may move it within the sentence. Keep the
   `\n` escapes and the `&` menu accelerators (two `&` in the same menu must not share a letter).
3. Counts use two keys, `<name>.one` (exactly 1) and `<name>.many` (every other number, including 0). Translate
   both; if your language needs only one form, write the same text in both.
4. Try it without building: put the file in `%APPDATA%\rexplorer\lang\<code>.ini`, set `language=<code>` under
   `[general]` in `settings.ini`, and restart. Run it on a scratch `APPDATA` (see Rules for changes).
5. To ship it built in, add `pub const FR: &str = include_str!("lang/fr.ini");` and an entry in `BUILTIN` in
   `crates/core/src/i18n.rs`, then run `cargo test -p rexplorer-core --lib i18n`. The tests check that every key
   in the code exists in English and that menu accelerators are unique.

## Dependencies

- `cargo audit` runs weekly in CI and on every pull request. Triage a failure within 7 days: update, or record
  in the issue why it does not apply.
- Dependabot opens pull requests weekly for Cargo and GitHub Actions. Review each within 7 days.
- The `windows` family (`windows`, `windows-core`, `windows-collections`) is pinned and ignored by Dependabot.
  Review a new minor every quarter, and at once on a security advisory. Upgrade in its own pull request.
- `unrar-ng` (it bundles libunrar; `crates/core/Cargo.toml`) is checked against RARLAB's releases every month. Bump
  it within 14 days of a release that fixes a security issue, and sooner for a published CVE. It is built from
  `vendor/unrar-ng` (0.7.7 with one fix) through `[patch.crates-io]` in both `Cargo.toml` and `fuzz/Cargo.toml`:
  after a bump Cargo only warns that the patch is unused and builds the published code again, so follow
  `vendor/unrar-ng/VENDORED.md` (drop the folder if the new release has the fix, else carry the fix over).

## Releasing

Before tagging:

1. Run `scripts\smoke.ps1 -NoSkip -UserState` on Windows 10 22H2 and on Windows 11, on a desktop nobody is using
   (a test account for `-UserState`). CI runs only `-Hosted`, which skips the checks that need such a desktop.
2. Run `scripts\soak.ps1 -Minutes 60` on the release build on an idle machine (or the "Fuzz and soak" workflow by
   hand); it must PASS every resource and reach its stated 300 rounds.
3. Review the Known gaps in the README against `docs/EXPLORER-PARITY.md` and copy the current list into
   `.github/release-notes.md` under the "Known gaps" heading. Remove the word "Draft" from its first line: the
   release job refuses to publish while it is there.
4. Move the Unreleased lines of `CHANGELOG.md` under the new version and date, then add the release notes
   (with their Known gaps) to it.
5. Bump the version, tag it, and check that the release workflow's zip and `SHA256SUMS.txt` match.
6. Copy `manifests/c/CodingDiego/rexplorer/<old>/` to the new version, put in the new URLs and the zips' SHA-256 from
   `SHA256SUMS.txt`, run `winget validate`, and submit it to microsoft/winget-pkgs. Skip this for a pre-release
   (a version with a `-`, such as 1.0.0-rc.1): the release job never marks it latest, and winget and the update
   notice follow the latest release.
7. Check that nothing deprecated was removed earlier than the README's Stability and deprecation section allows.

By contributing you agree that your contribution is licensed under the MIT license (`LICENSE`).
