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

On Windows (x86_64-pc-windows-msvc) pass `--sanitizer none` to every `build` and `run`. cargo-fuzz defaults to
ASan, which needs Visual Studio's "C++ AddressSanitizer" component and its DLL on PATH; the targets are safe Rust, so
the finds are panics either way. Without ASan nothing on MSVC defines the coverage section bounds the
SanitizerCoverage pass refers to (`__start___sancov_cntrs` and friends: LNK2001, LNK1120), so `src/lib.rs` defines
them the way compiler-rt's `sanitizer_coverage_win_sections.cpp` does. Every target links it through
`fuzz_target!(init: rexplorer_fuzz::init!(), |data: &[u8]| ...)`, and a new target needs that too: `init!` also
makes a panic end in the C runtime's `abort` (SIGABRT, which libFuzzer catches and so saves the input) instead of
Rust's `process::abort`, which on Windows is `__fastfail` (exit 0xC0000409, no `crash-<hash>` file written). Do not
combine it with `--sanitizer address` on MSVC.

In Windows PowerShell 5.1, write libFuzzer's options as literals (`-max_total_time=120`): a variable inside such a
token (`-max_total_time=$s`) is passed unexpanded, and libFuzzer then runs with no time limit.

From the repo root, in PowerShell:

```powershell
cargo +nightly fuzz list --fuzz-dir fuzz
# one target, two minutes
New-Item -ItemType Directory -Force fuzz/work/ini_file | Out-Null
cargo +nightly fuzz run ini_file fuzz/work/ini_file fuzz/corpus/ini_file --fuzz-dir fuzz --sanitizer none -- -max_total_time=120
# every target, unattended, ten minutes each
foreach ($t in 'ini_file','language_file','session_files','archive_member','command_line','search_query','update_response') {
    New-Item -ItemType Directory -Force fuzz/work/$t | Out-Null
    cargo +nightly fuzz run $t fuzz/work/$t fuzz/corpus/$t --fuzz-dir fuzz --sanitizer none -- -max_total_time=600 -max_len=65536
}
# reproduce a crash
cargo +nightly fuzz run ini_file --fuzz-dir fuzz --sanitizer none fuzz/artifacts/ini_file/crash-<hash>
```

A found crash becomes a test: add its input to the hostile list in `crates/core/src/fuzz.rs` (or the session test in
`crates/app/src/session.rs`) and fix the parser. The seeded and hostile-input tests run in the normal test suite.
