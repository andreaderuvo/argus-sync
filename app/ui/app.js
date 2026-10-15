// argus-sync's window. Everything it shows comes from the app's "snapshot" (sent after every round
// of every folder); everything it does is one command to the app. No framework, no build step.
import { t, bytes } from './words.js';

if (!window.__TAURI__) await import('./mock.js');          // a browser, for development: a pretend app
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
// A handful of line icons, drawn here so they look the same with every system font.
const PATHS = {
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  down: 'M12 4v14M6 12l6 6 6-6',
  up: 'M12 20V6M6 12l6-6 6 6',
  x: 'M6 6l12 12M18 6L6 18',
  warn: 'M12 3l10 18H2zM12 10v5M12 18h.01',
  plus: 'M12 5v14M5 12h14',
  later: 'M12 7v5l3 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z',
  folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
};
function icon(name) {
  const ns = 'http://www.w3.org/2000/svg';
  const s = document.createElementNS(ns, 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('class', `ico ${name}`);
  s.setAttribute('aria-hidden', 'true');
  const p = document.createElementNS(ns, 'path');
  p.setAttribute('d', PATHS[name]);
  s.append(p);
  return s;
}
function el(tag, props = {}, kids = []) {
  const n = Object.assign(document.createElement(tag), props);
  for (const k of [].concat(kids)) if (k != null && k !== false) n.append(k);
  return n;
}

let snap = { pairs: [], paused_all: false, computer: '' };
const open = new Set();                     // the activity lists someone opened, kept across repaints

const STATES = {
  idle: ['Up to date', 'ok'],
  syncing: ['Syncing…', 'busy'],
  offline: ['Cannot reach the server', 'bad'],
  refused: ['Key taken back', 'bad'],
  paused: ['Paused', 'warn'],
  starting: ['Starting…', ''],
};
const KINDS = {
  down: ['down', 'Downloaded'], up: ['up', 'Uploaded'], 'deleted-here': ['x', 'Deleted here'], 'deleted-there': ['x', 'Deleted on the server'],
  'folder-here': ['plus', 'New folder here'], 'folder-there': ['plus', 'New folder on the server'], conflict: ['warn', 'Conflict'], later: ['later', 'Later'],
};

/** What the app said — `{"say": template, "values": {…}}` or plain words — in the window's language. */
function explain(x) {
  const s = String(x?.message ?? x);
  try {
    const m = JSON.parse(s);
    if (m && m.say) return t(m.say, m.values || {});
  } catch { /* plain words */ }
  return t(s);
}

// The engine's reasons for leaving a file for later, said in the window's language.
const LATER = [
  ['changed on this computer meanwhile', 'changed on this computer meanwhile'],
  ['could not be replaced', 'could not be replaced — open in another program?'],
  ['differs from the server', 'differs from the server’s copy, which is too big to fetch without asking'],
];
function laterText(why) {
  const hit = LATER.find(([start]) => String(why).startsWith(start));
  return hit ? t(hit[1]) : why;
}

function toast(text) {
  const n = $('#toast');
  n.textContent = text;
  n.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { n.hidden = true; }, 3200);
}

async function run(what, ...args) {
  try { return await invoke(what, ...args); } catch (e) { toast(explain(e)); throw e; }
}

// Opened by the app, and only inside a synced folder.
const reveal = (id, rel = null) => invoke('open_in', { id, rel }).catch((e) => toast(explain(e)));
const pickFolder = (defaultPath) => invoke('plugin:dialog|open', { options: { directory: true, defaultPath } });
const timeOf = (at) => (at || '').slice(11, 16);

// ------------------------------------------------------------------------------------- paint

function paint() {
  $('#computer').textContent = snap.computer ? t('on {computer}', { computer: snap.computer }) : '';
  $('#add').textContent = `+ ${t('Connect a folder')}`;
  $('#pauseall').textContent = snap.paused_all ? t('Resume all') : t('Pause all');
  $('#pauseall').hidden = !snap.pairs.length;
  const main = $('#main');
  const keep = main.scrollTop;
  main.replaceChildren(...(snap.pairs.length ? snap.pairs.map(card) : [emptyState()]));
  main.scrollTop = keep;
}

