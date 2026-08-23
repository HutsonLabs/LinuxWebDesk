'use strict';

/* ------------------------------------------------------------------ api ---*/

async function api(path, opts = {}) {
  const res = await fetch(path, { credentials: 'same-origin', ...opts });
  const ct = res.headers.get('content-type') || '';
  const body = ct.includes('application/json') ? await res.json() : await res.text();
  if (!res.ok) throw new Error((body && body.error) || res.statusText);
  return body;
}

const jsonPost = (path, obj) =>
  api(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(obj),
  });

/* -------------------------------------------------------------- dialogs ---*/

/* prompt(), confirm() and alert() are never called. They are chrome-coloured,
   they freeze the whole tab, and on a page that is trying to look like a
   desktop they are the one thing that gives it away. Everything this UI asks
   is asked in the page: a modal for a question, a toast for a complaint. */

const reduceMotion = () =>
  !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);

/* Resolves to the typed string, to true for a plain confirmation, or to null
   if it was dismissed. Text goes in with textContent throughout -- filenames
   reach these dialogs and are not markup. */
function openModal({
  title, message = '', field = null,
  confirmLabel = 'OK', cancelLabel = 'Cancel', danger = false,
}) {
  return new Promise((resolve) => {
    const back = document.createElement('div');
    back.className = 'modal';

    const card = document.createElement('form');
    card.className = 'modal-card';
    card.setAttribute('role', 'dialog');
    card.setAttribute('aria-modal', 'true');

    const heading = document.createElement('h2');
    heading.textContent = title;
    card.appendChild(heading);

    for (const para of String(message).split('\n\n')) {
      if (!para) continue;
      const p = document.createElement('p');
      p.className = 'modal-text';
      p.textContent = para;
      card.appendChild(p);
    }

    let input = null;
    if (field) {
      const label = document.createElement('label');
      const caption = document.createElement('span');
      caption.textContent = field.label;
      input = document.createElement('input');
      input.type = 'text';
      input.value = field.value || '';
      input.spellcheck = false;
      input.autocapitalize = 'off';
      input.setAttribute('autocomplete', 'off');
      input.setAttribute('autocorrect', 'off');
      label.append(caption, input);
      card.appendChild(label);
    }

    const actions = document.createElement('div');
    actions.className = 'modal-actions';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'modal-btn';
    cancel.textContent = cancelLabel;
    const go = document.createElement('button');
    go.type = 'submit';
    go.className = 'modal-btn ' + (danger ? 'modal-btn--danger' : 'modal-btn--go');
    go.textContent = confirmLabel;
    actions.append(cancel, go);
    card.appendChild(actions);

    back.appendChild(card);
    document.body.appendChild(back);

    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      document.removeEventListener('keydown', onKey, true);
      back.remove();
      resolve(value);
    };
    // Captured, because the terminal window is a keen listener and the dialog
    // is on top of it.
    function onKey(e) {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      finish(null);
    }
    document.addEventListener('keydown', onKey, true);

    cancel.addEventListener('click', () => finish(null));
    back.addEventListener('pointerdown', (e) => { if (e.target === back) finish(null); });
    card.addEventListener('submit', (e) => {
      e.preventDefault();
      if (!input) return finish(true);
      const value = input.value.trim();
      // An empty name is not an answer; leave the dialog up rather than
      // treating it as a cancel.
      if (!value) return input.focus();
      finish(value);
    });

    (input || go).focus();
    if (input) input.select();
  });
}

const askText = (title, label, value, confirmLabel) =>
  openModal({ title, field: { label, value: value || '' }, confirmLabel: confirmLabel || 'OK' });

const askConfirm = (title, message, confirmLabel, danger = true) =>
  openModal({ title, message, confirmLabel, danger }).then((v) => v === true);

/* What alert() used to say. It does not stop anyone typing, and it leaves on
   its own. */
function toast(message, kind = '') {
  let host = document.querySelector('.toasts');
  if (!host) {
    host = document.createElement('div');
    host.className = 'toasts';
    host.setAttribute('role', 'status');
    host.setAttribute('aria-live', 'polite');
    document.body.appendChild(host);
  }

  const el = document.createElement('div');
  el.className = 'toast' + (kind ? ' ' + kind : '');
  el.textContent = message;
  host.appendChild(el);

  const play = (frames, duration) =>
    reduceMotion() || typeof el.animate !== 'function'
      ? null
      : el.animate(frames, { duration, easing: 'ease', fill: 'both' });

  play([{ opacity: 0, transform: 'translateY(8px)' }, { opacity: 1, transform: 'none' }], 160);

  let leaving = false;
  const dismiss = () => {
    if (leaving) return;
    leaving = true;
    clearTimeout(timer);
    const out = play([{ opacity: 1 }, { opacity: 0, transform: 'translateY(6px)' }], 140);
    const drop = () => el.remove();
    if (out) out.finished.then(drop, drop);
    else drop();
  };
  const timer = setTimeout(dismiss, 4200);
  el.addEventListener('click', dismiss);
}

/* -------------------------------------------------------------- windows ---*/

let zTop = 10;
let focused = null;
const openWindows = new Map();
let winSeq = 0;

/* The band at the bottom the dock sits in. The windows layer runs the whole
   height of the screen so that a window can slide under the frosted dock and
   be seen through it; what keeps that from being a nuisance is here, not in
   the layout: windows open clear of the band and no title bar can be dragged
   into it. */
function dockBand() {
  const v = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--dock-h'));
  return Number.isFinite(v) ? v : 74;
}

