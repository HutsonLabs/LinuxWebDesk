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

/* ------------------------------------------------------------------ tap ---*/

/* Controls listen for a tap, not a click. A click is what a mouse produces
   reliably and a finger does not: the browser holds a touch back while it
   decides between a tap and the start of a scroll, and a finger that slides a
   couple of pixels on the way down settles that the wrong way, after which no
   click is sent at all. The hover the touch applied on the way in stays, so
   the control lights up and does nothing -- which is what closing a window
   looked like on a touchscreen. touch-action in the stylesheet stops the
   browser claiming the gestures that were never scrolls; this deals with the
   release itself for the pointers it still gives up on.

   A mouse is left to its own click, with the buttons and modifier keys that
   come with it, and so is the keyboard -- Enter on a focused button arrives
   the same way. A finger has to come back up inside the control it went down
   on, and near enough to where it went down, so sliding off a close button
   still means no. */

const TAP_SLOP = 14;

function onTap(el, fn) {
  let from = null;
  let taken = -Infinity;

  el.addEventListener('pointerdown', (e) => {
    from = e.isPrimary && e.pointerType !== 'mouse' ? { x: e.clientX, y: e.clientY } : null;
  });

  el.addEventListener('pointerup', (e) => {
    const at = from;
    from = null;
    if (!at) return;
    // A touch is captured by whatever it went down on, so the release is
    // reported here wherever the finger has actually got to. Ask the geometry
    // rather than the target.
    const r = el.getBoundingClientRect();
    if (e.clientX < r.left || e.clientX > r.right) return;
    if (e.clientY < r.top || e.clientY > r.bottom) return;
    if (Math.abs(e.clientX - at.x) + Math.abs(e.clientY - at.y) > TAP_SLOP) return;
    taken = e.timeStamp;
    fn(e);
  });

  el.addEventListener('pointercancel', () => { from = null; });

  el.addEventListener('click', (e) => {
    // The browser sometimes sends the click after all. A tap already dealt
    // with is not to be dealt with twice.
    if (e.timeStamp - taken < 700) return;
    fn(e);
  });
}

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

    onTap(cancel, () => finish(null));
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
  onTap(el, dismiss);
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

/* Snap zones. Dragging a title bar until the pointer touches an edge of the
   desktop offers a region -- a half, a quarter, or the whole of it -- as an
   outline; letting go fills it. Dragging a filled window off again hands back
   the size it had before, keeping it under the cursor. Hold Alt to move a
   window without any of this. The work area stops above the dock: a snapped
   window sits on the dock rather than sliding under it. */
const SNAP_EDGE = 28;    // how close to an edge the pointer must come
const SNAP_CORNER = 140; // ...and how near the end of that edge to take a quarter

/* Every zone as a fraction of the work area, in the order the menu lists them.
   The drag and the menu are two ways of asking for the same seven regions, so
   the geometry is written once here and read by both -- and the same fractions
   draw the little diagram on each menu row. */
const ZONES = [
  { key: 'full', label: 'Full screen', f: [0, 0, 1, 1] },
  { key: 'left', label: 'Left half', f: [0, 0, 0.5, 1] },
  { key: 'right', label: 'Right half', f: [0.5, 0, 0.5, 1] },
  { key: 'top-left', label: 'Top left', f: [0, 0, 0.5, 0.5] },
  { key: 'top-right', label: 'Top right', f: [0.5, 0, 0.5, 0.5] },
  { key: 'bottom-left', label: 'Bottom left', f: [0, 0.5, 0.5, 0.5] },
  { key: 'bottom-right', label: 'Bottom right', f: [0.5, 0.5, 0.5, 0.5] },
];

function zoneRect(key, layer) {
  const z = ZONES.find((p) => p.key === key);
  if (!z) return null;
  const w = layer.clientWidth, h = layer.clientHeight - dockBand();
  const [fx, fy, fw, fh] = z.f;
  return { left: fx * w, top: fy * h, width: fw * w, height: fh * h };
}

const zoneFor = (key, layer) => ({ key, rect: zoneRect(key, layer) });

