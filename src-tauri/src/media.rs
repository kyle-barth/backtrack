//! Waveforms, per-track audio for preview, and generated thumbnails. All done locally with ffmpeg.

use crate::ffmpeg::Tools;
use crate::library::{Clip, Library};
use crate::util;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

const PEAKS_PER_SEC: f64 = 100.0;
const DECODE_RATE: usize = 8000;

/// Peak envelope (0..1) of one audio track, resampled to `points` buckets spanning the whole clip.
pub fn waveform(lib: &Library, tools: &Tools, clip: &Clip, track: usize, points: usize) -> Result<Vec<f32>, String> {
    if track >= clip.audio_tracks.len() {
        return Err(format!("Clip has no audio track {track}."));
    }
    let key = (clip.id.clone(), track);
    let cached = lib.wave_base.lock().unwrap().get(&key).cloned();
    let base = match cached {
        Some(b) => b,
        None => {
            let b = Arc::new(decode_peaks(tools, &clip.path, track)?);
            let mut map = lib.wave_base.lock().unwrap();
            if map.len() > 64 {
                map.clear();
            }
            map.insert(key, b.clone());
            b
        }
    };
    let points = points.clamp(16, 20_000);
    let total = ((clip.duration * PEAKS_PER_SEC).ceil() as usize).max(base.len()).max(1);
    // Scale each lane to its own loudest moment (GG records game audio quietly, around -15 dBFS), but
    // not by more than ~6x, so a silent or noise-only track still reads as quiet.
    let loudest = base.iter().cloned().fold(0.0f32, f32::max);
    let scale = 0.95 / loudest.max(0.15);
    let mut out = Vec::with_capacity(points);
    for i in 0..points {
        let a = i * total / points;
        let b = ((i + 1) * total / points).max(a + 1);
        let peak = base.get(a..b.min(base.len())).map(|s| s.iter().cloned().fold(0.0f32, f32::max)).unwrap_or(0.0);
        out.push((peak * scale).min(1.0));
    }
    Ok(out)
}

