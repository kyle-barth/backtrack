/* =====================================================================
   Backtrack mock backend
   Used by bridge.js when window.__TAURI__ is missing (a plain browser or
   headless Chrome). It speaks the same commands, events and JSON shapes as
   API.md, so the UI can be built and tested without the Rust side.

   Flags (in the query string or after "?" in the hash, e.g. index.html#library?mock=empty):
     mock=empty       no clip folders set
     mock=missing     a clip folder that holds no clips (moved / deleted)
     mock=noffmpeg    settings.ffmpegPath is null; exports and waveforms fail
     mock=exportfail  the first export fails part-way (to test the error state + Retry)
     mock=slow        the first list_clips takes 6 s, like the real first scan
     mock=static      keep the original recording dates instead of making the newest clip 3 min old
   Recorder (phase 2):
     mock=recorderoff       the replay buffer starts off
     mock=recorderstarting  the buffer stays in "starting"
     mock=recordererror     the encoder is set to NVENC on this (AMD) PC, so the recorder is in "error"
     mock=fresh             the buffer starts empty (otherwise it has been running for 41 s)
     mock=hotkeyconflict    Alt+S is taken by another app (GG), so the save hotkey fails
   Several flags can be combined with commas: mock=slow,exportfail
   ===================================================================== */
(function(){
'use strict';

// Optional thumbnails: index.html?thumbs=<folder url>/ loads <gameId>__<date>__<time>.jpg from there.
// Without it every clip shows the "no thumbnail yet" placeholder.
const THUMBS = new URLSearchParams(location.search).get('thumbs') || '';
const HOME = 'C:\\Users\\you';
const MOMENTS = HOME + '\\Videos\\SteelSeries Moments';
const EXPORTS = HOME + '\\Videos\\Backtrack\\Exports';
const CACHE = HOME + '\\AppData\\Local\\Backtrack\\cache';
const FFMPEG = 'C:\\Program Files\\FFmpeg\\bin\\ffmpeg.exe';
const REC_CLIPS = HOME + '\\Videos\\Backtrack\\Clips';
const MONITORS = [
  {index:0, name:'DELL S2721DGF', width:2560, height:1440, primary:true},
  {index:1, name:'LG 27UL500', width:1440, height:2560, primary:false},   // turned to portrait
];
const INPUTS = [
  {id:'{0.0.1.00000000}.{6b1d42e0-5a3c-4f0e-9d8a-2c1f4e7b9a10}', name:'Microphone (HyperX QuadCast S)', isDefault:true},
  {id:'{0.0.1.00000000}.{9c2e18f4-0b7d-4c55-8e21-7a4d3b6f2c91}', name:'Headset Microphone (Arctis Nova 7)', isDefault:false},
  {id:'{0.0.1.00000000}.{3f7a90c2-6e14-4b8a-a5d3-1c9e8f2b4d67}', name:'Line In (Realtek USB Audio)', isDefault:false},
];
const HK_ACTIONS = ['save', 'toggle', 'muteMic', 'mark', 'open'];

/* ------------------------------------------------------------------ source data
   27 sample clips with made-up dates plus generated older clips, so each game has a
   realistic count (Rocket League: 155; 269 clips in total). */
const T3 = [{index:0,name:'Game',channels:2},{index:1,name:'Chat',channels:2},{index:2,name:'Mic',channels:1}];
const GAMES = [
  {id:'rocket-league', name:'Rocket League', folder:'Rocket League', count:155},
  {id:'counter-strike-2', name:'Counter-Strike 2', folder:'Counter-Strike 2', count:52},
  {id:'peak', name:'PEAK', folder:'PEAK', count:18},
  {id:'call-of-duty', name:'Call of Duty®', folder:'Call of Duty', count:10},
  {id:'helldivers', name:'HELLDIVERS™ 2', folder:'HELLDIVERS 2', count:10},
  {id:'desktopcapture', name:'Desktop capture', folder:'Desktop', count:10,
    tracks:[{index:0,name:'Game',channels:2},{index:1,name:'Mic',channels:1}]},
  {id:'call-of-duty-modern-warfare-2', name:'Call of Duty: Modern Warfare 2 (2009)', folder:'Call of Duty Modern Warfare 2 (2009)', count:8},
  {id:'bodycam', name:'Bodycam', folder:'Bodycam', count:2},
  {id:'big-walk', name:'Big Walk', folder:'Big Walk', count:2,
    tracks:[{index:0,name:'Game',channels:2},{index:1,name:'Chat',channels:2}]},
  {id:'battlefield', name:'Battlefield™ 6', folder:'Battlefield 6', count:2,
    tracks:[{index:0,name:'Game',channels:2},{index:1,name:'Track 2',channels:2}]},
];
// [gameId, date, time, seconds, MB, flags]: made-up sample clips (the dates shift so the newest is 3 minutes old)
const SAMPLE = [
  ['rocket-league','2026-11-20','20-42-10',58,185.6,{story:1,edits:{inT:24.25,outT:48.15,markers:[31.3,44.6]}}],
  ['rocket-league','2026-11-20','20-18-33',60,192.0,{shared:1}],
  ['rocket-league','2026-11-20','19-57-02',45,144.0,{fav:1}],
  ['counter-strike-2','2026-11-19','21-12-45',72,214.3,{}],
  ['counter-strike-2','2026-11-14','20-31-08',48,151.7,{shared:1}],
  ['counter-strike-2','2026-11-14','19-48-27',90,236.8,{fav:1}],
  ['peak','2026-11-19','18-25-14',40,118.2,{}],
  ['peak','2026-11-18','21-06-39',65,176.4,{shared:1}],
  ['peak','2026-11-16','17-44-50',52,149.9,{}],
  ['bodycam','2026-11-15','20-09-22',35,108.4,{}],
  ['bodycam','2026-11-15','19-51-05',50,139.2,{}],
  ['helldivers','2026-11-11','21-37-48',80,231.5,{fav:1,shared:1,edits:{inT:12,outT:44,markers:[30.5]}}],
  ['helldivers','2026-11-08','16-22-31',58,172.9,{}],
  ['helldivers','2026-10-31','20-55-16',105,248.1,{}],
  ['big-walk','2026-11-02','15-13-40',60,121.6,{}],
  ['big-walk','2026-10-17','19-02-57',42,104.3,{}],
  ['call-of-duty','2026-10-28','21-46-03',60,188.7,{shared:1}],
  ['call-of-duty','2026-10-28','21-20-44',52,164.0,{}],
  ['call-of-duty','2026-10-28','20-58-19',84,229.9,{}],
  ['desktopcapture','2026-10-24','14-35-12',30,100.8,{}],
  ['desktopcapture','2026-10-24','14-31-58',44,112.5,{}],
  ['desktopcapture','2026-10-24','14-26-07',70,160.2,{}],
  ['call-of-duty-modern-warfare-2','2026-10-22','19-17-36',46,147.2,{fav:1}],
  ['call-of-duty-modern-warfare-2','2026-10-22','18-49-21',58,176.0,{}],
  ['call-of-duty-modern-warfare-2','2026-10-22','18-12-09',75,219.6,{}],
  ['battlefield','2025-10-18','20-27-43',62,196.3,{}],
  ['battlefield','2025-10-18','20-04-15',55,171.4,{}],
];

/* ------------------------------------------------------------------ helpers */
function rng(seed){let a=seed>>>0;return()=>{a|=0;a=a+0x6D2B79F5|0;let t=Math.imul(a^a>>>15,1|a);t=t+Math.imul(t^t>>>7,61|t)^t;return((t^t>>>14)>>>0)/4294967296;};}
function hash(s){let h=2166136261;for(let i=0;i<s.length;i++){h^=s.charCodeAt(i);h=Math.imul(h,16777619);}return h>>>0;}
const clone = o => JSON.parse(JSON.stringify(o));
const delay = (ms, v) => new Promise(r => setTimeout(() => r(v), ms));
const p2 = n => String(n).padStart(2, '0');
const stamp = d => `${d.getFullYear()}-${p2(d.getMonth()+1)}-${p2(d.getDate())}__${p2(d.getHours())}-${p2(d.getMinutes())}-${p2(d.getSeconds())}`;
const slugTitle = name => name.replace(/[^A-Za-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
const lower = s => s.toLowerCase();
const under = (path, dir) => lower(path).startsWith(lower(dir.replace(/[\\/]+$/, '')) + '\\');
const dirOf = p => p.replace(/[\\/][^\\/]*$/, '');

function parseFlags(){
  const m = (location.search + '&' + location.hash).match(/mock=([\w,-]+)/);
  return new Set(m ? m[1].split(',') : []);
}

/* waveform shapes from the prototype: game = bursts, voice = talk spurts, mic = mostly quiet */
const SR = 40;
function genWave(kind, dur, seed, story){
  const N = Math.ceil(dur * SR), r = rng(seed), a = new Float32Array(N);
  if (kind === 'game'){
    const ev = []; for (let t = .8 + r()*2; t < dur; t += 1.4 + r()*3.8) ev.push([t, .3 + r()*.45]);
    if (story) ev.push([31.3,1],[31.7,.95],[32.4,.85],[33.2,.7],[44.6,.75],[26.2,.6]);
    for (let i = 0; i < N; i++){ const t = i/SR; let v = .17 + .07*Math.sin(t*1.7) + .05*Math.sin(t*5.3+1) + r()*.13;
      for (const [et, ea] of ev){ const d = t - et; if (d >= 0 && d < 1.4) v += ea*Math.exp(-d*3.6)*(.7 + r()*.3); } a[i] = Math.min(1, v); }
  } else if (kind === 'voice'){
    let talking = false, until = 0;
    for (let i = 0; i < N; i++){ const t = i/SR; if (t >= until){ talking = r() < (talking ? .3 : .55); until = t + (talking ? .5 + r()*2.2 : .5 + r()*2.6); }
      let v = .02 + r()*.02;
      if (story && t > 32.2 && t < 37.8) v = .35 + .5*Math.abs(Math.sin(t*13))*(.55 + r()*.45);
      else if (talking) v = .1 + .45*Math.abs(Math.sin(t*11 + r()*.6))*(.45 + r()*.55);
      a[i] = v; }
  } else {
    for (let i = 0; i < N; i++){ const t = i/SR; let v = .015 + r()*.035; if (r() < .005) v = .2 + r()*.25;
      if (story && t > 32.5 && t < 34.6) v = .3 + .45*Math.abs(Math.sin(t*12))*(.55 + r()*.45); a[i] = v; }
  }
  for (let i = 1; i < N; i++) a[i] = a[i]*.62 + a[i-1]*.38;
  return a;
}
function resample(src, points){
  const out = new Array(points), N = src.length;
  for (let p = 0; p < points; p++){
    const i0 = Math.floor(p/points*N), i1 = Math.max(i0 + 1, Math.floor((p + 1)/points*N));
    let m = 0; for (let i = i0; i < i1 && i < N; i++) if (src[i] > m) m = src[i];
    out[p] = Math.round(m*1000)/1000;
  }
  return out;
}

/* the backend's export maths (API.md "Export maths"), mirrored so results look real */
const LADDER = [{k:6000,res:1080,fps:60},{k:3500,res:1080,fps:30},{k:2000,res:720,fps:60},{k:1500,res:720,fps:30},{k:1000,res:480,fps:30},{k:0,res:480,fps:30}];
const even = n => Math.max(2, Math.round(n/2)*2);
function planExport(req, clip){
  const dur = req.end - req.start, vert = req.canvas === '9:16', limit = req.limitMb;
  const copyMB = (clip.videoKbps + 192)*dur/8000;
  if (req.preset === 'orig' || (limit && copyMB <= limit*.95 && !vert)){
    return {mode:'copy', dur, width:clip.width, height:clip.height, fps:Math.round(clip.fps), vk:clip.videoKbps, ak:192, mb:copyMB*.995, secs:Math.max(1, dur/40)};
  }
  const target = limit*.95; let ak = 128, vk = target*8000/dur - ak; if (vk < 1500){ ak = 96; vk = target*8000/dur - ak; }
  vk = Math.min(vk, 20000);
  const L = LADDER.find(l => vk >= l.k), h = Math.min(L.res, clip.height), fps = Math.min(L.fps, Math.round(clip.fps));
  const width = vert ? even(h*9/16) : even(h*clip.width/clip.height), height = h;
  return {mode:'encode', dur, width, height, fps, vk:Math.round(vk), ak, mb:(vk + ak)*dur/8000*.995, secs:Math.max(3, dur*.45)};
}

/* ------------------------------------------------------------------ the mock */
function create(){
  const flags = parseFlags();
  const listeners = {};
  const emit = (ev, payload) => (listeners[ev] || []).slice().forEach(fn => { try { fn(payload); } catch (e) { setTimeout(() => { throw e; }); } });
  const logs = [];

  const settings = {
    clipDirs: flags.has('empty') ? [] : flags.has('missing') ? ['D:\\Old drive\\SteelSeries Moments'] : [MOMENTS],
    exportDir: EXPORTS,
    defaultPreset: 'd10',
    customMb: 25,
    autoCopy: true,
    quickLen: 20,
    ffmpegPath: flags.has('noffmpeg') ? null : FFMPEG,
    recorder: {
      enabled: !flags.has('recorderoff'),
      bufferSecs: 60, resolution: 1080, fps: 60, bitrateMbps: 25,
      encoder: flags.has('recordererror') ? 'nvenc' : 'auto',
      monitor: 0,
      clipsDir: REC_CLIPS,
      hotkeys: {save:'Alt+S', toggle:'Alt+F9', muteMic:'Alt+F8', mark:'Alt+F7', open:'Ctrl+Alt+B'},
      confirm: true, chatApp: 'Discord', micDevice: '', micEnabled: true, startWithWindows: false,
    },
  };
  // like the backend: the recorder's folder is always one of the clip folders
  const ensureRecDir = () => { if (!settings.clipDirs.some(d => lower(d) === lower(settings.recorder.clipsDir))) settings.clipDirs.push(settings.recorder.clipsDir); };
  ensureRecDir();

  /* clips: newest real clip becomes 3 minutes old (unless mock=static), everything else keeps its distance */
  const ts = r => { const [y,m,d] = r[1].split('-').map(Number), [hh,mm,ss] = r[2].split('-').map(Number); return new Date(y, m-1, d, hh, mm, ss).getTime(); };
  const newest = Math.max(...SAMPLE.map(ts));
  const shift = flags.has('static') ? 0 : (Date.now() - 3*60000) - newest;
  const G = Object.fromEntries(GAMES.map(g => [g.id, g]));
  const story = new Set();
  const clips = [];

  /* From file:// on Windows, Chrome can't open a path longer than MAX_PATH (260). Deep workspaces push a few
     reference thumbnails past it; treat those as "thumbnail not ready yet" (thumb: null) instead of a failed load. */
  const thumbOK = key => {
    if (!THUMBS) return false;
    if (location.protocol !== 'file:') return true;
    try { return decodeURIComponent(new URL(THUMBS + key + '.jpg', location.href).pathname).replace(/^\//, '').length < 260; }
    catch (e) { return true; }
  };
  function mkClip(gameId, at, dur, mb, thumbKey, f){
    const g = G[gameId], tracks = g.tracks || T3;
    if (thumbKey && !thumbOK(thumbKey)) thumbKey = null;
    const fileName = `${slugTitle(g.name)}__${stamp(new Date(at))}.mp4`;
    const path = `${MOMENTS}\\${g.folder}\\${fileName}`;
    const id = hash(path).toString(16).padStart(8, '0') + hash(path + '#').toString(16).padStart(8, '0');
    const sizeBytes = Math.round(mb*1e6);
    const c = {
      id, path, fileName, game:g.name, gameId:g.id, recordedAt:at, duration:dur, sizeBytes,
      width:1920, height:1080, fps:60, videoKbps:Math.round(mb*8000/dur) - tracks.length*160,
      audioTracks:clone(tracks), thumb:thumbKey ? `${THUMBS}${thumbKey}.jpg` : null,
      fav:!!f.fav, edits:f.edits ? clone(f.edits) : null, exports:[], source:'gg',
    };
    if (f.shared){
      const base = fileName.replace(/\.mp4$/, '') + '_discord';
      c.exports.push({path:`${EXPORTS}\\${base}.mp4`, sizeBytes:Math.round(9.2e6 + (hash(id) % 300)*1000), preset:'d10', width:1280, height:720, fps:60, at:at + 3600000*(2 + hash(id) % 30)});
    }
    if (f.story) story.add(id);
    return c;
  }
  SAMPLE.forEach(r => clips.push(mkClip(r[0], ts(r) + shift, r[3], r[4], `${r[0]}__${r[1]}__${r[2]}`, r[5])));
  GAMES.forEach(g => {
    const real = SAMPLE.filter(r => r[0] === g.id), extra = g.count - real.length; if (extra <= 0) return;
    const r = rng(hash(g.id)); let t = Math.min(...real.map(ts)) + shift;
    for (let i = 0; i < extra; i++){
      t -= (0.6 + r()*2.6)*86400000 + r()*3600000*5;
      const dur = 30 + Math.round(r()*90), mb = Math.round(dur*(2.6 + r()*.6)*10)/10;
      const src = real[i % real.length], noThumb = i % 41 === 40;   // a few thumbnails "not ready yet"
      clips.push(mkClip(g.id, t, dur, mb, noThumb ? null : `${src[0]}__${src[1]}__${src[2]}`, {fav:r() < .12, shared:r() < .3}));
    }
  });
  const byId = id => clips.find(c => c.id === id);
  const need = (id) => { const c = byId(id); if (!c) throw 'This clip is no longer in your library.'; return c; };
  const visible = () => clips.filter(c => settings.clipDirs.some(d => under(c.path, d))).sort((a, b) => b.recordedAt - a.recordedAt);

  let firstList = true;
  const waveCache = new Map();
  let job = null, failedOnce = false;
  const usedNames = new Set(clips.flatMap(c => c.exports.map(e => lower(e.path))));

  function waveKind(name){ return /^game$/i.test(name) ? 'game' : /^mic$/i.test(name) ? 'mic' : 'voice'; }

  /* ---------------------------------------------------------------- the recorder (phase 2) */
  const rec = {state:'off', error:null, startedAt:0, micMuted:false, lastSaved:null, marks:[], lv:{g:.3, v:0, m:.04}, talk:0, t:0, startT:0};
  const discordRunning = true;
  const PIPELINE = r => [r.bufferSecs, r.resolution, r.fps, r.bitrateMbps, r.encoder, r.monitor, r.chatApp, r.micDevice, r.micEnabled].join('|');
  function encoderInfo(){
    const e = settings.recorder.encoder;
    if (e === 'nvenc') return {err:"NVIDIA NVENC isn't available on this PC: no NVIDIA graphics card was found. Pick Automatic or AMD AMF in Settings → Encoder."};
    if (e === 'x264') return {name:'CPU · x264'};
    return {name:'AMD AMF · Radeon RX 7900 XTX'};
  }
  const micName = () => { const r = settings.recorder; const d = r.micDevice && INPUTS.find(i => i.id === r.micDevice); return (d || INPUTS.find(i => i.isDefault)).name; };
  function recSize(){
    const r = settings.recorder, mon = MONITORS[r.monitor] || MONITORS[0];
    const short = Math.min(mon.width, mon.height), long = Math.max(mon.width, mon.height);
    const s = r.resolution ? Math.min(r.resolution, short) : short, l = even(s*long/short);
    return mon.width >= mon.height ? [l, s] : [s, l];
  }
  function hotkeyStatus(){
    const hk = settings.recorder.hotkeys;
    return HK_ACTIONS.map((a, i) => {
      const keys = hk[a] || '';
      const dup = keys && HK_ACTIONS.slice(0, i).some(b => (hk[b] || '').toLowerCase() === keys.toLowerCase());
      const taken = flags.has('hotkeyconflict') && keys === 'Alt+S';
      const error = taken ? `${keys} is already used by another app, maybe SteelSeries GG. Pick another key.`
        : dup ? `${keys} is already used for another Backtrack action.` : null;
      return {action:a, keys, ok:!error, error};
    });
  }
  function status(){
    const r = settings.recorder, on = rec.state === 'on', info = encoderInfo();
    const filled = on ? Math.min(r.bufferSecs, (Date.now() - rec.startedAt)/1000) : 0;
    const [w, h] = recSize(), mic = r.micEnabled && !rec.micMuted;
    return {
      state:rec.state, error:rec.error, bufferSecs:r.bufferSecs, filledSecs:Math.round(filled*10)/10,
      memoryMb:Math.round(r.bitrateMbps*filled/8 + (on ? 3 : 0)), encoder:info.name || '', width:w, height:h, fps:r.fps,
      game:on ? 'Rocket League' : null, micMuted:rec.micMuted,
      tracks:[
        {name:'Game', source:r.chatApp ? `All apps except ${r.chatApp}` : 'All apps', level:on ? rec.lv.g : 0, active:true},
        {name:'Voice chat', source:r.chatApp || 'None', level:on && r.chatApp ? rec.lv.v : 0, active:!!r.chatApp && discordRunning},
        {name:'Mic', source:micName(), level:on && mic ? rec.lv.m : 0, active:r.micEnabled},
      ],
      hotkeys:hotkeyStatus(), lastSaved:rec.lastSaved ? clone(rec.lastSaved) : null,
    };
  }
  const emitStatus = () => emit('recorder-status', status());
  function startBuffer(fromEmpty){
    const info = encoderInfo();
    if (info.err){ rec.state = 'error'; rec.error = info.err; emitStatus(); return; }
    rec.state = 'starting'; rec.error = null; emitStatus();
    if (flags.has('recorderstarting')) return;
    clearTimeout(rec.startT);
    rec.startT = setTimeout(() => { if (rec.state !== 'starting') return; rec.state = 'on'; rec.startedAt = Date.now() - (fromEmpty ? 0 : 41000); emitStatus(); }, fromEmpty ? 800 : 0);
  }
  function stopBuffer(){ clearTimeout(rec.startT); rec.state = 'off'; rec.error = null; emitStatus(); }
  // levels that move: game bursts, talk spurts on voice chat, a quieter mic
  function stepLevels(){
    const L = rec.lv, t = (rec.t += .5);
    L.g = Math.min(1, .22 + .12*Math.abs(Math.sin(t*.9)) + Math.random()*.22 + (Math.random() < .18 ? .35 + Math.random()*.25 : 0));
    if (rec.talk > 0) rec.talk--; else if (Math.random() < .25) rec.talk = 2 + Math.floor(Math.random()*5);
    L.v = rec.talk > 0 ? .35 + Math.random()*.45 : Math.random()*.04;
    L.m = Math.random() < .2 ? .25 + Math.random()*.4 : .02 + Math.random()*.05;
    for (const k in L) L[k] = Math.round(L[k]*100)/100;
  }
  setInterval(() => { if (rec.state !== 'on') return; stepLevels(); emitStatus(); }, 500);
  if (settings.recorder.enabled){
    if (flags.has('fresh') || flags.has('recorderstarting')) setTimeout(() => startBuffer(true), 50);
    else { const info = encoderInfo(); if (info.err){ rec.state = 'error'; rec.error = info.err; } else { rec.state = 'on'; rec.startedAt = Date.now() - 41000; } }
  }
  // a saved replay: clip-saved right away, then the file is indexed (library-changed) and gets its thumbnail (library-changed again)
  function saveReplay(secs){
    const r = settings.recorder, st = status();
    if (st.state !== 'on') return Promise.reject('The replay buffer is off. Turn it on to save clips.');
    if (st.filledSecs < 1) return Promise.reject('Nothing to save yet: the replay buffer has only just started.');
    const dur = Math.round(Math.min(secs || r.bufferSecs, st.filledSecs)*10)/10, at = Date.now(), game = st.game || 'Desktop';
    const fileName = `${slugTitle(game)}__${stamp(new Date(at))}.mp4`, path = `${r.clipsDir}\\${fileName}`;
    const payload = {path, game, duration:dur};
    rec.lastSaved = {path, game, duration:dur, at};
    const marks = rec.marks.filter(m => m <= at && m >= at - dur*1000).map(m => Math.round((dur - (at - m)/1000)*100)/100);
    rec.marks = rec.marks.filter(m => m > at);
    const vk = r.bitrateMbps*1000, sizeBytes = Math.round((vk + 3*160)*dur/8*1000);
    const id = hash(path).toString(16).padStart(8, '0') + hash(path + '#').toString(16).padStart(8, '0');
    const c = {id, path, fileName, game, gameId:'rocket-league', recordedAt:at, duration:dur, sizeBytes, width:st.width, height:st.height, fps:r.fps,
      videoKbps:vk, audioTracks:clone(T3), thumb:null, fav:false, edits:marks.length ? {inT:0, outT:dur, markers:marks} : null, exports:[], source:'backtrack'};
    setTimeout(() => { emit('clip-saved', clone(payload)); emitStatus(); }, 250);
    setTimeout(() => { clips.push(c); emit('library-changed', null); }, 1200);
    const key = 'rocket-league__2026-11-20__00-01-27';
    setTimeout(() => { if (thumbOK(key)){ c.thumb = `${THUMBS}${key}.jpg`; emit('library-changed', null); } }, 2600);
    return delay(250, payload);
  }

  const commands = {
    get_settings: () => clone(settings),
    save_settings({settings: s}){
      const dirsBefore = JSON.stringify(settings.clipDirs);
      const seen = new Set();
      settings.clipDirs = (s.clipDirs || []).filter(d => d && d.trim() && !seen.has(lower(d)) && seen.add(lower(d)));
      settings.exportDir = (s.exportDir || '').trim() || EXPORTS;
      settings.defaultPreset = ['d10','d50','d500','orig','custom'].includes(s.defaultPreset) ? s.defaultPreset : 'd10';
      settings.customMb = s.customMb > .5 ? +s.customMb : 25;
      settings.autoCopy = !!s.autoCopy;
      settings.quickLen = s.quickLen >= 3 ? +s.quickLen : 20;
      // recorder: same normalisation as the backend; pipeline changes restart (and empty) the buffer
      if (s.recorder){
        const r0 = settings.recorder, n = Object.assign({}, r0, clone(s.recorder));
        if (![15, 30, 60, 120, 300].includes(+n.bufferSecs)) n.bufferSecs = Math.max(10, Math.min(600, +n.bufferSecs || 60));
        if (![0, 720, 1080, 1440, 2160].includes(+n.resolution)) n.resolution = 1080;
        if (![30, 60, 120, 144].includes(+n.fps)) n.fps = 60;
        n.bitrateMbps = Math.max(4, Math.min(100, +n.bitrateMbps || 25));
        if (!['auto', 'amf', 'nvenc', 'x264'].includes(n.encoder)) n.encoder = 'auto';
        if (!String(n.clipsDir || '').trim()) n.clipsDir = REC_CLIPS;
        n.hotkeys = Object.assign({}, r0.hotkeys, n.hotkeys || {});
        const restart = PIPELINE(n) !== PIPELINE(r0), toggled = !!n.enabled !== !!r0.enabled, hkChanged = JSON.stringify(n.hotkeys) !== JSON.stringify(r0.hotkeys);
        settings.recorder = n;
        if (toggled) n.enabled ? startBuffer(true) : stopBuffer();
        else if (restart && (n.enabled || rec.state === 'error')) startBuffer(true);
        else if (hkChanged) setTimeout(emitStatus, 30);   // hotkeys re-registered
      }
      ensureRecDir();
      if (JSON.stringify(settings.clipDirs) !== dirsBefore) setTimeout(() => emit('library-changed', null), 120);
      return clone(settings);
    },
    recorder_status: () => status(),
    set_buffer({enabled}){
      settings.recorder.enabled = !!enabled;
      enabled ? startBuffer(true) : stopBuffer();
      return delay(60, status());
    },
    save_replay: ({secs}) => saveReplay(secs),
    toggle_mic_mute(){ rec.micMuted = !rec.micMuted; setTimeout(emitStatus, 0); return delay(40, status()); },
    add_marker(){
      if (rec.state !== 'on') return Promise.reject('The replay buffer is off, so there is nothing to mark.');
      rec.marks.push(Date.now()); return delay(30);
    },
    list_audio_devices: () => delay(80, {inputs:clone(INPUTS), chatApps:[{name:'Discord', running:discordRunning}]}),
    list_monitors: () => delay(60, clone(MONITORS)),
    list_clips(){
      const wait = firstList && flags.has('slow') ? 6000 : firstList ? 180 : 40;
      firstList = false;
      return delay(wait).then(() => clone(visible()));
    },
    set_favourite({clipId, fav}){ need(clipId).fav = !!fav; },
    save_edits({clipId, edits}){
      const c = need(clipId);
      if (!(edits && edits.outT > edits.inT)) throw 'Invalid edits: Out must be after In.';
      c.edits = {inT:+edits.inT, outT:+edits.outT, markers:(edits.markers || []).map(Number).sort((a, b) => a - b)};
    },
    get_keyframes({clipId}){
      // GG-style clips: a keyframe every 2.083 s (125 frames at 60 fps)
      const c = need(clipId), out = [];
      for (let t = 0; t < c.duration; t += 125 / 60) out.push(+t.toFixed(6));
      return delay(60, out);
    },
    get_waveform({clipId, track, points}){
      if (!settings.ffmpegPath) return Promise.reject("FFmpeg wasn't found. Install it (winget install Gyan.FFmpeg.Essentials) and restart Backtrack.");
      const c = need(clipId), t = c.audioTracks.find(x => x.index === track);
      if (!t) throw `Audio track ${track} doesn't exist in this clip.`;
      const key = clipId + ':' + track;
      let src = waveCache.get(key), cached = !!src;
      if (!src){ src = genWave(waveKind(t.name), c.duration, hash(clipId) + track*7, story.has(clipId)); waveCache.set(key, src); }
      return delay(cached ? 25 : 300 + track*140, resample(src, Math.max(1, Math.min(20000, points|0))));
    },
    prepare_audio({clipId}){
      if (!settings.ffmpegPath) return Promise.reject("FFmpeg wasn't found. Install it (winget install Gyan.FFmpeg.Essentials) and restart Backtrack.");
      const c = need(clipId);
      return delay(150, c.audioTracks.map(t => ({index:t.index, name:t.name, path:`${CACHE}\\audio\\${c.id}_${t.index}.m4a`})));
    },
    export_clip({req}){
      if (job) return Promise.reject('Another export is already running.');
      if (!settings.ffmpegPath) return Promise.reject("FFmpeg wasn't found. Install it (winget install Gyan.FFmpeg.Essentials) and restart Backtrack.");
      const c = need(req.clipId);
      if (!(req.end > req.start)) return Promise.reject('The selection is empty.');
      const plan = planExport(req, c);
      // a faster clock than real x264 (≈ 0.45 s per selected second), so screenshots and tests finish quickly
      const T = plan.mode === 'copy' ? 1300 : Math.min(3200, Math.max(2000, 1600 + plan.dur*50));
      const fail = flags.has('exportfail') && !failedOnce;
      return new Promise((resolve, reject) => {
        const t0 = Date.now();
        const stop = () => { clearInterval(iv); job = null; };
        job = {cancel(){ stop(); reject('cancelled'); }};
        const iv = setInterval(() => {
          const k = Math.min(1, (Date.now() - t0)/T);
          let stage, pct;
          if (plan.mode === 'copy'){ stage = k < 1 ? 'copy' : 'finish'; pct = k < 1 ? k*95 : 100; }
          else { stage = k < .3 ? 'analyse' : k < 1 ? 'encode' : 'finish'; pct = k < .3 ? k*100 : k < 1 ? 30 + (k - .3)/.7*67 : 100; }
          const eta = k < .06 ? null : Math.round(plan.secs*(1 - k)*10)/10;
          emit('export-progress', {stage, percent:Math.round(pct*10)/10, etaSec:k < 1 ? eta : 0});
          if (fail && k > .45){ failedOnce = true; stop(); reject('FFmpeg exited with code 1 during pass 2: "Error while opening encoder for output stream #0:0 - maybe incorrect parameters such as bit_rate, rate, width or height".'); return; }
          if (k < 1) return;
          stop();
          const base = (req.fileName || 'clip').replace(/[<>:"/\\|?*\u0000-\u001f]+/g, '_').replace(/[. ]+$/, '').slice(0, 120) || 'clip';
          let name = base + '.mp4', n = 2;
          while (usedNames.has(lower(settings.exportDir + '\\' + name))) name = `${base} (${n++}).mp4`;
          const path = settings.exportDir + '\\' + name; usedNames.add(lower(path));
          const res = {path, fileName:name, sizeBytes:Math.round(plan.mb*1e6), width:plan.width, height:plan.height, fps:plan.fps,
            videoKbps:plan.vk, audioKbps:plan.ak, durationSec:Math.round(plan.dur*1000)/1000, elapsedSec:Math.round(plan.secs*10)/10,
            mode:plan.mode, copied:!!req.copyToClipboard};
          c.exports.unshift({path, sizeBytes:res.sizeBytes, preset:req.preset, width:res.width, height:res.height, fps:res.fps, at:Date.now()});
          resolve(res);
        }, 100);
      });
    },
    cancel_export(){ if (job) job.cancel(); },
    save_trimmed_copy({clipId, start, end}){
      if (job) return Promise.reject('An export is running; try again when it finishes.');
      const c = need(clipId);
      if (!(end > start)) return Promise.reject('The selection is empty.');
      const s0 = Math.max(0, start - 1.2);   // cuts snap to the earlier keyframe
      const base = c.fileName.replace(/\.mp4$/, '') + '_trim';
      let name = base + '.mp4', n = 2;
      while (usedNames.has(lower(settings.exportDir + '\\' + name))) name = `${base} (${n++}).mp4`;
      const path = settings.exportDir + '\\' + name; usedNames.add(lower(path));
      const sizeBytes = Math.round(c.sizeBytes*(end - s0)/c.duration);
      c.exports.unshift({path, sizeBytes, preset:'lossless', width:c.width, height:c.height, fps:Math.round(c.fps), at:Date.now()});
      return delay(700, {path, sizeBytes});
    },
    copy_file: ({path}) => { logs.push(['info', 'copy_file ' + path]); return delay(40); },
    reveal: ({path}) => { logs.push(['info', 'reveal ' + path]); return delay(30); },
    open_path: ({path}) => { logs.push(['info', 'open_path ' + path]); return delay(30); },
    open_discord: () => { logs.push(['info', 'open_discord']); return delay(30); },
    trash_clip({clipId}){
      const c = need(clipId); clips.splice(clips.indexOf(c), 1);
      setTimeout(() => emit('library-changed', null), 60);
    },
    log({level, msg}){ logs.push([level, msg]); if (console.debug) console.debug('[backtrack:' + level + ']', msg); },
  };

  const folders = ['D:\\Clips\\SteelSeries Moments', 'E:\\Game captures', HOME + '\\Videos\\Backtrack\\Exports (shared)'];
  let pick = 0;

  return {
    flags, logs,
    invoke(cmd, args){
      const fn = commands[cmd];
      if (!fn) return Promise.reject(`Unknown command: ${cmd}`);
      try { return Promise.resolve(fn(args || {})); } catch (e) { return Promise.reject(e); }
    },
    src(p){ return p && /^(\.\.?\/|data:|blob:|https?:)/.test(p) ? p : ''; },  // the mock has thumbnails, but no video or audio files
    listen(ev, fn){ (listeners[ev] = listeners[ev] || []).push(fn); return Promise.resolve(() => { listeners[ev] = (listeners[ev] || []).filter(f => f !== fn); }); },
    emit,
    window: { minimize: () => delay(0), toggleMaximize: () => delay(0), close: () => delay(0), startDragging: () => delay(0) },
    pickFolder: () => delay(120, folders[pick++ % folders.length]),
    appVersion: () => Promise.resolve('0.1.0'),
    dirOf,
  };
}

window.BTMock = { create };
})();
