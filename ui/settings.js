// Clippy Settings: every change saves at once (save_settings) and reaches Clippy, the Deck and the chat.
// Secrets go straight to the Keychain (secret_set) and are never read back into this window.
(async () => {
const T = window.__TAURI__;
const invoke = T.core.invoke;
const { listen, emit } = T.event;
const $ = (s) => document.querySelector(s);
let S = await invoke('get_settings');
$('#logo').innerHTML = CLIP_ICON;

let savedT;
async function save() {
  await invoke('save_settings', { settings: S });
  const el = $('#saved'); el.classList.add('show'); clearTimeout(savedT); savedT = setTimeout(() => el.classList.remove('show'), 1100);
}
const tell = (m) => T.dialog.message(String(m), { title: 'Clippy' });
const gb = (b) => (b / 1e9).toFixed(1) + ' GB';

// ---------------- models ----------------
let info = null;
async function models() {
  info = await invoke('models_info');
  const sel = $('#model');
  sel.innerHTML = info.installed.length ? info.installed.map((m) => `<option value="${esc(m.path)}" ${m.path === S.model ? 'selected' : ''}>${esc(m.name)} · ${gb(m.bytes)}${m.added ? ' (added)' : ''}</option>`).join('')
    : '<option value="">None yet: download one below</option>';
  const cur = info.installed.find((m) => m.path === S.model);
  $('#model-help').innerHTML = cur ? `Runs on this Mac.${S.web_lookup ? ' Only Wikipedia searches leave it.' : ' Nothing you type leaves it.'}${cur.added ? ` <a id="forget">Forget this file</a>` : ''}` : 'Download one below. It runs fully on this Mac.';
  if (cur?.added) $('#forget').onclick = async () => { await invoke('model_remove', { path: cur.path }); S = await invoke('get_settings'); models(); };
  const ram = info.recommended.ram;
  $('#recs').innerHTML = info.recommended.list.map((r) => {
    const busy = info.downloading === r.id;
    const btn = r.have ? (info.installed.find((m) => m.file === r.file)?.path === S.model ? '<span class="state ok">In use</span>' : `<button class="btn" data-use="${esc(r.file)}">Use</button><button class="del" title="Move to the Bin" data-rm="${esc(r.file)}">×</button>`)
      : busy ? '<button class="btn" data-cancel>Cancel</button>' : `<button class="btn ${r.suggested ? 'primary' : ''}" data-dl="${r.id}" ${info.downloading ? 'disabled' : ''}>${r.partial ? 'Resume' : 'Download'}</button>`;
    return `<div class="row"><div class="txt"><span>${esc(r.name)}${r.suggested ? '<span class="tag">Best for this Mac</span>' : ''}${!r.fits ? `<span class="tag gray">Needs ${r.minRam} GB</span>` : ''}</span>
      <small>${(+r.gb).toFixed(1).replace(/\.0$/, '')} GB download · ${esc(r.note)}</small>${busy || r.partial ? `<div class="bar"><i id="bar-${r.id}" style="width:${r.partial ? Math.min(99, r.partial / (r.gb * 1e9) * 100) : 0}%"></i></div><small id="dl-${r.id}"></small>` : ''}</div>${btn}</div>`;
  }).join('');
  $('#recs').querySelectorAll('[data-dl]').forEach((b) => { b.onclick = () => download(b.dataset.dl); });
  $('#recs').querySelectorAll('[data-cancel]').forEach((b) => { b.onclick = () => invoke('model_cancel'); });
  $('#recs').querySelectorAll('[data-use]').forEach((b) => { b.onclick = async () => { S.model = info.installed.find((m) => m.file === b.dataset.use).path; await save(); models(); }; });
  $('#recs').querySelectorAll('[data-rm]').forEach((b) => { b.onclick = async () => {
    const m = info.installed.find((x) => x.file === b.dataset.rm);
    if (!(await T.dialog.ask(`Move ${m.name} (${gb(m.bytes)}) to the Bin?`, { title: 'Clippy', kind: 'warning' }))) return;
    try { await invoke('model_remove', { path: m.path }); } catch (e) { tell(e); }
    S = await invoke('get_settings'); models();
  }; });
  const s = info.status;
  $('#eng-state').textContent = s.loading ? `Loading ${s.loading}…` : s.loaded ? `In memory now: ${s.loaded}${s.busy ? ' (answering)' : ''}` : 'Nothing in memory right now.';
  $('#ctx').value = String(S.ctx);
  $('#unload').value = String(S.unload_minutes);
  $('#web-lookup').checked = S.web_lookup !== false;
  $('#chat-top').checked = S.chat_on_top !== false;
}
async function download(id) {
  const p = invoke('model_download', { id });
  setTimeout(models, 200);
  try { await p; } catch (e) { if (e !== 'cancelled') tell('Download stopped: ' + e); }
  S = await invoke('get_settings'); models();
}
listen('model-progress', (e) => {
  const p = e.payload; if (p.finished) return;
  const bar = document.getElementById('bar-' + p.id), t = document.getElementById('dl-' + p.id);
  if (bar) bar.style.width = (p.done / p.total * 100).toFixed(1) + '%';
  if (t) t.textContent = `${gb(p.done)} of ${gb(p.total)}`;
});
listen('engine', () => models());
$('#model').onchange = async (e) => { if (!e.target.value) return; S.model = e.target.value; await save(); models(); };
$('#add-model').onclick = async () => {
  const p = await T.dialog.open({ multiple: false, directory: false, filters: [{ name: 'GGUF model', extensions: ['gguf'] }] });
  if (!p) return;
  try { await invoke('model_add', { path: typeof p === 'string' ? p : p.path }); S = await invoke('get_settings'); models(); } catch (e) { tell(e); }
};
$('#ctx').onchange = (e) => { S.ctx = +e.target.value; save(); };
$('#unload').onchange = (e) => { S.unload_minutes = +e.target.value; save(); };
$('#web-lookup').onchange = (e) => { S.web_lookup = e.target.checked; save(); };
$('#chat-top').onchange = (e) => { S.chat_on_top = e.target.checked; save(); };
// this page lives inside the chat window: back (or Esc) returns to the conversation
invoke('take_want_settings');
const back = () => location.replace('chat.html');
$('#back').onclick = back;
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !e.target.closest?.('input,textarea,select')) back(); });
$('#unload-now').onclick = async () => { await invoke('model_unload'); models(); };

