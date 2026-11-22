// Clippy's brain (the buddy window). Rust does the I/O; this file decides what Clippy says and when.
// Every integration is optional: a watcher only runs while its integration is on in Settings.
(async () => {
const T = window.__TAURI__;
const invoke = T.core.invoke;
const { listen, emit } = T.event;
const win = T.window.getCurrentWindow();
const $ = (s) => document.querySelector(s);

let S = await invoke('get_settings');
PERSONALITY = S.personality;
const st = {
  sleeping: false, held: [], focusEnd: 0, focusTick: null, focusOnScreen: false,
  hideTimer: null, keysOK: null, lastTip: {}, lastTipAny: 0,
  data: {},          // latest answer from each integration (for the chat's context and the tiles)
  prev: {},          // what each watcher saw last time, to spot changes
  svc: {},           // service checks: fails per service
  watches: [],       // custom actions being watched
};
const CAT = { homelab: 'homelab alerts', downloads: 'download alerts', media: 'Jellyfin news', stocks: 'stock alerts', tips: 'Windows → Mac tips', health: 'Mac health warnings' };
const SIZES = { small: 54, medium: 70, large: 92 };
const on = (n) => !!S.integrations?.[n]?.on;

// ---------- drawing + window size ----------
$('#art').innerHTML = clippySVG('track');
function applySize() { document.documentElement.style.setProperty('--cw', (SIZES[S.size] || 70) + 'px'); fit(); }
let fitQueued = false;
function fit() {
  if (fitQueued) return;
  fitQueued = true;
  requestAnimationFrame(() => {
    fitQueued = false;
    const r = $('#stage').getBoundingClientRect();
    invoke('buddy_fit', { width: Math.ceil(r.width) + 2, height: Math.ceil(r.height) + 2 });
  });
}
applySize();
win.show();

// ---------- bubble ----------
const bubble = $('#bubble');
function hop() { const b = $('#clippy'); b.classList.remove('hop'); void b.offsetWidth; b.classList.add('hop'); }
/** acts: [{label, primary, fn}]. opts: {extra (html), stay (ms, 0 = until closed), worried}. */
function say(html, acts, opts = {}) {
  wake(true);
  clearTimeout(st.hideTimer);
  bubble.hidden = false;
  bubble.innerHTML = `<button class="x" aria-label="Close">×</button><p>${html}</p>${opts.extra || ''}` +
    (acts && acts.length ? '<div class="acts">' + acts.map((a, i) => `<button data-i="${i}" class="${a.primary ? 'primary' : ''}">${esc(a.label)}</button>`).join('') + '</div>' : '');
  bubble.querySelector('.x').onclick = hush;
  (acts || []).forEach((a, i) => { bubble.querySelector(`.acts [data-i="${i}"]`).onclick = () => a.fn(); });
  document.body.classList.toggle('worried', !!opts.worried);
  const stay = opts.stay ?? (acts && acts.length ? 90000 : 20000);
  if (stay) st.hideTimer = setTimeout(hush, stay);
  hop();
  fit();
  return bubble;
}
function hush() { bubble.hidden = true; bubble.innerHTML = ''; document.body.classList.remove('worried'); fit(); }
const fail = (e) => say(L('failed', { err: esc(e) }), null, { worried: true });

// ---------- settings ----------
async function save() { await invoke('save_settings', { settings: S }); }
listen('settings', (e) => { S = e.payload; PERSONALITY = S.personality; applySize(); status(); });

// ---------- the Lian Li screen (optional) ----------
async function toScreen(head, sub, tone) {
  if (!on('lianli') || !S.mirror_to_screen) return false;
  try {
    const r = await invoke('integ', { name: 'lianli', action: 'say', args: { head, sub: sub || '', tone: tone || 'info', seconds: S.screen_seconds || 8, source: 'Clippy' } });
    return r.screen_off ? 'off' : true;
  } catch { return false; }
}
const plain = (h) => String(h).replace(/<[^>]+>/g, '');

// ---------- notifications ----------
/** n: {kind, html, acts, screen: [head, sub, tone], summary, worried} */
function notify(n) {
  if (n.kind && !S.kinds[n.kind]) return;
  if (st.sleeping || st.focusEnd) { st.held.push(n.summary || plain(n.html)); return; }
  say(n.html, n.acts, { worried: n.worried });
  if (n.screen) toScreen(...n.screen);
}
function notNow(kind) {
  hush();
  if (!S.quiet_two || !kind) return;
  S.ignores[kind] = (S.ignores[kind] || 0) + 1;
  if (S.ignores[kind] >= 2) { S.kinds[kind] = false; S.ignores[kind] = 0; say(L('quiet_kind', { cat: CAT[kind] })); }
  save();
}
const later = (kind) => ({ label: 'Not now', fn: () => notNow(kind) });
const okBtn = { label: 'OK', primary: true, fn: hush };

// ---------- watchers ----------
// run(fn, everyMs): calls fn now-ish and then on a timer; fn only does work while its integration is on.
function every(ms, fn, firstDelay) { setTimeout(() => { fn().catch(() => {}); setInterval(() => fn().catch(() => {}), ms); }, firstDelay ?? 3000); }

every(60000, async () => {                 // Proxmox: a running VM/container stopped
  if (!on('proxmox')) { delete st.data.proxmox; return; }
  const r = await invoke('integ', { name: 'proxmox', action: 'status' });
  st.data.proxmox = r;
  const now = Object.fromEntries(r.guests.map((g) => [g.vmid, g]));
  if (st.prev.proxmox) for (const g of r.guests) {
    const was = st.prev.proxmox[g.vmid];
    if (was && was.running && !g.running) notify({ kind: 'homelab', worried: true, summary: `${g.name} stopped`,
      html: L('guest_down', { name: esc(g.name || g.vmid), node: esc(g.node) }), screen: [`${g.name} stopped`, `Proxmox · ${g.node}`, 'warn'],
      acts: [{ label: 'Start it', primary: true, fn: () => startGuest(g) }, later('homelab')] });
  }
  st.prev.proxmox = now;
}, 4000);
async function startGuest(g) {
  say(L('working'), null, { stay: 0 });
  try { await invoke('integ', { name: 'proxmox', action: 'start', args: { node: g.node, type: g.type, vmid: g.vmid } }); say(L('start_ok', { name: esc(g.name) })); }
  catch (e) { fail(e); }
}

every(60000, async () => {                 // Portainer: a running container exited or went unhealthy
  if (!on('portainer')) { delete st.data.portainer; return; }
  const r = await invoke('integ', { name: 'portainer', action: 'status' });
  st.data.portainer = r;
  const now = Object.fromEntries(r.containers.map((c) => [c.name, c]));
  if (st.prev.portainer) for (const c of r.containers) {
    const was = st.prev.portainer[c.name];
    if (!was) continue;
    const bad = c.state !== 'running' || c.unhealthy, wasBad = was.state !== 'running' || was.unhealthy;
    if (bad && !wasBad) notify({ kind: 'homelab', worried: true, summary: `${c.name}: ${c.status}`,
      html: L('container_down', { name: esc(c.name), what: c.unhealthy ? 'is unhealthy' : 'stopped (' + esc(c.status) + ')' }),
      screen: [`${c.name} ${c.unhealthy ? 'unhealthy' : 'stopped'}`, 'Docker', 'warn'],
      acts: [{ label: 'Restart it', primary: true, fn: () => restartContainer(c) }, later('homelab')] });
    else if (!bad && wasBad) notify({ kind: 'homelab', html: L('container_back', { name: esc(c.name) }), summary: `${c.name} is back` });
  }
  st.prev.portainer = now;
}, 5000);
async function restartContainer(c) {
  say(L('working'), null, { stay: 0 });
  try { await invoke('integ', { name: 'portainer', action: 'restart', args: { id: c.id } }); say(L('restart_ok', { name: esc(c.name) })); toScreen(`${c.name} restarted`, 'From Clippy', 'good'); }
  catch (e) { fail(e); }
}

async function checkServices(quiet) {
  const list = await invoke('check_services');
  st.data.checks = list;
  for (const s of list) {
    const o = st.svc[s.name] || (st.svc[s.name] = { fails: 0, told: false });
    if (s.up) {
      if (o.told && !quiet) notify({ kind: 'homelab', html: L('service_up', { name: esc(s.name) }), summary: s.name + ' is back' });
      o.fails = 0; o.told = false;
    } else if (++o.fails === 2 && !quiet) {
      o.told = true;
      notify({ kind: 'homelab', worried: true, summary: s.name + ' is not answering', html: L('service_down', { name: esc(s.name) }),
        screen: [`${s.name} is down`, 'Service check', 'warn'],
        acts: [{ label: 'Open it', primary: true, fn: () => { invoke('open_url', { url: s.url }); hush(); } }, later('homelab')] });
    }
  }
  return list;
}
every(60000, async () => { if (S.checks_on && S.checks.length) await checkServices(false); else delete st.data.checks; }, 6000);

every(15000, async () => {                 // Lian Li screen container: its alerts and finished downloads
  if (!on('lianli')) { delete st.data.lianli; return; }
  try {
    const f = await invoke('integ', { name: 'lianli', action: 'feed' });
    st.data.lianli = f; st.lianliFails = 0;
    const ids = new Set((f.events || []).map((e) => e.id));
    if (st.prev.lianli) for (const ev of f.events || []) if (!st.prev.lianli.has(ev.id)) screenEvent(ev, f);
    st.prev.lianli = ids;
  } catch (e) { st.data.lianli = null; st.lianliErr = String(e); }
  status();
}, 2500);
function screenEvent(ev, f) {
  if (ev.kind === 'downloads') return notify({ kind: 'downloads', summary: 'finished: ' + ev.text, html: L('download_done', { title: esc(ev.text) }), acts: [okBtn, later('downloads')] });
  if (ev.level === 'info' && ev.kind === 'homelab') return;
  const m = /^Container (\S+) /.exec(ev.text);
  const c = m && (f.containers || []).find((x) => x.name === m[1]);
  notify({ kind: 'homelab', worried: ev.kind === 'homelab', summary: ev.text,
    html: c ? L('container_down', { name: esc(c.name), what: esc(ev.text.slice(m[0].length)) }) : L('screen_alert', { source: esc(ev.source || 'The screen'), text: esc(ev.text) }),
    acts: c ? [{ label: 'Restart it', primary: true, fn: () => restartViaScreen(c.name) }, later('homelab')] : [okBtn, later('homelab')] });
}
async function restartViaScreen(name) {
  say(L('working'), null, { stay: 0 });
  try { const r = await invoke('integ', { name: 'lianli', action: 'restart', args: { name } }); r.ok ? say(L('restart_ok', { name: esc(name) })) : fail(r.message); }
  catch (e) { fail(e); }
}

every(30000, async () => {                 // qBittorrent: torrents that just reached 100%
  if (!on('qbittorrent')) { delete st.data.qbittorrent; return; }
  const r = await invoke('integ', { name: 'qbittorrent', action: 'status' });
  st.data.qbittorrent = r;
  const now = Object.fromEntries(r.torrents.map((t) => [t.hash, t.progress]));
  if (st.prev.qbit) for (const t of r.torrents) {
    if (st.prev.qbit[t.hash] !== undefined && st.prev.qbit[t.hash] < 100 && t.progress >= 100)
      notify({ kind: 'downloads', summary: 'finished: ' + t.name, html: L('download_done', { title: esc(t.name) }), screen: ['Download finished', t.name, 'good'], acts: [okBtn, later('downloads')] });
  }
  st.prev.qbit = now;
}, 7000);

for (const name of ['radarr', 'sonarr']) every(300000, async () => {   // imported into the library
  if (!on(name)) { delete st.data[name]; return; }
  const r = await invoke('integ', { name, action: 'status' });
  st.data[name] = r;
  const ids = new Set(r.imported.map((x) => x.id));
  if (st.prev[name]) for (const x of r.imported) if (!st.prev[name].has(x.id))
    notify({ kind: 'downloads', summary: 'ready: ' + x.title, html: L('imported', { title: esc(x.title) }), screen: ['Ready to watch', x.title, 'good'], acts: [okBtn, later('downloads')] });
  st.prev[name] = ids;
}, name === 'radarr' ? 8000 : 9000);

every(300000, async () => {                // Jellyfin: newly added movies/episodes
  if (!on('jellyfin')) { delete st.data.jellyfin; return; }
  const r = await invoke('integ', { name: 'jellyfin', action: 'status' });
  st.data.jellyfin = r;
  const ids = new Set(r.latest.map((x) => x.id));
  if (st.prev.jellyfin) for (const x of r.latest) if (!st.prev.jellyfin.has(x.id))
    notify({ kind: 'media', summary: 'new: ' + x.title, html: L('new_media', { title: esc(x.title) }), acts: [okBtn, later('media')] });
  st.prev.jellyfin = ids;
}, 10000);

const money = (q) => `${q.price.toLocaleString(undefined, { maximumFractionDigits: 2 })} ${q.currency || ''}`.trim();
every(600000, async () => {                // Stocks: a big move today (once per symbol per day)
  if (!on('stocks')) { delete st.data.stocks; return; }
  const r = await invoke('integ', { name: 'stocks', action: 'status' });
  st.data.stocks = r;
  const day = new Date().toDateString();
  let told = {}; try { told = JSON.parse(localStorage.getItem('stockTold') || '{}'); } catch {}
  for (const q of r.quotes) {
    if (q.error || Math.abs(q.change) < r.alert || told[q.symbol] === day) continue;
    told[q.symbol] = day;
    notify({ kind: 'stocks', summary: `${q.symbol} ${q.change > 0 ? '+' : ''}${q.change.toFixed(1)}%`,
      html: L('stock_move', { sym: esc(q.symbol), dir: q.change > 0 ? 'up' : 'down', pct: Math.abs(q.change).toFixed(1), price: esc(money(q)) }),
      screen: [`${q.symbol} ${q.change > 0 ? '+' : ''}${q.change.toFixed(1)}%`, money(q), q.change > 0 ? 'good' : 'warn'], acts: [okBtn, later('stocks')] });
  }
  try { localStorage.setItem('stockTold', JSON.stringify(told)); } catch {}
}, 12000);

every(1800000, async () => {               // Weather (for the tile and the chat)
  if (!on('weather')) { delete st.data.weather; return; }
  st.data.weather = await invoke('integ', { name: 'weather', action: 'status' });
}, 5000);
every(300000, async () => {                // Pi-hole
  if (!on('pihole')) { delete st.data.pihole; return; }
  st.data.pihole = await invoke('integ', { name: 'pihole', action: 'status' });
}, 11000);
every(1800000, async () => {               // Mac health: disk nearly full (once a day)
  const h = await invoke('mac_health');
  st.data.health = h;
  let last = 0; try { last = +localStorage.getItem('diskWarned') || 0; } catch {}
  if (h.disk >= 90 && Date.now() - last > 86400000) {
    try { localStorage.setItem('diskWarned', Date.now()); } catch {}
    notify({ kind: 'health', worried: true, summary: `disk ${Math.round(h.disk)}% full`, html: L('disk_full', { pct: Math.round(h.disk), free: h.diskFreeGB.toFixed(0) }),
      acts: [{ label: 'Open Storage', primary: true, fn: () => { invoke('open_url', { url: 'x-apple.systempreferences:com.apple.settings.Storage' }); hush(); } }, later('health')] });
  }
}, 15000);

// ---------- what the chat can see ----------
const WMO = { 0: 'clear', 1: 'mostly clear', 2: 'partly cloudy', 3: 'overcast', 45: 'fog', 48: 'fog', 51: 'light drizzle', 53: 'drizzle', 55: 'heavy drizzle',
  61: 'light rain', 63: 'rain', 65: 'heavy rain', 71: 'light snow', 73: 'snow', 75: 'heavy snow', 80: 'showers', 81: 'showers', 82: 'heavy showers', 95: 'thunderstorm', 96: 'thunderstorm', 99: 'thunderstorm' };
function contextText() {
  const d = st.data, out = [];
  if (d.health) out.push(`Mac: CPU ${Math.round(d.health.cpu)}%, memory ${Math.round(d.health.mem)}% of ${d.health.memGB} GB, disk ${Math.round(d.health.disk)}% full (${d.health.diskFreeGB.toFixed(0)} GB free), up ${d.health.uptimeH} h.`);
  if (d.proxmox) { const down = d.proxmox.guests.filter((g) => !g.running); out.push(`Proxmox: ${d.proxmox.guests.length - down.length} of ${d.proxmox.guests.length} VMs/containers running${down.length ? '; stopped: ' + down.map((g) => g.name).join(', ') : ''}.`); }
  if (d.portainer) { const bad = d.portainer.containers.filter((c) => c.state !== 'running' || c.unhealthy); out.push(`Docker: ${d.portainer.containers.length - bad.length} of ${d.portainer.containers.length} containers fine${bad.length ? '; problems: ' + bad.map((c) => `${c.name} (${c.status})`).join(', ') : ''}.`); }
  if (d.checks) { const bad = d.checks.filter((s) => !s.up); out.push(`Service checks: ${d.checks.length - bad.length} of ${d.checks.length} answering${bad.length ? '; down: ' + bad.map((s) => s.name).join(', ') : ''}.`); }
  if (d.lianli) { const a = (d.lianli.events || []).filter((e) => e.level !== 'info').slice(0, 5); if (a.length) out.push('Homelab alerts: ' + a.map((e) => e.text).join('; ') + '.'); }
  if (d.qbittorrent) { const act = d.qbittorrent.torrents.filter((t) => t.progress < 100); out.push(`qBittorrent: ${act.length} downloading${act.length ? ' (' + act.slice(0, 4).map((t) => `${t.name} ${Math.round(t.progress)}%`).join(', ') + ')' : ''}.`); }
  for (const n of ['radarr', 'sonarr']) if (d[n]) out.push(`${n === 'radarr' ? 'Radarr (movies)' : 'Sonarr (TV)'}: ${d[n].queue.length} in queue; coming up: ${d[n].upcoming.slice(0, 5).map((u) => `${u.title} ${u.date}`).join(', ') || 'nothing this week'}.`);
  if (d.jellyfin) out.push(`Jellyfin: ${d.jellyfin.playing.length ? 'playing ' + d.jellyfin.playing.map((p) => `${p.title} (${p.user})`).join(', ') : 'nothing playing'}; newest: ${d.jellyfin.latest.slice(0, 4).map((x) => x.title).join(', ')}.`);
  if (d.pihole) out.push(`Pi-hole: ${d.pihole.blocking === 'enabled' ? 'blocking on' : 'blocking ' + d.pihole.blocking}, ${Number(d.pihole.percent || 0).toFixed(1)}% of ${d.pihole.total} queries blocked today.`);
  if (d.stocks) out.push('Stocks: ' + d.stocks.quotes.filter((q) => !q.error).map((q) => `${q.symbol} ${money(q)} (${q.change > 0 ? '+' : ''}${q.change.toFixed(1)}%)`).join(', ') + '.');
  if (d.weather?.current) { const c = d.weather.current, dd = d.weather.daily, u = d.weather.units?.temperature_2m || '°'; out.push(`Weather in ${d.weather.town}: ${Math.round(c.temperature_2m)}${u}, ${WMO[c.weather_code] || 'mixed'}; today ${Math.round(dd.temperature_2m_min[0])}–${Math.round(dd.temperature_2m_max[0])}${u}, rain chance ${dd.precipitation_probability_max[0]}%.`); }
  return out.join('\n');
}
setInterval(() => invoke('set_context', { text: contextText() }), 60000);
setTimeout(() => invoke('set_context', { text: contextText() }), 20000);

// ---------- status for the Deck ----------
function status() {
  const names = Object.entries(S.integrations || {}).filter(([, v]) => v.on).map(([k]) => k);
  let screen = null;
  if (on('lianli')) {
    const f = st.data.lianli;
    const nm = f && (f.dashboards || []).find((d) => d.key === f.active)?.name;
    screen = f ? (f.screen_off ? 'screen is off' : f.message ? 'showing Clippy' : nm || f.active || 'connected') : (st.lianliErr ? (/404|not found/i.test(st.lianliErr) ? 'container needs the Clippy update' : 'can\'t reach it') : 'connecting…');
  }
  emit('status', { count: names.length + (S.checks_on ? 1 : 0), screen, screenOk: !!st.data.lianli, focus: st.focusEnd });
}
listen('status-request', status);

// ---------- helper: tips, idle ----------
const TIPS = {
  'ctrl+c': 'Copy is <b>Cmd+C</b>. Cmd is the key next to the space bar.', 'ctrl+v': 'Paste is <b>Cmd+V</b>.', 'ctrl+x': 'Cut is <b>Cmd+X</b>.',
  'ctrl+z': 'Undo is <b>Cmd+Z</b>, redo is <b>Cmd+Shift+Z</b>.', 'ctrl+a': 'Select all is <b>Cmd+A</b>.', 'ctrl+s': 'Save is <b>Cmd+S</b>.',
  'ctrl+f': 'Find is <b>Cmd+F</b>.', 'ctrl+w': 'Close a tab or window with <b>Cmd+W</b>.', 'ctrl+t': 'A new tab is <b>Cmd+T</b>.',
  'ctrl+n': 'A new window is <b>Cmd+N</b>.', 'ctrl+p': 'Print is <b>Cmd+P</b>.', 'ctrl+r': 'Reload is <b>Cmd+R</b>.',
  'ctrl+y': 'Redo is <b>Cmd+Shift+Z</b> (Cmd+Y in a few apps).', 'ctrl+o': 'Open is <b>Cmd+O</b>.',
  'alt+tab': 'Switch apps with <b>Cmd+Tab</b>; windows of the same app with <b>Cmd+`</b>.', 'alt+f4': 'Quit an app with <b>Cmd+Q</b>. <b>Cmd+W</b> only closes the window.',
};
listen('helper', (e) => {
  const v = e.payload;
  if (v.t === 'keys') { st.keysOK = !!v.ok; try { localStorage.setItem('keysOK', v.ok ? '1' : '0'); } catch {} emit('keys', { ok: !!v.ok }); }
  if (v.t === 'key' && TIPS[v.combo]) {
    const now = Date.now();
    if (now - (st.lastTip[v.combo] || 0) < 600000 || now - st.lastTipAny < 60000) return;
    st.lastTip[v.combo] = now; st.lastTipAny = now;
    const NAMES = { ctrl: 'Ctrl', alt: 'Alt', tab: 'Tab', f4: 'F4' };
    const pretty = v.combo.split('+').map((p) => NAMES[p] || p.toUpperCase()).join('+');
    notify({ kind: 'tips', summary: null, html: L('tip', { combo: esc(pretty), app: v.app ? ' in ' + esc(v.app) : '', tip: TIPS[v.combo] }), acts: [{ label: 'Got it', primary: true, fn: hush }, later('tips')] });
  }
  if (v.t === 'idle') {
    if (!st.sleeping && v.secs >= (S.sleep_minutes || 5) * 60 && !st.focusEnd) sleep();
    else if (st.sleeping && v.secs < 20) wake(false);
  }
});
listen('keys-ask', () => emit('keys', { ok: st.keysOK }));

function sleep() { st.sleeping = true; hush(); document.body.classList.add('sleeping'); }
function wake(quiet) {
  if (!st.sleeping) return;
  st.sleeping = false;
  document.body.classList.remove('sleeping');
  if (!quiet) showHeld(L('welcome'));
}
function showHeld(intro) {
  const h = st.held.filter(Boolean); st.held = [];
  if (!h.length) return;
  const list = h.slice(0, 4).map((x) => `<li><span>${esc(x)}</span></li>`).join('') + (h.length > 4 ? `<li><span>…and ${h.length - 4} more</span></li>` : '');
  say(intro, null, { extra: `<ul>${list}</ul>`, stay: 60000 });
}

// ---------- Clippy himself: click, drag, right-click, eyes, drop ----------
const clip = $('#clippy');
let down = null, dragged = false;
clip.addEventListener('pointerdown', (e) => { if (e.button === 0) { down = { x: e.screenX, y: e.screenY }; dragged = false; } });
clip.addEventListener('pointermove', (e) => {
  if (down && e.buttons === 1 && Math.hypot(e.screenX - down.x, e.screenY - down.y) > 4) { dragged = true; down = null; win.startDragging(); }
});
clip.addEventListener('pointerup', () => { down = null; });
let moveT;
win.onMoved(() => { clearTimeout(moveT); moveT = setTimeout(() => invoke('buddy_moved'), 500); });
clip.addEventListener('click', async () => {
  if (dragged) { dragged = false; return; }
  if (st.sleeping) { wake(false); if (bubble.hidden) say(L('napping')); return; }
  if (S.deck_mode === 'click') { await invoke('deck_toggle'); hop(); return; }
  if (!bubble.hidden) { hush(); return; }
  invoke('show_window', { which: 'chat' });
  hop();
});
clip.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  say(L('greet'), [
    { label: 'Chat', primary: true, fn: () => { hush(); invoke('show_window', { which: 'chat' }); } },
    { label: 'Deck', fn: () => { hush(); invoke('show_window', { which: 'deck' }); } },
    { label: 'Settings', fn: () => { hush(); invoke('show_window', { which: 'settings' }); } },
    { label: 'Hide Clippy', fn: () => { hush(); invoke('hide_window', { which: 'buddy' }); } },
  ]);
});
listen('cursor', (e) => {
  if (st.sleeping) return;
  const svg = clip.querySelector('.clip'); if (!svg) return;
  const r = svg.getBoundingClientRect();
  const cx = r.left + r.width / 2, cy = r.top + r.height * 0.37;
  const dx = e.payload.x - cx, dy = e.payload.y - cy, len = Math.hypot(dx, dy) || 1, k = Math.min(1, len / 220);
  svg.querySelectorAll('.pupil').forEach((p) => { p.style.transform = `translate(${(dx / len * 4 * k).toFixed(2)}px,${(dy / len * 5 * k).toFixed(2)}px)`; });
});
T.webview.getCurrentWebview().onDragDropEvent(async (e) => {
  const t = e.payload.type;
  if (t === 'enter' || t === 'over') document.body.classList.add('drag');
  else if (t === 'leave') document.body.classList.remove('drag');
  else if (t === 'drop') {
    document.body.classList.remove('drag');
    const p = e.payload.paths?.[0]; if (!p) return;
    const c = await invoke('custom_list');
    const a = c.actions.find((x) => x.drop);
    if (!a) return say('Nice file. I don\'t have anything to do with dropped files yet.<span class="meta">Add a custom action with <span class="mono">"drop": true</span> in Settings → Custom actions.</span>');
    runCustom(a, '', p);
  }
});