/* Snap to grid. Drags and resizes land on a multiple of --grid, which is all
   it takes for windows to line up with one another without any of the fuss of
   a tiling manager. Hold Alt to place a window freely. The clamps that keep a
   title bar on screen are applied after the snap, so a window pushed against
   an edge sits flush there rather than on the nearest grid line. */
function grid() {
  const v = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--grid'));
  return Number.isFinite(v) && v >= 1 ? v : 16;
}

const snap = (v, step) => Math.round(v / step) * step;

function createWindow({ title, width = 720, height = 460, app = '', icon = '', build }) {
  const id = ++winSeq;
  const layer = document.getElementById('windows');
  const free = layer.clientHeight - dockBand();

  const win = document.createElement('div');
  win.className = 'win';
  const g = grid();
  const offset = (openWindows.size % 6) * 2 * g;
  win.style.width = Math.max(320, Math.min(snap(width, g), layer.clientWidth - 40)) + 'px';
  win.style.height = Math.max(200, Math.min(snap(height, g), free - 40)) + 'px';
  win.style.left = Math.max(g, snap((layer.clientWidth - width) / 2 + offset, g)) + 'px';
  win.style.top = Math.max(g, snap((free - height) / 2 - 20 + offset, g)) + 'px';

  const bar = document.createElement('div');
  bar.className = 'win-bar';
  const titleEl = document.createElement('div');
  titleEl.className = 'win-title';
  titleEl.textContent = title;
  const minBtn = document.createElement('button');
  minBtn.className = 'win-btn tip';
  minBtn.type = 'button';
  minBtn.dataset.tip = 'Minimize';
  minBtn.setAttribute('aria-label', 'Minimize');
  minBtn.textContent = '–';
  const closeBtn = document.createElement('button');
  closeBtn.className = 'win-btn close tip';
  closeBtn.type = 'button';
  closeBtn.dataset.tip = 'Close';
  closeBtn.setAttribute('aria-label', 'Close');
  closeBtn.textContent = '×';
  bar.append(titleEl, minBtn, closeBtn);

  const body = document.createElement('div');
  body.className = 'win-body';

  const grip = document.createElement('div');
  grip.className = 'win-resize';

  win.append(bar, body, grip);
  layer.appendChild(win);

  const entry = { id, win, body, titleEl, app, icon, gen: 0, onClose: null, onResize: null };
  openWindows.set(id, entry);

  const focus = () => {
    if (focused && focused !== win) focused.classList.remove('focused');
    win.style.zIndex = ++zTop;
    win.classList.add('focused');
    focused = win;
    paintDock();
  };
  win.addEventListener('pointerdown', focus, true);
  focus();

  // --- drag. Pointer capture plus a pointer-events guard on the body keeps
  // the gesture alive when the cursor passes over the terminal canvas.
  bar.addEventListener('pointerdown', (e) => {
    if (e.target.closest('.win-btn')) return;
    const sx = e.clientX, sy = e.clientY;
    const ox = win.offsetLeft, oy = win.offsetTop;
    const g = grid();
    bar.setPointerCapture(e.pointerId);
    win.classList.add('dragging');
    layer.classList.add('snapping');
    const move = (ev) => {
      let nx = ox + ev.clientX - sx;
      let ny = oy + ev.clientY - sy;
      if (!ev.altKey) { nx = snap(nx, g); ny = snap(ny, g); }
      layer.classList.toggle('snapping', !ev.altKey);
      nx = Math.min(Math.max(nx, -win.offsetWidth + 90), layer.clientWidth - 90);
      ny = Math.min(Math.max(ny, 0), layer.clientHeight - dockBand() - 34);
      win.style.left = nx + 'px';
      win.style.top = ny + 'px';
    };
    const up = () => {
      win.classList.remove('dragging');
      layer.classList.remove('snapping');
      bar.removeEventListener('pointermove', move);
      bar.removeEventListener('pointerup', up);
      bar.removeEventListener('pointercancel', up);
    };
    bar.addEventListener('pointermove', move);
    bar.addEventListener('pointerup', up);
    bar.addEventListener('pointercancel', up);
  });

  // --- resize
  grip.addEventListener('pointerdown', (e) => {
    e.stopPropagation();
    const sx = e.clientX, sy = e.clientY;
    const ow = win.offsetWidth, oh = win.offsetHeight;
    const g = grid();
    grip.setPointerCapture(e.pointerId);
    win.classList.add('dragging');
    layer.classList.add('snapping');
    const move = (ev) => {
      let nw = ow + ev.clientX - sx;
      let nh = oh + ev.clientY - sy;
      if (!ev.altKey) {
        // Snap the far edge, not the size, so a window whose left/top is on a
        // grid line stays boxed in by grid lines on all four sides.
        nw = snap(win.offsetLeft + nw, g) - win.offsetLeft;
        nh = snap(win.offsetTop + nh, g) - win.offsetTop;
      }
      layer.classList.toggle('snapping', !ev.altKey);
      win.style.width = Math.max(320, nw) + 'px';
      win.style.height = Math.max(200, nh) + 'px';
      if (entry.onResize) entry.onResize();
    };
    const up = () => {
      win.classList.remove('dragging');
      layer.classList.remove('snapping');
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', up);
      if (entry.onResize) entry.onResize();
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
  });

  minBtn.addEventListener('click', () => minimizeWindow(entry));
  closeBtn.addEventListener('click', () => closeWindow(id));

  build(entry);
  // The dock is painted first so that an editor's own dock item exists to be
  // flown out of.
  paintDock();
  genie(win, anchorRect(entry), 'in');
  return entry;
}

function closeWindow(id) {
  const e = openWindows.get(id);
  if (!e) return;
  if (e.onClose) { try { e.onClose(); } catch (_) {} }

  // Measured before the repaint, which is what takes an editor's dock item
  // away, and dropped from the book-keeping before the flight, so nothing can
  // act on a window that is already on its way out.
  const rect = anchorRect(e);
  openWindows.delete(id);
  if (e.win === focused) focused = null;
  paintDock();

  if (e.win.hidden) e.win.remove();
  else genie(e.win, rect, 'out').then(() => e.win.remove());
}

function minimizeWindow(e) {
  if (e.win.hidden) return;
  const gen = ++e.gen;
  genie(e.win, anchorRect(e), 'out').then(() => {
    // Raised again mid-flight -- leave it on screen.
    if (e.gen !== gen) return;
    e.win.hidden = true;
    paintDock();
  });
}

function raiseWindow(e) {
  e.gen++;
  const wasHidden = e.win.hidden;
  e.win.hidden = false;
  e.win.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
  if (wasHidden) genie(e.win, anchorRect(e), 'in');
  if (e.onResize) e.onResize();
}

/* ------------------------------------------------------------------ dock ---*/

const appWindows = (app) => [...openWindows.values()].filter((e) => e.app === app);

/* The dock item a window belongs to: its app's icon, its own item if it is an
   editor, or the account button for the System window. */
function anchorEl(e) {
  return (
    document.querySelector(`.dock-btn[data-win="${e.id}"]`) ||
    document.querySelector(`.dock-btn[data-app="${e.app}"]`) ||
    (e.app === 'system' ? document.getElementById('whoami') : null) ||
    document.querySelector('.dock')
  );
}

function anchorRect(e) {
  const el = anchorEl(e);
  return el ? el.getBoundingClientRect() : null;
}

/* Windows grow out of their dock icon and shrink back into it. Transforming
   the whole window takes its contents with it, which is the point -- and it is
   layout-free, so the terminal never re-fits to an in-between size. */
function genie(win, rect, dir) {
  const opening = dir === 'in';
  if (!rect || reduceMotion() || typeof win.animate !== 'function') return Promise.resolve();

  const w = win.getBoundingClientRect();
  if (!w.width || !w.height) return Promise.resolve();

  const sx = Math.max(rect.width / w.width, 0.05);
  const sy = Math.max(rect.height / w.height, 0.05);
  const dx = rect.left + rect.width / 2 - (w.left + w.width / 2);
  const dy = rect.top + rect.height / 2 - (w.top + w.height / 2);

  const atIcon = { transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`, opacity: 0 };
  const inPlace = { transform: 'translate(0px, 0px) scale(1, 1)', opacity: 1 };

  win.classList.add('win--flying');
  const anim = win.animate(opening ? [atIcon, inPlace] : [inPlace, atIcon], {
    duration: opening ? 240 : 190,
    easing: opening ? 'cubic-bezier(.16,.9,.3,1)' : 'cubic-bezier(.5,0,.85,.4)',
  });
  const land = () => win.classList.remove('win--flying');
  return anim.finished.then(land, land);
}

/* The dock is the whole window list. An app lights a dot while it has a window
   open -- minimised or not -- and every editor gets an item of its own, drawn
   with the file's own icon. */
let dockItems = '';

function paintDock() {
  const tasks = document.getElementById('tasks');
  if (!tasks) return;

  const editors = [...openWindows.values()].filter((e) => e.app === 'editor');
  // Focus runs on every pointerdown anywhere in a window, and so does this.
  // Rebuilding the items each time would throw away the button under the
  // pointer on every click, so only a real change to the list redraws it.
  const sig = editors.map((e) => e.id + ':' + e.icon + ':' + e.titleEl.textContent).join('|');
  if (sig !== dockItems) {
    dockItems = sig;
    tasks.textContent = '';
    for (const e of editors) {
      const name = e.titleEl.textContent;
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'dock-btn tip tip--up on';
      b.dataset.win = String(e.id);
      b.dataset.tip = name;
      b.setAttribute('aria-label', name);
      b.innerHTML =
        `<svg class="ic-d" aria-hidden="true"><use href="#${e.icon || 'i-file'}"></use></svg>` +
        '<span class="dock-dot" aria-hidden="true"></span>';
      b.addEventListener('click', () => raiseWindow(e));
      tasks.appendChild(b);
    }
  }

  for (const btn of document.querySelectorAll('.dock-btn[data-app]')) {
    btn.classList.toggle('on', appWindows(btn.dataset.app).length > 0);
  }
  const who = document.getElementById('whoami');
  if (who) who.classList.toggle('on', appWindows('system').length > 0);
}

/* A dock click raises what the app already has -- the minimised window first,
   otherwise the one after whichever is on top, so a second click on Terminal
   walks through the terminals. Alt- or middle-click always opens another. */
function activateApp(app, open, wantNew) {
  const wins = appWindows(app);
  if (wantNew || !wins.length) return open();

  const hidden = wins.filter((e) => e.win.hidden);
  if (hidden.length) return raiseWindow(hidden[hidden.length - 1]);

  const at = wins.findIndex((e) => e.win === focused);
  raiseWindow(wins[(at + 1) % wins.length]);
}

/* ----------------------------------------------------------------- icons ---*/

/* Two sprites: ui/icons.svg is the vendored Catppuccin file-type set, and
   ui/ui-icons.svg holds the Files toolbar's own actions.

   Both are injected into the document rather than referenced as images on
   purpose. The file-type icons colour themselves from --ctp-* custom
   properties and the toolbar icons from currentColor; an <img> is a separate
   document that can see neither. */

let iconsReady = null;

function loadSprite(url) {
  return fetch(url, { credentials: 'same-origin' })
    .then((r) => (r.ok ? r.text() : ''))
    .then((svg) => {
      if (!svg) return;
      const host = document.createElement('div');
      host.className = 'sprite';
      host.innerHTML = svg;
      document.body.prepend(host);
    })
    .catch(() => {});
}

function loadIcons() {
  if (iconsReady) return iconsReady;
  iconsReady = Promise.all([loadSprite('/icons.svg'), loadSprite('/ui-icons.svg')]);
  return iconsReady;
}

/* Whole filenames win over extensions, for the files that do not have a useful
   suffix to go on. Keys are compared lowercased. */
const ICON_BY_NAME = {
  dockerfile: 'docker', 'docker-compose.yml': 'docker', 'docker-compose.yaml': 'docker',
  'compose.yml': 'docker', 'compose.yaml': 'docker', '.dockerignore': 'docker',
  makefile: 'makefile', gnumakefile: 'makefile',
  'cmakelists.txt': 'cmake',
  license: 'license', licence: 'license', 'license.md': 'license', copying: 'license',
  readme: 'readme', 'readme.md': 'readme',
  changelog: 'changelog', 'changelog.md': 'changelog',
  contributing: 'contributing', 'contributing.md': 'contributing',
  todo: 'todo', 'todo.md': 'todo',
  '.gitignore': 'git', '.gitattributes': 'git', '.gitmodules': 'git',
  'cargo.toml': 'toml', 'cargo.lock': 'lock',
  'package-lock.json': 'lock', 'pnpm-lock.yaml': 'lock', 'yarn.lock': 'lock',
  '.env': 'env', '.bashrc': 'bash', '.bash_profile': 'bash', '.zshrc': 'bash',
  '.vimrc': 'vim',
};

const ICON_BY_EXT = {
  rs: 'rust', py: 'python', pyi: 'python',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'javascript',
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  json: 'json', jsonc: 'json',
  yaml: 'yaml', yml: 'yaml',
  toml: 'toml',
  md: 'markdown', markdown: 'markdown',
  html: 'html', htm: 'html',
  css: 'css', scss: 'css', sass: 'css', less: 'css',
  sh: 'bash', bash: 'bash', zsh: 'bash', fish: 'bash',
  ps1: 'powershell',
  go: 'go',
  c: 'c', h: 'c-header',
  cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp-header', hh: 'cpp-header',
  java: 'java', rb: 'ruby', php: 'php', nix: 'nix', vim: 'vim',
  log: 'log', xml: 'xml', csv: 'csv',
  sql: 'database', db: 'database', sqlite: 'database', sqlite3: 'database',
  lock: 'lock', env: 'env',
  conf: 'config', cfg: 'config', ini: 'config', service: 'config',
  repo: 'config', rules: 'config', list: 'config', desktop: 'config',
  pdf: 'pdf',
  zip: 'zip', gz: 'zip', tgz: 'zip', xz: 'zip', zst: 'zip', bz2: 'zip',
  '7z': 'zip', tar: 'zip', rar: 'zip',
  png: 'image', jpg: 'image', jpeg: 'image', gif: 'image', svg: 'image',
  webp: 'image', bmp: 'image', ico: 'image', avif: 'image',
  mp4: 'video', mkv: 'video', webm: 'video', mov: 'video', avi: 'video',
  mp3: 'audio', wav: 'audio', flac: 'audio', ogg: 'audio', m4a: 'audio',
  ttf: 'font', otf: 'font', woff: 'font', woff2: 'font',
  key: 'key', pub: 'key',
  pem: 'certificate', crt: 'certificate', cer: 'certificate',
  exe: 'exe',
  bin: 'binary', so: 'binary', o: 'binary', a: 'binary', deb: 'binary', rpm: 'binary',
  txt: 'text',
};

function iconIdFor(it) {
  if (it.kind === 'dir') return 'i-folder';
  if (it.kind === 'link') return 'i-symlink';
  const name = (it.name || '').toLowerCase();
  if (ICON_BY_NAME[name]) return 'i-' + ICON_BY_NAME[name];
  const dot = name.lastIndexOf('.');
  // A leading dot is the start of a dotfile, not an extension separator.
  const ext = dot > 0 ? name.slice(dot + 1) : '';
  return 'i-' + (ICON_BY_EXT[ext] || 'file');
}

function iconSvg(it) {
  return `<svg class="ic" aria-hidden="true"><use href="#${iconIdFor(it)}"></use></svg>`;
}

/* ---------------------------------------------------------------- files ---*/

const TEXT_EXT = /\.(txt|md|markdown|log|conf|cfg|ini|toml|yaml|yml|json|xml|csv|sh|bash|zsh|py|rs|go|c|h|cpp|hpp|js|ts|jsx|tsx|css|html|sql|service|env|gitignore|rules|repo|list)$/i;

function humanSize(n) {
  if (n === undefined || n === null) return '';
  const u = ['B', 'K', 'M', 'G', 'T'];
  let i = 0, v = Number(n);
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return (i === 0 ? v : v.toFixed(1)) + u[i];
}

/* Every toolbar button is the same square icon button. The word that used to
   be the button's label is now its accessible name and its tooltip, so nothing
   is lost by dropping the text -- see .fbtn--icon in ui/style.css. */
const barBtn = (a, label, cls = '') =>
  `<button type="button" class="fbtn fbtn--icon tip${cls ? ' ' + cls : ''}" data-a="${a}" data-tip="${label}" aria-label="${label}">` +
  `<svg class="ic-a" aria-hidden="true"><use href="#a-${a}"></use></svg></button>`;

function openFiles(startPath) {
  return createWindow({
    title: 'Files',
    app: 'files',
    width: 780,
    height: 500,
    build(entry) {
      const root = document.createElement('div');
      root.className = 'files';
      root.innerHTML = `
        <div class="files-bar">
          ${barBtn('up', 'Up')}
          ${barBtn('home', 'Home')}
          <div class="files-path">
            <svg class="ic" aria-hidden="true"><use href="#i-folder-open"></use></svg>
            <input data-el="path" type="text" aria-label="Current folder"
                   spellcheck="false" autocapitalize="off" autocomplete="off" autocorrect="off">
          </div>
          ${barBtn('refresh', 'Refresh')}
          ${barBtn('mkdir', 'New folder')}
          ${barBtn('upload', 'Upload')}
          ${barBtn('rename', 'Rename')}
          ${barBtn('delete', 'Delete', 'danger')}
          <input type="file" data-el="file" hidden>
        </div>
        <div class="files-head"><div>Name</div><div class="meta">Size</div><div class="meta">Mode</div><div class="meta l">Modified</div></div>
        <div class="files-list" data-el="list"></div>`;
      entry.body.appendChild(root);

      const $ = (n) => root.querySelector(`[data-el="${n}"]`);
      let cwd = startPath;
      let parent = null;
      let selected = null;

      async function load(path) {
        try {
          const d = await api('/api/fs/list?path=' + encodeURIComponent(path));
          cwd = d.path;
          parent = d.parent;
          selected = null;
          $('path').value = cwd;
          entry.titleEl.textContent = 'Files — ' + cwd;
          paintDock();
          render(d.entries);
        } catch (err) {
          $('list').innerHTML = `<div class="files-msg">${err.message}</div>`;
        }
      }

      function render(items) {
        const list = $('list');
        list.textContent = '';
        for (const it of items) {
          const row = document.createElement('div');
          row.className = 'frow' + (it.kind === 'dir' ? ' dir' : '');
          row.innerHTML = `
            <div class="nm">${iconSvg(it)}<span></span></div>
            <div class="meta">${it.kind === 'dir' ? '' : humanSize(it.size)}</div>
            <div class="meta">${it.mode || ''}</div>
            <div class="meta l">${it.mtime ? new Date(it.mtime * 1000).toLocaleString() : ''}</div>`;
          row.querySelector('.nm span:last-child').textContent = it.name;
          row.addEventListener('click', () => {
            list.querySelectorAll('.frow.sel').forEach((r) => r.classList.remove('sel'));
            row.classList.add('sel');
            selected = it;
          });
          row.addEventListener('dblclick', () => {
            const full = join(cwd, it.name);
            if (it.kind === 'dir') load(full);
            else if (TEXT_EXT.test(it.name) && it.size < 2 * 1024 * 1024) openEditor(full, iconIdFor(it));
            else download(full, it.name);
          });
          list.appendChild(row);
        }
      }

      const join = (a, b) => (a.endsWith('/') ? a + b : a + '/' + b);

      /* The path box is an address bar, not a caption: Enter goes there, Escape
         puts back the folder actually on screen. A failed load leaves what was
         typed alone so a typo can be corrected rather than retyped. */
      $('path').addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
          const want = $('path').value.trim();
          if (want) load(want);
        } else if (e.key === 'Escape') {
          $('path').value = cwd;
          $('path').blur();
        }
      });
      $('path').addEventListener('focus', () => $('path').select());

      root.addEventListener('click', async (e) => {
        const act = e.target.closest('[data-a]');
        if (!act) return;
        const a = act.dataset.a;
        try {
          if (a === 'up' && parent) load(parent);
          else if (a === 'home') load(STATE.home);
          else if (a === 'refresh') load(cwd);
          else if (a === 'upload') $('file').click();
          else if (a === 'mkdir') {
            const name = await askText('New folder', 'Name', '', 'Create');
            if (!name) return;
            await jsonPost('/api/fs/mkdir', { path: join(cwd, name) });
            load(cwd);
          } else if (a === 'rename') {
            if (!selected) return toast('Select something to rename first.');
            const was = selected.name;
            const name = await askText('Rename', 'New name', was, 'Rename');
            if (!name || name === was) return;
            await jsonPost('/api/fs/rename', { path: join(cwd, was), to: join(cwd, name) });
            load(cwd);
          } else if (a === 'delete') {
            if (!selected) return toast('Select something to delete first.');
            const doomed = selected.name;
            const ok = await askConfirm(
              'Delete ' + doomed + '?',
              selected.kind === 'dir'
                ? 'A folder has to be empty before it can go. This cannot be undone.'
                : 'This cannot be undone.',
              'Delete',
            );
            if (!ok) return;
            await jsonPost('/api/fs/remove', { path: join(cwd, doomed) });
            load(cwd);
          }
        } catch (err) {
          toast(err.message, 'bad');
        }
      });

      $('file').addEventListener('change', async (e) => {
        const f = e.target.files[0];
        if (!f) return;
        try {
          await api('/api/fs/write?path=' + encodeURIComponent(join(cwd, f.name)), {
            method: 'PUT',
            body: await f.arrayBuffer(),
          });
          load(cwd);
        } catch (err) { toast(err.message, 'bad'); }
        e.target.value = '';
      });

      load(startPath);
    },
  });
}

/* A file the editor will not take is saved, not opened in a tab of its own:
   window.open() is a pop-up, and a pop-up is the browser showing through. */
function download(path, name) {
  const a = document.createElement('a');
  a.href = '/api/fs/read?path=' + encodeURIComponent(path);
  a.download = name;
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}

/* --------------------------------------------------------------- editor ---*/

function openEditor(path, icon) {
  return createWindow({
    title: path.split('/').pop(),
    app: 'editor',
    icon: icon || 'i-file',
    width: 700,
    height: 460,
    build(entry) {
      const root = document.createElement('div');
      root.className = 'editor';
      root.innerHTML = `
        <div class="editor-bar">
          <button class="fbtn" data-a="save">Save</button>
          <span class="editor-state" data-el="state">loading…</span>
        </div>
        <textarea spellcheck="false" data-el="ta"></textarea>`;
      entry.body.appendChild(root);
      const ta = root.querySelector('[data-el="ta"]');
      const state = root.querySelector('[data-el="state"]');

      fetch('/api/fs/read?path=' + encodeURIComponent(path), { credentials: 'same-origin' })
        .then((r) => (r.ok ? r.text() : r.text().then((t) => { throw new Error(t); })))
        .then((t) => { ta.value = t; state.textContent = path; })
        .catch((e) => { state.textContent = 'error: ' + e.message; });

      ta.addEventListener('input', () => { state.textContent = path + ' — modified'; });

      root.querySelector('[data-a="save"]').addEventListener('click', async () => {
        state.textContent = 'saving…';
        try {
          await api('/api/fs/write?path=' + encodeURIComponent(path), {
            method: 'PUT',
            body: new Blob([ta.value]),
          });
          state.textContent = path + ' — saved';
        } catch (e) {
          state.textContent = 'error: ' + e.message;
        }
      });
    },
  });
}

/* ------------------------------------------------------------- terminal ---*/

function openTerminal() {
  return createWindow({
    title: 'Terminal',
    app: 'terminal',
    width: 760,
    height: 460,
    build(entry) {
      const host = document.createElement('div');
      host.className = 'term';
      entry.body.appendChild(host);

      const term = new Terminal({
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        fontSize: 13,
        cursorBlink: true,
        theme: { background: '#0d1117', foreground: '#e6eaf0', cursor: '#3fb6c8' },
      });
      const fit = new FitAddon.FitAddon();
      term.loadAddon(fit);
      term.open(host);
      setTimeout(() => fit.fit(), 0);

      const proto = location.protocol === 'https:' ? 'wss' : 'ws';
      const ws = new WebSocket(`${proto}://${location.host}/ws/term`);
      ws.binaryType = 'arraybuffer';
      const enc = new TextEncoder();

      const sendSize = () => {
        if (ws.readyState === 1) {
          ws.send(JSON.stringify({ t: 'resize', cols: term.cols, rows: term.rows }));
        }
      };

      ws.onopen = () => { fit.fit(); sendSize(); term.focus(); };
      ws.onmessage = (ev) => {
        term.write(typeof ev.data === 'string' ? ev.data : new Uint8Array(ev.data));
      };
      ws.onclose = () => term.write('\r\n\x1b[2m[session ended]\x1b[0m\r\n');
      ws.onerror = () => term.write('\r\n\x1b[31m[connection error]\x1b[0m\r\n');

      term.onData((d) => { if (ws.readyState === 1) ws.send(enc.encode(d)); });
      term.onResize(sendSize);

      entry.onResize = () => { try { fit.fit(); } catch (_) {} };
      entry.onClose = () => { try { ws.close(); } catch (_) {} term.dispose(); };

      const ro = new ResizeObserver(() => entry.onResize());
      ro.observe(host);
    },
  });
}

