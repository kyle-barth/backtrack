//! Hardware test: a recording keeps the colours that were on screen, including their colour temperature.
//!
//! A window fills a monitor with colour swatches the way a game does (a flip-model swap chain on the GPU that drives
//! that monitor, first RGBA like Rocket League, then BGRA), the recorder's own capture pipeline records it at 1080p
//! and at the monitor's size and it's saved like a clip, and the decoded clip is compared with what was drawn.
//! AMD's capture once handed RGBA game frames on labelled as BGRA, which swapped red and blue and made clips look
//! blue; and a clip whose stream doesn't say it's full range plays too dark.
//!
//! It covers a monitor for about 10 s (without taking focus):
//!   cargo test colour -- --ignored --nocapture
//! BACKTRACK_TEST_MONITOR=<index> picks the monitor (default 0, the recorder's numbering);
//! BACKTRACK_TEST_CAPTURE=vsrc_amf tries AMD's old capture instead.

use super::*;
use std::time::Duration;
use windows::core::{w, Interface, BOOL};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Colours a game might fill the screen with (sRGB, 0–255): warm orange, cool blue, a skin tone.
const SWATCHES: [[u8; 3]; 3] = [[230, 120, 30], [40, 110, 220], [200, 150, 120]];
/// Largest difference allowed per channel, and for the warmth (red minus blue), after H.264 4:2:0.
const TOLERANCE: i32 = 12;

