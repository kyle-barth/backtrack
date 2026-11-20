//! Locating and running the local ffmpeg / ffprobe binaries. Nothing here touches the network.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    pub fn ffmpeg_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffmpeg);
        c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
        c.args(["-hide_banner", "-nostdin"]);
        c
    }

    pub fn ffprobe_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffprobe);
        c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
        c
    }
}

fn pair_in(dir: &Path) -> Option<Tools> {
    let ffmpeg = dir.join("ffmpeg.exe");
    let ffprobe = dir.join("ffprobe.exe");
    (ffmpeg.is_file() && ffprobe.is_file()).then_some(Tools { ffmpeg, ffprobe })
}

/// Find ffmpeg + ffprobe: next to the app, on PATH, or in a winget install.
pub fn locate() -> Option<Tools> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("BACKTRACK_FFMPEG_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("ffmpeg"));
        dirs.push(exe_dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(local) = dirs::data_local_dir() {
        let winget = local.join("Microsoft").join("WinGet");
        dirs.push(winget.join("Links"));
        // winget's portable installs land in Packages\Gyan.FFmpeg*\ffmpeg-*\bin. PATH may not be refreshed
        // for an already-running process, so look there directly.
        if let Ok(rd) = std::fs::read_dir(winget.join("Packages")) {
            for pkg in rd.flatten() {
                let name = pkg.file_name().to_string_lossy().to_lowercase();
                if !name.contains("ffmpeg") {
                    continue;
                }
                if let Ok(inner) = std::fs::read_dir(pkg.path()) {
                    for build in inner.flatten() {
                        dirs.push(build.path().join("bin"));
                    }
                }
            }
        }
    }
    dirs.push(PathBuf::from(r"C:\ffmpeg\bin"));
    dirs.push(PathBuf::from(r"C:\Program Files\ffmpeg\bin"));

    // A WinGet\Links entry may be a symlink pair; pair_in follows symlinks via is_file().
    dirs.iter().find_map(|d| pair_in(d))
}