function emptyState() {
  const input = el('input', { type: 'text', placeholder: 'argus-sync://pair?…', spellcheck: false });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Continue'), onclick: () => connectSheet(input.value) });
  input.onkeydown = (e) => { if (e.key === 'Enter') go.click(); };
  return el('section', { className: 'empty' }, [
    el('h1', { textContent: t('No folder connected yet') }),
    el('p', { textContent: t('A folder of an Argus server, kept in sync with a folder on this computer — both ways.') }),
    el('ol', {}, [
      el('li', { textContent: t('In Argus, open the ⋮ menu of a folder → Sync to your computer…') }),
      el('li', { textContent: t('Click the link it gives: it opens here.') }),
      el('li', { textContent: t('Choose where the folder goes on this computer.') }),
    ]),
    el('p', { textContent: t('Or paste the link:') }),
    el('div', { className: 'pasterow' }, [input, go]),
  ]);
}

function card(p) {
  const s = p.status || {};
  const [label, tone] = STATES[s.state] || STATES.starting;
  const head = el('div', { className: 'cardhead' }, [
    el('span', { className: `dot ${s.state || ''}` }),
    el('span', { className: 'name', textContent: p.name }),
    el('span', { className: 'server', textContent: t('on {host}', { host: p.host }) }),
    el('span', { className: `chip ${tone}`, textContent: t(label) }),
    s.write === false ? el('span', { className: 'chip', textContent: t('read only') }) : null,
    el('span', { className: 'spacer' }),
    el('button', { className: 'ghost small iconbtn', type: 'button', title: t('More'), ariaLabel: t('More'), onclick: (e) => moreMenu(e, p) }, icon('more')),
  ]);
  const where = el('div', { className: 'where' }, [
    el('span', { className: 'path mono', title: p.local }, el('bdi', { textContent: p.local })),
    el('button', { className: 'small', type: 'button', onclick: () => reveal(p.id) }, [icon('folder'), t('Open')]),
  ]);
  const line = s.message
    ? el('div', { className: 'line bad', textContent: explain(s.message) })
    : el('div', { className: 'line', textContent: s.last_ok ? t('Last synced {when}', { when: timeOf(s.last_ok) }) : '' });
  const acts = el('div', { className: 'acts' }, [
    el('button', { className: 'small', type: 'button', textContent: p.paused ? t('Resume') : t('Pause'),
      onclick: () => run('set_paused', { id: p.id, paused: !p.paused }) }),
    el('button', { className: 'small', type: 'button', textContent: t('Sync now'), disabled: p.paused, onclick: () => run('sync_now') }),
  ]);
  return el('article', { className: 'card' }, [head, where, line, acts, conflicts(p), waiting(p), notes(p), activity(p)]);
}

function conflicts(p) {
  const list = p.status?.conflicts || [];
  if (!list.length) return null;
  return el('section', { className: 'sect alert' }, [
    el('h3', {}, [icon('warn'), t('Conflicts'), el('span', { className: 'n', textContent: String(list.length) })]),
    el('p', { className: 'hint', textContent: t('Changed on both sides. The server’s version is in place; yours is the copy:') }),
    ...list.map((copy) => el('div', { className: 'item' }, [
      el('a', { className: 'what mono link', href: '#', textContent: copy, title: t('Open'), onclick: (e) => { e.preventDefault(); reveal(p.id, copy); } }),
      el('button', { className: 'small', type: 'button', textContent: t('Keep mine'), onclick: () => run('resolve', { id: p.id, copy, choice: 'mine' }) }),
      el('button', { className: 'small', type: 'button', textContent: t('Keep the server’s'), onclick: () => run('resolve', { id: p.id, copy, choice: 'theirs' }) }),
      el('button', { className: 'small', type: 'button', textContent: t('Keep both'), onclick: () => run('resolve', { id: p.id, copy, choice: 'both' }) }),
    ])),
  ]);
}

