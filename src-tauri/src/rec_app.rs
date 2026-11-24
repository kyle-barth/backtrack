//! App-level wiring for the recorder: commands, global hotkeys, the tray icon, the corner pop-up, and the
//! status pump that feeds the UI's live meters.

use crate::recorder::{HotkeyStatus, RecorderStatus, Saved};
use crate::settings::{Hotkeys, RecorderSettings};
use crate::{util, winutil, AppState, FFMPEG_MISSING};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

type St<'a> = State<'a, Arc<AppState>>;

// ------------------------------------------------------------------ monitors

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// Monitors with the main one first; the index is what the GPU capture uses (0 = main on this PC).
pub fn monitors(app: &AppHandle) -> Vec<MonitorInfo> {
    let primary = app.primary_monitor().ok().flatten();
    let pname = primary.as_ref().and_then(|m| m.name().cloned());
    let mut all = app.available_monitors().unwrap_or_default();
    all.sort_by_key(|m| if m.name().cloned() == pname { 0 } else { 1 });
    all.iter()
        .enumerate()
        .map(|(i, m)| {
            let s = m.size();
            let is_primary = m.name().cloned() == pname;
            let kind = if s.height > s.width { "portrait" } else { "" };
            MonitorInfo {
                index: i as u32,
                name: if is_primary { "Main monitor".into() } else { format!("Monitor {}{}", i + 1, if kind.is_empty() { String::new() } else { format!(" ({kind})") }) },
                width: s.width,
                height: s.height,
                primary: is_primary,
            }
        })
        .collect()
}

fn monitor_size(app: &AppHandle, index: u32) -> (u32, u32) {
    let ms = monitors(app);
    ms.iter()
        .find(|m| m.index == index)
        .or(ms.first())
        .map(|m| (m.width, m.height))
        .unwrap_or((1920, 1080))
}

// ------------------------------------------------------------------ status + tray

/// Whether the main window's page can be seen (see page_visible).
static PAGE_VISIBLE: AtomicBool = AtomicBool::new(false);

fn emit_status(app: &AppHandle) {
    // The tray's check mark and tooltip only change with the state. Each update is a round trip to the main thread
    // and a message to Explorer, so they're only sent when they change, not twice a second.
    static TRAY: Mutex<Option<(bool, String)>> = Mutex::new(None);
    let st = app.state::<Arc<AppState>>();
    let status = st.recorder.status();
    let (on, tip) = tray_state(&status);
    let mut last = TRAY.lock().unwrap();
    if tray_changed(&mut last, &(on, tip.clone())) {
        if let Some(item) = st.tray_toggle.lock().unwrap().as_ref() {
            let _ = item.set_checked(on);
        }
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(tip));
        }
    }
    drop(last);
    // Recording stopped, is restarting, or is back: someone in a game hears about it in the corner (in Backtrack
    // itself, the page shows the state).
    if let Some((kind, title, line)) = st.recorder.take_notice() {
        if !main_in_front(app) {
            toast(app, &title, &line, &kind);
        }
    }
    let _ = app.emit("recorder-status", status);
}

/// Backtrack's own window is the one in front (its page tells people what happened; no corner pop-up needed).
fn main_in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false))
}

/// After a graphics driver reset (gpu.rs), get the replay buffer going again: its capture died with the driver.
pub fn watch_gpu(app: &AppHandle) {
    let app = app.clone();
    crate::gpu::watch(move || {
        util::log("warn", "graphics driver reset");
        let st = app.state::<Arc<AppState>>();
        if !st.settings.lock().unwrap().recorder.enabled {
            return;
        }
        // A running recorder is stopped and restarts as after any failure (recorder.rs). One that had already given
        // up, or is waiting to retry, gets a fresh start once the driver has had a moment.
        if !st.recorder.on_gpu_reset() {
            std::thread::sleep(Duration::from_secs(3));
            start_recorder(&app);
        }
    });
}

