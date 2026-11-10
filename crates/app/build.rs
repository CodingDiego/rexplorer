// Common Controls 6: LVM_SETVIEW (icon views), visual styles and SetWindowTheme need it; without a manifest
// the process gets comctl32 5.82. app.manifest adds supportedOS and longPathAware (paths over 260 characters
// where the LongPathsEnabled policy is on).
fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // The application icon and version info (app.rc -> app.res) via the newest Windows SDK rc.exe; skipped if
        // the SDK is absent.
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.res");
        let kits = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
        let mut rcs: Vec<_> = std::fs::read_dir(kits)
            .map(|d| d.flatten().map(|e| e.path().join("x64").join("rc.exe")).filter(|p| p.exists()).collect())
            .unwrap_or_default();
        rcs.sort();
        let ver = std::env::var("CARGO_PKG_VERSION").unwrap();
        let num: Vec<&str> = ver.split(['.', '-', '+']).take(3).collect();
        let defines = [format!("/dREX_VERSION={},0", num.join(",")), format!("/dREX_VERSION_STR=\"{ver}\"")];
        match rcs.last().map(|rc| std::process::Command::new(rc).arg("/nologo").args(&defines).arg("/fo").arg(&out).arg("app.rc").status()) {
            Some(Ok(s)) if s.success() => println!("cargo:rustc-link-arg-bins={}", out.display()),
            other => println!("cargo:warning=app icon not embedded: rc.exe {other:?}"),
        }
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
             version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
