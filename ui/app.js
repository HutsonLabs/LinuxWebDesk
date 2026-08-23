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

/* -------------------------------------------------------------- windows ---*/

let zTop = 10;
let focused = null;
const openWindows = new Map();
let winSeq = 0;

function createWindow({ title, width = 720, height = 460, build }) {
  const id = ++winSeq;
  const layer = document.getElementById('windows');

  const win = document.createElement('div');
  win.className = 'win';
  const offset = (openWindows.size % 6) * 26;
  win.style.width = Math.min(width, layer.clientWidth - 40) + 'px';
  win.style.height = Math.min(height, layer.clientHeight - 40) + 'px';
  win.style.left = Math.max(12, (layer.clientWidth - width) / 2 + offset) + 'px';
  win.style.top = Math.max(12, (layer.clientHeight - height) / 2 - 20 + offset) + 'px';

  const bar = document.createElement('div');
  bar.className = 'win-bar';
  const titleEl = document.createElement('div');
  titleEl.className = 'win-title';
  titleEl.textContent = title;
  const minBtn = document.createElement('button');
  minBtn.className = 'win-btn';
  minBtn.type = 'button';
  minBtn.title = 'Minimize';
  minBtn.textContent = '–';
  const closeBtn = document.createElement('button');
  closeBtn.className = 'win-btn close';
  closeBtn.type = 'button';
  closeBtn.title = 'Close';
  closeBtn.textContent = '×';
  bar.append(titleEl, minBtn, closeBtn);

  const body = document.createElement('div');
  body.className = 'win-body';

  const grip = document.createElement('div');
  grip.className = 'win-resize';

  win.append(bar, body, grip);
  layer.appendChild(win);

  const entry = { id, win, body, titleEl, onClose: null, onResize: null };
  openWindows.set(id, entry);

  const focus = () => {
    if (focused && focused !== win) focused.classList.remove('focused');
    win.style.zIndex = ++zTop;
    win.classList.add('focused');
    focused = win;
    paintTasks();
  };
  win.addEventListener('pointerdown', focus, true);
  focus();

  // --- drag. Pointer capture plus a pointer-events guard on the body keeps
  // the gesture alive when the cursor passes over the terminal canvas.
  bar.addEventListener('pointerdown', (e) => {
    if (e.target.closest('.win-btn')) return;
    const sx = e.clientX, sy = e.clientY;
    const ox = win.offsetLeft, oy = win.offsetTop;
    bar.setPointerCapture(e.pointerId);
    win.classList.add('dragging');
    const move = (ev) => {
      const nx = Math.min(Math.max(ox + ev.clientX - sx, -win.offsetWidth + 90), layer.clientWidth - 90);
      const ny = Math.min(Math.max(oy + ev.clientY - sy, 0), layer.clientHeight - 34);
      win.style.left = nx + 'px';
      win.style.top = ny + 'px';
    };
    const up = () => {
      win.classList.remove('dragging');
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
    grip.setPointerCapture(e.pointerId);
    win.classList.add('dragging');
    const move = (ev) => {
      win.style.width = Math.max(320, ow + ev.clientX - sx) + 'px';
      win.style.height = Math.max(200, oh + ev.clientY - sy) + 'px';
      if (entry.onResize) entry.onResize();
    };
    const up = () => {
      win.classList.remove('dragging');
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', up);
      if (entry.onResize) entry.onResize();
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
  });

  minBtn.addEventListener('click', () => { win.hidden = true; paintTasks(); });
  closeBtn.addEventListener('click', () => closeWindow(id));

  build(entry);
  paintTasks();
  return entry;
}

function closeWindow(id) {
  const e = openWindows.get(id);
  if (!e) return;
  if (e.onClose) { try { e.onClose(); } catch (_) {} }
  e.win.remove();
  openWindows.delete(id);
  paintTasks();
}

function paintTasks() {
  const bar = document.getElementById('tasks');
  bar.textContent = '';
  for (const [id, e] of openWindows) {
    const b = document.createElement('button');
    b.className = 'task' + (e.win === focused && !e.win.hidden ? ' active' : '');
    b.type = 'button';
    b.textContent = e.titleEl.textContent;
    b.addEventListener('click', () => {
      e.win.hidden = false;
      e.win.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
      if (e.onResize) e.onResize();
    });
    bar.appendChild(b);
  }
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
  `<button class="fbtn fbtn--icon${cls ? ' ' + cls : ''}" data-a="${a}" data-tip="${label}" aria-label="${label}">` +
  `<svg class="ic-a" aria-hidden="true"><use href="#a-${a}"></use></svg></button>`;

function openFiles(startPath) {
  createWindow({
    title: 'Files',
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
          paintTasks();
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
            else if (TEXT_EXT.test(it.name) && it.size < 2 * 1024 * 1024) openEditor(full);
            else window.open('/api/fs/read?path=' + encodeURIComponent(full), '_blank');
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
            const name = prompt('New folder name');
            if (!name) return;
            await jsonPost('/api/fs/mkdir', { path: join(cwd, name) });
            load(cwd);
          } else if (a === 'rename') {
            if (!selected) return alert('Select something first.');
            const name = prompt('Rename to', selected.name);
            if (!name || name === selected.name) return;
            await jsonPost('/api/fs/rename', { path: join(cwd, selected.name), to: join(cwd, name) });
            load(cwd);
          } else if (a === 'delete') {
            if (!selected) return alert('Select something first.');
            if (!confirm(`Delete ${selected.name}? Folders must be empty.`)) return;
            await jsonPost('/api/fs/remove', { path: join(cwd, selected.name) });
            load(cwd);
          }
        } catch (err) {
          alert(err.message);
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
        } catch (err) { alert(err.message); }
        e.target.value = '';
      });

      load(startPath);
    },
  });
}

/* --------------------------------------------------------------- editor ---*/

function openEditor(path) {
  createWindow({
    title: path.split('/').pop(),
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
  createWindow({
    title: 'Terminal',
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
    existing.win.hidden = false;
    existing.win.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    if (existing.onResize) existing.onResize();
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
        const ok = confirm(
          'Update WebDesk?\n\n' +
          'This rebuilds from source on this host and restarts the service, which ' +
          'takes a few minutes. Sessions are held in memory, so every signed-in user ' +
          '(including you) will be signed out and any open terminal will end.\n\n' +
          'If the build fails the running version is left untouched.'
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
  document.getElementById('desktop').hidden = true;
  document.getElementById('login').hidden = false;
  document.getElementById('login-err').textContent = msg || '';
  document.getElementById('u').focus();
}

function showDesktop() {
  document.getElementById('login').hidden = true;
  document.getElementById('desktop').hidden = false;
  document.getElementById('whoami').textContent = STATE.username + '@' + location.hostname;
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

document.querySelectorAll('.dock-btn[data-app]').forEach((b) => {
  b.addEventListener('click', () => {
    if (b.dataset.app === 'files') openFiles(STATE.home);
    else openTerminal();
  });
});

document.getElementById('whoami').addEventListener('click', () => {
  openSingleton('system', openSystem);
});

document.getElementById('logout').addEventListener('click', async () => {
  for (const id of [...openWindows.keys()]) closeWindow(id);
  try { await jsonPost('/api/logout', {}); } catch (_) {}
  STATE.username = null;
  STATE.admin = false;
  showLogin('Signed out.');
});

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
