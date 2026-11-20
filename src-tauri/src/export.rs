//! Export a trimmed range so it fits a size limit (Discord's 10 MB by default), entirely locally with ffmpeg.
//!
//! Maths mirror the UI's `estimate()`: target = limit × 0.95, audio 128 kbps (96 when tight), video gets the
//! rest, and the resolution/fps tier comes from the ladder. Encoding is two-pass libx264, which lands very
//! close to the requested size. Every non-muted audio track is mixed into one AAC track, because Discord
//! only plays the first audio track of a file.

use crate::ffmpeg::Tools;
use crate::library::Clip;
use crate::util;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackMix {
    pub index: usize,
    pub volume: f64,
    pub muted: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub clip_id: String,
    pub start: f64,
    pub end: f64,
    pub preset: String,
    pub limit_mb: Option<f64>,
    #[serde(default = "default_canvas")]
    pub canvas: String,
    #[serde(default = "half")]
    pub crop_x: f64,
    #[serde(default)]
    pub tracks: Vec<TrackMix>,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub copy_to_clipboard: bool,
}
fn default_canvas() -> String {
    "16:9".into()
}
fn half() -> f64 {
    0.5
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_kbps: f64,
    pub audio_kbps: f64,
    pub duration_sec: f64,
    pub elapsed_sec: f64,
    pub mode: String,
    pub copied: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: String,
    pub percent: f64,
    pub eta_sec: Option<f64>,
}

/// The one running export: a cancel flag and the live ffmpeg process.
#[derive(Default)]
pub struct Job {
    pub running: AtomicBool,
    pub cancel: AtomicBool,
    pub child: Mutex<Option<Child>>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Copy,
    Encode,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub mode: Mode,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_kbps: f64,
    pub audio_kbps: f64,
    pub target_bytes: Option<f64>,
    pub limit_bytes: Option<f64>,
}

const LADDER: [(f64, u32, u32); 6] = [
    (6000.0, 1080, 60),
    (3500.0, 1080, 30),
    (2000.0, 720, 60),
    (1500.0, 720, 30),
    (1000.0, 480, 30),
    (0.0, 480, 30),
];

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32).max(1) * 2
}

pub fn plan(clip: &Clip, dur: f64, preset: &str, limit_mb: Option<f64>, vertical: bool) -> Plan {
    let src_fps = clip.fps.round().max(1.0) as u32;
    let sel_mb = clip.size_bytes as f64 / 1e6 / clip.duration.max(0.1) * dur;
    let limit = if preset == "orig" { None } else { limit_mb.filter(|l| *l > 0.0).or(Some(10.0)) };
    if preset == "orig" || (limit.map_or(false, |l| sel_mb <= l * 0.95) && !vertical) {
        return Plan {
            mode: Mode::Copy,
            width: clip.width,
            height: clip.height,
            fps: src_fps,
            video_kbps: clip.video_kbps,
            audio_kbps: 192.0,
            target_bytes: None,
            limit_bytes: limit.map(|l| l * 1e6),
        };
    }
    let limit = limit.unwrap_or(10.0);
    let target_mb = limit * 0.95;
    let total = target_mb * 8000.0 / dur;
    let mut ak = 128.0;
    let mut vk = total - ak;
    if vk < 1500.0 {
        ak = 96.0;
        vk = total - ak;
    }
    vk = vk.min(20_000.0).max(150.0);
    let (_, res, tier_fps) = LADDER.iter().find(|(k, _, _)| vk >= *k).copied().unwrap_or((0.0, 480, 30));
    let fps = tier_fps.min(src_fps);
    let (width, height) = if vertical {
        let w = res.min(clip.height.max(2));
        (even(w as f64), even(w as f64 * 16.0 / 9.0))
    } else {
        let h = res.min(clip.height.max(2));
        (even(clip.width as f64 * h as f64 / clip.height.max(1) as f64), even(h as f64))
    };
    Plan {
        mode: Mode::Encode,
        width,
        height,
        fps,
        // ~1.5% of the budget goes to MP4 container overhead.
        video_kbps: (vk * 0.985).round(),
        audio_kbps: ak,
        target_bytes: Some(target_mb * 1e6),
        limit_bytes: Some(limit * 1e6),
    }
}

