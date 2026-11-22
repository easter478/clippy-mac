// Chat with Clippy: a local Qwen model answers on this Mac (Rust engine.rs). The conversation is kept in
// this window's local storage only, so it is still there next time you open the chat.
(async () => {
const T = window.__TAURI__;
const invoke = T.core.invoke;
const { listen, emit } = T.event;
const $ = (s) => document.querySelector(s);
let S = await invoke('get_settings');
let msgs = [];
try { msgs = JSON.parse(localStorage.getItem('chat') || '[]'); } catch {}
let busy = false, info = null;
$('#logo').innerHTML = CLIP_ICON;
const AV = `<div class="av">${CLIP_ICON}</div>`;
const SEND = '<svg viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6"/></svg>';
const STOP = '<svg viewBox="0 0 24 24"><rect x="7" y="7" width="10" height="10" rx="1.5" fill="currentColor"/></svg>';

function keep() { try { localStorage.setItem('chat', JSON.stringify(msgs.slice(-60))); } catch {} }

// --- a small, safe Markdown renderer (everything is escaped first) ---
function md(src) {
  const blocks = [];
  let s = esc(src).replace(/```[\w-]*\n?([\s\S]*?)(```|$)/g, (_, code) => { blocks.push(code); return `\u0000${blocks.length - 1}\u0000`; });
  const inline = (t) => t.replace(/`([^`]+)`/g, '<code>$1</code>').replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>').replace(/(^|[\s(])\*([^*\n]+)\*/g, '$1<i>$2</i>')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a data-href="$2">$1</a>');
  const out = [];
  let list = null;
  for (const line of s.split('\n')) {
    const ul = /^\s*[-*•]\s+(.*)/.exec(line), ol = /^\s*\d+[.)]\s+(.*)/.exec(line);
    const tag = ul ? 'ul' : ol ? 'ol' : null;
    if (tag) { if (list !== tag) { if (list) out.push(`</${list}>`); out.push(`<${tag}>`); list = tag; } out.push(`<li>${inline((ul || ol)[1])}</li>`); continue; }
    if (list) { out.push(`</${list}>`); list = null; }
    if (/^\u0000\d+\u0000$/.test(line.trim())) { out.push(line.trim()); continue; }
    if (line.trim()) out.push(`<p>${inline(line.replace(/^#{1,6}\s+/, ''))}</p>`);
  }
  if (list) out.push(`</${list}>`);
  return out.join('').replace(/\u0000(\d+)\u0000/g, (_, i) => `<pre><code>${blocks[+i].replace(/\n$/, '')}</code></pre>`);
}

function row(m, i) {
  if (m.role === 'user') return `<div class="msg me"><div class="body"><div class="bub">${esc(m.content)}</div></div></div>`;
  const meta = m.err ? '' : `<span>${esc(m.meta || '')}</span>${m.content ? `<button data-copy="${i}">Copy</button>` : ''}`;
  return `<div class="msg bot">${AV}<div class="body"><div class="bub${m.err ? ' err' : ''}${m.live ? ' cursor' : ''}">${m.err ? esc(m.content) : md(m.content)}</div><div class="meta">${meta}</div></div></div>`;
}
const SUGS = { normal: ['How do I take a screenshot?', 'Is my homelab OK?', 'What\'s the weather like?', 'Write a short haiku about paperclips'],
  sarcastic: ['Roast my Mac\'s health', 'Is my homelab on fire?', 'Why is my Mac slow?', 'Tell me a terrible joke'] };
function render() {
  const log = $('#log');
  if (!info) { log.innerHTML = ''; return; }
  if (!info.installed.length || !S.model) { log.innerHTML = noModel(); wireNoModel(); return; }
  if (!msgs.length) {
    log.innerHTML = `<div class="empty"><div class="big">${clippySVG()}</div><h2>${S.personality === 'sarcastic' ? 'Oh. It\'s you. What?' : 'Hi! What can I help with?'}</h2>
      <p>Answers come from <b>${esc(curName())}</b> running on this Mac.${S.web_lookup ? ' Questions like “who is…” are looked up on Wikipedia.' : ' Nothing you type leaves it.'}</p>
      <div class="sugs">${(SUGS[S.personality] || SUGS.normal).map((q) => `<button>${esc(q)}</button>`).join('')}</div></div>`;
    log.querySelectorAll('.sugs button').forEach((b) => { b.onclick = () => send(b.textContent); });
    return;
  }
  log.innerHTML = msgs.map(row).join('');
  log.querySelectorAll('[data-copy]').forEach((b) => { b.onclick = () => { invoke('copy_text', { text: msgs[+b.dataset.copy].content }); b.textContent = 'Copied'; }; });
  log.querySelectorAll('a[data-href]').forEach((a) => { a.onclick = () => invoke('open_url', { url: a.dataset.href }); });
  log.scrollTop = log.scrollHeight;
}
function curName() { return info?.installed.find((m) => m.path === S.model)?.name || 'a local model'; }

// --- no model yet: offer the suggested download right here ---
function noModel() {
  const rec = info.recommended.list.find((r) => r.suggested) || info.recommended.list[0];
  const dl = info.downloading;
  return `<div class="empty"><div class="big">${clippySVG()}</div><h2>Clippy needs a brain</h2>
    <p>Pick a Qwen model to chat with. It downloads once (from Hugging Face) and then runs completely on this Mac.</p>
    <p><b>${esc(rec.name)}</b> · ${rec.gb} GB<br>${esc(rec.note)}</p>
    <div class="bar" ${dl ? '' : 'hidden'}><i id="dlbar"></i></div><p id="dltext" style="font-size:11.5px">${dl ? 'Downloading…' : ''}</p>
    <div class="sugs"><button class="btn primary" id="dl" ${dl ? 'disabled' : ''}>Download ${esc(rec.name)}</button><button class="btn" id="more">Other models…</button></div></div>`;
}
function wireNoModel() {
  const rec = info.recommended.list.find((r) => r.suggested) || info.recommended.list[0];
  $('#dl').onclick = async () => {
    $('#dl').disabled = true; $('.bar').hidden = false; $('#dltext').textContent = 'Starting…';
    try { await invoke('model_download', { id: rec.id }); } catch (e) { if (e !== 'cancelled') $('#dltext').textContent = String(e); $('#dl').disabled = false; }
    await refresh();
  };
  $('#more').onclick = () => invoke('show_window', { which: 'settings' });
}
listen('model-progress', (e) => {
  const p = e.payload;
  const bar = document.getElementById('dlbar'), txt = document.getElementById('dltext');
  if (!bar || p.finished) return;
  bar.style.width = (p.done / p.total * 100).toFixed(1) + '%';
  txt.textContent = `${(p.done / 1e9).toFixed(2)} of ${(p.total / 1e9).toFixed(2)} GB`;
});

async function refresh() {
  info = await invoke('models_info');
  S = await invoke('get_settings');
  const sel = $('#model');
  sel.innerHTML = info.installed.length ? info.installed.map((m) => `<option value="${esc(m.path)}" ${m.path === S.model ? 'selected' : ''}>${esc(m.name)}</option>`).join('') + '<option value="__more">More models…</option>' : '<option>No model yet</option>';
  sel.disabled = !info.installed.length;
  if (info.installed.length && !info.installed.some((m) => m.path === S.model)) { S.model = info.installed[0].path; await invoke('save_settings', { settings: S }); }
  paintPersona();
  render();
}
$('#model').onchange = async (e) => {
  if (e.target.value === '__more') { e.target.value = S.model; return invoke('show_window', { which: 'settings' }); }
  S.model = e.target.value; await invoke('save_settings', { settings: S }); render();
};
function paintPersona() {
  $('#p-normal').setAttribute('aria-pressed', String(S.personality !== 'sarcastic'));
  $('#p-sarcastic').setAttribute('aria-pressed', String(S.personality === 'sarcastic'));
}
for (const p of ['normal', 'sarcastic']) $('#p-' + p).onclick = async () => { S.personality = p; await invoke('save_settings', { settings: S }); paintPersona(); if (!msgs.length) render(); };
$('#new').onclick = () => { if (busy) return; msgs = []; keep(); render(); $('#q').focus(); };
listen('settings', (e) => { const had = S.model; S = e.payload; paintPersona(); if (had !== S.model) refresh(); });
listen('engine', (e) => {
  const v = e.payload;
  $('#status').textContent = v.kind === 'loading' ? `Waking up ${v.model}…` : '';
});

// --- sending ---
const q = $('#q');
function grow() { q.style.height = 'auto'; q.style.height = Math.min(140, q.scrollHeight) + 'px'; }
q.addEventListener('input', grow);
q.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); $('#form').requestSubmit(); } });
$('#form').onsubmit = (e) => { e.preventDefault(); if (busy) { invoke('chat_stop'); return; } send(q.value); };
function setBusy(b) { busy = b; const s = $('#send'); s.classList.toggle('stop', b); s.innerHTML = b ? STOP : SEND; s.setAttribute('aria-label', b ? 'Stop' : 'Send'); }

