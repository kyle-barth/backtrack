//! Hardware test: a recording gets a new frame in every slot the game drew one for, so clips play as smoothly as the
//! game did.
//!
//! A window covers a monitor like a borderless game (a flip-model swap chain on the GPU that drives it) and shows a
//! frame counter (a row of black and white squares) that changes on every frame it presents: with vsync at the
//! monitor's refresh rate and at a half, a third and a quarter of it, and without vsync at 144, 100 and 75 fps.
//! The recorder's capture pipeline records it with the recorder's timing arguments, and the clip is decoded to count
//! how many of its frames repeat the one before (a repeat is a visible hitch). That is compared with the repeats a
//! recording can't avoid: the 60 fps slots in which the window presented nothing new.
//! A second test does the same while the window keeps the GPU busy like a demanding game.
//!
//! v0.4.0 had Windows deliver frames at most at the clip's rate, which held back frames that came a little early: it
//! repeated a fifth of frames from a game at 144 fps with vsync off, and its clips of Rocket League stuttered. Its
//! pipeline is recorded too, for comparison (printed, not checked).
//!
//! They cover a monitor for about two minutes (without taking focus); close Backtrack first, as a second capture of
//! the same monitor changes the results:
//!   cargo test frame_rate -- --ignored --nocapture --test-threads=1
//! BACKTRACK_TEST_MONITOR=<index> picks the monitor (default 0, the recorder's numbering).

use super::colour_test::{monitors, wndproc};
use super::*;
use std::path::PathBuf;
use std::time::Duration;
use windows::core::{w, Interface};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const FPS: u32 = 60;
const SECS: u32 = 4;
/// The frame counter: a row of `BITS` black or white squares of `SQUARE` monitor pixels at the top left.
const SQUARE: i32 = 64;
const BITS: i32 = 8;
/// Repeats allowed beyond the unavoidable ones, in percent of a recording's frames.
const SLACK: f64 = 3.0;

/// How the window paces its frames.
#[derive(Clone, Copy)]
enum Pace {
    /// Present every n-th refresh (vsync), like a capped game whose rate divides the refresh rate.
    Vsync(u32),
    /// Present at this rate without vsync, like a game with vsync off.
    Fps(f64),
    /// Present as fast as the GPU allows without vsync, copying a 128 MB texture this many times a frame to keep the
    /// GPU busy, like a demanding uncapped game.
    Loaded(u32),
}

/// The window's presents so far, by when each returned.
type Presents = Arc<Mutex<Vec<Instant>>>;

