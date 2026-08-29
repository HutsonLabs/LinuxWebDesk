'use strict';

/* Canned answers for every network call ui/app.js makes.
 *
 * This loads before app.js and replaces fetch() and WebSocket, so app.js runs
 * exactly as shipped -- same code paths, same error handling -- against data
 * that never touches a real host. Nothing here is reachable from the real
 * build: scripts/preview.py injects it, ui/ never references it.
 *
 * Adding a route: put it in ROUTES below. Anything unmatched falls through to
 * the server, which answers 501 "no mock for ..." so a miss is visible rather
 * than silent.
 */

window.PREVIEW = window.PREVIEW || {};

/* ------------------------------------------------------------- scene state */

const SCENES = {
  'sign-in': { label: 'Sign in', signedIn: false },
  'sign-in-error': { label: 'Sign in — bad password', signedIn: false, loginFails: true },
  desktop: { label: 'Desktop — Files', signedIn: true, open: ['files'] },
  terminal: { label: 'Terminal', signedIn: true, open: ['terminal'] },
  editor: { label: 'Editor', signedIn: true, open: ['editor'] },
  system: { label: 'System — up to date', signedIn: true, open: ['system'] },
  'system-outdated': { label: 'System — update available', signedIn: true, open: ['system'], update: 'behind' },
  'system-updating': { label: 'System — updating', signedIn: true, open: ['system'], update: 'running' },
  'system-failed': { label: 'System — update failed', signedIn: true, open: ['system'], update: 'failed' },
  'system-locked': { label: 'System — not an admin', signedIn: true, open: ['system'], admin: false },
  crowded: { label: 'Several windows', signedIn: true, open: ['files', 'editor', 'terminal', 'system'] },
  autohide: { label: 'Window — auto-hiding title bar', signedIn: true, open: ['terminal'], autohide: true },
  'files-denied': { label: 'Files — permission denied', signedIn: true, open: ['files'], fsError: true },
  'files-empty': { label: 'Files — empty folder', signedIn: true, open: ['files'], emptyDir: true },
  'dialog-rename': { label: 'Dialog — rename', signedIn: true, open: ['files'], dialog: 'rename' },
  'dialog-delete': { label: 'Dialog — delete', signedIn: true, open: ['files'], dialog: 'delete' },
};

const params = new URLSearchParams(location.search);
const saved = params.get('scene') || localStorage.getItem('wd-preview-scene') || 'desktop';
const sceneName = SCENES[saved] ? saved : 'desktop';
const scene = SCENES[sceneName];

PREVIEW.scenes = SCENES;
PREVIEW.sceneName = sceneName;
PREVIEW.scene = scene;
PREVIEW.setScene = (name) => {
  localStorage.setItem('wd-preview-scene', name);
  const url = new URL(location.href);
  url.searchParams.delete('scene');
  location.replace(url.toString());
};

/* --------------------------------------------------------------- fake data */

const USER = {
  username: 'hutson',
  home: '/home/hutson',
  admin: scene.admin !== false,
};

const HOUR = 3600;
const NOW = Math.floor(Date.now() / 1000);

const dir = (name, ago) => ({ name, kind: 'dir', mode: '755', mtime: NOW - ago });
const file = (name, size, ago, mode = '644') => ({
  name, kind: 'file', size, mode, mtime: NOW - ago,
});

/* A tree wide enough to exercise the icon set, the size and mode columns, long
   names, and the sort order the server promises (dirs first, then names). */
