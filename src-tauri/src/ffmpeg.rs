//! Locating and running the local ffmpeg / ffprobe binaries. Nothing here touches the network.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Helpers (thumbnails, waveforms, probes, saving, exports) run below normal priority: a game, at normal priority,
/// always gets the CPU first, and they still run at full speed when the CPU is free. Only the recorder, which must
/// keep up with the screen, runs at normal priority.
pub(crate) const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// ffmpeg for helper work, below normal priority.
    pub fn ffmpeg_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffmpeg);
        c.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS).stdin(Stdio::null());
        c.args(["-hide_banner", "-nostdin"]);
        c
    }

    /// ffmpeg for the recorder, normal priority.
    pub fn recorder_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffmpeg);
        c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null());
        c.args(["-hide_banner", "-nostdin"]);
        c
    }

    /// ffprobe, below normal priority.
    pub fn ffprobe_cmd(&self) -> Command {
        let mut c = Command::new(&self.ffprobe);
        c.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS).stdin(Stdio::null());
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

#[cfg(test)]
mod tests {
    use super::Tools;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    };

    /// The priority class Windows gave a process started by `cmd`. cmd.exe stands in for ffmpeg: it ignores ffmpeg's
    /// arguments and waits for input, so nothing heavy runs, and the builders' flags keep its window from opening.
    fn started_priority(mut cmd: Command) -> u32 {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("cmd.exe");
        let class = unsafe { GetPriorityClass(child.as_raw_handle()) };
        drop(child.stdin.take());
        let _ = child.kill();
        let _ = child.wait();
        class
    }

    /// Helpers (thumbnails, waveforms, probes, saving, exports) start below normal priority, so a game always gets
    /// the CPU first; only the recorder, which must keep up with the screen, starts at normal priority.
    #[test]
    fn helpers_start_below_normal_priority_and_the_recorder_does_not() {
        let cmd = PathBuf::from(std::env::var_os("ComSpec").unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into()));
        let tools = Tools { ffmpeg: cmd.clone(), ffprobe: cmd };
        assert_eq!(started_priority(tools.ffmpeg_cmd()), BELOW_NORMAL_PRIORITY_CLASS, "ffmpeg_cmd");
        assert_eq!(started_priority(tools.ffprobe_cmd()), BELOW_NORMAL_PRIORITY_CLASS, "ffprobe_cmd");
        // A process started without a priority runs at normal, unless its parent runs below normal (it then inherits
        // that), so this half needs the tests themselves to run at normal priority or above.
        let ours = unsafe { GetPriorityClass(GetCurrentProcess()) };
        if ours != BELOW_NORMAL_PRIORITY_CLASS && ours != IDLE_PRIORITY_CLASS {
            assert_eq!(started_priority(tools.recorder_cmd()), NORMAL_PRIORITY_CLASS, "recorder_cmd");
        }
    }

    /// Every ffmpeg and ffprobe starts through Tools' builders, and only the recorder (and the hardware tests that
    /// record like it) through recorder_cmd: a helper started any other way runs at normal priority, against the game.
    #[test]
    fn only_the_recorder_starts_ffmpeg_at_normal_priority() {
        fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    sources(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&src, &mut files);
        let (mut wrong, mut recorder) = (Vec::new(), 0);
        for path in files {
            let name = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            if name == "ffmpeg.rs" {
                continue;
            }
            for (i, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
                let at = format!("{name}:{}: {}", i + 1, line.trim());
                if line.contains("Command::new") && (line.contains("ffmpeg") || line.contains("ffprobe")) {
                    wrong.push(at);
                } else if line.contains("recorder_cmd(") {
                    match name.as_str() {
                        "recorder.rs" => recorder += 1,
                        n if n.starts_with("recorder/") && n.ends_with("_test.rs") => {}
                        _ => wrong.push(at),
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "start helpers with ffmpeg_cmd / ffprobe_cmd (below normal priority):\n{}", wrong.join("\n"));
        assert_eq!(recorder, 1, "recorder.rs starts the capture with recorder_cmd, once");
    }
}
