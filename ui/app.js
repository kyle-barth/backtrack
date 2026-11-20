/* =====================================================================
   Backtrack UI. Every bit of data and every side effect goes through
   window.BT (bridge.js); nothing here knows whether it talks to Tauri or
   to the mock backend.
   ===================================================================== */
(function(){
'use strict';
const $=(s,r=document)=>r.querySelector(s), $$=(s,r=document)=>[...r.querySelectorAll(s)];
const ic=(n,c='')=>`<svg class="i ${c}"><use href="#i-${n}"/></svg>`;
const esc=s=>String(s==null?'':s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const errMsg=e=>typeof e==='string'?e:(e&&e.message)?e.message:String(e);
const clamp=(v,a,b)=>Math.max(a,Math.min(b,v));
const r3=v=>Math.round(v*1000)/1000;
const lsGet=(k,d)=>{try{const v=localStorage.getItem(k);return v==null?d:v;}catch(_){return d;}};
const lsSet=(k,v)=>{try{localStorage.setItem(k,v);}catch(_){}};

/* ================================================================== STATE */
const S={settings:null,clips:[],byId:{},games:[],G:{},loaded:false,loadError:null,version:''};
const NEW_MS=30*60000;                                  // "New" = recorded in the last 30 minutes
const isNewClip=c=>{const d=Date.now()-c.recordedAt;return d>-120000&&d<NEW_MS;};
const isShared=c=>!!(c.exports&&c.exports.some(e=>e.preset!=='lossless')); // a lossless trimmed copy isn't a Discord share
const MBof=b=>b/1e6;                                    // the backend's maths use 1 MB = 1,000,000 bytes
function setClips(list){
  S.clips=[...list].sort((a,b)=>b.recordedAt-a.recordedAt);
  S.byId=Object.fromEntries(S.clips.map(c=>[c.id,c]));
  const m=new Map();
  for(const c of S.clips){
    let g=m.get(c.gameId);
    if(!g){g={id:c.gameId,name:c.game,count:0,bytes:0,clips:[],thumb:null};m.set(c.gameId,g);}
    g.count++;g.bytes+=c.sizeBytes;g.clips.push(c);if(!g.thumb&&c.thumb)g.thumb=c.thumb;
  }
  S.games=[...m.values()].sort((a,b)=>b.count-a.count||b.clips[0].recordedAt-a.clips[0].recordedAt);
  S.G=Object.fromEntries(S.games.map(g=>[g.id,g]));
}

/* ================================================================== FORMAT */
const MON=['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
const MONTH=['January','February','March','April','May','June','July','August','September','October','November','December'];
const DOW=['Sunday','Monday','Tuesday','Wednesday','Thursday','Friday','Saturday'];
function fmt(t,d=2){const f=Math.pow(10,d);t=Math.round(Math.max(0,t)*f)/f;const m=Math.floor(t/60);let s=(t-m*60).toFixed(d);if(parseFloat(s)<10)s='0'+s;return m+':'+s;}
const fmtLen=s=>{s=Math.round(s);return Math.floor(s/60)+':'+String(s%60).padStart(2,'0');};
const fmtMB=mb=>mb>=1000?(mb/1000).toFixed(2)+' GB':mb.toFixed(1)+' MB';
const fmtTotal=b=>b>=1e9?(b/1e9).toFixed(1)+' GB':Math.round(b/1e6)+' MB';
const fmtK=k=>k>=1000?(k/1000).toFixed(1)+' Mbps':Math.round(k)+' kbps';
const fmtKfull=k=>Math.round(k).toLocaleString('en-US')+' kbps';
const fmtNum=n=>String(+(+n).toFixed(1));
const fmtSecs=s=>Math.round(s)+' s';
function timeLabel(d){let h=d.getHours(),m=d.getMinutes();const ap=h<12?'AM':'PM';h=h%12||12;return `${h}:${String(m).padStart(2,'0')} ${ap}`;}
function dayDiff(d){const n=new Date(),a=new Date(d.getFullYear(),d.getMonth(),d.getDate()),b=new Date(n.getFullYear(),n.getMonth(),n.getDate());return Math.round((b-a)/86400000);}
function dayLabel(d){const n=dayDiff(d);if(n<=0)return 'Today';if(n===1)return 'Yesterday';if(n<7)return DOW[d.getDay()];return MON[d.getMonth()]+' '+d.getDate()+(d.getFullYear()!==new Date().getFullYear()?', '+d.getFullYear():'');}
const whenLabel=d=>`${dayLabel(d)}, ${timeLabel(d)}`;
const longDate=d=>`${MON[d.getMonth()]} ${d.getDate()}, ${d.getFullYear()} · ${timeLabel(d)}`;
function ago(ts){
  const m=Math.max(0,Math.round((Date.now()-ts)/60000));
  if(m<1)return 'just now';if(m<60)return m+' min ago';
  const h=Math.round(m/60);if(h<24)return h+' h ago';
  const d=dayDiff(new Date(ts));if(d<=1)return 'yesterday';if(d<30)return d+' days ago';
  return dayLabel(new Date(ts));
}
const fpsR=c=>Math.round(c.fps)||60;
const qualLbl=c=>`${c.height}p · ${fpsR(c)} fps`;
function aspectLbl(c){const r=c.width/c.height;for(const [n,v] of [['16:9',16/9],['21:9',64/27],['16:10',1.6],['4:3',4/3]])if(Math.abs(r-v)<.03)return n;return `${c.width}×${c.height}`;}
const dirOf=p=>String(p||'').replace(/[\\/][^\\/]*$/,'');
const baseName=p=>String(p||'').split(/[\\/]/).pop();
const under=(path,dir)=>{const d=String(dir||'').replace(/[\\/]+$/,'').toLowerCase();return !!d&&String(path||'').toLowerCase().replace(/\//g,'\\').startsWith(d.replace(/\//g,'\\')+'\\');};
/* keep the end of a long path: C:\Users\you\Videos\SteelSeries Moments\Rocket League → …\SteelSeries Moments\Rocket League */
function shortPath(p,max=34){p=String(p||'');if(p.length<=max)return p;const parts=p.split(/[\\/]/);let out=parts.pop();
  while(parts.length){const nx=parts[parts.length-1]+'\\'+out;if(nx.length+2>max)break;out=nx;parts.pop();}return '…\\'+out;}

/* ================================================================== IMAGES */
const initial=n=>(String(n||'?').match(/[A-Za-z0-9]/)||['?'])[0];
function thumbHTML(path,o={}){
  const u=path?BT.src(path):'';
  return u?`<img class="tim" src="${esc(u)}" alt=""${o.eager?'':' loading="lazy"'} draggable="false">`:`<span class="ph-thumb">${ic('film')}</span>`;
}
function gameIcon(g,cls=''){
  const u=g&&g.thumb?BT.src(g.thumb):'';
  return u?`<img class="gim ${cls}" src="${esc(u)}" alt="" data-ini="${esc(initial(g.name))}" draggable="false">`:`<span class="gi-ph ${cls}">${esc(initial(g&&g.name))}</span>`;
}
// a thumbnail that fails to load (file gone, not generated yet) becomes the neutral placeholder
document.addEventListener('error',e=>{
  const t=e.target;if(!(t instanceof HTMLImageElement))return;
  if(t.classList.contains('tim')){const s=document.createElement('span');s.className='ph-thumb';s.innerHTML=ic('film');t.replaceWith(s);}
  else if(t.classList.contains('gim')){const s=document.createElement('span');s.className='gi-ph '+[...t.classList].filter(c=>c!=='gim').join(' ');s.textContent=t.dataset.ini||'?';t.replaceWith(s);}
},true);

/* ================================================================== TOAST + POPOVER + CONFIRM */
let toastT=0;
/* action = {label, kbd, icon, fn, ms}: e.g. Undo after a one-click trim. Toasts with an action stay longer and take clicks. */
function toast(msg,icon='check-c',action){
  const t=$('#toast'),a=$('#toastAct'),undo=!!(action&&action.undo);if(!undo)pauseUndo();$('use',t).setAttribute('href','#i-'+icon);const m=$('#toastMsg');m.textContent=msg;m.title=msg;
  if(action){a.hidden=false;a.innerHTML=`${ic(action.icon||'undo','xs')}${esc(action.label)}${action.kbd?`<kbd>${action.kbd}</kbd>`:''}`;a.onclick=()=>{t.classList.remove('show');action.fn();if(!undo)setTimeout(showUndo,300);};}
  else{a.hidden=true;a.onclick=null;}
  t.classList.toggle('has-act',!!action);t.classList.toggle('is-undo',undo);t.classList.add('show');clearTimeout(toastT);
  // when another toast ends while a library Undo is still live, the Undo comes back for the time it has left
  toastT=setTimeout(()=>{t.classList.remove('show');if(!undo)setTimeout(showUndo,300);},action?(action.ms||6000):icon==='alert'?4500:2400);
}
function fail(what,e){const m=errMsg(e);BT.log('error',`${what}: ${m}`);toast(`${what} · ${m}`,'alert');}
/* library Undo (a move to the Recycle Bin, a bulk ★): 6 s of visible Undo. Another toast or a dialog can cover its toast; its
   clock pauses while it's covered, Ctrl+Z keeps working meanwhile, and the toast comes back afterwards with the time it had left. */
let libUndo=null;
function undoToast(msg,icon,fn,label){libUndo={fn,msg,icon,label:label||'Undo',until:Date.now()+6000,pausedAt:0};showUndo();}
function pauseUndo(){const t=$('#toast');if(libUndo&&!libUndo.pausedAt&&t.classList.contains('show')&&t.classList.contains('is-undo'))libUndo.pausedAt=Date.now();}
function resumeUndo(u){if(u&&u.pausedAt){u.until+=Date.now()-u.pausedAt;u.pausedAt=0;}}
function showUndo(){
  const u=libUndo;if(!u||!$('#confirmLayer').hidden)return;   // a dialog is up: it comes back when the dialog closes
  resumeUndo(u);const left=u.until-Date.now();if(left<800){libUndo=null;return;}
  toast(u.msg,u.icon,{label:u.label,kbd:cur()==='library'?'Ctrl Z':'',icon:'undo',undo:true,ms:left,fn:()=>{if(libUndo===u)libUndo=null;u.fn();}});
}
function runUndo(){
  const u=libUndo;libUndo=null;resumeUndo(u);if(!u||Date.now()>u.until)return false;
  const t=$('#toast');if(t.classList.contains('is-undo'))t.classList.remove('show');u.fn();return true;
}

let popEl=null;
function closePop(){if(popEl){popEl.remove();popEl=null;}$$('.menu-for').forEach(x=>x.classList.remove('menu-for'));}
function placePop(p,anchor,align,at){
  document.body.appendChild(p);popEl=p;
  const r=at?{left:at.x,right:at.x,top:at.y,bottom:at.y}:anchor.getBoundingClientRect(),pw=p.offsetWidth,ph=p.offsetHeight;
  let x=align==='right'?r.right-pw:r.left, y=r.bottom+(at?2:6); if(y+ph>innerHeight-8) y=r.top-ph-6; x=Math.max(8,Math.min(x,innerWidth-pw-8));
  p.style.left=x+'px';p.style.top=Math.max(8,y)+'px';
  setTimeout(()=>document.addEventListener('pointerdown',function h(e){if(popEl&&!popEl.contains(e.target)&&!(anchor&&anchor.contains(e.target))){closePop();}document.removeEventListener('pointerdown',h);},{once:true}),0);
}
/* items: {label, sub, icon, on, right, disabled} or {sep:true}; o = {head: html, cls} */
function openPop(anchor,items,onPick,align='left',at,o={}){
  closePop();const p=document.createElement('div');p.className='pop'+(o.cls?' '+o.cls:'');p.setAttribute('role','menu');
  p.innerHTML=(o.head?`<div class="pop-head">${o.head}</div>`:'')+items.map((it,i)=>it.sep?'<div class="pop-sep"></div>':`<button data-i="${i}" role="menuitem"${it.disabled?' disabled':''}><span class="ck">${it.on?ic('check','sm'):it.icon?ic(it.icon,'sm'):''}</span><span class="pm">${esc(it.label)}${it.sub?`<small>${esc(it.sub)}</small>`:''}</span>${it.right?`<em>${esc(it.right)}</em>`:''}</button>`).join('');
  placePop(p,anchor,align,at);
  p.addEventListener('click',e=>{const b=e.target.closest('button');if(!b)return;const it=items[+b.dataset.i];closePop();onPick(it);});
  return p;
}
/* editor shortcuts live behind ? instead of a permanent hint strip */
const KEYS=[[['Space'],'Play / pause'],[['I'],['O'],'Set In / Out at the playhead'],[['←'],['→'],'Prev / next frame · Shift: 1 s'],[['Home'],['End'],'Jump to In / Out'],[['L'],'Loop the selection'],[['M'],'Mark a moment'],[['Del'],'Remove the marker at the playhead'],[['Ctrl','Z'],'Undo trim change · Ctrl+Y redo'],[['+'],['−'],'Zoom timeline · or Ctrl+wheel'],[['F'],'Full-screen preview'],[['E'],'Export for Discord'],[['Shift','E'],'Export options'],[['Alt','←'],'Back to the library']];
function openKeys(anchor){
  if(popEl&&popEl.classList.contains('shortcuts')){closePop();return;}
  closePop();const p=document.createElement('div');p.className='pop shortcuts';p.setAttribute('role','dialog');p.setAttribute('aria-label','Keyboard shortcuts');
  p.innerHTML=`<div class="shortcuts-h">Keyboard shortcuts<kbd>?</kbd></div>`+KEYS.map(r=>{const lbl=r[r.length-1],ks=r.slice(0,-1);
    return `<div class="kr"><span class="kk">${ks.map(k=>k.map(x=>`<kbd>${x}</kbd>`).join('')).join('<span class="muted">/</span>')}</span><span>${lbl}</span></div>`;}).join('');
  placePop(p,anchor,'right');
}
let cfResolve=null,cfKeep=false,cfBack=null;
const shown=el=>!!el&&el.isConnected&&el.getClientRects().length>0&&getComputedStyle(el).visibility!=='hidden'&&!el.closest('[inert]');
/* o = {title, sub, html, ok, no:'Cancel', okCls:'danger', icon:'trash', tone:'bad', keep}. keep: a yes resolves but leaves the
   dialog open, so the caller can show cfBusy() and then close it (closeConfirm(true,true)) or ask again with new content. */
function confirmBox(o){
  return new Promise(res=>{
    if(cfResolve)cfResolve(false);cfResolve=res;cfKeep=!!o.keep;
    const L=$('#confirmLayer'),fresh=L.hidden;if(fresh){cfBack=document.activeElement;pauseUndo();$('#toast').classList.remove('show');}   // no toast over a dialog (a live Undo comes back after)
    cfBusy(null);cfFill(o);
    closePop();L.hidden=false;$('#app').inert=true;$('#modalLayer').inert=true;
    if(!fresh)$('#confirm').animate([{opacity:.6},{opacity:1}],{duration:160});
    setTimeout(()=>{const b=$('#cfNo');b&&b.focus();},0);   // a destructive dialog focuses Cancel
  });
}
/* content and look; also updates an open dialog in place (e.g. when "Keep favourites" changes what will happen).
   o.wait: OK is disabled and says what it's waiting for (the Recycle Bin check); o.alt: a third, middle button. */
function cfFill(o){
  const m=$('#confirm'),a=document.activeElement,aid=m.contains(a)&&a.id;
  $('#cfTitle').textContent=o.title;$('#cfSub').textContent=o.sub||'';$('#cfText').innerHTML=o.html;
  const mi=$('#confirm .m-ico');mi.className='m-ico '+(o.tone||'bad');$('use',mi).setAttribute('href','#i-'+(o.icon||'trash'));
  // a waiting OK stays focusable (aria-disabled, clicks ignored), so Tab doesn't skip it onto "Keep them"
  const ok=$('#cfOk');ok.className='btn '+(o.okCls==null?'danger':o.okCls);ok.setAttribute('aria-disabled',o.wait?'true':'false');
  if(o.wait)ok.innerHTML=`<span class="spin"></span>${esc(o.ok)}`;else ok.textContent=o.ok;
  const alt=$('#cfAlt');alt.hidden=!o.alt;alt.textContent=o.alt||'';$('#cfNo').textContent=o.no||'Cancel';
  m.classList.toggle('perm',!!o.perm);m.classList.toggle('wide',!!o.alt);
  // keep focus where it was; if that control went away or is waiting, Cancel
  if(aid){const b=document.getElementById(aid);(b&&!b.disabled&&shown(b)?b:$('#cfNo')).focus({preventScroll:true});}
}
/* busy: all buttons wait, Esc and the scrim do nothing, the OK button shows what's happening */
function cfBusy(label){
  const m=$('#confirm'),on=!!label;m.classList.toggle('busy',on);m.setAttribute('aria-busy',on);
  $('#cfNo').disabled=on;$('#cfOk').disabled=on;$('#cfAlt').disabled=on;$$('#cfText button').forEach(b=>b.disabled=on);
  if(on)$('#cfOk').innerHTML=`<span class="spin"></span>${esc(label)}`;
}
/* v: true (OK), 'alt' (the middle button) or false; with keep, a yes resolves and the dialog stays for the caller */
function closeConfirm(v,force){
  const L=$('#confirmLayer');if(L.hidden||(!force&&$('#confirm').classList.contains('busy')))return;
  const r=cfResolve;cfResolve=null;
  if(v&&cfKeep&&!force){r&&r(v);return;}
  L.hidden=true;cfBusy(null);$('#modalLayer').inert=false;$('#app').inert=!$('#modalLayer').hidden;
  // focus goes back where it was; if the library re-rendered underneath (library-changed), to the same clip's new card
  if(shown(cfBack))cfBack.focus({preventScroll:true});
  else if(!v&&cfBack&&!cfBack.isConnected&&cfBack.closest&&cfBack.dataset&&(cfBack.dataset.clip||cfBack.classList.contains('sel-ck'))&&cur()==='library')focusLib(cfBack.dataset.clip||cfBack.dataset.sel);
  cfBack=null;r&&r(v);setTimeout(showUndo,0);
}
$('#confirmLayer').addEventListener('click',e=>{
  const rv=e.target.closest('[data-cfreview]');if(rv){closeConfirm(false);openReview();return;}   // "see which" from the out-of-view note
  const b=e.target.closest('[data-cf]');if(b&&!b.disabled&&b.getAttribute('aria-disabled')!=='true')closeConfirm(b.dataset.cf==='yes'?true:b.dataset.cf==='alt'?'alt':false);
});
/* Tab stays inside the dialog */
function cfTrap(e){
  const f=$$('#confirm button:not([disabled])').filter(shown),i=f.indexOf(document.activeElement);if(!f.length)return;
  if(i<0||(e.shiftKey&&i===0)||(!e.shiftKey&&i===f.length-1)){e.preventDefault();f[i<0?0:e.shiftKey?f.length-1:0].focus();}
}
async function copyText(s){
  try{await navigator.clipboard.writeText(s);return true;}
  catch(_){const ta=document.createElement('textarea');ta.value=s;ta.style.cssText='position:fixed;opacity:0';document.body.appendChild(ta);ta.select();let ok=false;try{ok=document.execCommand('copy');}catch(__){}ta.remove();return ok;}
}

/* ================================================================== LIBRARY */
const lib={filter:'all',q:'',sort:'new',group:'game',selView:null,prev:null,emptied:null};   // filter: all | fav | shared | <gameId> | sel (Review; prev = where Back goes)
const SORTS={new:['Newest first',(a,b)=>b.recordedAt-a.recordedAt],old:['Oldest first',(a,b)=>a.recordedAt-b.recordedAt],long:['Longest',(a,b)=>b.duration-a.duration],big:['Largest',(a,b)=>b.sizeBytes-a.sizeBytes]};
function renderSide(){
  // the items are rebuilt: a keyboard user on one stays on it
  const af=document.activeElement,fk=af&&af.closest&&af.closest('.side [data-f]')?af.dataset.f:null;
  const favN=S.clips.filter(c=>c.fav).length,shN=S.clips.filter(isShared).length,ready=S.loaded&&!S.loadError;
  const top=[['all','clips','All clips',S.clips.length],['fav','star','Favourites',favN],['shared','chat','Shared to Discord',shN]];
  $('#sideTop').innerHTML=top.map(([id,icn,l,n])=>`<button class="side-item${lib.filter===id?' on':''}" data-f="${id}">${ic(icn)}<span class="nm">${l}</span><span class="n">${ready?n:''}</span></button>`).join('');
  $('#sideGames').innerHTML=S.games.length?S.games.map(g=>`<button class="side-item${lib.filter===g.id?' on':''}" data-f="${esc(g.id)}" title="${esc(g.name)}">${gameIcon(g,'gi')}<span class="nm">${esc(g.name)}</span><span class="sel-n" data-seln="${esc(g.id)}"></span><span class="n">${g.count}</span></button>`).join('')
    :`<div class="side-empty">${ready?'No games yet':S.loaded?'':'Loading…'}</div>`;
  renderStorage();renderBufCard();markSide();
  const fb=fk&&$(`.side [data-f="${CSS.escape(fk)}"]`);if(fb)fb.focus({preventScroll:true});
}
function renderStorage(){
  const total=S.clips.reduce((s,c)=>s+c.sizeBytes,0),n=S.clips.length;
  $('#stTotal').innerHTML=S.loaded?`<b class="tnum">${fmtTotal(total)}</b> <span class="muted" style="font-size:12px">· ${n} clip${n===1?'':'s'}</span>`:'<span class="muted" style="font-size:12px">Scanning…</span>';
  const by=[...S.games].sort((a,b)=>b.bytes-a.bytes),top=by.slice(0,3),rest=by.slice(3);
  const cols=['var(--lane-game)','var(--lane-voice)','var(--lane-mic)'];
  const segs=top.map((g,i)=>[g.name,g.bytes,cols[i]]);
  if(rest.length)segs.push([`${rest.length} other game${rest.length>1?'s':''}`,rest.reduce((s,g)=>s+g.bytes,0),'var(--bg-5)']);
  $('#stBar').innerHTML=total?segs.map(s=>`<i style="width:${(s[1]/total*100).toFixed(2)}%;background:${s[2]}"></i>`).join(''):'<i style="width:100%;background:var(--bg-5)"></i>';
  $('#stLegend').innerHTML=segs.map(s=>`<span style="--c:${s[2]}" title="${esc(s[0])}">${esc(s[0])}</span><em>${fmtTotal(s[1])}</em>`).join('');
  const dirs=S.settings?S.settings.clipDirs:[];
  $('#stFoot').innerHTML=dirs.length?`${ic('folder','xs')}<span class="nm" title="${esc(dirs.join('\n'))}">${esc(shortPath(dirs[0],30))}${dirs.length>1?` +${dirs.length-1}`:''}</span>`:`${ic('folder','xs')}<span>No clip folder set</span>`;
}
function cardHTML(c,opt={}){
  const fresh=isNewClip(c),d=new Date(c.recordedAt);
  const title=opt.showGame?c.game:whenLabel(d);
  const meta=opt.showGame?`${whenLabel(d)} · ${fmtMB(MBof(c.sizeBytes))}`:`${fmtMB(MBof(c.sizeBytes))} · ${c.height}p${fpsR(c)}`;
  return `<article class="card${fresh?' new':''}${sel.ids.has(c.id)?' is-sel':''}" data-clip="${esc(c.id)}" tabindex="0" aria-label="${esc(cardLbl(c))}">
    <div class="thumb">${thumbHTML(c.thumb)}${fresh?'<span class="badge-new">New</span>':''}<span class="dur">${fmtLen(c.duration)}</span>
      <div class="thumb-hover"><span class="play-c">${ic('play')}</span></div>${selCk(c)}
      <button class="quick" data-quick="${esc(c.id)}" data-tip="Export for Discord" aria-label="Export for Discord">${ic('upload','sm')}</button></div>
    <div class="card-body"><div class="card-text"><div class="card-title" title="${esc(title)}">${esc(title)}</div><div class="card-meta">${meta}</div></div>
      <div class="flags">${flagsHTML(c)}</div></div>
  </article>`;
}
const flagsHTML=c=>`${isShared(c)?`<span title="Shared to Discord">${ic('chat','xs')}</span>`:''}${c.fav?`<span class="fav" title="Favourite">${ic('star-f','xs')}</span>`:''}`;
function refreshClipUI(c){
  $$(`.lib [data-clip="${CSS.escape(c.id)}"] .flags`).forEach(f=>f.innerHTML=flagsHTML(c));
  renderSide();
}
function moreTile(g,shown){
  const left=g.count-shown,c=g.clips[shown]||g.clips[0];
  return `<article class="card more-tile" data-more="${esc(g.id)}" tabindex="0"><div class="thumb">${thumbHTML(c.thumb)}<div class="more-in"><b>+${left}</b><span>more ${esc(g.name)} clips</span><em class="more-sel"></em></div></div>
    <div class="card-body"><div class="card-text"><div class="card-title">View all ${g.count}</div><div class="card-meta">${fmtTotal(g.bytes)} in total</div></div></div></article>`;
}
/* Quick send exports a planned part of the clip (UX review r2 #3): inside the saved In/Out if the clip was trimmed,
   else the whole clip; markers first, then the ending. It never changes the clip's saved edits. */
function quickPlan(c){
  const e=c.edits;let a=0,b=c.duration;
  if(e&&e.outT-e.inT>=1&&e.outT-e.inT<c.duration-.05){a=clamp(e.inT,0,c.duration);b=clamp(e.outT,a,c.duration);}
  return planTrim(S.settings.quickLen,{a,b,t:null,markers:(e&&e.markers)||[]});
}
function heroHTML(){
  const c=S.clips[0];if(!c)return '';
  const fresh=isNewClip(c),d=new Date(c.recordedAt),n=c.audioTracks.length;
  const qp=quickPlan(c),qe=estimate(S.settings.defaultPreset,{clip:c,dur:qp.len,vert:false});
  const range=`${fmtLen(qp.inT)}–${fmtLen(qp.outT)}`;
  const sess=S.clips.slice(1,3);
  return `<section class="hero">
    <div class="hero-thumb${sel.ids.has(c.id)?' is-sel':''}" data-clip="${esc(c.id)}">${thumbHTML(c.thumb,{eager:true})}${fresh?'<span class="badge-new">New</span>':''}<span class="dur">${fmtLen(c.duration)}</span><div class="thumb-hover"><span class="play-c">${ic('play')}</span></div>${selCk(c)}</div>
    <div class="hero-info">
      <div class="eyebrow" id="heroEyebrow" data-ts="${c.recordedAt}" data-fresh="${fresh?1:0}"><span class="dot"></span>${fresh?'Just saved':'Newest clip'} · ${ago(c.recordedAt)}</div>
      <h2>${esc(c.game)} — ${whenLabel(d)}</h2>
      <div class="hero-meta"><span>${ic('clock','xs')}${fmtLen(c.duration)}</span><span>${ic('video','xs')}${qualLbl(c)}</span><span>${ic('drive','xs')}${fmtMB(MBof(c.sizeBytes))}</span><span>${ic('wave','xs')}${n} audio track${n===1?'':'s'}</span></div>
      <div class="hero-actions"><button class="btn primary lg" data-open="${esc(c.id)}">${ic('scissors')}Trim &amp; export</button><button class="btn lg two" data-send="${esc(c.id)}" data-tip="Exports ${range} (${planWhy(qp)}) for Discord right away" data-tip-pos="b">${ic('bolt')}<span class="tl"><b>Quick send · ${fmtSecs(qp.len)}</b><small>${range} · ${fmtMB(qe.size)} · ${tierLbl(qe.res,qe.fps)}</small></span></button><button class="btn lg sq" data-folder="${esc(c.id)}" data-tip="Show in folder" data-tip-pos="b" aria-label="Show in folder">${ic('folder')}</button></div>
    </div>
    <div class="hero-session">${sess.length?`<div class="cap" style="color:var(--text-3)">Recent</div>`+sess.map(s=>{
      const sub=[s.gameId!==c.gameId?esc(s.game):'',fmtMB(MBof(s.sizeBytes)),isShared(s)?'shared':''].filter(Boolean).join(' · ');
      return `<div class="mini${sel.ids.has(s.id)?' is-sel':''}" data-clip="${esc(s.id)}"><div class="mt">${thumbHTML(s.thumb,{eager:true})}<span class="dur" style="right:4px;bottom:4px;font-size:10.5px;padding:2px 4px">${fmtLen(s.duration)}</span></div><div style="min-width:0"><b>${whenLabel(new Date(s.recordedAt))}</b><span>${sub}</span></div></div>`;}).join(''):''}
    </div></section>`;
}
function ffBanner(){
  if(!S.settings||S.settings.ffmpegPath)return '';
  return `<div class="banner">${ic('alert','lg')}<div class="bt"><b>FFmpeg not found</b>Exports, waveforms and the audio preview need it. Install it with <code>winget install Gyan.FFmpeg.Essentials</code>, then restart Backtrack.</div><button class="btn sm" data-go="settings">Settings</button></div>`;
}
/* phase 2: the recorder's folder is always a clip folder, so "empty" means nothing recorded and nothing found elsewhere */
function emptyHTML(){
  const r=recCfg(),st=REC.status,hk=r&&r.hotkeys.save,len=r?fmtBuf(r.bufferSecs):'';
  const others=S.settings.clipDirs.filter(d=>!(r&&sameDir(d,r.clipsDir)));
  const how=!r?'':st&&st.state==='on'?(hk?`Press ${kbdHTML(hk)} in a game to save the last ${len}.`:`Use <b>Save now</b> on the left to save the last ${len}.`)
    :`Turn on the replay buffer on the left, then press ${hk?kbdHTML(hk):'Save now'} in a game to save the last ${len}.`;
  const more=others.length?`Backtrack also looked in <code>${esc(others.join(', '))}</code>. If that folder moved, choose its new location in Settings.`
    :'Already have clips from SteelSeries GG? Add your <code>Videos\\SteelSeries Moments</code> folder in Settings.';
  return `<div class="empty"><div class="empty-ico">${ic('clips')}</div><b>No clips yet</b><p>${how} ${more}</p><button class="btn" data-go="settings-folders">${ic('folder','sm')}Add a clip folder</button></div>`;
}
// right after a move took the last clip: say so (and offer Undo) instead of the first-run "No clips yet"
const emptiedHTML=m=>`<div class="empty"><div class="empty-ico">${ic('trash')}</div><b>${m.moved?'You moved all your clips to the Recycle Bin':'You deleted all your clips'}</b><p>${m.moved?`${m.deleted?`${nClips(m.deleted)} of them were deleted permanently. `:''}Undo brings ${m.deleted?'the others':'them'} back, or you can restore them from the Windows Recycle Bin.`:'New clips you save will show up here.'}</p>${m.moved&&lastMove?`<button class="btn" id="emptyUndo">${ic('undo','sm')}Undo</button>`:''}</div>`;
const loadingHTML=()=>`<div class="empty"><div class="spin"></div><b>Loading your clips…</b><p>The first scan reads every clip once, so it can take a few seconds.</p></div>`;
const noneHTML=(t,s)=>`<div class="empty"><b>${t}</b>${s}</div>`;

/* hundreds of clips: render the grid in batches as the user scrolls (cards keep their hover glow and tooltips).
   Driven by the scroll position rather than IntersectionObserver, which stalls when frames are throttled. */
let feedMore=null,feedTimer=0;
function stopFeed(){feedMore=null;clearInterval(feedTimer);}
function feed(root,items,o={}){
  stopFeed();let i=0,grid=null;const sc=$('#libScroll'),batch=o.batch||48;
  const sentinel=document.createElement('div');sentinel.className='lib-more';root.appendChild(sentinel);
  function more(){
    let n=0;
    while(i<items.length&&n<batch){const it=items[i++];
      if(it.card==null){const s=document.createElement(it.group!=null?'section':'div');
        if(it.group!=null){s.className='grp';s.innerHTML=it.group+'<div class="grid"></div>';grid=s.lastElementChild;}else{s.className='grid';grid=s;}
        root.insertBefore(s,sentinel);}
      else{grid.insertAdjacentHTML('beforeend',it.card);n++;}}
    if(sel.ids.size||sel.failed.size)markSel(root);   // the card strings were built before the latest selection change
    if(i>=items.length){sentinel.remove();if(feedMore===pump)feedMore=null;if(o.foot)root.insertAdjacentHTML('beforeend',`<div class="lib-count">${o.foot}</div>`);}
  }
  const near=()=>sentinel.isConnected&&sentinel.getBoundingClientRect().top<sc.getBoundingClientRect().bottom+900;
  function pump(){let guard=0;while(feedMore===pump&&i<items.length&&near()&&guard++<40)more();}
  feedMore=pump;more();
  while(o.fill&&i<items.length&&sc.scrollHeight<o.fill)more();
  if(i<items.length){
    // scroll events drive it; a slow timer is the fallback for when scroll events arrive late (throttled frames)
    feedTimer=setInterval(()=>{if(feedMore===pump)pump();else clearInterval(feedTimer);},300);
  } else if(feedMore===pump)feedMore=null;
}
$('#libScroll').addEventListener('scroll',()=>{if(feedMore)feedMore();},{passive:true});
window.addEventListener('resize',()=>{if(feedMore)feedMore();});
function bucketOf(ts){
  const d=new Date(ts),n=dayDiff(d),now=new Date();
  if(n<=0)return [0,'Today'];if(n===1)return [1,'Yesterday'];if(n<7)return [2,'Earlier this week'];
  if(d.getMonth()===now.getMonth()&&d.getFullYear()===now.getFullYear())return [3,'Earlier this month'];
  const back=(now.getFullYear()-d.getFullYear())*12+now.getMonth()-d.getMonth();
  return [3+back,MONTH[d.getMonth()]+(d.getFullYear()!==now.getFullYear()?' '+d.getFullYear():'')];
}
function renderLibrary(keepScroll){
  for(const id of sel.ids)if(!S.byId[id]){sel.ids.delete(id);sel.failed.delete(id);sel.bulk.delete(id);}   // gone (trashed, moved, folder removed)
  for(const id of sel.pending)if(S.byId[id]){sel.ids.add(id);sel.pending.delete(id);}   // back from the Recycle Bin (Undo)
  if(S.clips.length)lib.emptied=null;
  // a game view whose last clip just went (or a review of a selection that's now empty): back to All clips, from the top
  if(S.loaded&&(lib.filter==='sel'?!selOn():!['all','fav','shared'].includes(lib.filter)&&!S.G[lib.filter])){
    // an emptied Review goes back to where its "‹" pointed when that view still exists
    const p=lib.filter==='sel'&&lib.prev,ok=p&&(['all','fav','shared'].includes(p.filter)||S.G[p.filter]);
    lib.filter=ok?p.filter:'all';lib.q=ok?p.q:'';$('#search').value=lib.q;if(lib.filter!=='sel')lib.prev=null;keepScroll=false;$('#libScroll').scrollTop=0;}
  const ae=document.activeElement,aeLib=!!ae&&ae!==document.body&&$('.lib').contains(ae);
  const fcard=keepScroll&&ae&&ae.closest&&ae.closest('#libScroll .card[data-clip]'),focusId=fcard&&fcard.dataset.clip;
  renderSide();
  const tEl=$('#libTitle'),sc=$('#libScroll'),st=keepScroll?sc.scrollTop:0;stopFeed();grpMap=new Map();
  // keep what you were looking at in place, not just the pixel offset: the first card at the top that's still here after the change
  let anchors=null;
  if(keepScroll){const top=sc.getBoundingClientRect().top;anchors=[];for(const el of $$('#libScroll .card[data-clip]')){const b=el.getBoundingClientRect();if(b.bottom<=top+4)continue;anchors.push([el.dataset.clip,b.top-top]);if(anchors.length>=40)break;}}
  $('#sortLbl').textContent=SORTS[lib.sort][0];
  $$('#groupSeg button').forEach(b=>b.classList.toggle('on',b.dataset.g===lib.group));
  if(!S.loaded||!S.settings){tEl.innerHTML=`<div><h1>All clips</h1><div class="sub">Scanning your clip folders…</div></div>`;sc.innerHTML=loadingHTML();syncSel();return;}
  if(S.loadError){tEl.innerHTML=`<div><h1>All clips</h1><div class="sub">Couldn't load clips</div></div>`;
    sc.innerHTML=`<div class="empty"><div class="empty-ico">${ic('alert')}</div><b>Couldn't load your clips</b><p>${esc(S.loadError)}</p><button class="btn" id="retryLoad">${ic('reset','sm')}Try again</button></div>`;syncSel();return;}
  const banner=ffBanner();
  if(!S.clips.length){tEl.innerHTML=`<div><h1>All clips</h1><div class="sub">No clips${lib.emptied?'':' yet'}</div></div>`;sc.innerHTML=banner+(lib.emptied?emptiedHTML(lib.emptied):emptyHTML());syncSel();
    if(aeLib&&!ae.isConnected)focusLib();return;}
  const q=lib.q.trim().toLowerCase(),sorter=SORTS[lib.sort][1],fill=st+sc.clientHeight+200;
  if(q){
    const res=S.clips.filter(c=>matchQ(c,q)).sort(sorter);
    tEl.innerHTML=`<div><h1>Results for “${esc(lib.q)}”</h1><div class="sub">${res.length} clip${res.length===1?'':'s'}</div></div>`;
    sc.innerHTML=banner;
    if(res.length)feed(sc,[{grid:true},...res.map(c=>({card:cardHTML(c,{showGame:true})}))],{fill});
    else sc.insertAdjacentHTML('beforeend',noneHTML('No clips match',`Try a game name like “${esc(S.games[0].name)}” or a day like “yesterday”.`));
  } else if(lib.filter==='sel'){   // "Review": exactly what's selected, wherever it was picked (deselected ones stay until you leave)
    const list=viewClips().sort(sorter),games=new Set(list.map(c=>c.gameId)).size;
    const p=lib.prev||{filter:'all',q:''};
    const back=viewLbl(p.filter,p.q);
    tEl.innerHTML=`<button class="btn ghost rv-back" data-rvback data-tip="Back to ${esc(back)}, keeping the selection · Esc" data-tip-pos="b" aria-label="Back to ${esc(back)}">${ic('chev-l')}<span>${esc(back)}</span></button><span class="sep-v"></span><div><h1>Selected clips</h1><div class="sub">${nClips(list.length)} · ${fmtTotal(selBytes(list))} · ${games} game${games===1?'':'s'}</div></div>`;
    sc.innerHTML=banner;feed(sc,[{grid:true},...list.map(c=>({card:cardHTML(c,{showGame:true})}))],{fill});
  } else if(S.G[lib.filter]){
    const g=S.G[lib.filter],list=[...g.clips].sort(sorter);
    tEl.innerHTML=`${gameIcon(g)}<div><h1>${esc(g.name)}</h1><div class="sub">${g.count} clip${g.count===1?'':'s'} · ${fmtTotal(g.bytes)}</div></div>`;
    sc.innerHTML=banner;feed(sc,[{grid:true},...list.map(c=>({card:cardHTML(c)}))],{fill,foot:list.length>12?`All ${list.length} ${esc(g.name)} clips`:''});
  } else if(lib.filter==='fav'||lib.filter==='shared'){
    const fav=lib.filter==='fav',list=S.clips.filter(fav?c=>c.fav:isShared).sort(sorter);
    tEl.innerHTML=`<div><h1>${fav?'Favourites':'Shared to Discord'}</h1><div class="sub">${list.length} clip${list.length===1?'':'s'}</div></div>`;
    sc.innerHTML=banner;
    if(list.length)feed(sc,[{grid:true},...list.map(c=>({card:cardHTML(c,{showGame:true})}))],{fill});
    else sc.insertAdjacentHTML('beforeend',fav?noneHTML('No favourites yet','Star a clip in the editor, or right-click one here.'):noneHTML('Nothing shared yet','Clips you export for Discord show up here.'));
  } else {
    const total=S.clips.reduce((s,c)=>s+c.sizeBytes,0);
    tEl.innerHTML=`<div><h1>All clips</h1><div class="sub">${S.clips.length} clip${S.clips.length===1?'':'s'} · ${fmtTotal(total)} · ${S.games.length} game${S.games.length===1?'':'s'}</div></div>`;
    sc.innerHTML=banner+heroHTML();
    if(lib.group==='game'){
      const order=[...S.games].sort((a,b)=>sorter([...a.clips].sort(sorter)[0],[...b.clips].sort(sorter)[0]));
      sc.insertAdjacentHTML('beforeend',order.map(g=>{const list=[...g.clips].sort(sorter).slice(0,3);grpMap.set('g:'+g.id,g.clips.map(c=>c.id));
        return `<section class="grp"><div class="grp-head">${gameIcon(g)}<h2>${esc(g.name)}</h2><span class="meta">${g.count} clip${g.count===1?'':'s'} · ${fmtTotal(g.bytes)}<span class="grp-n"></span></span>${grpSelHTML('g:'+g.id,g.count,g.name)}<button class="link" data-f="${esc(g.id)}">View all${ic('chev-r','xs')}</button></div>
          <div class="grid">${list.map(c=>cardHTML(c)).join('')}${g.count>3?moreTile(g,3):''}</div></section>`;}).join(''));
    } else {
      const groups=new Map();
      for(const c of S.clips){const [rank,label]=bucketOf(c.recordedAt);let b=groups.get(label);if(!b){b={rank,label,cs:[]};groups.set(label,b);}b.cs.push(c);}
      const items=[];
      [...groups.values()].sort((a,b)=>lib.sort==='old'?b.rank-a.rank:a.rank-b.rank).forEach(b=>{grpMap.set('d:'+b.label,b.cs.map(c=>c.id));
        items.push({group:`<div class="grp-head"><h2>${b.label}</h2><span class="meta">${b.cs.length} clip${b.cs.length>1?'s':''}<span class="grp-n"></span></span>${grpSelHTML('d:'+b.label,b.cs.length,b.label)}</div>`});
        b.cs.sort(sorter).forEach(c=>items.push({card:cardHTML(c,{showGame:true})}));});
      feed(sc,items,{fill});
    }
  }
  if(keepScroll){
    sc.scrollTop=st;
    for(const [id,off] of anchors||[]){const el=$(`#libScroll .card[data-clip="${CSS.escape(id)}"]`);if(el){sc.scrollTop+=el.getBoundingClientRect().top-sc.getBoundingClientRect().top-off;break;}}
  }
  syncSel();
  // a re-render (library-changed, a move, a view change) keeps keyboard focus on the same clip when it's still here,
  // and never lets it fall to <body> when it was somewhere in the library (a "View all" link, a card that went)
  const fc=focusId&&$(`#libScroll .card[data-clip="${CSS.escape(focusId)}"]`);if(fc)fc.focus({preventScroll:true});
  else if(aeLib&&!ae.isConnected&&(document.activeElement===document.body||!document.activeElement))focusLib();
}
// keep "Just saved · 3 min ago" honest while the window stays open
setInterval(()=>{const e=$('#heroEyebrow');if(!e||cur()!=='library')return;const ts=+e.dataset.ts;e.lastChild.textContent=`${e.dataset.fresh==='1'?'Just saved':'Newest clip'} · ${ago(ts)}`;},30000);

document.addEventListener('click',e=>{
  const gt=e.target.closest('[data-go]');if(gt){go(gt.dataset.go);return;}
  if(!e.target.closest('.lib'))return;
  if(e.target.closest('#retryLoad')){loadAll();return;}
  if(e.target.closest('#emptyUndo')){restoreMoved(lastMove);return;}
  const ck=e.target.closest('[data-sel]');if(ck){selClick(ck.dataset.sel,e);return;}
  const gs=e.target.closest('[data-grpsel]');if(gs){const ids=grpMap.get(gs.dataset.grpsel)||[];setSel(ids,!ids.every(id=>sel.ids.has(id)),true);return;}
  if(e.target.closest('[data-rvback]')){leaveReview();return;}
  const f=e.target.closest('[data-f]');if(f){lib.filter=f.dataset.f;lib.q='';$('#search').value='';$('#libScroll').scrollTop=0;renderLibrary();return;}
  const q=e.target.closest('[data-quick]');if(q){e.stopPropagation();const c=S.byId[q.dataset.quick];if(c){openClip(c);go('export');}return;}
  const mo=e.target.closest('[data-more]');if(mo){lib.filter=mo.dataset.more;$('#libScroll').scrollTop=0;renderLibrary();return;}
  const sd=e.target.closest('[data-send]');if(sd){quickSend(S.byId[sd.dataset.send]);return;}
  const op=e.target.closest('[data-open]');if(op){const c=S.byId[op.dataset.open];if(c){openClip(c);go('editor');}return;}
  const fo=e.target.closest('[data-folder]');if(fo){const c=S.byId[fo.dataset.folder];if(c)BT.reveal(c.path).catch(err=>fail("Couldn't open the folder",err));return;}
  // while selecting, a click on any clip (card, hero, recent) toggles it; Ctrl/Shift+click starts selecting.
  // A double-click opens it in both modes: its second click puts back what the first one toggled.
  const cl=e.target.closest('[data-clip]');if(cl){const c=S.byId[cl.dataset.clip];if(!c)return;
    if(e.detail===2&&dblSel&&dblSel.id===c.id&&Date.now()-dblSel.t<700){setSel([c.id],dblSel.was);dblSel=null;openClip(c);go('editor');return;}
    if(selOn()||e.ctrlKey||e.metaKey||e.shiftKey){dblSel=e.shiftKey||e.ctrlKey||e.metaKey?null:{id:c.id,was:sel.ids.has(c.id),t:Date.now()};selClick(c.id,e);return;}
    openClip(c);go('editor');return;}
});
let dblSel=null;
document.addEventListener('contextmenu',e=>{
  if(e.target.closest('input,textarea'))return;
  const card=e.target.closest('.lib [data-clip]');
  if(!card){if(!BT.isMock)e.preventDefault();return;}  // no browser menu in the app
  e.preventDefault();const c=S.byId[card.dataset.clip];if(!c)return;
  // the Menu key / Shift+F10 opens it at the focused card instead of the window corner
  let at={x:e.clientX,y:e.clientY};if(!at.x&&!at.y){const r=card.getBoundingClientRect();at={x:r.left+16,y:r.top+16};}
  if(card.classList.contains('card'))card.classList.add('menu-for');   // which card the menu belongs to
  if(sel.ids.has(c.id)){   // a selected clip: this clip first (check it, find it), then what acts on the whole selection
    const n=sel.ids.size,cs=selClips(),allFav=cs.every(x=>x.fav);
    openPop(null,[
      {k:'open',icon:'play',label:'Open in editor',right:'Enter'},
      {k:'folder',icon:'folder',label:'Show in folder'},
      {k:'desel',icon:'box',label:'Deselect this clip',right:'Space'},
      {sep:true},
      {k:'fav',icon:allFav?'star-f':'star',label:allFav?`Remove ${n===1?'it':'all '+n} from Favourites`:`Add ${n===1?'it':'all '+n} to Favourites`},
      {k:'clear',icon:'x',label:'Clear selection',right:'Esc'},
      {k:'trash',icon:'trash',label:n===1?'Move to Recycle Bin…':`Move ${n} clips to Recycle Bin…`,right:'Del'},
    ],it=>{
      if(it.k==='open'){openClip(c);go('editor');}
      else if(it.k==='folder')BT.reveal(c.path).catch(err=>fail("Couldn't open the folder",err));
      else if(it.k==='fav')favSelected();else if(it.k==='desel')setSel([c.id],false);else if(it.k==='clear')clearSel();else if(it.k==='trash')trashSelected();
    },'left',at,{head:`<div><b>${n} selected</b>${fmtTotal(selBytes(cs))}</div>`});
    return;
  }
  const qp=quickPlan(c);
  openPop(null,[
    ...(selOn()?[{k:'add',icon:'check',label:'Add to selection',right:'Ctrl+click'},{sep:true}]:[]),
    {k:'open',icon:'scissors',label:'Trim & export'},
    {k:'send',icon:'bolt',label:`Quick send · ${fmtSecs(qp.len)}`,sub:`${fmtLen(qp.inT)}–${fmtLen(qp.outT)} · ${planWhy(qp)}`},
    {k:'opts',icon:'upload',label:'Export options…'},
    {k:'fav',icon:c.fav?'star-f':'star',label:c.fav?'Remove from Favourites':'Add to Favourites'},
    {k:'folder',icon:'folder',label:'Show in folder'},
    {sep:true},
    ...(selOn()?[]:[{k:'select',icon:'check',label:'Select',right:'Ctrl+click'}]),
    {k:'trash',icon:'trash',label:'Move to Recycle Bin…',right:selOn()?'':'Del'},
  ],it=>{
    if(it.k==='open'){openClip(c);go('editor');}
    else if(it.k==='send')quickSend(c);
    else if(it.k==='opts'){openClip(c);go('export');}
    else if(it.k==='fav')toggleFav(c);
    else if(it.k==='folder')BT.reveal(c.path).catch(err=>fail("Couldn't open the folder",err));
    else if(it.k==='select'||it.k==='add')selClick(c.id);
    else if(it.k==='trash')trashClip(c);
  },'left',at);
});
$('#search').addEventListener('input',e=>{lib.q=e.target.value;renderLibrary();});
$('#groupSeg').addEventListener('click',e=>{const b=e.target.closest('button');if(!b)return;lib.group=b.dataset.g;if(lib.filter!=='all')lib.filter='all';renderLibrary();});
$('#sortBtn').addEventListener('click',e=>openPop(e.currentTarget,Object.entries(SORTS).map(([k,v])=>({k,label:v[0],on:lib.sort===k})),it=>{lib.sort=it.k;renderLibrary();},'right'));

async function toggleFav(c){
  if(!c)return;const v=!c.fav;c.fav=v;favLast=null;if(ed.clip===c)syncFav();refreshClipUI(c);
  try{await BT.setFavourite(c.id,v);toast(v?'Added to Favourites':'Removed from Favourites','star');}
  catch(e){c.fav=!v;if(ed.clip===c)syncFav();refreshClipUI(c);fail("Couldn't update Favourites",e);}
}
/* ---------- multi-select. The selection is a set of clip ids: it outlives re-renders and view changes (search, filter,
   sort, group). The bar always counts everything selected and says when some of it isn't in the current view, the
   sidebar shows how many per game, and the confirm lists them by game, so nothing gets deleted without being shown. */
const sel={ids:new Set(),anchor:null,failed:new Map(),base:null,ranged:false,   // failed: clipId → why the last move failed
  bulk:new Set(),      // picked by a Select all / group / range / Ctrl+A rather than one by one ("Keep favourites" default)
  pending:new Set(),pendingT:0,say:''};   // pending: restored by Undo, selected again once library-changed lists them
const selOn=()=>sel.ids.size>0;
let grpMap=new Map();   // 'g:<gameId>' | 'd:<day label>' → every clip id in that group (By game shows only 3 of them)
let lastMove=null;      // {ids, reselect}: what the last move sent to the Recycle Bin, for Undo
const matchQ=(c,q)=>{const d=new Date(c.recordedAt);return (c.game+' '+whenLabel(d)+' '+longDate(d)+' '+c.fileName).toLowerCase().includes(q);};
const nClips=n=>`${n} clip${n===1?'':'s'}`;
const clipLbl=c=>`${c.game} — ${whenLabel(new Date(c.recordedAt))}`;
// what a screen reader says on a focused card
const skippedWhy=w=>String(w||'').startsWith('Skipped');   // left out on purpose (Move N, skip D:), not a failure
const cardLbl=c=>`${c.game}, ${whenLabel(new Date(c.recordedAt))}${sel.ids.has(c.id)?', selected':''}${sel.failed.has(c.id)?`, ${skippedWhy(sel.failed.get(c.id))?'':'couldn’t move: '}${sel.failed.get(c.id)}`:''}`;
const selClips=()=>[...sel.ids].map(id=>S.byId[id]).filter(Boolean);
const selBytes=cs=>cs.reduce((s,c)=>s+c.sizeBytes,0);
const selCk=c=>`<button class="sel-ck" data-sel="${esc(c.id)}" role="checkbox" aria-checked="${sel.ids.has(c.id)}" aria-label="Select ${esc(clipLbl(c))}" tabindex="-1" data-tip="Select · Ctrl+click"><i class="cbx"></i></button>`;
const grpSelHTML=(k,n,name)=>`<button class="link grp-sel" data-grpsel="${esc(k)}" role="checkbox" aria-checked="false" aria-label="Select all ${n} ${esc(name)} clips"><i class="cbx"></i>Select all</button>`;
/* every clip the current view stands for, not just the cards rendered so far (By game: also the ones behind "+N more") */
function viewClips(){
  const q=lib.q.trim().toLowerCase();
  if(q)return S.clips.filter(c=>matchQ(c,q));
  if(lib.filter==='sel')return (lib.selView||[]).map(id=>S.byId[id]).filter(Boolean);
  if(S.G[lib.filter])return S.G[lib.filter].clips;
  if(lib.filter==='fav'||lib.filter==='shared')return S.clips.filter(lib.filter==='fav'?c=>c.fav:isShared);
  return S.clips;
}
const viewLbl=(f,q)=>q?`Results for “${q}”`:f==='fav'?'Favourites':f==='shared'?'Shared to Discord':S.G[f]?S.G[f].name:'All clips';
function setSel(ids,on,bulk){
  for(const id of ids){if(!S.byId[id])continue;if(on){sel.ids.add(id);if(bulk)sel.bulk.add(id);else sel.bulk.delete(id);}else{sel.ids.delete(id);sel.failed.delete(id);sel.bulk.delete(id);}}
  if(!sel.ids.size)sel.anchor=null;sel.base=null;favLast=null;syncSel();
}
function clearSel(){sel.ids.clear();sel.failed.clear();sel.bulk.clear();sel.anchor=null;sel.base=null;favLast=null;if(lib.filter==='sel')renderLibrary();else syncSel();}   // leaving Review too
const selectView=on=>setSel(viewClips().map(c=>c.id),on,true);
/* the cards from a to b as they appear on screen: By game only has the 3 shown per game, never the ones behind "+N more" */
function rangeIds(a,b){const ids=[...new Set($$('#libScroll .card[data-clip]').map(x=>x.dataset.clip))],i=ids.indexOf(a),j=ids.indexOf(b);return i<0||j<0?null:ids.slice(Math.min(i,j),Math.max(i,j)+1);}
/* arrow keys move between the rendered cards (and "+N more" tiles) in reading order; Up/Down keep the column */
function cardNav(card,key){
  const all=$$('#libScroll .card'),i=all.indexOf(card);
  if(key==='ArrowLeft')return all[i-1]||null;if(key==='ArrowRight')return all[i+1]||null;
  if(key==='Home')return all[0]||null;if(key==='End')return all[all.length-1]||null;
  const r=card.getBoundingClientRect(),cx=r.left+r.width/2,dn=key==='ArrowDown',R=new Map(all.map(x=>[x,x.getBoundingClientRect()]));
  const cand=all.filter(x=>dn?R.get(x).top>r.bottom-2:R.get(x).bottom<r.top+2);if(!cand.length)return null;
  const top=(dn?Math.min:Math.max)(...cand.map(x=>R.get(x).top)),row=cand.filter(x=>Math.abs(R.get(x).top-top)<4);
  return row.reduce((b,x)=>{const q=R.get(x),d=Math.abs(q.left+q.width/2-cx);return !b||d<b.d?{x,d}:b;},null).x;
}
/* keyboard focus never falls to <body>: the clip if it's still here, else the first card, else the grid itself */
function focusLib(id){
  const el=(id&&$(`#libScroll .card[data-clip="${CSS.escape(id)}"]`))||$('#libScroll .card')||$('#libScroll');
  el.focus({preventScroll:true});if(el.classList.contains('card'))el.scrollIntoView({block:'nearest'});
}
/* click / Ctrl+click toggles; Shift+click gives the whole range the state the clicked clip takes (so it can also deselect) */
function selClick(id,e){
  const c=S.byId[id];if(!c)return;
  if(e&&e.shiftKey&&sel.anchor&&sel.anchor!==id){const r=rangeIds(sel.anchor,id);if(r){sel.ranged=true;setSel(r,!sel.ids.has(id),true);sel.anchor=id;return;}}
  const on=!sel.ids.has(id);sel.anchor=id;sel.say=`${on?'Selected':'Deselected'} ${clipLbl(c)}`;setSel([id],on);
}
/* paint the selection onto whatever is rendered (cards, hero, recent, group heads, "+N more" tiles) */
function markSel(root){
  $$('[data-clip]',root).forEach(x=>{
    const id=x.dataset.clip,on=sel.ids.has(id),why=sel.failed.get(id);x.classList.toggle('is-sel',on);
    if(x.classList.contains('card')&&S.byId[id])x.setAttribute('aria-label',cardLbl(S.byId[id]));
    const b=$('.sel-ck',x);if(b&&b.getAttribute('aria-checked')!==String(on))b.setAttribute('aria-checked',on);
    const t=$('.thumb',x)||(x.classList.contains('hero-thumb')?x:null),fb=t&&$('.fail-b',t);
    if(why&&t&&!fb)t.insertAdjacentHTML('beforeend',`<span class="fail-b" data-tip="${esc(why)}" aria-hidden="true">${ic('alert','xs')}${skippedWhy(why)?'Skipped':'Couldn’t move'}</span>`);
    else if(!why&&fb)fb.remove();
  });
  $$('[data-grpsel]',root).forEach(b=>{
    const ids=grpMap.get(b.dataset.grpsel)||[],n=ids.reduce((s,id)=>s+sel.ids.has(id),0);
    b.setAttribute('aria-checked',n&&n===ids.length?'true':n?'mixed':'false');
    const m=$('.grp-n',b.parentElement);if(m)m.textContent=n?`${n} selected`:'';
  });
  $$('[data-more]',root).forEach(t=>{
    const g=S.G[t.dataset.more],seen=new Set($$('.card[data-clip]',t.parentElement).map(x=>x.dataset.clip)),m=$('.more-sel',t);
    const n=g?g.clips.reduce((s,c)=>s+(sel.ids.has(c.id)&&!seen.has(c.id)),0):0;if(m)m.textContent=n?`${n} selected`:'';
  });
}
function markSide(){
  const per=new Map();for(const c of selClips())per.set(c.gameId,(per.get(c.gameId)||0)+1);
  $$('#sideGames [data-seln]').forEach(s=>{const n=per.get(s.dataset.seln)||0,g=S.G[s.dataset.seln],all=g&&n===g.count;s.textContent=n?(all?'All':n):'';s.title=n?(all?'All selected':`${n} selected`):'';});
}
let liveT=0,liveN=0;
function syncSel(){
  const on=selOn(),bar=$('#selBar');
  $('.lib').classList.toggle('selecting',on);markSel($('#libScroll'));markSide();
  bar.classList.toggle('show',on&&S.loaded);bar.inert=!on;
  if(lib.filter==='sel'){   // Review's header follows the selection as you untick things
    const sv=viewClips(),still=sv.filter(c=>sel.ids.has(c.id)),s=$('#libTitle .sub');
    if(s)s.textContent=still.length===sv.length?`${nClips(sv.length)} · ${fmtTotal(selBytes(sv))} · ${new Set(sv.map(c=>c.gameId)).size} game${new Set(sv.map(c=>c.gameId)).size===1?'':'s'}`:`${still.length} of ${sv.length} still selected · ${fmtTotal(selBytes(still))}`;
  }
  if(on){
    const cs=selClips(),n=cs.length,view=viewClips(),vs=new Set(view.map(c=>c.id)),inV=cs.filter(c=>vs.has(c.id)).length,out=n-inV;
    const nf=cs.filter(c=>sel.failed.has(c.id)).length,failedOnly=nf&&nf===n,size=fmtTotal(selBytes(cs));
    $('#selN').textContent=`${n} selected`;
    const parts=failedOnly?[`<span class="warn">${cs.every(c=>skippedWhy(sel.failed.get(c.id)))?'Skipped: no Recycle Bin':'Couldn’t be moved'}</span>`,size]:[size,out?`<span class="warn">${out} not in this view</span>`:'',n===1&&!sel.ranged?'Shift+click selects a range':''];
    const sub=$('#selSub');sub.innerHTML=parts.filter(Boolean).join(' · ');
    const rv=$('#selShow'),inRv=lib.filter==='sel';rv.classList.toggle('on',inRv);
    rv.setAttribute('aria-label',`${inRv?'Showing':'Review'} ${n} selected clip${n===1?'':'s'}, ${size}${nf?`, ${nf} couldn’t be moved`:''}${out?`, ${out} not in this view`:''}`);
    rv.dataset.tip=(inRv?'Showing the selected clips':'Review: show only the selected clips')+` · ${sub.textContent}`;
    const all=$('#selAll'),every=view.length>0&&inV===view.length;
    all.setAttribute('aria-checked',every?'true':inV?'mixed':'false');all.disabled=!view.length;
    $('#selAllLbl').textContent=`Select all ${view.length}`;
    all.dataset.tip=every?'Deselect everything in this view':lib.q.trim()?'Every search result · Ctrl+A':lib.filter==='all'&&lib.group==='game'?'Every clip, including the ones behind “+N more” · Ctrl+A':'Every clip in this view, including ones further down · Ctrl+A';
    const allFav=cs.every(c=>c.fav),fv=$('#selFav');$('use',fv).setAttribute('href',allFav?'#i-star-f':'#i-star');fv.classList.toggle('on',allFav);
    const takeBack=favLast&&favLast.key===selKey();   // ★ again right after a bulk ★ takes that one back
    fv.dataset.tip=takeBack?`Take back the last ★: ${favLast.val?'remove the':'re-add the'} ${favLast.ids.length} it changed`:allFav?`Remove ${n===1?'it':'all '+n} from Favourites`:`Add ${n===1?'it':'all '+n} to Favourites`;fv.setAttribute('aria-label',fv.dataset.tip);
  }
  // screen readers: one polite announcement once the clicking settles: which clip changed (one at a time), then the count
  clearTimeout(liveT);liveT=setTimeout(()=>{const n=sel.ids.size,say=sel.say;sel.say='';if(n===liveN&&!say)return;liveN=n;
    $('#selLive').textContent=(say?say+'. ':'')+(n?`${nClips(n)} selected, ${fmtTotal(selBytes(selClips()))}`:'Selection cleared');},350);
}
/* Review: exactly what's selected, from any view. Back (or Esc) returns to where you were, with the selection kept. */
function openReview(){
  if(!selOn())return;if(lib.filter!=='sel')lib.prev={filter:lib.filter,q:lib.q};
  lib.filter='sel';lib.selView=[...sel.ids];lib.q='';$('#search').value='';$('#libScroll').scrollTop=0;renderLibrary();focusLib();
}
function leaveReview(){
  const p=lib.prev||{filter:'all',q:''};lib.prev=null;lib.filter=p.filter;lib.q=p.q;$('#search').value=p.q;$('#libScroll').scrollTop=0;renderLibrary();focusLib();
}
$('#selShow').addEventListener('click',()=>{if(lib.filter!=='sel')openReview();});
$('#selClear').addEventListener('click',()=>{const inBar=$('#selBar').contains(document.activeElement),a=sel.anchor;clearSel();if(inBar)focusLib(a);});
$('#selAll').addEventListener('click',()=>{const v=viewClips();selectView(!(v.length&&v.every(c=>sel.ids.has(c.id))));});
$('#selFav').addEventListener('click',()=>favSelected());
$('#selDel').addEventListener('click',()=>trashSelected());

/* bulk ★: one set_favourites, and an Undo that puts back exactly the clips it changed (it never touches the others).
   ★ pressed again on the same selection, before anything else changes it, is that same Undo: after "Add all 269" it puts
   the 243 it added back out, so the 26 you had are never lost to an add-then-remove. */
let favLast=null;   // {key, ids, val}: the last bulk ★ and the selection it was made on
const selKey=()=>[...sel.ids].sort().join(',');
async function applyFav(cs,val){
  const paint=()=>{cs.forEach(c=>$$(`.lib [data-clip="${CSS.escape(c.id)}"] .flags`).forEach(f=>f.innerHTML=flagsHTML(c)));if(ed.clip&&cs.includes(ed.clip))syncFav();if(lib.filter==='fav')renderLibrary(true);else{renderSide();syncSel();}};
  const was=new Map(cs.map(c=>[c.id,c.fav]));cs.forEach(c=>c.fav=val);paint();
  try{await BT.setFavourites(cs.map(c=>c.id),val);return true;}catch(e){cs.forEach(c=>c.fav=was.get(c.id));paint();fail("Couldn't update Favourites",e);return false;}
}
async function favSelected(){
  const key=selKey();
  if(favLast&&favLast.key===key){   // take back the last bulk ★ instead of toggling everything the other way
    const fl=favLast,back=fl.ids.map(id=>S.byId[id]).filter(Boolean);favLast=null;if(libUndo&&libUndo.fav===fl)libUndo=null;
    if(await applyFav(back,!fl.val))toast(`Favourites restored · ${fl.val?'took':'put'} ${nClips(back.length)} ${fl.val?'back out':'back in'}`,'star');
    return;
  }
  const cs=selClips(),v=!cs.every(c=>c.fav),ch=cs.filter(c=>c.fav!==v);if(!ch.length)return;
  if(!await applyFav(ch,v))return;
  const fl={key:selKey(),ids:ch.map(c=>c.id),val:v};favLast=fl;
  undoToast(`${v?'Added':'Removed'} ${nClips(ch.length)} ${v?'to':'from'} Favourites`,'star',async()=>{
    if(favLast===fl)favLast=null;if(await applyFav(fl.ids.map(id=>S.byId[id]).filter(Boolean),!v))toast('Favourites restored','star');});
  libUndo.fav=fl;syncSel();
}

/* ---------- can the Recycle Bin keep these? perm = clips on a drive that would delete them for good: no Recycle Bin, set to
   delete straight away, or a batch bigger than that drive's Recycle Bin (Windows makes room by purging its oldest items).
   Each drive entry lists its clipIds (contract, round 3), so nothing is guessed from paths. */
async function binCheck(cs){
  if(!cs.length)return {ok:true,fits:true,bad:[],perm:[]};
  try{
    const r=await BT.trashCheck(cs.map(c=>c.id));if(r.fits)return {ok:true,fits:true,bad:[],perm:[]};
    let bad=(r.drives||[]).filter(d=>d.permanent||(d.capacity!=null&&d.bytes>d.capacity));if(!bad.length)bad=r.drives||[];
    const ids=new Set(bad.flatMap(d=>d.clipIds||[]));
    let perm=cs.filter(c=>ids.has(c.id));if(!perm.length)perm=cs;   // can't tell which clips: promise nothing
    return {ok:true,fits:false,bad,perm};
  }catch(e){BT.log('warn',`trash_check failed: ${errMsg(e)}`);return {ok:false,err:errMsg(e),fits:true,bad:[],perm:[]};}
}
/* why they can't be kept, in a red callout (or, if the check itself failed, a quiet note that drops the restore promise) */
function binWarn(chk,n,p){
  if(!chk.ok)return `<div class="cf-note warn">${ic('alert','xs')}<span>Backtrack couldn’t check that the Recycle Bin has room for ${n===1?'it':'them'}.</span></div>`;
  if(chk.fits)return '';
  const cap=chk.bad.find(d=>!d.permanent&&d.capacity!=null);
  const why=chk.bad.map(d=>d.permanent?`Drive ${esc(d.drive)} doesn’t keep deleted files: it has no Recycle Bin (a USB or network drive, for example) or it’s set to delete files straight away.`
    :`The Recycle Bin on drive ${esc(d.drive)} holds up to ${fmtTotal(d.capacity)}, and ${p===1?'this clip is':'these clips are'} ${fmtTotal(d.bytes)}. To make room, Windows would permanently delete its oldest items, which can include ${p===1?'this one':'some of these'}.`);
  // honest, not "move it in smaller batches": each batch pushes the previous ones out of a full Recycle Bin
  const tip=cap?`<span class="cf-tip">The Recycle Bin on ${esc(cap.drive)} keeps at most ${fmtTotal(cap.capacity)}, so moving more always pushes out older items, including clips you moved earlier. You can raise the limit in the Recycle Bin’s Properties.</span>`:'';
  return `<div class="callout bad cf-perm">${ic('alert','lg')}<div class="ct"><b>${p===n?(n===1?'This clip can’t':'These clips can’t'):`${p} of these clips can’t`} be kept in the Recycle Bin</b>${why.length>1?`<ul>${why.map(w=>`<li>${w}</li>`).join('')}</ul>`:`<span>${why[0]||''}</span>`}${tip}</div></div>`;
}
const checkingNote=n=>`<div class="cf-note cf-checking"><span class="spin"></span><span>Checking that the Recycle Bin can keep ${n===1?'it':'them'}…</span></div>`;
/* the Recycle Bin check is asked for before anything else, and OK waits for it: the dialog opens at once with OK disabled
   ("Checking the Recycle Bin…"), and whenever the batch changes (Keep favourites) the count updates straight away and OK waits
   again for the check of exactly that batch. A busy dialog is never refilled. On OK the checked batch must equal the batch being
   sent; if it ever doesn't, it's checked again and the dialog shown again instead of acting.
   text(cs, chk) builds the dialog; bind(recheck) lets the caller re-check. → {go: true|'alt', chk, list} or null (Cancel) */
async function checkedConfirm(pick,text,bind){
  const keyOf=cs=>cs.map(c=>c.id).sort().join(',');
  let tok=0,chk={pending:true,key:'',ok:true,fits:true,bad:[],perm:[]};
  const refresh=()=>{if($('#confirmLayer').hidden||$('#confirm').classList.contains('busy'))return;cfFill(Object.assign(text(pick(),chk),{keep:true}));};
  const recheck=async()=>{
    const my=++tok,list=pick(),key=keyOf(list);chk={pending:true,key,ok:true,fits:true,bad:[],perm:[]};refresh();
    const c=await binCheck(list);if(my!==tok)return;c.key=key;chk=c;refresh();
  };
  if(bind)bind(recheck);
  let p=confirmBox(Object.assign(text(pick(),chk),{keep:true}));recheck();
  for(;;){
    const v=await p;if(!v){tok++;return null;}
    const list=pick();if(!chk.pending&&chk.key===keyOf(list))return {go:v,chk,list};
    recheck();p=confirmBox(Object.assign(text(pick(),chk),{keep:true}));   // never act on a check of a different batch
  }
}

/* one clip (right-click, Delete on a focused card, the editor's bin): the same Recycle Bin check and Undo as a batch */
function oneText(c,chk){
  const file=`<b>${esc(c.fileName)}</b> (${fmtMB(MBof(c.sizeBytes))})`;
  if(chk.pending)return {title:'Move this clip to the Recycle Bin?',sub:clipLbl(c),ok:'Checking the Recycle Bin…',wait:true,
    html:`<p>${file} goes to the Windows Recycle Bin. Exports you already made are kept.</p><div class="cf-notes">${checkingNote(1)}</div>`};
  if(chk.perm.length)return {perm:true,icon:'alert',title:'Delete this clip permanently?',sub:clipLbl(c),ok:'Delete permanently',
    html:`${binWarn(chk,1,1)}<p>${file} will be deleted straight away and can’t be restored. Exports you already made are kept.</p>`};
  return {title:'Move this clip to the Recycle Bin?',sub:clipLbl(c),ok:'Move to Recycle Bin',
    html:`<p>${file} goes to the Windows Recycle Bin${chk.ok?', so you can still restore it from there':''}. Exports you already made are kept.</p>${chk.ok?'':binWarn(chk,1,1)}`};
}
async function trashClip(c){
  if(!c)return;
  const res=await checkedConfirm(()=>[c],(cs,chk)=>oneText(c,chk));if(!res)return;
  const perm=res.chk.perm.length>0;
  cfBusy(perm?'Deleting…':'Moving…');
  const wasOpen=ed.clip&&ed.clip.id===c.id;let next=null;
  if(wasOpen){const l=gameList(),i=l.indexOf(ed.clip);next=l[i+1]||l[i-1]||null;}
  try{
    if(perm){const r=await BT.trashClips([c.id],{permanent:true});if(r.failed.length)throw r.failed[0].error;}
    else await BT.trashClip(c.id);
  }catch(e){closeConfirm(false,true);fail(perm?"Couldn't delete the clip":"Couldn't move the clip to the Recycle Bin",e);return;}
  closeConfirm(true,true);
  setClips(S.clips.filter(x=>x.id!==c.id));   // drop it now; library-changed brings the authoritative list
  if(perm)toast(`Deleted ${c.fileName} permanently`,'trash');
  else{const mv={ids:[c.id],after:new Set(sel.ids)};lastMove=mv;undoToast(`Moved ${c.fileName} to the Recycle Bin`,'trash',()=>restoreMoved(mv));}
  if(wasOpen){if(next&&S.byId[next.id])openClip(S.byId[next.id]);else{closeClip();go('library');}}
  else if(cur()==='library')renderLibrary(true);else renderSide();
}

/* ---------- the selection to the Recycle Bin: one confirm that shows what's in it (and whether it can come back), a busy
   state, then a toast with Undo, or the list of clips that couldn't be moved (they stay selected, marked, for Try again). */
function trashText(cs,st,total){
  const n=cs.length,bytes=selBytes(cs),kept=total-n,chk=st.chk||{ok:true,fits:true,perm:[]};
  const pset=new Set(chk.perm.map(c=>c.id)),p=cs.filter(c=>pset.has(c.id)).length,k=n-p;
  const games=[...cs.reduce((m,c)=>{const g=m.get(c.gameId)||{id:c.gameId,name:c.game,n:0,b:0};g.n++;g.b+=c.sizeBytes;return m.set(c.gameId,g);},new Map()).values()].sort((a,b)=>b.n-a.n||b.b-a.b);
  const favs=st.favN,shared=cs.filter(isShared).length,vs=new Set(viewClips().map(c=>c.id)),out=cs.filter(c=>!vs.has(c.id)).length;
  const openEd=ed.clip&&cs.some(c=>c.id===ed.clip.id),everything=n===S.clips.length;
  const row=g=>`<div class="cf-row">${gameIcon(S.G[g.id]||{name:g.name},'gi')}<span class="nm">${esc(g.name)}</span><em>${nClips(g.n)} · ${fmtTotal(g.b)}</em></div>`;
  const rest=games.slice(4),restN=rest.reduce((s,g)=>s+g.n,0);
  const list=games.length>1?`<div class="cf-list">${games.slice(0,rest.length?3:4).map(row).join('')}${rest.length?`<div class="cf-row more"><span class="nm">${games.length-3} more games</span><em>${nClips(restN+games[3].n)} · ${fmtTotal(rest.reduce((s,g)=>s+g.b,0)+games[3].b)}</em></div>`:''}</div>`:'';
  const notes=[
    chk.pending?checkingNote(n):'',
    st.favN&&st.mixed?`<div class="cf-note fav">${ic('star-f','xs')}<span>${st.favN===1?'1 is a favourite':`${st.favN} are favourites`}</span><button class="cf-keep" id="cfKeepFav" role="checkbox" aria-checked="${st.keepFav}"><i class="cbx"></i>Keep ${st.favN===1?'it':'them'}</button></div>`
      :favs?`<div class="cf-note fav">${ic('star-f','xs')}<span>${n===1?'It’s a favourite':`All ${n} are favourites`}</span></div>`:'',
    shared&&n>1?`<div class="cf-note">${ic('chat','xs')}<span>${shared===n?`All ${n} were`:shared===1?'1 was':`${shared} were`} shared to Discord. ${shared===1?'Its':'Their'} exports are kept.</span></div>`:'',
    openEd?`<div class="cf-note">${ic('scissors','xs')}<span>${n===1?'It’s the clip that’s open in the editor.':'Includes the clip that’s open in the editor.'}</span></div>`:'',
    out?`<div class="cf-note warn">${ic('alert','xs')}<span>${out===n?(n===1?'It isn’t':'None of them are'):`${out} of them aren’t`} in the view you’re looking at.</span><button class="cf-link" data-cfreview>Review</button></div>`:'',
    chk.ok?'':binWarn(chk,n,p),
  ].join('');
  const nb=notes?`<div class="cf-notes">${notes}</div>`:'',c=cs[0],exp=shared?'':' Exports you already made are kept.';
  const sub=everything?`Every clip in your library · ${fmtTotal(bytes)}`:`${fmtTotal(bytes)} · ${games.length===1?games[0].name:games.length+' games'}${kept?` · ${kept} favourite${kept===1?'':'s'} kept`:''}`;
  if(chk.pending){   // the count is already right; OK waits for the check of this exact batch, and nothing is promised yet
    if(n===1)return {title:'Move this clip to the Recycle Bin?',sub:clipLbl(c),ok:'Checking the Recycle Bin…',wait:true,
      html:`<p><b>${esc(c.fileName)}</b> (${fmtMB(MBof(c.sizeBytes))}) goes to the Windows Recycle Bin. Exports you already made are kept.</p>${nb}`};
    return {title:everything?`Move all ${n} clips to the Recycle Bin?`:`Move ${n} clips to the Recycle Bin?`,sub,ok:'Checking the Recycle Bin…',wait:true,
      html:`<p>They go to the Windows Recycle Bin.${exp}</p>${list}${nb}`};
  }
  if(p&&p===n){   // the Recycle Bin can't keep any of them: say it plainly, and the button says what it really does
    if(n===1)return {perm:true,icon:'alert',title:'Delete this clip permanently?',sub:clipLbl(c),ok:'Delete permanently',
      html:`${binWarn(chk,1,1)}<p><b>${esc(c.fileName)}</b> (${fmtMB(MBof(c.sizeBytes))}) will be deleted straight away and can’t be restored. Exports you already made are kept.</p>${nb}`};
    return {perm:true,icon:'alert',title:`Delete ${everything?'all ':''}${n} clips permanently?`,sub,ok:`Delete ${n} clips permanently`,
      html:`${binWarn(chk,n,p)}<p>They’ll be deleted straight away and can’t be restored.${exp} Windows frees the ${fmtTotal(bytes)} right away.</p>${list}${nb}`};
  }
  if(p){   // mixed: move what can be kept; for the rest, delete permanently (OK) or skip them and leave them selected (the middle button)
    const dv=esc(chk.bad.map(d=>d.drive).join(', ')),pg=esc([...new Set(cs.filter(c=>pset.has(c.id)).map(c=>c.game))].join(', '));
    return {perm:true,icon:'alert',title:`Move ${nClips(k)} to the Recycle Bin and delete ${p} permanently?`,sub,ok:`Move ${k}, delete ${p} permanently`,alt:`Move ${k}, skip ${dv}`,
      html:`${binWarn(chk,n,p)}<p>The other ${k===1?'clip goes':`${k} go`} to the Windows Recycle Bin, so you can still restore ${k===1?'it':'them'}. The ${p===1?'one':p} on ${dv} (${pg}) ${p===1?'is':'are'} deleted straight away, or choose <b class="cf-em">Move ${k}, skip ${dv}</b> to leave ${p===1?'it':'them'} where ${p===1?'it is':'they are'}, still selected.${exp}</p>${list}${nb}`};
  }
  const back=chk.ok?(n===1?', so you can still restore it from there':', so you can still restore them from there'):'';
  if(n===1)return {title:'Move this clip to the Recycle Bin?',sub:clipLbl(c),ok:'Move to Recycle Bin',
    html:`<p><b>${esc(c.fileName)}</b> (${fmtMB(MBof(c.sizeBytes))}) goes to the Windows Recycle Bin${back}. Exports you already made are kept.</p>${nb}`};
  return {title:everything?`Move all ${n} clips to the Recycle Bin?`:`Move ${n} clips to the Recycle Bin?`,sub,ok:`Move ${n} clips to the Recycle Bin`,
    html:`<p>They go to the Windows Recycle Bin${back}.${exp} Windows frees the ${fmtTotal(bytes)} when the Recycle Bin is emptied.</p>${list}${nb}`};
}
const doneMsg=(mv,dl,file)=>mv&&dl?`Moved ${nClips(mv)} to the Recycle Bin, deleted ${dl} permanently`:dl?(file&&dl===1?`Deleted ${file} permanently`:`Deleted ${nClips(dl)} permanently`):(file&&mv===1?`Moved ${file} to the Recycle Bin`:`Moved ${nClips(mv)} to the Recycle Bin`);
async function trashSelected(){
  const all=selClips();if(!all.length)return;
  closePop();
  // "Keep favourites" starts on when a favourite came in with a bulk pick (Select all, a group, Ctrl+A, a range), off when you ticked each one
  const favs=all.filter(c=>c.fav),favN=favs.length,mixed=favN>0&&favN<all.length;
  const st={favN,mixed,keepFav:mixed&&favs.some(c=>sel.bulk.has(c.id))};
  const pick=()=>st.keepFav?all.filter(c=>!c.fav):all;
  // DOM order before anything moves, to put keyboard focus on the card that takes the first removed one's place
  const order=$$('#libScroll .card[data-clip]').map(x=>x.dataset.clip);
  let recheck=null;
  const onKeep=e=>{const b=e.target.closest('#cfKeepFav');if(!b||b.disabled)return;st.keepFav=!st.keepFav;recheck();$('#cfKeepFav').focus();};   // count now, OK after the check
  $('#cfText').addEventListener('click',onKeep);
  let res;try{res=await checkedConfirm(pick,(cs,chk)=>trashText(cs,Object.assign({},st,{chk}),all.length),rc=>recheck=rc);}
  finally{$('#cfText').removeEventListener('click',onKeep);}
  if(!res)return;
  const skip=res.go==='alt',permIds=new Set(res.chk.perm.map(c=>c.id)),skipped=skip?res.list.filter(c=>permIds.has(c.id)):[];
  let list=skip?res.list.filter(c=>!permIds.has(c.id)):res.list,first=true;
  const keptFav=all.length-res.list.length,moved=[],deleted=[];
  const mv={ids:moved,reselect:true,after:new Set()},file=all.length===1?all[0].fileName:null;
  const run=async(cs,perm)=>{if(!cs.length)return {trashed:[],failed:[]};
    try{return await BT.trashClips(cs.map(c=>c.id),perm?{permanent:true}:null);}catch(e){return {rejected:errMsg(e),trashed:[],failed:cs.map(c=>({clipId:c.id,error:errMsg(e)}))};}};
  for(;;){
    const rec=list.filter(c=>!permIds.has(c.id)),del=list.filter(c=>permIds.has(c.id));
    cfBusy(del.length?(rec.length?`Moving ${rec.length}, deleting ${del.length}…`:`Deleting ${nClips(del.length)}…`):`Moving ${nClips(rec.length)}…`);
    const a=await run(rec,false),b=await run(del,true);
    if((a.rejected||!rec.length)&&(b.rejected||!del.length)){   // nothing could be attempted: the selection stays as it was
      const why=a.rejected||b.rejected;BT.log('error',`trash_clips failed: ${why}`);
      const again=await confirmBox({title:del.length&&!rec.length?(list.length===1?'Couldn’t delete the clip':'Couldn’t delete the clips'):(list.length===1?'Couldn’t move the clip to the Recycle Bin':'Couldn’t move the clips to the Recycle Bin'),
        sub:'Nothing was moved. Your selection is unchanged.',icon:'alert',tone:'warn',
        html:`<div class="callout bad">${ic('alert','lg')}<div class="ct"><b>What went wrong</b><span class="err-msg">${esc(why)}</span></div></div>`,ok:'Try again',okCls:'',no:'Close',keep:true});
      if(!again)return;continue;
    }
    moved.push(...a.trashed);deleted.push(...b.trashed);
    const gone=new Set([...a.trashed,...b.trashed]),fails=[...a.failed,...b.failed];
    // what was attempted (and the favourites you kept) leaves the selection; what failed or was skipped stays in it, marked
    for(const c of first?all:list){sel.ids.delete(c.id);sel.bulk.delete(c.id);}
    sel.failed.clear();for(const f of fails)if(S.byId[f.clipId]&&!gone.has(f.clipId)){sel.ids.add(f.clipId);sel.failed.set(f.clipId,f.error||'Unknown error');}
    for(const c of skipped)if(S.byId[c.id]){sel.ids.add(c.id);sel.failed.set(c.id,`Skipped: drive ${res.chk.bad.map(d=>d.drive).join(', ')} can’t keep it in the Recycle Bin`);}
    first=false;sel.anchor=null;favLast=null;
    if(ed.clip&&gone.has(ed.clip.id))closeClip();
    setClips(S.clips.filter(c=>!gone.has(c.id)));   // drop them now; library-changed brings the authoritative list
    lastMove=moved.length?mv:null;lib.emptied=S.clips.length?null:{moved:moved.length,deleted:deleted.length};
    mv.after=new Set(sel.ids);   // Undo selects them again only if you haven't started something else since
    if(cur()==='library')renderLibrary(true);else{renderSide();syncSel();}
    const failed=fails.map(f=>({c:S.byId[f.clipId],why:f.error||'Unknown error'})).filter(f=>f.c);
    const tell=extra=>{const m=doneMsg(moved.length,deleted.length,file)+(extra||'');if(moved.length)undoToast(m,'trash',()=>restoreMoved(mv),deleted.length?'Undo move':'Undo');else toast(m,'trash');};
    const skipMsg=skipped.length?` · skipped ${skipped.length} on ${res.chk.bad.map(d=>d.drive).join(', ')}`:'';
    if(!failed.length){
      closeConfirm(true,true);
      tell((keptFav?` · kept ${keptFav} favourite${keptFav===1?'':'s'}`:'')+skipMsg);
      if(document.activeElement===document.body||!shown(document.activeElement)){   // focus lands on the next clip, never on <body>
        const i=order.findIndex(id=>gone.has(id));focusLib([...order.slice(i),...order.slice(0,i).reverse()].find(id=>S.byId[id]));
      }
      return;
    }
    const done=moved.length+deleted.length,verb=deleted.length&&!moved.length?'Deleted':'Moved';
    const rows=failed.map(f=>`<div class="cf-fail"><div class="mt">${thumbHTML(f.c.thumb)}</div><div><b>${esc(clipLbl(f.c))}</b><span>${esc(f.why)}</span></div></div>`).join('');
    const again=await confirmBox({icon:'alert',tone:'warn',okCls:'',ok:'Try again',no:'Done',keep:true,
      title:done?`${verb} ${done} of ${done+failed.length} clips${verb==='Moved'?' to the Recycle Bin':''}`:failed.length===1?'Couldn’t move this clip':`Couldn’t move ${failed.length} clips`,
      sub:done?`${nClips(failed.length)} couldn’t be ${verb==='Moved'?'moved':'deleted'}`:'Nothing was moved',
      html:`<div class="cf-fails">${rows}</div><p class="cf-hint">${failed.length===1?'It stays':'They stay'} selected in the library, so you can try again once nothing is using ${failed.length===1?'it':'them'}.</p>`});
    if(!again){   // Done: Undo for what did move; focus and scroll to the first clip that didn't
      if(done)tell(` · ${failed.length} couldn’t be`+(verb==='Moved'?' moved':' deleted')+skipMsg);
      const f0=failed.map(f=>f.c.id).find(id=>$(`#libScroll .card[data-clip="${CSS.escape(id)}"]`));
      if(f0){const el=$(`#libScroll .card[data-clip="${CSS.escape(f0)}"]`);el.focus({preventScroll:true});el.scrollIntoView({block:'center'});}else $('#selDel').focus();
      return;
    }
    list=failed.map(f=>f.c).filter(c=>S.byId[c.id]);if(!list.length){closeConfirm(true,true);return;}
  }
}
/* Undo: restore_clips puts them back (same ids, favourites, edits, exports). A batch comes back selected, as it was, unless
   you've started a different selection since: then they come back unselected and the toast says your selection is kept. */
async function restoreMoved(mv){
  if(!mv||mv.busy||!mv.ids.length)return;mv.busy=true;
  toast(`Restoring ${mv.ids.length===1?'the clip':nClips(mv.ids.length)}…`,'clock');
  const same=!!mv.after&&sel.ids.size===mv.after.size&&[...sel.ids].every(id=>mv.after.has(id)),again=mv.reselect&&same;
  try{
    const r=await BT.restoreClips(mv.ids);
    if(again){r.restored.forEach(id=>sel.pending.add(id));clearTimeout(sel.pendingT);sel.pendingT=setTimeout(()=>sel.pending.clear(),15000);}
    if(lastMove===mv)lastMove=null;lib.emptied=null;
    const n=r.restored.length,how=!mv.reselect||!n?'':again?' · selected again':' · your current selection is unchanged';
    if(r.failed.length)toast(n?`Restored ${n} of ${mv.ids.length} clips · ${r.failed[0].error}`:`Couldn’t restore ${mv.ids.length===1?'the clip':'the clips'} · ${r.failed[0].error}`,'alert');
    else toast((mv.ids.length===1?'Restored the clip':`Restored ${nClips(n)}`)+how,'undo');
  }catch(e){fail("Couldn't restore the clips",e);}
  finally{mv.busy=false;}
}

/* ================================================================== EDITOR */
const MIN_SEL=1;
const ed={clip:null,dur:1,inT:0,outT:1,t:0,playing:false,loop:true,snap:true,zoom:1,canvas:'16:9',cropX:.5,preset:'d10',customMB:25,speed:1,tracks:[],markers:[],audio:[],gen:0};
const mixMem=new Map();   // per-clip volumes / mutes / canvas for this session (the contract has nowhere to save them)
const tl={W:0,pps:1};
const els={scroll:$('#tlScroll'),inner:$('#tlInner'),ruler:$('#ruler'),lanes:$('#lanes'),laneV:$('#laneV'),sel:$('#sel'),hIn:$('#hIn'),hOut:$('#hOut'),maskL:$('#maskL'),maskR:$('#maskR'),ph:$('#ph'),selLabel:$('#selLabel'),rulerSel:$('#rulerSel')};
const vid=$('#pvVideo');

/* ---------- tracks: one lane per clip.audioTracks entry */
const TRACK_STYLE={game:{label:'Game',icon:'game',c:'--lane-game'},chat:{label:'Voice chat',icon:'headset',c:'--lane-voice'},mic:{label:'Mic',icon:'mic',c:'--lane-mic'}};
const LANE_COLS=['--lane-game','--lane-voice','--lane-mic'];
function trackLabel(c,index){const a=c.audioTracks.find(x=>x.index===index),st=a&&TRACK_STYLE[String(a.name).trim().toLowerCase()];return st?st.label:(a&&a.name)||`Track ${index+1}`;}
function trackState(c,mem){
  const styled=c.audioTracks.map(a=>({a,st:TRACK_STYLE[String(a.name||'').trim().toLowerCase()]||null}));
  const free=LANE_COLS.filter(x=>!styled.some(s=>s.st&&s.st.c===x));let fi=0;
  return styled.map(({a,st})=>{
    const m=mem&&mem.tracks.find(x=>x.index===a.index),kind=st?String(a.name).trim().toLowerCase():'other';
    const ch=a.channels===1?'mono':a.channels>2?a.channels+' ch':'stereo';
    return {index:a.index,id:'t'+a.index,kind,name:st?st.label:(a.name||`Track ${a.index+1}`),icon:st?st.icon:'wave',
      c:st?st.c:(free[fi++]||LANE_COLS[a.index%3]),sub:kind==='game'?c.game:`Track ${a.index+1} · ${ch}`,
      vol:m?m.vol:100,muted:m?m.muted:false};
  });
}
const trackById=id=>ed.tracks.find(x=>x.id===id);
const laneOf=t=>$(`.lane-a[data-track="${t.id}"]`);
function rememberMix(){if(ed.clip)mixMem.set(ed.clip.id,{tracks:ed.tracks.map(t=>({index:t.index,vol:t.vol,muted:t.muted})),canvas:ed.canvas,cropX:ed.cropX});}
function buildTracks(){
  const head=$('#thList'),c=ed.clip;
  head.innerHTML=`<div class="th th-v"><span class="th-ico">${ic('film')}</span><div class="th-name"><b>Video</b><span id="vSub"></span></div></div>`+
    ed.tracks.map(t=>`<div class="th th-a" data-track="${t.id}" style="--c:var(${t.c})">
      <button class="mute" data-mute="${t.id}" data-tip="Mute ${esc(t.name)}">${ic('vol','sm')}</button>
      <div class="th-name"><b>${esc(t.name)}</b><span class="tsub"></span></div>
      <span class="vol-wrap" data-tip="Scroll ±5 %  ·  double-click for 100 %"><input type="range" class="rng vol" min="0" max="150" value="${t.vol}" data-vol="${t.id}" style="--c:var(${t.c})" aria-label="${esc(t.name)} volume, 0 to 150 percent"></span>
      <span class="pct" data-pct="${t.id}">${t.vol}%</span></div>`).join('');
  $$('.lane-a',els.lanes).forEach(n=>n.remove());
  ed.tracks.forEach(t=>{const d=document.createElement('div');d.className='lane lane-a';d.dataset.track=t.id;d.style.setProperty('--c',`var(${t.c})`);
    d.innerHTML=`<div class="lane-tint"></div><canvas></canvas><span class="lane-tag">${ic('vol-x','xs')}Muted</span>`;els.lanes.insertBefore(d,els.maskL);});
  ed.tracks.forEach(syncTrack);syncVideoSub();
}
function syncVideoSub(){const v=$('#vSub'),c=ed.clip;if(v&&c)v.textContent=`${c.height}p${fpsR(c)} · `+(ed.canvas==='16:9'?`Original ${aspectLbl(c)}`:'Vertical 9:16');}
function syncTrack(t){
  const h=$(`.th[data-track="${t.id}"]`),l=laneOf(t);if(!h||!l)return;
  h.classList.toggle('is-muted',t.muted);l.classList.toggle('is-muted',t.muted);
  $('.mute use',h).setAttribute('href',t.muted?'#i-vol-x':'#i-vol');
  $('.mute',h).dataset.tip=(t.muted?'Unmute ':'Mute ')+t.name;
  $('.tsub',h).textContent=t.muted?'Muted':t.sub;
  const r=$('.vol',h);r.value=t.vol;r.style.setProperty('--p',(t.vol/150*100)+'%');
  $(`[data-pct="${t.id}"]`).textContent=t.vol+'%';
}
$('#thList').addEventListener('click',e=>{const b=e.target.closest('[data-mute]');if(!b)return;const t=trackById(b.dataset.mute);t.muted=!t.muted;syncTrack(t);applyVolumes();updateEstimates();rememberMix();toast(`${t.name} ${t.muted?'muted':'unmuted'} in the export`,t.muted?'vol-x':'vol');});
function setVol(t,v){t.vol=clamp(Math.round(v),0,150);if(t.vol>0&&t.muted)t.muted=false;syncTrack(t);drawLane(t);applyVolumes();updateEstimates();rememberMix();}
const volTrack=e=>{const r=e.target.closest('.vol-wrap');return r&&trackById($('[data-vol]',r).dataset.vol);};
$('#thList').addEventListener('input',e=>{const r=e.target.closest('[data-vol]');if(!r)return;setVol(trackById(r.dataset.vol),+r.value);});
// fine control without a wider panel: wheel steps of 5 %, double-click snaps back to 100 %
$('#thList').addEventListener('wheel',e=>{const t=volTrack(e);if(!t)return;e.preventDefault();setVol(t,Math.round(t.vol/5)*5+(e.deltaY<0?5:-5));},{passive:false});
$('#thList').addEventListener('dblclick',e=>{const t=volTrack(e);if(!t)return;setVol(t,100);toast(`${t.name} back to 100 %`,'vol');});

/* ---------- waveforms: get_waveform(clipId, track, points), points sized to the timeline width */
const waveCache=new Map();   // `${clipId}:${index}` → {points, data, pending}
function wantPoints(){return clamp(Math.ceil(tl.W/3/256)*256,256,8192);}
function ensureWave(t){
  if(!ed.clip||!tl.W)return;
  const clipId=ed.clip.id,key=clipId+':'+t.index,need=wantPoints();
  let w=waveCache.get(key);
  if(w&&(w.points>=need||w.pending>=need||w.failed))return;
  if(!w){w={points:0,data:null,pending:0,failed:false};waveCache.set(key,w);if(waveCache.size>90)waveCache.delete(waveCache.keys().next().value);}
  w.pending=need;
  const l=laneOf(t);if(l&&!w.data)l.classList.add('loading');
  BT.getWaveform(clipId,t.index,need).then(arr=>{
    if(w.pending===need)w.pending=0;
    if(!w.data||need>w.points){w.data=arr;w.points=need;}
    if(ed.clip&&ed.clip.id===clipId){const tt=ed.tracks.find(x=>x.index===t.index);if(tt){const ll=laneOf(tt);ll&&ll.classList.remove('loading');drawLane(tt);}}
  },err=>{
    w.pending=0;w.failed=true;
    if(S.settings&&S.settings.ffmpegPath)BT.log('warn',`get_waveform(${clipId}, ${t.index}) failed: ${errMsg(err)}`);
    if(ed.clip&&ed.clip.id===clipId){const tt=ed.tracks.find(x=>x.index===t.index),ll=tt&&laneOf(tt);ll&&ll.classList.remove('loading');}
  });
}
function cssVar(n){return getComputedStyle(document.documentElement).getPropertyValue(n).trim();}
// gain = track volume (0–1.5): turning a lane down visibly shrinks its waveform
function drawWave(cv,data,color,W,H,gain=1){
  const dpr=window.devicePixelRatio||1;cv.width=Math.round(W*dpr);cv.height=Math.round(H*dpr);cv.style.width=W+'px';cv.style.height=H+'px';
  const x=cv.getContext('2d');x.setTransform(dpr,0,0,dpr,0,0);x.clearRect(0,0,W,H);if(!data||!data.length)return;x.fillStyle=color;
  const N=data.length,step=3,bw=2,mid=H/2,maxA=H/2-5,cap=H/2-2;
  for(let px=0;px<W;px+=step){const i0=Math.floor(px/W*N),i1=Math.max(i0+1,Math.floor((px+step)/W*N));let m=0;for(let i=i0;i<i1&&i<N;i++)if(data[i]>m)m=data[i];const h=Math.min(cap,Math.max(.75,m*maxA*gain));x.fillRect(px,mid-h,bw,h*2);}
}
function drawLane(t){
  if(!ed.clip||!tl.W)return;const l=laneOf(t);if(!l)return;
  const w=waveCache.get(ed.clip.id+':'+t.index);
  drawWave($('canvas',l),w&&w.data,cssVar(t.c),tl.W,l.clientHeight,t.vol/100);
}
/* the video lane uses the clip thumbnail as its filmstrip */
function renderFilm(){
  const u=ed.clip.thumb?BT.src(ed.clip.thumb):'';
  if(!u){els.laneV.innerHTML='';return;}
  const url=esc(u.replace(/'/g,'%27')),h=els.laneV.clientHeight||50,fw=Math.round(h*16/9),n=Math.ceil(tl.W/fw);let s='';
  for(let i=0;i<n;i++){const sx=50+Math.sin(i*.9)*22,sy=48+Math.cos(i*.55)*14,sc=118+Math.sin(i*.45)*14;
    s+=`<div class="fr" style="left:${i*fw}px;width:${fw}px;background-image:url('${url}');background-size:${sc.toFixed(1)}% auto;background-position:${sx.toFixed(1)}% ${sy.toFixed(1)}%"></div>`;}
  els.laneV.innerHTML=s;
}
function renderRuler(){
  const pps=tl.pps,W=tl.W,H=parseFloat(getComputedStyle(els.ruler).height)||26;
  const minor=[.1,.2,.25,.5,1,2,5,10,15,30,60].find(s=>s*pps>=7)||60;
  const major=[.5,1,2,5,10,15,30,60,120,300].find(s=>s>=minor&&s*pps>=62)||300;
  const ratio=Math.round(major/minor);
  let lines='';const n=Math.floor(ed.dur/minor+1e-6);
  for(let i=0;i<=n;i++){const t=i*minor,x=Math.round(t*pps)+.5,isM=i%ratio===0,mid=!isM&&ratio%2===0&&i%(ratio/2)===0;
    lines+=`<line x1="${x}" x2="${x}" y1="${H-(isM?10:mid?7:4)}" y2="${H}" stroke="${isM?'rgba(255,255,255,.42)':'rgba(255,255,255,.2)'}"/>`;}
  // the end label is right-aligned at W; drop any regular label whose right edge comes within 30 px of it
  let labels='';const nm=Math.floor(ed.dur/major+1e-6);const endTxt=fmtLen(ed.dur),endW=endTxt.length*6.6+2,lblHalf=major<1?16:13;
  for(let i=0;i<=nm;i++){const t=i*major,x=t*pps;if(i>0&&(W-endW)-(x+lblHalf)<30)continue;labels+=`<span class="rl${i===0?' first':''}" style="left:${x}px">${major<1?fmt(t,1):fmtLen(t)}</span>`;}
  labels+=`<span class="rl last" style="left:${W}px">${endTxt}</span>`;
  $('#rulerTicks').innerHTML=`<svg width="${W}" height="${H}">${lines}</svg>${labels}`;
}
function renderTimeline(){
  if(!ed.clip)return;const cw=els.scroll.clientWidth;if(!cw)return;
  tl.W=Math.round(cw*ed.zoom);tl.pps=tl.W/ed.dur;els.inner.style.width=tl.W+'px';
  renderRuler();renderFilm();
  ed.tracks.forEach(t=>{drawLane(t);ensureWave(t);});
  renderMarkers();
  updateSel(false);updatePlayhead();
  $('#zoomLbl').textContent=ed.zoom<=1.02?'Fit':ed.zoom.toFixed(1)+'×';
  const z=$('#zoom');z.value=ed.zoom;z.style.setProperty('--p',((ed.zoom-1)/7*100)+'%');
}
function updateSel(changed=true){
  const x1=ed.inT*tl.pps,x2=ed.outT*tl.pps,w=x2-x1;
  els.sel.style.left=x1+'px';els.sel.style.width=w+'px';
  els.maskL.style.width=x1+'px';els.maskR.style.left=x2+'px';els.maskR.style.width=Math.max(0,tl.W-x2)+'px';
  $$('.lane-tint',els.lanes).forEach(t=>{t.style.left=x1+'px';t.style.width=w+'px';});
  els.rulerSel.style.left=x1+'px';els.rulerSel.style.width=w+'px';
  const d=ed.outT-ed.inT;
  els.selLabel.style.left=(x1+w/2)+'px';els.selLabel.textContent=`${d.toFixed(1)} s`;
  const lw=els.selLabel.offsetWidth||60,l0=x1+w/2-lw/2-6,l1=x1+w/2+lw/2+6;
  $$('.rl',els.ruler).forEach(r=>{const rx=parseFloat(r.style.left),rw=r.offsetWidth||28;const a0=r.classList.contains('first')?rx:r.classList.contains('last')?rx-rw:rx-rw/2;r.classList.toggle('hide',a0<l1&&a0+rw>l0);});
  $$('.lane-tag',els.lanes).forEach(t=>t.style.left=(x1+18)+'px');
  const iv=$('#inV'),ov=$('#outV');if(document.activeElement!==iv)iv.value=fmt(ed.inT);if(document.activeElement!==ov)ov.value=fmt(ed.outT);
  $('#selDur').textContent=d.toFixed(2)+' s';
  const full=ed.inT<=0.01&&ed.outT>=ed.dur-0.01;
  $('#pvSel').innerHTML=full?`${ic('film','xs')}<span>Playing full clip · ${d.toFixed(1)} s</span>`:`${ic('loop','xs')}<span>Previewing selection · ${fmt(ed.inT,1)} – ${fmt(ed.outT,1)} · ${d.toFixed(1)} s</span>`;
  if(changed){updateEstimates();scheduleSave();}
}

/* ---------- "Edits saved": In/Out + markers go to save_edits, debounced (no file is written) */
let saveT=0,savePending=null;
function setSaved(state){
  const s=$('#savedInd'),u=$('use',s),t=$('#savedTxt');
  s.classList.toggle('busy',state==='busy');s.style.color=state==='err'?'var(--bad)':'';
  const [icon,txt]={busy:['replay','Saving edits…'],err:['alert','Couldn’t save edits'],none:['check-c','Full clip · not trimmed'],ok:['check-c','Edits saved']}[state];
  u.setAttribute('href','#i-'+icon);t.textContent=txt;$('svg',s).style.color=state==='none'?'var(--text-3)':state==='err'?'var(--bad)':'';
}
function scheduleSave(){
  if(!ed.clip)return;const c=ed.clip;
  const edits={inT:r3(ed.inT),outT:r3(ed.outT),markers:ed.markers.map(r3)};
  c.edits=edits;savePending={clipId:c.id,edits};setSaved('busy');
  clearTimeout(saveT);saveT=setTimeout(flushSave,400);
}
function flushSave(){
  clearTimeout(saveT);const p=savePending;if(!p)return Promise.resolve();savePending=null;
  return BT.saveEdits(p.clipId,p.edits).then(()=>{if(!savePending&&ed.clip&&ed.clip.id===p.clipId)setSaved('ok');},
    err=>{if(ed.clip&&ed.clip.id===p.clipId)setSaved('err');fail("Couldn't save your edits",err);});
}
/* trim history: every In/Out change can be undone (Ctrl+Z / Ctrl+Y, and the Undo button in toasts) */
const hist={u:[],r:[]};
const trimState=()=>({inT:ed.inT,outT:ed.outT});
const sameTrim=(a,b)=>Math.abs(a.inT-b.inT)<.004&&Math.abs(a.outT-b.outT)<.004;
function pushUndo(prev){if(sameTrim(prev,trimState()))return;hist.u.push(prev);if(hist.u.length>60)hist.u.shift();hist.r.length=0;}
function restoreTrim(s){ed.inT=s.inT;ed.outT=s.outT;updateSel();if(ed.t<ed.inT||ed.t>ed.outT)seek(ed.inT);}
function undoTrim(){if(!hist.u.length){toast('Nothing to undo','info');return;}hist.r.push(trimState());restoreTrim(hist.u.pop());toast(`Undone · selection is ${fmt(ed.inT,1)} – ${fmt(ed.outT,1)}`,'undo');}
function redoTrim(){if(!hist.r.length){toast('Nothing to redo','info');return;}hist.u.push(trimState());restoreTrim(hist.r.pop());toast(`Redone · selection is ${fmt(ed.inT,1)} – ${fmt(ed.outT,1)}`,'replay');}
const UNDO={label:'Undo',kbd:'Ctrl Z',fn:()=>undoTrim()};

/* ---------- markers: from clip.edits.markers (GG's in-game marks, or M here); snap targets; saved with the edits */
function renderMarkers(){
  $('#mkLayer').innerHTML=ed.markers.map((m,i)=>`<span class="mk" data-mk="${i}" style="left:${m*tl.pps}px" data-tip="Marker · ${fmt(m)}  ·  Del removes"></span>`).join('');
  $('#mkLines').innerHTML=ed.markers.map(m=>`<span class="mk-line" style="left:${Math.round(m*tl.pps)}px"></span>`).join('');
}
function addMarker(){
  const t=Math.round(ed.t*100)/100;
  if(ed.markers.some(m=>Math.abs(m-t)<.25)){toast('There’s already a marker here','flag');return;}
  ed.markers.push(t);ed.markers.sort((a,b)=>a-b);renderMarkers();scheduleSave();toast('Marked '+fmt(t)+' · trims snap to it','flag');
}
function removeMarker(){
  const tol=Math.max(.25,8/tl.pps);let bi=-1,bd=tol;
  ed.markers.forEach((m,i)=>{const d=Math.abs(m-ed.t);if(d<=bd){bd=d;bi=i;}});
  if(bi<0){toast('No marker at the playhead · click a marker to jump to it','flag');return;}
  const [m]=ed.markers.splice(bi,1);renderMarkers();scheduleSave();
  toast('Removed the marker at '+fmt(m),'flag',{label:'Undo',fn:()=>{if(!ed.markers.some(x=>Math.abs(x-m)<.01)){ed.markers.push(m);ed.markers.sort((a,b)=>a-b);renderMarkers();scheduleSave();}}});
}
$('#mkLayer').addEventListener('pointerdown',e=>{const m=e.target.closest('[data-mk]');if(!m)return;e.stopPropagation();pause();seek(ed.markers[+m.dataset.mk]);});

/* ---------- playhead, scrubbing, drags */
function updatePlayhead(){
  els.ph.style.left=(ed.t*tl.pps)+'px';$('#tCur').textContent=fmt(ed.t);
  // the mock has no video file: give its still poster a little camera motion so scrubbing reads as scrubbing
  const img=$('#pvImg');img.style.transform=BT.isMock&&!vOK?`scale(${(1.04+.02*Math.sin(ed.t*.37)).toFixed(4)}) translate(${(Math.sin(ed.t*.8)*1.1).toFixed(3)}%,${(Math.cos(ed.t*.53)*.8).toFixed(3)}%)`:'';
}
function tAt(e){const r=els.inner.getBoundingClientRect();return clamp((e.clientX-r.left)/r.width*ed.dur,0,ed.dur);}
function snapT(t){if(!ed.snap)return t;const tol=8/tl.pps;const cands=[ed.t,0,ed.dur,...ed.markers];let best=t,bd=tol;for(const c of cands){const d=Math.abs(t-c);if(d<bd){bd=d;best=c;}}return best;}
let drag=null;
function startDrag(kind,e,extra={}){drag=Object.assign({kind,x0:e.clientX,moved:false,before:kind==='scrub'?null:trimState()},extra);hideGhost();e.preventDefault();window.addEventListener('pointermove',onDrag);window.addEventListener('pointerup',endDrag,{once:true});if(kind==='in')els.hIn.classList.add('drag');if(kind==='out')els.hOut.classList.add('drag');}
function onDrag(e){
  if(!drag)return;const t=tAt(e);if(Math.abs(e.clientX-drag.x0)>3)drag.moved=true;
  if(drag.kind==='in'){ed.inT=Math.max(0,Math.min(snapT(t),ed.outT-MIN_SEL));updateSel();}
  else if(drag.kind==='out'){ed.outT=Math.min(ed.dur,Math.max(snapT(t),ed.inT+MIN_SEL));updateSel();}
  else if(drag.kind==='move'&&drag.moved){const len=drag.out0-drag.in0;const ni=clamp(drag.in0+(t-drag.t0),0,ed.dur-len);ed.inT=ni;ed.outT=ni+len;updateSel();}
  else if(drag.kind==='scrub'){scrubTo(t);}
}
function endDrag(e){window.removeEventListener('pointermove',onDrag);els.hIn.classList.remove('drag');els.hOut.classList.remove('drag');
  if(drag&&drag.kind==='move'&&!drag.moved)seek(tAt(e));
  if(drag&&drag.kind==='scrub'){clearTimeout(vsk.settle);if(!ed.playing)vSeek(ed.t);}   // land on the exact frame
  if(drag&&drag.before)pushUndo(drag.before);drag=null;}
els.hIn.addEventListener('pointerdown',e=>{e.stopPropagation();startDrag('in',e);});
els.hOut.addEventListener('pointerdown',e=>{e.stopPropagation();startDrag('out',e);});
els.sel.addEventListener('pointerdown',e=>{if(e.target.closest('.h'))return;e.stopPropagation();startDrag('move',e,{t0:tAt(e),in0:ed.inT,out0:ed.outT});});
els.ruler.addEventListener('pointerdown',e=>{pause();seek(tAt(e));startDrag('scrub',e);});
els.lanes.addEventListener('pointerdown',e=>{if(e.target.closest('.sel'))return;pause();seek(tAt(e));startDrag('scrub',e);});
$('#phHead').addEventListener('pointerdown',e=>{e.stopPropagation();pause();startDrag('scrub',e);});

/* ---------- media: muted <video> + one <audio> per track (prepare_audio), kept in sync */
let vOK=false,vFailed=false;
const hasVideo=()=>!!vid.getAttribute('src')&&!vFailed;
const masterVol=()=>(+$('#pvVol').value||0)/100;
function applyVolumes(){
  const mv=masterVol();
  // <audio>.volume tops out at 1, so the preview caps lanes at 100 %; the export applies the full 0–150 %
  ed.audio.forEach(a=>{const t=ed.tracks.find(x=>x.index===a.index);a.el.volume=t&&!t.muted?Math.min(1,t.vol/100)*mv:0;});
}
const mediaTime=()=>hasVideo()?vid.currentTime:ed.t;
function syncAudio(force){
  const mt=mediaTime();
  ed.audio.forEach(a=>{
    if(force||Math.abs(a.el.currentTime-mt)>.08){try{a.el.currentTime=mt;}catch(_){}}
    if(a.el.playbackRate!==ed.speed)a.el.playbackRate=ed.speed;
    if(ed.playing&&a.el.paused&&mt<(a.el.duration||Infinity)-.05)a.el.play().catch(()=>{});
  });
}
function releaseAudio(){ed.audio.forEach(a=>{a.el.pause();a.el.removeAttribute('src');try{a.el.load();}catch(_){}});ed.audio=[];}
function unloadMedia(){vOK=false;vFailed=false;resetSeeks();vid.pause();vid.removeAttribute('src');try{vid.load();}catch(_){}$('#preview').classList.remove('has-video');releaseAudio();}
function loadMedia(c,gen){
  unloadMedia();$('#pvNote').hidden=true;
  const src=BT.src(c.path);
  if(src){
    vid.src=src;vid.playbackRate=ed.speed;vid.load();
    BT.getKeyframes(c.id).then(k=>{if(gen===ed.gen&&Array.isArray(k))vsk.keys=k;},
      err=>BT.log('warn',`get_keyframes(${c.id}) failed, scrubbing seeks exact frames only: ${errMsg(err)}`));
  }
  BT.prepareAudio(c.id).then(list=>{
    if(gen!==ed.gen)return;
    ed.audio=list.map(a=>{const s=BT.src(a.path);if(!s)return null;const el=new Audio();el.preload='auto';el.src=s;el.playbackRate=ed.speed;return {index:a.index,el};}).filter(Boolean);
    applyVolumes();if(ed.playing)syncAudio(true);
  },err=>{
    if(gen!==ed.gen)return;
    BT.log('warn',`prepare_audio(${c.id}) failed: ${errMsg(err)}`);
    if(S.settings.ffmpegPath)toast(`Couldn't load this clip's audio for preview · ${errMsg(err)}`,'alert');
  });
}
vid.addEventListener('loadeddata',()=>{if(!vid.getAttribute('src'))return;vOK=true;$('#preview').classList.add('has-video');if(!ed.playing&&Math.abs(vid.currentTime-ed.t)>.01)vSeekNow(ed.t);updatePlayhead();});
vid.addEventListener('error',()=>{
  if(!vid.getAttribute('src'))return;vFailed=true;vOK=false;$('#preview').classList.remove('has-video');$('#pvNote').hidden=false;last=performance.now();
  BT.log('warn',`video preview failed for ${ed.clip&&ed.clip.path}: code ${vid.error&&vid.error.code} ${vid.error&&vid.error.message||''}`);
});
/* ---------- seeking the preview video
   A <video> seek decodes forward from the keyframe before the target: ≈80 ms for a random spot in a GG clip
   (a keyframe every 2.08 s), up to ≈500 ms just before the next keyframe. Setting currentTime again cancels
   the seek in flight, so seeking on every pointer move left the preview frozen until the mouse stopped.
   Instead: one seek at a time, and only the newest target goes next. While a drag moves fast, aim at the
   nearest keyframe (≈10 ms to decode), then land on the exact frame once the drag slows down or stops. */
const vsk={busy:false,want:null,at:0,exact:true,cost:120,keys:null,settle:0};
function vSeekNow(t,exact=true){   // seek right away, dropping anything queued (play, load, loop)
  vsk.want=null;vsk.busy=true;vsk.at=performance.now();vsk.exact=exact;
  try{vid.currentTime=t;}catch(_){vsk.busy=false;}
}
function vSeek(t,exact=true){
  if(!hasVideo())return;
  vsk.want={t,exact};
  // the 2 s guard keeps scrubbing alive if a 'seeked' event never arrives
  if(!vsk.busy||performance.now()-vsk.at>2000)vSeekNext();
}
function vSeekNext(){
  const w=vsk.want;vsk.want=null;vsk.busy=false;
  if(!w||(!vid.seeking&&Math.abs(vid.currentTime-w.t)<.0005))return;
  vSeekNow(w.t,w.exact);
}
vid.addEventListener('seeked',()=>{
  if(vsk.busy&&vsk.exact)vsk.cost=clamp(vsk.cost*.7+(performance.now()-vsk.at)*.3,20,600);   // how long exact seeks take on this clip
  vsk.busy=false;if(vsk.want)vSeekNext();
});
function resetSeeks(){vsk.busy=false;vsk.want=null;vsk.keys=null;vsk.cost=120;clearTimeout(vsk.settle);}
// the keyframe nearest to t, and the gap between the keyframes either side of it
function keyNear(t){
  const k=vsk.keys;if(!k||k.length<2)return null;
  let lo=0,hi=k.length-1;while(hi-lo>1){const m=(lo+hi)>>1;if(k[m]<=t)lo=m;else hi=m;}
  const a=k[lo],b=k[hi];return {k:t-a<=b-t?a:b,gap:Math.max(.05,b-a)};
}
function seek(t){
  ed.t=clamp(t,0,ed.dur);updatePlayhead();
  if(hasVideo()){if(ed.playing)vSeekNow(ed.t);else vSeek(ed.t);}
  if(ed.playing)syncAudio(true);
}
function scrubTo(t){
  const now=performance.now(),dt=(now-(drag.lt||0))/1000;
  if(drag.lt&&dt>.002){const v=Math.abs(t-drag.lastT)/dt;drag.vel=drag.vel==null?v:drag.vel*.6+v*.4;}   // clip seconds per second
  drag.lt=now;drag.lastT=t;
  ed.t=clamp(t,0,ed.dur);updatePlayhead();
  if(!hasVideo())return;
  // moving so fast that the exact frame would be half a GOP out of date by the time it decodes:
  // show the nearest keyframe now, and the exact frame 90 ms after the drag slows down or stops
  const n=keyNear(ed.t);
  if(n&&(drag.vel||0)*vsk.cost/1000>n.gap/2&&Math.abs(n.k-ed.t)>.02){
    vSeek(n.k+.001,false);   // just past the keyframe, so rounding can't land on the one before it
    clearTimeout(vsk.settle);vsk.settle=setTimeout(()=>{if(!ed.playing)vSeek(ed.t);},90);
  } else vSeek(ed.t);
}

/* playback: the playhead follows video.currentTime while playing (or a clock when there's no playable video) */
// frame scheduler: whichever fires first of rAF or a short timer, so loops keep going when frames are throttled
const nextFrame=f=>{let done=false;const run=()=>{if(done)return;done=true;f(performance.now());};requestAnimationFrame(run);setTimeout(run,40);};
let playGen=0,last=0,lastDrift=0;
function setPlayIcon(){$('#playBtn use').setAttribute('href',ed.playing?'#i-pause':'#i-play');$('#playBtn svg').classList.toggle('ico-play',!ed.playing);$('#playBtn').dataset.tip=(ed.playing?'Pause':'Play')+'  ·  Space';$('#preview').classList.toggle('playing',ed.playing);}
function play(){
  if(ed.playing||!ed.clip)return;
  if(ed.loop&&(ed.t<ed.inT||ed.t>=ed.outT-.03))ed.t=ed.inT;else if(ed.t>=ed.dur-.03)ed.t=0;
  ed.playing=true;setPlayIcon();
  if(hasVideo()){vsk.want=null;clearTimeout(vsk.settle);if(Math.abs(vid.currentTime-ed.t)>.01)vSeekNow(ed.t);vid.playbackRate=ed.speed;vid.play().catch(err=>{if(err&&err.name!=='AbortError')BT.log('warn','video play failed: '+errMsg(err));});}
  syncAudio(true);
  last=performance.now();const g=++playGen;nextFrame(n=>tick(n,g));
}
function pause(){
  if(!vid.paused)vid.pause();ed.audio.forEach(a=>{if(!a.el.paused)a.el.pause();});
  if(!ed.playing)return;ed.playing=false;playGen++;
  if(hasVideo()&&vOK)ed.t=clamp(vid.currentTime,0,ed.dur);
  setPlayIcon();updatePlayhead();
}
function toggle(){ed.playing?pause():play();}
function tick(now,g){
  if(!ed.playing||g!==playGen)return;
  if(hasVideo()){
    ed.t=vid.currentTime;
    if(ed.loop&&ed.t>=ed.outT){seek(ed.inT);}
    else if(vid.ended||ed.t>=ed.dur-.01){ed.t=ed.dur;updatePlayhead();pause();return;}
  } else {
    const dt=Math.min(.1,(now-last)/1000);last=now;ed.t+=dt*ed.speed;
    if(ed.loop&&ed.t>=ed.outT)seek(ed.inT);else if(ed.t>=ed.dur){ed.t=ed.dur;updatePlayhead();pause();return;}
  }
  if(now-lastDrift>250){lastDrift=now;syncAudio(false);}   // resync any track that drifted more than 80 ms
  updatePlayhead();
  if(ed.zoom>1){const x=ed.t*tl.pps,sl=els.scroll.scrollLeft,cw=els.scroll.clientWidth;if(x>sl+cw-40||x<sl)els.scroll.scrollLeft=x-60;}
  nextFrame(n=>tick(n,g));
}
$('#playBtn').addEventListener('click',toggle);
$('#goIn').addEventListener('click',()=>seek(ed.inT));
$('#goOut').addEventListener('click',()=>seek(ed.outT));
$('#stepB').addEventListener('click',()=>step(-1));
$('#stepF').addEventListener('click',()=>step(1));
function step(d,big){pause();seek(ed.t+d*(big?1:1/(ed.clip?fpsR(ed.clip):60)));}
$('#loopBtn').addEventListener('click',e=>{ed.loop=!ed.loop;e.currentTarget.classList.toggle('on',ed.loop);toast(ed.loop?'Looping the selection':'Loop off — plays to the end','loop');});
function setSpeed(s){ed.speed=s;$('#speedBtn').textContent=s+'×';vid.playbackRate=s;ed.audio.forEach(a=>a.el.playbackRate=s);if(ed.playing)syncAudio(true);}
$('#speedBtn').addEventListener('click',e=>openPop(e.currentTarget,[.25,.5,1,1.5,2].map(s=>({s,label:s+'×',on:ed.speed===s})),it=>setSpeed(it.s),'right'));
function syncPvVol(){const r=$('#pvVol');r.style.setProperty('--p',r.value+'%');$('#pvMute use').setAttribute('href',+r.value===0?'#i-vol-x':'#i-vol');$('#pvMute').dataset.tip=`Preview volume · ${r.value} %`;applyVolumes();}
$('#pvVol').value=lsGet('bt.previewVolume','80');syncPvVol();
$('#pvVol').addEventListener('input',()=>{syncPvVol();lsSet('bt.previewVolume',$('#pvVol').value);});
$('#pvMute').addEventListener('click',()=>{const r=$('#pvVol');r.value=+r.value===0?80:0;r.dispatchEvent(new Event('input'));});
$('.tp-vol').addEventListener('wheel',e=>{e.preventDefault();const r=$('#pvVol');r.value=clamp(+r.value+(e.deltaY<0?5:-5),0,100);r.dispatchEvent(new Event('input'));},{passive:false});
$('#keysBtn').addEventListener('click',e=>openKeys(e.currentTarget));
function toggleFullscreen(){
  if(document.fullscreenElement){document.exitFullscreen().catch(()=>{});return;}
  const p=$('#preview');if(p.requestFullscreen)p.requestFullscreen().catch(()=>toast('Full screen isn’t available here','expand'));
}
$('#fsBtn').addEventListener('click',toggleFullscreen);
document.addEventListener('fullscreenchange',()=>positionCrop());
$('#snapBtn').addEventListener('click',e=>{ed.snap=!ed.snap;e.currentTarget.classList.toggle('on',ed.snap);toast(ed.snap?'Snapping to playhead and markers':'Snapping off','magnet');});
function setZoom(z,anchorT){
  z=clamp(z,1,8);const at=anchorT==null?ed.t:anchorT;const sc=els.scroll;const screenX=at*tl.pps-sc.scrollLeft;
  ed.zoom=z;renderTimeline();sc.scrollLeft=Math.max(0,at*tl.pps-screenX);
}
$('#zoom').addEventListener('input',e=>setZoom(+e.target.value));
$('#zIn').addEventListener('click',()=>setZoom(ed.zoom*1.5));
$('#zOut').addEventListener('click',()=>setZoom(ed.zoom/1.5));
$('#zoomLbl').addEventListener('click',()=>setZoom(1));
els.scroll.addEventListener('wheel',e=>{if(e.ctrlKey){e.preventDefault();setZoom(ed.zoom*(e.deltaY<0?1.15:1/1.15),tAt(e));}},{passive:false});
function setIn(){const b=trimState();ed.inT=Math.max(0,Math.min(ed.t,ed.outT-MIN_SEL));pushUndo(b);updateSel();toast('In point set to '+fmt(ed.inT),'to-in');}
function setOut(){const b=trimState();ed.outT=Math.min(ed.dur,Math.max(ed.t,ed.inT+MIN_SEL));pushUndo(b);updateSel();toast('Out point set to '+fmt(ed.outT),'to-out');}
$('#inSet').addEventListener('click',setIn);
$('#outSet').addEventListener('click',setOut);
/* typeable In / Out: M:SS.ss (also 24.5, 24.5 s or 0:24), Enter applies, Esc reverts */
function parseTime(s){
  s=String(s).trim().replace(',','.').replace(/\s*s$/i,'');
  let m=s.match(/^(\d{1,3}):(\d{1,2}(?:\.\d*)?)$/);if(m&&parseFloat(m[2])<60)return +m[1]*60+parseFloat(m[2]);
  m=s.match(/^\d+(?:\.\d*)?$/);if(m)return parseFloat(s);
  return NaN;
}
function commitTime(which,inp){
  const v=parseTime(inp.value);
  if(isNaN(v)){inp.classList.add('bad');setTimeout(()=>inp.classList.remove('bad'),400);toast('Type a time like 0:24.50','alert');inp.value=fmt(which==='in'?ed.inT:ed.outT);return;}
  const prev=which==='in'?ed.inT:ed.outT,before=trimState();
  if(which==='in')ed.inT=Math.max(0,Math.min(v,ed.outT-MIN_SEL));else ed.outT=Math.min(ed.dur,Math.max(v,ed.inT+MIN_SEL));
  const now=which==='in'?ed.inT:ed.outT;inp.value=fmt(now);
  if(Math.abs(now-prev)>.004){pushUndo(before);updateSel();if(Math.abs(now-v)>.004)toast(`${which==='in'?'In':'Out'} kept at ${fmt(now)} (selection must be at least ${MIN_SEL} s, within the clip)`,'info');}
}
[['in','#inV'],['out','#outV']].forEach(([w,s])=>{const inp=$(s);
  inp.addEventListener('focus',()=>setTimeout(()=>inp.select(),0));
  inp.addEventListener('keydown',e=>{if(e.key==='Enter'){e.preventDefault();commitTime(w,inp);inp.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();inp.value=fmt(w==='in'?ed.inT:ed.outT);inp.blur();}else if(e.key==='Tab'){commitTime(w,inp);}});
  inp.addEventListener('blur',()=>{if(ed.clip&&inp.value!==fmt(w==='in'?ed.inT:ed.outT))commitTime(w,inp);});
});
$('#resetTrim').addEventListener('click',()=>{const b=trimState();ed.inT=0;ed.outT=ed.dur;pushUndo(b);updateSel();toast('Selected the full clip','scissors',UNDO);});

/* ---------- canvas + crop (sent with the export request; remembered for this session) */
function setCanvas(c){ed.canvas=c;$$('.copt').forEach(b=>b.classList.toggle('on',b.dataset.canvas===c));$('#canvasSum').textContent=c==='16:9'?`Original · ${ed.clip?aspectLbl(ed.clip):'16:9'}`:'Vertical · 9:16';$('#crop').hidden=c==='16:9';syncVideoSub();positionCrop();updateEstimates();}
$$('.copt').forEach(b=>b.addEventListener('click',()=>{setCanvas(b.dataset.canvas);rememberMix();}));
function positionCrop(){const p=$('#preview'),w=$('#cropWin');if(!p.clientWidth)return;const cw=p.clientHeight*9/16,max=p.clientWidth-cw;w.style.left=(ed.cropX*max)+'px';}
$('#cropWin').addEventListener('pointerdown',e=>{const p=$('#preview'),x0=e.clientX,c0=ed.cropX,max=p.clientWidth-p.clientHeight*9/16;e.preventDefault();
  const mv=ev=>{ed.cropX=clamp(c0+(ev.clientX-x0)/max,0,1);positionCrop();};window.addEventListener('pointermove',mv);window.addEventListener('pointerup',()=>{window.removeEventListener('pointermove',mv);rememberMix();},{once:true});});

/* ---------- inspector: sections open in priority order while they fit above the pinned Export card */
let insUser=false,fitT=0;
function fitInspector(){
  if(insUser||$('#edMain').classList.contains('collapsed'))return;
  const ins=$('#inspector'),sc=$('.ins-scroll'),card=$('.export-card');if(!ins.clientHeight)return;
  ins.classList.add('no-anim');
  const avail=ins.clientHeight-card.offsetHeight;
  $$('.ins-sec').forEach(s=>{s.classList.add('open');if(sc.scrollHeight>avail+1)s.classList.remove('open');});
  void sc.offsetHeight;
  nextFrame(()=>nextFrame(()=>ins.classList.remove('no-anim')));
}
const fitSoon=()=>{clearTimeout(fitT);fitT=setTimeout(fitInspector,140);};
$$('.ins-head').forEach(h=>h.addEventListener('click',()=>{insUser=true;h.parentElement.classList.toggle('open');}));
function setPanel(open){$('#edMain').classList.toggle('collapsed',!open);$('#panelBtn').classList.toggle('on',open);$('#panelBtn').dataset.tip=open?'Hide side panel':'Show side panel';nextFrame(()=>{renderTimeline();positionCrop();fitInspector();});}
$('#panelBtn').addEventListener('click',()=>setPanel($('#edMain').classList.contains('collapsed')));
$$('[data-rail]').forEach(b=>b.addEventListener('click',()=>{insUser=true;setPanel(true);$$('.ins-sec').forEach(s=>s.classList.toggle('open',s.dataset.sec===b.dataset.rail));}));

/* ---------- clip bar + inspector actions */
$('#railExport').addEventListener('click',()=>exportNow());
// split button: main part exports right away with the current size; the chevron opens the full options dialog
$('#exportBtn').addEventListener('click',()=>exportNow());
$('#exportOpts').addEventListener('click',()=>go('export'));
$('#saveCopy').addEventListener('click',async()=>{
  const b=$('#saveCopy'),c=ed.clip;if(!c||b.disabled)return;
  b.disabled=true;$('#saveCopyLbl').textContent='Saving trimmed copy…';
  try{
    const r=await BT.saveTrimmedCopy(c.id,r3(ed.inT),r3(ed.outT));
    c.exports=[{path:r.path,sizeBytes:r.sizeBytes,preset:'lossless',width:c.width,height:c.height,fps:fpsR(c),at:Date.now()},...(c.exports||[])];
    if(ed.clip===c)renderActivity();
    toast(`Saved a trimmed copy · ${fmtMB(MBof(r.sizeBytes))}`,'scissors',{label:'Show in folder',icon:'folder',fn:()=>BT.reveal(r.path).catch(e=>fail("Couldn't open the folder",e))});
  }catch(e){fail("Couldn't save a trimmed copy",e);}
  finally{b.disabled=false;$('#saveCopyLbl').textContent='Save trimmed copy';}
});
$('#ecSeg').addEventListener('click',e=>{const b=e.target.closest('[data-p]');if(!b)return;ed.preset=b.dataset.p;updateEstimates();});
$('#copyPath').addEventListener('click',async()=>{if(!ed.clip)return;toast(await copyText(ed.clip.path)?'File path copied':'Couldn’t copy the path','copy');});
const revealClip=()=>{if(ed.clip)BT.reveal(ed.clip.path).catch(e=>fail("Couldn't open the folder",e));};
$('#openFolder').addEventListener('click',revealClip);
$('#locBtn').addEventListener('click',revealClip);
$('#favBtn').addEventListener('click',()=>toggleFav(ed.clip));
function syncFav(){const b=$('#favBtn'),f=!!(ed.clip&&ed.clip.fav);b.classList.toggle('on',f);$('use',b).setAttribute('href',f?'#i-star-f':'#i-star');b.style.color=f?'#f4c65a':'';b.dataset.tip=f?'Remove from Favourites':'Favourite';}
$('#delBtn').addEventListener('click',()=>trashClip(ed.clip));
$('#backLib').addEventListener('click',()=>{pause();go('library');});
function gameList(){const g=ed.clip&&S.G[ed.clip.gameId];return g?g.clips:[];}
$('#prevClip').addEventListener('click',()=>{const l=gameList(),i=l.indexOf(ed.clip);if(i>0)openClip(l[i-1]);});
$('#nextClip').addEventListener('click',()=>{const l=gameList(),i=l.indexOf(ed.clip);if(i>=0&&i<l.length-1)openClip(l[i+1]);});

function openClip(c){
  if(!c)return;
  pause();flushSave();ed.gen++;
  const dur=c.duration,e=c.edits,mem=mixMem.get(c.id);
  ed.clip=c;ed.dur=dur;
  let a=0,b=dur;
  if(e&&isFinite(e.inT)&&isFinite(e.outT)){a=clamp(e.inT,0,dur);b=clamp(e.outT,0,dur);if(b-a<Math.min(MIN_SEL,dur)){a=0;b=dur;}}
  ed.inT=a;ed.outT=b;ed.t=a;
  ed.markers=(e&&e.markers||[]).filter(m=>isFinite(m)&&m>=0&&m<=dur).sort((x,y)=>x-y);
  ed.zoom=1;ed.preset=S.settings.defaultPreset;ed.customMB=S.settings.customMb;
  ed.canvas=mem?mem.canvas:'16:9';ed.cropX=mem?mem.cropX:.5;
  ed.tracks=trackState(c,mem);
  hist.u.length=0;hist.r.length=0;hideGhost();
  setSaved(e?'ok':'none');
  const pv=$('#pvImg'),tu=c.thumb?BT.src(c.thumb):'';
  if(tu){pv.src=tu;pv.hidden=false;$('#pvPh').hidden=true;}else{pv.removeAttribute('src');pv.hidden=true;$('#pvPh').hidden=false;}
  syncClipChrome();
  $('#tTot').textContent='of '+fmt(dur);
  loadMedia(c,ed.gen);
  buildTracks();setCanvas(ed.canvas);
  els.scroll.scrollLeft=0;
  nextFrame(()=>{renderTimeline();positionCrop();updateEstimates();fitInspector();});
  updateCrumb();
}
/* everything in the clip bar / inspector that comes straight from the Clip object */
function syncClipChrome(){
  const c=ed.clip,g=S.G[c.gameId],d=new Date(c.recordedAt),dir=dirOf(c.path),n=c.audioTracks.length;
  $('#cbIcoWrap').innerHTML=gameIcon(g||{name:c.game,thumb:c.thumb});
  $('#cbTitle').textContent=`${c.game} — ${whenLabel(d)}`;$('#cbName').title=`${c.game} — ${whenLabel(d)}`;
  $('#mCreated').textContent=longDate(d);$('#mLen').textContent=fmtLen(c.duration);$('#mQual').textContent=qualLbl(c);$('#mSizeSrc').textContent=fmtMB(MBof(c.sizeBytes));
  $('#locLbl').textContent=shortPath(dir,26);$('#locBtn').dataset.tip=c.path;
  const l=gameList(),i=l.indexOf(c);$('#cbCount').textContent=i>=0?`${i+1} of ${l.length}`:'';
  $('#prevClip').style.opacity=i>0?1:.4;$('#nextClip').style.opacity=i>=0&&i<l.length-1?1:.4;
  $('#coptOrig').textContent=`${aspectLbl(c)} · ${c.height}p`;
  $('#detKv').innerHTML=[['Game',esc(c.game)],['Recorded',longDate(d)],['Length',fmt(c.duration)],['Video',`${c.width}×${c.height} · ${fpsR(c)} fps`],['Bitrate',fmtK(c.videoKbps)],
    ['Audio',`${n} track${n===1?'':'s'}${n?' · '+c.audioTracks.map(t=>esc(trackLabel(c,t.index))).join(', '):''}`],['Source',c.source==='gg'?'SteelSeries GG':'Backtrack']].map(([k,v])=>`<dt>${k}</dt><dd>${v}</dd>`).join('');
  $('#detSum').textContent=`${fmtK(c.videoKbps)} · ${n} audio track${n===1?'':'s'}`;
  $('#fileName').textContent=c.fileName;$('#fileName').title=c.path;$('#filePath').textContent=shortPath(dir,40);$('#filePath').title=dir;
  renderActivity();syncFav();
}
function closeClip(){pause();flushSave();ed.gen++;unloadMedia();ed.clip=null;}
const PRESET_LBL={d10:'Discord · 10 MB',d50:'Discord · 50 MB',d500:'Discord · 500 MB',orig:'Original quality',custom:'Custom size',lossless:'Trimmed copy · lossless'};
function renderActivity(){
  const ex=ed.clip.exports||[];
  $('#actSum').textContent=ex.length?`${ex.length} export${ex.length>1?'s':''} · last ${ago(ex[0].at)}`:'Not shared yet';
  $('#actBody').innerHTML=ex.length?ex.slice(0,12).map((x,i)=>`<div class="act-item" data-reveal="${i}" title="Show ${esc(baseName(x.path))} in its folder"><span class="ai">${ic(x.preset==='lossless'?'scissors':'upload','sm')}</span><div><b>${esc(PRESET_LBL[x.preset]||x.preset)} · ${fmtMB(MBof(x.sizeBytes))}</b><span>${x.height}p${x.fps} · ${ago(x.at)}</span></div></div>`).join('')+(ex.length>12?`<div class="muted" style="font-size:12px;padding:4px 0">+${ex.length-12} older</div>`:'')
    :`<div class="act-empty">${ic('share','sm')}<span>Exports of this clip show up here with their size and when you made them.</span></div>`;
}
$('#actBody').addEventListener('click',e=>{const it=e.target.closest('[data-reveal]');if(!it||!ed.clip)return;const x=ed.clip.exports[+it.dataset.reveal];if(x)BT.reveal(x.path).catch(err=>fail("Couldn't show the file",err));});

/* ================================================================== EXPORT MATHS (mirrors the backend, see API.md) */
const LADDER=[
  {k:6000,res:1080,fps:60,q:'Excellent',lv:5},
  {k:3500,res:1080,fps:30,q:'Great',lv:4},
  {k:2000,res:720,fps:60,q:'Good',lv:3},
  {k:1500,res:720,fps:30,q:'Fair',lv:2},
  {k:1000,res:480,fps:30,q:'Low',lv:1},
  {k:0,res:480,fps:30,q:'Poor',lv:0},
];
const COPY_AK=192;   // copy mode: video stream-copied, audio mixed to one AAC 192 kbps track
const PRESETS=[
  {id:'d10',name:'Discord',sub:'Free upload limit',limit:()=>10,limitLbl:()=>'10 MB',tag:'Default'},
  {id:'d50',name:'Discord Nitro Basic',sub:'50 MB upload limit',limit:()=>50,limitLbl:()=>'50 MB'},
  {id:'d500',name:'Discord Nitro',sub:'500 MB upload limit',limit:()=>500,limitLbl:()=>'500 MB'},
  {id:'orig',name:'Original quality',sub:'No re-encode · exact copy',limit:()=>null},
  {id:'custom',name:'Custom size',sub:'Set your own limit',limit:()=>ed.customMB,limitLbl:()=>fmtNum(ed.customMB)+' MB'},
];
const P=id=>PRESETS.find(p=>p.id===id)||PRESETS[0];
// o lets callers estimate another clip / length / limit (the library's Quick send, a running export)
function estimate(pid=ed.preset,o={}){
  const c=o.clip||ed.clip,dur=Math.max(.1,o.dur!=null?o.dur:ed.outT-ed.inT),vert=o.vert!=null?o.vert:ed.canvas==='9:16';
  const limit=o.limit!==undefined?o.limit:P(pid).limit();
  const srcK=c.sizeBytes*8/1000/c.duration,srcRes=c.height,srcFps=fpsR(c);
  const copyMB=(c.videoKbps+COPY_AK)*dur/8000,selMB=MBof(c.sizeBytes)*dur/c.duration;
  const base={pid,dur,limit,srcK,vert,clip:c,selMB};
  if(pid==='orig'||(limit&&copyMB<=limit*.95&&!vert)){
    return Object.assign(base,{mode:'copy',size:copyMB,res:srcRes,fps:srcFps,vk:c.videoKbps,ak:COPY_AK,q:'Original',lv:5,fits:limit?copyMB<=limit:copyMB<=10,encSec:Math.max(1,dur/40)});
  }
  const target=limit*.95;let total=target*8000/dur,ak=128,vk=total-ak;if(vk<1500){ak=96;vk=total-ak;}
  let capped=false;if(vk>20000){vk=20000;capped=true;}
  const L=LADDER.find(l=>vk>=l.k)||LADDER[LADDER.length-1];
  const size=capped?(vk+ak)*dur/8000:target*.99;
  const thresholds=LADDER.slice(0,5).map(l=>Object.assign({},l,{maxDur:target*8000/(l.k+(l.k<1500?96:128))}));
  // the tier never upscales past the source resolution or frame rate
  return Object.assign(base,{mode:'encode',size,res:Math.min(L.res,srcRes),fps:Math.min(L.fps,srcFps),vk,ak,q:L.q,lv:L.lv,tier:L,fits:true,target,thresholds,encSec:Math.max(3,dur*.45)});
}
const even=n=>Math.max(2,Math.round(n/2)*2);
function resLbl(e){const c=e.clip;if(e.mode==='copy')return `${c.width} × ${c.height}`;const h=e.res;return e.vert?`${even(h*9/16)} × ${h}`:`${even(h*c.width/c.height)} × ${h}`;}
const tierLbl=(res,fps)=>`${res}p${fps}`;
let measureCtx=null;
function textW(t,font){if(!measureCtx)measureCtx=document.createElement('canvas').getContext('2d');measureCtx.font=`${font} ${getComputedStyle(document.body).fontFamily}`;return measureCtx.measureText(t).width;}
/* trim nudge (UX review r2 #5): only for a real upgrade, where neither resolution nor frame rate gets worse
   (720p60 → 1080p60, 720p30 → 720p60, …). Sideways moves (720p60 → 1080p30) aren't worth cutting the moment for. */
function upgradeFor(e){
  const idx=LADDER.indexOf(e.tier);if(idx<=0)return null;
  const capT=l=>({res:Math.min(l.res,e.clip.height),fps:Math.min(l.fps,fpsR(e.clip))});
  for(let j=idx-1;j>=0;j--){
    const n=capT(LADDER[j]);
    if(n.res>=e.res&&n.fps>=e.fps&&(n.res>e.res||n.fps>e.fps)){const s=Math.floor(e.thresholds[j].maxDur);return s>=3&&s<e.dur?{j,res:n.res,fps:n.fps,s}:null;}
  }
  return null;
}
function advice(e){
  if(e.mode==='copy'){
    if(e.pid==='orig'&&e.size>10)return {kind:'warn',icon:'alert',title:`${fmtMB(e.size)} is over Discord's free 10 MB limit`,text:'Fine with Nitro. To share without it, use the Discord · 10 MB preset.',fix:{label:'Use 10 MB',preset:'d10'}};
    return {kind:'ok',icon:'check-c',title:'No re-encode needed',text:`Your ${e.dur.toFixed(1)} s selection already fits, so the video is copied exactly (${tierLbl(e.res,e.fps)}) and the audio tracks are mixed into one.${e.vert?' Original quality is an exact copy, so it stays in its original shape.':''}`};
  }
  if(e.lv<=1){const t=Math.floor(e.thresholds[2].maxDur);return {kind:e.lv===0?'bad':'warn',icon:'alert',title:e.lv===0?`Too long for ${fmtNum(e.limit)} MB`:`This will look blurry (${tierLbl(e.res,e.fps)})`,text:`${e.dur.toFixed(1)} s only gets ${fmtK(Math.max(0,e.vk))} at ${fmtNum(e.limit)} MB.${t>=1?` Trim to ${t} s or less for 720p60.`:' Pick a bigger size limit.'}`,fix:t>=1?{label:`Trim to ${t} s`,trim:t}:null};}
  const up=upgradeFor(e);
  if(up)return {kind:'info',icon:'bolt',title:`Want ${tierLbl(up.res,up.fps)}?`,text:`Trim to ${up.s} s or less and it fits at ${tierLbl(up.res,up.fps)}.`,fix:{label:`Trim to ${up.s} s`,trim:up.s}};
  return {kind:'ok',icon:'check-c',title:'Sharpest it can be at this length',text:`${tierLbl(e.res,e.fps)} at ${fmtK(e.vk)}. This will look sharp in Discord.`};
}
function fitState(e){
  if(e.mode==='copy'&&!e.fits)return ['bad','alert','Over 10 MB'];
  if(e.mode==='copy')return ['ok','check','Fits'];
  if(e.lv===0)return ['bad','alert','Too long'];
  if(e.lv===1)return ['warn','alert','Low quality'];
  return ['ok','check','Fits'];
}
function qbars(el,e){const k=e.lv<=1?(e.lv===0?'bad':'warn'):e.lv===2?'warn':'';el.className='qbars '+k;$$('i',el).forEach((b,i)=>b.classList.toggle('on',i<Math.max(1,e.lv)));}
/* One-click "Trim to N s": shorten the selection around what matters instead of cutting from the end.
   1. Markers inside the range: keep as many as possible and centre on them (ties go to the later group).
   2. No markers: centre on the playhead if it is inside the range.
   3. Otherwise keep the end (replay clips end just after the action).
   o = {a, b, t, markers}; defaults to the editor's selection. Also used by the library's Quick send. */
function planTrim(sec,o){
  o=o||{a:ed.inT,b:ed.outT,t:ed.t,markers:ed.markers};
  const a=o.a,b=o.b,len=Math.min(sec,b-a);
  const mk=(o.markers||[]).filter(t=>t>=a&&t<=b).sort((x,y)=>x-y);
  let c=null,why='end',kept=0;
  if(mk.length){
    let bi=0,bj=0;
    for(let i=0;i<mk.length;i++){let j=i;while(j+1<mk.length&&mk[j+1]-mk[i]<=len)j++;if(j-i+1>=kept){kept=j-i+1;bi=i;bj=j;}}
    c=(mk[bi]+mk[bj])/2;why='markers';
  } else if(o.t!=null&&o.t>a+.05&&o.t<b-.05){c=o.t;why='playhead';}
  let s=c==null?b-len:c-len/2;
  s=Math.round(s*10)/10;s=Math.max(a,Math.min(b-len,s));
  return {inT:s,outT:s+len,len,why,kept,total:mk.length};
}
function planWhy(p){
  if(p.why==='markers'){if(p.kept===p.total)return p.kept===1?'around your marker':p.kept===2?'around both markers':`around all ${p.kept} markers`;return `around ${p.kept} of your ${p.total} markers`;}
  return p.why==='playhead'?'centred on the playhead':'keeping the ending';
}
function applyTrim(sec){
  const p=planTrim(sec),b=trimState();ed.inT=p.inT;ed.outT=p.outT;
  pushUndo(b);hideGhost();updateSel();if(ed.t<ed.inT||ed.t>ed.outT)seek(ed.inT);
  toast(`Trimmed to ${p.len} s (${fmt(p.inT,1)} – ${fmt(p.outT,1)}), ${planWhy(p)}`,'scissors',UNDO);
}
function showGhost(sec){
  if(!tl.W||cur()!=='editor')return;const p=planTrim(sec),g=$('#ghost');
  g.style.left=(p.inT*tl.pps)+'px';g.style.width=((p.outT-p.inT)*tl.pps)+'px';
  $('#ghostLbl').innerHTML=`${ic('scissors','xs')}${p.len} s · ${fmt(p.inT,1)} – ${fmt(p.outT,1)} · ${planWhy(p)}`;g.hidden=false;
}
function hideGhost(){const g=$('#ghost');if(g)g.hidden=true;}
/* one-line version of advice() for the editor's Export card */
function tipFor(e){
  if(S.settings&&!S.settings.ffmpegPath)return {kind:'bad',icon:'alert',html:`<b>FFmpeg not found</b> · exports won't work`};
  const a=advice(e);
  if(e.mode==='copy'){
    if(a.fix)return {kind:'warn',icon:'alert',html:`<b>Over 10 MB</b> · needs Nitro`,fix:a.fix};
    return {kind:'ok',icon:'check-c',html:`<b>No re-encode</b> · exact ${tierLbl(e.res,e.fps)} copy`};
  }
  if(e.lv<=1)return {kind:e.lv===0?'bad':'warn',icon:'alert',html:`<b>${e.lv===0?'Too long':'Low quality'}</b> · ${tierLbl(e.res,e.fps)}`,fix:a.fix};
  if(a.fix&&a.fix.trim){const up=upgradeFor(e),more=Math.max(1,Math.ceil(e.dur-a.fix.trim));return {kind:'info',icon:'bolt',html:`Trim <b>${more} s</b> more for <b>${tierLbl(up.res,up.fps)}</b>`,fix:a.fix};}
  return {kind:'ok',icon:'check-c',html:`<b>Sharpest it can be</b> at this length`};
}
function updateEstimates(){
  if(!ed.clip)return;const e=estimate();
  $$('#ecSeg [data-p]').forEach(b=>{const on=b.dataset.p===ed.preset;b.classList.toggle('on',on);b.setAttribute('aria-checked',on);});
  $('#ecSize').textContent=fmtMB(e.size);
  // say which limit this is in words ("free" alone read as free space / free of charge)
  const tierName={d10:'Discord · no Nitro',d50:'Discord Nitro Basic',d500:'Discord Nitro'}[ed.preset]||'Custom limit',tierFull={d10:'Discord’s upload limit without Nitro',d50:'Discord Nitro Basic upload limit',d500:'Discord Nitro upload limit'}[ed.preset]||'Your custom size limit';
  const of=$('#ecOf');of.innerHTML=e.limit?`of ${fmtNum(e.limit)} MB<small>${tierName}</small>`:`no size limit<small>Original quality copy</small>`;of.dataset.tip=e.limit?tierFull:'Exact copy of your selection, no re-encode';
  const tp=tipFor(e),tip=$('#ecTip');tip.className='ec-tip '+tp.kind;hideGhost();
  const plan=tp.fix&&tp.fix.trim?planTrim(tp.fix.trim):null;
  tip.innerHTML=`${ic(tp.icon,'sm')}<span class="tt">${tp.html}</span>${tp.fix?`<button class="btn sm" id="ecFix"${plan?` data-tip="${fmt(plan.inT,1)} – ${fmt(plan.outT,1)} · ${planWhy(plan)}" data-tip-pos="l"`:''}>${tp.fix.label}</button>`:''}`;
  if(tp.fix){const fb=$('#ecFix');
    fb.onclick=()=>{if(tp.fix.trim)applyTrim(tp.fix.trim);if(tp.fix.preset){ed.preset=tp.fix.preset;updateEstimates();}};
    // plan is recomputed on hover: the playhead may have moved since this card was drawn
    if(tp.fix.trim){fb.onmouseenter=fb.onfocus=()=>{const p=planTrim(tp.fix.trim);fb.dataset.tip=`${fmt(p.inT,1)} – ${fmt(p.outT,1)} · ${planWhy(p)}`;showGhost(tp.fix.trim);};fb.onmouseleave=fb.onblur=hideGhost;}}
  const [fk,fi,fl]=fitState(e);const fit=$('#ecFit');fit.className='fit '+fk;fit.innerHTML=ic(fi,'xs')+fl;
  const lim=e.limit||10;$('#ecFill').style.width=Math.min(100,e.size/lim*100)+'%';$('#ecMeter').className='meter'+(fk==='bad'?' bad':fk==='warn'?' warn':'');
  $('#ecSpec').textContent=e.mode==='copy'?`${tierLbl(e.res,e.fps)} · ${fmtK(e.vk)} · no re-encode`:`${tierLbl(e.res,e.fps)}${e.vert?' vertical':''} · ${fmtK(e.vk)}`;
  qbars($('#ecQ'),e);$('#ecQl').textContent=e.q;
  $('#railSize').textContent=fmtMB(e.size).replace(' MB','MB');
  $('#saveCopyMeta').textContent=` · lossless · ${fmtMB(e.selMB)}`;
  if(!$('#modalLayer').hidden&&$('#modal').dataset.view==='setup')renderModal();
}

/* ================================================================== EXPORT MODAL */
function renderPresets(){
  $('#presetList').innerHTML=PRESETS.map(p=>{const e=estimate(p.id);
    const right=e.mode==='copy'?`<b>${fmtMB(e.size)}</b><span>${tierLbl(e.res,e.fps)} · copy</span>`:`<b>${tierLbl(e.res,e.fps)}</b><span>${fmtK(e.vk)}</span>`;
    return `<button class="preset${ed.preset===p.id?' on':''}" data-preset="${p.id}"><span class="radio"></span>
      <span style="min-width:0"><span class="p-name">${p.name}${p.limitLbl&&p.id!=='custom'?` · ${p.limitLbl()}`:''}${p.id===S.settings.defaultPreset?'<span class="tag">Default</span>':''}</span><span class="p-sub" style="display:block">${p.sub}</span>
      ${p.id==='custom'&&ed.preset==='custom'?`<span class="p-custom"><input type="number" id="customMB" min="1" max="2000" value="${ed.customMB}"><span class="muted">MB</span></span>`:''}</span>
      <span class="p-right">${right}</span></button>`;}).join('');
}
function renderModal(){
  const c=ed.clip,e=estimate();
  $('#mSub').innerHTML=`${esc(c.game)} — ${whenLabel(new Date(c.recordedAt))} · <span class="tnum">${fmt(ed.inT)} → ${fmt(ed.outT)}</span> · <b>${e.dur.toFixed(1)} s</b>${ed.canvas==='9:16'?' · vertical 9:16':''}`;
  const focused=document.activeElement&&document.activeElement.id==='customMB';
  if(!focused)renderPresets();
  $('#mSize').textContent=fmtMB(e.size);
  $('#mOf').textContent=e.limit?`of ${fmtNum(e.limit)} MB limit`:'no size limit';
  const [fk,fi,fl]=fitState(e);const b=$('#mBadge');b.className='badge '+fk;b.innerHTML=ic(fi,'xs')+(fk==='ok'?(e.limit?`Fits · ${(e.limit-e.size).toFixed(1)} MB spare`:'Fits Nitro'):fl);
  const lim=e.limit||Math.max(10,e.size);$('#mFill').style.width=Math.min(100,e.size/lim*100)+'%';$('#mMeter').className='meter lg'+(fk==='bad'?' bad':fk==='warn'?' warn':'');
  $('#mScale').textContent=e.limit?`${fmtNum(e.limit)} MB limit`:`${fmtMB(lim)}`;
  const specs=e.mode==='copy'?
    [['Resolution',resLbl(e)],['Frame rate',e.fps+' fps'],['Video bitrate',fmtKfull(e.vk)],['Audio',`${COPY_AK} kbps AAC · mixed`],['Quality',`<span class="qbars" id="mQ"><i></i><i></i><i></i><i></i><i></i></span>Original`],['Export time',`≈ ${Math.round(e.encSec)} s · stream copy`]]:
    [['Resolution',resLbl(e)],['Frame rate',e.fps+' fps'],['Video bitrate',fmtKfull(e.vk)],['Audio',`${e.ak} kbps AAC · mixed`],['Quality',`<span class="qbars" id="mQ"><i></i><i></i><i></i><i></i><i></i></span>${e.q}`],['Export time',`≈ ${Math.round(e.encSec)} s · x264 2-pass`]];
  $('#mSpecs').innerHTML=specs.map(([k,v])=>`<div><span class="k">${k}</span><span class="v">${v}</span></div>`).join('');
  qbars($('#mQ'),e);
  const lad=$('#ladder');
  if(e.mode==='copy'){
    lad.innerHTML=`<div class="ladder-head"><span class="cap">How it's made</span></div><div class="copy-note">${ic('bolt')}<span>The selected ${e.dur.toFixed(1)} s of video are copied straight from the original file, not re-encoded, so it's fast and looks exactly like the recording. The audio tracks are mixed into one, because Discord only plays the first.</span></div>`;
  } else {
    const th=e.thresholds,maxS=Math.max(th[4].maxDur*1.16,e.dur*1.08);
    const bounds=[0,...th.map(t=>Math.min(t.maxDur,maxS)),maxS];
    const names=['1080p60','1080p30','720p60','720p30','480p30','Too long'];
    const cols=['#2bd9b8','#35c7a5','#7fd28a','#e9c25a','#f39150','#ff5d6c'];
    const idx=LADDER.indexOf(e.tier),zi=Math.min(idx,5);
    // a block only gets its label when the text plus 8 px fits (UX review r2 #8); the rest keep a tooltip
    const barW=Math.max(200,(lad.clientWidth||560)-32-10);
    const zones=names.map((n,i)=>{const w=(bounds[i+1]-bounds[i])/maxS*100;if(w<=0)return '';
      const fits=w/100*barW>=textW(n,'700 11px')+8+2;   // 4 px padding each side, plus a hair
      return `<div class="lz${i===zi?' on':''}" style="width:${w}%;--z:${cols[i]}" title="${n}: ${i<5?`up to ${Math.floor(Math.min(th[i].maxDur,9999))} s`:'beyond the 480p limit'}"${i===zi?' aria-current="true"':''}>${fits?n:''}</div>`;}).join('');
    const ticks=th.map(t=>t.maxDur<maxS?`<span style="left:${t.maxDur/maxS*100}%">${Math.floor(t.maxDur)} s</span>`:'').join('');
    lad.innerHTML=`<div class="ladder-head"><span class="cap">Length vs. quality at ${fmtNum(e.limit)} MB</span><span>Your selection <b>${e.dur.toFixed(1)} s</b></span></div>
      <div class="lbar">${zones}<div class="lmark" style="left:${Math.min(99.5,e.dur/maxS*100)}%"></div></div><div class="lticks">${ticks}</div>`;
  }
  const a=advice(e),co=$('#mCallout');co.className='callout '+a.kind;
  // the timeline is behind the scrim here, so the callout spells out which part the one-click trim keeps
  const pl=a.fix&&a.fix.trim?planTrim(a.fix.trim):null;
  co.innerHTML=`${ic(a.icon,'lg')}<div class="ct"><b>${a.title}</b>${a.text}${pl?` One click picks <span class="tnum" style="color:var(--text-1);font-weight:600;white-space:nowrap">${fmt(pl.inT,1)} – ${fmt(pl.outT,1)}</span>, ${planWhy(pl)}.`:''}</div>${a.fix?`<button class="btn sm" id="fixBtn">${a.fix.label}</button>`:''}`;
  if(a.fix)$('#fixBtn').onclick=()=>{if(a.fix.trim)applyTrim(a.fix.trim);if(a.fix.preset){ed.preset=a.fix.preset;updateEstimates();}};
  $('#audioMix').innerHTML=ed.tracks.length?ed.tracks.map(t=>`<span class="amx${t.muted?' off':''}" style="--c:var(${t.c})"><i></i>${esc(t.name)}${t.muted?'':` ${t.vol}%`}</span>`).join(''):'<span class="amx off"><i></i>No audio</span>';
  $('#mExport').innerHTML=`${ic('upload')}${e.mode==='copy'?'Export copy':'Export'}<kbd>Enter</kbd>`;
}
$('#presetList').addEventListener('click',e=>{const b=e.target.closest('[data-preset]');if(!b||e.target.closest('input'))return;ed.preset=b.dataset.preset;updateEstimates();if(ed.preset==='custom'){const i=$('#customMB');i&&i.focus();i&&i.select();}});
$('#presetList').addEventListener('input',e=>{if(e.target.id==='customMB'){const v=parseFloat(e.target.value);if(v>.5){ed.customMB=Math.min(2000,v);updateEstimates();}}});
$('#presetList').addEventListener('change',e=>{if(e.target.id==='customMB'&&ed.customMB!==S.settings.customMb)saveSettings({customMb:ed.customMB});});
function defaultName(){const c=ed.clip;return `${c.fileName.replace(/\.mp4$/i,'')}_${ed.preset==='orig'?'trim':'discord'}`;}

/* ---------- running an export: ExportRequest → export_clip, export-progress events, ExportResult */
const exp={running:false,gen:0,req:null,est:null,result:null,error:null,direct:false,inflight:null};
function buildRequest(range){
  const c=ed.clip,limit=ed.preset==='orig'?null:P(ed.preset).limit();
  return {clipId:c.id,start:r3(range?range.start:ed.inT),end:r3(range?range.end:ed.outT),preset:ed.preset,limitMb:limit,
    canvas:range&&range.quick?'16:9':ed.canvas,cropX:r3(ed.cropX),
    tracks:ed.tracks.map(t=>({index:t.index,volume:Math.round(t.vol)/100,muted:!!t.muted})),
    fileName:($('#fname').value||'').trim()||defaultName(),copyToClipboard:!!$('#autoCopy').checked};
}
const reqEstimate=req=>{const c=S.byId[req.clipId]||ed.clip;return estimate(req.preset,{clip:c,dur:req.end-req.start,vert:req.canvas==='9:16',limit:req.limitMb});};
/* export right away with the current size (split button, E, rail, library Quick send) */
function exportNow(range){
  if(!ed.clip)return;
  if(!range){
    // "Too long" never exports blind (UX review r2 #4): open the options with the fix focused instead
    const e=estimate();
    if(e.mode==='encode'&&e.lv===0){go('export');nextFrame(()=>{const b=$('#fixBtn');if(b)b.focus();});toast(`Too long for ${fmtNum(e.limit)} MB · trim it or pick a bigger size first`,'alert');return;}
  }
  pause();exp.direct=true;$('#fname').value=defaultName();$('#autoCopy').checked=S.settings.autoCopy;
  runExport(buildRequest(range));
}
function quickSend(c){if(!c)return;openClip(c);const p=quickPlan(c);exportNow({start:p.inT,end:p.outT,quick:true});}
function renderProgressStart(req){
  const c=S.byId[req.clipId]||ed.clip,e=exp.est;
  $('#pThumbWrap').innerHTML=thumbHTML(c&&c.thumb,{eager:true});
  $('#pFill').style.width='0%';$('#pScan').style.width='0%';$('#pPct').textContent='0%';
  $('#pStage').textContent=e.mode==='copy'?'Copying your selection…':'Pass 1 of 2 · Analysing…';
  $('#pEta').textContent='Estimating time…';
  $('#pNext').textContent=req.copyToClipboard?'When it’s done, the file goes straight to your clipboard':'When it’s done, click the file to copy it';
}
function stageLabel(stage){
  const e=exp.est;
  switch(stage){
    case 'analyse':return 'Pass 1 of 2 · Analysing…';
    case 'encode':return `Pass 2 of 2 · Encoding ${tierLbl(e.res,e.fps)} at ${fmtK(e.vk)}…`;
    case 'copy':return 'Copying your selection…';
    case 'finish':return 'Done';
    default:return String(stage||'');
  }
}
BT.on('export-progress',p=>{
  if(!exp.running||!p)return;
  const pct=clamp(Math.round(+p.percent||0),0,100);
  $('#pFill').style.width=pct+'%';$('#pScan').style.width=pct+'%';$('#pPct').textContent=pct+'%';
  $('#pStage').textContent=stageLabel(p.stage);
  $('#pEta').textContent=p.stage==='finish'?'Finishing…':p.etaSec==null?'Estimating time…':`about ${Math.max(1,Math.round(p.etaSec))} s left`;
});
function runExport(req){
  const g=++exp.gen;exp.req=req;exp.est=reqEstimate(req);exp.error=null;exp.running=true;
  const m=$('#modal');m.dataset.view='progress';renderProgressStart(req);
  if(cur()!=='export'&&cur()!=='export-done')location.hash='export';else route();
  // one export at a time: wait for a cancelled one to wind down first
  const p=(exp.inflight||Promise.resolve()).then(()=>g===exp.gen?BT.exportClip(req):Promise.reject('superseded'));
  exp.inflight=p.catch(()=>{});
  p.then(res=>{
    if(g!==exp.gen)return;exp.running=false;
    const c=S.byId[req.clipId];
    if(c){c.exports=[{path:res.path,sizeBytes:res.sizeBytes,preset:req.preset,width:res.width,height:res.height,fps:res.fps,at:Date.now()},...(c.exports||[])];if(ed.clip===c)renderActivity();}
    exp.result={req,res};exp.direct=false;
    go('export-done');
  },err=>{
    if(g!==exp.gen)return;exp.running=false;
    const msg=errMsg(err);if(msg==='cancelled')return;
    exp.error={req,msg};BT.log('error','export_clip failed: '+msg);
    m.dataset.view='error';renderError();
  });
}
function cancelRunning(){
  if(!exp.running)return false;
  exp.gen++;exp.running=false;
  BT.cancelExport().catch(e=>BT.log('warn','cancel_export failed: '+errMsg(e)));
  toast('Export cancelled — nothing was saved','x');
  return true;
}
function renderError(){
  const E=exp.error;if(!E)return;
  $('#eMsg').textContent=E.msg;
  const ff=/ffmpeg/i.test(E.msg)&&/(found|install)/i.test(E.msg);
  $('#eHint').textContent=ff?'Backtrack makes exports with FFmpeg. After installing it, restart Backtrack and press Retry.'
    :'Retry usually works if it was a one-off. If it keeps failing, try another size or a shorter selection.';
  setTimeout(()=>{const b=$('#eRetry');if(b&&$('#modal').dataset.view==='error')b.focus();},0);
}
/* done screen: everything comes from the real ExportResult */
function mixLine(c,tracks){
  if(!tracks.length)return 'No audio';
  const on=tracks.filter(t=>!t.muted&&t.volume>0),off=tracks.filter(t=>t.muted||t.volume<=0);
  const s=on.length?on.map(t=>`${esc(trackLabel(c,t.index))} ${Math.round(t.volume*100)}%`).join(' + '):'No audio';
  return off.length?`${s} <span class="off">(${off.map(t=>esc(trackLabel(c,t.index))).join(', ')} muted)</span>`:s;
}
const TILE_HINT=()=>`${ic('copy','sm')}<span><b>Click</b> to copy the file, then paste it in Discord with <b>Ctrl+V</b></span>`;
function renderDone(){
  const R=exp.result;if(!R)return;const {req,res}=R,c=S.byId[req.clipId]||ed.clip;
  const mb=MBof(res.sizeBytes),lim=req.limitMb,over=!!lim&&mb>lim;
  $('#dSub').innerHTML=lim?(over?`<b class="tnum" style="color:var(--warn)">${fmtMB(mb)}</b> — over the ${fmtNum(lim)} MB limit, so Discord may refuse it.`:`<b class="tnum" style="color:var(--text-1)">${fmtMB(mb)}</b> — under the ${fmtNum(lim)} MB limit with ${(lim-mb).toFixed(1)} MB to spare.`)
    :`<b class="tnum" style="color:var(--text-1)">${fmtMB(mb)}</b> — original quality, video not re-encoded.`;
  $('#dThumbWrap').innerHTML=thumbHTML(c&&c.thumb,{eager:true});$('#dDur').textContent=fmtLen(res.durationSec);
  $('#dName').textContent=res.fileName;$('#dName').title=res.path;
  $('#dMeta').textContent=`${fmtMB(mb)} · MP4 · ${(+res.durationSec).toFixed(1)} s`;
  $('#dFacts').innerHTML=`<dt>Video</dt><dd>${res.width} × ${res.height} · ${res.fps} fps · ${fmtK(res.videoKbps)}${res.mode==='copy'?' · exact copy':''}</dd><dt>Audio</dt><dd>${c?mixLine(c,req.tracks):''}${res.audioKbps?` <span class="off">· ${res.audioKbps} kbps</span>`:''}</dd><dt>Part</dt><dd>${fmt(req.start)} → ${fmt(req.end)}${c?` of the ${fmtLen(c.duration)} clip`:''}</dd>`;
  $('#dSizeRow').textContent=lim?`${fmtMB(mb)} of ${fmtNum(lim)} MB`:fmtMB(mb);
  const l=lim||Math.max(10,mb);$('#dFill').style.width=Math.min(100,mb/l*100)+'%';$('#dMeter').className='meter'+(over?' bad':'');
  // emphasis: when the file is already on the clipboard, Open Discord leads and Copy becomes "Copy again"
  const copied=!!res.copied;
  $('#clipBanner').hidden=!copied;
  const cp=$('#dCopy'),dc=$('#dDiscord');
  cp.classList.remove('copied');$('use',cp).setAttribute('href','#i-copy');
  cp.classList.toggle('primary',!copied);dc.classList.toggle('primary',copied);
  $('.at b',cp).textContent=copied?'Copy again':'Copy file';
  $('.at small',cp).textContent=copied?'Puts the file back on your clipboard':'Then paste it into any Discord chat';
  $('#dDiscordSub').textContent=copied?'Then press Ctrl + V in any chat':'Switch to the Discord app';
  const dir=dirOf(res.path);
  $('#dFolderSub').textContent=shortPath(dir,44);$('#dFolder').title=dir;
  $('#dFoot').innerHTML=`${ic('clock','xs')}<span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${esc(res.path)}">Exported in ${(+res.elapsedSec).toFixed(1)} s · saved to ${esc(shortPath(dir,38))} · original unchanged</span>`;
  const t=$('#dragTile');t.classList.remove('copied');clearTimeout(t._t);$('#dtHint').innerHTML=TILE_HINT();
}
let copying=false;
async function copyResult(fromTile){
  const R=exp.result;if(!R||copying)return;copying=true;
  try{
    await BT.copyFile(R.res.path);R.res.copied=true;
    const b=$('#dCopy');b.classList.add('copied');$('use',b).setAttribute('href','#i-check');$('.at b',b).textContent='Copied to clipboard';$('.at small',b).textContent='Now press Ctrl + V in a Discord chat';$('#clipBanner').hidden=false;
    clearTimeout(b._t);b._t=setTimeout(()=>{$('use',b).setAttribute('href','#i-copy');},2200);
    if(fromTile){const t=$('#dragTile');t.classList.add('copied');$('#dtHint').innerHTML=`${ic('check','sm')}<span><b>Copied.</b> Now press Ctrl + V in a Discord chat</span>`;
      clearTimeout(t._t);t._t=setTimeout(()=>{t.classList.remove('copied');$('#dtHint').innerHTML=TILE_HINT();},3200);}
  }catch(e){fail("Couldn't copy the file",e);}
  finally{copying=false;}
}
$('#mExport').addEventListener('click',()=>{exp.direct=false;runExport(buildRequest());});
$('#pCancel').addEventListener('click',()=>{
  const direct=exp.direct||cur()==='export-done';if(!cancelRunning())return;exp.direct=false;
  $('#modal').dataset.view='setup';
  if(direct)go('editor');else{if(cur()!=='export')location.hash='export';renderModal();$('#modal').focus({preventScroll:true});}
});
$('#eRetry').addEventListener('click',()=>{if(exp.error)runExport(exp.error.req);});
$('#eOptions').addEventListener('click',()=>{exp.error=null;$('#modal').dataset.view='setup';if(cur()!=='export')location.hash='export';else route();});
$('#dCopy').addEventListener('click',()=>copyResult(false));
$('#dragTile').addEventListener('click',()=>copyResult(true));
$('#dragTile').addEventListener('keydown',e=>{if(e.key===' '||e.key==='Enter'){e.preventDefault();e.stopPropagation();copyResult(true);}});
$('#dDiscord').addEventListener('click',()=>{const copied=exp.result&&exp.result.res.copied;
  BT.openDiscord().then(()=>toast(copied?'Opening Discord… the file is on your clipboard':'Opening Discord… copy the file first, then paste it','chat'),e=>fail("Couldn't open Discord",e));});
$('#autoCopy').addEventListener('change',e=>{if(e.target.checked!==S.settings.autoCopy)saveSettings({autoCopy:e.target.checked});});
$('#dFolder').addEventListener('click',()=>{if(exp.result)BT.reveal(exp.result.res.path).catch(e=>fail("Couldn't open the folder",e));});
$('#dAgain').addEventListener('click',()=>{$('#modal').dataset.view='setup';go('export');});
$('#dDone').addEventListener('click',()=>go('editor'));
$$('[data-close]').forEach(b=>b.addEventListener('click',()=>go('editor')));
$('#scrim').addEventListener('click',()=>{if($('#modal').dataset.view!=='progress')go('editor');});

/* ================================================================== SETTINGS */
const DEF_HINT={d10:'Discord free · 10 MB',d50:'Nitro Basic · 50 MB',d500:'Nitro · 500 MB',orig:'Original quality · no re-encode'};
let setQ=Promise.resolve();
/* optimistic: the UI updates at once, save_settings confirms (and may normalise), failures roll back */
function saveSettings(patch,okToast){
  const prev=S.settings,next=Object.assign({},prev,patch);S.settings=next;afterSettings();
  // a recorder error that just arrived (e.g. the chosen encoder isn't available) keeps its toast
  setQ=setQ.then(()=>BT.saveSettings(next)).then(s=>{S.settings=s;afterSettings();if(okToast&&!(REC.errAt&&Date.now()-REC.errAt<2500))toast(...okToast);},
    e=>{S.settings=prev;afterSettings();fail("Couldn't save settings",e);});
  return setQ;
}
function afterSettings(){
  if(cur().startsWith('settings'))renderSettings();
  $('#autoCopy').checked=S.settings.autoCopy;
  if(ed.clip&&$('#modalLayer').hidden){ed.customMB=S.settings.customMb;updateEstimates();}
  renderRecorder();
}
function folderStats(d){
  if(!S.loaded)return 'Scanning…';
  const cs=S.clips.filter(c=>under(c.path,d));
  return cs.length?`${cs.length} clip${cs.length===1?'':'s'} · ${fmtTotal(cs.reduce((a,c)=>a+c.sizeBytes,0))}`:'No clips found here yet';
}
const sameDir=(a,b)=>String(a||'').replace(/[\\/]+$/,'').toLowerCase()===String(b||'').replace(/[\\/]+$/,'').toLowerCase();
function renderSettings(){
  const s=S.settings;if(!s)return;const r=s.recorder;
  const ff=s.ffmpegPath;
  $('#ffStatus').innerHTML=ff?`${ic('check-c','ok')}<div><b>FFmpeg ready</b><span title="${esc(ff)}">${esc(shortPath(ff,48))}</span></div>`
    :`${ic('alert','warn')}<div><b>FFmpeg not found</b><span>Exports need it · winget install Gyan.FFmpeg.Essentials</span></div>`;
  // the recorder's own folder is always in clipDirs (the backend re-adds it), so it can't be removed here
  $('#folderRows').innerHTML=s.clipDirs.length?s.clipDirs.map((d,i)=>{const own=r&&sameDir(d,r.clipsDir);
    return `<div class="srow"><span class="src-ico" style="--c:var(${own?'--rec':'--accent'})">${ic(own?'replay':'folder')}</span><div class="sl"><b class="fp" title="${esc(d)}">${esc(d)}</b><span>${own?'Your Backtrack recordings · ':''}${folderStats(d)}</span></div><button class="btn sm" data-fopen="${i}">Open</button>${own?`<button class="icon-btn" disabled aria-hidden="true" style="visibility:hidden">${ic('x')}</button>`:`<button class="icon-btn" data-fdel="${i}" data-tip="Stop showing these clips" aria-label="Remove ${esc(d)}">${ic('x')}</button>`}</div>`;}).join('')
    :`<div class="srow"><span class="src-ico" style="--c:var(--warn)">${ic('alert')}</span><div class="sl"><b>No clip folder yet</b><span>Add your SteelSeries Moments folder to see your clips</span></div></div>`;
  $$('[data-set="defpreset"] button').forEach(b=>b.classList.toggle('on',b.dataset.v===s.defaultPreset));
  $('#defHint').textContent=s.defaultPreset==='custom'?`Custom · ${fmtNum(s.customMb)} MB`:DEF_HINT[s.defaultPreset]||'';
  const cm=$('#customMbSet');if(document.activeElement!==cm)cm.value=fmtNum(s.customMb);
  $$('[data-sw="autocopy"]').forEach(x=>{x.classList.toggle('on',!!s.autoCopy);x.setAttribute('aria-checked',!!s.autoCopy);});
  $$('[data-set="quicklen"] button').forEach(b=>b.classList.toggle('on',+b.dataset.v===+s.quickLen));
  $('#exDir').textContent=shortPath(s.exportDir,30);$('#exDirBox').title=s.exportDir;
  const ex=S.clips.flatMap(c=>c.exports||[]).filter(x=>under(x.path,s.exportDir));
  $('#exStats').textContent=ex.length?`${ex.length} export${ex.length===1?'':'s'} · ${fmtTotal(ex.reduce((a,x)=>a+x.sizeBytes,0))}`:'No exports yet';
  if(r){
    $$('.seg[data-rec]').forEach(seg=>{const k=seg.dataset.rec;$$('button',seg).forEach(b=>b.classList.toggle('on',+b.dataset.v===+r[k]));});
    $$('[data-rsw]').forEach(x=>{const on=!!r[x.dataset.rsw];x.classList.toggle('on',on);x.setAttribute('aria-checked',on);});
    $('#ramHint').textContent=`Last ${fmtBuf(r.bufferSecs)} · ≈ ${fmtMem(r.bitrateMbps*r.bufferSecs/8)} of RAM`;
    $('#rateHint').textContent=`${RATE_NAME[r.bitrateMbps]||'Custom'} · ≈ ${fmtMem(r.bitrateMbps*60/8)} per minute`;
    const mon=monitorOf(r.monitor);
    $('#resHint').textContent=mon?(r.resolution&&r.resolution>Math.min(mon.width,mon.height)?`Above your display (${mon.width} × ${mon.height}), so it records at native size`:`Your display is ${mon.width} × ${mon.height}`):'';
    $('#monDdTxt').textContent=mon?monLabel(mon):`Monitor ${r.monitor+1}`;
    $$('[data-enc]').forEach(b=>{const on=b.dataset.enc===r.encoder;b.classList.toggle('on',on);b.setAttribute('aria-checked',on);b.setAttribute('role','radio');});
    $('#gameSrcTxt').textContent=r.chatApp?`All apps except ${r.chatApp}`:'All apps';
    $('#chatDdTxt').textContent=r.chatApp||'None';
    $('#micDdTxt').textContent=micLabel(r.micDevice);$('#micDd').style.opacity=r.micEnabled?'':'.55';
    {const def=REC.devices&&REC.devices.inputs.find(i=>i.isDefault);$('#micDd').title=r.micDevice?micLabel(r.micDevice):def?`Windows default: ${def.name}`:'Windows default';}
    $('#clipsDir').textContent=shortPath(r.clipsDir,30);$('#clipsDirBox').title=r.clipsDir;
    const own=S.clips.filter(c=>under(c.path,r.clipsDir));
    $('#clipsStats').textContent=own.length?`${own.length} clip${own.length===1?'':'s'} · ${fmtTotal(own.reduce((a,c)=>a+c.sizeBytes,0))}`:'No clips yet';$('#clipsStats').title='New clips are named after the game in the foreground';
    renderHotkeys();
  }
  syncBufferSwitches();
  renderRecLive();
  $('#setFoot').innerHTML=`<b>Backtrack${S.version?' '+esc(S.version):''}</b><br>Records, edits and shares your clips`;
}
$$('.seg[data-set]').forEach(s=>s.addEventListener('click',e=>{const b=e.target.closest('button');if(!b)return;const k=s.dataset.set,v=b.dataset.v;
  if(k==='defpreset'&&v!==S.settings.defaultPreset){saveSettings({defaultPreset:v},[`E now exports at ${v==='custom'?`Custom · ${fmtNum(S.settings.customMb)} MB`:DEF_HINT[v]}`,'upload']);}
  else if(k==='quicklen'&&+v!==S.settings.quickLen){saveSettings({quickLen:+v},[`Quick send now exports ${v} s`,'bolt']);}}));
$('#customMbSet').addEventListener('change',e=>{const v=parseFloat(e.target.value);if(!(v>.5)){e.target.value=fmtNum(S.settings.customMb);toast('Type a size in MB, like 25','alert');return;}
  saveSettings({customMb:Math.min(2000,Math.round(v*10)/10)},['Custom size saved','check']);});
$$('[data-sw="autocopy"]').forEach(x=>x.addEventListener('click',()=>saveSettings({autoCopy:!S.settings.autoCopy})));
$('#addFolder').addEventListener('click',async()=>{
  let p;try{p=await BT.pickFolder({title:'Choose a clip folder'});}catch(e){fail("Couldn't open the folder picker",e);return;}
  if(!p)return;
  if(S.settings.clipDirs.some(d=>sameDir(d,p))){toast('That folder is already in the list','folder');return;}
  saveSettings({clipDirs:[...S.settings.clipDirs,p]},[`Added ${shortPath(p,40)} · looking for clips`,'folder']);
});
$('#folderRows').addEventListener('click',e=>{
  const o=e.target.closest('[data-fopen]');if(o){const d=S.settings.clipDirs[+o.dataset.fopen];if(d)BT.openPath(d).catch(err=>fail("Couldn't open the folder",err));return;}
  const x=e.target.closest('[data-fdel]');if(!x)return;const i=+x.dataset.fdel,d=S.settings.clipDirs[i];if(d==null)return;
  saveSettings({clipDirs:S.settings.clipDirs.filter((_,j)=>j!==i)},[`Removed ${shortPath(d,36)} · its clips are hidden, not deleted`,'folder',
    {label:'Undo',fn:()=>{const l=[...S.settings.clipDirs];if(!l.includes(d)){l.splice(Math.min(i,l.length),0,d);saveSettings({clipDirs:l});}}}]);
});
$('#openExDir').addEventListener('click',()=>BT.openPath(S.settings.exportDir).catch(e=>fail("Couldn't open the folder",e)));
$('#changeExDir').addEventListener('click',async()=>{
  let p;try{p=await BT.pickFolder({title:'Choose where exports go',defaultPath:S.settings.exportDir});}catch(e){fail("Couldn't open the folder picker",e);return;}
  if(p&&p!==S.settings.exportDir)saveSettings({exportDir:p},[`Exports now go to ${shortPath(p,40)}`,'folder']);
});
/* settings nav: click scrolls to the card; scroll-spy keeps the highlight in sync.
   UX review r2 #1: with two columns, cards share a top, so a clicked item stays highlighted until the user scrolls 48 px. */
let spyLock=0,spyHold=null;
const SN=()=>$$('.sn').map(a=>a.dataset.sec);
function setSn(id){$$('.sn').forEach(x=>x.classList.toggle('on',x.dataset.sec===id));}
function spy(){
  const sm=$('#setMain'),h=sm.clientHeight;if(!h||performance.now()<spyLock)return;
  // the hold survives layout shifts (late data changing the page height); only the user's own scrolling releases it
  if(spyHold){if(!spyHold.user||Math.abs(sm.scrollTop-spyHold.top)<=48){setSn(spyHold.id);return;}spyHold=null;}
  const ids=SN(),top=sm.getBoundingClientRect().top,rs=ids.map(s=>{const r=document.getElementById(s).getBoundingClientRect();return {t:r.top-top,b:r.bottom-top};});let i;
  // active = first card (in nav order) reaching into the top 45 %; at the very bottom, the first card of the last row
  if(sm.scrollTop>4&&sm.scrollTop+h>=sm.scrollHeight-4){const m=Math.max(...rs.filter(r=>r.t<h-40).map(r=>r.t));i=rs.findIndex(r=>Math.abs(r.t-m)<6);}
  else i=rs.findIndex(r=>r.b>8&&r.t<h*.45);
  if(i>=0)setSn(ids[i]);
}
{const io=new IntersectionObserver(()=>spy(),{root:$('#setMain'),rootMargin:'0px 0px -55% 0px',threshold:0});SN().forEach(s=>io.observe(document.getElementById(s)));}
$('#setMain').addEventListener('scroll',spy,{passive:true});
{const userScroll=()=>{if(spyHold&&!spyHold.user){spyHold.user=true;spyHold.top=$('#setMain').scrollTop;}};
  ['wheel','pointerdown','touchstart'].forEach(ev=>$('#setMain').addEventListener(ev,userScroll,{passive:true}));
  document.addEventListener('keydown',e=>{if(/^(PageUp|PageDown|ArrowUp|ArrowDown|Home|End| )$/.test(e.key)&&cur().startsWith('settings'))userScroll();},true);}
const DEEP={export:'s-export',sharing:'s-sharing',folders:'s-folders',capture:'s-capture',buffer:'s-buffer',video:'s-video',encoder:'s-encoder',hotkeys:'s-hotkeys',audio:'s-audio',storage:'s-storage',files:'s-storage'};
function scrollSettingsTo(id,smooth){
  id=DEEP[id]||id;
  const t=document.getElementById(id);if(!t)return;const sm=$('#setMain');
  const navId=id==='s-capture'?'s-buffer':id==='s-sharing'?'s-export':id;setSn(navId);
  const target=clamp(sm.scrollTop+t.getBoundingClientRect().top-sm.getBoundingClientRect().top-16,0,Math.max(0,sm.scrollHeight-sm.clientHeight));
  spyHold={id:navId,top:target};spyLock=performance.now()+(smooth?900:50);
  sm.scrollTo({top:target,behavior:smooth?'smooth':'auto'});
  if(smooth&&t.classList.contains('scard')){t.classList.add('flash');setTimeout(()=>t.classList.remove('flash'),900);}
}
$$('.sn').forEach(a=>a.addEventListener('click',()=>scrollSettingsTo(a.dataset.sec,true)));
let lastMain='library';
$('#setBack').addEventListener('click',()=>go(lastMain));
$('#tbSettings').addEventListener('click',()=>go('settings'));

/* ================================================================== RECORDER (phase 2)
   recorder-status arrives about every 500 ms while the buffer is on; it drives the title-bar pill,
   the library's replay-buffer card and the live parts of Settings (status, meters, hotkey errors). */
const REC={status:null,devices:null,monitors:null,wave:Array(44).fill(0),hkSig:'',pendingOpen:null,lastPill:''};
const recCfg=()=>S.settings&&S.settings.recorder;
function saveRecorder(patch,okToast){const r=recCfg();if(!r)return Promise.resolve();return saveSettings({recorder:Object.assign({},r,patch)},okToast);}
const fmtBuf=s=>s>=120&&s%60===0?(s/60)+' min':Math.round(s)+' s';
const fmtMem=mb=>mb>=1000?(mb/1000).toFixed(1)+' GB':Math.round(mb)+' MB';
const kbdHTML=k=>k?k.split('+').map(x=>`<kbd>${esc(x)}</kbd>`).join(''):'';
const encShort=s=>String(s||'').split(' · ')[0];
const RATE_NAME={15:'Balanced',25:'High',40:'Maximum'};
const HK=[['save','Save replay clip'],['toggle','Turn replay buffer on / off'],['muteMic','Mute mic in recordings'],['mark','Mark a moment'],['open','Open Backtrack']];
const HK_LABEL=a=>(HK.find(h=>h[0]===a)||[a,a])[1];
const HK_SUB={save:()=>`Keeps the last ${fmtBuf(recCfg().bufferSecs)}`,toggle:()=>'Plays a short sound so you know',muteMic:()=>'Doesn’t affect your voice chat',mark:()=>'The next saved clip gets a marker that trims snap to',open:()=>'Jumps straight to your newest clip'};
const lvlOf=(st,name)=>{const t=st&&st.tracks.find(x=>x.name===name);return t?clamp(+t.level||0,0,1):0;};
// a hotkey's status only counts while it still describes the keys in settings
function hkState(a){const st=REC.status,r=recCfg();const h=st&&st.hotkeys&&st.hotkeys.find(x=>x.action===a);return h&&r&&h.keys===(r.hotkeys[a]||'')?h:null;}
const hkSig=()=>JSON.stringify([recCfg()&&recCfg().hotkeys,REC.status&&REC.status.hotkeys]);
const monitorOf=i=>(REC.monitors||[]).find(m=>m.index===i)||null;
const monLabel=m=>`${m.index+1} · ${m.name} · ${m.width} × ${m.height}`;
function micLabel(id){const ins=REC.devices?REC.devices.inputs:[],def=ins.find(i=>i.isDefault);
  if(!id)return 'Windows default';const d=ins.find(i=>i.id===id);return d?d.name:(REC.devices?'Unplugged device':'Chosen microphone');}
function loadDevices(){
  return Promise.all([BT.listAudioDevices(),BT.listMonitors()]).then(([d,m])=>{REC.devices=d;REC.monitors=m;if(cur().startsWith('settings'))renderSettings();},
    e=>BT.log('warn','list_audio_devices / list_monitors failed: '+errMsg(e)));
}
function applyStatus(st){
  if(!st)return;const prev=REC.status;REC.status=st;
  if(st.state==='on'){REC.wave.shift();REC.wave.push(Math.max(lvlOf(st,'Game'),lvlOf(st,'Mic')));}
  else if(prev&&prev.state==='on')REC.wave.fill(0);
  renderRecorder();
  if(prev&&prev.state!==st.state&&st.state==='error'){REC.errAt=Date.now();toast(`Recorder error · ${st.error||'something went wrong'}`,'alert');}
}
function renderRecorder(){
  renderPill();renderBufCard();renderRecLive();
  // an open recorder menu keeps its status line live (rewritten only when the text changes)
  if(popEl&&popEl.classList.contains('recpop')){const h=$('.pop-head',popEl),html=recHead();if(h&&h.dataset.h!==html){h.innerHTML=html;h.dataset.h=html;}}
}
/* ---------- title-bar pill */
function renderPill(){
  const p=$('#bufferPill'),st=REC.status,r=recCfg();
  let cls='buffer-pill off nokb',html='<span class="rec-dot off"></span><b>Replay buffer</b>',tip='Checking the recorder…';
  if(st&&r){
    const len=fmtBuf(r.bufferSecs),hk=r.hotkeys.save,hs=hkState('save'),ok=!hs||hs.ok;
    if(st.state==='on'){cls='buffer-pill'+(hk?'':' nokb');html=`<span class="rec-dot"></span><b>Replay buffer on</b><span>· last ${len}</span>${hk?`<span class="kb${ok?'':' bad'}">${kbdHTML(hk)}</span>`:''}`;
      tip=!hk?'No save hotkey · click to save or change it':ok?`${hk} saves the last ${len} · click for more`:`${hk} isn't working: ${hs.error}`;}
    else if(st.state==='starting'){html='<span class="rec-dot starting"></span><b>Starting replay buffer…</b>';tip='The recorder is starting';}
    else if(st.state==='error'){cls='buffer-pill err nokb';html='<span class="rec-dot warn"></span><b>Recorder error</b>';tip='Click for details';}
    else{html='<span class="rec-dot off"></span><b>Replay buffer off</b>';tip='Click to turn it on';}
  }
  const sig=cls+html+tip;if(sig===REC.lastPill)return;REC.lastPill=sig;
  p.className=cls;p.innerHTML=html;p.dataset.tip=tip;p.setAttribute('aria-label',p.textContent.trim()+'. '+tip);
}
function recHead(){
  const st=REC.status,r=recCfg();if(!st||!r)return '';
  if(st.state==='on'){const f=Math.min(r.bufferSecs,Math.floor(st.filledSecs));
    return `<span class="rec-dot"></span><div><b>Replay buffer on</b>${esc([st.game,`${f>=r.bufferSecs?'full':`${f} of ${fmtBuf(r.bufferSecs)} ready`}`,`${fmtMem(st.memoryMb)} RAM`].filter(Boolean).join(' · '))}<br>${esc(`${st.height}p${st.fps} · ${st.encoder}`)}${st.micMuted?'<br>Mic muted in recordings':''}</div>`;}
  if(st.state==='error')return `<span class="rec-dot warn"></span><div><b>Recorder error</b><span class="perr">${esc(st.error||'Something went wrong.')}</span></div>`;
  if(st.state==='starting')return '<span class="rec-dot starting"></span><div><b>Starting the replay buffer…</b>It keeps the last '+fmtBuf(r.bufferSecs)+' once it’s running</div>';
  return '<span class="rec-dot off"></span><div><b>Replay buffer off</b>The save hotkey does nothing until it’s on</div>';
}
function openRecMenu(anchor){
  if(popEl&&popEl.classList.contains('recpop')){closePop();return;}
  const st=REC.status,r=recCfg();if(!st||!r)return;const on=st.state==='on',hk=r.hotkeys,items=[];
  if(on){
    items.push({k:'save',icon:'bolt',label:'Save now',sub:`Up to the last ${fmtBuf(r.bufferSecs)}`,right:hk.save,disabled:st.filledSecs<1});
    items.push({k:'mark',icon:'flag',label:'Mark a moment',sub:'The next clip gets a marker here',right:hk.mark});
    if(r.micEnabled)items.push({k:'mic',icon:st.micMuted?'mic':'vol-x',label:st.micMuted?'Unmute mic':'Mute mic',sub:'In recordings only, not in Discord',right:hk.muteMic});
  }
  if(st.state==='error')items.push({k:'retry',icon:'reset',label:'Try again'});
  items.push(st.state==='off'?{k:'on',icon:'power',label:'Turn on',right:hk.toggle}:{k:'off',icon:'power',label:'Turn off',sub:'The save hotkey stops working',right:hk.toggle});
  items.push({sep:true},{k:'settings',icon:'gear',label:'Recorder settings'});
  openPop(anchor,items,it=>{
    if(it.k==='save')saveNow();
    else if(it.k==='mark')BT.addMarker().then(()=>toast('Marked · the next clip you save gets a marker here','flag'),e=>fail("Couldn't mark the moment",e));
    else if(it.k==='mic')toggleMic();
    else if(it.k==='retry'||it.k==='on')setBufferEnabled(true);
    else if(it.k==='off')setBufferEnabled(false);
    else if(it.k==='settings')go('settings-capture');
  },'left',null,{head:recHead(),cls:'recpop'});
}
$('#bufferPill').addEventListener('click',e=>{const st=REC.status;if(!st)return;if(st.state==='off'&&!(popEl&&popEl.classList.contains('recpop'))){setBufferEnabled(true);return;}openRecMenu(e.currentTarget);});
/* ---------- library sidebar card */
function syncBufferSwitches(){const r=recCfg(),on=!!(r&&r.enabled);$$('[data-sw="buffer"]').forEach(s=>{s.classList.toggle('on',on);s.setAttribute('aria-checked',on);});}
function renderBufCard(){
  const card=$('#bufCard'),r=recCfg(),st=REC.status;
  if(!r){card.hidden=true;return;}card.hidden=false;
  const state=st?st.state:'unknown',on=state==='on',len=fmtBuf(r.bufferSecs);
  card.className='buf-card'+(on?'':state==='error'?' err':' off');
  $('#bcDot').className='rec-dot'+(on?'':state==='error'?' warn':state==='starting'?' starting':' off');
  const sub=on?`Recording · ${st.height}p${st.fps}${st.micMuted?' · mic muted':''}`:state==='starting'?'Starting…':state==='error'?(st.error||'Recorder error'):state==='off'?'Off · not recording':'Checking…';
  const se=$('#bcSub');if(se.textContent!==sub)se.textContent=sub;se.title=on?`${st.width} × ${st.height} · ${st.fps} fps · ${st.encoder}`:sub;
  const w=$('#bcWave');if(!w.children.length)w.innerHTML='<i></i>'.repeat(REC.wave.length);
  [...w.children].forEach((b,i)=>b.style.height=(on?Math.max(8,REC.wave[i]*100):8)+'%');
  const filled=on?clamp(+st.filledSecs||0,0,r.bufferSecs):0;
  $('#bcFill').style.width=(on?filled/r.bufferSecs*100:0)+'%';
  $('#bcGauge').title=on?`${Math.floor(filled)} of ${len} buffered`:'';
  $('#bcFoot').innerHTML=`Saves the last <b>${len}</b>`;
  $('#bcReady').textContent=on?(filled>=r.bufferSecs-.5?'Full':`${Math.floor(filled)} s ready`):'';
  $('#bcSave').disabled=!on||filled<1;$('#bcKeys').innerHTML=kbdHTML(r.hotkeys.save);
  syncBufferSwitches();
}
let bufBusy=false;
async function setBufferEnabled(on){
  const r=recCfg();if(!r||bufBusy)return;bufBusy=true;
  S.settings=Object.assign({},S.settings,{recorder:Object.assign({},r,{enabled:on})});renderRecorder();
  try{
    applyStatus(await BT.setBuffer(on));
    toast(on?`Replay buffer on${r.hotkeys.save?` · ${r.hotkeys.save} saves the last ${fmtBuf(r.bufferSecs)}`:''}`:'Replay buffer off · the save hotkey won’t work until it’s on',on?'replay':'x');
  }catch(e){S.settings=Object.assign({},S.settings,{recorder:Object.assign({},recCfg(),{enabled:!on})});renderRecorder();fail(`Couldn't turn the replay buffer ${on?'on':'off'}`,e);}
  finally{bufBusy=false;}
}
$$('[data-sw="buffer"]').forEach(s=>s.addEventListener('click',()=>{const r=recCfg();if(r)setBufferEnabled(!r.enabled);}));
async function saveNow(){try{await BT.saveReplay(null);}catch(e){fail("Couldn't save a clip",e);}}   // clip-saved brings the toast
$('#bcSave').addEventListener('click',saveNow);
function toggleMic(){BT.toggleMicMute().then(st=>{applyStatus(st);toast(st.micMuted?'Mic muted in recordings · Discord still hears you':'Mic is recorded again',st.micMuted?'vol-x':'mic');},e=>fail("Couldn't change the mic",e));}
/* ---------- settings: live parts */
function renderRecLive(){
  if(!cur().startsWith('settings'))return;const st=REC.status,r=recCfg();if(!r)return;
  let dot='off',title='Replay buffer',sub='Checking the recorder…';
  if(st){
    if(st.state==='on'){dot='';title='Replay buffer is running';sub=[st.game,encShort(st.encoder),`using ≈ ${fmtMem(st.memoryMb)} RAM`].filter(Boolean).join(' · ');}
    else if(st.state==='starting'){dot='starting';title='Starting the replay buffer…';sub=`Keeps the last ${fmtBuf(r.bufferSecs)} once it’s running`;}
    else if(st.state==='error'){dot='warn';title='Recorder error';sub=st.error||'Something went wrong';}
    else{title='Replay buffer is off';sub=`Turn it on to keep the last ${fmtBuf(r.bufferSecs)}`;}
  }
  const card=$('#recStatusCard'),html=`<span class="rec-dot ${dot}"></span><div><b>${title}</b><span title="${esc(sub)}">${esc(sub)}</span></div>`;
  card.className='status-card'+(st&&st.state==='error'?' err':'');if(card.dataset.h!==html){card.innerHTML=html;card.dataset.h=html;}
  $$('.lvl[data-lvl]').forEach(l=>{if(!l.children.length)l.innerHTML='<i></i>'.repeat(10);const t=st&&st.tracks.find(x=>x.name===l.dataset.lvl),n=st&&st.state==='on'&&t?Math.round(lvlOf(st,t.name)*10):0;
    [...l.children].forEach((b,i)=>b.classList.toggle('on',i<n));l.style.opacity=t&&!t.active?'.4':'';});
  const chat=st&&st.tracks.find(t=>t.name==='Voice chat');
  $('#chatNote').textContent=!r.chatApp?'Off':chat&&!chat.active?`${r.chatApp} isn’t running`:'';
  const mn=$('#micNote'),mh=!r.micEnabled?'Off':st&&st.micMuted?`<span class="trk warn" data-unmute role="button" tabindex="0" title="Click to unmute">Muted${r.hotkeys.muteMic?` · ${esc(r.hotkeys.muteMic)}`:''}</span>`:'';
  if(mn.dataset.h!==mh){mn.innerHTML=mh;mn.dataset.h=mh;}
  $('#encInUse').innerHTML=!st?'':st.state==='error'?`<span class="err-t">${esc(st.error||'Recorder error')}</span>`:esc(st.encoder||(st.state==='off'?'Nothing while the buffer is off':'Starting…'));
  $('#encAutoSub').textContent=r.encoder==='auto'&&st&&st.encoder?`Uses ${st.encoder}`:'Picks the best one for this PC';
  $$('[data-enc] .et>span').forEach(s=>s.classList.toggle('inuse',r.encoder==='auto'&&s.id==='encAutoSub'&&!!(st&&st.encoder)));
  if(hkSig()!==REC.hkSig)renderHotkeys();
}
$('#micNote').addEventListener('click',e=>{if(e.target.closest('[data-unmute]'))toggleMic();});
/* ---------- settings: capture controls */
$$('.seg[data-rec]').forEach(s=>s.addEventListener('click',e=>{
  const b=e.target.closest('button'),r=recCfg();if(!b||!r)return;const k=s.dataset.rec,v=+b.dataset.v;if(+r[k]===v)return;
  const msg={bufferSecs:`Replay buffer keeps the last ${fmtBuf(v)} · ≈ ${fmtMem(r.bitrateMbps*v/8)} of RAM`,resolution:v?`Recording at ${v}p`:'Recording at your monitor’s native size',fps:`Recording at ${v} fps`,bitrateMbps:`Recording at ${v} Mbps`}[k];
  saveRecorder({[k]:v},[msg+(r.enabled?' · the buffer restarted':''),'replay']);
}));
$$('[data-rsw]').forEach(x=>x.addEventListener('click',()=>{
  const k=x.dataset.rsw,r=recCfg();if(!r)return;const v=!r[k];
  const msg={confirm:v?'You’ll see a pop-up when a clip is saved':'No pop-up when a clip is saved',micEnabled:v?'The mic is recorded again':'The mic isn’t recorded any more',startWithWindows:v?'Backtrack starts with Windows, in the tray':'Backtrack won’t start with Windows'}[k];
  saveRecorder({[k]:v},[msg,k==='micEnabled'?(v?'mic':'vol-x'):'check']);
}));
$$('[data-enc]').forEach(b=>b.addEventListener('click',()=>{const r=recCfg(),v=b.dataset.enc;if(!r||r.encoder===v)return;saveRecorder({encoder:v},[`Encoder: ${$('b',b).firstChild.textContent.trim()}`,'chip']);}));
$('#monDd').addEventListener('click',async e=>{
  const a=e.currentTarget;await loadDevices();const r=recCfg(),ms=REC.monitors||[];
  if(!ms.length){toast('No monitors found','alert');return;}
  openPop(a,ms.map(m=>({m,label:`${m.index+1} · ${m.name}${m.primary?' (main)':''}`,sub:`${m.width} × ${m.height}${m.height>m.width?' · portrait':''}`,on:m.index===r.monitor})),
    it=>{if(it.m.index!==recCfg().monitor)saveRecorder({monitor:it.m.index},[`Recording monitor ${it.m.index+1} · ${it.m.name}`,'video']);},'right');
});
$('#chatDd').addEventListener('click',async e=>{
  const a=e.currentTarget;await loadDevices();const r=recCfg(),apps=(REC.devices&&REC.devices.chatApps)||[];
  openPop(a,[...apps.map(x=>({v:x.name,label:x.name,sub:x.running?'Running now':'Not running right now',on:r.chatApp===x.name})),
    {v:'',label:'None',sub:'Voice chat stays silent; the game track gets all audio',on:!r.chatApp}],
    it=>{if(it.v!==recCfg().chatApp)saveRecorder({chatApp:it.v},[it.v?`Voice chat records ${it.v}`:'No chat app · the game track gets all audio','headset']);},'right');
});
$('#micDd').addEventListener('click',async e=>{
  const a=e.currentTarget;await loadDevices();const r=recCfg(),ins=(REC.devices&&REC.devices.inputs)||[],def=ins.find(i=>i.isDefault);
  openPop(a,[{v:'',label:'Windows default',sub:def?def.name:'Follows the Windows sound settings',on:!r.micDevice},
    ...ins.map(i=>({v:i.id,label:i.name,sub:i.isDefault?'The Windows default right now':'',on:r.micDevice===i.id}))],
    it=>{if(it.v!==recCfg().micDevice)saveRecorder({micDevice:it.v},[`Microphone: ${it.v?it.label:'Windows default'}`,'mic']);},'right');
});
$('#openClipsDir').addEventListener('click',()=>{const r=recCfg();if(r)BT.openPath(r.clipsDir).catch(e=>fail("Couldn't open the folder",e));});
$('#changeClipsDir').addEventListener('click',async()=>{
  const r=recCfg();if(!r)return;
  let p;try{p=await BT.pickFolder({title:'Choose where new clips are saved',defaultPath:r.clipsDir});}catch(e){fail("Couldn't open the folder picker",e);return;}
  if(p&&!sameDir(p,r.clipsDir))saveRecorder({clipsDir:p},[`New clips go to ${shortPath(p,40)}`,'folder']);
});
/* ---------- hotkeys: click, press a combo ("Ctrl+Alt+B"), Backspace clears, Esc cancels */
let hkEdit=null;
function renderHotkeys(force){
  const r=recCfg(),box=$('#hkRows');if(!r)return;
  if(hkEdit&&!force)return;   // never redraw under a capture in progress
  if(hkEdit){hkEdit=null;hkSuspend(false);}
  box.innerHTML=HK.map(([a,label])=>{const keys=r.hotkeys[a]||'',st=hkState(a),bad=st&&!st.ok;
    return `<div class="srow" data-hkrow="${a}"><div class="sl"><b>${label}</b><span>${esc(HK_SUB[a]())}${bad?`<span class="hk-err">${esc(st.error||`${keys} isn't working`)}</span>`:''}</span></div>`+
      (keys?`<button class="icon-btn hk-clear" data-hkclear="${a}" data-tip="Remove this hotkey" aria-label="Remove the ${label} hotkey">${ic('x','sm')}</button>`:'')+
      `<button class="keys${keys?'':' unset'}${bad?' bad':''}" data-hk="${a}" aria-label="${label}: ${keys||'not set'}. Click to change">${keys?keys.split('+').map(k=>`<kbd>${esc(k)}</kbd>`).join('<span class="plus">+</span>'):'Not set'}${ic('pencil','xs edit')}</button></div>`;}).join('');
  REC.hkSig=hkSig();
}
// Backtrack's own global hotkeys would swallow the combo being captured; pause them meanwhile.
function hkSuspend(on){try{const p=BT.suspendHotkeys&&BT.suspendHotkeys(on);if(p&&p.catch)p.catch(()=>{});}catch(_){}}
function startHk(a,el){cancelHk();hkEdit={a,el};hkSuspend(true);el.classList.add('listening');el.classList.remove('bad','unset');el.innerHTML='Press a key combo…';el.title='Esc cancels · Backspace removes the hotkey';}
function cancelHk(){if(!hkEdit)return;hkEdit=null;hkSuspend(false);renderHotkeys(true);}
function clearHk(a){const r=recCfg();if(!r||!r.hotkeys[a])return;saveRecorder({hotkeys:Object.assign({},r.hotkeys,{[a]:''})},[`${HK_LABEL(a)} has no hotkey now`,'keyboard']);}
const HK_KEY=c=>{let m;
  if((m=c.match(/^Key([A-Z])$/)))return m[1];if((m=c.match(/^Digit(\d)$/)))return m[1];if(/^F([1-9]|1\d|2[0-4])$/.test(c))return c;
  if((m=c.match(/^Numpad(\d)$/)))return 'Numpad'+m[1];if((m=c.match(/^Arrow(Up|Down|Left|Right)$/)))return m[1];
  return ['Space','Home','End','PageUp','PageDown','Insert','Delete'].includes(c)?c:null;};
function hkKey(e){
  if(!hkEdit)return false;
  e.preventDefault();e.stopPropagation();
  const mods=[];if(e.ctrlKey)mods.push('Ctrl');if(e.altKey)mods.push('Alt');if(e.shiftKey)mods.push('Shift');if(e.metaKey)mods.push('Win');
  if(['Control','Alt','Shift','Meta','OS','AltGraph'].includes(e.key)){hkEdit.el.innerHTML=mods.map(m=>`<kbd>${m}</kbd>`).join('<span class="plus">+</span>')+'<span class="plus">+ …</span>';return true;}
  if(!mods.length&&e.key==='Escape'){cancelHk();return true;}
  if(!mods.length&&e.key==='Backspace'){const a=hkEdit.a;cancelHk();clearHk(a);return true;}
  const key=HK_KEY(e.code||'');
  if(!key){toast(`${e.key===' '?'Space':e.key} can't be a hotkey · use F1–F24, or a letter or number with Ctrl or Alt`,'keyboard');return true;}
  if(/^([A-Z0-9]|Space)$/.test(key)&&!mods.some(m=>m!=='Shift')){toast('Add Ctrl or Alt to letters, numbers and Space, so typing in a game doesn’t trigger it','keyboard');return true;}
  const combo=[...mods,key].join('+'),a=hkEdit.a,r=recCfg();
  const other=HK.find(([b])=>b!==a&&(r.hotkeys[b]||'').toLowerCase()===combo.toLowerCase());
  if(other){toast(`${combo} is already used for “${other[1]}”`,'alert');return true;}
  hkEdit=null;hkSuspend(false);
  if(r.hotkeys[a]===combo){renderHotkeys(true);return true;}
  saveRecorder({hotkeys:Object.assign({},r.hotkeys,{[a]:combo})},[`${HK_LABEL(a)}: ${combo}`,'keyboard']);
  return true;
}
$('#hkRows').addEventListener('click',e=>{
  const c=e.target.closest('[data-hkclear]');if(c){clearHk(c.dataset.hkclear);return;}
  const b=e.target.closest('[data-hk]');if(!b)return;if(hkEdit&&hkEdit.el===b){cancelHk();return;}startHk(b.dataset.hk,b);
});
document.addEventListener('pointerdown',e=>{if(hkEdit&&!hkEdit.el.contains(e.target))cancelHk();},true);
/* ---------- clip-saved: toast with Open; the clip opens once library-changed has indexed it (matched on path) */
const clipByPath=p=>{const l=String(p||'').toLowerCase();return S.clips.find(c=>c.path.toLowerCase()===l)||null;};
function openSavedClip(path){
  const c=clipByPath(path);if(c){openClip(c);go('editor');return;}
  REC.pendingOpen={path,until:Date.now()+30000};toast('Opening it as soon as it’s ready…','clock');
}
function openPendingClip(){
  const p=REC.pendingOpen;if(!p)return;if(Date.now()>p.until){REC.pendingOpen=null;return;}
  const c=clipByPath(p.path);if(!c)return;REC.pendingOpen=null;
  if(!exp.running&&$('#confirmLayer').hidden){openClip(c);go('editor');}
}
BT.on('clip-saved',p=>{
  if(!p)return;
  toast(`Clip saved · ${p.game||'Clip'} · ${Math.round(+p.duration||0)} s`,'check-c',{label:'Open',icon:'play',fn:()=>openSavedClip(p.path)});
});
BT.on('recorder-status',applyStatus);

/* ================================================================== TITLE BAR */
const winCall=(f,what)=>f().catch(e=>BT.log('warn',`window ${what} failed: ${errMsg(e)}`));
$('#winMin').addEventListener('click',()=>winCall(BT.window.minimize,'minimize'));
$('#winMax').addEventListener('click',()=>winCall(BT.window.toggleMaximize,'toggleMaximize'));
$('#winClose').addEventListener('click',()=>{flushSave();winCall(BT.window.close,'close');});
// Tauri drags elements that carry data-tauri-drag-region; this covers their other children (icons, text spans)
$('#titlebar').addEventListener('mousedown',e=>{
  if(e.button!==0||BT.isMock||e.target.closest('button,a,input'))return;
  if(e.target.hasAttribute&&e.target.hasAttribute('data-tauri-drag-region'))return;
  if(e.detail===2)winCall(BT.window.toggleMaximize,'toggleMaximize');else winCall(BT.window.startDragging,'startDragging');
});

/* ================================================================== ROUTER + KEYS */
function cur(){return (location.hash||'#library').slice(1).split('?')[0]||'library';}
function go(r){if(cur()===r)route();else location.hash=r;}
function updateCrumb(){const r=cur();$('#crumb').textContent=r.startsWith('settings')?'Settings':r==='library'||!ed.clip?'Library':`Editor · ${ed.clip.game}`;}
const ROUTES=['library','editor','export','export-done','settings'];
function route(){
  let r=cur(),deep=null;const dm=r.match(/^settings-([\w-]+)$/);if(dm){deep=dm[1];r='settings';}
  if(!ROUTES.includes(r))r='library';
  const ml=$('#modalLayer'),m=$('#modal'),wasHidden=ml.hidden;
  // until the first list_clips answers, everything but Settings shows the library's loading state (the hash is kept)
  if(!S.loaded&&!(r==='settings'&&S.settings)){
    $$('.screen').forEach(s=>s.hidden=s.dataset.screen!=='library');ml.hidden=true;$('#app').inert=false;renderLibrary();updateCrumb();return;
  }
  if((r==='editor'||r.startsWith('export'))&&!ed.clip){
    if(S.clips.length)openClip(S.clips[0]);   // #editor without a selected clip opens the newest
    else{location.replace('#library');return;}
  }
  const screen=r.startsWith('export')?'editor':r,from=lastMain;
  if(screen!=='settings'){lastMain=screen;cancelHk();}
  if(screen!=='editor')pause();
  $$('.screen').forEach(s=>s.hidden=s.dataset.screen!==screen);
  $('#tbSettings').classList.toggle('on',screen==='settings');
  closePop();
  if(screen==='library'){renderLibrary();   // back from the editor: keyboard focus returns to the clip you opened
    const fc=from==='editor'&&ed.clip&&$(`#libScroll .card[data-clip="${CSS.escape(ed.clip.id)}"]`);if(fc&&(document.activeElement===document.body||!document.activeElement))fc.focus({preventScroll:true});}
  if(screen==='settings'){renderSettings();if(deep)scrollSettingsTo(deep,false);loadDevices();}
  if(screen==='editor')nextFrame(()=>{renderTimeline();positionCrop();fitInspector();});
  if(r==='export'){
    pause();ml.hidden=false;
    if(m.dataset.view!=='progress'&&m.dataset.view!=='error'){m.dataset.view='setup';$('#fname').value=defaultName();$('#autoCopy').checked=S.settings.autoCopy;renderModal();}
  } else if(r==='export-done'){
    pause();ml.hidden=false;
    if(exp.running)m.dataset.view='progress';
    else if(exp.result){m.dataset.view='done';renderDone();}
    else if(m.dataset.view==='error'){}
    else if(BT.isMock){   // a direct link in the mock: run a real (mock) export of the selection, then land here
      m.dataset.view='progress';$('#fname').value=defaultName();$('#autoCopy').checked=S.settings.autoCopy;
      setTimeout(()=>{if(!exp.running&&!exp.result)runExport(buildRequest());},0);
    } else {location.replace('#editor');return;}
  } else {
    if(exp.running)cancelRunning();
    ml.hidden=true;exp.direct=false;exp.error=null;m.dataset.view='setup';
  }
  // while a dialog is open, nothing underneath can take focus, clicks or keys
  $('#app').inert=!ml.hidden||!$('#confirmLayer').hidden;
  if(!ml.hidden&&(wasHidden||!m.contains(document.activeElement)))m.focus({preventScroll:true});
  updateCrumb();
}
window.addEventListener('hashchange',route);
document.addEventListener('keydown',e=>{
  if(hkKey(e))return;   // Settings → Hotkeys is listening for a new combo
  if(!$('#confirmLayer').hidden){if(e.key==='Escape'){e.preventDefault();closeConfirm(false);}else if(e.key==='Tab')cfTrap(e);
    else if(e.key==='Enter'&&e.target.closest&&e.target.closest('[role="checkbox"]'))e.preventDefault();   // a checkbox toggles on Space or a click, never Enter
    return;}   // Enter/Space act on the focused button
  const r=cur(),tag=(e.target.tagName||'').toLowerCase(),typing=(tag==='input'&&e.target.type!=='range'&&e.target.type!=='checkbox')||tag==='textarea';
  if(e.key==='Escape'&&popEl){closePop();return;}
  if(r==='library'){
    const card=!typing&&e.target.closest?e.target.closest('#libScroll .card'):null,id=card&&card.dataset.clip;
    if(e.key!=='Shift'&&!(e.shiftKey&&/^(Arrow\w+|Home|End)$/.test(e.key)))sel.base=null;   // any other key ends a Shift+arrow run
    if((e.ctrlKey&&e.key.toLowerCase()==='k')||(e.key==='/'&&!typing)){e.preventDefault();$('#search').focus();$('#search').select();}
    else if(e.key==='Escape'&&typing){e.target.blur();}
    else if(e.key==='Escape'&&lib.filter==='sel'){e.preventDefault();leaveReview();}   // Review: Esc goes back, the selection stays
    else if(e.key==='Escape'&&selOn()){e.preventDefault();const back=$('#selBar').contains(document.activeElement)&&sel.anchor;clearSel();
      const el=back&&$(`#libScroll .card[data-clip="${CSS.escape(back)}"]`);if(el)el.focus();}
    else if(typing||e.altKey)return;
    else if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='z'&&libUndo){e.preventDefault();runUndo();}
    else if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='a'){e.preventDefault();selectView(true);}
    else if(e.key==='Delete'){if(selOn()){e.preventDefault();trashSelected();}else if(id){e.preventDefault();trashClip(S.byId[id]);}}
    else if(e.key===' '&&id){e.preventDefault();
      if(e.shiftKey&&sel.anchor){const rr=rangeIds(sel.anchor,id);if(rr){sel.ranged=true;setSel(rr,true,true);return;}}
      selClick(id);}
    else if(card&&!e.ctrlKey&&!e.metaKey&&/^(ArrowLeft|ArrowRight|ArrowUp|ArrowDown|Home|End)$/.test(e.key)){
      e.preventDefault();const t=cardNav(card,e.key);if(!t)return;
      if(e.shiftKey&&id&&t.dataset.clip){   // Shift+arrows: the selection as it was, plus anchor → focus (moving back shrinks it)
        if(!sel.base){sel.base=new Set(sel.ids);if(!sel.anchor||!rangeIds(sel.anchor,id))sel.anchor=id;}
        const rr=rangeIds(sel.anchor,t.dataset.clip)||[t.dataset.clip];rr.forEach(x=>{if(!sel.base.has(x))sel.bulk.add(x);});
        sel.ids=new Set([...sel.base,...rr]);sel.ranged=true;syncSel();
      }
      t.focus({preventScroll:true});t.scrollIntoView({block:'nearest'});
    }
    // Enter opens, in both modes (Space is the toggle, as in Explorer); on a "+N more" tile it opens the game
    else if(e.key==='Enter'&&card&&(e.target===card||e.target.tagName!=='BUTTON')){e.preventDefault();if(id){openClip(S.byId[id]);go('editor');}else card.click();}
    return;}
  if(r==='export'||r==='export-done'){
    // keys never fall through to the editor: Space/Enter only act on a focused control inside the dialog
    const m=$('#modal'),view=m.dataset.view,onCtl=m.contains(e.target)&&e.target!==m&&/^(button|a|label)$/i.test(e.target.tagName);
    if(e.key===' '&&!typing&&!onCtl){e.preventDefault();return;}
    if(e.key==='Escape'){e.preventDefault();if(view==='progress')$('#pCancel').click();else go('editor');return;}
    if(view==='setup'&&!typing&&(e.ctrlKey||e.metaKey)&&['z','y'].includes(e.key.toLowerCase())){e.preventDefault();(e.key.toLowerCase()==='y'||e.shiftKey)?redoTrim():undoTrim();return;}
    if(e.key==='Enter'){
      if(onCtl)return;   // let the focused button do its own thing
      e.preventDefault();
      if(view==='setup'&&!(typing&&e.target.id==='customMB'))$('#mExport').click();
      else if(view==='done')go('editor');
      else if(view==='error')$('#eRetry').click();
      return;}
    if(view==='done'&&(e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='c'&&!typing){e.preventDefault();copyResult(false);}
    return;}
  if(r==='editor'&&!typing){
    if((e.ctrlKey||e.metaKey)&&['z','y'].includes(e.key.toLowerCase())){e.preventDefault();(e.key.toLowerCase()==='y'||e.shiftKey)?redoTrim():undoTrim();return;}
    if(e.ctrlKey||e.metaKey)return;
    // leaving the editor takes a deliberate Alt+← (UX review r2 #2: Backspace/Delete now remove a marker instead)
    if(e.altKey&&e.key==='ArrowLeft'){e.preventDefault();pause();go('library');return;}
    if(e.altKey)return;
    if(e.key==='?'){e.preventDefault();openKeys($('#keysBtn'));return;}
    switch(e.key){
      case ' ':e.preventDefault();toggle();break;
      case 'i':case 'I':setIn();break;
      case 'o':case 'O':setOut();break;
      case 'e':case 'E':e.preventDefault();if(e.shiftKey)go('export');else exportNow();break;
      case 'm':case 'M':addMarker();break;
      case 'Delete':case 'Backspace':e.preventDefault();removeMarker();break;
      case 'l':case 'L':$('#loopBtn').click();break;
      case 'f':case 'F':toggleFullscreen();break;
      case 'ArrowLeft':e.preventDefault();step(-1,e.shiftKey);break;
      case 'ArrowRight':e.preventDefault();step(1,e.shiftKey);break;
      case 'Home':e.preventDefault();seek(ed.inT);break;
      case 'End':e.preventDefault();seek(ed.outT);break;
      case '+':case '=':setZoom(ed.zoom*1.5);break;
      case '-':setZoom(ed.zoom/1.5);break;
      case 'Escape':hideGhost();if(document.activeElement&&document.activeElement!==document.body)document.activeElement.blur();break;
    }
  }
});
new ResizeObserver(()=>{if(cur()==='editor'||cur().startsWith('export')){renderTimeline();positionCrop();}}).observe(els.scroll);
new ResizeObserver(()=>positionCrop()).observe($('#preview'));
{const ro=new ResizeObserver(()=>fitSoon());ro.observe($('#inspector'));ro.observe($('.export-card'));}
window.addEventListener('beforeunload',()=>{flushSave();});

/* ================================================================== DATA: first load + library-changed */
async function loadAll(){
  S.loaded=false;S.loadError=null;route();
  try{
    if(!S.settings){
      S.settings=await BT.getSettings();ed.customMB=S.settings.customMb;
      BT.recorderStatus().then(applyStatus,e=>BT.log('warn','recorder_status failed: '+errMsg(e)));
      renderRecorder();route();
    }
    setClips(await BT.listClips());
  }catch(e){
    S.loadError=errMsg(e);BT.log('error','initial load failed: '+S.loadError);
    if(!S.settings)S.settings={clipDirs:[],exportDir:'',defaultPreset:'d10',customMb:25,autoCopy:true,quickLen:20,ffmpegPath:''};
  }
  S.loaded=true;route();
}
let refetchT=0,refetching=false,refetchAgain=false;
async function refetch(){
  if(refetching){refetchAgain=true;return;}refetching=true;
  try{
    const [s,list]=await Promise.all([BT.getSettings(),BT.listClips()]);
    S.settings=s;
    // keep the open clip's unsaved edits; the rest of the editor state (In/Out, playhead, mix) stays as it is
    if(ed.clip&&savePending){const n=list.find(x=>x.id===savePending.clipId);if(n)n.edits=savePending.edits;}
    setClips(list);S.loaded=true;S.loadError=null;
    if(ed.clip){
      const n=S.byId[ed.clip.id];
      if(n){const thumbChanged=n.thumb!==ed.clip.thumb;if(ed.clip.edits)n.edits=ed.clip.edits;ed.clip=n;syncClipChrome();if(thumbChanged&&n.thumb){$('#pvImg').src=BT.src(n.thumb);$('#pvImg').hidden=false;$('#pvPh').hidden=true;renderFilm();}}
      else if(!exp.running){
        const inEd=cur()==='editor'||cur().startsWith('export');closeClip();
        if(inEd){toast('This clip was moved or deleted outside Backtrack','alert');go('library');}
      }
    }
    const r=cur();
    if(r==='library')renderLibrary(true);else renderSide();
    if(r.startsWith('settings'))renderSettings();
  }catch(e){BT.log('warn','refresh after library-changed failed: '+errMsg(e));}
  finally{refetching=false;openPendingClip();if(refetchAgain){refetchAgain=false;refetch();}}
}
BT.on('library-changed',()=>{clearTimeout(refetchT);refetchT=setTimeout(refetch,200);});
BT.appVersion().then(v=>{S.version=v||'';if(cur().startsWith('settings'))renderSettings();}).catch(()=>{});

renderSide();
loadAll();
})();