const TREE = {
  '/': [dir('boot', 90 * 24 * HOUR), dir('etc', 3 * 24 * HOUR), dir('home', 30 * 24 * HOUR),
        dir('opt', 12 * 24 * HOUR), dir('srv', 60 * 24 * HOUR), dir('usr', 30 * 24 * HOUR),
        dir('var', 2 * HOUR)],
  '/home': [dir('hutson', 2 * HOUR)],
  '/home/hutson': [
    dir('projects', 3 * HOUR), dir('notes', 26 * HOUR), dir('.ssh', 40 * 24 * HOUR),
    dir('Downloads', 5 * HOUR),
    file('.bashrc', 3771, 40 * 24 * HOUR),
    file('.gitignore', 118, 9 * 24 * HOUR),
    file('.vimrc', 2044, 120 * 24 * HOUR),
    file('README.md', 18254, 4 * HOUR),
    file('deploy.sh', 1312, 26 * HOUR, '755'),
    file('inventory.csv', 44210, 2 * 24 * HOUR),
    file('notes.txt', 812, 20 * 60),
    file('screenshot-2026-08-21-at-14.02.11.png', 1_842_004, 47 * HOUR),
    file('server.log', 9_120_733, 60),
    file('webdesk.service', 604, 8 * 24 * HOUR),
  ],
  '/home/hutson/projects': [
    dir('webdesk', 30 * 60),
    file('Cargo.lock', 36398, 30 * 60),
    file('Cargo.toml', 1073, 30 * 60),
    file('build.rs', 3830, 31 * 60),
    file('main.rs', 12723, 30 * 60),
    file('config.toml', 402, 6 * HOUR),
    file('docker-compose.yml', 1180, 3 * 24 * HOUR),
    file('index.html', 2044, 2 * HOUR),
    file('style.css', 6810, 2 * HOUR),
    file('app.js', 29711, 2 * HOUR),
    file('schema.sql', 8801, 5 * 24 * HOUR),
    file('LICENSE', 4633, 200 * 24 * HOUR),
  ],
  '/home/hutson/projects/webdesk': [
    dir('src', 30 * 60), dir('ui', 30 * 60),
    file('.env', 210, 9 * 24 * HOUR, '600'),
    file('Makefile', 1420, 12 * 24 * HOUR),
    file('notes-on-a-very-long-file-name-that-should-truncate-cleanly.md', 3300, HOUR),
  ],
  '/home/hutson/notes': [
    file('meeting-2026-08-20.md', 4210, 3 * 24 * HOUR),
    file('todo.md', 980, 90 * 60),
    file('reading.txt', 2200, 10 * 24 * HOUR),
  ],
  '/home/hutson/Downloads': [
    file('archive.tar.gz', 88_120_442, 8 * HOUR),
    file('report.pdf', 2_910_004, 30 * HOUR),
    file('installer.bin', 44_000_000, 70 * HOUR, '755'),
  ],
  '/home/hutson/.ssh': [
    file('authorized_keys', 1420, 40 * 24 * HOUR, '600'),
    file('id_ed25519', 464, 400 * 24 * HOUR, '600'),
    file('id_ed25519.pub', 96, 400 * 24 * HOUR),
    file('known_hosts', 8820, 2 * 24 * HOUR, '600'),
  ],
  /* Root promises these, so the path bar's completion can walk into them:
     typing "/v" offers /var, and it takes the slash before /var/lib shows. */
  '/var': [
    dir('cache', 6 * HOUR), dir('lib', 90 * 24 * HOUR), dir('log', 2 * HOUR),
    dir('spool', 30 * 24 * HOUR), dir('tmp', HOUR),
  ],
  '/var/lib': [
    dir('docker', 20 * HOUR), dir('systemd', 90 * 24 * HOUR),
    dir('webdesk', 4 * HOUR),
  ],
  '/var/log': [
    file('auth.log', 220_411, 90 * 60),
    file('syslog', 4_120_882, 120),
    file('webdesk.log', 88_204, 300),
  ],
  '/usr': [dir('bin', 30 * 24 * HOUR), dir('lib', 30 * 24 * HOUR),
           dir('local', 12 * 24 * HOUR), dir('share', 30 * 24 * HOUR)],
  '/usr/local': [dir('bin', 4 * HOUR), dir('share', 12 * 24 * HOUR)],
  '/opt': [dir('webdesk', 4 * HOUR)],
  '/srv': [dir('http', 60 * 24 * HOUR)],
  '/boot': [file('config-6.8.0', 220_411, 90 * 24 * HOUR),
            file('vmlinuz', 12_204_882, 90 * 24 * HOUR)],
  '/etc': [
    file('hostname', 12, 90 * 24 * HOUR),
    file('hosts', 220, 90 * 24 * HOUR),
    file('os-release', 402, 90 * 24 * HOUR),
    file('resolv.conf', 108, 4 * HOUR),
  ],
};