async function send(text) {
  text = String(text || '').trim();
  if (!text || busy) return;
  q.value = ''; grow();
  msgs.push({ role: 'user', content: text });
  const reply = { role: 'assistant', content: '', live: true };
  msgs.push(reply);
  render();
  setBusy(true);
  const ch = new T.core.Channel();
  let paint = 0;
  ch.onmessage = (ev) => {
    if (ev.kind === 'piece') {
      reply.content += ev.text;
      $('#status').textContent = '';
      if (!paint) paint = requestAnimationFrame(() => { paint = 0; const bubs = document.querySelectorAll('.msg.bot .bub'); const b = bubs[bubs.length - 1]; if (b) { b.innerHTML = md(reply.content.replace(/^<think>[\s\S]*?(<\/think>|$)/, '').replace(/^\s*`?(ACTION:|Lights:|Sensors:|Climate |Weather \(|Doors, windows and locks:|Switches and fans|Home Assistant \(live).*$/gm, '').trimEnd()); $('#log').scrollTop = $('#log').scrollHeight; } });
    } else if (ev.kind === 'status') {
      $('#status').textContent = ev.text;
    } else if (ev.kind === 'source') {
      reply.source = { title: ev.title, url: ev.url };
    } else if (ev.kind === 'done') {
      reply.content = ev.text || reply.content; reply.live = false;
      reply.meta = `${curName()}${ev.tps ? ` · ${ev.tps.toFixed(0)} tok/s` : ''}${ev.stopped ? ' · stopped' : ''}${ev.trimmed ? ' · forgot the oldest messages' : ''}`;
      if (reply.source) reply.content += `\n\nSource: [Wikipedia: ${reply.source.title}](${reply.source.url})`;
      if (!reply.content) { reply.content = ev.stopped ? '(stopped)' : '(no answer)'; }
      finish();
    } else if (ev.kind === 'error') { reply.content = ev.message; reply.err = true; reply.live = false; finish(); }
  };
  function finish() { setBusy(false); $('#status').textContent = ''; keep(); render(); q.focus(); }
  try {
    const history = msgs.filter((m) => m !== reply && !m.err).map((m) => ({ role: m.role, content: m.content })).slice(-30);
    await invoke('chat_send', { messages: history, onEvent: ch });
  } catch (e) {
    if (e === 'no-model') { msgs.splice(-2, 2); setBusy(false); emit('no-model'); return refresh(); }
    reply.content = String(e); reply.err = true; reply.live = false; finish();
  }
}

T.window.getCurrentWindow().onFocusChanged(({ payload }) => { if (payload) { refresh(); setTimeout(() => q.focus(), 30); } });
await refresh();
q.focus();
const t = await invoke('test_ask');
if (t) setTimeout(() => send(t), 600);
})();