/* Windows that there is no reason to have two of. Re-opening one raises the
   window that already exists instead of stacking another identical copy. */
const singletons = new Map();

function openSingleton(key, open) {
  const existing = singletons.get(key);
  if (existing && openWindows.has(existing.id)) {
    raiseWindow(existing);
    return existing;
  }
  const entry = open();
  singletons.set(key, entry);
  return entry;
}

/* --------------------------------------------------------------- system ---*/

/* The update controls are painted from /api/system/info, which reports whether
   this session may use them. That is presentation only -- every update route
   re-checks on the server, because a hidden button is not an access control. */

function fmtTime(sec) {
  if (!sec) return 'unknown';
  return new Date(sec * 1000).toLocaleString();
}

const shortSha = (sha) => (sha && /^[0-9a-f]{7,}/.test(sha) ? sha.slice(0, 12) : sha || 'unknown');

function openSystem() {
  return createWindow({
    title: 'System',
    app: 'system',
    width: 760,
    height: 540,
    build(entry) {
      const root = document.createElement('div');
      root.className = 'sys';
      root.innerHTML = `
        <div class="sys-bar">
          <button class="fbtn" data-a="check">Check for updates</button>
          <button class="fbtn" data-a="apply" disabled>Update now</button>
          <span class="sys-state" data-el="state"></span>
        </div>
        <div class="sys-scroll">
          <div class="sys-info" data-el="info"></div>
          <div class="sys-note" data-el="note" hidden></div>
          <pre class="sys-log" data-el="log" hidden></pre>
        </div>`;
      entry.body.appendChild(root);

      const $ = (n) => root.querySelector(`[data-el="${n}"]`);
      const btn = (a) => root.querySelector(`[data-a="${a}"]`);

      let info = null;
      let timer = null;

      const setState = (t, cls) => {
        const el = $('state');
        el.textContent = t || '';
        el.className = 'sys-state' + (cls ? ' ' + cls : '');
      };

      function note(text, cls) {
        const el = $('note');
        el.hidden = !text;
        el.textContent = text || '';
        el.className = 'sys-note' + (cls ? ' ' + cls : '');
      }

      function row(label, value, mono) {
        const k = document.createElement('div');
        k.className = 'sys-k';
        k.textContent = label;
        const v = document.createElement('div');
        v.className = 'sys-v' + (mono ? ' mono' : '');
        v.textContent = value;
        const frag = document.createDocumentFragment();
        frag.append(k, v);
        return frag;
      }

      function showLog(text) {
        const el = $('log');
        if (!text) return;
        // Only stick to the bottom if the reader was already there, so scrolling
        // back through a build does not keep yanking them forward.
        const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        el.hidden = false;
        el.textContent = text;
        if (atBottom) el.scrollTop = el.scrollHeight;
      }

      function renderInfo() {
        const g = $('info');
        g.textContent = '';
        g.append(
          row('Version', info.build.version),
          row('Commit', shortSha(info.build.commit), true),
          row('Built', fmtTime(info.build.built)),
          row('Tracking', `${info.build.repo} @ ${info.build.ref}`, true),
          row('Host', info.hostname || location.hostname),
        );

        if (!info.updates.allowed) {
          const why = !info.updates.supported
            ? info.updates.reason
            : `updating requires membership of ${info.updates.admin_groups.join(' or ')}`;
          note('Updates are unavailable here: ' + why + '.');
          btn('check').disabled = true;
          btn('apply').disabled = true;
        }
      }

      /* Raw fetch rather than api(): during an update the difference between
         "connection refused" and "401" is the whole story, and api() flattens
         both into a thrown Error. */
      async function pollOnce() {
        let res;
        try {
          res = await fetch('/api/update/status', { credentials: 'same-origin' });
        } catch (_) {
          return { down: true };
        }
        if (res.status === 401) return { signedOut: true };
        if (!res.ok) {
          const b = await res.json().catch(() => ({}));
          return { error: b.error || res.statusText };
        }
        return { data: await res.json() };
      }

      function stopPolling() {
        if (timer) clearInterval(timer);
        timer = null;
      }

      function describe(st) {
        if (st.state === 'running') return `updating — ${st.phase || 'working'}…`;
        if (st.state === 'ok') return 'update complete';
        if (st.state === 'failed') return 'update failed' + (st.error ? ` — ${st.error}` : '');
        return '';
      }

      async function tick() {
        const r = await pollOnce();

        if (r.down) {
          // Expected: install.sh restarts the service at the end.
          setState('service restarting…');
          return;
        }
        if (r.signedOut) {
          // The service came back. Sessions live in memory, so it came back
          // without ours -- which is itself the signal that the restart landed.
          stopPolling();
          setState('update applied — signed out', 'ok');
          note('The service restarted, so every session ended. Sign in again to confirm the new version.', 'ok');
          setTimeout(() => {
            for (const id of [...openWindows.keys()]) closeWindow(id);
            STATE.username = null;
            STATE.admin = false;
            showLogin('Updated — the service restarted, so you were signed out.');
          }, 2500);
          return;
        }
        if (r.error) {
          stopPolling();
          setState(r.error, 'bad');
          return;
        }

        const st = r.data.status || {};
        showLog(r.data.log);
        setState(describe(st), st.state === 'failed' ? 'bad' : st.state === 'ok' ? 'ok' : '');

        if (st.state !== 'running') {
          stopPolling();
          btn('check').disabled = false;
          btn('apply').disabled = true;
          // The build may have changed under us if it finished without a
          // restart being needed.
          info = await api('/api/system/info').catch(() => info);
          if (info) renderInfo();
          if (st.state === 'failed') {
            note('The update failed and the running service was left as it was. The log above is the whole story; ' +
                 'the same run is also in `journalctl -u webdesk-update`.', 'bad');
          }
        }
      }

      function startPolling() {
        stopPolling();
        tick();
        timer = setInterval(tick, 2000);
      }

      btn('check').addEventListener('click', async () => {
        btn('check').disabled = true;
        setState('checking…');
        note('');
        try {
          const d = await jsonPost('/api/update/check', {});
          if (!d.comparable) {
            setState('cannot compare', 'bad');
            note(`This binary reports its commit as "${d.current}", so it cannot be compared with the remote. ` +
                 `The tracked ref is at ${shortSha(d.latest)}. Updating will install that.`);
            btn('apply').disabled = false;
          } else if (d.behind) {
            setState('update available', 'ok');
            note(`${shortSha(d.latest)} — ${d.message || 'no commit message'}` +
                 (d.date ? ` (${new Date(d.date).toLocaleString()})` : ''));
            btn('apply').disabled = false;
          } else {
            setState('up to date', 'ok');
            note(`Already at ${shortSha(d.latest)}, the newest commit on ${d.ref}.`);
            btn('apply').disabled = false;
            btn('apply').textContent = 'Reinstall';
          }
        } catch (e) {
          setState(e.message, 'bad');
        } finally {
          btn('check').disabled = false;
        }
      });

      btn('apply').addEventListener('click', async () => {
        const ok = await askConfirm(
          'Update WebDesk?',
          'This rebuilds from source on this host and restarts the service, which ' +
          'takes a few minutes. Sessions are held in memory, so every signed-in user ' +
          '(including you) will be signed out and any open terminal will end.\n\n' +
          'If the build fails the running version is left untouched.',
          'Update now',
        );
        if (!ok) return;
        btn('apply').disabled = true;
        btn('check').disabled = true;
        setState('starting…');
        note('');
        try {
          await jsonPost('/api/update/apply', {});
          startPolling();
        } catch (e) {
          setState(e.message, 'bad');
          btn('check').disabled = false;
        }
      });

      entry.onClose = () => {
        stopPolling();
        singletons.delete('system');
      };

      (async function load() {
        try {
          info = await api('/api/system/info');
          renderInfo();
        } catch (e) {
          note('Could not read system info: ' + e.message, 'bad');
          return;
        }
        // An update started before this window was opened -- or one that
        // finished while nobody was watching -- is still worth showing.
        const r = await pollOnce();
        if (r.data) {
          const st = r.data.status || {};
          if (st.state && st.state !== 'idle') {
            showLog(r.data.log);
            setState(describe(st), st.state === 'failed' ? 'bad' : st.state === 'ok' ? 'ok' : '');
            if (st.state === 'running') startPolling();
            else if (st.finished) {
              note(`Last update ${st.state === 'ok' ? 'succeeded' : 'failed'} at ${fmtTime(st.finished)}` +
                   (st.actor ? `, started by ${st.actor}` : '') + '.',
                   st.state === 'ok' ? 'ok' : 'bad');
            }
          }
        }
      })();
    },
  });
}