// ---------------- personality, Clippy, Deck ----------------
const DECK_HELP = { always: 'Docked on the right edge of the screen. Clicking Clippy opens the chat.', click: 'Hidden until you click Clippy; click him again to hide it. Chat is a tile on the Deck.', never: 'No Deck panel. Clicking Clippy opens the chat; its tiles are behind the grid button next to ⚙ in the chat.' };
function render() {
  PERSONALITY = S.personality;
  for (const p of ['normal', 'sarcastic']) $('#p-' + p).setAttribute('aria-pressed', String(S.personality === p));
  $('#persona-help').innerHTML = S.personality === 'sarcastic' ? `Sarcastic and cheeky, swears a little. For example: “${L('sent', { msg: 'your file is sent.' })}”` : 'Friendly and helpful.';
  for (const d of ['always', 'click', 'never']) $('#d-' + d).setAttribute('aria-pressed', String(S.deck_mode === d));
  $('#deck-help').textContent = DECK_HELP[S.deck_mode] || '';
  for (const id of ['buddy', 'quiet_two', 'checks_on']) $('#' + id).checked = !!S[id];
  for (const id of ['size', 'sleep_minutes']) if (document.activeElement !== $('#' + id)) $('#' + id).value = S[id];
  document.querySelectorAll('[data-kind]').forEach((el) => { el.checked = !!S.kinds[el.dataset.kind]; });
  renderSvcs();
}
for (const p of ['normal', 'sarcastic']) $('#p-' + p).onclick = () => { S.personality = p; save(); render(); };
for (const d of ['always', 'click', 'never']) $('#d-' + d).onclick = () => { S.deck_mode = d; save(); render(); };
for (const id of ['buddy', 'quiet_two', 'checks_on']) $('#' + id).onchange = (e) => { S[id] = e.target.checked; save(); };
document.querySelectorAll('[data-kind]').forEach((el) => { el.onchange = () => { S.kinds[el.dataset.kind] = el.checked; S.ignores[el.dataset.kind] = 0; save(); }; });
$('#sleep_minutes').onchange = (e) => { S.sleep_minutes = Math.max(1, Math.min(120, +e.target.value || 5)); save(); render(); };
$('#size').onchange = (e) => { S.size = e.target.value; save(); };