function waiting(p) {
  const list = p.status?.big || [];
  if (!list.length) return null;
  return el('section', { className: 'sect' }, [
    el('h3', {}, [t('Waiting to be asked for'), el('span', { className: 'n', textContent: String(list.length) })]),
    el('p', { className: 'hint', textContent: t('Over {mb} MB: fetched only when you say.', { mb: p.big_mb }) }),
    ...list.map((w) => el('div', { className: 'item' }, [
      el('span', { className: 'what mono', textContent: w.path }),
      el('span', { className: 'size', textContent: bytes(w.size) }),
      el('button', { className: 'small', type: 'button', textContent: t('Fetch'), onclick: () => run('fetch', { id: p.id, path: w.path }) }),
    ])),
  ]);
}

function notes(p) {
  const list = p.status?.notes || [];
  if (!list.length) return null;
  return el('section', { className: 'sect' }, [
    el('h3', {}, [t('Notes'), el('span', { className: 'n', textContent: String(list.length) })]),
    ...list.map((n) => el('div', { className: 'item hint', textContent: noteText(n) })),
  ]);
}

function noteText(n) {
  if (typeof n === 'string') return n;
  if (n.kind === 'case') return t('{skipped} differs from {kept} only by upper/lower case: this computer keeps {kept}', n);
  if (n.kind === 'name') {
    const why = n.why.startsWith('a character') ? 'a character Windows does not allow'
      : n.why.startsWith('a name ending') ? 'a name ending with a dot or a space' : 'a name Windows reserves';
    return t('{path} cannot exist on this computer: {why}', { path: n.path, why: t(why) });
  }
  if (n.kind === 'type') return t('{path} is a file on one side and a folder on the other: left alone', n);
  return JSON.stringify(n);
}

function activity(p) {
  const list = p.status?.recent || [];
  const d = el('details', { className: 'sect', open: open.has(p.id) }, [
    el('summary', { textContent: `${t('Recent activity')} (${list.length})` }),
    ...(list.length ? list.slice(0, 30).map((a) => {
      const [glyph, word] = KINDS[a.kind] || ['later', a.kind];
      const detail = a.kind === 'down' || a.kind === 'up' ? bytes(Number(a.detail)) : a.kind === 'later' ? laterText(a.detail) : a.detail;
      return el('div', { className: 'act', title: `${t(word)} — ${a.path}${detail ? ` — ${detail}` : ''}` }, [
        el('span', { className: `k ${a.kind}` }, icon(glyph)),
        el('span', { className: 'p mono', textContent: a.path + (a.kind === 'conflict' ? ` → ${a.detail}` : '') }),
        el('span', { className: 't', textContent: timeOf(a.at) }),
      ]);
    }) : [el('p', { className: 'hint', textContent: t('Nothing has moved yet.') })]),
  ]);
  d.ontoggle = () => (d.open ? open.add(p.id) : open.delete(p.id));
  return d;
}

// ---------------------------------------------------------------------------------- menus, sheets

function closeMenu() { document.querySelector('.menu')?.remove(); }

function moreMenu(e, p) {
  closeMenu();
  const r = e.currentTarget.getBoundingClientRect();
  const m = el('div', { className: 'menu', role: 'menu' }, [
    el('button', { type: 'button', textContent: t('Move on this computer…'), onclick: () => { closeMenu(); moveIt(p); } }),
    el('button', { type: 'button', textContent: t('Large files…'), onclick: () => { closeMenu(); bigSheet(p); } }),
    el('button', { type: 'button', className: 'danger', textContent: t('Disconnect…'), onclick: () => { closeMenu(); disconnectSheet(p); } }),
  ]);
  document.body.append(m);
  m.style.top = `${Math.min(r.bottom + 4, innerHeight - m.offsetHeight - 8)}px`;
  m.style.left = `${Math.max(8, r.right - m.offsetWidth)}px`;
  setTimeout(() => addEventListener('pointerdown', function away(ev) {
    if (!m.contains(ev.target)) { closeMenu(); removeEventListener('pointerdown', away, true); }
  }, true));
}

function sheet(title, body, buttons) {
  const veil = $('#sheet');
  const box = el('div', { className: 'box', role: 'dialog', ariaModal: 'true' }, [el('h2', { textContent: title }), ...body, el('div', { className: 'foot' }, buttons)]);
  veil.replaceChildren(box);
  veil.hidden = false;
  veil.onpointerdown = (e) => { if (e.target === veil) shut(); };
  document.onkeydown = (e) => { if (e.key === 'Escape') shut(); };
  box.querySelector('input')?.focus();
  return box;
}
function shut() { $('#sheet').hidden = true; $('#sheet').replaceChildren(); document.onkeydown = null; }

