// Clippy's drawing (our own paperclip with eyes), shared by every window. Same shape as the Lian Li screen's.
let clipGid = 0;
function clippySVG(extra) {
  const g = 'w' + (++clipGid);
  const d = 'M40 122 V42 A12 12 0 0 1 64 42 V132 A20 20 0 0 1 24 132 V32 A28 28 0 0 1 80 32 V112';
  return `<svg class="clip ${extra || ''}" viewBox="0 0 104 166" aria-hidden="true"><defs><linearGradient id="${g}" x1="0" x2="1"><stop offset="0" stop-color="#8f9aaa"/><stop offset=".45" stop-color="#f6f8fc"/><stop offset="1" stop-color="#7c8696"/></linearGradient></defs>
  <path d="${d}" fill="none" stroke="#4e5765" stroke-width="10" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="${d}" fill="none" stroke="url(#${g})" stroke-width="7" stroke-linecap="round" stroke-linejoin="round"/>
  <g class="eyes"><ellipse cx="35" cy="60" rx="11.5" ry="13" fill="#fff" stroke="#23272e" stroke-width="1.8"/><ellipse cx="67" cy="60" rx="11.5" ry="13" fill="#fff" stroke="#23272e" stroke-width="1.8"/>
  <circle class="pupil" cx="37" cy="62" r="5" fill="#15181d"/><circle class="pupil" cx="69" cy="62" r="5" fill="#15181d"/></g>
  <path class="brow" d="M24 42 Q34 33 45 40" fill="none" stroke="#23272e" stroke-width="3" stroke-linecap="round"/><path class="brow" d="M57 40 Q68 33 78 42" fill="none" stroke="#23272e" stroke-width="3" stroke-linecap="round"/></svg>`;
}
const CLIP_ICON = '<svg viewBox="0 0 104 166" aria-hidden="true"><path d="M40 122 V42 A12 12 0 0 1 64 42 V132 A20 20 0 0 1 24 132 V32 A28 28 0 0 1 80 32 V112" fill="none" stroke="currentColor" stroke-width="11" stroke-linecap="round"/></svg>';
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