const FILES = {
  '/home/hutson/notes.txt':
    'Preview data -- nothing here is on a real disk.\n\n' +
    'The editor opens for anything matching TEXT_EXT under 2 MB; everything\n' +
    'else opens in a new tab against /api/fs/read, which the shim also answers.\n',
  '/home/hutson/README.md':
    '# WebDesk\n\nA desktop in a browser tab, served by one static Rust binary.\n\n' +
    '## Preview\n\n    scripts/preview.py\n\nSaving any file under ui/ reloads the tab.\n\n' +
    '## Notes\n\n- The editor state line tracks loading / modified / saved.\n' +
    '- Saving here is a no-op that reports success, so the saved state is reachable.\n',
  '/home/hutson/projects/webdesk/Makefile':
    'build:\n\tcargo build --release\n\nrun:\n\t./target/release/webdesk\n\npreview:\n\tscripts/preview.py\n',
};

const LONG_TEXT = Array.from(
  { length: 60 },
  (_, i) => `${String(i + 1).padStart(3, ' ')}  line of sample content for the editor to scroll`
).join('\n');

const BUILD = {
  version: '26.8.1',
  commit: '0a6f29f2c41b8ed3a9075c2b6f1e4a8d3b90cc12',
  ref: 'main',
  repo: 'HutsonLabs/WebDesk',
  built: NOW - 4 * HOUR,
};

const UPDATE_LOG = [
  '+ fetching HutsonLabs/WebDesk @ main',
  '+ 7f3a91c  Tighten the dock spacing on narrow viewports',
  '+ cargo build --release',
  '   Compiling webdesk v26.8.1',
  '    Finished `release` profile [optimized] target(s) in 1m 48s',
  '+ installing to /usr/local/bin/webdesk',
  '+ restarting webdesk.service',
].join('\n');

/* ----------------------------------------------------------------- routing */

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });

const text = (body, status = 200) =>
  new Response(body, { status, headers: { 'content-type': 'text/plain' } });

const unauthorized = () => json({ error: 'not signed in' }, 401);

// Mutable across a session: signing in and out inside the preview should
// behave like the real thing without reloading.
let signedIn = scene.signedIn;
let updatePhase = scene.update || 'idle';
let updateTicks = 0;

function fsList(path) {
  if (scene.fsError && path !== USER.home) {
    return json({ error: 'permission denied' }, 403);
  }
  if (scene.emptyDir) {
    return json({ path, parent: '/home', entries: [] });
  }
  const entries = TREE[path];
  if (!entries) return json({ error: `no such directory: ${path}` }, 404);
  const parent = path === '/' ? null : path.slice(0, path.lastIndexOf('/')) || '/';
  return json({ path, parent, entries });
}

function fsRead(path) {
  return text(FILES[path] || `${path}\n\n${LONG_TEXT}\n`);
}

function systemInfo() {
  const supported = true;
  return json({
    build: BUILD,
    hostname: 'orchard',
    user: { username: USER.username, admin: USER.admin },
    updates: {
      allowed: USER.admin && supported,
      supported,
      reason: null,
      admin_groups: ['wheel', 'sudo'],
    },
  });
}

function updateStatus() {
  if (updatePhase === 'running') {
    updateTicks++;
    const phases = ['fetching', 'building', 'installing'];
    // Land on 'ok' after a few polls so the finished state is reachable
    // without waiting on a real build.
    if (updateTicks > 6) updatePhase = 'ok';
    return json({
      status: { state: 'running', phase: phases[Math.min(updateTicks - 1, 2)] },
      log: UPDATE_LOG.split('\n').slice(0, 2 + updateTicks).join('\n'),
      build: BUILD,
    });
  }
  if (updatePhase === 'failed') {
    return json({
      status: { state: 'failed', error: 'cargo build exited 101' },
      log: UPDATE_LOG.split('\n').slice(0, 4).join('\n') +
        '\nerror[E0308]: mismatched types\n  --> src/pty.rs:88:21\nerror: could not compile `webdesk`',
      build: BUILD,
    });
  }
  if (updatePhase === 'ok') {
    return json({ status: { state: 'ok' }, log: UPDATE_LOG, build: BUILD });
  }
  return json({ status: {}, log: '', build: BUILD });
}