/// Start a fresh Backtrack and quit this one: the recorder, the window and all their graphics start over. The new
/// one waits for this one to be gone (main.rs, --restarted-from) before its single-instance check, which would
/// otherwise just hand it back to this one.
pub fn restart(app: &AppHandle) {
    util::log("info", "restarting Backtrack");
    app.state::<Arc<AppState>>().recorder.stop();
    let visible = app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false));
    let spawned = std::env::current_exe().and_then(|exe| {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--restarted-from").arg(std::process::id().to_string());
        if !visible {
            cmd.arg("--background");
        }
        cmd.spawn()
    });
    match spawned {
        Ok(_) => app.exit(0),
        Err(e) => util::log("error", &format!("restart: couldn't start Backtrack again: {e}")),
    }
}

#[tauri::command]
pub fn restart_app(app: AppHandle) {
    restart(&app);
}

/// The tray's check mark and tooltip for a status. Only the state, the buffer length and an error go in, never the
/// live meters, so it changes with the state rather than on every tick.
fn tray_state(status: &RecorderStatus) -> (bool, String) {
    let on = status.state == "on" || status.state == "starting";
    let tip = match status.state.as_str() {
        "on" => format!("Backtrack · replay buffer on (last {} s)", status.buffer_secs),
        "starting" => "Backtrack · starting the replay buffer".into(),
        "error" => format!("Backtrack · {}", status.error.clone().unwrap_or_default()),
        _ => "Backtrack · replay buffer off".into(),
    };
    (on, tip)
}

/// Whether the tray needs updating to show `now` (it differs from `last`, what it shows), remembering it if so.
fn tray_changed(last: &mut Option<(bool, String)>, now: &(bool, String)) -> bool {
    if last.as_ref() == Some(now) {
        return false;
    }
    *last = Some(now.clone());
    true
}

fn on_change_fn(app: &AppHandle) -> Arc<dyn Fn() + Send + Sync> {
    let app = app.clone();
    Arc::new(move || emit_status(&app))
}

