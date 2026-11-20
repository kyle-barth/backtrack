//! The clip library: scans clip folders, probes each clip once with ffprobe (cached by size + mtime),
//! reads SteelSeries GG's embedded metadata (game name, trim points, thumbnail), and keeps the user's
//! own data (favourites, In/Out edits, export history) in %APPDATA%\Backtrack\store.json.

use crate::ffmpeg::Tools;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const CLIP_EXTS: [&str; 3] = ["mp4", "mov", "mkv"];

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub index: usize,
    pub name: String,
    pub channels: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Edits {
    pub in_t: f64,
    pub out_t: f64,
    #[serde(default)]
    pub markers: Vec<f64>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportRecord {
    pub path: String,
    pub size_bytes: u64,
    pub preset: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub at: u64,
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub path: String,
    pub file_name: String,
    pub game: String,
    pub game_id: String,
    pub recorded_at: u64,
    pub duration: f64,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_kbps: f64,
    pub audio_tracks: Vec<AudioTrack>,
    pub thumb: Option<String>,
    pub fav: bool,
    pub edits: Option<Edits>,
    pub exports: Vec<ExportRecord>,
    pub source: String,
}

/// Cached ffprobe result for one file.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
struct Probe {
    size: u64,
    mtime: u64,
    created: u64,
    duration: f64,
    width: u32,
    height: u32,
    fps: f64,
    video_kbps: f64,
    audio: Vec<AudioTrack>,
    gg: bool,
    gg_game: Option<String>,
    gg_trim: Option<(f64, f64)>,
    gg_thumb: Option<String>,
    gg_recorded: Option<u64>,
    /// Markers from a clip Backtrack recorded (BACKTRACK_META).
    #[serde(default)]
    bt_markers: Vec<f64>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
struct UserEntry {
    #[serde(default)]
    fav: bool,
    #[serde(default)]
    edits: Option<Edits>,
    #[serde(default)]
    exports: Vec<ExportRecord>,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    #[serde(default)]
    clips: HashMap<String, UserEntry>,
}

/// What `Library::forget` dropped for a clip (see `remember`).
pub struct Forgotten {
    key: String,
    probe: Option<Probe>,
    entry: Option<UserEntry>,
}

pub struct Library {
    index: Mutex<HashMap<String, Probe>>,
    store: Mutex<Store>,
    clips: Mutex<Vec<Clip>>,
    scan_lock: Mutex<()>,
    /// Files ffprobe couldn't read, with the (size, mtime) they had at the time.
    unreadable: Mutex<HashMap<String, (u64, u64)>>,
    /// Decoded waveform peaks at 100 points per second, per (clip id, audio track).
    pub wave_base: Mutex<HashMap<(String, usize), Arc<Vec<f32>>>>,
    pub thumbs_busy: AtomicBool,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

impl Library {
    pub fn load() -> Library {
        Library {
            index: Mutex::new(read_json(&util::cache_dir().join("index.json"))),
            store: Mutex::new(read_json(&util::data_dir().join("store.json"))),
            clips: Mutex::new(Vec::new()),
            scan_lock: Mutex::new(()),
            unreadable: Mutex::new(HashMap::new()),
            wave_base: Mutex::new(HashMap::new()),
            thumbs_busy: AtomicBool::new(false),
        }
    }

    fn save_store(&self, store: &Store) {
        if let Err(e) = write_json_atomic(&util::data_dir().join("store.json"), store) {
            util::log("error", &format!("saving store.json: {e}"));
        }
    }

    /// Distinct game names in the library (for naming newly recorded clips consistently).
    pub fn game_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.clips.lock().unwrap().iter().map(|c| c.game.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn get(&self, id: &str) -> Option<Clip> {
        self.clips.lock().unwrap().iter().find(|c| c.id == id).cloned()
    }

    /// Scan every clip folder, probe new or changed files, and rebuild the clip list (newest first).
    pub fn scan(&self, tools: Option<&Tools>, dirs: &[String]) -> Vec<Clip> {
        let _guard = self.scan_lock.lock().unwrap();

        // 1. Enumerate files.
        struct Found {
            key: String,
            path: PathBuf,
            size: u64,
            mtime: u64,
            created: u64,
        }
        let mut found: Vec<Found> = Vec::new();
        let mut thumb_maps: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();
        for dir in dirs {
            let dir = PathBuf::from(dir);
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                if !CLIP_EXTS.contains(&ext.as_str()) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                if !meta.is_file() || meta.len() < 64 * 1024 {
                    continue;
                }
                let mtime = meta.modified().map(util::system_time_ms).unwrap_or(0);
                let created = meta.created().map(util::system_time_ms).unwrap_or(mtime);
                found.push(Found { key: util::path_key(&path), path, size: meta.len(), mtime, created });
            }
            thumb_maps.insert(dir.clone(), gg_thumb_map(&dir.join("thumbs")));
        }

        // 2. Probe what's new or changed (in parallel).
        let todo: Vec<&Found> = {
            let index = self.index.lock().unwrap();
            let bad = self.unreadable.lock().unwrap();
            found
                .iter()
                .filter(|f| index.get(&f.key).map_or(true, |p| p.size != f.size || p.mtime != f.mtime))
                // Don't re-probe a broken file until it changes (e.g. a recording that was never finalised).
                .filter(|f| bad.get(&f.key) != Some(&(f.size, f.mtime)))
                .collect()
        };
        if let (Some(tools), false) = (tools, todo.is_empty()) {
            let queue = Mutex::new(todo.iter().collect::<Vec<_>>());
            let results: Mutex<Vec<(String, Probe)>> = Mutex::new(Vec::new());
            let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
            std::thread::scope(|s| {
                for _ in 0..workers {
                    s.spawn(|| loop {
                        let next = queue.lock().unwrap().pop();
                        let Some(f) = next else { break };
                        match probe(tools, &f.path) {
                            Ok(mut p) => {
                                p.size = f.size;
                                p.mtime = f.mtime;
                                p.created = f.created;
                                results.lock().unwrap().push((f.key.clone(), p));
                            }
                            // Either still being written (it will change and be probed again) or broken.
                            Err(e) => {
                                util::log("warn", &format!("probe failed for {}: {}", f.path.display(), e.lines().last().unwrap_or("")));
                                self.unreadable.lock().unwrap().insert(f.key.clone(), (f.size, f.mtime));
                            }
                        }
                    });
                }
            });
            let mut index = self.index.lock().unwrap();
            for (k, p) in results.into_inner().unwrap() {
                index.insert(k, p);
            }
            // Forget files that no longer exist.
            let live: std::collections::HashSet<&str> = found.iter().map(|f| f.key.as_str()).collect();
            index.retain(|k, _| live.contains(k.as_str()));
            if let Err(e) = write_json_atomic(&util::cache_dir().join("index.json"), &*index) {
                util::log("error", &format!("saving index.json: {e}"));
            }
        }

        // 3. Build the clip list.
        let index = self.index.lock().unwrap();
        let store = self.store.lock().unwrap();
        let thumb_cache = util::cache_sub("thumbs");
        let mut clips: Vec<Clip> = Vec::new();
        for f in &found {
            let Some(p) = index.get(&f.key) else { continue };
            let id = util::fnv_hex(&f.key);
            let file_name = f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = f.path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let game = p
                .gg_game
                .clone()
                .filter(|g| !g.trim().is_empty())
                .map(|g| if g.starts_with("moments.") { "Desktop capture".to_string() } else { g })
                .unwrap_or_else(|| game_from_stem(&stem));
            let dir = f.path.parent().map(Path::to_path_buf).unwrap_or_default();
            let thumb = p
                .gg_thumb
                .as_ref()
                .map(PathBuf::from)
                .filter(|t| t.is_file())
                .or_else(|| thumb_maps.get(&dir).and_then(|m| m.get(&stem.to_lowercase()).cloned()))
                .or_else(|| Some(thumb_cache.join(format!("{id}.jpg"))).filter(|t| t.is_file()))
                .map(|t| t.to_string_lossy().into_owned());
            let user = store.clips.get(&f.key).cloned().unwrap_or_default();
            let edits = user.edits.clone().or_else(|| {
                p.gg_trim.map(|(a, b)| Edits { in_t: a, out_t: b, markers: vec![] }).or_else(|| {
                    (!p.bt_markers.is_empty()).then(|| Edits { in_t: 0.0, out_t: p.duration, markers: p.bt_markers.clone() })
                })
            });
            clips.push(Clip {
                id,
                path: f.path.to_string_lossy().into_owned(),
                file_name,
                game_id: util::slugify(&game),
                game,
                recorded_at: p.gg_recorded.unwrap_or(p.created),
                duration: p.duration,
                size_bytes: f.size,
                width: p.width,
                height: p.height,
                fps: p.fps,
                video_kbps: p.video_kbps,
                audio_tracks: p.audio.clone(),
                thumb,
                fav: user.fav,
                edits,
                exports: user.exports.clone(),
                source: if p.gg { "gg".into() } else { "backtrack".into() },
            });
        }
        // One display name per game: GG's name wins over one guessed from a file name
        // ("Counter-Strike 2" vs "Counter Strike 2"), then the most common spelling.
        let mut names: HashMap<String, HashMap<String, (bool, usize)>> = HashMap::new();
        for c in &clips {
            let from_gg = index.get(&util::path_key(Path::new(&c.path))).and_then(|p| p.gg_game.as_ref()) == Some(&c.game);
            let e = names.entry(c.game_id.clone()).or_default().entry(c.game.clone()).or_insert((false, 0));
            e.0 |= from_gg;
            e.1 += 1;
        }
        let best: HashMap<String, String> = names
            .into_iter()
            .filter_map(|(id, m)| m.into_iter().max_by_key(|(_, (gg, n))| (*gg, *n)).map(|(name, _)| (id, name)))
            .collect();
        for c in &mut clips {
            if let Some(name) = best.get(&c.game_id) {
                c.game = name.clone();
            }
        }
        clips.sort_by(|a, b| b.recorded_at.cmp(&a.recorded_at));
        *self.clips.lock().unwrap() = clips.clone();
        clips
    }

    /// Clips that have no thumbnail yet: (id, path, duration).
    pub fn missing_thumbs(&self) -> Vec<(String, String, f64)> {
        self.clips
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.thumb.is_none())
            .map(|c| (c.id.clone(), c.path.clone(), c.duration))
            .collect()
    }

    fn with_entry<F: FnOnce(&mut UserEntry)>(&self, id: &str, f: F) -> Result<(), String> {
        let clip = self.get(id).ok_or("That clip is no longer in the library.")?;
        let key = util::path_key(Path::new(&clip.path));
        let mut store = self.store.lock().unwrap();
        f(store.clips.entry(key).or_default());
        self.save_store(&store);
        drop(store);
        // Keep the in-memory list in sync without a rescan.
        let store = self.store.lock().unwrap();
        let entry = store.clips.get(&util::path_key(Path::new(&clip.path))).cloned().unwrap_or_default();
        drop(store);
        if let Some(c) = self.clips.lock().unwrap().iter_mut().find(|c| c.id == id) {
            c.fav = entry.fav;
            if entry.edits.is_some() {
                c.edits = entry.edits.clone();
            }
            c.exports = entry.exports.clone();
        }
        Ok(())
    }

    pub fn set_fav(&self, id: &str, fav: bool) -> Result<(), String> {
        self.with_entry(id, |e| e.fav = fav)
    }

    pub fn save_edits(&self, id: &str, edits: Edits) -> Result<(), String> {
        self.with_entry(id, |e| e.edits = Some(edits))
    }

    pub fn add_export(&self, id: &str, rec: ExportRecord) -> Result<(), String> {
        self.with_entry(id, |e| {
            e.exports.insert(0, rec);
            e.exports.truncate(50);
        })
    }

    /// Favourite (or not) several clips with one write.
    pub fn set_favs(&self, ids: &[String], fav: bool) -> Result<(), String> {
        let keys: Vec<String> = {
            let clips = self.clips.lock().unwrap();
            clips.iter().filter(|c| ids.contains(&c.id)).map(|c| util::path_key(Path::new(&c.path))).collect()
        };
        if keys.is_empty() {
            return Err("Those clips are no longer in the library.".into());
        }
        let mut store = self.store.lock().unwrap();
        for k in keys {
            store.clips.entry(k).or_default().fav = fav;
        }
        self.save_store(&store);
        drop(store);
        for c in self.clips.lock().unwrap().iter_mut().filter(|c| ids.contains(&c.id)) {
            c.fav = fav;
        }
        Ok(())
    }

    /// Drop a clip whose file is gone. Returns what was dropped, so `remember` can put it back
    /// if the file comes back from the Recycle Bin (Undo).
    pub fn forget(&self, id: &str) -> Option<Forgotten> {
        let mut clips = self.clips.lock().unwrap();
        let pos = clips.iter().position(|c| c.id == id)?;
        let clip = clips.remove(pos);
        let key = util::path_key(Path::new(&clip.path));
        let probe = self.index.lock().unwrap().remove(&key);
        let mut store = self.store.lock().unwrap();
        let entry = store.clips.remove(&key);
        if entry.is_some() {
            self.save_store(&store);
        }
        Some(Forgotten { key, probe, entry })
    }

    /// Restore a forgotten clip's favourite, edits, export history and probe cache. The next scan lists it again.
    pub fn remember(&self, f: Forgotten) {
        if let Some(p) = f.probe {
            self.index.lock().unwrap().insert(f.key.clone(), p);
        }
        if let Some(e) = f.entry {
            let mut store = self.store.lock().unwrap();
            store.clips.insert(f.key, e);
            self.save_store(&store);
        }
    }
}

/// GG keeps thumbnails in `<clip dir>\thumbs\<stem>-<epoch>-thumb.jpg`. Map lowercase stem → path.
fn gg_thumb_map(dir: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return map };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(base) = name.strip_suffix("-thumb.jpg") else { continue };
        let stem = match base.rsplit_once('-') {
            Some((s, epoch)) if epoch.chars().all(|c| c.is_ascii_digit()) => s,
            _ => base,
        };
        map.insert(stem.to_lowercase(), entry.path());
    }
    map
}