function updateCheck() {
  if (updatePhase === 'behind' || scene.update === 'behind') {
    return json({
      comparable: true,
      behind: true,
      current: BUILD.commit,
      latest: '7f3a91cd42e8b0175a3c9f6021de4b8a7c135ee9',
      ref: 'main',
      message: 'Tighten the dock spacing on narrow viewports',
      date: new Date(Date.now() - 40 * 60 * 1000).toISOString(),
    });
  }
  return json({
    comparable: true,
    behind: false,
    current: BUILD.commit,
    latest: BUILD.commit,
    ref: 'main',
  });
}

/* ----------------------------------------------------------- container apps */

/* A representative slice of the catalog rather than all of it: one Selkies
   desktop, the editor, and the terminal -- which between them use every kind of
   blank the install form can draw (text, secret, toggle, path). Keeping a
   second full copy of src/catalog.rs in step would be a chore with no payoff.

   Which entries appear is still chosen here, but what they say is not: every
   field below that src/catalog.rs also names is overwritten from it at load
   (see "keeping up with the source"), so the strings in this literal are a
   fallback for when the Rust cannot be read, not a second opinion. */

/* Desktop entries ask nothing: the title is the app's own name, the clock is
   the host's, the identity is the installer's. Empty on purpose -- it is what
   makes Install a single press with no dialog worth showing. */
const DESKTOP_PARAMS = [];

const DESKTOP_NOTE =
  'A desktop application, drawn in the browser. Its state lives in the app ' +
  'directory, so it is still there next time.';

const APP_CATALOG = [
  {
    slug: 'firefox', name: 'Firefox', icon: 'a-firefox',
    tagline: 'The browser, running on this host rather than on your machine.',
    image: 'lscr.io/linuxserver/firefox', notes: DESKTOP_NOTE, params: DESKTOP_PARAMS,
  },
  {
    slug: 'inkscape', name: 'Inkscape', icon: 'a-inkscape',
    tagline: 'Vector drawing, for the SVGs this desktop is drawn with.',
    image: 'lscr.io/linuxserver/inkscape', notes: DESKTOP_NOTE, params: DESKTOP_PARAMS,
  },
  {
    slug: 'vscodium-web', name: 'VSCodium', icon: 'a-vscodium',
    tagline: 'VS Code without the telemetry, as a web editor rather than a drawn desktop.',
    image: 'lscr.io/linuxserver/vscodium-web',
    notes: 'Its extensions and settings live in the app directory.',
    params: [
      { key: 'DEFAULT_WORKSPACE', label: 'Workspace folder', kind: 'path', default: '', required: false,
        help: 'Directory on the host to open. Mounted into the editor.' },
      { key: 'CONNECTION_TOKEN', label: 'Connection token', kind: 'secret', default: '', required: false,
        help: 'Optional. A secret the editor asks for; leave empty to run without one.' },
      { key: 'SUDO_PASSWORD', label: 'sudo password', kind: 'secret', default: '', required: false,
        help: 'Optional. Lets the editor’s terminal use sudo inside the container only.' },
    ],
  },
  {
    slug: 'term-hut-host', name: 'term.hut', icon: 'a-termhut',
    tagline: 'An agent-aware terminal, run as a service on this host -- so its shell is the host\'s.',
    image: '',
    host: { unit: 'term-hut-web.service' },
    notes: 'Runs on the host rather than in a container, which is the point: the shell it ' +
           'hands out is a shell on this machine, with its packages, its services and its ' +
           'files. Everyone who can sign in to WebDesk can open it, so it is worth being ' +
           'sure that is the same set of people you would give an SSH account. Installing ' +
           'fetches the term.hut Flatpak and writes a system unit that runs it as you, ' +
           'bound to loopback. A unit already on this host is adopted untouched instead.',
    /* No params, and that is the entry rather than an omission: everything a
       host service is told lives in its unit file. */
    params: [],
  },
];

/* Starts with one running and one stopped, so the dock, the running and
   stopped rows, and the "not running" frame are all reachable without
   installing anything first. */
