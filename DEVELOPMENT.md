# Developing Backtrack

Backtrack is a [Tauri v2](https://tauri.app) app: a Rust backend and a plain HTML/CSS/JS frontend. There's no bundler and no npm. It's built with plain `cargo`, not the Tauri CLI.

## How it works

- **Library, editor and exporter:**
  - It scans clip folders, probes them with ffprobe and caches the results.
  - It reads the metadata SteelSeries GG writes: game names, trim points, thumbnails and the separate Game / Chat / Mic audio tracks.
  - Exports are two-pass x264, sized to fit a file-size limit.
- **Replay buffer:**
  - FFmpeg captures the screen and encodes it on the GPU: AMD AMF, or NVIDIA NVENC through Desktop Duplication, or x264 on the CPU as a fallback.
  - The stream is kept in RAM as MPEG-TS chunks that start on a keyframe.
  - Saving a clip copies the last N seconds into an MP4 without re-encoding.
- **Audio:**
  - WASAPI per-process loopback gives three tracks: Game (every app except the chat app), Voice chat (Discord) and Mic.
  - The tracks reach FFmpeg through named pipes, paced on a wall clock.
- **Everything stays local.** Nothing is uploaded, and the only network request is opening Discord when you ask it to.

## Layout

| Path | What |
|---|---|
| `ui/` | The frontend. It talks to Rust only through `ui/bridge.js`. Opened in a plain browser, it runs on a mock backend (`ui/mock.js`); see [ui/README.md](ui/README.md). |
| `API.md` | The contract between the UI and Rust: commands, events and types |
| `src-tauri/src/main.rs` | Tauri setup, commands, folder watcher, thumbnail worker |
| `src-tauri/src/library.rs` | Folder scan, cached ffprobe, GG metadata, favourites / edits / export history |
| `src-tauri/src/export.rs` | Discord size maths, two-pass x264, audio mixing, progress, cancel, lossless trim |
| `src-tauri/src/media.rs` | Waveforms, per-track preview audio, keyframe index, generated thumbnails |
| `src-tauri/src/sys.rs` | Copying a file to the clipboard, Explorer, Discord, Recycle Bin |
| `src-tauri/src/recorder.rs` | Replay buffer: the FFmpeg capture/encode pipeline, the in-RAM ring of keyframe chunks, saving clips |
| `src-tauri/src/audio.rs` | WASAPI capture for Game / Voice chat / Mic, streamed to FFmpeg over named pipes |
| `src-tauri/src/rec_app.rs` | Recorder commands, global hotkeys, tray, the corner pop-up, the status pump |
| `src-tauri/src/winutil.rs` | Foreground-game detection, local time, the chime, start with Windows |
| `ui/toast.html` | The corner pop-up shown in-game when a clip is saved |
| `tools/cdp.ps1` | Drives a running build through WebView2's DevTools protocol, for testing |
| `tools/stage-ffmpeg.ps1` | Puts the pinned, checksum-verified FFmpeg build that the installer ships into `src-tauri/ffmpeg/` |
| `src-tauri/tauri.installer.conf.json` | Installer-only Tauri settings (NSIS, per-user, bundled FFmpeg), merged in by `--config` |
| `src-tauri/installer-hooks.nsh` | Installer hooks: stop the app and its FFmpeg before files are replaced; clean up on uninstall |

## Requirements

- Windows 11, 64-bit.
- Rust (stable, MSVC toolchain) and the Visual Studio Build Tools.
- FFmpeg, for running from source: `winget install Gyan.FFmpeg.Essentials`, or `ffmpeg.exe` and `ffprobe.exe` in an `ffmpeg` folder next to `backtrack.exe`. Backtrack looks there first, then on PATH and in the winget install folder. The installer ships its own copy.
- WebView2, which comes with Windows 11.

## Build and run

```powershell
cd src-tauri
cargo run              # debug build, with a console that shows logs
cargo build --release  # optimised exe → src-tauri/target/release/backtrack.exe
```

If linking fails with `LNK1104: cannot open file`, the checkout folder is too deep for Windows' 260-character path limit. Build into a shorter folder instead:

```powershell
$env:CARGO_TARGET_DIR = 'C:\bt-target'
```

## Building the installer

The installer is an NSIS setup made by the Tauri CLI. It installs for the current user, without admin rights, into `%LOCALAPPDATA%\Backtrack`, and puts FFmpeg in an `ffmpeg` folder next to `backtrack.exe`. The app looks there first.

```powershell
cargo install tauri-cli --version 2.12.6 --locked   # once
.\tools\stage-ffmpeg.ps1                              # or: -From <an extracted FFmpeg 9.0.2 essentials build>
cd src-tauri
cargo tauri build --config tauri.installer.conf.json
# → target\release\bundle\nsis\Backtrack_<version>_x64-setup.exe
```

The plain `cargo build` doesn't need FFmpeg staged: the bundle settings only apply when they're merged in with `--config`.

## Tests

```powershell
cargo test                             # unit tests: export ladder, audio mix graph, date and keyframe parsing, performance guards
cargo test -- --ignored --nocapture    # end to end on the clips in your own library: scan, waveform, two 10 MB exports
cargo test colour -- --ignored --nocapture   # recorded colours match the screen (covers a monitor for ~10 s)
cargo test frame_rate -- --ignored --nocapture --test-threads=1   # recordings don't drop frames (covers a monitor for ~2 min)
cargo test probe -- --ignored          # the graphics-driver-reset probe (gpu.rs) works on this GPU
```

Some unit tests guard against costs that once came back unnoticed. They need no GPU, screen or running app:
- `ui_test.rs` reads the pages' CSS. No animation repeats forever, except on the short-lived states listed with a reason in `FOREVER_OK` (loading spinners and the like), because while any CSS animation runs, WebView2 draws the page at the monitor's refresh rate. The recording dot is static: any pulse, even a stepped one or a 10 Hz timer, kept the page drawing. The corner pop-up animates for at most 0.5 s at a time.
- Helper ffmpeg and ffprobe processes start below normal priority, and only the recorder starts at normal priority (`ffmpeg.rs`; the test starts cmd.exe with each builder's flags). Thumbnails decode, scale and encode on one thread each (`media.rs`).
- Windows Graphics Capture asks for frames at 4× the clip's rate, and `fps` drops the extra frames before `hwdownload` (`recorder.rs`).
- The tray is updated only when its tooltip or check mark changes (`rec_app.rs`). index.json is rewritten only when the index changed, and a new thumbnail updates just its own clip (`library.rs`). Two-pass logs are deleted when an export ends, and stale ones at startup (`export.rs`).

The colour and frame-rate tests draw on a monitor, so close Backtrack first (a second capture of the same monitor skews the frame-rate results) and run them one at a time. `BACKTRACK_TEST_MONITOR` picks the monitor (default 0).

The colour test fills a monitor with colour swatches through a game-like swap chain (RGBA, then BGRA), records it with the recorder's own pipeline, saves it the way a clip is saved, and checks each swatch's colour and warmth (red minus blue). Set `BACKTRACK_TEST_CAPTURE=vsrc_amf` to try AMD's old capture, which fails it.

The frame-rate tests cover a monitor with a frame counter that changes on every frame the window presents: with vsync at fractions of the refresh rate, without vsync at 144, 100 and 75 fps, and (`frame_rate_under_gpu_load`) as fast as it can while keeping the GPU busy like a demanding game (`BACKTRACK_TEST_LOAD=<copies a frame>`, default 12). Each recording is decoded and its repeated frames (visible hitches) are compared with the ones it couldn't avoid: 60 fps slots in which the window presented nothing new. v0.4.0's pipeline is recorded alongside for comparison: it let Windows hand over frames only at the clip's rate, held back any that came a little early, and repeated up to a third of frames from games with vsync off.

After a graphics driver crash the recorder gets itself going again: `gpu.rs` notices the reset (a small Direct3D device of Backtrack's own reports itself lost) and stops the recorder's ffmpeg, which may otherwise hang, still "recording". `recorder.rs` then restarts it with longer and longer waits (about four minutes in all) before it gives up and the app offers Restart Backtrack. To try it, with no game open, run `dxcap -forcetdr` in an administrator PowerShell (DXCap comes with Windows' optional Graphics Tools). The screen goes black for a moment, and the corner pop-up says "Graphics driver reset", then "Replay buffer back on". The page's restarting and failed states can be seen without a GPU: `index.html?mock=driverreset` and `?mock=driverfail`.

To drive a running app from a script, start it with `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'`. Then use `tools/cdp.ps1 -Eval "<js>"` or `-Shot out.png`.

## Releasing

There are two ways to make a release. Either way, the **Release** workflow tests and builds on GitHub, then publishes these on the Releases page:
- `backtrack-vX.Y.Z-windows-x64-setup.exe`, the installer, with FFmpeg included;
- `backtrack-vX.Y.Z-windows-x64.zip`, a portable copy without FFmpeg;
- `SHA256SUMS.txt`, the checksums for both.

- **Push a tag:**
  ```powershell
  git tag v0.2.0
  git push origin v0.2.0
  ```
  The workflow sets the version from the tag before it builds.
- **From GitHub:** go to **Actions → Release → Run workflow** and enter a version such as `0.2.0`. The workflow updates the version in `Cargo.toml` and `tauri.conf.json`, commits, tags and publishes.

A version with a suffix, such as `0.2.0-beta.1`, is published as a pre-release.

## Where data lives

- Settings, favourites, edits, export history and the log: `%APPDATA%\Backtrack\`
- Cache (probe index, generated thumbnails, preview audio): `%LOCALAPPDATA%\Backtrack\`
- Recordings: `Videos\Backtrack\Clips`
- Exports: `Videos\Backtrack\Exports` (changeable in Settings)
