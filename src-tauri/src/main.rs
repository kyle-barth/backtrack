// Backtrack: a local clip library, trimmer and Discord exporter. Everything runs on this PC.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod export;
mod ffmpeg;
mod library;
mod media;
mod rec_app;
mod recorder;
mod settings;
mod sys;
mod update;
mod util;
mod winutil;

use export::{ExportRequest, ExportResult, Job};
use ffmpeg::Tools;
use library::{Clip, Edits, ExportRecord, Library};
use media::PreparedTrack;
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use settings::Settings;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const FFMPEG_MISSING: &str = "FFmpeg wasn't found. Install it (winget install Gyan.FFmpeg.Essentials) and restart Backtrack.";

struct AppState {
    settings: Mutex<Settings>,
    tools: Mutex<Option<Tools>>,
    lib: Library,
    job: Job,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    thumb_failed: Mutex<HashSet<String>>,
    recorder: Arc<recorder::Recorder>,
    /// Registered global shortcut ids → action.
    hotkey_map: Mutex<Vec<(u32, &'static str)>>,
    tray_toggle: Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>,
    told_tray: std::sync::atomic::AtomicBool,
    /// Clips this run moved to the Recycle Bin: id → (original path, what the library dropped), for Undo.
    binned: Mutex<HashMap<String, (String, library::Forgotten)>>,
}

type St<'a> = State<'a, Arc<AppState>>;

impl AppState {
    /// The resolved ffmpeg tools; looks again if they weren't found earlier (e.g. installed since).
    fn tools(&self) -> Option<Tools> {
        let mut t = self.tools.lock().unwrap();
        if t.is_none() {
            *t = ffmpeg::locate();
        }
        t.clone()
    }

    fn settings(&self) -> Settings {
        let mut s = self.settings.lock().unwrap().clone();
        s.ffmpeg_path = self.tools().map(|t| t.ffmpeg.to_string_lossy().into_owned());
        s
    }

    fn scan(&self) -> Vec<Clip> {
        let dirs = self.settings.lock().unwrap().clip_dirs.clone();
        self.lib.scan(self.tools().as_ref(), &dirs)
    }

    fn clip(&self, id: &str) -> Result<Clip, String> {
        self.lib.get(id).ok_or_else(|| "That clip is no longer in the library.".to_string())
    }

    /// A clip went to the Recycle Bin: drop it from the library, but remember it so Undo can bring it back.
    fn binned(&self, id: &str, path: &str) {
        if let Some(f) = self.lib.forget(id) {
            self.binned.lock().unwrap().insert(id.to_string(), (path.to_string(), f));
        }
    }
}

/// Run blocking work off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// Let the webview load files from these folders through the asset protocol.
fn allow_dirs(app: &AppHandle, dirs: &[String]) {
    let scope = app.asset_protocol_scope();
    for d in dirs {
        if Path::new(d).is_dir() {
            let _ = scope.allow_directory(d, true);
        }
    }
}

fn allowed_dirs(st: &AppState) -> Vec<String> {
    let s = st.settings.lock().unwrap();
    let mut v = s.clip_dirs.clone();
    v.push(s.export_dir.clone());
    v.push(util::cache_dir().to_string_lossy().into_owned());
    v
}

/// Watch clip folders; emit `library-changed` once writes settle (a new clip can take a few seconds to save).
fn start_watcher(app: &AppHandle, st: &Arc<AppState>) {
    let dirs = st.settings.lock().unwrap().clip_dirs.clone();
    let (tx, rx) = mpsc::channel::<()>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let is_clip = ev.paths.iter().any(|p| {
                let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                matches!(e.as_str(), "mp4" | "mov" | "mkv")
            });
            if is_clip {
                let _ = tx.send(());
            }
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            util::log("error", &format!("watcher: {e}"));
            return;
        }
    };
    for d in &dirs {
        if let Err(e) = watcher.watch(Path::new(d), RecursiveMode::NonRecursive) {
            util::log("warn", &format!("can't watch {d}: {e}"));
        }
    }
    // Replacing the old watcher drops its sender, which ends its debounce thread.
    *st.watcher.lock().unwrap() = Some(watcher);
    let app = app.clone();
    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            loop {
                match rx.recv_timeout(Duration::from_millis(2500)) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let _ = app.emit("library-changed", ());
        }
    });
}