fn monitors() -> Vec<(isize, RECT)> {
    unsafe extern "system" fn add(m: HMONITOR, _: HDC, r: *mut RECT, list: LPARAM) -> BOOL {
        unsafe { (*(list.0 as *mut Vec<(isize, RECT)>)).push((m.0 as isize, *r)) };
        BOOL(1)
    }
    let mut list: Vec<(isize, RECT)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

/// Fill the monitor with the swatches through a flip-model swap chain of `format` until `stop` is set.
fn show_swatches(monitor: isize, r: RECT, format: DXGI_FORMAT, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<windows::core::Result<()>> {
    std::thread::spawn(move || unsafe {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let inst = GetModuleHandleW(None)?;
        let class = w!("BacktrackColourTest");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, class, w!("Backtrack colour test"), WS_POPUP,
            r.left, r.top, w, h, None, None, Some(inst.into()), None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        // Render on the GPU that drives this monitor, so the display can take the frames straight from the swap
        // chain, like a full-screen game's.
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
            Format: format,
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
        let band = w / SWATCHES.len() as i32;
        let mut msg = MSG::default();
        while !stop.load(Ordering::SeqCst) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            for (i, c) in SWATCHES.iter().enumerate() {
                let right = if i == SWATCHES.len() - 1 { w } else { band * (i as i32 + 1) };
                let rect = RECT { left: band * i as i32, top: 0, right, bottom: h };
                ctx.ClearView(&rtv, &[c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0], Some(&[rect]));
            }
            chain.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        drop((rtv, buffer, chain));
        DestroyWindow(hwnd)?;
        Ok(())
    })
}

/// Record 2 s of the monitor at `(w, h)` with the recorder's pipeline and return the swatches' average colours in
/// the clip, or why recording failed.
fn record_swatches(tools: &Tools, encoder: &str, gfx: bool, monitor: u32, native: (u32, u32), (w, h): (u32, u32), out: &Path) -> Result<Vec<[i32; 3]>, String> {
    let (src, rest) = video_pipeline(encoder, gfx, monitor, 60, (w, h), (w, h) != native);
    // Like the recorder: MPEG-TS out of ffmpeg, then a saved clip is a stream copy of that into MP4.
    let ts = out.with_extension("ts");
    let rec = tools.ffmpeg_cmd().args(["-v", "error", "-y", "-f", "lavfi", "-i"]).arg(&src).args(&rest).args(["-b:v", "20M", "-t", "2", "-f", "mpegts"]).arg(&ts).output().unwrap();
    if !rec.status.success() {
        return Err(format!("recording failed: {}", String::from_utf8_lossy(&rec.stderr).lines().next().unwrap_or("")));
    }
    let save = tools.ffmpeg_cmd().args(["-v", "error", "-y", "-f", "mpegts", "-i"]).arg(&ts).args(["-map", "0:v:0", "-c", "copy", "-movflags", "+faststart"]).arg(out).output().unwrap();
    if !save.status.success() {
        return Err(format!("saving failed: {}", String::from_utf8_lossy(&save.stderr).lines().next().unwrap_or("")));
    }
    // Decode one frame to RGB the way any player would (ffmpeg follows the clip's colour tags).
    let dec = tools.ffmpeg_cmd().args(["-v", "error", "-ss", "1", "-i"]).arg(out).args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).output().unwrap();
    if dec.stdout.len() != (w * h * 3) as usize {
        return Err(format!("decoding failed: {}", String::from_utf8_lossy(&dec.stderr).lines().next().unwrap_or("")));
    }
    let n = SWATCHES.len();
    let colours = (0..n)
        .map(|i| {
            let (cx, cy) = (((i as f64 + 0.5) / n as f64 * w as f64) as u32, h / 2);
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for y in cy - 16..cy + 16 {
                for x in cx - 16..cx + 16 {
                    let p = ((y * w + x) * 3) as usize;
                    for c in 0..3 {
                        sum[c] += dec.stdout[p + c] as u64;
                    }
                    count += 1;
                }
            }
            sum.map(|s| (s / count) as i32)
        })
        .collect();
    Ok(colours)
}

#[test]
#[ignore]
fn colour_recording_matches_the_screen() {
    let tools = crate::ffmpeg::locate().expect("ffmpeg");
    let index: usize = std::env::var("BACKTRACK_TEST_MONITOR").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let (hmon, rect) = monitors()[index];
    let native = (even((rect.right - rect.left) as f64), even((rect.bottom - rect.top) as f64));
    let encoder = detect_encoder(&tools);
    let gfx = encoder == "amf" && std::env::var("BACKTRACK_TEST_CAPTURE").as_deref() != Ok("vsrc_amf") && gfx_capture(&tools);
    println!("encoder {encoder}, {}, monitor {index} ({}x{})", if gfx { "Windows Graphics Capture" } else { "encoder's own capture" }, native.0, native.1);
    let dir = std::env::temp_dir().join("backtrack-colour-test");
    std::fs::create_dir_all(&dir).unwrap();

    let mut failures = Vec::new();
    for (format, fname) in [(DXGI_FORMAT_R8G8B8A8_UNORM, "RGBA swap chain (like Rocket League)"), (DXGI_FORMAT_B8G8R8A8_UNORM, "BGRA swap chain")] {
        let stop = Arc::new(AtomicBool::new(false));
        let window = show_swatches(hmon, rect, format, stop.clone());
        std::thread::sleep(Duration::from_millis(1500));
        for res in [1080, 0] {
            let (w, h) = output_dims(native.0, native.1, res);
            let got = match record_swatches(&tools, &encoder, gfx, index as u32, native, (w, h), &dir.join(format!("{res}.mp4"))) {
                Ok(got) => got,
                Err(e) => {
                    println!("FAIL {fname}, {w}x{h}: {e}");
                    failures.push(format!("{fname} {w}x{h}: {e}"));
                    continue;
                }
            };
            for (want, got) in SWATCHES.iter().zip(&got) {
                let want = want.map(|c| c as i32);
                let off = (0..3).map(|c| (got[c] - want[c]).abs()).max().unwrap();
                let warmth = (want[0] - want[2], got[0] - got[2]);
                let ok = off <= TOLERANCE && (warmth.0 - warmth.1).abs() <= TOLERANCE;
                println!(
                    "{} {fname}, {w}x{h}: drew {want:?} recorded {got:?}  warmth (R-B) {} -> {}",
                    if ok { "ok  " } else { "FAIL" }, warmth.0, warmth.1
                );
                if !ok {
                    failures.push(format!("{fname} {w}x{h}: drew {want:?}, recorded {got:?}"));
                }
            }
        }
        stop.store(true, Ordering::SeqCst);
        window.join().unwrap().expect("test window");
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "recorded colours differ from the screen:\n{}", failures.join("\n"));
}