/* ------------------------------------------------------------- bootstrap --*/

const STATE = { username: null, home: '/', admin: false };

function showLogin(msg) {
  closeMenu();
  document.getElementById('desktop').hidden = true;
  document.getElementById('login').hidden = false;
  document.getElementById('login-err').textContent = msg || '';
  document.getElementById('u').focus();
}

function showDesktop() {
  document.getElementById('login').hidden = true;
  document.getElementById('desktop').hidden = false;

  const who = STATE.username + '@' + location.hostname;
  const btn = document.getElementById('whoami');
  btn.dataset.tip = who;
  btn.setAttribute('aria-label', who + ' — account menu');
  document.querySelector('#user-menu [data-el="who"]').textContent = who;
}

document.getElementById('login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const btn = document.getElementById('login-btn');
  const err = document.getElementById('login-err');
  btn.disabled = true;
  err.textContent = '';
  try {
    const d = await jsonPost('/api/login', {
      username: document.getElementById('u').value,
      password: document.getElementById('p').value,
    });
    STATE.username = d.username;
    STATE.home = d.home;
    STATE.admin = !!d.admin;
    document.getElementById('p').value = '';
    await loadIcons();
    showDesktop();
    openFiles(STATE.home);
  } catch (ex) {
    err.textContent = ex.message;
  } finally {
    btn.disabled = false;
  }
});