/// Generate thumbnails for clips that don't have one (GG usually provides them).
fn spawn_thumbs(app: AppHandle, st: Arc<AppState>) {
    if st.lib.thumbs_busy.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let failed = st.thumb_failed.lock().unwrap().clone();
        let missing: Vec<_> = st.lib.missing_thumbs().into_iter().filter(|(id, _, _)| !failed.contains(id)).collect();
        let mut made = 0;
        if let (Some(tools), false) = (st.tools(), missing.is_empty()) {
            let dir = util::cache_sub("thumbs");
            for (id, path, dur) in missing {
                if media::generate_thumb(&tools, &path, dur, &dir.join(format!("{id}.jpg"))) {
                    made += 1;
                } else {
                    st.thumb_failed.lock().unwrap().insert(id);
                }
            }
        }
        st.lib.thumbs_busy.store(false, Ordering::SeqCst);
        if made > 0 {
            st.scan();
            let _ = app.emit("library-changed", ());
        }
    });
}

// ------------------------------------------------------------------ commands

#[tauri::command]
fn get_settings(state: St) -> Settings {
    state.settings()
}

#[tauri::command]
async fn save_settings(app: AppHandle, state: St<'_>, settings: Settings) -> Result<Settings, String> {
    let st = state.inner().clone();
    let mut s = settings;
    s.normalize();
    s.save()?;
    let _ = std::fs::create_dir_all(&s.recorder.clips_dir);
    let old = st.settings.lock().unwrap().clone();
    let dirs_changed = old.clip_dirs != s.clip_dirs;
    let new_rec = s.recorder.clone();
    *st.settings.lock().unwrap() = s;
    allow_dirs(&app, &allowed_dirs(&st));
    if dirs_changed {
        start_watcher(&app, &st);
        let _ = app.emit("library-changed", ());
    }
    rec_app::apply_settings(&app, &old.recorder, &new_rec);
    Ok(st.settings())
}

#[tauri::command]
async fn list_clips(app: AppHandle, state: St<'_>) -> Result<Vec<Clip>, String> {
    let st = state.inner().clone();
    let st2 = st.clone();
    let clips = blocking(move || Ok(st2.scan())).await?;
    spawn_thumbs(app, st);
    Ok(clips)
}

#[tauri::command]
fn set_favourite(state: St, clip_id: String, fav: bool) -> Result<(), String> {
    state.lib.set_fav(&clip_id, fav)
}

#[tauri::command]
fn save_edits(state: St, clip_id: String, edits: Edits) -> Result<(), String> {
    state.lib.save_edits(&clip_id, edits)
}

#[tauri::command]
async fn get_waveform(state: St<'_>, clip_id: String, track: usize, points: usize) -> Result<Vec<f32>, String> {
    let st = state.inner().clone();
    blocking(move || {
        let tools = st.tools().ok_or(FFMPEG_MISSING)?;
        let clip = st.clip(&clip_id)?;
        media::waveform(&st.lib, &tools, &clip, track, points)
    })
    .await
}

#[tauri::command]
async fn get_keyframes(state: St<'_>, clip_id: String) -> Result<Vec<f64>, String> {
    let st = state.inner().clone();
    blocking(move || {
        let tools = st.tools().ok_or(FFMPEG_MISSING)?;
        let clip = st.clip(&clip_id)?;
        media::keyframes(&tools, &clip).map(|k| k.as_ref().clone())
    })
    .await
}

