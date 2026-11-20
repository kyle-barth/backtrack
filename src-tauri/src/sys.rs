//! Windows shell helpers: clipboard file copy, Explorer, opening Discord, Recycle Bin.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

/// Put a file on the clipboard (CF_HDROP), the same as Ctrl+C in Explorer. Ctrl+V in Discord attaches it.
pub fn copy_file(path: &str) -> Result<(), String> {
    if !Path::new(path).is_file() {
        return Err("The exported file is missing.".into());
    }
    let _clip = clipboard_win::Clipboard::new_attempts(10).map_err(|e| format!("Clipboard is busy: {e}"))?;
    clipboard_win::raw::empty().map_err(|e| format!("Couldn't clear the clipboard: {e}"))?;
    clipboard_win::raw::set_file_list(&[path]).map_err(|e| format!("Couldn't copy the file: {e}"))
}

/// Open Explorer with the file selected.
pub fn reveal(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.is_file() {
        Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.replace('/', "\\")))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if p.is_dir() {
        open_path(path)
    } else if let Some(parent) = p.parent().filter(|d| d.is_dir()) {
        open_path(&parent.to_string_lossy())
    } else {
        Err("That file or folder no longer exists.".into())
    }
}

pub fn open_path(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        std::fs::create_dir_all(path).map_err(|_| "That folder doesn't exist.".to_string())?;
    }
    open::that_detached(path).map_err(|e| e.to_string())
}

/// Launch the Discord app via its URL protocol; fall back to Discord in the browser.
pub fn open_discord() -> Result<(), String> {
    let installed = dirs::data_local_dir().map(|d| d.join("Discord").is_dir()).unwrap_or(false);
    if installed && open::that_detached("discord://-/channels/@me").is_ok() {
        return Ok(());
    }
    open::that_detached("https://discord.com/app").map_err(|e| e.to_string())
}

/// Send a file to the Recycle Bin (recoverable). The error is the reason, in a few plain words.
pub fn trash(path: &str) -> Result<(), String> {
    trash::delete(path).map_err(|e| trash_reason(&e))
}

/// Send several files to the Recycle Bin in one shell operation, then say how each one went.
/// A file that is gone afterwards counts as moved (it may already have been deleted outside Backtrack);
/// any that are still there are tried once more on their own, which also gives the reason for each.
/// The shell refuses (rather than permanently deletes) a file that can't go to the Recycle Bin.
pub fn trash_all(paths: &[String]) -> Vec<Result<(), String>> {
    if let Err(e) = trash::delete_all(paths) {
        crate::util::log("warn", &format!("Recycle Bin batch of {}: {e}", paths.len()));
    }
    paths
        .iter()
        .map(|p| if Path::new(p).exists() { trash(p) } else { Ok(()) })
        .collect()
}

/// Delete files outright, skipping the Recycle Bin (only when the user chose that). A file that's already gone counts as deleted.
pub fn delete_all(paths: &[String]) -> Vec<Result<(), String>> {
    paths
        .iter()
        .map(|p| match std::fs::remove_file(p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(match e.raw_os_error() {
                Some(32) | Some(33) => "It's open in another program".into(), // sharing / lock violation
                Some(5) => "Windows denied access to it".into(),
                _ => e.to_string(),
            }),
        })
        .collect()
}

/// The Recycle Bin of the drive a file is on.
pub struct Bin {
    /// "C:" (or the mount folder for a drive mounted in a folder)
    pub drive: String,
    /// The most it holds, in bytes. `None` if Windows doesn't say.
    pub capacity: Option<u64>,
    /// Files on this drive are deleted outright: it has no Recycle Bin (USB stick, network drive)
    /// or is set to "Don't move files to the Recycle Bin".
    pub permanent: bool,
}

pub fn recycle_bin(path: &str) -> Bin {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
    const DRIVE_FIXED: u32 = 3;
    let w = wide(path);
    let mut root = [0u16; 512];
    if unsafe { GetVolumePathNameW(w.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        let drive = path.split(['\\', '/']).next().unwrap_or(path).to_string();
        return Bin { drive, capacity: None, permanent: false };
    }
    let root_s = from_wide(&root);
    let fixed = unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_FIXED;
    let mut vol = [0u16; 64];
    // "\\?\Volume{GUID}\" → "{GUID}", which names the drive's Recycle Bin settings in the registry
    let guid = (unsafe { GetVolumeNameForVolumeMountPointW(root.as_ptr(), vol.as_mut_ptr(), vol.len() as u32) } != 0)
        .then(|| from_wide(&vol))
        .and_then(|v| Some(v[v.find('{')?..=v.find('}')?].to_string()));
    let key = guid.map(|g| format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{g}"));
    let dword = |name: &str| key.as_deref().and_then(|k| reg_dword(k, name));
    Bin {
        drive: root_s.trim_end_matches('\\').to_string(),
        capacity: dword("MaxCapacity").map(|mb| mb as u64 * 1024 * 1024),
        permanent: !fixed || dword("NukeOnDelete") == Some(1),
    }
}

fn reg_dword(key: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let (k, v) = (wide(key), wide(value));
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut size)
    };
    (r == 0).then_some(data)
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())])
}

