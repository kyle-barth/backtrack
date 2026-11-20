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

/// Send a file to the Recycle Bin (recoverable).
pub fn trash(path: &str) -> Result<(), String> {
    trash::delete(path).map_err(|e| format!("Couldn't move the clip to the Recycle Bin: {e}"))
}