#[tauri::command]
async fn prepare_audio(state: St<'_>, clip_id: String) -> Result<Vec<PreparedTrack>, String> {
    let st = state.inner().clone();
    blocking(move || {
        let tools = st.tools().ok_or(FFMPEG_MISSING)?;
        let clip = st.clip(&clip_id)?;
        media::prepare_audio(&tools, &clip)
    })
    .await
}

#[tauri::command]
async fn export_clip(app: AppHandle, state: St<'_>, req: ExportRequest) -> Result<ExportResult, String> {
    let st = state.inner().clone();
    if st.job.running.swap(true, Ordering::SeqCst) {
        return Err("Another export is already running.".into());
    }
    st.job.cancel.store(false, Ordering::SeqCst);
    let st2 = st.clone();
    let result = blocking(move || {
        let tools = st2.tools().ok_or(FFMPEG_MISSING)?;
        let clip = st2.clip(&req.clip_id)?;
        let out_dir = PathBuf::from(st2.settings.lock().unwrap().export_dir.clone());
        let ctx = export::ExportCtx { tools: &tools, clip: &clip, out_dir, job: &st2.job };
        let emitter = app.clone();
        let (res, _plan) = export::export(&ctx, &req, |p| {
            let _ = emitter.emit("export-progress", p);
        })?;
        let _ = st2.lib.add_export(
            &clip.id,
            ExportRecord {
                path: res.path.clone(),
                size_bytes: res.size_bytes,
                preset: req.preset.clone(),
                width: res.width,
                height: res.height,
                fps: res.fps,
                at: util::now_ms(),
            },
        );
        allow_dirs(&app, &allowed_dirs(&st2));
        util::log("info", &format!("exported {} ({} bytes in {:.1}s)", res.path, res.size_bytes, res.elapsed_sec));
        Ok(res)
    })
    .await;
    st.job.running.store(false, Ordering::SeqCst);
    result
}

#[tauri::command]
fn cancel_export(state: St) {
    state.job.cancel();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedCopy {
    path: String,
    size_bytes: u64,
}

#[tauri::command]
async fn save_trimmed_copy(state: St<'_>, clip_id: String, start: f64, end: f64) -> Result<SavedCopy, String> {
    let st = state.inner().clone();
    if st.job.running.swap(true, Ordering::SeqCst) {
        return Err("An export is running; try again when it finishes.".into());
    }
    st.job.cancel.store(false, Ordering::SeqCst);
    let st2 = st.clone();
    let result = blocking(move || {
        let tools = st2.tools().ok_or(FFMPEG_MISSING)?;
        let clip = st2.clip(&clip_id)?;
        let out_dir = PathBuf::from(st2.settings.lock().unwrap().export_dir.clone());
        let out = export::save_trimmed_copy(&tools, &clip, start, end, &out_dir, &st2.job)?;
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        let _ = st2.lib.add_export(
            &clip.id,
            ExportRecord {
                path: out.to_string_lossy().into_owned(),
                size_bytes: size,
                preset: "lossless".into(),
                width: clip.width,
                height: clip.height,
                fps: clip.fps.round() as u32,
                at: util::now_ms(),
            },
        );
        Ok(SavedCopy { path: out.to_string_lossy().into_owned(), size_bytes: size })
    })
    .await;
    st.job.running.store(false, Ordering::SeqCst);
    result
}

#[tauri::command]
fn copy_file(path: String) -> Result<(), String> {
    sys::copy_file(&path)
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    sys::reveal(&path)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    sys::open_path(&path)
}

#[tauri::command]
fn open_discord() -> Result<(), String> {
    sys::open_discord()
}

/// Drag a file out of the window as if from Explorer (e.g. an export into a Discord chat), with `icon` (a thumbnail)
/// under the cursor. The drag runs on the window's thread and lasts until the file is dropped or the drag cancelled.
#[tauri::command]
fn start_drag(window: tauri::WebviewWindow, path: String, icon: Option<String>) -> Result<(), String> {
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err("The file isn't there any more".into());
    }
    let image = match icon.filter(|i| Path::new(i).is_file()) {
        Some(i) => drag::Image::File(i.into()),
        None => drag::Image::Raw(include_bytes!("../icons/icon.png").to_vec()),
    };
    let w = window.clone();
    window
        .run_on_main_thread(move || {
            if let Err(e) = drag::start_drag(&w, drag::DragItem::Files(vec![file]), image, |_, _| {}, drag::Options::default()) {
                util::log("warn", &format!("drag: {e}"));
            }
        })
        .map_err(|e| e.to_string())
}

