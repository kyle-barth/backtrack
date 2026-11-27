//! The clip library: scans clip folders, probes each clip once with ffprobe (cached by size + mtime),
//! reads SteelSeries GG's embedded metadata (game name, trim points, thumbnail), and keeps the user's
//! own data (favourites, In/Out edits, export history) in %APPDATA%\Backtrack\store.json.

use crate::ffmpeg::Tools;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Edits {
    pub in_t: f64,
    pub out_t: f64,
    #[serde(default)]
    pub markers: Vec<f64>,
    /// The frame exports get, which thumbnails show too: "original", "16:9" (a crop of a clip wider than that, like
    /// an ultrawide one) or "9:16" (vertical). Edits saved before v0.4.9 have none: the original frame.
    #[serde(default = "original_canvas")]
    pub canvas: String,
    /// Where a crop sits in the room it leaves: across (0 = left edge, 1 = right edge) and down (0 = top, 1 = bottom).
    #[serde(default = "half")]
    pub crop_x: f64,
    #[serde(default = "half")]
    pub crop_y: f64,
    /// How much of the clip's height a crop keeps (1 = all of it): smaller to frame a game in a window, like one on
    /// half of an ultrawide screen.
    #[serde(default = "whole")]
    pub crop_h: f64,
}

fn original_canvas() -> String {
    "original".into()
}
fn half() -> f64 {
    0.5
}
fn whole() -> f64 {
    1.0
}

impl Default for Edits {
    fn default() -> Self {
        Edits { in_t: 0.0, out_t: 0.0, markers: vec![], canvas: original_canvas(), crop_x: half(), crop_y: half(), crop_h: whole() }
    }
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
    /// A thumbnail run is going, and whether another was asked for since it last looked for work.
    pub thumbs_busy: AtomicBool,
    pub thumbs_again: AtomicBool,
}

/// A thumbnail to make: the frame `at` seconds into the clip at `path`, saved as `name` in the thumbnail cache.
#[derive(Debug, PartialEq)]
pub struct ThumbJob {
    pub id: String,
    pub path: String,
    pub at: f64,
    pub name: String,
}

/// A clip trimmed at the start shows the first frame of its trim, the one its exports start on: (time, cache file
/// name). The time is in the name, so a new trim gets a new file (and the window doesn't show a cached old one).
fn trim_thumb(id: &str, edits: Option<&Edits>) -> Option<(f64, String)> {
    let at = edits?.in_t;
    (at.is_finite() && at > 0.01).then(|| (at, format!("{id}-in-{}.jpg", (at * 1000.0).round() as u64)))
}

/// Where an untrimmed clip's own thumbnail is taken from: a little way in, past a fade or a black first frame.
fn default_thumb_at(duration: f64) -> f64 {
    (duration * 0.3).clamp(0.0, 3.0)
}

/// Is `name` one of the trimmed-clip thumbnails trim_thumb names ("<hex id>-in-<ms>.jpg")?
fn is_trim_thumb(name: &str) -> bool {
    let Some(base) = name.strip_suffix(".jpg") else { return false };
    let Some((id, ms)) = base.split_once("-in-") else { return false };
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit()) && !ms.is_empty() && ms.chars().all(|c| c.is_ascii_digit())
}

