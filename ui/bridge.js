/* =====================================================================
   window.BT: the only way the UI talks to the Rust backend (see ../API.md).
   In Tauri (window.__TAURI__ present) every call goes to the real commands.
   Anywhere else (plain browser, headless Chrome) it falls back to the mock
   backend in mock.js, which speaks the same contract.

   Every command returns a Promise. Failures reject with a human-readable
   string (the backend's message), never an Error object.
   ===================================================================== */
(function(){
'use strict';

const T = window.__TAURI__;
const tauri = !!(T && T.core && typeof T.core.invoke === 'function');

let backend;
if (tauri){
  const { invoke, convertFileSrc } = T.core;
  const win = () => T.window.getCurrentWindow();
  backend = {
    invoke,
    src: p => p ? convertFileSrc(p) : '',
    listen: (ev, fn) => T.event.listen(ev, e => fn(e.payload)),
    window: {
      minimize: () => win().minimize(),
      toggleMaximize: () => win().toggleMaximize(),
      close: () => win().close(),
      startDragging: () => win().startDragging(),
    },
    pickFolder: opts => T.dialog.open(Object.assign({ directory: true, multiple: false }, opts || {})),
    appVersion: () => (T.app && T.app.getVersion) ? T.app.getVersion() : Promise.resolve(''),
  };
} else if (window.BTMock){
  backend = window.BTMock.create();
} else {
  throw new Error('Backtrack: no Tauri runtime and no mock backend (mock.js) loaded');
}

const msgOf = e => typeof e === 'string' ? e : (e && e.message) ? e.message : (e == null ? 'Unknown error' : String(e));
function call(cmd, args){
  let p;
  try { p = backend.invoke(cmd, args); } catch (e) { return Promise.reject(msgOf(e)); }
  return Promise.resolve(p).catch(e => { throw msgOf(e); });
}
const lift = f => (...a) => { try { return Promise.resolve(f(...a)).catch(e => { throw msgOf(e); }); } catch (e) { return Promise.reject(msgOf(e)); } };

const BT = {
  /** true when running on the mock backend (no Tauri) */
  isMock: !tauri,
  mock: tauri ? null : backend,

  /* ---------------------------------------------------------------- commands */
  getSettings:      ()                      => call('get_settings'),
  saveSettings:     settings                => call('save_settings', { settings }),
  listClips:        ()                      => call('list_clips'),
  setFavourite:     (clipId, fav)           => call('set_favourite', { clipId, fav }),
  /** bulk set_favourite, one write */
  setFavourites:    (clipIds, fav)          => call('set_favourites', { clipIds, fav }),
  saveEdits:        (clipId, edits)         => call('save_edits', { clipId, edits }),
  getWaveform:      (clipId, track, points) => call('get_waveform', { clipId, track, points }),
  getKeyframes:     clipId                  => call('get_keyframes', { clipId }),
  prepareAudio:     clipId                  => call('prepare_audio', { clipId }),
  exportClip:       req                     => call('export_clip', { req }),
  cancelExport:     ()                      => call('cancel_export'),
  saveTrimmedCopy:  (clipId, start, end)    => call('save_trimmed_copy', { clipId, start, end }),
  copyFile:         path                    => call('copy_file', { path }),
  reveal:           path                    => call('reveal', { path }),
  openPath:         path                    => call('open_path', { path }),
  openDiscord:      ()                      => call('open_discord'),
  openTaskbarSettings: ()                   => call('open_taskbar_settings'),
  trashClip:        clipId                  => call('trash_clip', { clipId }),
  /** can the Recycle Bin keep this batch? → { fits, drives: [{ drive, bytes, capacity|null, permanent }] } (no file changes) */
  trashCheck:       clipIds                 => call('trash_check', { clipIds }),
  /** many clips to the Recycle Bin, one at a time → { trashed: [clipId], failed: [{ clipId, error }] }; rejects only if nothing
      could be attempted. { permanent: true } deletes them outright: only when the user chose that in the confirm. */
  trashClips:       (clipIds, o)            => call('trash_clips', o && o.permanent ? { clipIds, permanent: true } : { clipIds }),
  /** Undo: clips this run moved to the Recycle Bin go back (same ids) → { restored: [clipId], failed: [{ clipId, error }] } */
  restoreClips:     clipIds                 => call('restore_clips', { clipIds }),

  /* ---------------------------------------------------------------- phase 2: the recorder */
  recorderStatus:   ()                      => call('recorder_status'),
  setBuffer:        enabled                 => call('set_buffer', { enabled: !!enabled }),
  /** secs: how many seconds to save, or null for the whole buffer → { path, game, duration } */
  saveReplay:       (secs = null)           => call('save_replay', { secs }),
  toggleMicMute:    ()                      => call('toggle_mic_mute'),
  addMarker:        ()                      => call('add_marker'),
  listAudioDevices: ()                      => call('list_audio_devices'),
  listMonitors:     ()                      => call('list_monitors'),
  // Pause Backtrack's global hotkeys while Settings is capturing a new combo (so pressing it doesn't fire it).
  suspendHotkeys:   (suspended)             => call('suspend_hotkeys', { suspended }),
  /** never throws, never rejects: logging must not break the UI */
  log(level, msg){
    try { return call('log', { level: String(level), msg: String(msg).slice(0, 8000) }).catch(() => {}); }
    catch (e) { return Promise.resolve(); }
  },

  /* ---------------------------------------------------------------- events */
  /** BT.on('library-changed' | 'export-progress' | 'recorder-status' | 'clip-saved', fn) → unsubscribe() */
  on(event, fn){
    let off = null, dead = false;
    Promise.resolve(backend.listen(event, fn)).then(u => { if (dead) u(); else off = u; })
      .catch(e => BT.log('error', `listen(${event}) failed: ${msgOf(e)}`));
    return () => { dead = true; if (off) { off(); off = null; } };
  },

  /* ---------------------------------------------------------------- files, window, dialogs */
  /** absolute path on disk → URL usable in <img>/<video>/<audio> (Tauri asset protocol). '' when there is nothing to load. */
  src: p => backend.src(p),
  /** folder picker → absolute path, or null when cancelled */
  pickFolder: lift(opts => backend.pickFolder(opts)),
  appVersion: lift(() => backend.appVersion()),
  window: {
    minimize:       lift(() => backend.window.minimize()),
    toggleMaximize: lift(() => backend.window.toggleMaximize()),
    close:          lift(() => backend.window.close()),
    startDragging:  lift(() => backend.window.startDragging()),
  },
};
window.BT = BT;

/* ------------------------------------------------------------------ uncaught errors → app log */
let errCount = 0;
const MAX_ERR = 100;
window.onerror = function(message, source, line, col, error){
  if (errCount++ < MAX_ERR){
    const where = source ? ` (${String(source).split('/').pop()}:${line}:${col})` : '';
    BT.log('error', `${message}${where}${error && error.stack ? '\n' + error.stack : ''}`);
  }
  return false;
};
window.addEventListener('unhandledrejection', e => {
  if (errCount++ >= MAX_ERR) return;
  const r = e.reason;
  BT.log('error', 'Unhandled promise rejection: ' + (r && r.stack ? r.stack : msgOf(r)));
});
})();