function zoneAt(cx, cy, layer) {
  const r = layer.getBoundingClientRect();
  const x = cx - r.left, y = cy - r.top;
  const w = layer.clientWidth, h = layer.clientHeight - dockBand();
  if (x < 0 || y < 0 || x > w || y > h) return null;

  const nearL = x <= SNAP_EDGE, nearR = x >= w - SNAP_EDGE;

  // A corner beats the edge it sits on, so the quarters are reachable without
  // having to find a 28px square.
  if (nearL || nearR) {
    const near = nearL ? 'left' : 'right';
    if (y <= SNAP_CORNER) return zoneFor('top-' + near, layer);
    if (y >= h - SNAP_CORNER) return zoneFor('bottom-' + near, layer);
    return zoneFor(near, layer);
  }
  if (y <= SNAP_EDGE) return zoneFor('full', layer);
  return null;
}

/* One outline, reused. It is parked in whichever layer asked for it and sits
   just under the window being dragged. */
let ghost = null;
let ghostOn = false;

function showZone(layer, zone) {
  if (!zone) return hideZone();
  if (!ghost) {
    ghost = document.createElement('div');
    ghost.className = 'snap-ghost';
  }
  if (ghost.parentNode !== layer) layer.appendChild(ghost);
  ghost.style.zIndex = Math.max(1, zTop - 1);
  const { left, top, width, height } = zone.rect;
  ghost.style.left = Math.round(left) + 'px';
  ghost.style.top = Math.round(top) + 'px';
  ghost.style.width = Math.round(width) + 'px';
  ghost.style.height = Math.round(height) + 'px';
  // Only an outline that is already up slides between zones; the first one
  // fades in where it belongs instead of flying in from the corner.
  ghost.classList.add('on');
  ghostOn = true;
}

function hideZone() {
  if (ghost && ghostOn) ghost.classList.remove('on');
  ghostOn = false;
}

function applyZone(entry, zone) {
  const { win } = entry;
  // Remember the loose size once, so snapping from one zone straight to
  // another does not record a half screen as the size to come back to.
  if (!entry.snapped) entry.snapped = { w: win.offsetWidth, h: win.offsetHeight };
  const { left, top, width, height } = zone.rect;
  win.classList.add('settling');
  win.style.left = Math.round(left) + 'px';
  win.style.top = Math.round(top) + 'px';
  win.style.width = Math.round(Math.max(320, width)) + 'px';
  win.style.height = Math.round(Math.max(200, height)) + 'px';
  setTimeout(() => {
    win.classList.remove('settling');
    if (entry.onResize) entry.onResize();
  }, 150);
}

/* The layout menu: the same seven regions the drag offers, as a list you can
   pick from without dragging anything. Hovering a row lights the outline the
   drag would have shown, so the menu teaches the gesture rather than replacing
   it. Only one is ever open, and it lives on the body -- a window clips its own
   overflow, which would cut the menu off at the title bar. */

let zoneMenu = null;
let zoneMenuBtn = null;

const zoneMini = (f) =>
  '<span class="zone-mini" aria-hidden="true"><span class="zone-fill" style="' +
  `left:${f[0] * 100}%;top:${f[1] * 100}%;width:${f[2] * 100}%;height:${f[3] * 100}%"></span></span>`;

function closeZoneMenu() {
  if (!zoneMenu) return;
  const btn = zoneMenuBtn;
  const hadFocus = zoneMenu.contains(document.activeElement);
  zoneMenu.remove();
  zoneMenu = null;
  zoneMenuBtn = null;
  document.removeEventListener('pointerdown', onZoneOutside, true);
  document.removeEventListener('keydown', onZoneKey, true);
  window.removeEventListener('resize', closeZoneMenu);
  hideZone();
  if (btn) {
    btn.setAttribute('aria-expanded', 'false');
    // Only take focus back if the menu still had it, so a click elsewhere is
    // not yanked back to the window that was being arranged.
    if (hadFocus) btn.focus();
  }
}

/* Shutting a window, or folding it away, takes its menu with it -- the menu is
   on the body, so nothing else would. */
function closeZoneMenuFor(entry) {
  if (zoneMenuBtn && entry.win.contains(zoneMenuBtn)) closeZoneMenu();
}

function onZoneOutside(e) {
  if (zoneMenu && !zoneMenu.contains(e.target) && e.target.closest('button') !== zoneMenuBtn) {
    closeZoneMenu();
  }
}