/// "Call-of-Duty-Modern-Warfare-2-(2009)__2026-10-23__03-26-01" → "Call of Duty Modern Warfare 2 (2009)"
fn game_from_stem(stem: &str) -> String {
    let prefix = stem.split("__").next().unwrap_or(stem);
    if prefix.eq_ignore_ascii_case("DESKTOPCAPTURE") || prefix.eq_ignore_ascii_case("desktop") {
        return "Desktop capture".into();
    }
    let name = prefix.replace('-', " ").replace('_', " ");
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        "Other".into()
    } else {
        name
    }
}

fn parse_rate(s: &str) -> f64 {
    match s.split_once('/') {
        Some((a, b)) => {
            let (a, b) = (a.parse::<f64>().unwrap_or(0.0), b.parse::<f64>().unwrap_or(0.0));
            if b > 0.0 {
                a / b
            } else {
                0.0
            }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn probe(tools: &Tools, path: &Path) -> Result<Probe, String> {
    let out = tools
        .ffprobe_cmd()
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let j: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut p = Probe::default();
    let format = &j["format"];
    p.duration = num(&format["duration"]).unwrap_or(0.0);
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let mut audio_n = 0usize;
    for s in &streams {
        match s["codec_type"].as_str() {
            Some("video") if p.width == 0 => {
                // Skip attached cover-art pictures.
                if s["disposition"]["attached_pic"].as_i64() == Some(1) {
                    continue;
                }
                p.width = s["width"].as_u64().unwrap_or(0) as u32;
                p.height = s["height"].as_u64().unwrap_or(0) as u32;
                let avg = parse_rate(s["avg_frame_rate"].as_str().unwrap_or("0/0"));
                p.fps = if avg > 0.0 { avg } else { parse_rate(s["r_frame_rate"].as_str().unwrap_or("0/0")) };
                p.video_kbps = num(&s["bit_rate"]).map(|b| b / 1000.0).unwrap_or(0.0);
                if p.duration <= 0.0 {
                    p.duration = num(&s["duration"]).unwrap_or(0.0);
                }
            }
            Some("audio") => {
                let tags = &s["tags"];
                let raw = tags["title"].as_str().or(tags["handler_name"].as_str()).unwrap_or("").trim().to_string();
                let generic = raw.is_empty() || raw.eq_ignore_ascii_case("SoundHandler") || raw.to_lowercase().contains("handler");
                p.audio.push(AudioTrack {
                    index: audio_n,
                    name: if generic { format!("Track {}", audio_n + 1) } else { raw },
                    channels: s["channels"].as_u64().unwrap_or(2) as u32,
                });
                audio_n += 1;
            }
            _ => {}
        }
    }
    if p.width == 0 || p.duration <= 0.0 {
        return Err("no video stream / unknown duration".into());
    }
    if p.video_kbps <= 0.0 {
        p.video_kbps = num(&format["bit_rate"]).map(|b| b / 1000.0).unwrap_or(0.0);
    }

    // Clips Backtrack recorded carry {"game", "markers", "recordedAt"} in BACKTRACK_META.
    if let Some(meta) = format["tags"]["BACKTRACK_META"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        p.gg_game = meta["game"].as_str().map(str::to_string).filter(|s| !s.trim().is_empty());
        p.gg_recorded = meta["recordedAt"].as_u64();
        p.bt_markers = meta["markers"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
    }
    // SteelSeries GG splits a JSON blob across STEELSERIES_META0000, 0001, ... tags.
    if let Some(tags) = format["tags"].as_object() {
        let mut parts: Vec<(&String, &Value)> = tags.iter().filter(|(k, _)| k.starts_with("STEELSERIES_META")).collect();
        if !parts.is_empty() {
            parts.sort_by(|a, b| a.0.cmp(b.0));
            let blob: String = parts.iter().filter_map(|(_, v)| v.as_str()).collect();
            if let Ok(meta) = serde_json::from_str::<Value>(&blob) {
                p.gg = true;
                p.gg_game = meta["last_game_name"].as_str().map(|g| {
                    // GG stores an untranslated key for desktop recordings.
                    if g.starts_with("moments.") { "Desktop capture".to_string() } else { g.trim().to_string() }
                }).filter(|s| !s.is_empty());
                if meta["is_manually_trimmed"].as_bool() == Some(true) {
                    if let (Some(a), Some(b)) = (num(&meta["clip_start_point"]), num(&meta["clip_end_point"])) {
                        if b > a && b - a >= 0.5 {
                            p.gg_trim = Some((a.max(0.0), b.min(p.duration)));
                        }
                    }
                }
                p.gg_thumb = meta["thumbnail_path"].as_str().map(str::to_string);
                p.gg_recorded = meta["recording_timestamp"].as_str().and_then(util::parse_iso_utc_ms);
            } else {
                p.gg = true;
            }
        }
    }
    Ok(p)
}
