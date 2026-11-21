//! Small Win32 helpers for the recorder: which game is in front, local time, the "clip saved" chime,
//! start-with-Windows, and the GPU's name.

use crate::util;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW, DISPLAY_DEVICE_PRIMARY_DEVICE};
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `YYYY-MM-DD__HH-MM-SS` in local time, the same pattern GG uses in clip names.
pub fn local_stamp() -> String {
    let t = unsafe {
        let mut st = std::mem::zeroed();
        GetLocalTime(&mut st);
        st
    };
    format!("{:04}-{:02}-{:02}__{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

struct Foreground {
    exe: String,
    title: String,
}

fn foreground() -> Option<Foreground> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mut tbuf = [0u16; 512];
        let tlen = GetWindowTextW(hwnd, tbuf.as_mut_ptr(), tbuf.len() as i32).max(0) as usize;
        let title = String::from_utf16_lossy(&tbuf[..tlen]);
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return Some(Foreground { exe: String::new(), title });
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        let exe = if ok != 0 { String::from_utf16_lossy(&buf[..len as usize]) } else { String::new() };
        Some(Foreground { exe, title })
    }
}

/// FileDescription (or ProductName) from an executable's version resource.
fn file_description(exe: &str) -> Option<String> {
    unsafe {
        let w = wide(exe);
        let size = GetFileVersionInfoSizeW(w.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr() as _) == 0 {
            return None;
        }
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<String> = Vec::new();
        if VerQueryValueW(data.as_ptr() as _, wide(r"\VarFileInfo\Translation").as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let pair = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for c in pair.chunks(2) {
                if c.len() == 2 {
                    langs.push(format!("{:04x}{:04x}", c[0], c[1]));
                }
            }
        }
        langs.extend(["040904b0".to_string(), "040904e4".to_string()]);
        for key in ["FileDescription", "ProductName"] {
            for lang in &langs {
                let q = wide(&format!(r"\StringFileInfo\{lang}\{key}"));
                if VerQueryValueW(data.as_ptr() as _, q.as_ptr(), &mut ptr, &mut len) != 0 && len > 1 {
                    let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
                    let v = String::from_utf16_lossy(&s[..end]).trim().to_string();
                    if !v.is_empty() {
                        return Some(v);
                    }
                }
            }
        }
        None
    }
}

/// Apps that are never "the game".
const NOT_GAMES: &[&str] = &[
    "explorer", "backtrack", "discord", "discordptb", "discordcanary", "chrome", "msedge", "firefox", "opera", "brave",
    "spotify", "code", "devenv", "steam", "steamwebhelper", "epicgameslauncher", "battle.net", "steelseriesgg",
    "steelseriesengine", "obs64", "claude", "windowsterminal", "powershell", "cmd", "taskmgr", "notepad", "slack",
    "teams", "ms-teams", "outlook", "searchhost", "shellexperiencehost", "startmenuexperiencehost", "applicationframehost",
    "lockapp", "textinputhost",
];

/// Executable stems whose descriptions don't match how players name the game.
const KNOWN: &[(&str, &str)] = &[
    ("rocketleague", "Rocket League"),
    ("cs2", "Counter-Strike 2"),
    ("csgo", "Counter-Strike: Global Offensive"),
    ("peak", "PEAK"),
    ("helldivers2", "HELLDIVERS™ 2"),
    ("cod", "Call of Duty®"),
    ("iw4x", "Call of Duty: Modern Warfare 2 (2009)"),
    ("iw4mp", "Call of Duty: Modern Warfare 2 (2009)"),
    ("bf6", "Battlefield 6"),
    ("bodycam", "Bodycam"),
    ("bodycam-win64-shipping", "Bodycam"),
    ("valorant-win64-shipping", "VALORANT"),
    ("fortniteclient-win64-shipping", "Fortnite"),
    ("r5apex", "Apex Legends"),
    ("r5apex_dx12", "Apex Legends"),
    ("overwatch", "Overwatch 2"),
    ("leagueoflegends", "League of Legends"),
    ("minecraft", "Minecraft"),
    ("javaw", "Minecraft"),
    ("gta5", "Grand Theft Auto V"),
    ("eldenring", "ELDEN RING"),
    ("cyberpunk2077", "Cyberpunk 2077"),
];