function onZoneKey(e) {
  if (e.key === 'Escape') {
    e.preventDefault();
    e.stopPropagation();
    closeZoneMenu();
    return;
  }
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
  e.preventDefault();
  const rows = [...zoneMenu.querySelectorAll('.zone-row')];
  const at = rows.indexOf(document.activeElement);
  const step = e.key === 'ArrowDown' ? 1 : -1;
  rows[(at + step + rows.length) % rows.length].focus();
}

function toggleZoneMenu(entry, btn, layer) {
  const mine = zoneMenuBtn === btn;
  closeZoneMenu();
  if (mine) return;

  const el = document.createElement('div');
  el.className = 'menu zone-menu';
  el.setAttribute('role', 'menu');
  el.setAttribute('aria-label', 'Fill a region');
  el.innerHTML = ZONES.map(
    (z) =>
      `<button type="button" class="zone-row" role="menuitem" data-zone="${z.key}">` +
      `${zoneMini(z.f)}<span class="menu-label">${z.label}</span></button>`,
  ).join('');
  document.body.appendChild(el);

  // Hung off the button's right edge, since the control sits near the window's
  // own right edge, and flipped above the bar if there is no room below.
  const r = btn.getBoundingClientRect();
  const w = el.offsetWidth, h = el.offsetHeight;
  const left = Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8));
  const below = r.bottom + 8;
  const top = below + h > window.innerHeight - 8 ? Math.max(8, r.top - 8 - h) : below;
  el.style.left = Math.round(left) + 'px';
  el.style.top = Math.round(top) + 'px';

  zoneMenu = el;
  zoneMenuBtn = btn;
  btn.setAttribute('aria-expanded', 'true');
  document.addEventListener('pointerdown', onZoneOutside, true);
  document.addEventListener('keydown', onZoneKey, true);
  window.addEventListener('resize', closeZoneMenu);

  if (!reduceMotion() && typeof el.animate === 'function') {
    el.animate(
      [{ opacity: 0, transform: 'scale(.94) translateY(-4px)' }, { opacity: 1, transform: 'none' }],
      { duration: 130, easing: 'cubic-bezier(.16,.9,.3,1)' },
    );
  }

  for (const row of el.querySelectorAll('.zone-row')) {
    const zone = () => zoneFor(row.dataset.zone, layer);
    row.addEventListener('mouseenter', () => showZone(layer, zone()));
    row.addEventListener('focus', () => showZone(layer, zone()));
    onTap(row, () => {
      const z = zone();
      closeZoneMenu();
      applyZone(entry, z);
    });
  }
  el.addEventListener('mouseleave', () => hideZone());

  const first = el.querySelector('.zone-row');
  if (first) first.focus();
}