fn decode_peaks(tools: &Tools, path: &str, track: usize) -> Result<Vec<f32>, String> {
    let out = tools
        .ffmpeg_cmd()
        .args(["-v", "error", "-i", path, "-map", &format!("0:a:{track}"), "-vn", "-ac", "1"])
        .args(["-ar", &DECODE_RATE.to_string(), "-f", "s16le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("Couldn't read the audio: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let per = (DECODE_RATE as f64 / PEAKS_PER_SEC) as usize;
    let samples: Vec<i16> = out.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    Ok(samples
        .chunks(per)
        .map(|c| {
            let peak = c.iter().map(|s| (*s as i32).unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0;
            // A gentle curve so quiet voice/mic tracks stay visible next to loud game audio.
            peak.min(1.0).powf(0.7)
        })
        .collect())
}

/// Times (seconds) of the video keyframes, read from the packet index (nothing is decoded: ≈0.2 s for a
/// 200 MB clip). The editor seeks to the nearest one while a timeline drag moves fast: a keyframe decodes
/// in ≈10 ms, while an exact frame can need up to a whole GOP (2 s in GG's clips) decoded first.
pub fn keyframes(tools: &Tools, clip: &Clip) -> Result<Arc<Vec<f64>>, String> {
    static CACHE: OnceLock<Mutex<HashMap<(String, u64), Arc<Vec<f64>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (clip.id.clone(), clip.size_bytes);
    if let Some(k) = cache.lock().unwrap().get(&key) {
        return Ok(k.clone());
    }
    let out = tools
        .ffprobe_cmd()
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"])
        .arg(&clip.path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Couldn't read the video's keyframes: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let keys = Arc::new(parse_keyframes(&String::from_utf8_lossy(&out.stdout)));
    let mut map = cache.lock().unwrap();
    if map.len() > 64 {
        map.clear();
    }
    map.insert(key, keys.clone());
    Ok(keys)
}

/// ffprobe `pts_time,flags` lines ("2.083333,K__") → sorted keyframe times.
fn parse_keyframes(csv: &str) -> Vec<f64> {
    let mut keys: Vec<f64> = csv
        .lines()
        .filter_map(|l| {
            let (t, flags) = l.trim().split_once(',')?;
            if flags.contains('K') { t.parse::<f64>().ok().filter(|t| t.is_finite()) } else { None }
        })
        .collect();
    keys.sort_by(|a, b| a.total_cmp(b));
    keys.dedup();
    keys
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PreparedTrack {
    pub index: usize,
    pub name: String,
    pub path: String,
}

/// Extract each audio track to its own .m4a (stream copy, no re-encode) so the editor can play every
/// track with its own volume. Cached per clip + file size.
pub fn prepare_audio(tools: &Tools, clip: &Clip) -> Result<Vec<PreparedTrack>, String> {
    let dir = util::cache_sub("audio");
    let target = |i: usize| dir.join(format!("{}_{}_{}.m4a", clip.id, clip.size_bytes, i));
    let missing: Vec<usize> = clip.audio_tracks.iter().map(|t| t.index).filter(|i| !target(*i).is_file()).collect();
    if !missing.is_empty() {
        let mut cmd = tools.ffmpeg_cmd();
        cmd.args(["-v", "error", "-y", "-i", &clip.path]);
        let tmp = |i: usize| dir.join(format!("{}_{}_{}.part.m4a", clip.id, clip.size_bytes, i));
        for &i in &missing {
            cmd.args(["-map", &format!("0:a:{i}"), "-vn", "-c", "copy", "-movflags", "+faststart"]);
            cmd.arg(tmp(i));
        }
        let out = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            for &i in &missing {
                let _ = std::fs::remove_file(tmp(i));
            }
            return Err(format!("Couldn't prepare the audio tracks: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        for &i in &missing {
            std::fs::rename(tmp(i), target(i)).map_err(|e| e.to_string())?;
        }
        prune(&dir, 90);
    }
    Ok(clip
        .audio_tracks
        .iter()
        .map(|t| PreparedTrack { index: t.index, name: t.name.clone(), path: target(t.index).to_string_lossy().into_owned() })
        .collect())
}

/// Keep at most `keep` files in a cache folder, deleting the least recently modified.
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

/// Grab one frame as a 480px-wide JPG thumbnail, on one thread each for decoding, scaling and encoding. By default
/// ffmpeg starts threads for every CPU (39 in all here) for one frame: 39% more CPU time, all at once, and often
/// right after a save while a game runs. One thread takes ~0.4 s instead of ~0.2 s, in the background.
pub fn generate_thumb(tools: &Tools, clip_path: &str, duration: f64, out: &Path) -> bool {
    let tmp = out.with_extension("part.jpg");
    let ok = thumb_cmd(tools, clip_path, duration, &tmp).status().map(|s| s.success()).unwrap_or(false);
    ok && std::fs::rename(&tmp, out).is_ok()
}

/// generate_thumb's ffmpeg command, writing to `tmp` (built apart so a test can check its arguments).
fn thumb_cmd(tools: &Tools, clip_path: &str, duration: f64, tmp: &Path) -> std::process::Command {
    let at = (duration * 0.3).clamp(0.0, 3.0);
    let mut cmd = tools.ffmpeg_cmd();
    cmd.args(["-v", "error", "-y", "-threads", "1", "-ss", &format!("{at:.2}"), "-i", clip_path, "-frames:v", "1"])
        .args(["-filter_threads", "1", "-threads", "1"])
        .args(["-vf", "scale=480:-2", "-q:v", "4"])
        .arg(tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframes_from_ffprobe_csv() {
        let csv = "0.000000,K__\r\n0.016667,___\n2.083333,K__\n2.100000,___\n\n4.166667,K_\nN/A,K__\n";
        assert_eq!(parse_keyframes(csv), vec![0.0, 2.083333, 4.166667]);
    }

    /// A thumbnail decodes, scales and encodes on one thread each (by default ffmpeg starts threads for every CPU, 39
    /// here, for one frame, often right after a save while a game runs), and seeks before opening the clip, so only
    /// from the keyframe before that point is decoded.
    #[test]
    fn thumbnail_runs_on_one_thread_each() {
        let tools = Tools { ffmpeg: "ffmpeg.exe".into(), ffprobe: "ffprobe.exe".into() };
        let cmd = thumb_cmd(&tools, r"C:\Clips\a.mp4", 60.0, Path::new(r"C:\Thumbs\a.part.jpg"));
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let input = args.iter().position(|a| a == "-i").expect("an input");
        let (before, after) = args.split_at(input);
        let has = |part: &[String], k: &str, v: &str| part.windows(2).any(|w| w[0] == k && w[1] == v);
        assert!(has(before, "-threads", "1"), "decoder on one thread: {args:?}");
        assert!(has(before, "-ss", "3.00"), "seeks before opening the clip: {args:?}");
        assert!(has(after, "-filter_threads", "1"), "scaler on one thread: {args:?}");
        assert!(has(after, "-threads", "1"), "encoder on one thread: {args:?}");
        assert!(has(after, "-frames:v", "1"), "one frame: {args:?}");
    }
}
