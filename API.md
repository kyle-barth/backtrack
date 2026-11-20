# Backtrack: frontend ↔ Rust API contract (phase 1)

Phase 1 is the **library, editor and Discord exporter** working on real clips. The clips come from the user's SteelSeries Moments folder now, and from Backtrack's own recorder later. The recorder (replay buffer, capture, hotkeys) is **phase 2**. In phase 1 its UI is shown but disabled.

The frontend is plain HTML/JS in `ui/`, loaded by Tauri v2 with `withGlobalTauri: true`:

```js
const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();   // win.minimize(), win.toggleMaximize(), win.close()
const { open } = window.__TAURI__.dialog;                  // open({ directory: true }) → string | null
```

All JSON field names are **camelCase**. Command arguments are passed as `invoke('cmd', { argName: value })`. Failures reject the promise with a human-readable string.

Files on disk (thumbnails, clips, audio tracks) are shown with `convertFileSrc(absPath)`, which uses Tauri's asset protocol and supports range requests for `<video>`/`<audio>`. The title bar uses `data-tauri-drag-region`.

## Types

```ts
type Clip = {
  id: string;              // stable id (hash of the path)
  path: string;            // absolute path to the .mp4
  fileName: string;        // "Rocket-League__2026-11-20__00-01-27.mp4"
  game: string;            // display name, e.g. "Rocket League", "Call of Duty®", "Desktop capture"
  gameId: string;          // slug for grouping, e.g. "rocket-league"
  recordedAt: number;      // ms since epoch
  duration: number;        // seconds (float)
  sizeBytes: number;
  width: number; height: number; fps: number;
  videoKbps: number;
  audioTracks: { index: number; name: string; channels: number }[];
      // index = audio-stream index (0-based). name comes from the stream's handler ("Game", "Chat", "Mic"), or "Track N".
      // UI label mapping: Game → "Game", Chat → "Voice chat", Mic → "Mic".
  thumb: string | null;    // absolute path to a JPG (GG's thumbnail, or one Backtrack generated); null while not ready yet
  fav: boolean;
  edits: { inT: number; outT: number; markers: number[] } | null;
      // Backtrack's saved In/Out (or GG's own trim points when the clip was trimmed in GG); null = never trimmed
  exports: ExportRecord[]; // newest first
  source: "gg" | "backtrack";
};

type ExportRecord = {
  path: string; sizeBytes: number; preset: string;  // "d10" | "d50" | "d500" | "orig" | "custom" | "lossless"
  width: number; height: number; fps: number; at: number /* ms epoch */;
};

type Settings = {
  clipDirs: string[];          // folders scanned for clips (default: %USERPROFILE%\Videos\SteelSeries Moments if it exists)
  exportDir: string;           // default: %USERPROFILE%\Videos\Backtrack\Exports
  defaultPreset: "d10" | "d50" | "d500" | "orig" | "custom";
  customMb: number;            // default 25
  autoCopy: boolean;           // copy the exported file to the clipboard when done (default true)
  quickLen: number;            // seconds for "Quick send" (default 20)
  ffmpegPath: string | null;   // read-only: resolved ffmpeg.exe, or null if not found
};

type ExportRequest = {
  clipId: string;
  start: number; end: number;            // seconds in the source clip
  preset: "d10" | "d50" | "d500" | "orig" | "custom";
  limitMb: number | null;                // the size limit (10/50/500/customMb); null for "orig"
  canvas: "16:9" | "9:16";
  cropX: number;                         // 0..1, horizontal crop centre for 9:16
  tracks: { index: number; volume: number /* 0..1.5 */; muted: boolean }[];
  fileName: string;                      // without ".mp4"; the backend sanitises it and de-duplicates it
  copyToClipboard: boolean;
};

type ExportResult = {
  path: string; fileName: string; sizeBytes: number;
  width: number; height: number; fps: number;
  videoKbps: number; audioKbps: number;
  durationSec: number; elapsedSec: number;
  mode: "encode" | "copy";               // copy = video stream-copied (only when it already fits)
  copied: boolean;                       // the file is on the clipboard
};

type ExportProgress = { stage: "analyse" | "encode" | "copy" | "finish"; percent: number /* 0..100, overall */; etaSec: number | null };
```

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `get_settings` | none | `Settings` | |
| `save_settings` | `{ settings: Settings }` | `Settings` | Changing `clipDirs` triggers a rescan and a `library-changed` event. |
| `list_clips` | none | `Clip[]` | Newest first. The first call scans and probes (cached on disk afterwards, so it's fast). |
| `set_favourite` | `{ clipId, fav }` | none | |
| `save_edits` | `{ clipId, edits: { inT, outT, markers } }` | none | Debounce calls (≈400 ms). |
| `get_waveform` | `{ clipId, track, points }` | `number[]` | `track` = audio index, `points` = how many peaks (0..1) to return, spread over the whole clip. Cached. ≈0.3–1 s the first time. |
| `get_keyframes` | `{ clipId }` | `number[]` | Keyframe times in seconds, sorted. Read from the packet index with ffprobe (no decoding, ≈0.2 s for 200 MB; cached). The editor uses them to scrub fast: while a drag moves quickly it seeks to the nearest keyframe (≈10 ms to decode) and lands on the exact frame when the drag slows or stops. |
| `prepare_audio` | `{ clipId }` | `{ index, name, path }[]` | Extracts each audio track to its own .m4a (stream copy, cached) so the editor can play every track at its own volume with `<audio>` elements kept in sync with a **muted** `<video>`. |
| `export_clip` | `{ req: ExportRequest }` | `ExportResult` | Emits `export-progress` events while running. Rejects with `"cancelled"` if cancelled. One export at a time. |
| `cancel_export` | none | none | |
| `save_trimmed_copy` | `{ clipId, start, end }` | `{ path, sizeBytes }` | Lossless stream copy of every track. Cuts snap to the nearest earlier keyframe. |
| `copy_file` | `{ path }` | none | Puts the file on the Windows clipboard (CF_HDROP), so Ctrl+V in Discord attaches it. |
| `reveal` | `{ path }` | none | Opens Explorer with the file selected. |
| `open_path` | `{ path }` | none | Opens a folder or file with the default app. |
| `open_discord` | none | none | Opens the Discord app (falls back to discord.com in the browser). |
| `trash_clip` | `{ clipId }` | none | Sends the clip to the Recycle Bin (the UI must confirm first). Emits `library-changed`. |
| `trash_clips` | `{ clipIds: string[], permanent?: bool }` | `{ trashed: string[], failed: [{ clipId, error }] }` | Sends several clips to the Recycle Bin in one go (the UI must confirm first, after `trash_check`). With `permanent: true` it deletes the files outright instead; only when the user chose that. Per-clip failures are listed in `failed` with a short reason ("It's open in another program"), not thrown; it rejects only if `clipIds` is empty. Emits `library-changed` once. |
| `trash_check` | `{ clipIds: string[] }` | `{ fits, drives: [{ drive, bytes, capacity, permanent, clipIds }] }` | Before a delete: per drive, which of the clips are on it (`clipIds`), their total `bytes`, that drive's Recycle Bin limit `capacity` in bytes (`null` if Windows doesn't say) and `permanent` if the drive has no Recycle Bin or is set to delete immediately. `fits` is false if any drive is `permanent` or `bytes > capacity`; Windows would then permanently delete some of the batch to make room. Changes nothing. |
| `restore_clips` | `{ clipIds: string[] }` | `{ restored: string[], failed: [{ clipId, error }] }` | Undo: puts clips Backtrack moved to the Recycle Bin during this run back where they were, with their favourite, edits and export history. Fails per clip if it's no longer in the Recycle Bin or a file with that name is back in its folder. Emits `library-changed` once. |
| `set_favourites` | `{ clipIds: string[], fav }` | none | Bulk `set_favourite`, one write. |
| `log` | `{ level, msg }` | none | Writes frontend logs and errors to the app log (`%APPDATA%\Backtrack\backtrack.log`). |