let APPS_INSTALLED = scene.apps === 'none' ? [] : [
  {
    slug: 'term-hut-host', name: 'term.hut', icon: 'a-termhut', state: 'running',
    tagline: 'An agent-aware terminal, run as a service on this host -- so its shell is the host\'s.',
    // A host service, so: no image, and a unit where a container name would be.
    image: '', url: '/app/term-hut-host/', unit: 'term-hut-web.service',
    installed: NOW - 4 * HOUR, actor: 'hutson', env: {},
    secrets: [], mounts: [], notes: '',
  },
  {
    slug: 'firefox', name: 'Firefox', icon: 'a-firefox', state: 'exited',
    tagline: 'The browser, running on this host rather than on your machine.',
    image: 'lscr.io/linuxserver/firefox:latest', url: '/app/firefox/',
    // TZ and TITLE are set by the installer from the host, not asked for, so
    // this is what an installed desktop app's settings actually look like.
    installed: NOW - 26 * HOUR, actor: 'hutson',
    env: { TZ: 'America/Chicago', TITLE: 'firefox', PUID: '1000', PGID: '1000' },
    secrets: [], mounts: [], notes: '',
  },
];

/* ------------------------------------------------- keeping up with the source */

/* The two lists above are a slice of src/catalog.rs, and a slice drifts. An
   entry gets a new icon or a new name in the Rust, this copy keeps the old
   one, and the preview draws something the app itself never draws -- a bug in
   nothing but the preview, wearing the costume of a bug in the UI. That is how
   term.hut came to sit in the dock under the terminal's icon.
   scripts/preview.py reads the real entries out of the Rust and leaves them in
   window.PREVIEW_CATALOG, which is what lets that be fixed here instead of
   noticed months later.

   Corrected rather than merely reported: a preview that knows it is drawing
   the wrong icon and draws it anyway is worth less than one that draws the
   right one. What cannot be corrected -- a slug the catalog no longer has, an
   icon no sprite defines -- is said out loud, in the same spirit as the
   server's 501 for a route nobody mocked.

   Params stay hand-written. They are the part preview.py does not read, and
   the part this file exists to have opinions about: which blanks the install
   form has to draw. A missing one shows up as a form with nothing on it. */

const PREVIEW_DRIFT = [];

function reconcile(entries, fields, where) {
  const truth = window.PREVIEW_CATALOG || {};
  for (const entry of entries) {
    const real = truth[entry.slug];
    if (!real) {
      PREVIEW_DRIFT.push(`${where} "${entry.slug}" is no longer in src/catalog.rs`);
      continue;
    }
    for (const field of fields) {
      if (!(field in real) || real[field] === entry[field]) continue;
      PREVIEW_DRIFT.push(
        `${where} "${entry.slug}" ${field}: had ${JSON.stringify(entry[field])}, ` +
        `catalog.rs says ${JSON.stringify(real[field])} — corrected`);
      entry[field] = real[field];
    }
  }
}

/* An installed app keeps its own image (the catalog's reference plus the tag
   it was installed at) and its own notes (empty -- the install form's prose is
   not what the settings pane shows), so only the fields that identify the
   application are taken from source. */
if (window.PREVIEW_CATALOG && Object.keys(window.PREVIEW_CATALOG).length) {
  reconcile(APP_CATALOG, ['name', 'tagline', 'image', 'icon', 'notes'], 'catalog');
  reconcile(APPS_INSTALLED, ['name', 'tagline', 'icon'], 'installed');
} else {
  PREVIEW_DRIFT.push(
    'src/catalog.rs could not be read, so nothing here was checked against it ' +
    '— the app entries are whatever this file last said they were');
}

/* The icons are checked against the sprite rather than against the catalog:
   catalog.rs naming a symbol ui/ui-icons.svg does not define is a real bug,
   but it is the release's bug and its own test catches it (see
   every_icon_the_catalog_names_is_in_the_sprite in src/apps.rs). What this
   catches is the preview's own version -- a scene or an installed entry
   pointing at a symbol that was renamed out from under it, which draws as an
   empty square and looks like a CSS problem. */
