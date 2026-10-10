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

`scripts\smoke.ps1` drives the real `target\release\rexplorer.exe` (about five minutes, `-Only <regex>` for a
subset). It runs every instance on a scratch `%APPDATA%`, so it does not touch your settings.

## Rules for changes

- Never run `--install`, `--uninstall`, `--register` or `--import-explorer` against your real profile while
  testing; set `REXPLORER_INSTALL_DIR` and `REXPLORER_REG_BASE=Software\rexplorer-test\<tag>` and a scratch
  `APPDATA` in the environment.
- Keep `cargo build --release` free of warnings and `cargo test --release` green.
- Match the surrounding code: small diffs, comments only for a why the code cannot show.
- The `windows` crate is pinned on purpose (`Cargo.toml`); upgrade it in its own change.

By contributing you agree that your contribution is licensed under the MIT license (`LICENSE`).
