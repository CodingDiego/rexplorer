//! Make rexplorer the handler for opening folders, via per-user overrides only.
//!
//! `register` writes, for each of `Folder`, `Directory` and `Drive`, under
//! `HKCU\<base>\<class>\shell\open\command`: the default value `"<exe>" "%1"` and an
//! empty `DelegateExecute` string (which stops the shell from using Explorer's
//! DelegateExecute handler). HKLM is never touched. `unregister` deletes
//! `HKCU\<base>\<class>\shell\open` for each class.
//!
//! Win+E and the taskbar icon still open Explorer: they do not go through these verbs.
use std::path::Path;

use windows::Win32::Foundation::{E_FAIL, ERROR_FILE_NOT_FOUND};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

/// Shell classes whose `open` verb is overridden.
pub const CLASSES: [&str; 3] = ["Folder", "Directory", "Drive"];
/// The real per-user classes key, relative to HKCU.
pub const REAL_BASE: &str = r"Software\Classes";

fn command_key(base: &str, class: &str) -> HSTRING {
    HSTRING::from(format!(r"{base}\{class}\shell\open\command"))
}

/// Default value of `HKCU\key`, or None if the key or value is missing.
fn read_default(key: &HSTRING) -> Option<String> {
    let mut cb = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, PCWSTR::null(), RRF_RT_REG_SZ, None, None, Some(&mut cb))
    }
    .ok()
    .ok()?;
    let mut buf = vec![0u16; cb as usize / 2 + 1];
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut cb),
        )
    }
    .ok()
    .ok()?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// REG_SZ write; cbdata counts bytes including the terminating NUL.
fn set_sz(key: &HSTRING, name: PCWSTR, value: &str) -> Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    }
    .ok()
}

fn notify() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// Point the folder open verbs at `exe`. Refuses (writing nothing) if another program's
/// override already exists.
pub fn register(base: &str, exe: &Path) -> Result<()> {
    for class in CLASSES {
        let key = command_key(base, class);
        if let Some(cur) = read_default(&key) {
            if !cur.to_lowercase().contains("rexplorer") {
                return Err(Error::new(E_FAIL, format!("another override exists: {key}")));
            }
        }
    }
    let cmd = format!("\"{}\" \"%1\"", exe.display());
    for class in CLASSES {
        let key = command_key(base, class);
        set_sz(&key, PCWSTR::null(), &cmd)?;
        set_sz(&key, w!("DelegateExecute"), "")?;
    }
    notify();
    Ok(())
}

/// Remove the overrides written by `register`.
pub fn unregister(base: &str) -> Result<()> {
    for class in CLASSES {
        let key = HSTRING::from(format!(r"{base}\{class}\shell\open"));
        let err = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &key) };
        if err != ERROR_FILE_NOT_FOUND {
            err.ok()?;
        }
    }
    notify();
    Ok(())
}

/// True if the Folder open command currently launches `exe`.
pub fn is_registered(base: &str, exe: &Path) -> bool {
    read_default(&command_key(base, "Folder"))
        .is_some_and(|v| v.to_lowercase().contains(&exe.display().to_string().to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_base(tag: &str) -> (String, HSTRING) {
        let root = format!(r"Software\rexplorer-test-{}-{tag}", std::process::id());
        (format!(r"{root}\Classes"), HSTRING::from(root))
    }

    fn cleanup(root: &HSTRING) {
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, root) };
    }

    #[test]
    fn register_and_unregister_under_test_base() {
        let (base, root) = test_base("a");
        let exe = Path::new(r"C:\x\rexplorer.exe");
        register(&base, exe).unwrap();
        assert!(is_registered(&base, exe));
        assert_eq!(
            read_default(&command_key(&base, "Directory")).as_deref(),
            Some(r#""C:\x\rexplorer.exe" "%1""#)
        );
        unregister(&base).unwrap();
        assert!(!is_registered(&base, exe));
        cleanup(&root);
    }

    #[test]
    fn register_refuses_foreign_override() {
        let (base, root) = test_base("b");
        set_sz(&command_key(&base, "Folder"), PCWSTR::null(), r"C:\other.exe %1").unwrap();
        assert!(register(&base, Path::new(r"C:\x\rexplorer.exe")).is_err());
        assert!(read_default(&command_key(&base, "Directory")).is_none());
        cleanup(&root);
    }
}