/// Windows' taskbar settings. Only its "Other system tray icons" switches can put an app's icon next to the clock:
/// Windows 11 doesn't let apps do it themselves (the NotifyIconSettings registry values are only a copy).
#[tauri::command]
fn open_taskbar_settings() -> Result<(), String> {
    open::that_detached("ms-settings:taskbar").map_err(|e| e.to_string())
}

#[tauri::command]
fn trash_clip(app: AppHandle, state: St, clip_id: String) -> Result<(), String> {
    let clip = state.clip(&clip_id)?;
    sys::trash(&clip.path)?;
    state.binned(&clip_id, &clip.path);
    util::log("info", &format!("moved to Recycle Bin: {}", clip.path));
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrashFailure {
    clip_id: String,
    error: String,
}

#[derive(Serialize)]
struct TrashResult {
    trashed: Vec<String>,
    failed: Vec<TrashFailure>,
}

/// Move several clips to the Recycle Bin, or with `permanent` delete them outright (only when the user chose that).
/// Per-clip failures are reported, not thrown.
#[tauri::command]
async fn trash_clips(app: AppHandle, state: St<'_>, clip_ids: Vec<String>, permanent: Option<bool>) -> Result<TrashResult, String> {
    if clip_ids.is_empty() {
        return Err("No clips were selected.".into());
    }
    let permanent = permanent.unwrap_or(false);
    let st = state.inner().clone();
    let res = blocking(move || {
        let mut out = TrashResult { trashed: Vec::new(), failed: Vec::new() };
        let mut found = Vec::new();
        for id in clip_ids {
            match st.clip(&id) {
                Ok(c) => found.push((id, c.path)),
                Err(error) => out.failed.push(TrashFailure { clip_id: id, error }),
            }
        }
        let paths: Vec<String> = found.iter().map(|(_, p)| p.clone()).collect();
        let results = if permanent { sys::delete_all(&paths) } else { sys::trash_all(&paths) };
        let what = if permanent { "deleted" } else { "moved to Recycle Bin" };
        for ((id, path), r) in found.into_iter().zip(results) {
            match r {
                Ok(()) => {
                    if permanent {
                        st.lib.forget(&id);
                    } else {
                        st.binned(&id, &path);
                    }
                    util::log("info", &format!("{what}: {path}"));
                    out.trashed.push(id);
                }
                Err(error) => {
                    util::log("warn", &format!("not {what}: {path}: {error}"));
                    out.failed.push(TrashFailure { clip_id: id, error });
                }
            }
        }
        Ok(out)
    })
    .await?;
    let _ = app.emit("library-changed", ());
    Ok(res)
}

#[derive(Serialize)]
struct RestoreResult {
    restored: Vec<String>,
    failed: Vec<TrashFailure>,
}

/// Undo: put clips this run moved to the Recycle Bin back where they were, with their favourite, edits and exports.
#[tauri::command]
async fn restore_clips(app: AppHandle, state: St<'_>, clip_ids: Vec<String>) -> Result<RestoreResult, String> {
    let st = state.inner().clone();
    let res = blocking(move || {
        let mut out = RestoreResult { restored: Vec::new(), failed: Vec::new() };
        let mut known = Vec::new();
        {
            let binned = st.binned.lock().unwrap();
            for id in clip_ids {
                match binned.get(&id) {
                    Some((path, _)) => known.push((id, path.clone())),
                    None => out.failed.push(TrashFailure { clip_id: id, error: "Backtrack didn't move this clip to the Recycle Bin".into() }),
                }
            }
        }
        let paths: Vec<String> = known.iter().map(|(_, p)| p.clone()).collect();
        for ((id, path), r) in known.into_iter().zip(sys::restore_all(&paths)?) {
            match r {
                Ok(()) => {
                    if let Some((_, f)) = st.binned.lock().unwrap().remove(&id) {
                        st.lib.remember(f);
                    }
                    util::log("info", &format!("restored from Recycle Bin: {path}"));
                    out.restored.push(id);
                }
                Err(error) => {
                    util::log("warn", &format!("couldn't restore {path}: {error}"));
                    out.failed.push(TrashFailure { clip_id: id, error });
                }
            }
        }
        Ok(out)
    })
    .await?;
    let _ = app.emit("library-changed", ());
    Ok(res)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DriveCheck {
    drive: String,
    bytes: u64,
    capacity: Option<u64>,
    permanent: bool,
    /// The clips (of those asked about) that are on this drive.
    clip_ids: Vec<String>,
}

#[derive(Serialize)]
struct TrashCheck {
    fits: bool,
    drives: Vec<DriveCheck>,
}

/// Before a delete: can the Recycle Bin of each drive hold these clips? If a batch is bigger than a drive's
/// Recycle Bin, Windows makes room by permanently deleting the oldest items, which can be clips from the same batch.
#[tauri::command]
fn trash_check(state: St, clip_ids: Vec<String>) -> TrashCheck {
    let mut drives: Vec<DriveCheck> = Vec::new();
    for c in clip_ids.iter().filter_map(|id| state.lib.get(id)) {
        let bin = sys::recycle_bin(&c.path);
        match drives.iter_mut().find(|d| d.drive.eq_ignore_ascii_case(&bin.drive)) {
            Some(d) => {
                d.bytes += c.size_bytes;
                d.clip_ids.push(c.id);
            }
            None => drives.push(DriveCheck {
                drive: bin.drive,
                bytes: c.size_bytes,
                capacity: bin.capacity,
                permanent: bin.permanent,
                clip_ids: vec![c.id],
            }),
        }
    }
    let fits = drives.iter().all(|d| !d.permanent && d.capacity.map_or(true, |cap| d.bytes <= cap));
    TrashCheck { fits, drives }
}

#[tauri::command]
fn set_favourites(state: St, clip_ids: Vec<String>, fav: bool) -> Result<(), String> {
    state.lib.set_favs(&clip_ids, fav)
}

#[tauri::command]
fn log(level: String, msg: String) {
    util::log(&format!("ui:{level}"), &msg);
}

fn main() {
    util::log("info", "Backtrack starting");
    let settings = Settings::load();
    let state = Arc::new(AppState {
        settings: Mutex::new(settings),
        tools: Mutex::new(ffmpeg::locate()),
        lib: Library::load(),
        job: Job::default(),
        watcher: Mutex::new(None),
        thumb_failed: Mutex::new(HashSet::new()),
        recorder: recorder::Recorder::new(),
        hotkey_map: Mutex::new(Vec::new()),
        tray_toggle: Mutex::new(None),
        told_tray: std::sync::atomic::AtomicBool::new(false),
        binned: Mutex::new(HashMap::new()),
    });
    match &*state.tools.lock().unwrap() {
        Some(t) => util::log("info", &format!("ffmpeg: {}", t.ffmpeg.display())),
        None => util::log("warn", "ffmpeg not found"),
    }
    let background = std::env::args().any(|a| a == "--background");

    tauri::Builder::default()
        // A second launch just brings the running Backtrack to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| rec_app::show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        rec_app::on_hotkey(app, shortcut);
                    }
                })
                .build(),
        )
        .manage(state.clone())
        .on_window_event(|window, event| {
            // Minimized or back: the page stops or starts drawing (rec_app::page_visible).
            if let tauri::WindowEvent::Resized(_) = event {
                static MINIMIZED: AtomicBool = AtomicBool::new(false);
                let minimized = window.is_minimized().unwrap_or(false);
                if window.label() == "main" && MINIMIZED.swap(minimized, Ordering::SeqCst) != minimized && window.is_visible().unwrap_or(false) {
                    if let Some(w) = window.app_handle().get_webview_window("main") {
                        rec_app::page_visible(&w, !minimized);
                    }
                }
            }
            // Closing the main window while the buffer runs keeps Backtrack recording in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let app = window.app_handle();
                    let st = app.state::<Arc<AppState>>();
                    if st.recorder.is_running() {
                        api.prevent_close();
                        let _ = window.hide();
                        if let Some(w) = app.get_webview_window("main") {
                            rec_app::page_visible(&w, false);
                        }
                        if !st.told_tray.swap(true, Ordering::SeqCst) {
                            rec_app::toast(app, "Still recording in the tray", "Right-click the tray icon to quit", "info");
                        }
                    } else {
                        app.exit(0);
                    }
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = std::fs::create_dir_all(&state.settings.lock().unwrap().recorder.clips_dir);
            allow_dirs(&handle, &allowed_dirs(&state));
            start_watcher(&handle, &state);
            // Warm the library in the background so the first list_clips is quick.
            let st = state.clone();
            std::thread::spawn(move || {
                st.scan();
            });
            if let Some(win) = app.get_webview_window("main") {
                // Fit the window on smaller or scaled displays.
                if let Ok(Some(m)) = win.current_monitor() {
                    let size = m.size().to_logical::<f64>(m.scale_factor());
                    if size.width < 1500.0 || size.height < 960.0 {
                        let _ = win.maximize();
                    }
                }
                // Started with Windows: stay in the tray, with the page not drawing until it's opened.
                rec_app::page_visible(&win, !background);
                if !background {
                    let _ = win.show();
                }
            }
            if let Err(e) = rec_app::build_tray(&handle) {
                util::log("error", &format!("tray: {e}"));
            }
            rec_app::register_hotkeys(&handle);
            rec_app::status_pump(handle.clone());
            if state.settings.lock().unwrap().recorder.enabled {
                rec_app::start_recorder(&handle);
            }
            // "Start with Windows": rewrite the Run entry with this exe's path on every launch. That way it
            // follows the app when it moves (portable zip → installer), and comes back after an upgrade's
            // uninstall step removed it.
            if state.settings.lock().unwrap().recorder.start_with_windows {
                std::thread::spawn(|| {
                    if let Err(e) = winutil::set_start_with_windows(true) {
                        util::log("warn", &format!("start with Windows: {e}"));
                    }
                });
            }
            // Updates: an installed copy installs a newer release, if there is one, as it starts.
            update::announce_if_updated(&handle);
            if state.settings.lock().unwrap().recorder.auto_update {
                update::check_at_launch(&handle);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_clips,
            set_favourite,
            save_edits,
            get_waveform,
            get_keyframes,
            prepare_audio,
            export_clip,
            cancel_export,
            save_trimmed_copy,
            copy_file,
            reveal,
            open_path,
            open_discord,
            open_taskbar_settings,
            start_drag,
            trash_clip,
            trash_clips,
            trash_check,
            restore_clips,
            set_favourites,
            log,
            rec_app::recorder_status,
            rec_app::set_buffer,
            rec_app::save_replay,
            rec_app::toggle_mic_mute,
            rec_app::add_marker,
            rec_app::list_audio_devices,
            rec_app::list_monitors,
            rec_app::suspend_hotkeys
        ])
        .build(tauri::generate_context!())
        .expect("error while building Backtrack")
        .run(|app, event| {
            // Make sure the capture process never outlives the app.
            if let tauri::RunEvent::Exit = event {
                app.state::<Arc<AppState>>().recorder.stop();
            }
        });
}