/// Cover the monitor with dark grey and the frame counter, which changes on every present, until `stop` is set.
fn show_counter(monitor: isize, r: RECT, pace: Pace, stop: Arc<AtomicBool>, presents: Presents) -> std::thread::JoinHandle<windows::core::Result<()>> {
    std::thread::spawn(move || unsafe {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let inst = GetModuleHandleW(None)?;
        let class = w!("BacktrackFrameTest");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, class, w!("Backtrack frame test"), WS_POPUP,
            r.left, r.top, w, h, None, None, Some(inst.into()), None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let mut adapter: Option<IDXGIAdapter> = None;
        'find: for i in 0.. {
            let Ok(a) = factory.EnumAdapters1(i) else { break };
            for j in 0.. {
                let Ok(o) = a.EnumOutputs(j) else { break };
                if o.GetDesc()?.Monitor.0 as isize == monitor {
                    adapter = Some(a.cast()?);
                    break 'find;
                }
            }
        }
        let driver = if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE };
        let (mut device, mut ctx) = (None, None);
        D3D11CreateDevice(adapter.as_ref(), driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))?;
        let device: ID3D11Device = device.unwrap();
        let ctx: ID3D11DeviceContext1 = ctx.unwrap().cast()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w as u32,
            Height: h as u32,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            ..Default::default()
        };
        let chain = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
        let buffer: ID3D11Texture2D = chain.GetBuffer(0)?;
        let mut rtv = None;
        device.CreateRenderTargetView(&buffer, None, Some(&mut rtv))?;
        let rtv = rtv.unwrap();
        let load = match pace {
            Pace::Loaded(_) => {
                let td = D3D11_TEXTURE2D_DESC {
                    Width: 4096,
                    Height: 4096,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                };
                let (mut a, mut b) = (None, None);
                device.CreateTexture2D(&td, None, Some(&mut a))?;
                device.CreateTexture2D(&td, None, Some(&mut b))?;
                Some((a.unwrap(), b.unwrap()))
            }
            _ => None,
        };
        let mut msg = MSG::default();
        let mut k = 0u64;
        let start = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            ctx.ClearView(&rtv, &[0.12, 0.12, 0.12, 1.0], None);
            for bit in 0..BITS {
                let v = ((k >> bit) & 1) as f32;
                let rect = RECT { left: bit * SQUARE, top: 0, right: (bit + 1) * SQUARE, bottom: SQUARE };
                ctx.ClearView(&rtv, &[v, v, v, 1.0], Some(&[rect]));
            }
            match pace {
                Pace::Vsync(n) => chain.Present(n, DXGI_PRESENT(0)).ok()?,
                Pace::Fps(fps) => {
                    // Wait for this frame's time (spinning: Windows' sleep is too coarse), then present at once.
                    let due = start + Duration::from_secs_f64((k + 1) as f64 / fps);
                    while Instant::now() < due {
                        std::thread::yield_now();
                    }
                    chain.Present(0, DXGI_PRESENT(0)).ok()?
                }
                Pace::Loaded(copies) => {
                    if let Some((a, b)) = &load {
                        for i in 0..copies {
                            if i % 2 == 0 { ctx.CopyResource(b, a) } else { ctx.CopyResource(a, b) }
                        }
                    }
                    chain.Present(0, DXGI_PRESENT(0)).ok()?
                }
            }
            k += 1;
            presents.lock().unwrap().push(Instant::now());
        }
        drop((rtv, buffer, chain));
        DestroyWindow(hwnd)?;
        Ok(())
    })
}

/// Percent of a 60 fps recording's frames that repeat even if it always gets the newest frame presented: slots in
/// which nothing new was presented between `from` and `to`. Averaged over where the slots fall.
fn unavoidable_repeats(presents: &[Instant], from: Instant, to: Instant) -> f64 {
    let slot = Duration::from_secs_f64(1.0 / FPS as f64);
    let phases = 12;
    let mut sum = 0.0;
    for phase in 0..phases {
        let mut t = from + slot.mul_f64(phase as f64 / phases as f64);
        let mut seen = presents.partition_point(|&p| p <= t);
        let (mut slots, mut repeats) = (0u32, 0u32);
        while t + slot <= to {
            t += slot;
            let now = presents.partition_point(|&p| p <= t);
            slots += 1;
            repeats += (now == seen) as u32;
            seen = now;
        }
        sum += repeats as f64 / slots.max(1) as f64;
    }
    100.0 * sum / phases as f64
}

/// How a recording turned out.
struct Smoothness {
    /// Percent of its frames that repeat the one before.
    repeats: f64,
    /// The longest run of one frame, in frames.
    longest: usize,
    /// Percent that would repeat anyway (see unavoidable_repeats).
    unavoidable: f64,
    /// Frames the window presented per second meanwhile.
    drawn: f64,
}