## Events

| Event | Payload | When |
|---|---|---|
| `library-changed` | none | A clip folder changed (a new clip was saved, a clip was deleted, settings changed) or thumbnails finished generating. Call `list_clips` again. |
| `export-progress` | `ExportProgress` | During `export_clip`. |

## Export maths (the backend mirrors the UI's `estimate()`)

- Target = limit × 0.95. `total kbps = target_MB × 8000 / dur`. Audio 128 kbps, or 96 kbps when video would drop below 1500 kbps.
- Ladder (video kbps → tier): ≥6000 → 1080p60, ≥3500 → 1080p30, ≥2000 → 720p60, ≥1500 → 720p30, ≥1000 → 480p30 (Low), otherwise 480p30 (Poor).
- The tier never upscales past the source resolution or fps.
- `copy` mode: when the selection already fits under the limit at source bitrate and the canvas isn't 9:16 (or the preset is `orig`), video is stream-copied and audio is mixed and re-encoded as a single AAC 192 kbps track.
- `encode` mode: two-pass libx264, then all non-muted tracks are mixed (with volumes) into a single AAC track. Discord only plays the first audio track, so mixing is required.
- After encoding, if the file is over the limit, the backend re-encodes once at a proportionally lower bitrate.

---

# Phase 2: the recorder

Backtrack now records by itself. A replay buffer, always on, keeps the last N seconds in **RAM**; a hotkey saves them as a clip. Everything stays on the PC.

- **Video:** the GPU's own capture and encoder (AMD AMF, NVIDIA NVENC, or x264 on the CPU as a fallback), 1080p60 by default.
- **Audio:** three tracks, the same layout as GG's:
  - **Game:** everything the PC plays *except* the chat app.
  - **Voice chat:** only the chat app (Discord) and its child processes.
  - **Mic:** the default (or chosen) input device.