/// Name for a clip saved right now: the foreground game, matched to a game already in the library when
/// possible so new clips group with GG's; "Desktop capture" when no game is in front.
pub fn current_game(library_games: &[String]) -> String {
    let Some(fg) = foreground() else { return "Desktop capture".into() };
    let stem = Path::new(&fg.exe).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if stem.is_empty() || NOT_GAMES.contains(&stem.as_str()) {
        return "Desktop capture".into();
    }
    // Candidates, best first.
    let mut cands: Vec<String> = Vec::new();
    if let Some((_, n)) = KNOWN.iter().find(|(s, _)| *s == stem) {
        cands.push(n.to_string());
    }
    if let Some(d) = file_description(&fg.exe) {
        cands.push(d);
    }
    let title = fg.title.split(" (").next().unwrap_or("").split(" - ").next().unwrap_or("").trim().to_string();
    if !title.is_empty() {
        cands.push(title);
    }
    cands.push(stem.replace(['-', '_'], " "));
    // Prefer a name the library already uses (same slug).
    for c in &cands {
        let slug = util::slugify(c);
        if let Some(g) = library_games.iter().find(|g| util::slugify(g) == slug) {
            return g.clone();
        }
    }
    cands.into_iter().find(|c| !c.trim().is_empty()).unwrap_or_else(|| "Desktop capture".into())
}

/// Name of the primary display adapter, e.g. "AMD Radeon RX 7900 XTX".
pub fn gpu_name() -> String {
    unsafe {
        let mut i = 0;
        loop {
            let mut dd: DISPLAY_DEVICEW = std::mem::zeroed();
            dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) == 0 {
                return String::new();
            }
            if dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0 {
                let end = dd.DeviceString.iter().position(|c| *c == 0).unwrap_or(dd.DeviceString.len());
                return String::from_utf16_lossy(&dd.DeviceString[..end]);
            }
            i += 1;
        }
    }
}

/// A short two-note chime, generated once as an in-memory WAV.
fn chime() -> &'static [u8] {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| {
        let rate = 44_100u32;
        let notes = [(880.0f32, 0.0f32), (1318.5, 0.09)];
        let len = (rate as f32 * 0.34) as usize;
        let mut pcm = vec![0f32; len];
        for (freq, start) in notes {
            let s0 = (start * rate as f32) as usize;
            for (i, v) in pcm.iter_mut().enumerate().skip(s0) {
                let t = (i - s0) as f32 / rate as f32;
                let env = (t * 200.0).min(1.0) * (-t * 14.0).exp();
                *v += (2.0 * std::f32::consts::PI * freq * t).sin() * env * 0.22;
            }
        }
        let data: Vec<u8> = pcm.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let mut w = Vec::with_capacity(44 + data.len());
        w.extend(b"RIFF");
        w.extend((36 + data.len() as u32).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(rate.to_le_bytes());
        w.extend((rate * 2).to_le_bytes());
        w.extend(2u16.to_le_bytes());
        w.extend(16u16.to_le_bytes());
        w.extend(b"data");
        w.extend((data.len() as u32).to_le_bytes());
        w.extend(data);
        w
    })
}

pub fn play_chime() {
    let wav = chime();
    unsafe {
        PlaySoundW(wav.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

/// Whether Windows says a game is running exclusive full screen (it then holds notifications back too). Any window
/// shown on top of such a game, even a click-through one that never takes focus, makes it minimize. A borderless
/// full-screen game is an ordinary window, so a pop-up can sit on top of it.
pub fn exclusive_fullscreen() -> bool {
    use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    let mut state = 0;
    unsafe { SHQueryUserNotificationState(&mut state) == 0 && state == QUNS_RUNNING_D3D_FULL_SCREEN }
}

/// Top-left corner (physical pixels) of the monitor that the window in front fully covers, like a full-screen game.
pub fn fullscreen_monitor() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect};
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return None;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
        // The desktop covers the whole monitor too.
        if matches!(String::from_utf16_lossy(&cls[..n]).as_str(), "Progman" | "WorkerW") {
            return None;
        }
        let mut r: RECT = std::mem::zeroed();
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONULL);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if mon.is_null() || GetWindowRect(h, &mut r) == 0 || GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let m = mi.rcMonitor;
        (r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom).then_some((m.left, m.top))
    }
}

/// Make a window click-through without activating it. Tauri's `set_ignore_cursor_events` re-shows the window with
/// SW_SHOW, which can activate it, and right after a hotkey press Windows lets that take the foreground from a game.
pub fn click_through(hwnd: *mut core::ffi::c_void) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
    };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE) as isize);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}

/// Add or remove Backtrack from HKCU\...\Run (per user, no admin needed).
pub fn set_start_with_windows(on: bool) -> Result<(), String> {
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut cmd = std::process::Command::new("reg.exe");
    cmd.creation_flags(0x0800_0000);
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        cmd.args(["add", key, "/v", "Backtrack", "/t", "REG_SZ", "/f", "/d"])
            .arg(format!("\"{}\" --background", exe.display()));
    } else {
        cmd.args(["delete", key, "/v", "Backtrack", "/f"]);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() || !on {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