function checkIcons() {
  const wanted = new Set(
    [...APP_CATALOG, ...APPS_INSTALLED].map((a) => a.icon).filter(Boolean));
  return fetch('/ui-icons.svg', { credentials: 'same-origin' })
    .then((r) => (r.ok ? r.text() : ''))
    .then((svg) => {
      if (!svg) return;
      const have = new Set(
        [...svg.matchAll(/<symbol[^>]+id="([^"]+)"/g)].map((m) => m[1]));
      for (const icon of wanted) {
        if (!have.has(icon)) {
          PREVIEW_DRIFT.push(`icon "${icon}" is not a symbol in ui/ui-icons.svg`);
        }
      }
    })
    .catch(() => {});
}

/* After load, so it lands under the banner devtools.js prints rather than
   above it, and so a drift report is the last thing in the console. */
window.addEventListener('load', () => {
  checkIcons().then(() => {
    if (!PREVIEW_DRIFT.length) return;
    console.groupCollapsed(
      `%cWebDesk preview%c  ${PREVIEW_DRIFT.length} drifted from source`,
      'background:#3fb6c8;color:#0f1319;padding:1px 5px;border-radius:3px',
      'color:#f0a05a');
    for (const line of PREVIEW_DRIFT) console.warn(line);
    console.info('Fix these in scripts/preview/mock.js — src/catalog.rs is the source.');
    console.groupEnd();
  });
});

const APPS_ENGINE = scene.engine === 'missing'
  ? { name: null, error: 'no container engine found on this host', ready: false }
  : { name: 'docker 27.1.1', error: null, ready: true };

const PULL_LOG = [
  '$ docker pull lscr.io/linuxserver/SLUG:latest',
  'latest: Pulling from linuxserver/SLUG',
  '1f7ce2fa46ab: Pull complete',
  '9d3e1a7c0b21: Pull complete',
  'a04f8c2e5d13: Downloading [==============>        ]  18.2MB/31.4MB',
  'a04f8c2e5d13: Pull complete',
  'Digest: sha256:6b1c4f0e9a7d3852be10c4f9a2d7e5b3c8f0a91d4e6b27c5083fa1d9e4c7b206',
  'Status: Downloaded newer image for lscr.io/linuxserver/SLUG:latest',
  '$ docker run -d --name webdesk-SLUG ...',
  'c3f9a1e7b204d85fa0c6e19b7d3428f5019ace6b7d24f80915ca3e6b7089d1f4',
];

let installState = { state: 'idle' };
let installTicks = 0;

function appsStatus() {
  if (installState.state !== 'running') return json({ status: installState, log: '' });

  installTicks++;
  const slug = installState.slug;
  const lines = PULL_LOG.map((l) => l.replaceAll('SLUG', slug));

  if (installTicks > 6) {
    // Land on a finished state so the installed row, the toast and the new
    // dock icon are all reachable without a registry.
    if (scene.install === 'fails') {
      installState = {
        state: 'failed', slug, name: installState.name,
        error: 'docker run failed (exit status: 125)',
      };
      return json({ status: installState, log: lines.slice(0, 8).join('\n') +
        '\ndocker: Error response from daemon: driver failed programming external connectivity' });
    }
    const entry = APP_CATALOG.find((a) => a.slug === slug);
    APPS_INSTALLED = [...APPS_INSTALLED, {
      slug, name: entry.name, icon: entry.icon, tagline: entry.tagline, notes: entry.notes,
      image: `${entry.image}:latest`, url: `/app/${slug}/`, state: 'running',
      installed: NOW, actor: USER.username, env: {}, secrets: [], mounts: [],
    }];
    installState = { state: 'done', slug, name: entry.name };
    return json({ status: installState, log: lines.join('\n') });
  }
  return json({
    status: {
      state: 'running',
      phase: installTicks > 4 ? 'creating' : 'pulling',
      slug,
      name: installState.name,
    },
    log: lines.slice(0, 2 + installTicks).join('\n'),
  });
}

const ROUTES = [
  ['GET', /^\/api\/me$/, () => (signedIn ? json(USER) : unauthorized())],
  ['POST', /^\/api\/login$/, (_m, _q, body) => {
    if (scene.loginFails || (body && body.password === 'wrong')) {
      return json({ error: 'invalid username or password' }, 401);
    }
    signedIn = true;
    return json({ ...USER, username: (body && body.username) || USER.username });
  }],
  ['POST', /^\/api\/logout$/, () => { signedIn = false; return json({ ok: true }); }],

  ['GET', /^\/api\/fs\/list$/, (_m, q) => fsList(q.get('path') || USER.home)],
  ['GET', /^\/api\/fs\/read$/, (_m, q) => fsRead(q.get('path') || '')],
  ['PUT', /^\/api\/fs\/write$/, () => json({ ok: true })],
  ['POST', /^\/api\/fs\/mkdir$/, () => json({ ok: true })],
  ['POST', /^\/api\/fs\/rename$/, () => json({ ok: true })],
  ['POST', /^\/api\/fs\/remove$/, () => json({ ok: true })],

  ['GET', /^\/api\/system\/info$/, () => (signedIn ? systemInfo() : unauthorized())],
  ['GET', /^\/api\/update\/status$/, () => (signedIn ? updateStatus() : unauthorized())],
  ['POST', /^\/api\/update\/check$/, () => updateCheck()],
  ['POST', /^\/api\/update\/apply$/, () => {
    updatePhase = 'running';
    updateTicks = 0;
    return json({ ok: true });
  }],

  ['GET', /^\/api\/apps\/catalog$/, () => (signedIn ? json({
    apps: APP_CATALOG,
    engine: APPS_ENGINE,
    allowed: USER.admin && APPS_ENGINE.ready,
    admin: USER.admin,
    admin_groups: ['wheel', 'sudo'],
  }) : unauthorized())],
  ['GET', /^\/api\/apps\/list$/, () => (signedIn ? json({
    apps: APPS_INSTALLED, admin: USER.admin, engine: APPS_ENGINE.name,
  }) : unauthorized())],
  ['GET', /^\/api\/apps\/status$/, () => (signedIn ? appsStatus() : unauthorized())],
  ['POST', /^\/api\/apps\/install$/, (_m, _q, body) => {
    const entry = APP_CATALOG.find((a) => a.slug === (body && body.slug));
    if (!entry) return json({ error: 'not in the catalog' }, 404);
    installState = { state: 'running', phase: 'pulling', slug: entry.slug, name: entry.name };
    installTicks = 0;
    return json({ ok: true, started: true, slug: entry.slug });
  }],
  ['POST', /^\/api\/apps\/start$/, (_m, _q, body) => {
    APPS_INSTALLED = APPS_INSTALLED.map(
      (a) => (a.slug === body.slug ? { ...a, state: 'running' } : a));
    return json({ ok: true });
  }],
  ['POST', /^\/api\/apps\/stop$/, (_m, _q, body) => {
    APPS_INSTALLED = APPS_INSTALLED.map(
      (a) => (a.slug === body.slug ? { ...a, state: 'exited' } : a));
    return json({ ok: true });
  }],
  ['POST', /^\/api\/apps\/remove$/, (_m, _q, body) => {
    APPS_INSTALLED = APPS_INSTALLED.filter((a) => a.slug !== body.slug);
    return json({ ok: true, purged: !!(body && body.purge) });
  }],
];

/* Writes are accepted and reported as successful because the point is to reach
   the "saved" and "renamed" states in the UI, not to keep a filesystem. The
   listing is static, so a rename shows the original name again on refresh. */

const realFetch = window.fetch.bind(window);

window.fetch = async function (input, init = {}) {
  const url = new URL(typeof input === 'string' ? input : input.url, location.origin);
  const method = (init.method || (input && input.method) || 'GET').toUpperCase();

  if (!url.pathname.startsWith('/api/')) return realFetch(input, init);

  let body = null;
  if (init.body && typeof init.body === 'string') {
    try { body = JSON.parse(init.body); } catch (_) { /* not json; fine */ }
  }

  for (const [verb, pattern, handler] of ROUTES) {
    if (verb === method && pattern.test(url.pathname)) {
      // A little latency, so spinners and disabled states are actually visible.
      await new Promise((r) => setTimeout(r, PREVIEW.latency ?? 90));
      const res = handler(url.pathname, url.searchParams, body);
      PREVIEW.log?.(`${method} ${url.pathname}${url.search} -> ${res.status}`);
      return res;
    }
  }

  PREVIEW.log?.(`${method} ${url.pathname} -> unmocked`);
  return json({ error: `no mock for ${method} ${url.pathname}` }, 501);
};

/* --------------------------------------------------------------- terminal */

/* A shell that only knows how to look like one. Enough to show the prompt,
   wrapping, colour and the cursor at a real font size -- which is all the
   terminal contributes visually. */

const MOTD = [
  '\x1b[38;5;80mWebDesk preview\x1b[0m -- this terminal is a stub, no shell behind it.',
  'Try: ls, pwd, whoami, uname -a, neofetch, clear, help',
  '',
].join('\r\n');

const PROMPT = '\x1b[38;5;80mhutson\x1b[0m@\x1b[38;5;110morchard\x1b[0m:\x1b[38;5;180m~\x1b[0m$ ';

const FAKE_LS = [
  '\x1b[38;5;110mDownloads\x1b[0m  \x1b[38;5;110mnotes\x1b[0m      \x1b[38;5;110mprojects\x1b[0m',
  'README.md  \x1b[38;5;114mdeploy.sh\x1b[0m  inventory.csv  notes.txt  server.log',
].join('\r\n');

const COMMANDS = {
  ls: FAKE_LS,
  'ls -la': FAKE_LS,
  pwd: '/home/hutson',
  whoami: 'hutson',
  hostname: 'orchard',
  date: () => new Date().toString(),
  'uname -a': 'Linux orchard 6.9.7-200.fc40.x86_64 #1 SMP x86_64 GNU/Linux',
  uptime: ' 14:02:11 up 6 days,  3:41,  1 user,  load average: 0.08, 0.12, 0.09',
  help: 'Stub commands: ls, pwd, whoami, hostname, date, uname -a, uptime, neofetch, clear',
  neofetch: [
    '\x1b[38;5;80m      ___      \x1b[0m  hutson@orchard',
    '\x1b[38;5;80m     (o o)     \x1b[0m  ---------------',
    '\x1b[38;5;80m    (  V  )    \x1b[0m  OS: Fedora 40 x86_64',
    '\x1b[38;5;80m   /--m-m--/   \x1b[0m  Shell: bash 5.2.26',
    '\x1b[38;5;80m               \x1b[0m  WebDesk: 26.8.1 (preview)',
  ].join('\r\n'),
};

const RealWebSocket = window.WebSocket;

class MockSocket {
  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.binaryType = 'blob';
    this.onopen = this.onmessage = this.onclose = this.onerror = null;
    this._line = '';
    setTimeout(() => {
      this.readyState = 1;
      this.onopen && this.onopen({});
      this._write(MOTD + '\r\n' + PROMPT);
    }, 120);
  }

  _write(s) {
    this.onmessage && this.onmessage({ data: s });
  }

  send(data) {
    // app.js sends resize frames as JSON strings and keystrokes as bytes.
    if (typeof data === 'string' && data.startsWith('{')) return;
    const s = typeof data === 'string' ? data : new TextDecoder().decode(data);
    for (const ch of s) this._key(ch);
  }

  _key(ch) {
    if (ch === '\r') {
      const cmd = this._line.trim();
      this._line = '';
      this._write('\r\n');
      if (cmd === 'clear') {
        this._write('\x1b[2J\x1b[H');
      } else if (cmd) {
        const out = COMMANDS[cmd];
        const body = typeof out === 'function' ? out() : out;
        this._write(
          (body !== undefined ? body : `\x1b[31m${cmd.split(' ')[0]}: command not found\x1b[0m`) + '\r\n'
        );
      }
      this._write(PROMPT);
      return;
    }
    if (ch === '\x7f') {
      if (this._line) {
        this._line = this._line.slice(0, -1);
        this._write('\b \b');
      }
      return;
    }
    if (ch === '\x03') { // ctrl-c
      this._line = '';
      this._write('^C\r\n' + PROMPT);
      return;
    }
    if (ch < ' ') return;
    this._line += ch;
    this._write(ch);
  }

  close() {
    this.readyState = 3;
    this.onclose && this.onclose({});
  }

  addEventListener(type, fn) { this['on' + type] = fn; }
  removeEventListener(type) { this['on' + type] = null; }
}

window.WebSocket = function (url, protocols) {
  if (String(url).includes('/ws/term')) return new MockSocket(url);
  return new RealWebSocket(url, protocols);
};
window.WebSocket.prototype = MockSocket.prototype;
