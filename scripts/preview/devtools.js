'use strict';

/* The preview's own controls. Loaded after ui/app.js, and deliberately the
 * only thing on the page that knows it is a preview.
 *
 *   scene switcher   jump straight to the state you want to look at
 *   device frames    check a layout at phone/tablet width without resizing
 *   inspector        click any pixel, get the ui/ file and line to name in a
 *                    prompt, copied to the clipboard
 *   hot reload       save a file under ui/, the tab reloads itself
 *
 * app.js declares its helpers as plain globals in a classic script, so they
 * are reachable here by bare name -- guarded, in case app.js failed to parse.
 */

const PREFIX = '/__preview';
const bar = document.createElement('div');
bar.className = 'pv-bar';
bar.innerHTML = `
  <span class="pv-dot" title="WebDesk preview — no server behind it"></span>
  <select class="pv-sel" data-el="scene" title="Scene"></select>
  <select class="pv-sel" data-el="device" title="Viewport">
    <option value="full">Full window</option>
    <option value="390x844">Phone · 390×844</option>
    <option value="768x1024">Tablet · 768×1024</option>
    <option value="1024x700">Small laptop · 1024×700</option>
    <option value="1440x900">Laptop · 1440×900</option>
  </select>
  <button class="pv-btn" data-a="inspect" title="Click an element to copy its ui/ source location (⌥I)">Inspect</button>
  <button class="pv-btn" data-a="reload" title="Reload now">↻</button>
  <button class="pv-btn pv-btn--quiet" data-a="hide" title="Hide this bar (⌥P)">×</button>`;
document.body.appendChild(bar);

const $ = (n) => bar.querySelector(`[data-el="${n}"]`);
const btn = (a) => bar.querySelector(`[data-a="${a}"]`);

/* ----------------------------------------------------------------- scenes */

const sceneSel = $('scene');
for (const [name, s] of Object.entries(PREVIEW.scenes)) {
  const opt = document.createElement('option');
  opt.value = name;
  opt.textContent = s.label;
  sceneSel.appendChild(opt);
}
sceneSel.value = PREVIEW.sceneName;
sceneSel.addEventListener('change', () => PREVIEW.setScene(sceneSel.value));

/* app.js opens Files on sign-in. Once it has, put the windows the scene asked
   for on screen instead -- driving the same functions the dock buttons do, so
   a scene can never drift from what a real click produces. */

function applyScene() {
  const want = PREVIEW.scene.open;
  if (!want || typeof openWindows === 'undefined') return;

  for (const id of [...openWindows.keys()]) closeWindow(id);

  for (const app of want) {
    if (app === 'files') openFiles('/home/hutson');
    else if (app === 'terminal') openTerminal();
    else if (app === 'editor') openEditor('/home/hutson/README.md', 'i-readme');
    else if (app === 'system') openSingleton('system', openSystem);
  }

  // The in-page dialogs replaced prompt()/confirm(), so they are part of the
  // UI now and worth a scene of their own.
  if (PREVIEW.scene.dialog === 'rename') {
    setTimeout(() => askText('Rename', 'New name', 'notes.md', 'Rename'), 120);
  } else if (PREVIEW.scene.dialog === 'delete') {
    setTimeout(() => askConfirm('Delete notes.md?', 'This cannot be undone.', 'Delete'), 120);
  }
}

// Wait for boot's /api/me round trip to paint the desktop before rearranging.
if (PREVIEW.scene.signedIn) {
  const settle = setInterval(() => {
    const desktop = document.getElementById('desktop');
    if (desktop && !desktop.hidden) {
      clearInterval(settle);
      setTimeout(applyScene, 30);
    }
  }, 40);
  setTimeout(() => clearInterval(settle), 4000);
}

/* ---------------------------------------------------------------- devices */

/* Everything in this UI is position:fixed, which normally resolves against the
   viewport and ignores any wrapper. A transformed ancestor becomes the
   containing block for fixed descendants, so a transform on the frame is what
   makes the app lay itself out at phone width. */

const DEVICE_KEY = 'wd-preview-device';
const deviceSel = $('device');
let frame = null;

