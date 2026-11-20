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

## Requirements

- Windows 11, 64-bit.
- Rust (stable, MSVC toolchain) and the Visual Studio Build Tools.
- FFmpeg, either `winget install Gyan.FFmpeg.Essentials` or `ffmpeg.exe` and `ffprobe.exe` next to `backtrack.exe`. Backtrack also looks on PATH and in the winget install folder.
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

## Tests

```powershell
cargo test                             # unit tests: export ladder, audio mix graph, date and keyframe parsing
cargo test -- --ignored --nocapture    # end to end on the clips in your own library: scan, waveform, two 10 MB exports
```

To drive a running app from a script, start it with `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'`. Then use `tools/cdp.ps1 -Eval "<js>"` or `-Shot out.png`.

## Releasing

There are two ways to make a release. Either way, the **Release** workflow tests and builds on GitHub, then publishes `backtrack-vX.Y.Z-windows-x64.zip` and a SHA-256 checksum on the Releases page.

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
