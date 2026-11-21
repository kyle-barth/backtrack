//! The replay buffer.
//!
//! One long-running ffmpeg process captures the monitor on the GPU, encodes it with the GPU's hardware
//! encoder, mixes in the three audio pipes (see audio.rs) as AAC, and writes an MPEG-TS stream to stdout.
//! We keep that stream in RAM as a ring of chunks that each start at a video keyframe, dropping chunks
//! older than the buffer length. Saving writes the last N seconds back through ffmpeg (stream copy, no
//! re-encode) into an .mp4 that looks like a GG clip to the library: Game / Chat / Mic audio tracks plus
//! a BACKTRACK_META tag with the game name and markers.

use crate::audio::{Audio, AudioConfig, RATE};
use crate::ffmpeg::Tools;
use crate::settings::RecorderSettings;
use crate::util;
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const TS: usize = 188;
const VIDEO_PID: u16 = 0x100;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackStatus {
    pub name: String,
    pub source: String,
    pub level: f32,
    pub active: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    pub action: String,
    pub keys: String,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub path: String,
    pub game: String,
    pub duration: f64,
    pub at: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub state: String,
    pub error: Option<String>,
    pub buffer_secs: u32,
    pub filled_secs: f64,
    pub memory_mb: f64,
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub game: Option<String>,
    pub mic_muted: bool,
    pub tracks: Vec<TrackStatus>,
    pub hotkeys: Vec<HotkeyStatus>,
    pub last_saved: Option<Saved>,
}

struct Chunk {
    wall_ms: u64,
    data: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Ring {
    chunks: VecDeque<Chunk>,
    cur: Vec<u8>,
    cur_wall: u64,
    pat: Option<Vec<u8>>,
    pmt: Option<Vec<u8>>,
    pmt_pid: u16,
    bytes: usize,
    keep_ms: u64,
}

impl Ring {
    fn push_packet(&mut self, p: &[u8]) -> bool {
        let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
        let pusi = p[1] & 0x40 != 0;
        let afc = (p[3] >> 4) & 3;
        if pid == 0 {
            if pusi {
                let off = 5 + p[4] as usize;
                if off + 12 <= TS {
                    self.pmt_pid = (((p[off + 10] & 0x1f) as u16) << 8) | p[off + 11] as u16;
                }
            }
            self.pat = Some(p.to_vec());
            return false;
        }
        if pid == self.pmt_pid && self.pmt_pid != 0 {
            self.pmt = Some(p.to_vec());
            return false;
        }
        let keyframe = pid == VIDEO_PID && afc & 2 != 0 && p[4] > 0 && p[5] & 0x40 != 0;
        let mut new_chunk = false;
        if keyframe {
            if !self.cur.is_empty() {
                let data = Arc::new(std::mem::take(&mut self.cur));
                self.bytes += data.len();
                self.chunks.push_back(Chunk { wall_ms: self.cur_wall, data });
                new_chunk = true;
            }
            self.cur_wall = util::now_ms();
            self.cur.reserve(4 << 20);
        }
        if self.cur_wall == 0 {
            return false; // nothing useful before the first keyframe
        }
        self.cur.extend_from_slice(p);
        if new_chunk {
            let now = util::now_ms();
            while self.chunks.len() > 1 && now.saturating_sub(self.chunks[1].wall_ms) > self.keep_ms {
                if let Some(c) = self.chunks.pop_front() {
                    self.bytes -= c.data.len();
                }
            }
        }
        new_chunk
    }