// ---------- custom actions ----------
async function runCustom(a, input, file) {
  say(L('working'), null, { stay: 0 });
  try {
    const r = await invoke('custom_run', { index: a.index, input, file: file || null });
    const acts = r.open ? [{ label: 'Open', fn: () => { invoke('open_url', { url: r.open }); hush(); } }] : null;
    say(L('sent', { msg: esc(r.message) }) + (r.copy ? `<span class="meta mono">${esc(r.copy)}</span><span class="meta">Copied.</span>` : ''), acts);
    if (r.watch?.length && a.watch) watchCustom(a, r.watch);
  } catch (e) { fail(e); }
}
function watchCustom(a, ids) {
  const w = { a, ids: new Set(ids) };
  st.watches.push(w);
  const tick = async () => {
    try {
      const list = await invoke('custom_watch', { index: a.index, ids: [...w.ids] });
      for (const j of list) {
        if (j.state === 'done') { w.ids.delete(j.id); notify({ kind: 'downloads', summary: `${a.name}: ${j.title}`, html: j.text ? esc(j.text) : L('download_done', { title: esc(j.title || a.name) }), acts: [okBtn], screen: [a.name + ' done', j.title, 'good'] }); }
        else if (j.state === 'failed') { w.ids.delete(j.id); say(L('failed', { err: `${esc(j.title || a.name)} (${esc(j.status)})` }), null, { worried: true }); }
      }
    } catch {}
    if (w.ids.size) setTimeout(tick, (a.watch || 15) * 1000); else st.watches = st.watches.filter((x) => x !== w);
  };
  setTimeout(tick, (a.watch || 15) * 1000);
}
function askCustom(a) {
  if (a.input === 'none') return runCustom(a, '', null);
  if (a.input === 'file') return say(esc(a.prompt || 'Drag a file onto me, or choose one:'), [{ label: 'Choose a file…', primary: true, fn: async () => {
    const p = await T.dialog.open({ multiple: false, directory: false });
    if (p) runCustom(a, '', typeof p === 'string' ? p : p.path); else hush();
  } }]);
  const isUrl = a.input === 'url';
  say(esc(a.prompt || (isUrl ? 'Which link?' : 'What should I send?')), null, { stay: 0,
    extra: `<form id="cf"><input id="ci" placeholder="${esc(a.placeholder || (isUrl ? 'https://…' : ''))}" autocomplete="off">${isUrl ? '<button type="button" id="cp" class="ghost">Paste</button>' : ''}<button>Send</button></form>` });
  const i = $('#ci'); win.setFocus(); setTimeout(() => i.focus(), 50);
  if (isUrl) $('#cp').onclick = async () => { i.value = (await invoke('paste_text')).trim(); i.focus(); };
  $('#cf').onsubmit = (e) => { e.preventDefault(); const v = i.value.trim(); if (v) runCustom(a, v, null); };
}

