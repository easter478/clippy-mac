// The Deck: tiles for what's turned on in Settings, plus your custom actions. A tile sends a "tile"
// event; Clippy (buddy.js) runs it and answers in his bubble.
(async () => {
const T = window.__TAURI__;
const invoke = T.core.invoke;
const { listen, emit } = T.event;
const $ = (s) => document.querySelector(s);
let S = await invoke('get_settings');
let custom = { actions: [] };

const I = TILE_ICONS;
const on = (n) => !!S.integrations?.[n]?.on;
function tiles() { return deckTiles(S, custom, false); }
function paint() {
  const list = tiles();
  $('#tiles').innerHTML = list.map((t, i) => `<button class="tile" data-i="${i}"><svg viewBox="0 0 24 24">${I[t.icon]}</svg><span>${esc(t.label)}</span></button>`).join('');
  document.querySelectorAll('.tile').forEach((b) => {
    b.onclick = async () => {
      const t = list[+b.dataset.i];
      if (!S.buddy) await invoke('show_window', { which: 'buddy' });
      b.classList.add('busy'); setTimeout(() => b.classList.remove('busy'), 600);
      emit('tile', { id: t.id, index: t.index });
      // the Deck's job is done: close it (a click on Clippy brings it back)
      if (S.deck_autoclose !== false) setTimeout(() => invoke(S.deck_mode === 'always' ? 'deck_collapse' : 'hide_window', S.deck_mode === 'always' ? { collapsed: true } : { which: 'deck' }), 150);
    };
  });
  document.body.classList.toggle('collapsed', !!S.deck_collapsed && S.deck_mode === 'always');
  $('#foot').innerHTML = on('lianli') && S.mirror_to_screen ? '<i></i>also shows on the Lian Li screen' : 'Everything runs on this Mac';
  // size the Deck to its tiles (the window is never taller than the screen; the tiles scroll then)
  if (!document.body.classList.contains('collapsed')) requestAnimationFrame(() => {
    const h = (el) => el.getBoundingClientRect().height;
    const tl = $('#tiles'), last = tl.lastElementChild;
    const content = last ? last.getBoundingClientRect().bottom - tl.getBoundingClientRect().top + tl.scrollTop : 0;
    const need = 22 + h($('.head')) + h($('#mode')) + h($('#foot')) + 30 + content + 2;
    invoke('deck_fit', { height: Math.ceil(need) });
  });
}
$('#logo').innerHTML = CLIP_ICON;
$('#tab-clip').innerHTML = CLIP_ICON;
$('#gear').onclick = () => invoke('show_window', { which: 'settings' });
$('#fold').onclick = () => invoke('deck_collapse', { collapsed: true });
$('#tab').onclick = () => invoke('deck_collapse', { collapsed: false });

async function loadCustom() { try { custom = await invoke('custom_list'); } catch { custom = { actions: [] }; } paint(); }
listen('settings', (e) => { S = e.payload; paint(); });
listen('custom-changed', loadCustom);
listen('status', (e) => {
  const s = e.payload, m = $('#mode');
  if (s.screen) { m.className = 'mode ' + (s.screenOk ? 'ok' : 'down'); $('#mode-text').innerHTML = `<b>Lian Li</b> · ${esc(s.screen)}`; }
  else { m.className = 'mode standalone'; $('#mode-text').innerHTML = s.count ? `<b>${s.count}</b> integration${s.count > 1 ? 's' : ''} on` : '<b>This Mac</b> · no integrations yet'; }
});
T.window.getCurrentWindow().onFocusChanged(({ payload }) => { if (payload) loadCustom(); });
await loadCustom();
emit('status-request');
setInterval(() => emit('status-request'), 15000);
})();
