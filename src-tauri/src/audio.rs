//! The replay buffer's three audio tracks, captured with WASAPI and streamed to ffmpeg over named pipes as
//! 48 kHz stereo f32. Each track is paced by the wall clock from a shared start time, the clock ffmpeg stamps
//! the video with, so the tracks stay aligned with the video and with each other (see [`frames_due`]). When a
//! source has nothing to play (loopback sends no data while silent), the gap is filled with silence.
//!
//! - Game:       per-process loopback in EXCLUDE mode: everything the PC plays except the chat app.
//! - Voice chat: per-process loopback in INCLUDE mode on the chat app's process tree (Discord).
//! - Mic:        the default or chosen capture device.

use crate::util;
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use wasapi::{AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode, WaveFormat};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING, PIPE_ACCESS_OUTBOUND};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

pub const RATE: usize = 48_000;
const FRAME_BYTES: usize = 8; // f32 × 2 channels
/// Most silence added in one write (0.5 s), so a jump of the clock is caught up in steps.
const MAX_PAD: usize = RATE / 2;

/// Audio frames due at `now` for tracks started at `t0`.
///
/// The video's timestamps come from the wall clock (ffmpeg's -use_wallclock_as_timestamps), so the audio is
/// paced by the same clock. Paced by a monotonic clock (as up to v0.4.2), the two drifted apart as Windows'
/// time sync adjusted the wall clock (2.2 s in 43 hours on one PC). ffmpeg holds a stream back while it's more
/// than 100 ms ahead of the others, so after an hour or two of recording it held the video back, Windows
/// dropped the frames it couldn't hand over, and clips repeated up to 42% of their frames.
pub(crate) fn frames_due(t0: SystemTime, now: SystemTime) -> u64 {
    now.duration_since(t0).map(|d| (d.as_secs_f64() * RATE as f64) as u64).unwrap_or(0)
}

