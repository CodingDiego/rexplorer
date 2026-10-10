//! Whole-volume filename index read from the NTFS MFT via FSCTL_ENUM_USN_DATA. Needs elevation.
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr;

use windows::Win32::Foundation::{CloseHandle, ERROR_HANDLE_EOF, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, MFT_ENUM_DATA_V0, USN_JOURNAL_DATA_V0,
};
use windows::core::{HSTRING, Result};

const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const ROOT_INDEX: u64 = 5;

struct Entry {
    parent: u64,
    name: String,
}

/// Name/parent table of every file on one NTFS volume, keyed by masked file reference number.
pub struct VolumeIndex {
    drive: char,
    entries: HashMap<u64, Entry>,
}

/// `build` needs an elevated token.
pub use crate::elevation::is_elevated;

/// Enumerates the MFT of `drive:`. Fails with the Win32 error (access denied when not elevated).
pub fn build(drive: char) -> Result<VolumeIndex> {
    let path = HSTRING::from(format!("\\\\.\\{drive}:"));
    let h = unsafe {
        CreateFileW(
            &path,
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )?
    };
    let r = enumerate(h, drive);
    let _ = unsafe { CloseHandle(h) };
    r
}

fn enumerate(h: HANDLE, drive: char) -> Result<VolumeIndex> {
    let mut journal = USN_JOURNAL_DATA_V0::default();
    let mut ret = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(&mut journal as *mut _ as *mut c_void),
            size_of::<USN_JOURNAL_DATA_V0>() as u32,
            Some(&mut ret),
            None,
        )?
    };
    let mut med = MFT_ENUM_DATA_V0 {
        StartFileReferenceNumber: 0,
        LowUsn: 0,
        HighUsn: journal.NextUsn,
    };
    let mut buf = vec![0u8; 1 << 20];
    let mut entries = HashMap::new();
    loop {
        let r = unsafe {
            DeviceIoControl(
                h,
                FSCTL_ENUM_USN_DATA,
                Some(&med as *const _ as *const c_void),
                size_of::<MFT_ENUM_DATA_V0>() as u32,
                Some(buf.as_mut_ptr() as *mut c_void),
                buf.len() as u32,
                Some(&mut ret),
                None,
            )
        };
        match r {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_HANDLE_EOF.to_hresult() => break,
            Err(e) => return Err(e),
        }
        let n = ret as usize;
        if n < 8 {
            break;
        }
        med.StartFileReferenceNumber = u64::from_le_bytes(buf[..8].try_into().unwrap());
        parse_records(&buf[8..n], &mut entries);
    }
    Ok(VolumeIndex { drive, entries })
}

/// Parses packed USN_RECORD_V2 records (no leading cursor) into `into`; stops at the first malformed one.
fn parse_records(buf: &[u8], into: &mut HashMap<u64, Entry>) {
    let mut off = 0usize;
    while off + 60 <= buf.len() {
        let rec = &buf[off..];
        let rd = |o: usize| unsafe { ptr::read_unaligned(rec.as_ptr().add(o) as *const u64) };
        let rd32 = |o: usize| unsafe { ptr::read_unaligned(rec.as_ptr().add(o) as *const u32) };
        let rd16 = |o: usize| unsafe { ptr::read_unaligned(rec.as_ptr().add(o) as *const u16) };
        let len = rd32(0) as usize;
        if len < 60 || len > rec.len() {
            return;
        }
        let (name_len, name_off) = (rd16(56) as usize, rd16(58) as usize);
        if rd16(4) == 2 && name_off + name_len <= len {
            let name: Vec<u16> = rec[name_off..name_off + name_len]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            into.insert(
                rd(8) & FRN_MASK,
                Entry { parent: rd(16) & FRN_MASK, name: String::from_utf16_lossy(&name) },
            );
        }
        off += len;
    }
}

impl VolumeIndex {
    /// Full path of `frn`, or None when the chain is broken, cyclic or deeper than 256.
    pub fn path_of(&self, frn: u64) -> Option<PathBuf> {
        let mut parts = Vec::new();
        let mut cur = frn & FRN_MASK;
        for _ in 0..256 {
            if cur == ROOT_INDEX {
                let mut p = PathBuf::from(format!("{}:\\", self.drive));
                p.extend(parts.iter().rev());
                return Some(p);
            }
            let e = self.entries.get(&cur)?;
            parts.push(e.name.as_str());
            cur = e.parent;
        }
        None
    }

    /// Case-insensitive substring match on file names; at most `limit` resolvable paths.
    pub fn search(&self, needle: &str, limit: usize) -> Vec<PathBuf> {
        let needle = needle.to_lowercase();
        self.entries
            .iter()
            .filter(|(_, e)| e.name.to_lowercase().contains(&needle))
            .filter_map(|(&frn, _)| self.path_of(frn))
            .take(limit)
            .collect()
    }

    /// Number of indexed entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(frn: u64, parent: u64, name: &str) -> Vec<u8> {
        let n: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let len = (60 + n.len() + 7) & !7;
        let mut b = vec![0u8; len];
        b[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        b[4..6].copy_from_slice(&2u16.to_le_bytes());
        b[8..16].copy_from_slice(&frn.to_le_bytes());
        b[16..24].copy_from_slice(&parent.to_le_bytes());
        b[56..58].copy_from_slice(&(n.len() as u16).to_le_bytes());
        b[58..60].copy_from_slice(&60u16.to_le_bytes());
        b[60..60 + n.len()].copy_from_slice(&n);
        b
    }

    #[test]
    fn parse_records_reads_hand_built_buffer() {
        let mut buf = rec(0x0003_0000_0000_000A, 5, "Users");
        buf.extend(rec(11, 0x0001_0000_0000_000A, "user"));
        let mut m = HashMap::new();
        parse_records(&buf, &mut m);
        assert_eq!(m.len(), 2);
        assert_eq!((m[&10].parent, m[&10].name.as_str()), (5, "Users"));
        assert_eq!((m[&11].parent, m[&11].name.as_str()), (10, "user"));
    }

    #[test]
    fn path_of_resolves_chain() {
        let e = |parent, name: &str| Entry { parent, name: name.into() };
        let entries = HashMap::from([
            (5, e(5, "")),
            (10, e(5, "Users")),
            (11, e(10, "user")),
            (20, e(21, "a")),
            (21, e(20, "b")),
        ]);
        let idx = VolumeIndex { drive: 'C', entries };
        assert_eq!(idx.path_of(11), Some(PathBuf::from("C:\\Users\\user")));
        assert_eq!(idx.path_of(20), None);
    }

    #[test]
    fn build_real_volume_when_elevated() {
        if !is_elevated() {
            eprintln!("skipped: not elevated");
            return;
        }
        let idx = build('C').unwrap();
        assert!(idx.len() > 10_000);
        assert!(!idx.search("notepad.exe", 5).is_empty());
    }
}