// ---------- Deck tiles ----------
async function homelab() {
  say(L('checking'), null, { stay: 0 });
  const rows = [];
  const add = (cls, text) => rows.push(`<li class="${cls}"><span>${esc(text)}</span></li>`);
  const tasks = [];
  if (on('proxmox')) tasks.push(invoke('integ', { name: 'proxmox', action: 'status' }).then((r) => { st.data.proxmox = r; r.nodes.forEach((n) => add(n.online ? 'ok' : 'bad', `${n.name}: CPU ${Math.round(n.cpu)}%, memory ${Math.round(n.mem)}%`)); r.guests.filter((g) => !g.running).forEach((g) => add('bad', `${g.name} is stopped`)); if (r.guests.every((g) => g.running)) add('ok', `All ${r.guests.length} VMs/containers running`); }).catch((e) => add('bad', 'Proxmox: ' + e)));
  if (on('portainer')) tasks.push(invoke('integ', { name: 'portainer', action: 'status' }).then((r) => { st.data.portainer = r; const bad = r.containers.filter((c) => c.state !== 'running' || c.unhealthy); bad.forEach((c) => add('bad', `${c.name}: ${c.status}`)); if (!bad.length) add('ok', `All ${r.containers.length} Docker containers fine`); }).catch((e) => add('bad', 'Portainer: ' + e)));
  if (S.checks_on && S.checks.length) tasks.push(checkServices(true).then((l) => l.forEach((s) => add(s.up ? 'ok' : 'bad', s.name))).catch(() => {}));
  if (on('lianli')) tasks.push(invoke('integ', { name: 'lianli', action: 'feed' }).then((f) => { st.data.lianli = f; const a = (f.events || []).filter((e) => e.kind === 'homelab' && e.level !== 'info'); a.slice(0, 6).forEach((e) => add('bad', e.text)); if (!a.length) add('ok', `Screen container: ${(f.containers || []).filter((c) => c.state === 'running').length} of ${(f.containers || []).length} containers running`); }).catch((e) => add('bad', 'Lian Li screen: ' + e)));
  if (!tasks.length) return say(L('nothing_on'), [{ label: 'Open Settings', primary: true, fn: () => { hush(); invoke('show_window', { which: 'settings' }); } }]);
  await Promise.all(tasks);
  const bad = rows.filter((r) => r.includes('class="bad"')).length;
  say(bad ? L('homelab_bad') : L('homelab_ok'), null, { extra: `<ul>${rows.join('')}</ul>`, stay: 60000 });
  toScreen(bad ? `${bad} homelab problem${bad > 1 ? 's' : ''}` : 'Homelab: all good', 'From Clippy', bad ? 'warn' : 'good');
}
async function lights() {
  say(L('checking'), null, { stay: 0 });
  let r; try { r = await invoke('integ', { name: 'home_assistant', action: 'lights' }); } catch (e) { return fail(e); }
  const ls = r.lights.filter((l) => l.available);
  if (!ls.length) return say('Home Assistant has no lights I can switch.');
  const set = async (ents, val) => {
    try { await invoke('integ', { name: 'home_assistant', action: 'set', args: { entities: ents, on: val } });
      say(L('lights', { n: ents.length, s: ents.length > 1 ? 's' : '', state: val ? 'on' : 'off' })); toScreen(`Lights ${val ? 'on' : 'off'}`, `${ents.length} light${ents.length > 1 ? 's' : ''}`, 'info'); }
    catch (e) { fail(e); }
  };
  const anyOn = ls.some((l) => l.on);
  say(`${ls.filter((l) => l.on).length} of ${ls.length} lights are on.`, [
    { label: anyOn ? 'All off' : 'All on', primary: true, fn: () => set(ls.map((l) => l.entity), !anyOn) }],
    { stay: 60000, extra: '<ul>' + ls.map((l, i) => `<li><span>${esc(l.name)}</span><button data-l="${i}">${l.on ? 'Turn off' : 'Turn on'}</button></li>`).join('') + '</ul>' });
  bubble.querySelectorAll('[data-l]').forEach((b) => { b.onclick = () => { const l = ls[+b.dataset.l]; set([l.entity], !l.on); }; });
}
async function media() {
  say(L('checking'), null, { stay: 0 });
  const rows = [];
  const li = (t) => rows.push(`<li><span>${esc(t)}</span></li>`);
  const tasks = [];
  if (on('jellyfin')) tasks.push(invoke('integ', { name: 'jellyfin', action: 'status' }).then((r) => { st.data.jellyfin = r; r.playing.forEach((p) => li(`▶ ${p.title} · ${p.user}${p.paused ? ' (paused)' : ''}`)); r.latest.slice(0, 3).forEach((x) => li(`New: ${x.title}`)); }).catch((e) => li('Jellyfin: ' + e)));
  for (const n of ['radarr', 'sonarr']) if (on(n)) tasks.push(invoke('integ', { name: n, action: 'status' }).then((r) => { st.data[n] = r; r.queue.slice(0, 3).forEach((q) => li(`↓ ${q.title} ${Math.round(q.progress)}%`)); r.upcoming.slice(0, 3).forEach((u) => li(`${u.date.slice(5)} · ${u.title}`)); }).catch((e) => li(`${n}: ${e}`)));
  if (!tasks.length) return say(L('nothing_on'));
  await Promise.all(tasks);
  say(rows.length ? 'Here\'s what\'s happening:' : 'Nothing playing, nothing coming.', null, { extra: rows.length ? `<ul>${rows.join('')}</ul>` : '', stay: 60000 });
}
async function downloads() {
  say(L('checking'), null, { stay: 0 });
  try {
    const r = await invoke('integ', { name: 'qbittorrent', action: 'status' }); st.data.qbittorrent = r;
    const act = r.torrents.filter((t) => t.progress < 100).sort((a, b) => b.dlspeed - a.dlspeed);
    if (!act.length) return say('Nothing downloading right now.');
    say(`${act.length} downloading:`, null, { stay: 60000, extra: '<ul>' + act.slice(0, 8).map((t) => `<li><span>${esc(t.name)}</span><b>${Math.round(t.progress)}%</b></li>`).join('') + '</ul>' });
  } catch (e) { fail(e); }
}
async function pihole() {
  try {
    const r = await invoke('integ', { name: 'pihole', action: 'status' }); st.data.pihole = r;
    const blocking = r.blocking === 'enabled';
    say(`Pi-hole blocked <b>${Number(r.percent || 0).toFixed(1)}%</b> of ${Number(r.total || 0).toLocaleString()} queries today. Blocking is <b>${blocking ? 'on' : 'off'}</b>.`, [
      blocking ? { label: 'Pause 5 min', fn: async () => { try { await invoke('integ', { name: 'pihole', action: 'pause', args: { seconds: 300 } }); say(L('pihole_paused', { min: 5 })); } catch (e) { fail(e); } } }
        : { label: 'Turn back on', primary: true, fn: async () => { try { await invoke('integ', { name: 'pihole', action: 'resume' }); say(L('pihole_on')); } catch (e) { fail(e); } } }]);
  } catch (e) { fail(e); }
}
async function stocks() {
  say(L('checking'), null, { stay: 0 });
  try {
    const r = await invoke('integ', { name: 'stocks', action: 'status' }); st.data.stocks = r;
    say('Your stocks today:', null, { stay: 60000, extra: '<ul>' + r.quotes.map((q) => q.error ? `<li><span>${esc(q.symbol)}</span><span>${esc(q.error)}</span></li>` :
      `<li class="${q.change >= 0 ? 'ok' : 'bad'}"><span>${esc(q.symbol)} · ${esc(money(q))}</span><b>${q.change > 0 ? '+' : ''}${q.change.toFixed(2)}%</b></li>`).join('') + '</ul>' });
  } catch (e) { fail(e); }
}
async function weather() {
  try {
    const r = await invoke('integ', { name: 'weather', action: 'status' }); st.data.weather = r;
    const c = r.current, d = r.daily, u = r.units?.temperature_2m || '°';
    say(`<b>${Math.round(c.temperature_2m)}${esc(u)}</b> and ${WMO[c.weather_code] || 'mixed'} in ${esc(r.town)} (feels like ${Math.round(c.apparent_temperature)}${esc(u)}).`, null,
      { extra: `<div class="stats"><div><b>${Math.round(d.temperature_2m_max[0])}°</b><span>High</span></div><div><b>${Math.round(d.temperature_2m_min[0])}°</b><span>Low</span></div><div><b>${d.precipitation_probability_max[0]}%</b><span>Rain</span></div></div><span class="meta">Tomorrow ${Math.round(d.temperature_2m_min[1])}–${Math.round(d.temperature_2m_max[1])}°, ${WMO[d.weather_code[1]] || 'mixed'}. Weather by Open-Meteo.</span>` });
  } catch (e) { fail(e); }
}
async function health() {
  try {
    const h = await invoke('mac_health'); st.data.health = h;
    say(L('health'), null, { extra: `<div class="stats"><div><b>${Math.round(h.cpu)}%</b><span>CPU</span></div><div><b>${Math.round(h.mem)}%</b><span>Memory</span></div><div><b>${Math.round(h.disk)}%</b><span>Disk</span></div></div><span class="meta">${h.diskFreeGB.toFixed(0)} GB free · ${h.memGB} GB memory · up ${h.uptimeH} h${h.cpu < 50 && h.mem < 80 ? ' · ' + L('health_good') : ''}</span>` });
    toScreen(`Mac: CPU ${Math.round(h.cpu)}% · memory ${Math.round(h.mem)}%`, `Disk ${Math.round(h.disk)}% full · ${h.diskFreeGB.toFixed(0)} GB free`, h.disk >= 90 ? 'warn' : 'info');
  } catch (e) { fail(e); }
}
function fmt(s) { return String(Math.floor(s / 60)).padStart(2, '0') + ':' + String(s % 60).padStart(2, '0'); }
async function startFocus() {
  st.focusEnd = Date.now() + 25 * 60000; st.focusOnScreen = false;
  if (on('lianli')) { try { const r = await invoke('integ', { name: 'lianli', action: 'focus', args: { action: 'start' } }); st.focusOnScreen = !!r.ok; } catch {} }
  clearInterval(st.focusTick);
  st.focusTick = setInterval(() => { const left = Math.max(0, Math.round((st.focusEnd - Date.now()) / 1000)); invoke('set_tray_title', { title: fmt(left) }); if (!left) endFocus(true); }, 1000);
  say(L('focus_start'), [{ label: 'Stop the timer', fn: () => endFocus(false) }]);
  status();
}
async function endFocus(done) {
  clearInterval(st.focusTick); st.focusTick = null; st.focusEnd = 0;
  invoke('set_tray_title', { title: '' });
  if (!done && st.focusOnScreen) invoke('integ', { name: 'lianli', action: 'focus', args: { action: 'reset' } }).catch(() => {});
  if (done) { say(L('focus_done')); if (!st.focusOnScreen) toScreen('Break time!', '25 minutes done', 'good'); setTimeout(() => showHeld(L('also_focus')), 6000); }
  else { say(L('focus_stop')); setTimeout(() => showHeld(L('also_focus')), 2500); }
  status();
}
function sendToScreen() {
  say('What should the Lian Li screen say?', null, { stay: 0, extra: '<form id="msg"><input id="m" value="Dinner\'s ready!" maxlength="120" autocomplete="off"><button>Send</button></form>' });
  const m = $('#m'); win.setFocus(); setTimeout(() => { m.focus(); m.select(); }, 50);
  $('#msg').onsubmit = async (e) => {
    e.preventDefault();
    const t = m.value.trim(); if (!t) return;
    try { const r = await invoke('integ', { name: 'lianli', action: 'say', args: { head: t, sub: '', tone: 'info', seconds: S.screen_seconds || 8, source: 'Clippy' } });
      say(r.screen_off ? 'Sent, but the screen is switched off.' : L('sent', { msg: 'it\'s on the screen now.' })); } catch (e) { fail(e); }
  };
}
async function screenPick() {
  let f; try { f = await invoke('integ', { name: 'lianli', action: 'feed' }); st.data.lianli = f; } catch (e) { return fail(e); }
  const ds = f.dashboards || [];
  say(`The screen shows <b>${esc(ds.find((d) => d.key === f.active)?.name || f.active)}</b>. Pick another:`, null,
    { stay: 60000, extra: '<div class="grid">' + ds.map((d) => `<button data-k="${esc(d.key)}" class="${d.key === f.active ? 'on' : ''}">${esc(d.name)}</button>`).join('') + '</div>' });
  bubble.querySelectorAll('[data-k]').forEach((b) => { b.onclick = async () => { try { await invoke('integ', { name: 'lianli', action: 'mode', args: { mode: b.dataset.k } }); say(`The screen now shows <b>${esc(b.textContent)}</b>.`); } catch (e) { fail(e); } }; });
}