function createWindow({ title, width = 720, height = 460, app = '', icon = '', titleIcon = '', build }) {
  const id = ++winSeq;
  const layer = document.getElementById('windows');
  const free = layer.clientHeight - dockBand();

  const win = document.createElement('div');
  win.className = 'win';
  const offset = (openWindows.size % 6) * 26;
  win.style.width = Math.max(320, Math.min(width, layer.clientWidth - 40)) + 'px';
  win.style.height = Math.max(200, Math.min(height, free - 40)) + 'px';
  win.style.left = Math.max(12, (layer.clientWidth - width) / 2 + offset) + 'px';
  win.style.top = Math.max(12, (free - height) / 2 - 20 + offset) + 'px';

  const bar = document.createElement('div');
  bar.className = 'win-bar';
  if (titleIcon) {
    const mark = document.createElement('span');
    mark.className = 'win-mark';
    mark.innerHTML = `<svg class="ic-a" aria-hidden="true"><use href="#${titleIcon}"></use></svg>`;
    bar.appendChild(mark);
  }
  const titleEl = document.createElement('div');
  titleEl.className = 'win-title';
  titleEl.textContent = title;
  // An app's own bar controls sit next to its name; the gap after them is what
  // keeps the minimise and close buttons on the far right.
  const tools = document.createElement('div');
  tools.className = 'win-tools';
  const gap = document.createElement('div');
  gap.className = 'win-gap';
  const minBtn = document.createElement('button');
  minBtn.className = 'win-btn tip';
  minBtn.type = 'button';
  minBtn.dataset.tip = 'Minimize';
  minBtn.setAttribute('aria-label', 'Minimize');
  minBtn.textContent = '–';
  // The middle control of the three. Its mark is a square drawn in the button
  // rather than a sprite icon, so it carries the same hairline weight as the
  // dash and the cross it sits between.
  const zoneBtn = document.createElement('button');
  zoneBtn.className = 'win-btn win-btn--zone tip';
  zoneBtn.type = 'button';
  zoneBtn.dataset.tip = 'Fill a region';
  zoneBtn.setAttribute('aria-label', 'Fill a region');
  zoneBtn.setAttribute('aria-haspopup', 'menu');
  zoneBtn.setAttribute('aria-expanded', 'false');
  // A real child, not a pseudo-element: .tip already owns both ::before and
  // ::after on this button to draw its tooltip.
  zoneBtn.innerHTML = '<span class="zone-mark" aria-hidden="true"></span>';
  const closeBtn = document.createElement('button');
  closeBtn.className = 'win-btn close tip';
  closeBtn.type = 'button';
  closeBtn.dataset.tip = 'Close';
  closeBtn.setAttribute('aria-label', 'Close');
  closeBtn.textContent = '×';
  bar.append(titleEl, gap, tools, minBtn, zoneBtn, closeBtn);

  const body = document.createElement('div');
  body.className = 'win-body';

  const grip = document.createElement('div');
  grip.className = 'win-resize';

  win.append(bar, body, grip);
  layer.appendChild(win);

  const entry = { id, win, body, titleEl, tools, app, icon, gen: 0, snapped: null, onClose: null, onResize: null };
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
    let ox = win.offsetLeft, oy = win.offsetTop;
    let zone = null;
    bar.setPointerCapture(e.pointerId);
    win.classList.add('dragging');
    const move = (ev) => {
      // A snapped window comes loose at the size it had before, re-hung under
      // the cursor at the same fraction along its bar.
      if (entry.snapped && Math.abs(ev.clientX - sx) + Math.abs(ev.clientY - sy) > 12) {
        const frac = win.offsetWidth ? (sx - ox) / win.offsetWidth : 0.5;
        const { w, h } = entry.snapped;
        win.style.width = w + 'px';
        win.style.height = h + 'px';
        ox = Math.round(sx - w * frac);
        entry.snapped = null;
        if (entry.onResize) entry.onResize();
      }
      const nx = Math.min(Math.max(ox + ev.clientX - sx, -win.offsetWidth + 90), layer.clientWidth - 90);
      const ny = Math.min(Math.max(oy + ev.clientY - sy, 0), layer.clientHeight - dockBand() - 34);
      win.style.left = nx + 'px';
      win.style.top = ny + 'px';
      zone = ev.altKey ? null : zoneAt(ev.clientX, ev.clientY, layer);
      showZone(layer, zone);
    };
    const up = () => {
      win.classList.remove('dragging');
      hideZone();
      if (zone) applyZone(entry, zone);
      zone = null;
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
    entry.snapped = null;
    const move = (ev) => {
      win.style.width = Math.max(320, ow + ev.clientX - sx) + 'px';
      win.style.height = Math.max(200, oh + ev.clientY - sy) + 'px';
      if (entry.onResize) entry.onResize();
    };
    const up = () => {
      win.classList.remove('dragging');
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', up);
      grip.removeEventListener('pointercancel', up);
      if (entry.onResize) entry.onResize();
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', up);
    // A touch the browser decides it wants back ends the gesture without a
    // pointerup. Without this the window stayed .dragging, which is to say
    // with its body untouchable, for as long as it was open.
    grip.addEventListener('pointercancel', up);
  });

  onTap(minBtn, () => minimizeWindow(entry));
  onTap(closeBtn, () => closeWindow(id));
  onTap(zoneBtn, () => toggleZoneMenu(entry, zoneBtn, layer));

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
  closeZoneMenuFor(e);
  openWindows.delete(id);
  if (e.win === focused) focused = null;
  paintDock();

  if (e.win.hidden) e.win.remove();
  else genie(e.win, rect, 'out').then(() => e.win.remove());
}