/// Record `secs` s of the monitor with `src` and `rest` and the recorder's timing arguments while the window presents,
/// and count repeated frames.
fn record(tools: &Tools, src: &str, rest: &[String], (w, mon_w): (u32, u32), secs: u32, presents: &Presents, ts: &Path) -> Result<Smoothness, String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
    let started = Instant::now();
    let rec = tools
        .ffmpeg_cmd()
        .args(["-v", "error", "-y", "-thread_queue_size", "64", "-use_wallclock_as_timestamps", "1", "-itsoffset"])
        .arg(format!("-{now:.6}"))
        .args(["-f", "lavfi", "-i", src])
        .args(rest)
        .args(["-b:v", "20M", "-g", "60", "-bf", "0", "-fps_mode", "cfr", "-r", "60", "-copyts", "-t"])
        .arg(secs.to_string())
        .args(["-f", "mpegts"])
        .arg(ts)
        .output()
        .unwrap();
    let ended = Instant::now();
    if !rec.status.success() {
        return Err(format!("recording failed: {}", String::from_utf8_lossy(&rec.stderr).lines().next().unwrap_or("")));
    }
    // The counter's squares, scaled like the recording, each averaged to one grey value, read back as bits.
    let side = (SQUARE as u32 * w / mon_w).max(4);
    let crop = format!("crop={}:{side}:0:0,scale={BITS}:1:flags=area,format=gray", side * BITS as u32);
    let dec = tools.ffmpeg_cmd().args(["-v", "error", "-i"]).arg(ts).args(["-vf", &crop, "-f", "rawvideo", "-"]).output().unwrap();
    let counters: Vec<u32> = dec.stdout.chunks_exact(BITS as usize).map(|px| px.iter().enumerate().map(|(b, &v)| ((v > 128) as u32) << b).sum()).collect();
    if counters.len() < (FPS * secs / 2) as usize {
        return Err(format!("decoding failed: {}", String::from_utf8_lossy(&dec.stderr).lines().next().unwrap_or("")));
    }
    // Skip the first half second (the capture starting up).
    let counters = &counters[(FPS / 2) as usize..];
    let (mut repeated, mut run, mut longest) = (0, 0, 0);
    for pair in counters.windows(2) {
        if pair[1] == pair[0] {
            repeated += 1;
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    // The presents while it recorded (ffmpeg starts in well under a second; the first half second is skipped).
    let presents = presents.lock().unwrap().clone();
    let from = (started + Duration::from_millis(1000)).min(ended);
    let drawn = presents.iter().filter(|&&p| p >= from && p <= ended).count() as f64 / (ended - from).as_secs_f64().max(0.001);
    Ok(Smoothness {
        repeats: 100.0 * repeated as f64 / (counters.len() - 1) as f64,
        longest: longest + 1,
        unavoidable: unavoidable_repeats(&presents, from, ended),
        drawn,
    })
}

/// One way to capture: a name, the capture source and the filter and encoder arguments.
type Variant = (String, String, Vec<String>);

/// The recorder's pipeline and, when it captures through Windows Graphics Capture, v0.4.0's for comparison.
fn variants(encoder: &str, gfx: bool, monitor: u32, size: (u32, u32), scaled: bool) -> Vec<Variant> {
    let (src, rest) = video_pipeline(encoder, gfx, monitor, FPS, size, scaled);
    let mut variants = vec![("recorder".to_string(), src.clone(), rest.clone())];
    let fast = format!("max_framerate={}", FPS * 4);
    if gfx && src.contains(&fast) {
        let mut r = rest;
        let vf = r.iter().position(|a| a == "-vf").unwrap() + 1;
        r[vf] = r[vf].replacen(&format!("fps={FPS},"), "", 1);
        variants.push(("v0.4.0".to_string(), src.replacen(&fast, &format!("max_framerate={FPS}"), 1), r));
    }
    variants
}

/// Record with each variant while the window presents at `pace`; print how each did and return the recorder's
/// failure, if any.
fn check(tools: &Tools, variants: &[Variant], (hmon, rect): (isize, RECT), sizes: (u32, u32), pace: Pace, secs: u32, dir: &Path) -> Option<String> {
    let stop = Arc::new(AtomicBool::new(false));
    let presents: Presents = Arc::default();
    let window = show_counter(hmon, rect, pace, stop.clone(), presents.clone());
    std::thread::sleep(Duration::from_millis(1500));
    let what = match pace {
        Pace::Vsync(n) => format!("every {n} refresh"),
        Pace::Fps(_) => "vsync off".into(),
        Pace::Loaded(copies) => format!("vsync off, GPU busy with {copies} copies a frame"),
    };
    let mut failure = None;
    for (i, (name, src, rest)) in variants.iter().enumerate() {
        match record(tools, src, rest, sizes, secs, &presents, &dir.join(format!("{i}.ts"))) {
            Ok(s) => {
                let ok = s.repeats <= s.unavoidable + SLACK;
                println!(
                    "  {} window drawing {:.0} fps ({what}): {name}: {:.1}% repeats ({:.1}% unavoidable), longest freeze {} frames",
                    if ok { "ok  " } else { "FAIL" }, s.drawn, s.repeats, s.unavoidable, s.longest
                );
                if !ok && i == 0 {
                    failure = Some(format!("drawing {:.0} fps ({what}): {:.1}% of frames repeat, {:.1}% unavoidable", s.drawn, s.repeats, s.unavoidable));
                }
            }
            Err(e) => {
                println!("  FAIL {name}: {e}");
                if i == 0 {
                    failure = Some(e);
                }
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    window.join().unwrap().expect("test window");
    failure
}

/// What both tests need.
struct Setup {
    tools: Tools,
    /// The monitor's handle and rectangle.
    monitor: (isize, RECT),
    /// The recording's width and the monitor's.
    sizes: (u32, u32),
    variants: Vec<Variant>,
    /// A scratch folder.
    dir: PathBuf,
}

fn setup() -> Setup {
    let tools = crate::ffmpeg::locate().expect("ffmpeg");
    let index: usize = std::env::var("BACKTRACK_TEST_MONITOR").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let (hmon, rect) = monitors()[index];
    let native = (even((rect.right - rect.left) as f64), even((rect.bottom - rect.top) as f64));
    let encoder = detect_encoder(&tools);
    let gfx = encoder == "amf" && gfx_capture(&tools);
    let (w, h) = output_dims(native.0, native.1, 1080);
    println!("encoder {encoder}, {}, monitor {index} ({}x{}), recording {w}x{h}@{FPS}", if gfx { "Windows Graphics Capture" } else { "encoder's own capture" }, native.0, native.1);
    let variants = variants(&encoder, gfx, index as u32, (w, h), (w, h) != native);
    let dir = std::env::temp_dir().join("backtrack-frame-test");
    std::fs::create_dir_all(&dir).unwrap();
    Setup { tools, monitor: (hmon, rect), sizes: (w, native.0), variants, dir }
}

#[test]
#[ignore]
fn frame_rate_recording_keeps_every_frame() {
    let Setup { tools, monitor, sizes, variants, dir } = setup();
    let paces = [Pace::Vsync(1), Pace::Vsync(2), Pace::Vsync(3), Pace::Vsync(4), Pace::Fps(144.0), Pace::Fps(100.0), Pace::Fps(75.0)];
    let failures: Vec<String> = paces.into_iter().filter_map(|pace| check(&tools, &variants, monitor, sizes, pace, SECS, &dir)).collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "recordings repeat frames the window drew:\n{}", failures.join("\n"));
}

/// The same with the GPU kept busy like a demanding game: BACKTRACK_TEST_LOAD=<copies a frame> (default 12, ~220 fps
/// here; 30 is ~95 fps), recorded for 8 s.
///   cargo test frame_rate_under -- --ignored --nocapture
#[test]
#[ignore]
fn frame_rate_under_gpu_load() {
    let Setup { tools, monitor, sizes, variants, dir } = setup();
    let copies: u32 = std::env::var("BACKTRACK_TEST_LOAD").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let failure = check(&tools, &variants, monitor, sizes, Pace::Loaded(copies), 8, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failure.is_none(), "{}", failure.unwrap_or_default());
}
