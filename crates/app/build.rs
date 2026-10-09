// Common Controls 6: LVM_SETVIEW (icon views), visual styles and SetWindowTheme need it; without a manifest
// the process gets comctl32 5.82.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
             version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
