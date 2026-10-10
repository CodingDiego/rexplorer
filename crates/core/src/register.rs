//! Make rexplorer the handler for opening folders, via per-user overrides only.
//!
//! `register` writes five command keys: for each of `Folder`, `Directory` and `Drive`,
//! `HKCU\<base>\<class>\shell\open\command` = `"<exe>" "%1"`, and for the File Explorer CLSID (Win+E, the taskbar
//! icon, Start's File Explorer) `HKCU\<base>\CLSID\{52205fd8-...}\shell\{open,opennewwindow}\command` = `"<exe>"`
//! (those verbs carry no argument). Each also gets an empty `DelegateExecute` string, which stops the shell from
//! using Explorer's DelegateExecute handler. HKLM is never touched. `unregister` deletes the command keys that
//! still launch rexplorer, and the keys left empty above them up to `<base>`.
use std::path::Path;

use windows::Win32::Foundation::{E_FAIL, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW, RegGetValueW,
    RegOpenKeyExW, RegQueryInfoKeyW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, SHDeleteEmptyKeyW};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

/// Shell classes whose `open` verb is overridden.
pub const CLASSES: [&str; 3] = ["Folder", "Directory", "Drive"];
/// The real per-user classes key, relative to HKCU.
pub const REAL_BASE: &str = r"Software\Classes";
/// File Explorer: Win+E, the taskbar icon and Start's "File Explorer" open it through these verbs.
pub const EXPLORER_CLSID: &str = "{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";
const CLSID_VERBS: [&str; 2] = ["open", "opennewwindow"];

fn command_key(base: &str, class: &str) -> HSTRING {
    HSTRING::from(format!(r"{base}\{class}\shell\open\command"))
}

/// The five command keys `register` writes, each with whether its command takes the `%1` argument.
pub fn command_keys(base: &str) -> Vec<(HSTRING, bool)> {
    let classes = CLASSES.iter().map(|c| (command_key(base, c), true));
    let verbs =
        CLSID_VERBS.iter().map(|v| (HSTRING::from(format!(r"{base}\CLSID\{EXPLORER_CLSID}\shell\{v}\command")), false));
    classes.chain(verbs).collect()
}

/// True when a command or path value names rexplorer (ours to replace or remove).
pub(crate) fn is_ours(value: &str) -> bool {
    value.to_lowercase().contains("rexplorer")
}

pub(crate) fn key_exists(key: &str) -> bool {
    let mut h = HKEY::default();
    let ok = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(key), None, KEY_READ, &mut h) }.is_ok();
    if ok {
        let _ = unsafe { RegCloseKey(h) };
    }
    ok
}

/// Deletes `key` if it has neither values nor subkeys; false when it is not empty or missing.
pub(crate) fn delete_empty(key: &str) -> bool {
    unsafe { SHDeleteEmptyKeyW(HKEY_CURRENT_USER, &HSTRING::from(key)) }.is_ok()
}

/// True when `key` has any value or subkey.
fn has_content(key: &HSTRING) -> bool {
    let mut h = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key, None, KEY_READ, &mut h) }.is_err() {
        return false;
    }
    let (mut subkeys, mut values) = (0u32, 0u32);
    let ok = unsafe {
        RegQueryInfoKeyW(h, None, None, None, Some(&mut subkeys), None, None, Some(&mut values), None, None, None, None)
    }
    .is_ok();
    let _ = unsafe { RegCloseKey(h) };
    !ok || subkeys + values > 0
}

/// A key that holds anything but a default value naming rexplorer is another program's.
pub(crate) fn is_foreign(key: &HSTRING) -> bool {
    !read_default(key).is_some_and(|v| is_ours(&v)) && has_content(key)
}

/// Deletes the values rexplorer writes (default, `DelegateExecute`, App Paths' `Path`) and then the key if that
/// leaves it empty, so values another program added survive. Ok(false) when the key was left alone as foreign.
pub(crate) fn remove_if_ours(key: &HSTRING) -> Result<bool> {
    if is_foreign(key) {
        return Ok(false);
    }
    for name in [PCWSTR::null(), w!("DelegateExecute"), w!("Path")] {
        let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name) };
        if err != ERROR_FILE_NOT_FOUND && err != ERROR_PATH_NOT_FOUND {
            err.ok()?;
        }
    }
    delete_empty(&key.to_string());
    Ok(true)
}

/// Default value of `HKCU\key`, or None if the key or value is missing.
pub(crate) fn read_default(key: &HSTRING) -> Option<String> {
    read_sz(key, PCWSTR::null())
}