/// Pushes status to the UI twice a second while the buffer runs and the page can be seen (live meters), and keeps
/// the "which game would a clip be named after" guess, shown in the UI, fresh. In the tray (most of the time, behind
/// a game) it does nothing: changes of state still reach the page and tray through on_change, and the page gets a
/// fresh status as soon as it's shown (page_visible). Saving works out the game itself.
pub fn status_pump(app: AppHandle) {
    std::thread::spawn(move || {
        let mut tick = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let st = app.state::<Arc<AppState>>();
            if !st.recorder.is_running() || !PAGE_VISIBLE.load(Ordering::SeqCst) {
                continue;
            }
            if tick % 4 == 0 {
                let game = winutil::current_game(&st.lib.game_names());
                *st.recorder.game.lock().unwrap() = Some(game);
            }
            tick += 1;
            emit_status(&app);
        }
    });
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let st = app.state::<Arc<AppState>>();
    let enabled = st.settings.lock().unwrap().recorder.enabled;
    let save_keys = st.settings.lock().unwrap().recorder.hotkeys.save.clone();
    let save = MenuItem::with_id(app, "save", "Save clip", true, if save_keys.is_empty() { None } else { Some(save_keys.as_str()) })?;
    let toggle = CheckMenuItem::with_id(app, "toggle", "Replay buffer", true, enabled, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Backtrack", true, None::<&str>)?;
    let restart_item = MenuItem::with_id(app, "restart", "Restart Backtrack", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Backtrack", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&save, &toggle, &sep, &open, &restart_item, &quit])?;
    *st.tray_toggle.lock().unwrap() = Some(toggle);
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("icon"))
        .tooltip("Backtrack")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "save" => run_action(app, "save"),
            "toggle" => run_action(app, "toggle"),
            "open" => show_main(app),
            "restart" => restart(app),
            "quit" => {
                app.state::<Arc<AppState>>().recorder.stop();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Tell WebView2 whether the main window's page can be seen. Tauri hides and minimizes the window without telling it,
/// and the page then goes on drawing at the monitor's refresh rate (with an animation running, a fifth of a GPU, all
/// the time Backtrack sits in the tray behind a game). A hidden page doesn't draw and its timers slow down; its
/// scripts and events still run, so edits still save.
pub fn page_visible(w: &tauri::WebviewWindow, visible: bool) {
    let page: &tauri::Webview = w.as_ref();
    let _ = if visible { page.show() } else { page.hide() };
    if !PAGE_VISIBLE.swap(visible, Ordering::SeqCst) && visible {
        // The status pump skips a hidden page: bring it up to date.
        emit_status(w.app_handle());
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        page_visible(&w, true);
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ------------------------------------------------------------------ corner pop-up

/// Small always-on-top, click-through, non-activating pop-up in the top-right of the main monitor. Shown on top of
/// borderless games, but never on top of one running exclusive full screen (it would minimize): then it goes on
/// another monitor, or isn't shown. One window is made the first time and reused: building a WebView2 window for
/// each pop-up, mid-game, cost far more than showing a hidden one.
pub fn toast(app: &AppHandle, title: &str, sub: &str, kind: &str) {
    static SHOWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static MAKING: Mutex<()> = Mutex::new(());
    let app = app.clone();
    let data = serde_json::json!({ "title": title, "sub": sub, "kind": kind }).to_string();
    let title = title.to_string();
    std::thread::spawn(move || {
        let (w, h) = (340.0, 76.0);
        // Over a borderless game is fine; only a game running exclusive full screen would minimize.
        let busy = winutil::fullscreen_monitor().filter(|_| winutil::exclusive_fullscreen());
        let free = |m: &tauri::Monitor| busy != Some((m.position().x, m.position().y));
        let monitor = match app.primary_monitor().ok().flatten() {
            Some(m) if free(&m) => Some(m),
            _ => app.available_monitors().unwrap_or_default().into_iter().find(|m| free(m)),
        };
        // (logical position for building the window, physical for moving it)
        let ((x, y), (px, py)) = match monitor {
            Some(m) => {
                let sf = m.scale_factor();
                let pos = m.position().to_logical::<f64>(sf);
                let size = m.size().to_logical::<f64>(sf);
                let (mp, ms) = (m.position(), m.size());
                ((pos.x + size.width - w - 20.0, pos.y + 20.0), (mp.x + ms.width as i32 - ((w + 20.0) * sf) as i32, mp.y + (20.0 * sf) as i32))
            }
            None if busy.is_some() => {
                util::log("info", &format!("pop-up not shown, a full-screen app is in front: {title}"));
                return;
            }
            None => ((40.0, 40.0), (40, 40)),
        };
        let shown = SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
        let hwnd = {
            let _making = MAKING.lock().unwrap();
            match app.get_webview_window("toast") {
                Some(win) => {
                    // New text, animations restarted; a moment to draw it, then show it where it belongs now.
                    let _ = win.eval(format!("show({data})"));
                    std::thread::sleep(Duration::from_millis(50));
                    let Ok(h) = win.hwnd() else { return };
                    winutil::show_no_activate(h.0, px, py);
                    h.0 as isize
                }
                None => {
                    let built = WebviewWindowBuilder::new(&app, "toast", WebviewUrl::App("toast.html".into()))
                        .initialization_script(format!("window.__TOAST = {data};"))
                        .title("Backtrack")
                        .inner_size(w, h)
                        .position(x, y)
                        .decorations(false)
                        .transparent(true)
                        .always_on_top(true)
                        .skip_taskbar(true)
                        .resizable(false)
                        .shadow(false)
                        .focused(false)
                        .focusable(false)
                        .build();
                    match built.map(|win| win.hwnd()) {
                        Ok(Ok(h)) => {
                            winutil::click_through(h.0);
                            h.0 as isize
                        }
                        Ok(Err(e)) | Err(e) => {
                            util::log("warn", &format!("toast: {e}"));
                            return;
                        }
                    }
                }
            }
        };
        std::thread::sleep(Duration::from_millis(3400));
        // Hidden until the next one, unless a newer pop-up is showing in it.
        if SHOWN.load(Ordering::SeqCst) == shown {
            winutil::hide(hwnd as *mut core::ffi::c_void);
        }
    });
}

// ------------------------------------------------------------------ actions (hotkeys, tray)

pub fn start_recorder(app: &AppHandle) {
    let st = app.state::<Arc<AppState>>().inner().clone();
    let app = app.clone();
    std::thread::spawn(move || {
        let cfg = st.settings.lock().unwrap().recorder.clone();
        let _ = std::fs::create_dir_all(&cfg.clips_dir);
        let Some(tools) = st.tools() else {
            st.recorder.fail(FFMPEG_MISSING);
            emit_status(&app);
            return;
        };
        let mon = monitor_size(&app, cfg.monitor);
        // Started by someone (or a driver reset): if it fails, it gets the full set of retries again.
        st.recorder.reset_failures();
        if let Err(e) = st.recorder.start(&tools, &cfg, mon, on_change_fn(&app)) {
            util::log("error", &format!("recorder start: {e}"));
            st.recorder.fail(&e);
        }
        emit_status(&app);
    });
}

pub fn stop_recorder(app: &AppHandle) {
    app.state::<Arc<AppState>>().recorder.stop();
    emit_status(app);
}

fn set_enabled(app: &AppHandle, enabled: bool) {
    let st = app.state::<Arc<AppState>>();
    {
        let mut s = st.settings.lock().unwrap();
        s.recorder.enabled = enabled;
        let _ = s.save();
    }
    if enabled {
        start_recorder(app);
    } else {
        stop_recorder(app);
    }
}

fn save_now(app: &AppHandle, secs: Option<f64>) -> Result<Saved, String> {
    let st = app.state::<Arc<AppState>>();
    let tools = st.tools().ok_or(FFMPEG_MISSING)?;
    let cfg = st.settings.lock().unwrap().recorder.clone();
    let game = winutil::current_game(&st.lib.game_names());
    let secs = secs.unwrap_or(cfg.buffer_secs as f64).clamp(3.0, cfg.buffer_secs as f64);
    let saved = st.recorder.save(&tools, secs, &game, &PathBuf::from(&cfg.clips_dir))?;
    if cfg.confirm {
        winutil::play_chime();
        // When Backtrack itself is in front, its in-app notice is enough.
        if !main_in_front(app) {
            toast(app, "Clip saved", &format!("{} · {:.0} s", saved.game, saved.duration), "ok");
        }
    }
    let _ = app.emit("clip-saved", &saved);
    emit_status(app);
    Ok(saved)
}

pub fn run_action(app: &AppHandle, action: &str) {
    let st = app.state::<Arc<AppState>>().inner().clone();
    let app = app.clone();
    let action = action.to_string();
    std::thread::spawn(move || match action.as_str() {
        "save" => {
            if st.recorder.state() == "error" {
                toast(&app, "Recording stopped", "Nothing to save · open Backtrack to restart it", "err");
                return;
            }
            if !st.recorder.is_running() {
                toast(&app, "Replay buffer is off", "Turn it on in Backtrack to save clips", "warn");
                return;
            }
            if let Err(e) = save_now(&app, None) {
                util::log("warn", &format!("save hotkey: {e}"));
                toast(&app, "Couldn't save the clip", &e, "err");
            }
        }
        "toggle" => {
            let on = !st.recorder.is_running();
            set_enabled(&app, on);
            toast(&app, if on { "Replay buffer on" } else { "Replay buffer off" }, if on { "Backtrack is keeping your last moments" } else { "Clips can't be saved until it's back on" }, "info");
        }
        "muteMic" => {
            let muted = !st.recorder.mic_muted.load(Ordering::SeqCst);
            st.recorder.mic_muted.store(muted, Ordering::SeqCst);
            toast(&app, if muted { "Mic muted in recordings" } else { "Mic back on in recordings" }, "Your voice chat isn't affected", "info");
            emit_status(&app);
        }
        "mark" => {
            st.recorder.add_marker();
            toast(&app, "Moment marked", "It shows on the timeline of your next clip", "mark");
        }
        "open" => show_main(&app),
        _ => {}
    });
}

// ------------------------------------------------------------------ hotkeys

/// "Ctrl+Alt+B" → a Shortcut. Accepts Ctrl/Control, Alt, Shift, Win/Super.
fn parse_shortcut(keys: &str) -> Result<Shortcut, String> {
    let norm: Vec<String> = keys
        .split('+')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| match t.to_lowercase().as_str() {
            "ctrl" | "control" => "Control".to_string(),
            "win" | "windows" | "meta" | "super" | "cmd" => "Super".to_string(),
            "up" => "ArrowUp".into(),
            "down" => "ArrowDown".into(),
            "left" => "ArrowLeft".into(),
            "right" => "ArrowRight".into(),
            "esc" => "Escape".into(),
            "del" => "Delete".into(),
            "ins" => "Insert".into(),
            "pgup" => "PageUp".into(),
            "pgdn" => "PageDown".into(),
            _ => t.to_string(),
        })
        .collect();
    norm.join("+").parse::<Shortcut>().map_err(|e| e.to_string())
}

pub fn register_hotkeys(app: &AppHandle) {
    let st = app.state::<Arc<AppState>>();
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let hk: Hotkeys = st.settings.lock().unwrap().recorder.hotkeys.clone();
    let mut statuses = Vec::new();
    let mut map = Vec::new();
    for (action, keys) in [("save", hk.save), ("toggle", hk.toggle), ("muteMic", hk.mute_mic), ("mark", hk.mark), ("open", hk.open)] {
        let mut status = HotkeyStatus { action: action.into(), keys: keys.clone(), ok: true, error: None };
        if !keys.trim().is_empty() {
            match parse_shortcut(&keys) {
                Ok(sc) => match gs.register(sc) {
                    Ok(()) => map.push((sc.id(), action)),
                    Err(e) => {
                        util::log("warn", &format!("hotkey {keys} for {action}: {e}"));
                        status.ok = false;
                        status.error = Some(format!("{keys} is already used by another app. Pick another key."));
                    }
                },
                Err(_) => {
                    status.ok = false;
                    status.error = Some(format!("{keys} isn't a shortcut Windows can use."));
                }
            }
        }
        statuses.push(status);
    }
    *st.hotkey_map.lock().unwrap() = map;
    *st.recorder.hotkeys.lock().unwrap() = statuses;
}

/// Called from the global-shortcut plugin's handler.
pub fn on_hotkey(app: &AppHandle, shortcut: &Shortcut) {
    let st = app.state::<Arc<AppState>>();
    let action = st.hotkey_map.lock().unwrap().iter().find(|(id, _)| *id == shortcut.id()).map(|(_, a)| *a);
    if let Some(a) = action {
        run_action(app, a);
    }
}

/// Apply recorder setting changes after save_settings.
pub fn apply_settings(app: &AppHandle, old: &RecorderSettings, new: &RecorderSettings) {
    if old.hotkeys != new.hotkeys {
        register_hotkeys(app);
    }
    if old.start_with_windows != new.start_with_windows {
        if let Err(e) = winutil::set_start_with_windows(new.start_with_windows) {
            util::log("warn", &format!("start with Windows: {e}"));
        }
    }
    let running = app.state::<Arc<AppState>>().recorder.is_running();
    if new.enabled && (!running || old.pipeline_key() != new.pipeline_key()) {
        start_recorder(app);
    } else if !new.enabled && running {
        stop_recorder(app);
    } else {
        emit_status(app);
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn recorder_status(state: St) -> RecorderStatus {
    state.recorder.status()
}

#[tauri::command]
pub fn set_buffer(app: AppHandle, state: St, enabled: bool) -> RecorderStatus {
    set_enabled(&app, enabled);
    state.recorder.status()
}

#[tauri::command]
pub async fn save_replay(app: AppHandle, secs: Option<f64>) -> Result<Saved, String> {
    tauri::async_runtime::spawn_blocking(move || save_now(&app, secs)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn toggle_mic_mute(app: AppHandle, state: St) -> RecorderStatus {
    let m = !state.recorder.mic_muted.load(Ordering::SeqCst);
    state.recorder.mic_muted.store(m, Ordering::SeqCst);
    emit_status(&app);
    state.recorder.status()
}

#[tauri::command]
pub fn add_marker(state: St) {
    state.recorder.add_marker();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatApp {
    name: String,
    running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevices {
    inputs: Vec<crate::audio::InputDevice>,
    chat_apps: Vec<ChatApp>,
}

#[tauri::command]
pub async fn list_audio_devices() -> Result<AudioDevices, String> {
    tauri::async_runtime::spawn_blocking(|| AudioDevices {
        inputs: crate::audio::list_inputs(),
        chat_apps: vec![ChatApp { name: "Discord".into(), running: crate::audio::chat_root("Discord").is_some() }],
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_monitors(app: AppHandle) -> Vec<MonitorInfo> {
    monitors(&app)
}

/// While Settings captures a new combo, release our global hotkeys so the key press reaches the page.
#[tauri::command]
pub fn suspend_hotkeys(app: AppHandle, suspended: bool) {
    if suspended {
        let _ = app.global_shortcut().unregister_all();
    } else {
        register_hotkeys(&app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::TrackStatus;

    fn status(state: &str, filled: f64, level: f32) -> RecorderStatus {
        RecorderStatus {
            state: state.into(), error: None, buffer_secs: 120, filled_secs: filled, memory_mb: filled * 3.0,
            encoder: "AMD AMF".into(), width: 1920, height: 1080, fps: 60, game: Some("Rocket League".into()),
            mic_muted: false,
            tracks: vec![TrackStatus { name: "Game".into(), source: "Game".into(), level, active: true }],
            hotkeys: vec![], last_saved: None,
        }
    }

    /// The status goes out twice a second while recording, but the tray's check mark and tooltip only change with the
    /// state: each update is a round trip to the main thread and a message to Explorer, so they're sent only then.
    #[test]
    fn tray_is_only_updated_when_its_text_or_check_changes() {
        let mut last = None;
        let on = tray_state(&status("on", 10.0, 0.2));
        assert!(tray_changed(&mut last, &on), "first status");
        // Ticks of the live meters (buffer filling, levels, memory) leave the tray alone.
        for i in 0..10 {
            let tick = tray_state(&status("on", 10.5 + i as f64, (i as f32) / 10.0));
            assert!(!tray_changed(&mut last, &tick), "tick {i} updated the tray: {tick:?}");
        }
        assert_eq!(on, (true, "Backtrack · replay buffer on (last 120 s)".to_string()));
        let off = tray_state(&status("off", 0.0, 0.0));
        assert_eq!(off, (false, "Backtrack · replay buffer off".to_string()));
        assert!(tray_changed(&mut last, &off), "turned off");
        assert!(!tray_changed(&mut last, &off));
        assert!(tray_state(&status("starting", 0.0, 0.0)).0, "starting shows as on");
        let mut failed = status("error", 0.0, 0.0);
        failed.error = Some("ffmpeg stopped".into());
        let err = tray_state(&failed);
        assert_eq!(err, (false, "Backtrack · ffmpeg stopped".to_string()));
        assert!(tray_changed(&mut last, &err), "a new error");
    }
}