// ---------------- integrations ----------------
// f: settings field (url | user | extra), s: Keychain secret, ph: placeholder
const SPEC = [
  { id: 'home_assistant', name: 'Home Assistant', desc: 'Lights, temperature, humidity, AC and door sensors: ask Clippy in the chat, or use the Lights tile. Clippy can switch lights only, never switches or plugs.',
    fields: [{ f: 'url', label: 'Address', ph: 'http://homeassistant.local:8123' }, { s: true, label: 'Long-lived token', help: 'Your profile → Security → Long-lived access tokens' }], insecure: true },
  { id: 'proxmox', name: 'Proxmox VE', desc: 'Tells you when a VM or container stops, and can start it again.',
    fields: [{ f: 'url', label: 'Address', ph: 'https://192.168.1.10:8006' }, { f: 'user', label: 'API token ID', ph: 'root@pam!clippy' }, { s: true, label: 'Token secret', help: 'Datacenter → Permissions → API Tokens' }], insecure: true },
  { id: 'portainer', name: 'Portainer (Docker)', desc: 'Tells you when a container stops or is unhealthy, and can restart it.',
    fields: [{ f: 'url', label: 'Address', ph: 'https://192.168.1.10:9443' }, { s: true, label: 'Access token', help: 'My account → Access tokens' }, { f: 'user', label: 'Environment ID', ph: 'blank = the first one' }], insecure: true },
  { id: 'pihole', name: 'Pi-hole', desc: 'Today\'s blocking stats, and pause ad blocking for 5 minutes. Pi-hole v6.',
    fields: [{ f: 'url', label: 'Address', ph: 'http://pi.hole' }, { s: true, label: 'Password', help: 'Leave empty if Pi-hole has no password' }], insecure: true },
  { id: 'jellyfin', name: 'Jellyfin', desc: 'What\'s playing and what\'s new.',
    fields: [{ f: 'url', label: 'Address', ph: 'http://192.168.1.10:8096' }, { s: true, label: 'API key', help: 'Dashboard → API Keys' }] },
  { id: 'radarr', name: 'Radarr', desc: 'Movies downloading, coming up, and ready to watch.',
    fields: [{ f: 'url', label: 'Address', ph: 'http://192.168.1.10:7878' }, { s: true, label: 'API key', help: 'Settings → General' }] },
  { id: 'sonarr', name: 'Sonarr', desc: 'Episodes downloading, airing this week, and ready to watch.',
    fields: [{ f: 'url', label: 'Address', ph: 'http://192.168.1.10:8989' }, { s: true, label: 'API key', help: 'Settings → General' }] },
  { id: 'qbittorrent', name: 'qBittorrent', desc: 'What\'s downloading, and a ping when something finishes.',
    fields: [{ f: 'url', label: 'Address', ph: 'http://192.168.1.10:8080' }, { f: 'user', label: 'User name', ph: 'admin' }, { s: true, label: 'Password', help: 'Leave empty if local logins are bypassed' }] },
  { id: 'stocks', name: 'Stocks', desc: 'Your watchlist, and a ping on big moves. Quotes from Yahoo Finance.',
    fields: [{ f: 'extra', label: 'Symbols', ph: 'AAPL, MSFT, BTC-USD' }, { f: 'user', label: 'Ping when it moves over (%)', ph: '3' }] },
  { id: 'weather', name: 'Weather', desc: 'Today and tomorrow, from Open-Meteo. No account needed.',
    fields: [{ f: 'extra', label: 'Town', ph: 'London' }, { f: 'user', label: 'Units', options: [['celsius', 'Celsius'], ['fahrenheit', 'Fahrenheit']] }] },
  { id: 'lianli', name: 'Lian Li screen', desc: 'Show Clippy\'s messages on a Lian Li 8.8" screen driven by the lianli-dashboard container.',
    fields: [{ f: 'url', label: 'Container address', ph: 'http://192.168.1.10:8097' }], screen: true },
];
let secretState = {};
async function loadSecrets() { secretState = await invoke('secrets_status', { names: SPEC.map((x) => x.id) }); }
function renderIntegs() {
  const box = $('#integs');
  const openNow = new Set([...box.querySelectorAll('.integ.open')].map((e) => e.dataset.id));
  box.innerHTML = SPEC.map((x) => {
    const c = S.integrations[x.id] || {};
    const fields = x.fields.map((f) => {
      if (f.s) return `<label>${esc(f.label)}</label><div class="full" style="grid-column:auto"><input type="password" data-secret="${x.id}" placeholder="${secretState[x.id] ? '•••••••• saved in Keychain' : 'Paste it here'}" autocomplete="off">${secretState[x.id] ? `<button class="del" title="Remove" data-clear="${x.id}">×</button>` : ''}</div>${f.help ? `<span></span><small class="state" style="margin-top:-4px">${esc(f.help)}</small>` : ''}`;
      if (f.options) return `<label>${esc(f.label)}</label><select data-f="${f.f}" data-id="${x.id}">${f.options.map(([v, l]) => `<option value="${v}" ${(c[f.f] || f.options[0][0]) === v ? 'selected' : ''}>${l}</option>`).join('')}</select>`;
      return `<label>${esc(f.label)}</label><input type="${f.f === 'url' ? 'url' : 'text'}" class="${f.f === 'url' ? 'mono' : ''}" data-f="${f.f}" data-id="${x.id}" value="${esc(c[f.f] || '')}" placeholder="${esc(f.ph || '')}">`;
    }).join('');
    const extra = (x.insecure ? `<span></span><label class="chk"><input type="checkbox" data-insecure="${x.id}" ${c.insecure ? 'checked' : ''}> Self-signed HTTPS certificate (common on home servers)</label>` : '') +
      (x.screen ? `<label>Also show messages</label><label class="chk"><input type="checkbox" id="mirror" ${S.mirror_to_screen ? 'checked' : ''}> on the screen, for <input type="number" id="screen_seconds" min="3" max="60" value="${S.screen_seconds}"> seconds</label>` : '');
    return `<div class="integ ${c.on || openNow.has(x.id) ? 'open' : ''}" data-id="${x.id}">
      <div class="head"><div class="txt"><span>${esc(x.name)}</span><small>${esc(x.desc)}</small></div><input type="checkbox" class="sw" data-on="${x.id}" ${c.on ? 'checked' : ''} aria-label="${esc(x.name)}"></div>
      <div class="body">${fields}${extra}<div class="full"><button class="btn" data-test="${x.id}">${x.screen ? 'Send a test message' : 'Test'}</button><span class="state" id="st-${x.id}"></span></div></div></div>`;
  }).join('');
  box.querySelectorAll('[data-on]').forEach((el) => { el.onchange = () => { S.integrations[el.dataset.on].on = el.checked; save(); el.closest('.integ').classList.toggle('open', el.checked); if (el.checked) test(el.dataset.on); }; });
  box.querySelectorAll('[data-f]').forEach((el) => { el.onchange = () => {
    let v = el.value.trim();
    if (el.dataset.f === 'url') { v = v.replace(/\/+$/, ''); if (v && !/^https?:\/\/\S+$/.test(v)) { el.value = S.integrations[el.dataset.id].url; return; } }
    S.integrations[el.dataset.id][el.dataset.f] = v; save();
  }; });
  box.querySelectorAll('[data-insecure]').forEach((el) => { el.onchange = () => { S.integrations[el.dataset.insecure].insecure = el.checked; save(); }; });
  box.querySelectorAll('[data-secret]').forEach((el) => { el.onchange = async () => {
    if (!el.value.trim()) return;
    await invoke('secret_set', { name: el.dataset.secret, value: el.value }); el.value = '';
    await loadSecrets(); renderIntegs(); test(el.dataset.secret);
  }; });
  box.querySelectorAll('[data-clear]').forEach((el) => { el.onclick = async () => { await invoke('secret_set', { name: el.dataset.clear, value: '' }); await loadSecrets(); renderIntegs(); }; });
  box.querySelectorAll('[data-test]').forEach((el) => { el.onclick = () => test(el.dataset.test, true); });
  const m = $('#mirror'); if (m) m.onchange = (e) => { S.mirror_to_screen = e.target.checked; save(); };
  const ss = $('#screen_seconds'); if (ss) ss.onchange = (e) => { S.screen_seconds = Math.max(3, Math.min(60, +e.target.value || 8)); save(); };
}
const SUMMARY = {
  home_assistant: (r) => r.message, proxmox: (r) => `${r.nodes.length} node${r.nodes.length === 1 ? '' : 's'}, ${r.guests.filter((g) => g.running).length} of ${r.guests.length} guests running`,
  portainer: (r) => `${r.containers.length} containers`, pihole: (r) => `${Number(r.percent || 0).toFixed(1)}% of ${r.total} queries blocked today`,
  jellyfin: (r) => `${r.playing.length} playing, ${r.latest.length} recent items`, radarr: (r) => `${r.queue.length} in queue, ${r.upcoming.length} coming up`,
  sonarr: (r) => `${r.queue.length} in queue, ${r.upcoming.length} airing this week`, qbittorrent: (r) => `${r.torrents.length} torrents`,
  stocks: (r) => r.quotes.map((q) => q.error ? `${q.symbol}: ${q.error}` : `${q.symbol} ${q.price}`).join(' · '), weather: (r) => `${r.town}: ${Math.round(r.current.temperature_2m)}°`,
  lianli: () => 'Sent. Look at the screen.',
};
async function test(id, explicit) {
  const el = document.getElementById('st-' + id); if (!el) return;
  const c = S.integrations[id];
  if (!explicit && !c.url && !['stocks', 'weather'].includes(id)) { el.textContent = 'Add the address, then press Test.'; el.className = 'state'; return; }
  el.textContent = 'Checking…'; el.className = 'state';
  try {
    let r;
    if (id === 'lianli') { await invoke('integ', { name: 'lianli', action: 'test' }); r = c.on ? await invoke('integ', { name: 'lianli', action: 'say', args: { head: 'Hi from Clippy!', sub: 'This is a test', tone: 'good', seconds: S.screen_seconds } }) : {}; if (!c.on) { el.textContent = 'It answers. Turn it on to send messages.'; el.className = 'state ok'; return; } }
    else r = await invoke('integ', { name: id, action: 'test' });
    el.textContent = 'Working: ' + SUMMARY[id](r); el.className = 'state ok';
  } catch (e) { el.textContent = String(e); el.className = 'state bad'; }
}