/// Delete trimmed-clip thumbnails in `dir` that are no longer in `keep` (a trim that changed, a clip that's gone).
/// Nothing else in the folder is touched.
pub fn prune_trim_thumbs(dir: &Path, keep: &HashSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_trim_thumb(&name) && !keep.contains(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

impl Library {
    pub fn load() -> Library {
        Library {
            index: Mutex::new(read_json(&util::cache_dir().join("index.json"))),
            store: Mutex::new(util::read_json_kept(&util::data_dir().join("store.json"))),
            clips: Mutex::new(Vec::new()),
            scan_lock: Mutex::new(()),
            unreadable: Mutex::new(HashMap::new()),
            wave_base: Mutex::new(HashMap::new()),
            thumbs_busy: AtomicBool::new(false),
            thumbs_again: AtomicBool::new(false),
        }
    }

    fn save_store(&self, store: &Store) {
        if let Err(e) = util::write_json_atomic(&util::data_dir().join("store.json"), store) {
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
                if !CLIP_EXTS.contains(&ext.as_str()) || util::is_partial(&path) {
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
            // New probes in, files that no longer exist out; written only if that changed anything.
            let live: std::collections::HashSet<&str> = found.iter().map(|f| f.key.as_str()).collect();
            if update_index(&mut index, results.into_inner().unwrap(), &live) {
                if let Err(e) = util::write_json_atomic(&util::cache_dir().join("index.json"), &*index) {
                    util::log("error", &format!("saving index.json: {e}"));
                }
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
            let user = store.clips.get(&f.key).cloned().unwrap_or_default();
            let edits = user.edits.clone().or_else(|| {
                p.gg_trim.map(|(a, b)| Edits { in_t: a, out_t: b, ..Edits::default() }).or_else(|| {
                    (!p.bt_markers.is_empty()).then(|| Edits { out_t: p.duration, markers: p.bt_markers.clone(), ..Edits::default() })
                })
            });
            // A clip trimmed at the start shows its first trimmed frame once that's been made (thumbs_wanted); until
            // then, and for every other clip, GG's thumbnail or the one Backtrack made.
            let thumb = trim_thumb(&id, edits.as_ref())
                .map(|(_, name)| thumb_cache.join(name))
                .filter(|t| t.is_file())
                .or_else(|| p.gg_thumb.as_ref().map(PathBuf::from).filter(|t| t.is_file()))
                .or_else(|| thumb_maps.get(&dir).and_then(|m| m.get(&stem.to_lowercase()).cloned()))
                .or_else(|| Some(thumb_cache.join(format!("{id}.jpg"))).filter(|t| t.is_file()))
                .map(|t| t.to_string_lossy().into_owned());
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

    /// Give one clip the thumbnail just made for it.
    pub fn set_thumb(&self, id: &str, thumb: &str) {
        if let Some(c) = self.clips.lock().unwrap().iter_mut().find(|c| c.id == id) {
            c.thumb = Some(thumb.to_string());
        }
    }

    /// Thumbnails to make, newest clip first, skipping file names in `failed`: the first trimmed frame of each clip
    /// trimmed at the start that doesn't show it yet, and one a little way in for each other clip with no thumbnail
    /// (including a trimmed one whose own couldn't be made).
    pub fn thumbs_wanted(&self, failed: &HashSet<String>) -> Vec<ThumbJob> {
        let clips = self.clips.lock().unwrap();
        let mut jobs = Vec::new();
        for c in clips.iter() {
            let shows = |name: &str| c.thumb.as_deref().and_then(|t| Path::new(t).file_name()).map_or(false, |n| n == name);
            let job = |at: f64, name: String| ThumbJob { id: c.id.clone(), path: c.path.clone(), at, name };
            if let Some((at, name)) = trim_thumb(&c.id, c.edits.as_ref()) {
                if shows(&name) {
                    continue;
                }
                if !failed.contains(&name) {
                    jobs.push(job(at, name));
                    continue;
                }
            }
            let name = format!("{}.jpg", c.id);
            if c.thumb.is_none() && !failed.contains(&name) {
                jobs.push(job(default_thumb_at(c.duration), name));
            }
        }
        jobs
    }

    /// The trimmed-clip thumbnails still in use or wanted (prune_trim_thumbs deletes the rest).
    pub fn trim_thumbs_in_use(&self) -> HashSet<String> {
        let clips = self.clips.lock().unwrap();
        let mut keep = HashSet::new();
        for c in clips.iter() {
            if let Some(name) = c.thumb.as_deref().and_then(|t| Path::new(t).file_name()) {
                keep.insert(name.to_string_lossy().into_owned());
            }
            if let Some((_, name)) = trim_thumb(&c.id, c.edits.as_ref()) {
                keep.insert(name);
            }
        }
        keep
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

/// Add a scan's new probes to the index and forget files that no longer exist (not in `live`). Returns whether the
/// index changed, so index.json (166 KB here) is only rewritten when it did: a file that fails to probe, e.g. a broken
/// one probed again on every launch, changes nothing.
fn update_index(index: &mut HashMap<String, Probe>, results: Vec<(String, Probe)>, live: &std::collections::HashSet<&str>) -> bool {
    let before = index.len();
    let probed = !results.is_empty();
    for (k, p) in results {
        index.insert(k, p);
    }
    index.retain(|k, _| live.contains(k.as_str()));
    probed || index.len() != before
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Edits must survive an update: a store.json written by an older Backtrack, or with fields a newer one added,
    /// still loads with every In/Out, marker, favourite and export record, and saves them back unchanged.
    #[test]
    fn store_from_older_and_newer_versions_keeps_edits() {
        let old = r#"{"clips":{
            "c:\\clips\\a.mp4":{"fav":true,"edits":{"inT":12.5,"outT":30.25,"markers":[15.0]},"exports":[]},
            "c:\\clips\\b.mp4":{"edits":{"inT":0.0,"outT":9.0}}}}"#;
        let s: Store = serde_json::from_str(old).unwrap();
        let a = &s.clips["c:\\clips\\a.mp4"];
        let e = a.edits.as_ref().unwrap();
        assert!(a.fav);
        assert_eq!((e.in_t, e.out_t, e.markers.clone()), (12.5, 30.25, vec![15.0]));
        assert!(s.clips["c:\\clips\\b.mp4"].edits.as_ref().unwrap().markers.is_empty());

        let newer = r#"{"version":2,"clips":{"c:\\clips\\a.mp4":{"fav":false,"tags":["x"],
            "edits":{"inT":1.0,"outT":2.0,"markers":[],"speed":1.5},"exports":[]}}}"#;
        let s: Store = serde_json::from_str(newer).unwrap();
        assert_eq!(s.clips["c:\\clips\\a.mp4"].edits.as_ref().unwrap().out_t, 2.0);

        let again: Store = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(again.clips["c:\\clips\\a.mp4"].edits.as_ref().unwrap().in_t, 1.0);
    }

    /// The Canvas is saved with the edits (v0.4.9), so thumbnails and exports keep the crop. Edits saved before have
    /// none: the original frame, with a crop starting in the middle.
    #[test]
    fn canvas_and_crop_are_saved_with_the_edits() {
        let old: Edits = serde_json::from_str(r#"{"inT":12.5,"outT":30.25,"markers":[15.0]}"#).unwrap();
        assert_eq!((old.canvas.as_str(), old.crop_x), ("original", 0.5));
        let new: Edits = serde_json::from_str(r#"{"inT":0,"outT":45,"canvas":"16:9","cropX":0.316}"#).unwrap();
        let back: Value = serde_json::to_value(&new).unwrap();
        assert_eq!((back["canvas"].as_str(), back["cropX"].as_f64()), (Some("16:9"), Some(0.316)));
    }

    fn clip(id: &str) -> Clip {
        Clip {
            id: id.into(), path: format!("C:\\Clips\\{id}.mp4"), file_name: format!("{id}.mp4"), game: "Rocket League".into(),
            game_id: "rocket-league".into(), recorded_at: 0, duration: 60.0, size_bytes: 1 << 20, width: 1920, height: 1080,
            fps: 60.0, video_kbps: 25_000.0, audio_tracks: vec![], thumb: None, fav: false, edits: None, exports: vec![],
            source: "backtrack".into(),
        }
    }

    /// A library in memory only: nothing read from or written to the user's folders.
    fn library(clips: Vec<Clip>) -> Library {
        Library {
            index: Mutex::new(HashMap::new()),
            store: Mutex::new(Store::default()),
            clips: Mutex::new(clips),
            scan_lock: Mutex::new(()),
            unreadable: Mutex::new(HashMap::new()),
            wave_base: Mutex::new(HashMap::new()),
            thumbs_busy: AtomicBool::new(false),
            thumbs_again: AtomicBool::new(false),
        }
    }

    fn trimmed(id: &str, in_t: f64, thumb: Option<&str>) -> Clip {
        Clip { edits: Some(Edits { in_t, out_t: 40.0, ..Edits::default() }), thumb: thumb.map(str::to_string), ..clip(id) }
    }

    fn names(jobs: &[ThumbJob]) -> Vec<&str> {
        jobs.iter().map(|j| j.name.as_str()).collect()
    }

    /// After a save, the new clip's thumbnail is given to that one clip instead of rescanning the library (276 files, a
    /// probe and index.json, while the game runs).
    #[test]
    fn set_thumb_changes_only_that_clip() {
        let lib = library(vec![clip("a"), clip("b")]);
        lib.set_thumb("b", r"C:\Thumbs\b.jpg");
        assert_eq!(lib.get("b").unwrap().thumb.as_deref(), Some(r"C:\Thumbs\b.jpg"));
        assert_eq!(lib.get("a").unwrap().thumb, None);
        assert_eq!(names(&lib.thumbs_wanted(&HashSet::new())), vec!["a.jpg"]);
        lib.set_thumb("gone", r"C:\Thumbs\gone.jpg"); // a clip deleted meanwhile: nothing to do
        assert_eq!(lib.clips.lock().unwrap().len(), 2);
    }

    /// A clip trimmed at the start shows the first frame of its trim, the one its exports start on; one that isn't
    /// (or is only trimmed at the end) keeps GG's thumbnail, or gets one a little way in.
    #[test]
    fn trimmed_clips_show_their_first_trimmed_frame() {
        let lib = library(vec![
            clip("a"),                                               // no thumbnail: one 3 s in
            Clip { thumb: Some(r"C:\Moments\thumbs\b-1-thumb.jpg".into()), ..clip("b") }, // GG's: kept
            trimmed("c", 12.345, Some(r"C:\Moments\thumbs\c-1-thumb.jpg")), // GG's, but trimmed: the In point
            trimmed("d", 0.0, None),                                 // only the end trimmed: as untrimmed
            trimmed("e", 7.5, Some(r"C:\Cache\thumbs\e-in-7500.jpg")),   // already shows its In point
            trimmed("f", 9.0, Some(r"C:\Cache\thumbs\f-in-7500.jpg")),   // the In point moved since
        ]);
        let jobs = lib.thumbs_wanted(&HashSet::new());
        assert_eq!(names(&jobs), vec!["a.jpg", "c-in-12345.jpg", "d.jpg", "f-in-9000.jpg"]);
        let at: Vec<f64> = jobs.iter().map(|j| j.at).collect();
        assert_eq!(at, vec![3.0, 12.345, 3.0, 9.0]);
        assert_eq!(jobs[1].path, r"C:\Clips\c.mp4");

        // Short clips: 30% of the way in, not past the end.
        let short = library(vec![Clip { duration: 2.0, ..clip("a") }]);
        assert!((short.thumbs_wanted(&HashSet::new())[0].at - 0.6).abs() < 1e-9);
    }

    /// A thumbnail that couldn't be made isn't tried again on every rescan. A trimmed clip whose own failed still gets
    /// an ordinary one if it has none, and keeps the one it has otherwise.
    #[test]
    fn failed_thumbnails_are_not_retried() {
        let lib = library(vec![clip("a"), trimmed("b", 5.0, None), trimmed("c", 5.0, Some(r"C:\Cache\thumbs\c.jpg"))]);
        let failed: HashSet<String> = ["a.jpg", "b-in-5000.jpg", "c-in-5000.jpg"].map(String::from).into();
        assert_eq!(names(&lib.thumbs_wanted(&failed)), vec!["b.jpg"]);
        let failed: HashSet<String> = ["b-in-5000.jpg", "b.jpg"].map(String::from).into();
        assert_eq!(names(&lib.thumbs_wanted(&failed)), vec!["a.jpg", "c-in-5000.jpg"]);
    }

    /// Trimmed-clip thumbnails from an older trim, or a clip that's gone, are deleted; the ones shown or about to be
    /// made, untrimmed ones, GG's, and anything else in the folder are left alone.
    #[test]
    fn old_trim_thumbnails_are_deleted() {
        let dir = std::env::temp_dir().join(format!("backtrack-thumbs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shown = dir.join("e-in-7500.jpg").to_string_lossy().into_owned();
        let lib = library(vec![trimmed("e", 9.0, Some(&shown)), clip("a")]);
        let files = ["e-in-7500.jpg", "e-in-1000.jpg", "e-in-9000.part.jpg", "a.jpg", "9a7-in-1.jpg", "notes-in-5.jpg", "x-in-5.png"];
        for f in files {
            std::fs::write(dir.join(f), b"jpg").unwrap();
        }
        let keep = lib.trim_thumbs_in_use();
        assert!(keep.contains("e-in-7500.jpg") && keep.contains("e-in-9000.jpg"), "{keep:?}");
        prune_trim_thumbs(&dir, &keep);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(left, vec!["a.jpg", "e-in-7500.jpg", "e-in-9000.part.jpg", "notes-in-5.jpg", "x-in-5.png"]);
    }

    /// index.json is rewritten only when a scan changed the index: not when nothing is new, nor when the only files
    /// to probe can't be read (two broken clips here were probed, and the whole file rewritten, on every launch).
    #[test]
    fn index_is_only_rewritten_when_it_changed() {
        let probe = |size: u64| Probe { size, duration: 1.0, ..Probe::default() };
        let mut index: HashMap<String, Probe> = [("a".to_string(), probe(1)), ("b".to_string(), probe(2))].into();
        let live: std::collections::HashSet<&str> = ["a", "b", "broken"].into();
        assert!(!update_index(&mut index, vec![], &live), "nothing new (or only unreadable files)");
        assert_eq!(index.len(), 2);

        let live: std::collections::HashSet<&str> = ["a", "b", "c"].into();
        assert!(update_index(&mut index, vec![("c".into(), probe(3))], &live), "a new clip");
        assert_eq!(index["c"].size, 3);
        assert!(update_index(&mut index, vec![("a".into(), probe(4))], &live), "a changed clip");
        assert_eq!(index["a"].size, 4);

        let live: std::collections::HashSet<&str> = ["a", "c"].into();
        assert!(update_index(&mut index, vec![], &live), "a deleted clip");
        assert!(!index.contains_key("b"));
        assert!(!update_index(&mut index, vec![], &live), "and then nothing new");
    }
}
