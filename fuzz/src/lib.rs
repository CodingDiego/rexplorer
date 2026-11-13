//! Shared by every fuzz target: `fuzz_target!(init: rexplorer_fuzz::init!(), |data: &[u8]| ...)`.
//!
//! `init!` makes a panic leave a reproducer on Windows. libfuzzer-sys's panic hook ends in `std::process::abort`,
//! which on Windows is `__fastfail` (exit 0xC0000409): no handler runs, so libFuzzer never writes
//! `artifacts/<target>/crash-<hash>`. The C runtime's `abort` raises SIGABRT, which libFuzzer handles
//! (FuzzerUtilWindows.cpp, `-handle_abrt=1`, the default) by saving the input first.
//!
//! On x86_64-pc-windows-msvc the SanitizerCoverage pass refers to `__start___sancov_cntrs`/`__stop___sancov_cntrs`
//! and `__start___sancov_pcs`/`__stop___sancov_pcs`. ELF linkers make those up; on COFF, LLVM expects them from
//! compiler-rt's `sanitizer_coverage_win_sections.cpp`, which only the ASan runtime brings. libfuzzer-sys builds
//! libFuzzer alone, so a `--sanitizer none` build fails with LNK2001 / LNK1120. These are the same delimiters as that
//! file: the MSVC linker sorts `.SCOV$CA` < `.SCOV$CM` (LLVM's counters) < `.SCOV$CZ`, and LLVM skips the 8-byte start
//! value on Windows. Do not combine with `--sanitizer address` on MSVC (the runtime would define them twice).
#![allow(non_upper_case_globals)]

/// Replaces libfuzzer-sys's panic hook (runs after it is set) with one that prints the panic and aborts through the
/// C runtime, so libFuzzer saves the crashing input on Windows too (elsewhere the C `abort` is what Rust's does).
/// A macro, not a function: a function here would be instrumented, and the coverage pass's own references to the
/// delimiters below would then clash with them in this crate (LNK2019 `__start___sancov_cntrs.13`).
#[macro_export]
macro_rules! init {
    () => {
        std::panic::set_hook({
            unsafe extern "C" {
                fn abort() -> !;
            }
            drop(std::panic::take_hook()); // libfuzzer-sys's; the default one is back in place
            let default = std::panic::take_hook();
            Box::new(move |info| {
                default(info);
                unsafe { abort() }
            })
        })
    };
}

#[cfg(all(windows, target_env = "msvc"))]
mod sancov_sections {
    // Counters are written at run time, so their delimiters sit in a writable section like LLVM's own `.SCOV$CM`.
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOV$CA")]
    static mut __start___sancov_cntrs: u64 = 0;
    // One byte, so the linker adds no alignment padding before it (the counters are one byte each).
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOV$CZ")]
    static mut __stop___sancov_cntrs: u8 = 0;

    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOVP$A")]
    static __start___sancov_pcs: u64 = 0;
    #[used]
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".SCOVP$Z")]
    static __stop___sancov_pcs: u8 = 0;
}
