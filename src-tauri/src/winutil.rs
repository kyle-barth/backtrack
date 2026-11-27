//! Small Win32 helpers for the recorder: which game is in front, local time, the "clip saved" chime,
//! start-with-Windows, and the GPU's name.

use crate::util;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW, DISPLAY_DEVICE_PRIMARY_DEVICE};
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

/// Wait (up to `max`) for process `pid` to end. Returns at once if it's already gone.
pub fn wait_for_exit(pid: u32, max: std::time::Duration) {
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        WaitForSingleObject(h, max.as_millis().min(u32::MAX as u128) as u32);
        CloseHandle(h);
    }
}
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

#[derive(Clone)]
struct Foreground {
    exe: String,
    title: String,
    pid: u32,
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
        Some(Foreground { exe: process_exe(pid).unwrap_or_default(), title, pid })
    }
}

/// The full path of process `pid`'s program, while it's running.
fn process_exe(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return None;
        }
        // A process that has ended but whose handle someone still holds can still be opened.
        let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (running && ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Whether a window's program could be the game: anything but the apps listed in NOT_GAMES.
fn could_be_game(fg: &Foreground) -> bool {
    let stem = Path::new(&fg.exe).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    !stem.is_empty() && !NOT_GAMES.contains(&stem.as_str())
}

/// The game last seen in front, as it was then (see note_game).
static LAST_GAME: Mutex<Option<Foreground>> = Mutex::new(None);

/// Remember the game in front, if one is. Called every 2 s while recording (and at every save), so a clip saved
/// while another window is in front, like Discord beside a windowed game, is still named after the game.
pub fn note_game() {
    if let Some(fg) = foreground().filter(could_be_game) {
        *LAST_GAME.lock().unwrap() = Some(fg);
    }
}

/// The game a clip saved now comes from: the one in front, else the game last seen in front while it's still
/// running (behind another window, or minimized). Once that game has closed, none: the clip is a desktop capture.
fn clip_game() -> Option<Foreground> {
    pick_game(foreground(), &mut LAST_GAME.lock().unwrap(), still_open)
}

/// Whether the game seen earlier is still running: same process, same program (a process id is reused once its
/// process ends).
fn still_open(g: &Foreground) -> bool {
    process_exe(g.pid).map_or(false, |exe| exe.eq_ignore_ascii_case(&g.exe))
}

/// clip_game's rule, given the window in front, the game last seen in front, and how to tell it's still open.
fn pick_game(front: Option<Foreground>, last: &mut Option<Foreground>, open: impl Fn(&Foreground) -> bool) -> Option<Foreground> {
    if let Some(fg) = front.filter(could_be_game) {
        *last = Some(fg.clone());
        return Some(fg);
    }
    if !last.as_ref().map_or(false, open) {
        *last = None;
    }
    last.clone()
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
    // Stream decks, overlays, launchers and peripheral apps: in front for a moment, never what's being played.
    "streamdeck", "nvidia overlay", "nvidia app", "eadesktop", "ubisoftconnect", "upc", "galaxyclient", "riotclientux",
    "riotclientservices", "medal", "overwolf", "lghub", "icue",
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
    ("fc24", "EA SPORTS FC 24"),
    ("fc25", "EA SPORTS FC 25"),
    ("fc26", "EA SPORTS FC 26"),
    ("fc27", "EA SPORTS FC 27"),
];

/// Name for a clip saved right now: the game in front, or the one last in front while it's still running (see
/// clip_game), matched to a game already in the library when possible so new clips group with GG's; "Desktop
/// capture" when there's no game.
pub fn current_game(library_games: &[String]) -> String {
    let Some(fg) = clip_game() else { return "Desktop capture".into() };
    let stem = Path::new(&fg.exe).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
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

/// Whether a game seems to be in front, for things that shouldn't happen mid-match (installing an update, the "updated"
/// pop-up): a game running exclusive full screen, a known game in front, or a window covering its whole monitor
/// (borderless full screen) that isn't an app that's never a game (a browser playing a video full screen, say).
pub fn game_in_front() -> bool {
    if exclusive_fullscreen() {
        return true;
    }
    let Some(fg) = foreground() else { return false };
    let stem = Path::new(&fg.exe).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if stem.is_empty() || NOT_GAMES.contains(&stem.as_str()) {
        return false;
    }
    KNOWN.iter().any(|(s, _)| *s == stem) || fullscreen_monitor().is_some()
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

/// Show a window topmost at a position (physical pixels) without activating it, for the reused pop-up. Only Win32 is
/// used to show and hide it after it's made, so Tauri never re-shows it with SW_SHOW (see click_through).
pub fn show_no_activate(hwnd: *mut core::ffi::c_void, x: i32, y: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW};
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// Hide a window (hiding one that isn't active doesn't move the focus).
pub fn hide(hwnd: *mut core::ffi::c_void) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn win(exe: &str, pid: u32) -> Foreground {
        Foreground { exe: exe.into(), title: String::new(), pid }
    }

    /// A player has FC in a window beside Discord. A clip saved with Discord in front is still FC's while FC runs; once
    /// FC is closed it's a desktop capture, never named after a game that isn't open.
    #[test]
    fn a_clip_is_named_after_a_game_only_while_it_is_open() {
        let fc = win(r"C:\Games\FC 27\FC27.exe", 4242);
        let discord = win(r"C:\Users\x\AppData\Local\Discord\app-1.0\Discord.exe", 17);
        let mut last = None;
        let open = |_: &Foreground| true;
        let closed = |_: &Foreground| false;

        assert_eq!(pick_game(Some(fc.clone()), &mut last, open).map(|g| g.pid), Some(4242), "in front");
        assert_eq!(pick_game(Some(discord.clone()), &mut last, open).map(|g| g.pid), Some(4242), "behind Discord");
        assert_eq!(pick_game(None, &mut last, open).map(|g| g.pid), Some(4242), "minimized, nothing in front");
        let deck = win(r"C:\Program Files\Elgato\StreamDeck\StreamDeck.exe", 9);
        assert_eq!(pick_game(Some(deck), &mut last, open).map(|g| g.pid), Some(4242), "a Stream Deck isn't a game");

        assert!(pick_game(Some(discord.clone()), &mut last, closed).is_none(), "FC closed: a desktop capture");
        assert!(last.is_none(), "and forgotten");
        assert!(pick_game(Some(discord), &mut last, open).is_none(), "even if its process id comes back");
    }

    /// still_open's check: a process that has ended doesn't count, even while a handle to it is still held.
    #[test]
    fn an_ended_process_is_not_open() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let exe = process_exe(child.id()).expect("running");
        assert!(exe.to_lowercase().ends_with("cmd.exe"), "{exe}");
        assert!(still_open(&win(&exe, child.id())));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(process_exe(child.id()).is_none());
        assert!(!still_open(&win(&exe, child.id())));
    }
}
