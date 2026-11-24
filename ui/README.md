# Backtrack frontend (`ui/`)

The Tauri v2 frontend. All data and behaviour go through one bridge (`window.BT`), which talks to the Rust commands in `../API.md`, or to a mock backend when there's no Tauri.

Phase 1 wired the library, editor and Discord exporter. Phase 2 wires the recorder: the replay buffer, hotkeys, audio sources and capture settings (see **Phase 2: the recorder** below).

Plain HTML/CSS/JS: no bundler, no npm, no network requests. Tauri serves this folder (`frontendDist: "../ui"`, `withGlobalTauri: true`).

## Files

| File | What it is |
|---|---|
| `index.html` | Markup and all CSS (the prototype's design tokens and styles, plus "Phase 1 wiring" and "phase 2: recorder states" blocks for the new states). It loads the three scripts below, in order. The corner pop-up `toast.html` belongs to the Rust side and isn't here. |
| `mock.js` | `window.BTMock`: the mock backend. Only used when `window.__TAURI__` is missing. |
| `bridge.js` | `window.BT`: one wrapper per command and event in API.md, file URLs, window controls, folder picker, and the global error handlers. |
| `app.js` | The UI: library, editor, export dialog, settings and router. It never touches Tauri or the mock directly. |

## Running it

- **In the app:** `cargo tauri dev` from `src-tauri/`. The bridge sees `window.__TAURI__` and uses the real commands.
- **In a browser (mock):** open `ui/index.html` directly (`file://` works) or serve the repo root. Routes: `#library`, `#editor`, `#export`, `#export-done`, `#settings`, plus Settings deep links `#settings-<page>` for its six pages (`recording`, `hotkeys`, `audio`, `exports`, `folders`, `app`) and `#settings-<section>` for `capture`, `buffer`, `video`, `encoder`, `ffmpeg`, `storage`, `export`, `sharing`, `startup` and `updates` (each opens the page that holds it).
- **Screenshots:** headless Chrome works: `chrome --headless=new --window-size=1440,900 --screenshot=library.png "file:///…/ui/index.html#library"`.

## The bridge: `window.BT`

Every call returns a Promise. Failures reject with the backend's human-readable string (never an `Error`).

| BT | Tauri |
|---|---|
| `getSettings()`, `saveSettings(settings)` | `get_settings`, `save_settings { settings }` |
| `listClips()` | `list_clips` |
| `setFavourite(clipId, fav)` | `set_favourite { clipId, fav }` |
| `setFavourites(clipIds, fav)` | `set_favourites { clipIds, fav }` (bulk, one write) |
| `saveEdits(clipId, {inT,outT,markers})` | `save_edits { clipId, edits }` (the UI debounces it by 400 ms) |
| `getWaveform(clipId, track, points)` | `get_waveform { clipId, track, points }` |
| `getKeyframes(clipId)` | `get_keyframes { clipId }` |
| `prepareAudio(clipId)` | `prepare_audio { clipId }` |
| `exportClip(req)`, `cancelExport()` | `export_clip { req }`, `cancel_export` |
| `saveTrimmedCopy(clipId, start, end)` | `save_trimmed_copy { clipId, start, end }` |
| `copyFile(path)`, `reveal(path)`, `openPath(path)`, `openDiscord()` | `copy_file`, `reveal`, `open_path`, `open_discord` |
| `trashClip(clipId)` | `trash_clip { clipId }` |
| `trashCheck(clipIds)` → `{ fits, drives: [{ drive, bytes, capacity, permanent, clipIds }] }` | `trash_check { clipIds }` (no file changes: can each drive's Recycle Bin keep its part of the batch? `clipIds` = the requested clips on that drive) |
| `trashClips(clipIds, { permanent })` → `{ trashed, failed: [{ clipId, error }] }` | `trash_clips { clipIds, permanent? }` (one clip at a time, one `library-changed` at the end; rejects only if nothing could be attempted; `permanent` only when the user chose "Delete permanently") |
| `restoreClips(clipIds)` → `{ restored, failed }` | `restore_clips { clipIds }` (Undo: clips this run moved to the Recycle Bin go back with the same ids; one `library-changed`) |
| `log(level, msg)` | `log { level, msg }` (never rejects) |
| `recorderStatus()`, `setBuffer(enabled)` | `recorder_status`, `set_buffer { enabled }` |
| `saveReplay(secs = null)` | `save_replay { secs }` → `{ path, game, duration }` |
| `toggleMicMute()`, `addMarker()` | `toggle_mic_mute`, `add_marker` |
| `listAudioDevices()`, `listMonitors()` | `list_audio_devices`, `list_monitors` |
| `on('library-changed' \| 'export-progress' \| 'recorder-status' \| 'clip-saved', fn)` → `unsubscribe()` | `event.listen`, unwraps `e.payload` |
| `src(path)` | `core.convertFileSrc(path)` (`''` for a null path) |
| `pickFolder({title, defaultPath})` → path or `null` | `dialog.open({ directory: true, multiple: false, … })` |
| `window.minimize / toggleMaximize / close / startDragging()` | `window.getCurrentWindow().…` |
| `appVersion()` | `app.getVersion()` |
| `isMock`, `mock` | `true` and the mock instance when running without Tauri |

`bridge.js` also installs `window.onerror` and an `unhandledrejection` handler that forward to `BT.log('error', …)`, capped at 100 per session.

I checked the real branch by running the UI against a fake `window.__TAURI__` that recorded every call. Every command uses exactly the API.md argument names; the full `ExportRequest` shape is `clipId, start, end, preset, limitMb, canvas, cropX, tracks[{index, volume, muted}], fileName, copyToClipboard`. The phase 2 calls were checked the same way: `set_buffer { enabled }`, `save_replay { secs: null }`, and `save_settings` sends all 14 `recorder` fields with the five `hotkeys`.

## The mock backend

`mock.js` builds a sample library and answers every command with the contract's types:

- **Clips:** 27 sample clips with made-up dates plus generated older ones, giving 269 clips in 10 games (Rocket League: 155). The names keep their ™/® (e.g. "HELLDIVERS™ 2"). Some games have 2 tracks or an unnamed "Track 2", to exercise the lane mapping. The newest Rocket League clip carries GG-style trim points (24.25 → 48.15) and two markers.
- **Thumbnails:** none by default, so every card shows the "no thumbnail yet" placeholder. Pass `?thumbs=<folder url>/` to load `<gameId>__<date>__<time>.jpg` images from a folder of your own.
- **Dates:** everything shifts so the newest clip is 3 minutes old. This lets the "New" and "Just saved" states render against the real current date. Add `mock=static` to keep the original dates.
- **Media:** there are no video or audio files, so `src()` returns `''` for clip and audio paths and the editor runs its clock fallback (the still thumbnail gets a subtle camera drift in the mock only). Waveforms come from the prototype's generators, resampled to the requested `points`, with a 0.3–0.6 s first-call delay so the loading shimmer shows.
- **Export:** it mirrors the backend maths (copy versus encode, ladder, never upscales) and emits `export-progress` in the real stage order (`analyse` 0–30 %, `encode` 30–97 %, `finish` 100; copy mode sends `copy` then `finish`). It runs about 3× faster than real x264 so tests finish quickly, rejects with `"cancelled"` on cancel, records an `ExportRecord`, and de-duplicates file names.
- **Folders:** `list_clips` only returns clips under the current `clipDirs`, so removing the folder empties the library just as the real backend would. `trash_clip` and `save_settings` (when the folders change) emit `library-changed`.
- **Flags:** put them in the query string or after `?` in the hash, e.g. `index.html#library?mock=empty`, comma-separated:
  `empty` (only the recorder's empty folder), `missing` (a folder with no clips), `noffmpeg`, `exportfail` (first export fails part-way, to test Retry), `slow` (first scan takes 6 s), `static`.
  `trashfail=N` (its own parameter, e.g. `index.html?trashfail=2#library`): the first N clips sent to `trash_clips` fail with "The file is open in another program", to test the partial-failure dialog and Try again. `trash_clips` takes about 30 ms per clip so the busy state shows.
  `binfull=1` and `nuke=1` (also their own parameters): `trash_check` reports the Recycle Bin on the clips' drive as holding only 2 GB (bigger batches don't fit), or that drive as deleting straight away. `restore_clips` puts back what `trash_clips` / `trash_clip` moved in this session.
  `mixed=1` puts PEAK's clips on a D: drive with no Recycle Bin (a mixed-drive batch: "Move N, skip D:"), and `checkms=N` makes `trash_check` take N ms (e.g. `checkms=1500`, to see OK wait for the check).
  Recorder flags: `recorderoff`, `recorderstarting` (stays in "starting"), `recordererror` (NVENC chosen on an AMD PC), `fresh` (the buffer starts empty), `hotkeyconflict` (Alt+S is taken by another app).
- **Recorder:** see Phase 2 below.

## What's wired

- **Library:** everything is derived from `list_clips`: games (grouped by `gameId`, most clips first), counts, sizes, storage, Favourites, and "Shared to Discord" (`exports.length > 0`). The hero is the newest clip: "Just saved" plus the New badge when it's under 30 minutes old, otherwise "Newest clip". The library re-fetches on `library-changed`. There's a loading state for the slow first scan, an empty state (no folder, or no clips found) with a button to Settings, and an "FFmpeg not found" banner.
  - **View all:** renders the whole game (all 155) in batches of 48 as you scroll; By date covers every clip, grouped by day and then by month.
  - **Clip actions:** favourite, Show in folder, and Move to Recycle Bin (a confirm dialog that names the file, with Cancel focused). They're in the editor and in a new right-click menu on library cards.
  - **Multi-select:** select clips (hover check, Ctrl/Shift+click, right-click → Select, a group's Select all, Space, Ctrl+A) and move them to the Recycle Bin in one go (`trash_check` first: when the Recycle Bin can't keep them the confirm says "Delete permanently"), with Undo (`restore_clips`); or add them to Favourites (`set_favourites`, with Undo).
- **Editor preview:** a muted `<video>` (`BT.src(clip.path)`). The playhead follows `video.currentTime` on each animation frame. Play/pause, seeking, frame step (1/fps), selection loop and speed all act on the video.
  - **Audio:** `prepare_audio` gives one `<audio>` per track, re-synced on play, pause, seek and rate change, and every 250 ms whenever it drifts more than 80 ms. Lane volume and mute set `audio.volume` (times the preview volume).
  - **Fallback:** if the video can't play, it falls back to a clock with the thumbnail and a "Preview unavailable · it still exports" note.
- **Lanes:** built from `clip.audioTracks` (Game → "Game", Chat → "Voice chat", Mic → "Mic", anything else keeps its name). Lane colours are kept. Waveforms come from `get_waveform` with `points` sized to the timeline width (rounded up to 256, re-requested when you zoom in), with a shimmer until they arrive.
- **Edits:** In/Out and markers load from `clip.edits` (else 0..duration) and save through a debounced `save_edits`. The "Edits saved" indicator reflects the real save state. M adds a marker at the playhead; Delete or Backspace removes the one under it (with Undo).
- **Export:** keeps the prototype's `estimate()` and ladder. The time estimate now uses the measured x264 numbers: max(3, dur × 0.45) s for encode, max(1, dur / 40) s for copy. E, the split button and the rail build an `ExportRequest` and call `export_clip`.
  - **Progress and errors:** progress comes from `export-progress`, and Cancel calls `cancel_export`. Errors show an inline error view with the backend message, **Retry** (same request) and **Export options…**.
  - **Done screen:** built from the real `ExportResult`: size against the limit, resolution, fps, bitrate, mix, folder, and whether the file was copied. Copy again → `copy_file`, Show in folder → `reveal`, Open Discord → `open_discord`. The tile is now **click to copy** (no drag wording).
  - **Save trimmed copy:** calls `save_trimmed_copy` and shows a toast with Show in folder.
  - **Quick send:** exports the planned range directly and lands on the done screen.
- **Settings:**
  - **Sharing & library:** export defaults (10/50/500/Custom/Original, custom MB, auto-copy, Quick send length) and clip folders (list with clip counts, Add via the folder picker, Open, remove with Undo). All save via `save_settings`. The FFmpeg status card shows the path or "not found".
  - **Capture:** live since phase 2, see below.
- **Title bar:** `data-tauri-drag-region` sits on the bar and its non-interactive children, never on buttons. A mousedown fallback calls `startDragging()` for children that don't carry the attribute, and double-click toggles maximise. Minimise, maximise and close go through the bridge. The replay-buffer pill shows the recorder's real state (phase 2).

## UX review round 2: all 8 items fixed

1. **Settings scroll-spy:** a clicked nav item (or a deep link) stays highlighted until you scroll more than 48 px yourself, so right-column cards that share a top with left-column cards keep their highlight. Layout shifts from late data don't release it.
2. **Backspace no longer leaves the editor:** Delete and Backspace remove the marker at the playhead. Only Alt+← (and the Library button) navigate.
3. **Quick send uses `planTrim()`:** markers first, then the ending, inside the saved In/Out if the clip was trimmed. The button shows the range it picked (e.g. "0:28–0:48") and never writes the clip's saved edits.
4. **"Too long" never exports blind:** when the estimate is "Too long" (lv 0), E, the main button and the rail open the export dialog with the fix button focused.
5. **No sideways nudges:** the trim nudge only appears when neither resolution nor frame rate gets worse (720p60 → 1080p60, 720p30 → 720p60). Otherwise the card says "Sharpest it can be at this length".
6. **Stuck-drag click bug:** gone. There's no HTML5 drag any more; the tile copies on click, Enter or Space.
7. **Clipboard status line:** "Already on your clipboard" is now a borderless status line, not a card that reads as a fourth button.
8. **Small polish:** `.ec-tip .btn` is 30 px tall. Ladder blocks have 4 px side padding and drop their label (keeping a tooltip) when the measured text plus 8 px doesn't fit.

## Contract gaps and open issues

None of these needed a new command. They're things the UI would use if the contract grew:

1. **No rename command.** The editor title is no longer clickable (the prototype's pencil/"Rename" affordance is gone).
2. **Only In/Out/markers are persisted.** Track volumes and mutes, canvas (16:9 / 9:16) and the crop position aren't in `Clip.edits`, so the UI keeps them per clip for the session only. The "Edits saved" tooltip now says "In / Out and markers". Suggested: `edits.tracks[]`, `edits.canvas`, `edits.cropX`.
3. **No video codec on `Clip`.** WebView2 can't play some codecs (e.g. HEVC without the extension, possibly AV1). The UI only finds out when `<video>` errors, then shows "Preview unavailable · it still exports". A `videoCodec` field would let it warn earlier, and Clip details could show the codec again (the prototype's "H.264" was removed because it was invented).
4. **No disk free space.** The storage card shows the clip folder instead of the prototype's "1.31 TB free".
5. **No per-folder status.** The UI can't tell "folder missing" from "folder has no clips", so the empty state and the folder rows cover both in one sentence. Suggested: `Settings.clipDirStatus[{path, exists, clips}]`.
6. **FFmpeg can't be pointed at or re-detected** without a restart (`ffmpegPath` is read-only). The banner gives the backend's winget hint.
7. **"Shared to Discord" counts trimmed copies.** As specified, it's `exports.length > 0`, but `save_trimmed_copy` also adds an `ExportRecord` (`preset: "lossless"`), so a clip with only a trimmed copy counts as shared. Filtering out `lossless` is a one-line change if wanted. Share activity labels these "Trimmed copy · lossless".
8. **The 20 Mbps video cap** in the prototype's `estimate()` (it only matters for 9:16 at 500 MB) isn't in API.md's maths. The backend should mirror it, or both should drop it.
9. **No native drag-out yet:** the done tile is click-to-copy. A native OS file drag (a Tauri drag plugin) can come later.
10. **Preview audio above 100 % is capped at 100 %** because `HTMLMediaElement.volume` maxes at 1; the export still gets the real 0–150 % gain. Web Audio gain would need CORS headers on the asset protocol.
11. **Full-screen preview** uses element fullscreen inside the webview; it fills the window, not the monitor. A real fullscreen would need `core:window:allow-set-fullscreen`.
12. **Marker origin isn't in the contract** (in-game Alt+F7 or added in Backtrack), so marker tooltips just say "Marker · 0:31.30".
13. **`export_clip` and `save_trimmed_copy` don't emit `library-changed`** after adding an `ExportRecord`, so the UI prepends the record locally. The next `list_clips` brings the authoritative list.
14. **The mock can't check the asset-protocol scope.** Clips in a folder outside `$VIDEO`/`$HOME`/… need `allow_dirs` on the backend (it already does this for `clipDirs` and `exportDir`).

## Phase 2: the recorder

Contract: the "Phase 2: the recorder" section of `../API.md`. Everything below goes through the same bridge.

### What's wired

- **Live state:** `recorder_status` at start-up, then every `recorder-status` event, drives the title-bar pill, the library card and the live parts of Settings. Ticks arrive about twice a second, so only the changing bits are touched (meters, gauge, wave, status line). A switch to `error` raises a toast with the message.
- **Title-bar pill** (the approved design, now real):
  - **On:** red dot, "Replay buffer on · last 60 s" and the save hotkey as key chips (amber when Windows refused that hotkey; the tooltip gives the reason). Clicking opens a menu with the live status, **Save now**, **Mark a moment** (`add_marker`), **Mute / Unmute mic** (`toggle_mic_mute`), **Turn off** and **Recorder settings**.
  - **Starting:** a neutral spinner, "Starting replay buffer…".
  - **Off:** grey "Replay buffer off"; clicking turns it on (`set_buffer`).
  - **Error:** amber "Recorder error"; the menu shows the message, **Try again**, **Turn off** and **Settings**.
- **Library card** (the approved design): the switch calls `set_buffer`. The mini wave scrolls with max(Game, Mic) from `status.tracks[].level`, one bar per tick, with the last three bars red for "now". A thin gauge shows `filledSecs / bufferSecs`; the footer says "Saves the last 60 s" and "45 s ready" or "Full". **Save now** (with the hotkey chips) calls `save_replay { secs: null }`. The subtitle reads "Recording · 1080p60" (plus "mic muted"), and the tooltip has the size, fps and encoder.
- **clip-saved:** shows "Clip saved · Rocket League · 60 s" with **Open**. If `library-changed` hasn't indexed the file yet, Open waits for it (matched on `path`, case-insensitive, for up to 30 s) and then opens the clip in the editor. The library's hero then shows it as "Just saved". Markers from the Mark hotkey arrive through `clip.edits.markers` like GG's, so the editor shows them and trims snap to them.
- **Settings → Capture** (restored to the approved two-column layout; the "coming soon" note and SOON chip are gone). Every control saves `settings.recorder` via `save_settings`, except the on/off switch, which uses `set_buffer`. A toast says when the buffer restarted.
  - **Header:** a live status card shows the state, game, encoder and RAM, or the error.
  - **Replay buffer:** on/off, length 15 / 30 / 60 s / 2 / 5 min with "≈ bitrateMbps × bufferSecs / 8 MB of RAM", and "Confirm when a clip is saved".
  - **Video quality:** 720p / 1080p / 1440p / Native (0), 30 / 60 / 120 fps, and 15 / 25 / 40 Mbps with an MB-per-minute hint. The resolution hint names the chosen monitor's size (or says it records at native size when the choice is above it). **Capture monitor** lists `list_monitors`.
  - **Encoder:** Automatic / AMD AMF / NVIDIA NVENC / CPU (x264). "Recording with" shows `status.encoder`, or the error, and Automatic's subtitle names the encoder and GPU, e.g. "Uses AMD AMF · Radeon RX 7900 XTX". No fake detection text.
  - **Hotkeys:** click a key, then press a combo: modifiers Ctrl / Alt / Shift / Win plus one key. While you hold modifiers it previews "Ctrl + …". Letters, digits and Space need Ctrl or Alt, a combo already used by another action is refused with a toast, Backspace (or the × that appears on hover) clears a binding, Esc or clicking away cancels, and leaving Settings cancels too. Per-hotkey errors from `status.hotkeys` show under the row in amber, with an amber key box. "Mark a moment" is no longer Beta.
  - **Audio sources:** live 10-step meters per track from `status.tracks[].level`. Game is a fixed "All apps except Discord" (or "All apps"). Voice chat is a menu of `chatApps` plus None (with "Discord isn't running" when inactive). Microphone is a menu of Windows default plus `inputs`, with an on/off switch; "Muted · Alt+F8" appears when the mic is muted, and clicking it unmutes.
  - **Storage & startup:** "Save clips to" (`recorder.clipsDir`: Open, Change…), "Save exports to" (the existing export folder: Open, Change…), and Start with Windows.
- **Clip folders:** the recorder's folder is always listed (the backend re-adds it to `clipDirs`), marked "Your Backtrack recordings", and can't be removed there.
- **Library empty state:** now explains the hotkey ("Press Alt S in a game to save the last 60 s") and offers adding the SteelSeries Moments folder.

### The mock recorder

- **State:** on by default, as if it had been running for 41 s, so the buffer is partly full and keeps filling. `recorder-status` is emitted every 500 ms while on, with moving levels (game bursts, talk spurts on voice chat, a quieter mic) and RAM = bitrate × filled / 8.
- **Devices:** Discord is running. There are three inputs (HyperX QuadCast S is the default) and two monitors: DELL 2560×1440 (main) and LG 1440×2560 portrait. The recording size follows the resolution and monitor (e.g. 1080 on the portrait monitor gives 1080×1920).
- **Settings changes:** pipeline settings restart the buffer ("starting" for 0.8 s, then on, from empty), like the backend's `pipeline_key`. Hotkey changes re-emit the status. `encoder: "nvenc"` puts the recorder in `error` (this mock PC is AMD), and x264 reports "CPU · x264".
- **save_replay:** emits `clip-saved` at +0.25 s and `library-changed` at +1.2 s (indexed, no thumbnail yet). At +2.6 s it emits `library-changed` again (thumbnail ready). Moments marked inside the saved window become `edits.markers`. It rejects when the buffer is off or empty.
- **Hotkeys:** with `hotkeyconflict`, Alt+S reports "already used by another app, maybe SteelSeries GG". Duplicates inside Backtrack report an error on the second action.

### Recorder contract gaps and open issues

1. **Hotkey key names:** API.md says "arrows" without naming them. The UI produces `Up`, `Down`, `Left`, `Right` and `Numpad0`–`Numpad9` (from `KeyboardEvent.code`), so please parse those names.
2. **Rebinding while hotkeys are registered:** Backtrack's own global hotkeys (RegisterHotKey) swallow their combos before the webview sees them. Re-pressing the current combo in the capture box therefore does nothing, and pressing e.g. the save combo while rebinding saves a clip. A `suspend_hotkeys { suspended }` command for the time the capture box is open would fix it.
3. **No list of available encoders/GPUs.** The UI can't grey out NVENC on an AMD PC (as the prototype did); the user only finds out through the `error` state. Suggested: `RecorderStatus.encoders: { id, available, name }[]`.
4. **No levels while the buffer is off,** so the mic can't be tested without recording. A "monitor levels" mode or levels in the off state would help.
5. **`clip-saved` has no clip id,** so Open matches on `path` after `library-changed` (as specified). If the backend ever normalises paths differently (case, slashes) between the event and `Clip.path`, Open would wait and give up after 30 s; matching is case-insensitive to soften that.
6. **No event when devices or monitors change.** The UI re-asks `list_audio_devices` / `list_monitors` when Settings opens and when a menu opens.
7. **`set_buffer` and `save_settings({ recorder.enabled })` overlap.** The UI uses `set_buffer` for every on/off toggle and keeps `recorder.enabled` in its copy of the settings in step, so a later `save_settings` doesn't flip it back.
8. **In-app toast and the native corner pop-up both show** when a clip is saved while Backtrack is in front. The UI has no signal for whether the native pop-up was shown; the backend could skip its pop-up while the main window is focused.
9. **Headless screenshots and live animation:** while `recorder-status` ticks, headless Chrome stops advancing CSS transitions and keyframe animations, so the live wave, gauge and meters don't use transitions. The screenshot harness turns off the popover's pop-in animation; the real app keeps it.

## Keyboard

Editor: Space play/pause · I / O set In/Out · ← / → frame (Shift: 1 s) · Home / End jump to In/Out · L loop · M mark · Del/Backspace remove marker · Ctrl+Z / Ctrl+Y · + / − zoom · F full screen · E export · Shift+E options · Alt+← library · ? shortcuts.
Library: Ctrl+K or / to search, Enter opens the focused card (also while selecting; a double-click too), arrows move between cards, Ctrl+Z undoes a move to the Recycle Bin or a bulk favourite while its toast shows. Selecting: Space toggles the focused card, Shift+Space or Shift+arrows select a range, Ctrl+A selects everything in the view, Delete moves the selection (or the focused card) to the Recycle Bin, Esc clears the selection (in Review, Esc goes back first). Export dialog: Enter export, Esc close or cancel, Ctrl+Z/Y, Ctrl+C copies again on the done screen.
