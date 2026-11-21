//! App-level wiring for the recorder: commands, global hotkeys, the tray icon, the corner pop-up, and the
//! status pump that feeds the UI's live meters.

use crate::recorder::{HotkeyStatus, RecorderStatus, Saved};
use crate::settings::{Hotkeys, RecorderSettings};
use crate::{util, winutil, AppState, FFMPEG_MISSING};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
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

fn emit_status(app: &AppHandle) {
    let st = app.state::<Arc<AppState>>();
    let status = st.recorder.status();
    let on = status.state == "on" || status.state == "starting";
    if let Some(item) = st.tray_toggle.lock().unwrap().as_ref() {
        let _ = item.set_checked(on);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let tip = match status.state.as_str() {
            "on" => format!("Backtrack · replay buffer on (last {} s)", status.buffer_secs),
            "starting" => "Backtrack · starting the replay buffer".into(),
            "error" => format!("Backtrack · {}", status.error.clone().unwrap_or_default()),
            _ => "Backtrack · replay buffer off".into(),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
    let _ = app.emit("recorder-status", status);
}

fn on_change_fn(app: &AppHandle) -> Arc<dyn Fn() + Send + Sync> {
    let app = app.clone();
    Arc::new(move || emit_status(&app))
}

/// Pushes status to the UI twice a second while the buffer runs (live meters), and keeps the
/// "which game would a clip be named after" guess fresh.
pub fn status_pump(app: AppHandle) {
    std::thread::spawn(move || {
        let mut tick = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let st = app.state::<Arc<AppState>>();
            if !st.recorder.is_running() {
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
    let quit = MenuItem::with_id(app, "quit", "Quit Backtrack", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&save, &toggle, &sep, &open, &quit])?;
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

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ------------------------------------------------------------------ corner pop-up

/// Small always-on-top, click-through, non-activating pop-up in the top-right of the main monitor. Never on top of a
/// full-screen game (it would minimize): then it goes on another monitor, or isn't shown.
pub fn toast(app: &AppHandle, title: &str, sub: &str, kind: &str) {
    let app = app.clone();
    let data = serde_json::json!({ "title": title, "sub": sub, "kind": kind }).to_string();
    let title = title.to_string();
    std::thread::spawn(move || {
        let (w, h) = (340.0, 76.0);
        let busy = winutil::fullscreen_monitor();
        let free = |m: &tauri::Monitor| busy != Some((m.position().x, m.position().y));
        let monitor = match app.primary_monitor().ok().flatten() {
            Some(m) if free(&m) => Some(m),
            _ => app.available_monitors().unwrap_or_default().into_iter().find(|m| free(m)),
        };
        let (x, y) = match monitor {
            Some(m) => {
                let sf = m.scale_factor();
                let pos = m.position().to_logical::<f64>(sf);
                let size = m.size().to_logical::<f64>(sf);
                (pos.x + size.width - w - 20.0, pos.y + 20.0)
            }
            None if busy.is_some() => {
                util::log("info", &format!("pop-up not shown, a full-screen app is in front: {title}"));
                return;
            }
            None => (40.0, 40.0),
        };
        let label = format!("toast{}", util::now_ms());
        let built = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("toast.html".into()))
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
        match built {
            Ok(win) => {
                if let Ok(h) = win.hwnd() {
                    winutil::click_through(h.0);
                }
                std::thread::sleep(Duration::from_millis(3400));
                let _ = win.close();
            }
            Err(e) => util::log("warn", &format!("toast: {e}")),
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
        let in_front = app.get_webview_window("main").map_or(false, |w| w.is_focused().unwrap_or(false) && w.is_visible().unwrap_or(false));
        if !in_front {
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
