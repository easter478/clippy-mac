// The Deck's tiles, shared by the Deck panel and the Deck sheet inside the chat, so both always match.
// A tile sends a "tile" event; Clippy (buddy.js) runs it and answers in his bubble.
const TILE_ICONS = {
  chat: '<path d="M4 5h16v11H9l-5 4z"/>',
  server: '<rect x="4" y="4" width="16" height="6" rx="1.5"/><rect x="4" y="14" width="16" height="6" rx="1.5"/><path d="M8 7h.01M8 17h.01"/>',
  bulb: '<path d="M9 18h6M10 21h4M12 3a6 6 0 0 0-4 10.5c.8.8 1 1.5 1 2.5h6c0-1 .2-1.7 1-2.5A6 6 0 0 0 12 3z"/>',
  film: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M7 5v14M17 5v14M3 10h4M3 14h4M17 10h4M17 14h4"/>',
  download: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
  shield: '<path d="M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6z"/>',
  chart: '<path d="M4 19h16M6 15l4-5 3 3 5-7"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5L19 19M5 19l1.5-1.5M17.5 6.5L19 5"/>',
  timer: '<circle cx="12" cy="13" r="8"/><path d="M12 9v4l3 2M9 2h6"/>',
  chip: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>',
  screen: '<rect x="2" y="7" width="20" height="8" rx="2"/><path d="M6 11h8"/>',
  layout: '<rect x="2" y="7" width="20" height="8" rx="2"/><path d="M8 7v8M16 7v8"/>',
  upload: '<path d="M12 20V9M7 14l5-5 5 5M5 4h14"/>',
  play: '<rect x="3" y="5" width="18" height="14" rx="4"/><path d="M10 9.5v5l4.5-2.5z"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
  bolt: '<path d="M13 2L4 14h7l-1 8 9-12h-7z"/>',
  send: '<path d="M4 12l16-8-6 16-3-7z"/>',
  home: '<path d="M4 11l8-7 8 7v9h-5v-6H9v6H4z"/>',
  folder: '<path d="M3 7h6l2 2h10v10H3z"/>',
};

/** Tiles for what's turned on in Settings, plus custom actions. `inChat` leaves out the Chat tile. */
function deckTiles(S, custom, inChat) {
  const on = (n) => !!S.integrations?.[n]?.on;
  const t = inChat ? [] : [{ id: 'chat', icon: 'chat', label: 'Chat' }];
  if (on('proxmox') || on('portainer') || on('lianli') || (S.checks_on && S.checks.length)) t.push({ id: 'homelab', icon: 'server', label: 'Homelab' });
  if (on('home_assistant')) t.push({ id: 'lights', icon: 'bulb', label: 'Lights' });
  if (on('jellyfin') || on('radarr') || on('sonarr')) t.push({ id: 'media', icon: 'film', label: 'Media' });
  if (on('qbittorrent')) t.push({ id: 'downloads', icon: 'download', label: 'Downloads' });
  if (on('pihole')) t.push({ id: 'pihole', icon: 'shield', label: 'Pi-hole' });
  if (on('stocks')) t.push({ id: 'stocks', icon: 'chart', label: 'Stocks' });
  if (on('weather')) t.push({ id: 'weather', icon: 'sun', label: 'Weather' });
  for (const a of (custom.actions || [])) t.push({ id: 'custom', index: a.index, icon: TILE_ICONS[a.icon] ? a.icon : 'bolt', label: a.name });
  t.push({ id: 'focus', icon: 'timer', label: 'Focus 25 min' }, { id: 'health', icon: 'chip', label: 'Mac health' });
  if (on('lianli')) t.push({ id: 'send', icon: 'screen', label: 'Send to screen' }, { id: 'screen', icon: 'layout', label: 'Screen design' });
  return t;
}