    fn filled_secs(&self) -> f64 {
        self.chunks.front().map(|c| util::now_ms().saturating_sub(c.wall_ms) as f64 / 1000.0).unwrap_or(0.0)
    }
}

struct Session {
    id: u64,
    child: Option<Child>,
    audio: Arc<Audio>,
    stopping: Arc<AtomicBool>,
}

pub struct Recorder {
    state: Mutex<(String, Option<String>)>,
    ring: Mutex<Ring>,
    session: Mutex<Option<Session>>,
    next_session: AtomicU64,
    cfg: Mutex<RecorderSettings>,
    dims: Mutex<(u32, u32, u32)>,
    encoder_desc: Mutex<String>,
    pub mic_muted: Arc<AtomicBool>,
    markers: Mutex<Vec<u64>>,
    pub last_saved: Mutex<Option<Saved>>,
    pub hotkeys: Mutex<Vec<HotkeyStatus>>,
    pub game: Mutex<Option<String>>,
    saving: AtomicBool,
    started_at: Mutex<Option<Instant>>,
    /// The GPU scaler failed on a game's frames: record at the monitor's own size until Backtrack restarts.
    no_gpu_scale: AtomicBool,
}

static ENCODER: OnceLock<String> = OnceLock::new();

/// Pick a hardware encoder that actually works on this PC (tested once with a 1-frame capture).
fn detect_encoder(tools: &Tools) -> String {
    ENCODER
        .get_or_init(|| {
            let try_run = |args: &[&str]| {
                tools
                    .ffmpeg_cmd()
                    .args(["-v", "error"])
                    .args(args)
                    .args(["-frames:v", "1", "-f", "null", "-"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            };
            let enc = if try_run(&["-f", "lavfi", "-i", "vsrc_amf=framerate=10", "-c:v", "h264_amf"]) {
                "amf"
            } else if try_run(&["-f", "lavfi", "-i", "ddagrab=framerate=10", "-c:v", "h264_nvenc"]) {
                "nvenc"
            } else {
                "x264"
            };
            util::log("info", &format!("recorder encoder: {enc}"));
            enc.to_string()
        })
        .clone()
}

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32).max(1) * 2
}

/// Output size: the monitor scaled so its short side is `res` (never upscaled); 0 = native.
pub fn output_dims(mon_w: u32, mon_h: u32, res: u32) -> (u32, u32) {
    let short = mon_w.min(mon_h);
    if res == 0 || res >= short {
        return (even(mon_w as f64), even(mon_h as f64));
    }
    if mon_w >= mon_h {
        (even(mon_w as f64 * res as f64 / mon_h as f64), res)
    } else {
        (res, even(mon_h as f64 * res as f64 / mon_w as f64))
    }
}

fn file_safe_game(game: &str) -> String {
    let s: String = game.chars().map(|c| if c.is_alphanumeric() || "()®™".contains(c) { c } else { '-' }).collect();
    let mut out = String::new();
    for c in s.chars() {
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() { "Clip".into() } else { t }
}

impl Recorder {
    pub fn new() -> Arc<Recorder> {
        Arc::new(Recorder {
            state: Mutex::new(("off".into(), None)),
            ring: Mutex::new(Ring::default()),
            session: Mutex::new(None),
            next_session: AtomicU64::new(1),
            cfg: Mutex::new(RecorderSettings::default()),
            dims: Mutex::new((0, 0, 0)),
            encoder_desc: Mutex::new(String::new()),
            mic_muted: Arc::new(AtomicBool::new(false)),
            markers: Mutex::new(Vec::new()),
            last_saved: Mutex::new(None),
            hotkeys: Mutex::new(Vec::new()),
            game: Mutex::new(None),
            saving: AtomicBool::new(false),
            started_at: Mutex::new(None),
            no_gpu_scale: AtomicBool::new(false),
        })
    }

    pub fn state(&self) -> String {
        self.state.lock().unwrap().0.clone()
    }

    fn set_state(&self, s: &str, err: Option<String>) {
        *self.state.lock().unwrap() = (s.to_string(), err);
    }

    pub fn fail(&self, msg: &str) {
        self.set_state("error", Some(msg.to_string()));
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state().as_str(), "on" | "starting")
    }

    /// Start (or restart) the buffer. `mon` is the capture monitor's native size.
    pub fn start(self: &Arc<Self>, tools: &Tools, cfg: &RecorderSettings, mon: (u32, u32), on_change: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        self.stop();
        self.set_state("starting", None);
        *self.cfg.lock().unwrap() = cfg.clone();
        on_change();

        let encoder = if cfg.encoder == "auto" { detect_encoder(tools) } else { cfg.encoder.clone() };
        let native = (even(mon.0.max(2) as f64), even(mon.1.max(2) as f64));
        let (mut w, mut h) = output_dims(mon.0.max(2), mon.1.max(2), cfg.resolution);
        let fps = cfg.fps;
        // Once the GPU scaler has refused a game's frames, record at the monitor's own size (see the restart below).
        if encoder != "x264" && self.no_gpu_scale.load(Ordering::SeqCst) {
            (w, h) = native;
        }
        *self.dims.lock().unwrap() = (w, h, fps);
        let gpu = crate::winutil::gpu_name();
        *self.encoder_desc.lock().unwrap() = match encoder.as_str() {
            "amf" => format!("AMD AMF · {}", if gpu.is_empty() { "GPU".into() } else { gpu }),
            "nvenc" => format!("NVIDIA NVENC · {}", if gpu.is_empty() { "GPU".into() } else { gpu }),
            _ => "CPU (x264)".into(),
        };

        {
            let mut ring = self.ring.lock().unwrap();
            *ring = Ring::default();
            ring.keep_ms = cfg.buffer_secs as u64 * 1000 + 1500;
        }

        let id = self.next_session.fetch_add(1, Ordering::SeqCst);
        let audio = Audio::new(
            AudioConfig { chat_app: cfg.chat_app.clone(), mic_device: cfg.mic_device.clone(), mic_enabled: cfg.mic_enabled },
            self.mic_muted.clone(),
            id,
        );
        let t0 = Instant::now();
        let t0_wall = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        audio.start(t0)?;

        // ---- ffmpeg command
        let b = cfg.bitrate_mbps;
        let mut args: Vec<String> = Vec::new();
        let s = |x: &str| x.to_string();
        args.extend(["-v", "error", "-nostats"].map(s));
        for name in &audio.pipe_names {
            args.extend(["-f", "f32le", "-ar"].map(s));
            args.push(RATE.to_string());
            args.extend(["-ac", "2", "-probesize", "32", "-analyzeduration", "0", "-thread_queue_size", "4096", "-i"].map(s));
            args.push(name.clone());
        }
        // Video gets wall-clock timestamps on the same timeline as the audio clocks (audio sample 0 = t0).
        args.extend(["-thread_queue_size", "64", "-use_wallclock_as_timestamps", "1", "-itsoffset"].map(s));
        args.push(format!("-{t0_wall:.6}"));
        args.extend(["-f", "lavfi", "-i"].map(s));
        let scaled = (w, h) != native;
        match encoder.as_str() {
            "amf" => {
                args.push(format!("vsrc_amf=monitor_index={}:framerate={fps},settb=AVTB", cfg.monitor));
                args.extend(["-map", "3:v:0"].map(s));
                if scaled {
                    args.extend(["-vf".into(), format!("vpp_amf=w={w}:h={h}:format=nv12")]);
                }
                args.extend(["-c:v", "h264_amf", "-quality", "balanced", "-rc", "vbr_peak"].map(s));
            }
            "nvenc" => {
                args.push(format!("ddagrab=output_idx={}:framerate={fps}:draw_mouse=0,settb=AVTB", cfg.monitor));
                args.extend(["-map", "3:v:0"].map(s));
                if scaled {
                    args.extend(["-vf".into(), format!("scale_d3d11=width={w}:height={h}:format=nv12")]);
                }
                args.extend(["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "ll", "-rc", "vbr"].map(s));
            }
            _ => {
                args.push(format!("ddagrab=output_idx={}:framerate={fps}:draw_mouse=0,settb=AVTB", cfg.monitor));
                args.extend(["-map", "3:v:0"].map(s));
                args.extend(["-vf".into(), format!("hwdownload,format=bgra,scale={w}:{h},format=yuv420p")]);
                args.extend(["-c:v", "libx264", "-preset", "veryfast"].map(s));
            }
        }
        args.extend(["-b:v".into(), format!("{b}M"), "-maxrate".into(), format!("{}M", b * 3 / 2), "-bufsize".into(), format!("{}M", b * 2)]);
        args.extend(["-g".into(), fps.to_string(), "-bf".into(), "0".into()]);
        args.extend(["-fps_mode", "cfr", "-r"].map(s));
        args.push(fps.to_string());
        args.extend(["-map", "0:a:0", "-map", "1:a:0", "-map", "2:a:0", "-c:a", "aac", "-b:a", "192k"].map(s));
        args.extend(["-copyts", "-f", "mpegts", "pipe:1"].map(s));

        util::log("info", &format!("recorder: {} {}x{}@{} {}Mbps monitor {}", encoder, w, h, fps, b, cfg.monitor));
        let mut child = tools
            .ffmpeg_cmd()
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                audio.shutdown();
                format!("Couldn't start ffmpeg: {e}")
            })?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        *self.session.lock().unwrap() = Some(Session { id, child: Some(child), audio: audio.clone(), stopping: stopping.clone() });
        *self.started_at.lock().unwrap() = Some(Instant::now());

        let err_tail = Arc::new(Mutex::new(String::new()));
        let tail2 = err_tail.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut t = tail2.lock().unwrap();
                        t.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if t.len() > 4000 {
                            let cut = t.len() - 2000;
                            *t = t[cut..].to_string();
                        }
                    }
                }
            }
        });

        let me = self.clone();
        let tools2 = tools.clone();
        let cfg2 = cfg.clone();
        std::thread::Builder::new()
            .name("ts-reader".into())
            .spawn(move || {
                me.read_stream(stdout, &on_change);
                // ffmpeg ended.
                audio.shutdown();
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let tail = err_tail.lock().unwrap().clone();
                let last = tail.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("ffmpeg stopped").to_string();
                util::log("error", &format!("recorder stopped unexpectedly: {tail}"));
                let ran_long = me.started_at.lock().unwrap().map_or(false, |t| t.elapsed().as_secs() > 20);
                let same_session = me.session.lock().unwrap().as_ref().map(|s| s.id) == Some(id);
                // Some full-screen games switch the frames they hand the display to a format the GPU scaler refuses
                // mid-recording (AMD vpp_amf: "SubmitInput() failed with error 4", seen with Rocket League). The
                // encoder itself takes them, so carry on at the monitor's own size.
                let scaler_failed = scaled && (tail.contains("vpp_amf") || tail.contains("scale_d3d11"));
                if scaler_failed {
                    me.no_gpu_scale.store(true, Ordering::SeqCst);
                    util::log("warn", "recorder: the GPU scaler refused the game's frames, recording at the monitor's own size until Backtrack restarts");
                }
                if (ran_long || scaler_failed) && same_session {
                    // A transient failure (e.g. display mode change when a game goes full screen): restart.
                    util::log("info", "restarting recorder");
                    std::thread::sleep(std::time::Duration::from_secs(if scaler_failed { 0 } else { 2 }));
                    if let Err(e) = me.start(&tools2, &cfg2, mon, on_change.clone()) {
                        me.set_state("error", Some(e));
                        on_change();
                    }
                } else if same_session {
                    me.set_state("error", Some(format!("Recording stopped: {last}")));
                    on_change();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn read_stream(&self, mut out: std::process::ChildStdout, on_change: &Arc<dyn Fn() + Send + Sync>) {
        let mut buf = vec![0u8; TS * 1024];
        let mut carry: Vec<u8> = Vec::with_capacity(TS * 1100);
        let mut announced = false;
        loop {
            let n = match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            carry.extend_from_slice(&buf[..n]);
            let mut i = 0;
            let mut ring = self.ring.lock().unwrap();
            while i + TS <= carry.len() {
                if carry[i] != 0x47 {
                    i += 1; // lost sync; skip until the next sync byte
                    continue;
                }
                ring.push_packet(&carry[i..i + TS]);
                i += TS;
            }
            let has_chunk = !ring.chunks.is_empty();
            drop(ring);
            carry.drain(..i);
            if has_chunk && !announced {
                announced = true;
                self.set_state("on", None);
                util::log("info", "replay buffer running");
                on_change();
            }
        }
    }

    pub fn stop(&self) {
        let sess = self.session.lock().unwrap().take();
        if let Some(mut s) = sess {
            s.stopping.store(true, Ordering::SeqCst);
            if let Some(mut c) = s.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            s.audio.shutdown();
        }
        let mut ring = self.ring.lock().unwrap();
        *ring = Ring::default();
        drop(ring);
        self.set_state("off", None);
    }

    pub fn add_marker(&self) {
        let mut m = self.markers.lock().unwrap();
        m.push(util::now_ms());
        let len = m.len();
        if len > 100 {
            m.drain(..len - 100);
        }
    }

    pub fn status(&self) -> RecorderStatus {
        let (state, error) = self.state.lock().unwrap().clone();
        let cfg = self.cfg.lock().unwrap().clone();
        let (filled, mem) = {
            let r = self.ring.lock().unwrap();
            (r.filled_secs().min(cfg.buffer_secs as f64), (r.bytes + r.cur.len()) as f64 / 1e6)
        };
        let (w, h, fps) = *self.dims.lock().unwrap();
        let tracks = match self.session.lock().unwrap().as_ref() {
            Some(s) => s
                .audio
                .tracks
                .iter()
                .map(|t| TrackStatus { name: t.name.into(), source: t.source.lock().unwrap().clone(), level: t.level(), active: t.active.load(Ordering::Relaxed) })
                .collect(),
            None => vec![],
        };
        RecorderStatus {
            state,
            error,
            buffer_secs: cfg.buffer_secs,
            filled_secs: (filled * 10.0).round() / 10.0,
            memory_mb: mem.round(),
            encoder: self.encoder_desc.lock().unwrap().clone(),
            width: w,
            height: h,
            fps,
            game: self.game.lock().unwrap().clone(),
            mic_muted: self.mic_muted.load(Ordering::Relaxed),
            tracks,
            hotkeys: self.hotkeys.lock().unwrap().clone(),
            last_saved: self.last_saved.lock().unwrap().clone(),
        }
    }

    /// Save the last `secs` seconds (keyframe-aligned) as an .mp4 in `out_dir`, named after `game`.
    pub fn save(&self, tools: &Tools, secs: f64, game: &str, out_dir: &Path) -> Result<Saved, String> {
        if self.state() != "on" {
            return Err("The replay buffer isn't running.".into());
        }
        if self.saving.swap(true, Ordering::SeqCst) {
            return Err("Already saving a clip.".into());
        }
        let result = self.save_inner(tools, secs, game, out_dir);
        self.saving.store(false, Ordering::SeqCst);
        if let Ok(s) = &result {
            *self.last_saved.lock().unwrap() = Some(s.clone());
        }
        result
    }

    fn save_inner(&self, tools: &Tools, secs: f64, game: &str, out_dir: &Path) -> Result<Saved, String> {
        let now = util::now_ms();
        let (parts, first_wall, pat, pmt) = {
            let ring = self.ring.lock().unwrap();
            if ring.chunks.is_empty() {
                return Err("Nothing buffered yet; try again in a second.".into());
            }
            let start_wall = now.saturating_sub((secs * 1000.0) as u64);
            let idx = ring.chunks.iter().rposition(|c| c.wall_ms <= start_wall).unwrap_or(0);
            let mut parts: Vec<Arc<Vec<u8>>> = ring.chunks.iter().skip(idx).map(|c| c.data.clone()).collect();
            if !ring.cur.is_empty() {
                parts.push(Arc::new(ring.cur.clone()));
            }
            (parts, ring.chunks[idx].wall_ms, ring.pat.clone(), ring.pmt.clone())
        };
        let (Some(pat), Some(pmt)) = (pat, pmt) else { return Err("The stream isn't ready yet.".into()) };
        let duration = now.saturating_sub(first_wall) as f64 / 1000.0;
        let markers: Vec<f64> = self
            .markers
            .lock()
            .unwrap()
            .iter()
            .filter(|m| **m >= first_wall && **m <= now)
            .map(|m| ((*m - first_wall) as f64 / 1000.0 * 100.0).round() / 100.0)
            .collect();

        std::fs::create_dir_all(out_dir).map_err(|e| format!("Couldn't create the clips folder: {e}"))?;
        let stem = format!("{}__{}", file_safe_game(game), crate::winutil::local_stamp());
        let out = util::unique_path(out_dir, &stem, "mp4");
        let part = out.with_extension("part.mp4");
        let meta = serde_json::json!({ "game": game, "markers": markers, "recordedAt": first_wall, "app": "Backtrack" }).to_string();

        let mut child = tools
            .ffmpeg_cmd()
            .args(["-v", "error", "-y", "-f", "mpegts", "-i", "pipe:0", "-map", "0:v:0", "-map", "0:a?", "-c", "copy"])
            .args(["-avoid_negative_ts", "make_zero"])
            .args(["-metadata:s:v:0", "handler_name=VideoHandler"])
            .args(["-metadata:s:a:0", "handler_name=Game", "-metadata:s:a:1", "handler_name=Chat", "-metadata:s:a:2", "handler_name=Mic"])
            .arg("-metadata")
            .arg(format!("BACKTRACK_META={meta}"))
            .args(["-movflags", "+faststart+use_metadata_tags"])
            .arg(&part)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            stdin.write_all(&pat)?;
            stdin.write_all(&pmt)?;
            for p in parts {
                stdin.write_all(&p)?;
            }
            Ok(())
        });
        let w = writer.join();
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() || !matches!(w, Ok(Ok(()))) {
            let _ = std::fs::remove_file(&part);
            let err = String::from_utf8_lossy(&output.stderr);
            util::log("error", &format!("saving clip failed: {err}"));
            return Err(format!("Couldn't save the clip: {}", err.lines().last().unwrap_or("ffmpeg failed")));
        }
        std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
        util::log("info", &format!("saved replay {} ({duration:.1}s, game {game})", out.display()));
        Ok(Saved { path: out.to_string_lossy().into_owned(), game: game.to_string(), duration, at: now })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dims() {
        assert_eq!(output_dims(2560, 1440, 1080), (1920, 1080));
        assert_eq!(output_dims(2560, 1440, 0), (2560, 1440));
        assert_eq!(output_dims(2560, 1440, 1440), (2560, 1440));
        assert_eq!(output_dims(1440, 2560, 1080), (1080, 1920));
        assert_eq!(output_dims(1920, 1080, 1440), (1920, 1080));
    }
    #[test]
    fn names() {
        assert_eq!(file_safe_game("Rocket League"), "Rocket-League");
        assert_eq!(file_safe_game("Call of Duty: Modern Warfare 2 (2009)"), "Call-of-Duty-Modern-Warfare-2-(2009)");
        assert_eq!(file_safe_game("HELLDIVERS™ 2"), "HELLDIVERS™-2");
    }
}