/// The `name` value of `HKCU\key` (null or "" is the default value), or None if the key or value is missing.
pub(crate) fn read_sz(key: &HSTRING, name: PCWSTR) -> Option<String> {
    let mut cb = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key, name, RRF_RT_REG_SZ, None, None, Some(&mut cb)) }.ok().ok()?;
    let mut buf = vec![0u16; cb as usize / 2 + 1];
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
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
pub(crate) fn set_sz(key: &HSTRING, name: PCWSTR, value: &str) -> Result<()> {
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

pub(crate) fn notify() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// The first command key holding another program's override, if any.
pub fn foreign_override(base: &str) -> Option<HSTRING> {
    command_keys(base).into_iter().map(|(k, _)| k).find(is_foreign)
}

/// Point the folder open verbs and File Explorer's verbs at `exe`. Refuses (writing nothing) if another
/// program's override already exists.
pub fn register(base: &str, exe: &Path) -> Result<()> {
    if let Some(key) = foreign_override(base) {
        return Err(Error::new(E_FAIL, format!("another override exists: {key}")));
    }
    for (key, name, value) in writes(base, exe) {
        set_sz(&key, PCWSTR(HSTRING::from(name).as_ptr()), &value)?;
    }
    notify();
    Ok(())
}

/// The values `register` writes for `exe`: (key, value name with "" for the default value, data).
pub(crate) fn writes(base: &str, exe: &Path) -> Vec<(HSTRING, &'static str, String)> {
    let mut v = vec![];
    for (key, arg) in command_keys(base) {
        let cmd = if arg { format!("\"{}\" \"%1\"", exe.display()) } else { format!("\"{}\"", exe.display()) };
        v.push((key.clone(), "", cmd));
        v.push((key, "DelegateExecute", String::new()));
    }
    v
}

/// Remove the overrides written by `register`, leaving another program's in place, and the keys left empty
/// above them (never `base` itself).
pub fn unregister(base: &str) -> Result<()> {
    for (key, _) in command_keys(base) {
        if remove_if_ours(&key)? {
            let mut k = key.to_string();
            while let Some((parent, _)) = k.rsplit_once('\\') {
                if parent.len() <= base.len() || !delete_empty(parent) {
                    break;
                }
                k = parent.to_string();
            }
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
    use windows::Win32::System::Registry::RegDeleteTreeW;

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
        let verb = HSTRING::from(format!(r"{base}\CLSID\{EXPLORER_CLSID}\shell\opennewwindow\command"));
        assert_eq!(read_default(&verb).as_deref(), Some(r#""C:\x\rexplorer.exe""#));
        unregister(&base).unwrap();
        assert!(!is_registered(&base, exe));
        // The keys emptied above the commands go too; the base stays.
        assert!(!key_exists(&format!(r"{base}\CLSID")) && !key_exists(&format!(r"{base}\Folder")));
        assert!(key_exists(&base));
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

    #[test]
    fn register_refuses_a_key_with_only_other_values() {
        let (base, root) = test_base("d");
        let key = command_key(&base, "Folder");
        set_sz(&key, w!("DelegateExecute"), "{GUID}").unwrap();
        assert!(register(&base, Path::new(r"C:\x\rexplorer.exe")).is_err());
        assert!(read_default(&command_key(&base, "Directory")).is_none());
        // Unregister must not delete a key that is not ours.
        unregister(&base).unwrap();
        assert!(key_exists(&format!(r"{base}\Folder\shell\open\command")));
        cleanup(&root);
    }

    #[test]
    fn unregister_keeps_other_values_in_our_key() {
        let (base, root) = test_base("e");
        let exe = Path::new(r"C:\x\rexplorer.exe");
        register(&base, exe).unwrap();
        let key = command_key(&base, "Folder");
        set_sz(&key, w!("Extra"), "keep").unwrap();
        unregister(&base).unwrap();
        assert!(read_default(&key).is_none());
        assert!(key_exists(&format!(r"{base}\Folder\shell\open\command")));
        cleanup(&root);
    }

    #[test]
    fn unregister_leaves_foreign_overrides() {
        let (base, root) = test_base("c");
        let exe = Path::new(r"C:\x\rexplorer.exe");
        register(&base, exe).unwrap();
        let drive = command_key(&base, "Drive");
        set_sz(&drive, PCWSTR::null(), r"C:\other.exe %1").unwrap();
        unregister(&base).unwrap();
        assert!(!is_registered(&base, exe));
        assert_eq!(read_default(&drive).as_deref(), Some(r"C:\other.exe %1"));
        cleanup(&root);
    }
}