// ---------------- homelab checks ----------------
function renderSvcs() {
  if ($('#svcs').contains(document.activeElement)) return;
  $('#svcs').innerHTML = S.checks.map((s, i) => `<div class="svc"><input type="text" value="${esc(s.name)}" data-i="${i}" data-k="name" aria-label="Name" placeholder="Name"><input type="url" value="${esc(s.url)}" data-i="${i}" data-k="url" aria-label="Address" placeholder="http://192.168.1.10:8080/"><button class="del" data-del="${i}" aria-label="Remove">×</button></div>`).join('');
  $('#svcs').querySelectorAll('input').forEach((el) => { el.onchange = () => { S.checks[+el.dataset.i][el.dataset.k] = el.value.trim(); save(); }; });
  $('#svcs').querySelectorAll('[data-del]').forEach((el) => { el.onclick = () => { S.checks.splice(+el.dataset.del, 1); save(); renderSvcs(); }; });
}
$('#svc-add').onclick = () => { S.checks.push({ name: 'New service', url: 'http://' }); S.checks_on = true; renderSvcs(); render(); save(); };

// ---------------- custom actions ----------------
async function custom() {
  const c = await invoke('custom_list');
  $('#cust-path').textContent = c.path.replace(/^\/Users\/[^/]+/, '~');
  $('#cust-example').hidden = c.exists;
  $('#cust-title').textContent = c.actions.length ? `${c.actions.length} custom action${c.actions.length > 1 ? 's' : ''}` : 'Your own Deck tiles';
  $('#cust-help').textContent = c.error ? c.error : c.actions.length ? 'They show on the Deck. Edit the file and press Reload.' : 'Call any service on your network: upload a file, send a link, trigger a webhook. Described in a JSON file.';
  $('#cust-help').className = c.error ? 'state bad' : '';
  $('#cust-list').innerHTML = c.actions.map((a) => `<div class="row"><div class="txt"><span>${esc(a.name)}${a.drop ? '<span class="tag">Drop files on Clippy</span>' : ''}</span><small>Asks for: ${esc({ none: 'nothing', text: 'some text', url: 'a link', file: 'a file' }[a.input] || a.input)}</small></div></div>`).join('') +
    c.secrets.map((s) => `<div class="row"><div class="txt"><span>Secret <span class="mono">${esc(s.name)}</span></span><small>${s.set ? 'Saved in the Keychain' : 'Not set yet: the actions that use it will fail'}</small></div><input type="password" data-cs="${esc(s.name)}" placeholder="${s.set ? 'Replace…' : 'Paste it here'}" style="width:180px"></div>`).join('');
  $('#cust-list').querySelectorAll('[data-cs]').forEach((el) => { el.onchange = async () => { if (!el.value.trim()) return; await invoke('secret_set', { name: 'custom:' + el.dataset.cs, value: el.value }); custom(); }; });
  emit('custom-changed');
}
$('#cust-reload').onclick = custom;
$('#cust-reveal').onclick = () => invoke('reveal_data');
$('#cust-example').onclick = async () => { try { await invoke('custom_example'); custom(); invoke('reveal_data'); } catch (e) { tell(e); } };
$('#cust-import').onclick = async () => {
  const p = await T.dialog.open({ multiple: false, directory: false, filters: [{ name: 'Custom actions', extensions: ['json'] }] });
  if (!p) return;
  try { await invoke('custom_import', { path: typeof p === 'string' ? p : p.path }); custom(); } catch (e) { tell(e); }
};

// ---------------- Windows → Mac tips permission ----------------
function keys(ok) {
  $('#keys-allow').hidden = ok !== false;
  if (ok === false) $('#keys-help').textContent = 'Needs Input Monitoring (macOS asks once). Clippy only looks at Ctrl/Alt shortcuts, never what you type.';
}
listen('keys', (e) => keys(e.payload.ok));
try { const k = localStorage.getItem('keysOK'); if (k !== null) keys(k === '1'); } catch {}
emit('keys-ask');
$('#keys-allow').onclick = async () => { await invoke('request_key_access'); invoke('open_url', { url: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent' }); };

listen('settings', (e) => { const old = S; S = e.payload; render(); if (old.model !== S.model) models(); });
T.window.getCurrentWindow().onFocusChanged(({ payload }) => { if (payload) { models(); custom(); } });
render();
await loadSecrets();
renderIntegs();
models();
custom();
})();