async function connectSheet(link = '') {
  const input = el('input', { type: 'text', value: link, placeholder: 'argus-sync://pair?…', spellcheck: false });
  const found = el('div', { hidden: true });
  const dest = el('input', { type: 'text', spellcheck: false });
  const destRow = el('label', { hidden: true }, [t('On this computer'), el('div', { className: 'row' }, [dest,
    el('button', { type: 'button', textContent: t('Choose…'), onclick: async () => {
      const picked = await pickFolder(dest.value || undefined).catch(() => null);
      if (picked) dest.value = picked;
    } })])]);
  const note = el('p', { className: 'hint', hidden: true, textContent: t('If the folder already has files, they are merged with the server’s: nothing is overwritten, differing files are kept both.') });
  const err = el('p', { className: 'err', hidden: true });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Connect'), disabled: true });
  let looked = '';
  const look = async () => {
    const value = input.value.trim();
    if (!value || value === looked) return;
    looked = value;
    err.hidden = true;
    found.hidden = false;
    found.className = 'hint';
    found.textContent = t('Looking…');
    try {
      const v = await invoke('inspect', { link: value });
      found.className = 'found';
      found.replaceChildren(el('img', { src: 'logo.png', width: 24, height: 24, alt: '' }), el('div', {}, [
        el('b', { textContent: t('{folder} on {host}', { folder: v.folder, host: v.host }) }),
        el('div', { className: 'hint', textContent: v.write ? t('both ways') : t('read only — this computer receives, the server does not change') }),
      ]));
      if (!dest.value) dest.value = v.suggested;
      destRow.hidden = false;
      note.hidden = false;
      go.disabled = false;
    } catch (e) {
      found.hidden = true;
      err.textContent = explain(e);
      err.hidden = false;
      go.disabled = true;
      looked = '';
    }
  };
  input.oninput = () => { clearTimeout(input.timer); input.timer = setTimeout(look, 300); };
  go.onclick = async () => {
    go.disabled = true;
    try {
      await invoke('connect', { link: input.value.trim(), local: dest.value.trim() });
      shut();
      toast(t('Connected: {name}', { name: dest.value.split(/[\\/]/).pop() }));
    } catch (e) {
      err.textContent = explain(e);
      err.hidden = false;
      go.disabled = false;
    }
  };
  sheet(t('Connect a folder from Argus'), [el('label', {}, [t('Link from Argus'), input]), found, destRow, note, err],
    [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }), go]);
  if (link) look();
}

function disconnectSheet(p) {
  sheet(t('Disconnect {name}?', { name: p.name }), [el('p', { textContent: t('Syncing stops. The files stay where they are, on this computer and on the server.') })], [
    el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }),
    el('button', { className: 'danger', type: 'button', textContent: t('Disconnect'), onclick: async () => { await run('disconnect', { id: p.id }); shut(); } }),
  ]);
}

function bigSheet(p) {
  const mb = el('input', { type: 'number', min: 0, step: 50, value: p.big_mb });
  sheet(t('Large files'), [el('label', {}, [t('Files bigger than this wait for you to fetch them (0 = no limit).'),
    el('div', { className: 'row' }, [mb, el('span', { className: 'hint', textContent: 'MB' })])])], [
    el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }),
    el('button', { className: 'primary', type: 'button', textContent: t('Save'), onclick: async () => {
      await run('set_big', { id: p.id, mb: Math.max(0, Math.round(Number(mb.value) || 0)) });
      shut();
    } }),
  ]);
}

async function moveIt(p) {
  const to = await pickFolder().catch(() => null);
  if (!to) return;
  await run('move_local', { id: p.id, to });
  toast(t('Moved.'));
}

// --------------------------------------------------------------------------------------- start

$('#add').onclick = () => connectSheet();
$('#pauseall').onclick = () => run('pause_all', { paused: !snap.paused_all });
listen('snapshot', (e) => { snap = e.payload; paint(); });
listen('invite', (e) => connectSheet(e.payload));
snap = await invoke('get_snapshot');
paint();
const pending = await invoke('take_invite');
if (pending) connectSheet(pending);