function setDevice(value) {
  localStorage.setItem(DEVICE_KEY, value);
  const login = document.getElementById('login');
  const desktop = document.getElementById('desktop');

  if (value === 'full') {
    if (frame) {
      document.body.append(login, desktop);
      frame.remove();
      frame = null;
    }
    document.body.classList.remove('pv-framed');
    return;
  }

  const [w, h] = value.split('x').map(Number);
  if (!frame) {
    frame = document.createElement('div');
    frame.className = 'pv-frame';
    document.body.appendChild(frame);
  }
  frame.style.width = w + 'px';
  frame.style.height = h + 'px';
  frame.append(login, desktop);
  document.body.classList.add('pv-framed');

  // The windows layer sized itself to the old viewport; nudge the app to
  // re-measure the way a real resize would.
  window.dispatchEvent(new Event('resize'));
}

deviceSel.value =
  new URLSearchParams(location.search).get('device') ||
  localStorage.getItem(DEVICE_KEY) ||
  'full';
deviceSel.addEventListener('change', () => {
  setDevice(deviceSel.value);
  // Open windows kept the size they were given for the old viewport; reopening
  // them is the only honest way to see the layout at the new one.
  setTimeout(applyScene, 30);
});
// Synchronously, before any window opens: createWindow() measures the layer it
// is placed in, so a frame applied afterwards would leave windows sized for the
// full viewport and hanging off the edge.
if (deviceSel.value !== 'full') setDevice(deviceSel.value);

/* -------------------------------------------------------------- inspector */

let index = { css: {}, js: {} };
fetch(PREFIX + '/index')
  .then((r) => r.json())
  .then((d) => { index = d; })
  .catch(() => {});

const halo = document.createElement('div');
halo.className = 'pv-halo';
halo.hidden = true;
document.body.appendChild(halo);

const card = document.createElement('div');
card.className = 'pv-card';
card.hidden = true;
document.body.appendChild(card);

let inspecting = false;

function setInspect(on) {
  inspecting = on;
  btn('inspect').classList.toggle('on', on);
  document.body.classList.toggle('pv-inspecting', on);
  if (!on) {
    halo.hidden = true;
    card.hidden = true;
  }
}

btn('inspect').addEventListener('click', () => setInspect(!inspecting));

const classesOf = (el) => [...el.classList].filter((c) => !c.startsWith('pv-'));

/** A short, readable path — the ancestors that carry a class, nearest last. */
function domPath(el) {
  const parts = [];
  for (let n = el; n && n !== document.body; n = n.parentElement) {
    if (n.id) { parts.unshift('#' + n.id); break; }
    const cls = classesOf(n);
    if (cls.length) parts.unshift(n.tagName.toLowerCase() + '.' + cls.join('.'));
  }
  return parts.slice(-4).join(' > ');
}

/* Text nodes and layout spans often carry no class of their own. The nearest
   classed ancestor is what a prompt would have to name anyway, so look up
   from there rather than reporting nothing. */
function anchorOf(el) {
  for (let n = el; n && n !== document.body; n = n.parentElement) {
    if (classesOf(n).length) return n;
  }
  return el;
}

/** Where the rules that style this element live, base rules before states. */
function cssHits(el) {
  const out = [];
  for (const cls of classesOf(el)) {
    for (const hit of index.css[cls] || []) {
      if (!out.some((o) => o.line === hit.line)) out.push(hit);
    }
  }
  // A prompt wants ".fbtn" before ".fbtn:hover": the base rule is what an edit
  // usually targets, and a state rule only makes sense once you have seen it.
  const rank = (s) => (s.includes(':') ? 1 : 0) * 100 + s.split(/\s+/).length;
  return out
    .sort((a, b) => rank(a.selector) - rank(b.selector) || a.line - b.line)
    .slice(0, 6);
}

function jsHits(el) {
  const out = [];
  for (const cls of classesOf(el)) {
    for (const hit of index.js[cls] || []) {
      if (!out.some((o) => o.line === hit.line)) out.push(hit);
    }
  }
  return out.slice(0, 4);
}