Saved clips go to `recorder.clipsDir`, which is added to `clipDirs` automatically, so they show up in the library like any other clip. They're named `<Game>__YYYY-MM-DD__HH-MM-SS.mp4`. The game is the app in the foreground when you press the hotkey.

## Settings additions

`Settings` gains a `recorder` object (all camelCase):

```ts
recorder: {
  enabled: boolean;          // replay buffer on/off (persisted; the buffer starts with the app when true)
  bufferSecs: number;        // 15 | 30 | 60 | 120 | 300
  resolution: number;        // short side of the recording: 720 | 1080 | 1440; 0 = the monitor's native size
  fps: number;               // 30 | 60 | 120
  bitrateMbps: number;       // 15 | 25 | 40
  encoder: "auto" | "amf" | "nvenc" | "x264";
  monitor: number;           // capture monitor index, 0 = main (see list_monitors)
  clipsDir: string;          // default %USERPROFILE%\Videos\Backtrack\Clips
  hotkeys: { save: string; toggle: string; muteMic: string; mark: string; open: string };
      // "Alt+F10" style: modifiers Ctrl / Alt / Shift / Win joined with "+", then one key
      // (A–Z, 0–9, F1–F24, Space, Home, End, PageUp, PageDown, Insert, Delete, arrows, Numpad0–9).
      // "" = unbound. Defaults: save Alt+S (same as SteelSeries GG), toggle Alt+F9, muteMic Alt+F8, mark Alt+F7, open Ctrl+Alt+B.
  confirm: boolean;          // pop-up + sound in the corner when a clip is saved
  chatApp: string;           // "Discord" (default) or "" for none (then Game = all audio, Voice chat = silent)
  micDevice: string;         // "" = Windows default input, else an id from list_audio_devices
  micEnabled: boolean;
  startWithWindows: boolean; // starts minimised to the tray with the buffer running
}
```

Changing any recorder setting via `save_settings` restarts the buffer if needed (it's emptied) and re-registers the hotkeys.

## Types

```ts
type RecorderStatus = {
  state: "off" | "starting" | "on" | "error";
  error: string | null;            // human-readable when state = "error"
  bufferSecs: number;              // configured length
  filledSecs: number;              // how much is buffered right now (0 → bufferSecs)
  memoryMb: number;                // RAM the buffer uses right now
  encoder: string;                 // e.g. "AMD AMF · Radeon RX 7900 XTX"
  width: number; height: number; fps: number;
  game: string | null;             // what a clip saved right now would be named after
  micMuted: boolean;
  tracks: { name: "Game" | "Voice chat" | "Mic"; source: string; level: number /* 0..1 peak, last ~250 ms */; active: boolean }[];
      // source e.g. "All apps except Discord", "Discord", "Microphone (HyperX QuadCast S)"; active = the source exists right now
  hotkeys: { action: "save" | "toggle" | "muteMic" | "mark" | "open"; keys: string; ok: boolean; error: string | null }[];
      // ok = false when Windows refused the shortcut (usually another app, e.g. GG, already uses it)
  lastSaved: { path: string; game: string; duration: number; at: number } | null;
};

type AudioDevices = {
  inputs: { id: string; name: string; isDefault: boolean }[];
  chatApps: { name: string; running: boolean }[];   // e.g. [{ name: "Discord", running: true }]
};

type Monitor = { index: number; name: string; width: number; height: number; primary: boolean };
```

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `recorder_status` | none | `RecorderStatus` | |
| `set_buffer` | `{ enabled }` | `RecorderStatus` | Turns the replay buffer on or off and persists it (same as the Alt+F9 hotkey). |
| `save_replay` | `{ secs: number \| null }` | `{ path, game, duration }` | Saves the last `secs` seconds (null = bufferSecs), the same as the save hotkey. Rejects if the buffer is off or empty. |
| `toggle_mic_mute` | none | `RecorderStatus` | Mic track goes silent in recordings (doesn't affect Discord). |
| `add_marker` | none | none | Marks "now"; the next saved clip that contains this moment gets a marker there (shows in `clip.edits.markers`). |
| `list_audio_devices` | none | `AudioDevices` | |
| `list_monitors` | none | `Monitor[]` | |

## Events

| Event | Payload | When |
|---|---|---|
| `recorder-status` | `RecorderStatus` | About every 500 ms while the buffer is on (for live meters and the fill gauge), and on every state change. |
| `clip-saved` | `{ path, game, duration }` | A replay was saved (hotkey, tray or `save_replay`). `library-changed` follows once the file is indexed. |

## Window behaviour

- Closing the window while the buffer is on hides Backtrack to the tray; it keeps recording. The tray menu has Save clip, Replay buffer on/off, Open Backtrack and Quit.
- The "clip saved" pop-up in the corner of the game is a separate small native window (`ui/toast.html`), owned by the Rust side; the main UI doesn't render it.
