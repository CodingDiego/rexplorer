# Fuzzing the parsers (#57)

One cargo-fuzz target per pure parser. The crate is outside the workspace (own `[workspace]`, lock file ignored), so
the workspace never sees libfuzzer-sys. It needs nightly and `cargo install cargo-fuzz`.

| target | parser |
| --- | --- |
| `ini_file` | settings.ini, theme files, `#RRGGBB` colors |
| `language_file` | `lang\*.ini` and placeholder substitution |
| `session_files` | session.txt and folders.txt (`crates/app/src/session.rs` compiled in by path) |
| `archive_member` | archive entry name checks |
| `command_line` | the command line and the hand-off payload (fields split on U+001F) |
| `search_query` | the search box syntax (`crates/core/src/query.rs`) |
| `update_response` | the update check's JSON body and version strings |

Seed corpora are in `corpus/<target>/`, built from the fixtures and tests in the repo. Runs read them and write
new finds to the ignored `work/<target>/` (libFuzzer writes to the first corpus folder), so the tracked seeds stay
as they are; a crash goes to `fuzz/artifacts/<target>/`.

From the repo root, in PowerShell:

```powershell
cargo +nightly fuzz list --fuzz-dir fuzz
# one target, two minutes
New-Item -ItemType Directory -Force fuzz/work/ini_file | Out-Null
cargo +nightly fuzz run ini_file fuzz/work/ini_file fuzz/corpus/ini_file --fuzz-dir fuzz -- -max_total_time=120
# every target, unattended, ten minutes each
foreach ($t in 'ini_file','language_file','session_files','archive_member','command_line','search_query','update_response') {
    New-Item -ItemType Directory -Force fuzz/work/$t | Out-Null
    cargo +nightly fuzz run $t fuzz/work/$t fuzz/corpus/$t --fuzz-dir fuzz -- -max_total_time=600 -max_len=65536
}
# reproduce a crash
cargo +nightly fuzz run ini_file --fuzz-dir fuzz fuzz/artifacts/ini_file/crash-<hash>
```

A found crash becomes a test: add its input to the hostile list in `crates/core/src/fuzz.rs` (or the session test in
`crates/app/src/session.rs`) and fix the parser. The seeded and hostile-input tests run in the normal test suite.