const TILE = {
  chat: () => invoke('show_window', { which: 'chat' }), homelab, lights, media, downloads, pihole, stocks, weather, health, send: sendToScreen, screen: screenPick,
  focus: () => st.focusEnd ? say(`The focus timer is running: about <b>${Math.round((st.focusEnd - Date.now()) / 60000)} min</b> left.`, [{ label: 'Stop the timer', fn: () => endFocus(false) }]) : startFocus(),
};
listen('tile', async (e) => {
  const { id, index } = e.payload;
  try {
    if (id === 'custom') { const c = await invoke('custom_list'); const a = c.actions[index]; if (a) askCustom(a); return; }
    if (TILE[id]) await TILE[id]();
  } catch (err) { fail(err); }
});
listen('no-model', () => say(L('no_model'), [{ label: 'Open Settings', primary: true, fn: () => { hush(); invoke('show_window', { which: 'settings' }); } }]));

// ---------- first hello ----------
let first = false; try { first = !localStorage.getItem('helloed2'); localStorage.setItem('helloed2', '1'); } catch {}
setTimeout(() => say(first ? L('hello_first') : L('hello_again'), first ? [{ label: 'Pick a brain (model)', primary: true, fn: () => { hush(); invoke('show_window', { which: 'settings' }); } }] : null, { stay: first ? 60000 : 6000 }), 900);
})();