/** The block to paste into a prompt: what was clicked, and where it is defined. */
function report(el) {
  const anchor = anchorOf(el);
  const css = cssHits(anchor);
  const js = jsHits(anchor);
  const lines = [];
  lines.push(`element: ${el.tagName.toLowerCase()}${classesOf(el).map((c) => '.' + c).join('')}`);
  if (el.textContent && el.textContent.trim().length < 60) {
    lines.push(`text: ${JSON.stringify(el.textContent.trim())}`);
  }
  lines.push(`path: ${domPath(el)}`);
  if (anchor !== el) {
    lines.push(
      `(no class of its own — lines below are for the nearest classed ancestor, ` +
      `${anchor.tagName.toLowerCase()}${classesOf(anchor).map((c) => '.' + c).join('')})`
    );
  }
  if (css.length) {
    lines.push('styled by:');
    for (const h of css) lines.push(`  ui/style.css:${h.line}  ${h.selector}`);
  }
  if (js.length) {
    lines.push('built in:');
    for (const h of js) lines.push(`  ui/app.js:${h.line}  ${h.text}`);
  }
  return lines.join('\n');
}

function place(el) {
  const r = el.getBoundingClientRect();
  halo.hidden = false;
  halo.style.left = r.left + 'px';
  halo.style.top = r.top + 'px';
  halo.style.width = r.width + 'px';
  halo.style.height = r.height + 'px';
}

document.addEventListener('mousemove', (e) => {
  if (!inspecting) return;
  const el = e.target;
  if (!el || bar.contains(el) || el === halo || card.contains(el)) return;
  place(el);
}, true);

document.addEventListener('click', async (e) => {
  if (!inspecting) return;
  if (bar.contains(e.target) || card.contains(e.target)) return;
  e.preventDefault();
  e.stopPropagation();

  const text = report(e.target);
  card.textContent = '';
  const pre = document.createElement('pre');
  pre.textContent = text;
  const foot = document.createElement('div');
  foot.className = 'pv-card-foot';
  const status = document.createElement('span');
  const close = document.createElement('button');
  close.className = 'pv-btn pv-btn--quiet';
  close.textContent = 'close';
  close.addEventListener('click', () => { card.hidden = true; });
  foot.append(status, close);
  card.append(pre, foot);
  card.hidden = false;

  try {
    await navigator.clipboard.writeText(text);
    status.textContent = 'copied to clipboard';
  } catch (_) {
    status.textContent = 'select and copy';
  }
}, true);

/* ------------------------------------------------------------- hot reload */

let token = null;
// ?nowatch=1 leaves the tab alone -- for screenshotting, or for reading a
// state that a stray save would otherwise reset.
const watching = new URLSearchParams(location.search).get('nowatch') !== '1';
if (watching) setInterval(async () => {
  try {
    const now = await (await fetch(PREFIX + '/reload')).text();
    if (token === null) token = now;
    else if (now !== token) location.reload();
  } catch (_) {
    // Server stopped; keep polling so the tab recovers when it comes back.
  }
}, 600);

btn('reload').addEventListener('click', () => location.reload());
btn('hide').addEventListener('click', () => bar.classList.add('pv-hidden'));

document.addEventListener('keydown', (e) => {
  if (!e.altKey || e.metaKey || e.ctrlKey) return;
  const k = e.key.toLowerCase();
  if (k === 'p') { bar.classList.toggle('pv-hidden'); e.preventDefault(); }
  if (k === 'i') { setInspect(!inspecting); e.preventDefault(); }
  if (k === 'escape') setInspect(false);
});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && inspecting) setInspect(false);
});

PREVIEW.log = (msg) => { PREVIEW.calls = (PREVIEW.calls || []).concat(msg).slice(-50); };

console.info(
  '%cWebDesk preview%c  scene: ' + PREVIEW.sceneName +
  '\n⌥I inspect · ⌥P hide bar · PREVIEW.setScene(name) · PREVIEW.calls',
  'background:#3fb6c8;color:#062229;padding:2px 6px;border-radius:4px;font-weight:600',
  'color:#8d9aad'
);