const APPS = {
  files: () => openFiles(STATE.home),
  terminal: () => openTerminal(),
};

document.querySelectorAll('.dock-btn[data-app]').forEach((b) => {
  const app = b.dataset.app;
  // Alt- or middle-click asks for another window rather than the one that is
  // already there.
  b.addEventListener('click', (e) => activateApp(app, APPS[app], e.altKey || e.metaKey));
  b.addEventListener('auxclick', (e) => {
    if (e.button === 1) { e.preventDefault(); APPS[app](); }
  });
});

/* ------------------------------------------------------------ user menu ---*/

/* The account button says nothing at all -- the username is its tooltip -- and
   the two things it used to take two buttons at the far end of the dock to do
   are two rows of a menu: who you are, which opens System, and the way out. */

const menuBtn = () => document.getElementById('whoami');
const menuEl = () => document.getElementById('user-menu');

function openMenu() {
  const menu = menuEl();
  if (!menu || !menu.hidden) return;
  menu.hidden = false;
  menuBtn().setAttribute('aria-expanded', 'true');
  document.addEventListener('pointerdown', onMenuOutside, true);
  document.addEventListener('keydown', onMenuKey, true);
  if (!reduceMotion() && typeof menu.animate === 'function') {
    menu.animate(
      [{ opacity: 0, transform: 'scale(.94) translateY(-4px)' }, { opacity: 1, transform: 'none' }],
      { duration: 130, easing: 'cubic-bezier(.16,.9,.3,1)' },
    );
  }
  const first = menu.querySelector('.menu-row');
  if (first) first.focus();
}