#[cfg(test)]
mod e2e {
    //! Runs against the real SteelSeries Moments folder: `cargo test -- --ignored --nocapture`
    use super::*;

    #[test]
    #[ignore]
    fn scan_waveform_audio_and_export_real_clip() {
        let tools = ffmpeg::locate().expect("ffmpeg");
        let s = Settings::default();
        assert!(!s.clip_dirs.is_empty(), "no SteelSeries Moments folder");
        let lib = Library::load();
        let t = std::time::Instant::now();
        let clips = lib.scan(Some(&tools), &s.clip_dirs);
        println!("scanned {} clips in {:.1}s", clips.len(), t.elapsed().as_secs_f64());
        let games: HashSet<&str> = clips.iter().map(|c| c.game.as_str()).collect();
        println!("games: {games:?}");
        println!("missing thumbs: {}", lib.missing_thumbs().len());
        let clip = clips.iter().find(|c| c.file_name == "Rocket-League__2026-11-20__00-01-27.mp4").expect("RL clip");
        println!("clip: {} {}x{}@{:.0} {:.2}s tracks={:?} edits={:?} thumb={:?}",
            clip.game, clip.width, clip.height, clip.fps, clip.duration,
            clip.audio_tracks.iter().map(|a| &a.name).collect::<Vec<_>>(), clip.edits, clip.thumb.is_some());

        let t = std::time::Instant::now();
        let w = media::waveform(&lib, &tools, clip, 2, 500).unwrap();
        println!("waveform mic: {} pts, max {:.2}, {:.2}s", w.len(), w.iter().cloned().fold(0.0, f32::max), t.elapsed().as_secs_f64());
        let t = std::time::Instant::now();
        let tracks = media::prepare_audio(&tools, clip).unwrap();
        println!("prepared {} tracks in {:.2}s", tracks.len(), t.elapsed().as_secs_f64());

        let out_dir = util::cache_sub("e2e-exports");
        let job = Job::default();
        let ctx = export::ExportCtx { tools: &tools, clip, out_dir: out_dir.clone(), job: &job };
        let edits = clip.edits.clone().unwrap_or(Edits { in_t: 24.0, out_t: 48.0, markers: vec![] });
        for (preset, limit, canvas) in [("d10", Some(10.0), "16:9"), ("d10", Some(10.0), "9:16")] {
            let req = ExportRequest {
                clip_id: clip.id.clone(), start: edits.in_t, end: edits.out_t, preset: preset.into(), limit_mb: limit,
                canvas: canvas.into(), crop_x: 0.5,
                tracks: vec![
                    export::TrackMix { index: 0, volume: 1.0, muted: false },
                    export::TrackMix { index: 1, volume: 0.8, muted: false },
                    export::TrackMix { index: 2, volume: 0.7, muted: true },
                ],
                file_name: format!("e2e_{preset}_{}", canvas.replace(':', "x")), copy_to_clipboard: false,
            };
            let mut last = 0.0;
            let (res, plan) = export::export(&ctx, &req, |p| last = p.percent).unwrap();
            println!("{preset} {canvas}: {:.2} MB {}x{}@{} v={}k a={}k mode={} in {:.1}s (plan v={}k) last%={last}",
                res.size_bytes as f64 / 1e6, res.width, res.height, res.fps, res.video_kbps, res.audio_kbps, res.mode,
                res.elapsed_sec, plan.video_kbps);
            assert!(res.size_bytes as f64 <= 10e6, "over 10 MB");
            assert!(res.size_bytes as f64 >= 8.0e6, "wasted the budget");
            let probe = tools.ffprobe_cmd().args(["-v", "error", "-show_entries", "stream=codec_name,width,height,r_frame_rate:format=duration", "-of", "compact"]).arg(&res.path).output().unwrap();
            println!("{}", String::from_utf8_lossy(&probe.stdout));
        }
    }
}