function minimizeWindow(e) {
  if (e.win.hidden) return;
  closeZoneMenuFor(e);
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
      onTap(b, () => raiseWindow(e));
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
    titleIcon: 'a-files',
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
      let entries = [];
      // Dotfiles are noise in most folders, so the folder opens without them.
      let showHidden = false;
      // What last touched a row, so a tap and a click can mean different
      // things on the machines that have both.
      let pointer = 'mouse';

      async function load(path) {
        try {
          const d = await api('/api/fs/list?path=' + encodeURIComponent(path));
          cwd = d.path;
          parent = d.parent;
          selected = null;
          $('path').value = cwd;
          entries = d.entries;
          render();
        } catch (err) {
          $('list').innerHTML = `<div class="files-msg">${err.message}</div>`;
        }
      }

      const isHidden = (it) => (it.name || '').startsWith('.');

      function render() {
        const list = $('list');
        list.textContent = '';
        // Shown, the dotfiles are grouped above everything else rather than
        // scattered through it -- the server's order is kept within each group.
        const items = showHidden
          ? [...entries.filter(isHidden), ...entries.filter((it) => !isHidden(it))]
          : entries.filter((it) => !isHidden(it));
        // Rename and Delete act on the selection, so it may not outlive the
        // row: hiding the dotfiles drops one that has just gone off screen.
        if (selected && !items.includes(selected)) selected = null;
        for (const it of items) {
          const row = document.createElement('div');
          row.className = 'frow' + (it.kind === 'dir' ? ' dir' : '') + (isHidden(it) ? ' hid' : '');
          row.innerHTML = `
            <div class="nm">${iconSvg(it)}<span></span></div>
            <div class="meta">${it.kind === 'dir' ? '' : humanSize(it.size)}</div>
            <div class="meta">${it.mode || ''}</div>
            <div class="meta l">${it.mtime ? new Date(it.mtime * 1000).toLocaleString() : ''}</div>`;
          row.querySelector('.nm span:last-child').textContent = it.name;
          if (it === selected) row.classList.add('sel');
          const open = () => {
            const full = join(cwd, it.name);
            if (it.kind === 'dir') load(full);
            else if (TEXT_EXT.test(it.name) && it.size < 2 * 1024 * 1024) openEditor(full, iconIdFor(it));
            else download(full, it.name);
          };
          row.addEventListener('pointerdown', (e) => { pointer = e.pointerType; });
          row.addEventListener('click', () => {
            const was = row.classList.contains('sel');
            list.querySelectorAll('.frow.sel').forEach((r) => r.classList.remove('sel'));
            row.classList.add('sel');
            selected = it;
            // A finger has no double-click worth waiting for, and asking for
            // one on a 26px row was asking to miss. Tapping the row that is
            // already selected opens it, however long since the tap that
            // selected it -- which still leaves one tap to pick something for
            // the Rename and Delete buttons to act on.
            if (was && pointer !== 'mouse') open();
          });
          row.addEventListener('dblclick', () => { if (pointer === 'mouse') open(); });
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

      onTap(root, async (e) => {
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

      /* The dotfile switch lives in the title bar rather than the toolbar: it
         changes what the window is showing, not what it is about to do. */
      const hideBtn = document.createElement('button');
      hideBtn.type = 'button';
      hideBtn.className = 'win-btn win-btn--icon tip';
      hideBtn.innerHTML = '<svg class="ic-a" aria-hidden="true"><use href="#a-hidden"></use></svg>';
      const markHide = () => {
        const label = showHidden ? 'Hide dotfiles' : 'Show dotfiles';
        hideBtn.classList.toggle('on', showHidden);
        hideBtn.setAttribute('aria-pressed', String(showHidden));
        hideBtn.dataset.tip = label;
        hideBtn.setAttribute('aria-label', label);
      };
      markHide();
      onTap(hideBtn, () => {
        showHidden = !showHidden;
        markHide();
        render();
      });
      entry.tools.appendChild(hideBtn);

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

      onTap(root.querySelector('[data-a="save"]'), async () => {
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

      // xterm takes focus from a mouse on its own. A finger gives it none of
      // the events it is watching for, so the window could be tapped all day
      // without the keyboard ever coming up.
      host.addEventListener('pointerup', (e) => {
        if (e.pointerType !== 'mouse') term.focus();
      });

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

      onTap(btn('check'), async () => {
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

      onTap(btn('apply'), async () => {
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
  onTap(b, (e) => activateApp(app, APPS[app], e.altKey || e.metaKey));
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

onTap(menuBtn(), () => {
  if (menuEl().hidden) openMenu();
  else closeMenu();
});

onTap(menuEl(), (e) => {
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