/// `[0:a:0]volume=1.000[a0];[0:a:1]volume=0.800[a1];[a0][a1]amix=inputs=2:normalize=0[aout]`, or None if silent.
fn audio_graph(clip: &Clip, tracks: &[TrackMix]) -> Option<String> {
    let mixes: Vec<TrackMix> = if tracks.is_empty() {
        clip.audio_tracks.iter().map(|t| TrackMix { index: t.index, volume: 1.0, muted: false }).collect()
    } else {
        tracks.to_vec()
    };
    let active: Vec<&TrackMix> = mixes
        .iter()
        .filter(|t| !t.muted && t.volume > 0.001 && t.index < clip.audio_tracks.len())
        .collect();
    if active.is_empty() {
        return None;
    }
    let mut g = String::new();
    for (k, t) in active.iter().enumerate() {
        g.push_str(&format!("[0:a:{}]volume={:.3}[a{k}];", t.index, t.volume.clamp(0.0, 2.0)));
    }
    if active.len() == 1 {
        g.push_str("[a0]anull[aout]");
    } else {
        for k in 0..active.len() {
            g.push_str(&format!("[a{k}]"));
        }
        g.push_str(&format!("amix=inputs={}:normalize=0:dropout_transition=0[aout]", active.len()));
    }
    Some(g)
}

fn video_chain(clip: &Clip, plan: &Plan, vertical: bool, crop_x: f64) -> String {
    let mut f = Vec::new();
    if vertical {
        f.push(format!("crop=w=trunc(ih*9/16/2)*2:h=ih:x=(iw-ow)*{:.4}:y=0", crop_x.clamp(0.0, 1.0)));
    }
    if vertical || plan.width != clip.width || plan.height != clip.height {
        f.push(format!("scale={}:{}:flags=lanczos", plan.width, plan.height));
    }
    if (plan.fps as f64) < clip.fps.round() {
        f.push(format!("fps={}", plan.fps));
    }
    f.push("format=yuv420p".into());
    format!("[0:v:0]{}[vout]", f.join(","))
}

/// Run ffmpeg with `-progress pipe:1`, reporting the fraction of `dur` encoded so far.
fn run_ffmpeg(
    tools: &Tools,
    args: &[String],
    dur: f64,
    job: &Job,
    mut on_frac: impl FnMut(f64),
) -> Result<(), String> {
    if job.cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    let mut cmd = tools.ffmpeg_cmd();
    cmd.args(["-v", "error", "-nostats", "-progress", "pipe:1"]).args(args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    util::log("info", &format!("ffmpeg {}", args.join(" ")));
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start ffmpeg: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    *job.child.lock().unwrap() = Some(child);

    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
            if let Ok(us) = v.trim().parse::<f64>() {
                on_frac((us / 1e6 / dur).clamp(0.0, 1.0));
            }
        }
    }
    let status = job.child.lock().unwrap().take().map(|mut c| c.wait());
    let err = err_thread.join().unwrap_or_default();
    if job.cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    match status {
        Some(Ok(s)) if s.success() => Ok(()),
        _ => {
            let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            util::log("error", &format!("ffmpeg failed: {tail}"));
            Err(format!("Export failed: {}", if tail.is_empty() { "ffmpeg exited unexpectedly".into() } else { tail }))
        }
    }
}

pub struct ExportCtx<'a> {
    pub tools: &'a Tools,
    pub clip: &'a Clip,
    pub out_dir: PathBuf,
    pub job: &'a Job,
}