/// Put files back from the Recycle Bin to where they were. For each path: Ok, or why not.
/// When the same path was binned more than once, the most recent one comes back.
pub fn restore_all(paths: &[String]) -> Result<Vec<Result<(), String>>, String> {
    let items = trash::os_limited::list().map_err(|e| format!("Couldn't read the Recycle Bin: {}", trash_reason(&e)))?;
    let mut out: Vec<Result<(), String>> = Vec::with_capacity(paths.len());
    let mut picks = Vec::new();
    for p in paths {
        let key = crate::util::path_key(Path::new(p));
        let item = items
            .iter()
            .filter(|i| crate::util::path_key(&i.original_path()) == key)
            .max_by_key(|i| i.time_deleted);
        out.push(match item {
            _ if Path::new(p).exists() => Err("A file with the same name is back in its folder".into()),
            None => Err("It's no longer in the Recycle Bin".into()),
            Some(i) => {
                picks.push(i.clone());
                Ok(())
            }
        });
    }
    if !picks.is_empty() {
        if let Err(e) = trash::os_limited::restore_all(picks) {
            crate::util::log("warn", &format!("restoring from the Recycle Bin: {e}"));
        }
    }
    // the file being back in place is what counts
    for (p, r) in paths.iter().zip(out.iter_mut()) {
        if r.is_ok() && !Path::new(p).exists() {
            *r = Err("Windows couldn't put it back".into());
        }
    }
    Ok(out)
}

/// Why the shell didn't move a file, for the UI: "It's open in another program" rather than an HRESULT.
fn trash_reason(e: &trash::Error) -> String {
    let trash::Error::Os { code, description } = e else { return e.to_string() };
    match *code as u32 {
        // COPYENGINE_E_SHARING_VIOLATION_SRC, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        0x8027_0027 | 0x8007_0020 | 0x8007_0021 => return "It's open in another program".into(),
        // COPYENGINE_E_ACCESS_DENIED_SRC, COPYENGINE_E_ACCESSDENIED_READONLY, E_ACCESSDENIED
        0x8027_0021 | 0x8027_003F | 0x8007_0005 => return "Windows denied access to it".into(),
        // COPYENGINE_E_PATH_NOT_FOUND_SRC, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND
        0x8027_0023 | 0x8007_0002 | 0x8007_0003 => return "The file isn't there any more".into(),
        // COPYENGINE_E_RECYCLE_FORCE_NUKE, COPYENGINE_E_RECYCLE_BIN_NOT_FOUND
        0x8027_0036 | 0x8027_003A => return "Its drive has no Recycle Bin".into(),
        0x8027_0037 => return "It's too big for the Recycle Bin".into(), // COPYENGINE_E_RECYCLE_SIZE_TOO_BIG
        // COPYENGINE_E_NET_DISCONNECT_SRC, COPYENGINE_E_CANT_REACH_SOURCE
        0x8027_0025 | 0x8027_0035 => return "Its drive can't be reached".into(),
        _ => {}
    }
    // "windows error: C:\…\clip.mp4: the shell's file operation failed (0x…)" → "The shell's file operation failed (0x…)"
    let d = description.strip_prefix("windows error: ").unwrap_or(description);
    let d = d.rsplit_once(": ").map_or(d, |(_, r)| r).trim();
    let mut c = d.chars();
    c.next().map_or_else(|| "Windows couldn't move it".into(), |f| f.to_uppercase().chain(c).collect())
}

#[cfg(test)]
mod tests {
    use super::trash_reason;

    fn os(code: u32, description: &str) -> trash::Error {
        trash::Error::Os { code: code as i32, description: description.into() }
    }

    #[test]
    fn trash_reasons_are_plain_words() {
        assert_eq!(trash_reason(&os(0x8027_0027, "x")), "It's open in another program");
        assert_eq!(trash_reason(&os(0x8007_0005, "x")), "Windows denied access to it");
        assert_eq!(trash_reason(&os(0x8027_0036, "x")), "Its drive has no Recycle Bin");
        assert_eq!(
            trash_reason(&os(0x8027_0001, r"windows error: C:\Clips\a.mp4: the shell's file operation failed (0x80270001)")),
            "The shell's file operation failed (0x80270001)"
        );
        assert_eq!(trash_reason(&os(0x8000_4005, "windows error: Unspecified error")), "Unspecified error");
    }
}

/// Real Recycle Bin round trip on throwaway files: `cargo test -- --ignored recycle_bin_round_trip`
#[cfg(test)]
mod bin_tests {
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    #[test]
    #[ignore]
    fn recycle_bin_round_trip() {
        let dir = std::env::temp_dir().join(format!("backtrack-bin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<String> = (0..3)
            .map(|i| {
                let p = dir.join(format!("clip-{i}.mp4"));
                std::fs::write(&p, vec![i as u8; 4096]).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();

        let bin = super::recycle_bin(&paths[0]);
        println!("drive {} capacity {:?} permanent {}", bin.drive, bin.capacity, bin.permanent);
        assert!(!bin.drive.is_empty());

        // hold clip-1 open with no sharing, the way a player or the recorder would
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&paths[1]).unwrap();
        let r = super::trash_all(&paths);
        println!("trash: {r:?}");
        assert!(r[0].is_ok() && r[2].is_ok(), "{r:?}");
        assert_eq!(r[1].as_ref().unwrap_err(), "It's open in another program");
        assert!(!Path::new(&paths[0]).exists() && Path::new(&paths[1]).exists() && !Path::new(&paths[2]).exists());
        drop(lock);

        let back = super::restore_all(&[paths[0].clone(), paths[2].clone()]).unwrap();
        println!("restore: {back:?}");
        assert!(back.iter().all(|r| r.is_ok()), "{back:?}");
        assert!(Path::new(&paths[0]).exists() && Path::new(&paths[2]).exists());
        // a second restore finds nothing left to bring back
        std::fs::remove_file(&paths[2]).unwrap();
        let again = super::restore_all(&[paths[2].clone()]).unwrap();
        assert_eq!(again[0].as_ref().unwrap_err(), "It's no longer in the Recycle Bin");

        let gone = super::delete_all(&paths);
        assert!(gone.iter().all(|r| r.is_ok()), "{gone:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