/// Silence to add when the source has only `take` of the `need` frames due: none within `jitter`, then at most
/// [`MAX_PAD`] a write (the clock jumping forward makes `need` hours long).
fn pad_frames(need: usize, take: usize, jitter: usize) -> usize {
    if need - take > jitter {
        (need - take - jitter / 2).min(MAX_PAD)
    } else {
        0
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Source {
    AllExceptChat,
    Chat,
    Mic,
}

pub struct Track {
    pub name: &'static str,
    pub kind: Source,
    pub source: Mutex<String>,
    level: AtomicU32,
    pub active: AtomicBool,
}

impl Track {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

pub struct AudioConfig {
    pub chat_app: String,
    pub mic_device: String,
    pub mic_enabled: bool,
}

pub struct Audio {
    pub tracks: [Track; 3],
    pub cfg: AudioConfig,
    pub mic_muted: Arc<AtomicBool>,
    pub stop: AtomicBool,
    chat_root: Mutex<Option<u32>>,
    mic_default_id: Mutex<String>,
    /// Bumped when a source changes (chat app started/stopped, default mic changed); capture threads reopen.
    generation: AtomicU64,
    pub pipe_names: Vec<String>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_pipe(name: &str) -> std::io::Result<File> {
    let n = wide(name);
    let h = unsafe {
        CreateNamedPipeW(
            n.as_ptr(),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            1 << 20,
            0,
            0,
            std::ptr::null(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(h as _) })
}

fn connect_pipe(f: &File) -> std::io::Result<()> {
    let ok = unsafe { ConnectNamedPipe(f.as_raw_handle() as _, std::ptr::null_mut()) };
    if ok == 0 {
        let e = unsafe { GetLastError() };
        if e != ERROR_PIPE_CONNECTED {
            return Err(std::io::Error::from_raw_os_error(e as i32));
        }
    }
    Ok(())
}

/// Open our own pipe as a client so a thread blocked in ConnectNamedPipe wakes up (used when stopping).
fn poke_pipe(name: &str) {
    let n = wide(name);
    let h = unsafe { CreateFileW(n.as_ptr(), GENERIC_READ, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h != INVALID_HANDLE_VALUE {
        unsafe { CloseHandle(h) };
    }
}

/// Executables that belong to a chat app.
fn chat_exes(app: &str) -> &'static [&'static str] {
    match app.to_lowercase().as_str() {
        "discord" => &["discord.exe", "discordptb.exe", "discordcanary.exe"],
        _ => &[],
    }
}

/// (pid, parent pid, lowercase exe name) for every process.
pub fn processes() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase()));
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// The root process of the chat app (Discord's processes form a tree under one Discord.exe).
pub fn chat_root(app: &str) -> Option<u32> {
    let exes = chat_exes(app);
    if exes.is_empty() {
        return None;
    }
    let procs = processes();
    let is_chat = |pid: u32| procs.iter().any(|(p, _, n)| *p == pid && exes.contains(&n.as_str()));
    procs
        .iter()
        .filter(|(_, _, n)| exes.contains(&n.as_str()))
        .find(|(_, parent, _)| !is_chat(*parent))
        .map(|(pid, _, _)| *pid)
}

fn default_mic_id() -> String {
    DeviceEnumerator::new()
        .and_then(|e| e.get_default_device(Direction::Capture))
        .and_then(|d| d.get_id())
        .unwrap_or_default()
}

fn mic_name(cfg: &AudioConfig) -> String {
    let dev = DeviceEnumerator::new().and_then(|e| {
        if cfg.mic_device.is_empty() {
            e.get_default_device(Direction::Capture)
        } else {
            e.get_device(&cfg.mic_device)
        }
    });
    dev.and_then(|d| d.get_friendlyname()).unwrap_or_else(|_| "No microphone".into())
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_inputs() -> Vec<InputDevice> {
    let _ = wasapi::initialize_mta();
    let mut out = Vec::new();
    let Ok(e) = DeviceEnumerator::new() else { return out };
    let def = e.get_default_device(Direction::Capture).and_then(|d| d.get_id()).unwrap_or_default();
    if let Ok(coll) = e.get_device_collection(Direction::Capture) {
        for d in (&coll).into_iter().flatten() {
            if let (Ok(id), Ok(name)) = (d.get_id(), d.get_friendlyname()) {
                out.push(InputDevice { is_default: id == def, id, name });
            }
        }
    }
    out
}

struct Capture {
    _client: AudioClient,
    cc: AudioCaptureClient,
    event: Handle,
}

fn open_capture(kind: Source, audio: &Audio) -> Result<Option<Capture>, String> {
    let fmt = WaveFormat::new(32, 32, SampleType::Float, RATE, 2, None);
    let e = |x: wasapi::WasapiError| x.to_string();
    let mut client = match kind {
        Source::AllExceptChat => {
            // Exclude the chat app's tree; with no chat app, exclude ourselves (the editor's preview audio).
            let target = audio.chat_root.lock().unwrap().unwrap_or_else(std::process::id);
            AudioClient::new_application_loopback_client(target, false).map_err(e)?
        }
        Source::Chat => match *audio.chat_root.lock().unwrap() {
            Some(pid) => AudioClient::new_application_loopback_client(pid, true).map_err(e)?,
            None => return Ok(None),
        },
        Source::Mic => {
            if !audio.cfg.mic_enabled {
                return Ok(None);
            }
            let en = DeviceEnumerator::new().map_err(e)?;
            let dev = if audio.cfg.mic_device.is_empty() {
                en.get_default_device(Direction::Capture)
            } else {
                en.get_device(&audio.cfg.mic_device).or_else(|_| en.get_default_device(Direction::Capture))
            }
            .map_err(e)?;
            dev.get_iaudioclient().map_err(e)?
        }
    };
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: if kind == Source::Mic { 200_000 } else { 0 } };
    client.initialize_client(&fmt, Direction::Capture, mode, None).map_err(e)?;
    let event = client.set_get_eventhandle().map_err(e)?;
    let cc = client.get_audiocaptureclient().map_err(e)?;
    client.start_stream().map_err(e)?;
    Ok(Some(Capture { _client: client, cc, event }))
}

impl Audio {
    pub fn new(cfg: AudioConfig, mic_muted: Arc<AtomicBool>, session: u64) -> Arc<Audio> {
        let pid = std::process::id();
        let chat = chat_root(&cfg.chat_app);
        let mk = |name: &'static str, kind: Source| Track {
            name,
            kind,
            source: Mutex::new(String::new()),
            level: AtomicU32::new(0),
            active: AtomicBool::new(false),
        };
        Arc::new(Audio {
            tracks: [mk("Game", Source::AllExceptChat), mk("Voice chat", Source::Chat), mk("Mic", Source::Mic)],
            pipe_names: (0..3).map(|i| format!(r"\\.\pipe\backtrack-{pid}-{session}-{i}")).collect(),
            cfg,
            mic_muted,
            stop: AtomicBool::new(false),
            chat_root: Mutex::new(chat),
            mic_default_id: Mutex::new(String::new()),
            generation: AtomicU64::new(1),
        })
    }

    fn describe(&self) {
        let chat = self.cfg.chat_app.clone();
        let running = self.chat_root.lock().unwrap().is_some();
        *self.tracks[0].source.lock().unwrap() =
            if chat.is_empty() { "All apps".into() } else { format!("All apps except {chat}") };
        *self.tracks[1].source.lock().unwrap() = if chat.is_empty() {
            "No chat app".into()
        } else if running {
            chat.clone()
        } else {
            format!("{chat} (not running)")
        };
        *self.tracks[2].source.lock().unwrap() =
            if self.cfg.mic_enabled { mic_name(&self.cfg) } else { "Off".into() };
    }

    /// Create the pipes and start one capture thread per track, plus a watcher for source changes.
    /// Returns the pipe paths for ffmpeg's inputs. All tracks are clocked from `t0` (see [`frames_due`]).
    pub fn start(self: &Arc<Self>, t0: SystemTime) -> Result<(), String> {
        let _ = wasapi::initialize_mta();
        *self.mic_default_id.lock().unwrap() = default_mic_id();
        self.describe();
        for (i, name) in self.pipe_names.iter().enumerate() {
            let pipe = create_pipe(name).map_err(|e| format!("Couldn't create the audio pipe: {e}"))?;
            let me = self.clone();
            std::thread::Builder::new()
                .name(format!("audio-{i}"))
                .spawn(move || me.run_track(i, pipe, t0))
                .map_err(|e| e.to_string())?;
        }
        let me = self.clone();
        std::thread::spawn(move || me.watch_sources());
        Ok(())
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for n in &self.pipe_names {
            poke_pipe(n);
        }
    }

    fn watch_sources(self: Arc<Self>) {
        let _ = wasapi::initialize_mta();
        while !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(2));
            let mut changed = false;
            let root = chat_root(&self.cfg.chat_app);
            {
                let mut cur = self.chat_root.lock().unwrap();
                if *cur != root {
                    util::log("info", &format!("chat app process changed: {:?} -> {:?}", *cur, root));
                    *cur = root;
                    changed = true;
                }
            }
            if self.cfg.mic_device.is_empty() {
                let id = default_mic_id();
                let mut cur = self.mic_default_id.lock().unwrap();
                if *cur != id {
                    *cur = id;
                    changed = true;
                }
            }
            if changed {
                self.describe();
                self.generation.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn run_track(self: Arc<Self>, idx: usize, mut pipe: File, t0: SystemTime) {
        let _ = wasapi::initialize_mta();
        let track = &self.tracks[idx];
        if let Err(e) = connect_pipe(&pipe) {
            util::log("error", &format!("audio pipe {idx}: {e}"));
            return;
        }
        let mut queue: VecDeque<u8> = VecDeque::with_capacity(RATE * FRAME_BYTES);
        let mut capture: Option<Capture> = None;
        let mut seen_gen = 0u64;
        let mut written: u64 = 0;
        let mut level = 0f32;
        let mut last_tick = Instant::now();
        let jitter = RATE / 25; // 40 ms of slack before filling with silence
        let mut out: Vec<u8> = Vec::with_capacity(RATE * FRAME_BYTES / 10);

        while !self.stop.load(Ordering::SeqCst) {
            let g = self.generation.load(Ordering::SeqCst);
            if g != seen_gen {
                seen_gen = g;
                capture = None;
                queue.clear();
                match open_capture(track.kind, &self) {
                    Ok(c) => {
                        track.active.store(c.is_some(), Ordering::Relaxed);
                        capture = c;
                    }
                    Err(e) => {
                        track.active.store(false, Ordering::Relaxed);
                        util::log("warn", &format!("{} capture unavailable: {e}", track.name));
                    }
                }
            }

            // Collect whatever the source has (or just pace ourselves when there's no source).
            match &capture {
                Some(c) => {
                    let _ = c.event.wait_for_event(10);
                    loop {
                        match c.cc.get_next_packet_size() {
                            Ok(Some(n)) if n > 0 => {
                                if c.cc.read_from_device_to_deque(&mut queue).is_err() {
                                    break;
                                }
                            }
                            Ok(_) => break,
                            Err(_) => {
                                // Device gone (unplugged): try reopening once on the next pass.
                                seen_gen = 0;
                                break;
                            }
                        }
                    }
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }

            // Write exactly as many frames as the wall clock says are due.
            let due = frames_due(t0, SystemTime::now());
            let need = due.saturating_sub(written) as usize;
            if need == 0 {
                // Nothing due for a while only if the clock went back: keep at most a second of what came in.
                let max = RATE * FRAME_BYTES;
                if queue.len() > max {
                    queue.drain(..queue.len() - max);
                }
                continue;
            }
            let avail = queue.len() / FRAME_BYTES;
            // Too much backlog (we fell behind, or padded silence before late data arrived): drop the oldest.
            if avail > need + RATE / 5 {
                let drop = avail - need - RATE / 50;
                queue.drain(..drop * FRAME_BYTES);
            }
            let avail = queue.len() / FRAME_BYTES;
            let take = avail.min(need);
            let pad = pad_frames(need, take, jitter);
            if take + pad == 0 {
                continue;
            }
            out.clear();
            out.extend(queue.drain(..take * FRAME_BYTES));
            let silent = (track.kind == Source::Mic && self.mic_muted.load(Ordering::Relaxed)) || capture.is_none();
            if silent {
                out.iter_mut().for_each(|b| *b = 0);
            }
            out.resize(out.len() + pad * FRAME_BYTES, 0);

            // Level meter: peak with a ~250 ms decay.
            let peak = out.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).abs()).fold(0f32, f32::max);
            let dt = last_tick.elapsed().as_secs_f32();
            last_tick = Instant::now();
            level = peak.max(level * (-dt / 0.25).exp()).min(1.0);
            track.level.store(level.to_bits(), Ordering::Relaxed);

            if pipe.write_all(&out).is_err() {
                break; // ffmpeg went away
            }
            written += (take + pad) as u64;
        }
        track.level.store(0f32.to_bits(), Ordering::Relaxed);
        track.active.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_paced_by_the_wall_clock_like_the_video() {
        // The recorder stamps video with the wall clock; the audio must count its samples on that same clock,
        // or the two drift apart over hours and ffmpeg holds the video back (clips repeat frames).
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_375_747);
        assert_eq!(frames_due(t0, t0), 0);
        assert_eq!(frames_due(t0, t0 + Duration::from_millis(1500)), RATE as u64 * 3 / 2);
        assert_eq!(frames_due(t0, t0 + Duration::from_secs(7200)), RATE as u64 * 7200);
        // The clock set back before the start: nothing due, rather than a panic or a wrap-around.
        assert_eq!(frames_due(t0, t0 - Duration::from_secs(5)), 0);
    }

    #[test]
    fn silence_is_added_in_bounded_steps() {
        let jitter = RATE / 25;
        // Within the jitter allowance: wait for the source rather than pad.
        assert_eq!(pad_frames(jitter, 0, jitter), 0);
        assert_eq!(pad_frames(RATE / 10, RATE / 10, jitter), 0);
        // A source that went quiet: pad what's due, less half the allowance.
        assert_eq!(pad_frames(RATE / 10, 0, jitter), RATE / 10 - jitter / 2);
        // The clock jumping an hour forward: half a second a write, not an hour of silence at once.
        assert_eq!(pad_frames(RATE * 3600, 0, jitter), MAX_PAD);
    }
}