pub fn export(ctx: &ExportCtx, req: &ExportRequest, mut progress: impl FnMut(Progress)) -> Result<(ExportResult, Plan), String> {
    let clip = ctx.clip;
    let start = req.start.clamp(0.0, clip.duration);
    let end = req.end.clamp(start, clip.duration);
    let dur = end - start;
    if dur < 0.2 {
        return Err("The selection is too short to export.".into());
    }
    let vertical = req.canvas == "9:16";
    let plan = plan(clip, dur, &req.preset, req.limit_mb, vertical);
    std::fs::create_dir_all(&ctx.out_dir).map_err(|e| format!("Couldn't create the export folder: {e}"))?;

    let default_stem = format!("{}_discord", Path::new(&clip.file_name).file_stem().unwrap_or_default().to_string_lossy());
    let stem = util::sanitize_stem(if req.file_name.trim().is_empty() { &default_stem } else { &req.file_name });
    let final_path = util::unique_path(&ctx.out_dir, &stem, "mp4");
    let part = final_path.with_extension("part.mp4");
    let started = Instant::now();
    let ss = format!("{start:.3}");
    let t = format!("{dur:.3}");
    let audio = audio_graph(clip, &req.tracks);

    let mut report = |stage: &str, pct: f64| {
        let el = started.elapsed().as_secs_f64();
        let eta = if pct > 3.0 && pct < 100.0 { Some(el * (100.0 - pct) / pct) } else { None };
        progress(Progress { stage: stage.into(), percent: (pct * 10.0).round() / 10.0, eta_sec: eta });
    };

    let s = |x: &str| x.to_string();
    let result = (|| -> Result<(), String> {
        match plan.mode {
            Mode::Copy => {
                let mut args = vec![s("-y"), s("-ss"), ss.clone(), s("-i"), clip.path.clone(), s("-t"), t.clone()];
                args.extend([s("-map"), s("0:v:0"), s("-c:v"), s("copy")]);
                match &audio {
                    Some(g) => args.extend([s("-filter_complex"), g.clone(), s("-map"), s("[aout]"), s("-c:a"), s("aac"), s("-b:a"), s("192k")]),
                    None => args.push(s("-an")),
                }
                args.extend([s("-avoid_negative_ts"), s("make_zero"), s("-movflags"), s("+faststart")]);
                args.push(part.to_string_lossy().into_owned());
                report("copy", 0.0);
                run_ffmpeg(ctx.tools, &args, dur, ctx.job, |f| report("copy", f * 97.0))
            }
            Mode::Encode => {
                let passlog = util::cache_sub("passlog").join(format!("job{}", util::now_ms()));
                let passlog_s = passlog.to_string_lossy().into_owned();
                let vchain = video_chain(clip, &plan, vertical, req.crop_x);
                let x264 = |vk: f64| {
                    vec![
                        s("-c:v"), s("libx264"), s("-preset"), s("medium"), s("-profile:v"), s("high"),
                        s("-b:v"), format!("{}k", vk.round()), s("-maxrate"), format!("{}k", (vk * 1.6).round()),
                        s("-bufsize"), format!("{}k", (vk * 2.0).round()), s("-passlogfile"), passlog_s.clone(),
                    ]
                };
                // Pass 1: video only, analysis.
                let mut p1 = vec![s("-y"), s("-ss"), ss.clone(), s("-i"), clip.path.clone(), s("-t"), t.clone()];
                p1.extend([s("-filter_complex"), vchain.clone(), s("-map"), s("[vout]"), s("-an")]);
                p1.extend(x264(plan.video_kbps));
                p1.extend([s("-pass"), s("1"), s("-f"), s("null"), s("NUL")]);
                report("analyse", 0.0);
                run_ffmpeg(ctx.tools, &p1, dur, ctx.job, |f| report("analyse", f * 30.0))?;

                // Pass 2 (repeated once at a lower bitrate if it overshoots the limit).
                let mut vk = plan.video_kbps;
                for attempt in 0..2 {
                    let graph = match &audio { Some(a) => format!("{vchain};{a}"), None => vchain.clone() };
                    let mut p2 = vec![s("-y"), s("-ss"), ss.clone(), s("-i"), clip.path.clone(), s("-t"), t.clone()];
                    p2.extend([s("-filter_complex"), graph, s("-map"), s("[vout]")]);
                    if audio.is_some() {
                        p2.extend([s("-map"), s("[aout]"), s("-c:a"), s("aac"), s("-b:a"), format!("{}k", plan.audio_kbps), s("-ar"), s("48000")]);
                    } else {
                        p2.push(s("-an"));
                    }
                    p2.extend(x264(vk));
                    p2.extend([s("-pass"), s("2"), s("-movflags"), s("+faststart")]);
                    p2.push(part.to_string_lossy().into_owned());
                    let (lo, span) = if attempt == 0 { (30.0, 67.0) } else { (90.0, 8.0) };
                    run_ffmpeg(ctx.tools, &p2, dur, ctx.job, |f| report("encode", lo + f * span))?;
                    let size = std::fs::metadata(&part).map(|m| m.len() as f64).unwrap_or(0.0);
                    match (plan.limit_bytes, plan.target_bytes) {
                        (Some(limit), Some(target)) if size > limit && attempt == 0 => {
                            util::log("warn", &format!("export overshot ({size} > {limit}); re-encoding"));
                            vk = (vk * target / size * 0.97).max(100.0);
                        }
                        _ => break,
                    }
                }
                for suffix in ["-0.log", "-0.log.mbtree", "-0.log.temp", "-0.log.mbtree.temp"] {
                    let _ = std::fs::remove_file(format!("{passlog_s}{suffix}"));
                }
                Ok(())
            }
        }
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &final_path).map_err(|e| format!("Couldn't save the file: {e}"))?;
    report("finish", 100.0);

    let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
    let total_kbps = size as f64 * 8.0 / 1000.0 / dur;
    let audio_kbps = if audio.is_some() { plan.audio_kbps } else { 0.0 };
    let copied = req.copy_to_clipboard && crate::sys::copy_file(&final_path.to_string_lossy()).is_ok();
    Ok((
        ExportResult {
            path: final_path.to_string_lossy().into_owned(),
            file_name: final_path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            size_bytes: size,
            width: plan.width,
            height: plan.height,
            fps: plan.fps,
            video_kbps: (total_kbps - audio_kbps).max(0.0).round(),
            audio_kbps,
            duration_sec: dur,
            elapsed_sec: started.elapsed().as_secs_f64(),
            mode: if plan.mode == Mode::Copy { "copy".into() } else { "encode".into() },
            copied,
        },
        plan,
    ))
}

/// Lossless trim: every stream copied as-is. The start snaps to the keyframe at or before `start`.
pub fn save_trimmed_copy(tools: &Tools, clip: &Clip, start: f64, end: f64, out_dir: &Path, job: &Job) -> Result<PathBuf, String> {
    let start = start.clamp(0.0, clip.duration);
    let dur = (end.clamp(start, clip.duration) - start).max(0.1);
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let stem = format!("{}_trim", Path::new(&clip.file_name).file_stem().unwrap_or_default().to_string_lossy());
    let out = util::unique_path(out_dir, &util::sanitize_stem(&stem), "mp4");
    let part = out.with_extension("part.mp4");
    let args: Vec<String> = [
        "-y", "-ss", &format!("{start:.3}"), "-i", &clip.path, "-t", &format!("{dur:.3}"),
        "-map", "0", "-c", "copy", "-avoid_negative_ts", "make_zero", "-movflags", "+faststart",
    ]
    .iter()
    .map(|x| x.to_string())
    .chain(std::iter::once(part.to_string_lossy().into_owned()))
    .collect();
    if let Err(e) = run_ffmpeg(tools, &args, dur, job, |_| {}) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::AudioTrack;

    fn rl_clip() -> Clip {
        Clip {
            id: "x".into(), path: "x.mp4".into(), file_name: "x.mp4".into(), game: "Rocket League".into(),
            game_id: "rocket-league".into(), recorded_at: 0, duration: 58.333, size_bytes: 194_674_943,
            width: 1920, height: 1080, fps: 60.0, video_kbps: 26104.0,
            audio_tracks: (0..3).map(|i| AudioTrack { index: i, name: format!("T{i}"), channels: 2 }).collect(),
            thumb: None, fav: false, edits: None, exports: vec![], source: "gg".into(),
        }
    }

    #[test]
    fn ladder_matches_the_ui() {
        let c = rl_clip();
        let p = plan(&c, 24.0, "d10", Some(10.0), false);
        assert_eq!(p.mode, Mode::Encode);
        assert_eq!((p.width, p.height, p.fps), (1280, 720, 60)); // 24 s → ≈3.0 Mbps → 720p60
        let p = plan(&c, 58.0, "d10", Some(10.0), false);
        assert_eq!((p.width, p.height, p.fps), (854, 480, 30)); // 58 s → ≈1.2 Mbps → 480p30
        assert_eq!(p.audio_kbps, 96.0);
        let p = plan(&c, 10.0, "d10", Some(10.0), false);
        assert_eq!((p.width, p.height, p.fps), (1920, 1080, 60));
        let p = plan(&c, 24.0, "d10", Some(10.0), true);
        assert_eq!((p.width, p.height), (720, 1280));
        let p = plan(&c, 2.0, "d10", Some(10.0), false);
        assert_eq!(p.mode, Mode::Copy);
        let p = plan(&c, 24.0, "orig", None, false);
        assert_eq!(p.mode, Mode::Copy);
    }

    #[test]
    fn mix_graph() {
        let c = rl_clip();
        let g = audio_graph(&c, &[
            TrackMix { index: 0, volume: 1.0, muted: false },
            TrackMix { index: 1, volume: 0.8, muted: false },
            TrackMix { index: 2, volume: 0.7, muted: true },
        ]).unwrap();
        assert_eq!(g, "[0:a:0]volume=1.000[a0];[0:a:1]volume=0.800[a1];[a0][a1]amix=inputs=2:normalize=0:dropout_transition=0[aout]");
        assert!(audio_graph(&c, &[TrackMix { index: 0, volume: 0.0, muted: false }]).is_none());
    }
}