function closeMenu() {
  const menu = menuEl();
  if (!menu || menu.hidden) return;
  menu.hidden = true;
  menuBtn().setAttribute('aria-expanded', 'false');
  document.removeEventListener('pointerdown', onMenuOutside, true);
  document.removeEventListener('keydown', onMenuKey, true);
  // Only take focus back if the menu still had it; otherwise whatever was
  // clicked next keeps it.
  if (menu.contains(document.activeElement)) menuBtn().focus();
}

function onMenuOutside(e) {
  // The button's own pointerdown lands inside .account, so the click handler
  // below is left to do the toggling.
  if (!e.target.closest('.account')) closeMenu();
}

function onMenuKey(e) {
  if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    closeMenu();
    return;
  }
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
  e.preventDefault();
  const rows = [...menuEl().querySelectorAll('.menu-row')];
  const at = rows.indexOf(document.activeElement);
  const step = e.key === 'ArrowDown' ? 1 : -1;
  rows[(at + step + rows.length) % rows.length].focus();
}

menuBtn().addEventListener('click', () => {
  if (menuEl().hidden) openMenu();
  else closeMenu();
});

menuEl().addEventListener('click', (e) => {
  const row = e.target.closest('.menu-row');
  if (!row) return;
  closeMenu();
  if (row.dataset.a === 'system') openSingleton('system', openSystem);
  else if (row.dataset.a === 'logout') signOut();
});

async function signOut() {
  for (const id of [...openWindows.keys()]) closeWindow(id);
  try { await jsonPost('/api/logout', {}); } catch (_) {}
  STATE.username = null;
  STATE.admin = false;
  showLogin('Signed out.');
}

(async function boot() {
  try {
    const me = await api('/api/me');
    STATE.username = me.username;
    STATE.home = me.home;
    STATE.admin = !!me.admin;
    await loadIcons();
    showDesktop();
    openFiles(STATE.home);
  } catch (_) {
    showLogin();
  }
})();
