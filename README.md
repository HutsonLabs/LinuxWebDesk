# WebDesk

A web desktop for Linux servers. Sign in with your system account, get a file
manager and a real terminal in the browser. One binary, no runtime, no build
step, no npm.

Targets Debian/Ubuntu/Mint, RHEL/Fedora/Rocky, and Arch, on x86_64 and
aarch64.

## Install

On the target host:

```sh
curl -fsSL https://raw.githubusercontent.com/HutsonLabs/WebDesk/main/bootstrap.sh | sudo sh
```

That fetches the source, installs build dependencies, builds, installs a systemd
unit and a PAM service file, opens the firewall port, and starts the service.
Then open **http://<host>:6767** and sign in with any normal account on that box.

The first build takes a few minutes; it is compiling Rust with fat LTO on the
host. Nothing is left running that was not asked for, and everything it writes
is listed under [What gets installed](#what-gets-installed).

Knobs, all optional:

```sh
curl -fsSL .../bootstrap.sh | sudo PORT=9000 WD_REF=v0.2.0 sh
```

| | |
| --- | --- |
| `PORT` | listen port (default 6767) |
| `PREFIX` | where the binary goes (default `/usr/local/bin`) |
| `WD_REF` | branch, tag or commit to install (default `main`) |
| `WD_REPO` | source repository, for a fork |
| `WD_ADMIN_GROUPS` | who may update from the browser (default `wheel,sudo`) |
| `WD_UPDATE=off` | build without the update capability at all |

### From a checkout instead

```sh
./deploy.sh 10.1.2.40          # or user@host
```

rsyncs the working tree to the host and runs `install.sh` there, which is
useful for testing a change you have not pushed. Or, already on the target:

```sh
sudo bash install.sh
sudo systemctl status webdesk
journalctl -u webdesk -f
```

Every one of these is also an upgrade path, and re-running any of them is safe.

### Coming from `linuxwebdesk`

This project was called `linuxwebdesk` until August 2026, and before that
`rockywebde`. **There is no in-place upgrade from either.** Everything moved at
once -- the binary, the systemd unit, the PAM service, `/etc/`, `/var/lib/`,
`/usr/local/src/`, the release asset names, the `WD_*` environment prefix and
the session cookie -- so an old install and a new one share no paths and would
simply coexist, with the old unit still holding the port.

Remove the old install first, then install as above:

```sh
sudo systemctl disable --now linuxwebdesk
sudo rm -f /etc/systemd/system/linuxwebdesk.service /etc/pam.d/linuxwebdesk \
           /usr/local/bin/linuxwebdesk /usr/local/bin/linuxwebdesk-update \
           /usr/local/libexec/linuxwebdesk-update
sudo rm -rf /usr/local/src/linuxwebdesk /etc/linuxwebdesk /var/lib/linuxwebdesk
sudo systemctl daemon-reload
```

Settings are not carried over, so pass `PORT` and any other knobs again if the
old install used something other than the defaults.

## Updating

Click the account button in the upper left of the desktop — the one whose
tooltip is your `you@host` — and take the top row of the menu, which says the
same name, to open **System**. It shows the running build and, for a member of
`wheel` or `sudo`, checks the tracked ref for a newer commit and updates on a
button, streaming the build log into the window as it goes. Everyone else sees
the build information and a note saying why the update controls are not theirs
to use.

It opens as a single window: taking that row again raises the one already open
rather than stacking another copy. The second row of the menu signs you out.

The same thing from a shell, doing exactly the same work:

```sh
sudo webdesk-update
```

An update fetches the source for the tracked ref, rebuilds it on the host,
reinstalls, and restarts the service. Worth knowing before pressing it:

- **Usually it is quick.** CI publishes a binary for each architecture and libc
  family on every push to `main`, and the updater installs that when it matches
  the commit being installed — a download rather than a build. It falls back to
  compiling on the host when there is no matching artifact, which takes a few
  minutes. See [Release binaries](#release-binaries).
- **Everyone gets signed out.** Sessions live in memory and the service
  restarts. Open terminals end with them.
- **A failed build changes nothing.** The new binary is only installed after it
  compiles, so a broken commit leaves the running version untouched. The log
  stays in the System window and in `journalctl -u webdesk-update`.
- **Settings survive.** Port, prefix and tracked ref are recorded in
  `/etc/webdesk/install.conf` at install time and read back on update, so a
  host installed on port 9000 comes back on port 9000.
- **The installer comes from the tracked ref**, not from the copy the last
  update left on disk, so a change to how installing works takes effect on the
  next update rather than the one after it. If it cannot be fetched, the copy on
  disk is used instead, which still installs the source already there.

To follow something other than `main`, edit `WD_REF` in that file and restart.
To remove the capability from a host entirely, set `WD_UPDATE=off` there and in
the unit — the endpoints then refuse everyone, including admins.

## Release binaries

Every push to `main` is built by GitHub Actions for four targets — `x86_64` and
`aarch64`, each against the Debian and RHEL families — and published twice: to a
numbered, immutable release of its own, and to a rolling `latest-main`
prerelease that carries the same assets. Both get `SHA256SUMS`, a
`manifest.json` naming the commit and the version, and a signed build-provenance
attestation. Hosts tracking a branch install from `latest-main`; the numbered
release is the permanent record of what that version was.

### Version numbers

Releases are numbered `YY.MM.Build` — two-digit year, month with no leading
zero, then a counter that restarts each month. `26.8.1` is the first release of
August 2026; the next is `26.8.2`, and the first of September is `26.9.1`.

The counter is assigned by CI, not stored in the tree: it is derived from the
highest `v26.8.*` tag already published. That is deliberate. A number kept in a
file would have to be committed, so two commits landing close together would
race to claim the same one, and the commit that bumped it would itself trigger
another build. Deriving it from the tag list means the tags *are* the ledger.

It reads the highest number rather than counting the tags, so deleting a release
leaves a gap instead of handing a used number to a different commit.

`version` in `Cargo.toml` is only a floor. It is what a build reports when no CI
number was stamped into it — a working copy, or a host that had to compile from
source because no release matched its commit. A binary installed from a release
reports the real number, and `bootstrap.sh` records it in `.wd-source` so a
later rebuild of that same tree still reports it.

Pushing a `v*` tag by hand still works and takes the tag's own number verbatim,
which is the escape hatch for cutting a release out of band. Tags that CI
creates do not re-trigger the workflow: GitHub suppresses workflow triggers for
refs pushed with the default `GITHUB_TOKEN`, which is the only reason the `v*`
trigger is not a loop.

Arch has no artifact of its own and does not need one: glibc is backward
compatible and Arch's is newer than either build base, so it takes the RHEL
binary, which is built against the older of the two.

`latest-main` is only ever used to *find* a build. The manifest names the
numbered release it came from, and the binary and `SHA256SUMS` are then fetched
from that release instead, which is published once and never rewritten — so no
cache in front of the rolling pointer can hand a host a previous build's bytes.
The manifest fetch itself is cache-busted for the same reason.

Before installing one, `bootstrap.sh`:

1. refuses unless `manifest.json` names **exactly** the commit it was about to
   build — a release for any other commit is ignored rather than installed;
2. verifies `SHA256SUMS`, using `sha256sum` or `openssl`, and declines if
   neither is available rather than installing something it could not check;
3. verifies the provenance attestation with `gh attestation verify` when `gh`
   is installed **and new enough to have that command** (2.49+). A `gh` that
   checks and says no is always fatal; one too old to check is not, because it
   is declining to have an opinion rather than accusing the binary, and is
   treated as if `gh` were absent.

Any of these failing costs a compile, not an install. The knobs:

| | |
| --- | --- |
| `WD_PREBUILT=off` | never use a release binary; always compile on the host |
| `WD_REQUIRE_ATTESTATION=1` | refuse to install unless provenance is verified — implies `gh` 2.49+ must be present |
| `WD_RELEASE_TAG=tag` | take the binary from a specific release |

Be clear about what the checksum does and does not buy you: it comes from the
same origin as the binary, so it catches corruption, not a compromised release.
The attestation is the part that establishes where the binary came from, and it
is only checked if `gh` is on the host. Verify one by hand with:

```sh
gh attestation verify webdesk-x86_64-rhel --repo HutsonLabs/WebDesk
```

## What gets installed

```
/usr/local/bin/webdesk              the binary
/usr/local/bin/webdesk-update       symlink to the updater
/usr/local/libexec/webdesk-update   the updater
/usr/local/src/webdesk/             source, kept for incremental rebuilds
/etc/webdesk/install.conf           settings, so updates preserve them
/etc/pam.d/webdesk                  PAM service
/etc/systemd/system/webdesk.service the unit
/var/lib/webdesk/                   update log and status (root, 0700)
/var/lib/webdesk/apps.json          which container apps are installed
/var/lib/webdesk/appdata/<slug>/    one app's /config, owned by its installer
```

Container apps add to this only once something is installed, and the images
themselves live wherever the container engine keeps them — not here. Removing
an app deletes its container; its `appdata` directory is kept unless the
removal says otherwise.

`/usr/local/src/webdesk/target` is a Rust build directory and is the large
one. Deleting it costs nothing but a cold build next update.

## What it does

- **Files** — browse, open, edit and save text files, upload, download, create
  folders, rename, delete. File types are shown with
  [Catppuccin Icons](https://github.com/catppuccin/vscode-icons) (MIT, see
  `ui/icons.LICENSE`), vendored as a single sprite.
- **Terminal** — a real login shell via `su - <user>`, xterm.js in the browser,
  resize-aware.
- **Windows** — draggable and resizable panes with focus, minimize and a dock.
  Not a compositor; they are positioned divs, which is all a shell like this
  needs.
- **Dock** — a floating frosted bar rather than a panel. Windows grow out of
  their icon and shrink back into it, and an icon carries a dot while its app
  has a window open, minimised or not. Clicking an icon raises what is already
  there — the minimised window first, otherwise the next one, so a second click
  on Terminal walks through the terminals; alt- or middle-click opens another.
  Open editors get a dock item each, drawn with the file's own icon. The dock
  is applications and open windows and nothing else.
- **Apps** — a short catalog of container applications. Pick one, answer its
  questions, and it is pulled, started and given a dock icon of its own; it
  opens in a window like everything else. See [Container apps](#container-apps).
- **Account** — one button in the upper left, carrying no text: the username is
  its tooltip. It drops a two-row menu — the username again, which opens
  System, and Sign out.
- **No browser dialogs** — nothing calls `prompt()`, `confirm()` or `alert()`.
  Questions are asked in an in-page modal, complaints arrive as a toast above
  the dock, and a file the editor will not take is downloaded rather than
  opened in a tab of its own. Every button names itself in a styled tooltip;
  there is not a `title` attribute in the UI.

## Container apps

WebDesk can install a small, fixed set of applications as containers and show
each one in a window. Open **Apps** from the dock, choose something from
*Available*, fill in its blanks, and press Install. The image is pulled — the
log streams into the window as it goes — the container is created, and the app
appears in the dock with its own icon. Clicking it opens a window.

It needs a container engine on the host. Docker is what this was written
against and what is assumed. Podman is accepted too, because every command used
here takes the same arguments in both, but **it is untested**; set
`WD_CONTAINER_ENGINE=docker` or `=podman` to override the guess.

### What is in it

| | | |
| --- | --- | --- |
| **Firefox** | `linuxserver/firefox` | the browser, running on this host |
| **Helium** | `linuxserver/helium` | a quieter Chromium |
| **OnlyOffice** | `linuxserver/onlyoffice` | documents, spreadsheets, slides |
| **Inkscape** | `linuxserver/inkscape` | vector drawing |
| **IntelliJ IDEA** | `linuxserver/intellij-idea` | the JetBrains IDE |
| **VSCodium** | `linuxserver/vscodium-web` | VS Code without the telemetry |
| **term.hut** | `ghcr.io/hutsonlabs/term.hut` | an agent-aware terminal |

The first five are **desktop applications, not web apps** — real GTK and Java
programs running headless on the host, drawn into the browser by
[Selkies](https://github.com/selkies-project). That is the same "stream the
pixels in" trade `docs/architecture.html` describes, arrived at by installing a
container rather than by building a streaming stack. It is why they want
[more shared memory](#what-you-choose-and-what-webdesk-chooses) than a server
process does, and why they feel like a remote desktop rather than a web page.

The last two are ordinary web servers: VSCodium serves an editor, term.hut
serves a terminal.

### What you choose, and what WebDesk chooses

You answer the questions the catalog entry asks — a password, a timezone, a
folder to serve. WebDesk decides everything it depends on to keep working, and
none of it is reachable from the browser:

| | |
| --- | --- |
| Container name | `webdesk-<slug>` |
| Published port | assigned from 47000–47999, **bound to `127.0.0.1`** |
| State directory | `/var/lib/webdesk/appdata/<slug>`, owned by whoever installed it, mounted where the entry says (`/config`, or `/home/hut` for term.hut) |
| `PUID` / `PGID` | the installing user's — but only for images that read them |
| `--shm-size` | `1g` for the desktop applications; a browser or IDE dies on the 64 MB default |
| Base path | set for the two apps that must be told: `CODE_ARGS=--server-base-path=…`, `HUT_BASE_PATH=…` |
| Restart policy | `unless-stopped` |
| Image tag | `latest`, or `develop` — not free text |

`--shm-size` is a tmpfs size and nothing more. **No entry loosens the sandbox**:
nothing here emits `--security-opt`, `--privileged`, `--cap-add`, or host
networking, and there is a test that fails if one ever does.

A folder you name is mounted where the catalog entry says, read-only where that
makes sense, and it is checked first: it must be an absolute path to a real
directory, and it may not be `/` or anything under `/etc`, `/proc`, `/sys`,
`/dev`, `/boot`, `/run`, `/usr`, `/bin`, `/sbin`, `/lib` — or under WebDesk's
own state directory, since an app that could write there could rewrite the list
of what gets run. The check resolves symlinks before testing, so a link into
`/etc` is refused as if it had been typed.

On a host with SELinux, mounts are relabelled: `Z` for `/config`, which is ours
alone, and the shared `z` for a directory you named and may still want to reach
yourself.

### Why every app is on this origin

Containers publish on loopback only. The single route to one is
`/app/<slug>/` on WebDesk's own port, which is a reverse proxy that refuses
anyone without a session. Three things follow, and they are the reason it is
built this way:

- **An app is never exposed to the network.** Not even one with no login of its
  own — reaching it means getting past WebDesk first.
- **The iframe works, and the app arrives signed in.** Same origin, so the
  browser raises no objection and `X-Frame-Options` from the app is dropped:
  it is advice about a page we are serving, not a claim about a site we do not
  control.
- **There is still one port to open.** The firewall story is unchanged: 6767.

Two rules keep an app inside its own prefix. The session cookie is stripped
from every request before the app sees it, and cookies coming back are pinned
to `/app/<slug>/` — with any attempt to set `wd_session` dropped outright.
Redirects are moved back under the prefix, and `frame-ancestors` is removed
from any policy the app sends.

### Why the catalog is fixed

Creating a container chooses what code the engine runs, and the engine runs as
root. That is not something to hand to a browser, so the catalog lives in the
binary and is reviewed like any other code. **You cannot define your own
container yet**, by design.

Most entries are [LinuxServer.io](https://www.linuxserver.io/) images from
`lscr.io`, which share one contract — `/config`, `PUID`/`PGID`, `TZ` — so the
installer has one shape to implement rather than one per application. `term.hut`
is the exception: it runs as its own fixed user, keeps state in `/home/hut`, and
would ignore `PUID`/`PGID`. Entries say which they are rather than the installer
assuming.

**The requirement that decides membership** is that an entry must work when
served from `/app/<slug>/` instead of `/`. An application that assumes it owns
the root emits links that escape its prefix and renders as a blank frame. There
are two ways to satisfy it:

- **It works it out itself.** The Selkies desktop images derive everything from
  `location.pathname` — assets as `./assets/…`, their socket as
  `<base>websockets`. Nothing to configure.
- **It is told.** VSCodium needs `--server-base-path` or its assets come out
  rooted at `/stable-<hash>/…`; term.hut takes `HUT_BASE_PATH`. The entry names
  the variable and the shape to put the prefix in.

Check this first before adding anything, and check it by running the image
rather than by reading its documentation — every port, volume and prefix
behaviour recorded in `src/catalog.rs` was observed, not looked up.

Installing, starting, stopping and removing require an admin group, the same
one the self-updater uses and for the same reason — see
[How privileges work](#how-privileges-work). Anyone signed in can see what is
installed and open it: an installed app is part of the host, like a package,
not the property of whoever installed it.

Removing deletes the container. Its data is kept unless you tick the box.

## Icons

File and folder icons are the `css-variables` build of
[Catppuccin Icons](https://github.com/catppuccin/vscode-icons) (MIT). A curated
subset — 53 symbols, 23 KB — is vendored into `ui/icons.svg` as one sprite and
committed, so neither a build nor an install ever fetches them.

The sprite is injected into the document at boot rather than referenced with
`<img>`. That is not incidental: those icons colour themselves from
`--vscode-ctp-*` custom properties, and an image is a separate document that
cannot see this page's variables. Injected, `<use>` resolves in the same
document and the Mocha palette in `style.css` applies.

To cover more file types or move to a newer upstream, edit `SHA` or `ICONS` in
`scripts/vendor-icons.py`, re-run it, and commit the result.

## How privileges work

This is the part worth reading before trusting it.

The daemon runs as **root**, because authenticating against PAM requires it.
It never touches your files with those privileges. On a successful login it
forks a **helper** — the same binary, re-executed with `--helper` — which
permanently drops to the authenticated user via `setgid` → `initgroups` →
`setuid` before doing anything. Every filesystem operation for that session is
performed by that child.

```
browser → daemon (root) → PAM → uid/gid
                        ↘ helper (that user) → filesystem
```

The consequence is the point: **there is no permission logic in this program.**
The kernel decides what the session can read and write, exactly as it would for
that user at a shell. Nothing to get wrong, nothing to bypass. Terminals get the
same treatment for free — `su -` is the shortest correct path to a login shell.

Because it terminates at PAM, whatever the host already uses works unchanged:
local accounts, SSSD, LDAP, Kerberos. Nothing here knows or cares which.

`root` itself is refused a session on purpose.

### The two exceptions

Two things cannot work that way, and both are gated the same way.

**Updating** replaces a root-owned binary and restarts a root service, so it
runs with the daemon's privileges rather than the user's.

**Installing a container app** chooses what code the engine runs, and the
engine runs as root; there is no version of that which the kernel can decide on
a session's behalf. Only install, remove, start and stop are gated. Listing the
installed apps and opening them is open to any session, because an installed
app is part of the host rather than the property of whoever installed it.

These are the only two places in the program where authorisation is a decision
in code instead of a question for the kernel — which is exactly why they are
worth naming rather than burying.

Both are gated on something the host already decided: membership of
`wheel` or `sudo`, resolved through `getgrouplist` exactly as `sudo` resolves
it. An SSSD- or LDAP-provided `wheel` works without being told about. A session
outside those groups gets `403` from every update and app-management route, and
the check is on each route rather than on the button — the controls are hidden
for non-admins, but hiding a button is not an access control.

Installing an app trusts its registry — `lscr.io` or `ghcr.io` — and whoever
publishes the image, for as long as the container runs. Nothing is verified
beyond the registry's own TLS: there is no signature check and no digest
pinning, so `latest` means whatever it means on the day. What limits the blast
radius is that the set of images is fixed in the binary and the container is
published on loopback only.

Be clear-eyed about what updating trusts. Pressing update runs code fetched from
GitHub as root on your host. The trust anchor is TLS to `codeload.github.com`
plus whoever can push to the tracked ref — there is no signature check, and a
compromised upstream is a compromised host. That is the same bargain as any
`curl | sh` installer, which is what this is; it is just wearing a button. If
that is not a bargain you want on a particular box, `WD_UPDATE=off` removes it
and `sudo bash install.sh` from a checkout you control still works.

## Layout

```
src/main.rs     axum server, sessions, filesystem API
src/auth.rs     PAM authentication, NSS lookup, the admin-group check
src/helper.rs   the privilege-dropping child and its file operations
src/proto.rs    JSON-line + binary-payload framing for the helper channel
src/pty.rs      terminal sessions over WebSocket
src/update.rs   version reporting, update check, launching the updater
src/catalog.rs  the fixed list of installable apps and the blanks each one asks
src/engine.rs   docker/podman, as a thin wrapper over their command line
src/apps.rs     installing, running and removing container apps; the state file
src/proxy.rs    the reverse proxy that puts an app on this origin
ui/             the whole frontend — vanilla JS, no build step
ui/icons.svg    vendored Catppuccin icon sprite, injected at boot
ui/ui-icons.svg hand-drawn sprite for the Files toolbar's own actions
scripts/        vendor-icons.py, run by hand to refresh the vendored sprite
scripts/preview.py  serve ui/ on any machine with mocked data — see Previewing the UI
.github/        the release workflow: build, attest, publish
bootstrap.sh    curl | sh installer; also the engine behind an update
install.sh      runs on the target: deps, build, PAM, systemd, firewall
libexec/        the updater: lock, log, status, run bootstrap.sh
deploy.sh       rsync + remote install
```

The UI is compiled into the binary with `rust-embed`, so deployment is a single
file. Measured on Rocky Linux 10 (x86_64): **2.13 MB** including the frontend,
idling at **4.4 MB** resident.

PAM is bound through hand-written FFI rather than `bindgen`, and `build.rs`
links `libpam.so.0` directly when no `-devel` symlink is present. The practical
effect is that this builds on a stock host with **no packages beyond gcc** --
no clang, no pam-devel -- on either distro family.

## Previewing the UI

The app itself needs Linux, PAM and a login shell. A colour, a gap or a label
needs none of that, so there is a look-only preview that runs anywhere Python
does — macOS included:

```sh
scripts/preview.py            # http://127.0.0.1:6868, opens a browser
scripts/preview.py --port 7000 --no-open
```

It serves the same `ui/` files the binary embeds, with a shim that answers every
`/api` call and the terminal socket with canned data. Nothing is copied or
rewritten on disk: `index.html` is patched in flight to load the shim, so what
renders is the file that ships. No npm, no cargo, no venv.

The bar in the corner has four controls:

| | |
| --- | --- |
| **Scene** | jump to a state — sign-in, a failed sign-in, the file manager, the editor, the terminal, System with an update pending / running / failed, a non-admin session, a permission-denied listing, four windows at once, the rename and delete dialogs |
| **Viewport** | render at phone, tablet or laptop size without resizing the window |
| **Inspect** (⌥I) | click any pixel; the `ui/` file and line that style and build it are copied to the clipboard |
| **↻** | reload — though saving anything under `ui/` already reloads the tab |

Inspect is the part that pays for itself when the next step is a prompt. Clicking
the Delete button in the file manager copies:

```
element: button.fbtn.danger
text: "Delete"
path: div.win-body > div.files > div.files-bar > button.fbtn.danger
styled by:
  ui/style.css:141  .fbtn
  ui/style.css:142  .fbtn:hover
  ui/style.css:143  .fbtn.danger:hover
built in:
  ui/app.js:275  <button class="fbtn" data-a="up">Up</button>
```

Scenes and viewports are URL-addressable — `?scene=terminal&device=390x844` —
and `&nowatch=1` turns off the reload poll, which is what makes the preview
screenshottable from a headless browser.

Writes are accepted and reported as successful so the "saved" and "renamed"
states are reachable, but the listing is static and nothing is kept. Anything the
shim does not recognise answers `501 no mock for ...` rather than failing
quietly, so a missed route looks like a missed route and not a UI bug. The whole
harness lives in `scripts/` and is never embedded: `rust-embed` only takes `ui/`.

## API

All endpoints require the session cookie set by `/api/login`.

| Method | Path | |
| --- | --- | --- |
| POST | `/api/login` | `{username, password}` → sets `wd_session` |
| POST | `/api/logout` | ends the session, kills its helper |
| GET | `/api/me` | current identity |
| GET | `/api/fs/list?path=` | directory listing |
| GET | `/api/fs/read?path=` | file contents (64 MB cap) |
| PUT | `/api/fs/write?path=` | body is written verbatim |
| POST | `/api/fs/mkdir` | `{path}` |
| POST | `/api/fs/remove` | `{path}` — directories must be empty |
| POST | `/api/fs/rename` | `{path, to}` |
| GET | `/ws/term` | WebSocket: binary = I/O, text = `{"t":"resize",...}` |
| GET | `/api/apps/catalog` | what may be installed, and whether this session may |
| GET | `/api/apps/list` | installed apps, each with its container's live state |
| ANY | `/app/<slug>/…` | reverse proxy to an installed app |

These additionally require the session to be in an admin group, and return
`403` otherwise:

| Method | Path | |
| --- | --- | --- |
| GET | `/api/system/info` | build, host, and whether this session may update |
| POST | `/api/apps/install` | `{slug, params, tag}` — returns once the pull is handed off |
| POST | `/api/apps/start` | `{slug}` |
| POST | `/api/apps/stop` | `{slug}` |
| POST | `/api/apps/remove` | `{slug, purge}` — `purge` also deletes its data |
| GET | `/api/apps/status` | state, phase and log tail of the current or last install |
| POST | `/api/update/check` | compare the running commit with the tracked ref |
| POST | `/api/update/apply` | start an update; returns as soon as it is handed off |
| GET | `/api/update/status` | state, phase and log tail of the current or last run |

`/api/system/info` is the exception — any session may read it, but it reports
`updates.allowed: false` for one that may not update.

## Known limits

- **No TLS.** The session cookie is `HttpOnly; SameSite=Strict` but not
  `Secure`, because this currently expects plain HTTP on a LAN. Put it behind a
  reverse proxy with a certificate before it leaves one, and add `; Secure` in
  `login()`.
- **Sessions are in memory.** Restarting the service signs everyone out.
- **No static musl build.** PAM `dlopen`s its modules, so it cannot be
  statically linked. Build on each distro family, or build against the oldest
  glibc you intend to support.
- **Delete is non-recursive** — deliberately, for now.
- Files are read into memory rather than streamed, hence the 64 MB cap.
- **Compiling on the host is still the fallback**, and it is expensive when it
  happens. Measured on Rocky Linux 10.2 (x86_64, 32 cores): 42 s wall and
  **2.2 GB peak memory**, leaving **282 MB** in `target/` and **687 MB** of Rust
  toolchain in `/opt/rust`. A 1 GB VM will be OOM-killed mid-build. Release
  binaries avoid all of it, so the fallback should be rare — but a host on an
  architecture or family with no artifact, or one that cannot reach the release,
  pays this every update.
- **Provenance is only checked when `gh` 2.49 or newer is installed.** The
  attestation is always produced and can be verified out of band, but a host
  without a `gh` that can check installs on a checksum alone, which is not a
  provenance control. Debian and Ubuntu LTS archives still carry older `gh`
  builds, so this is the common case rather than the exotic one. Set
  `WD_REQUIRE_ATTESTATION=1` to make it mandatory.
- **No static musl build.** PAM `dlopen`s its modules, so the binary cannot be
  statically linked; that is why artifacts are per libc family rather than one
  universal build.
- **An update signs everyone out**, because sessions are in memory.
- **A container app runs with the engine's default capabilities.** Nothing is
  dropped and no seccomp profile is narrowed, because the LinuxServer images
  step down from root themselves at startup and a tighter default breaks them
  in ways that are hard to diagnose. The containment that is relied on is the
  loopback binding and the fixed catalog, not a hardened runtime. Note this
  cuts the other way too: nothing is *loosened* either.
- **The desktop applications are heavy.** Each one is a real browser or IDE with
  an X server behind it, holding a gigabyte of shared memory and a CPU budget
  for encoding frames. They are not the same kind of thing as a web app that
  idles at a few megabytes, and a host that runs several at once will feel it.
- **term.hut's workspace sync cannot work here.** It is mDNS on port 6768 and
  wants host networking, which is incompatible with the loopback port mapping
  every app gets. Only the web interface on 6767 is proxied.
- **Podman is accepted but untested.** Every command used takes the same
  arguments in both engines, which is why it is offered at all; nothing has
  been run against it.
- **An app must tolerate living under a path prefix.** This is a property of
  the application, not something the proxy can fix for it, and it is why the
  catalog is curated rather than open.
- **Installs are one at a time**, host-wide. A second one is refused while the
  first is running rather than queued.

## Not built yet

Deliberately out of scope for this pass: service and log viewers, storage,
networking, users, multi-window terminals per session, and drag-and-drop
upload.

For container apps specifically: **defining your own container**, which is the
deliberate omission described in [Container apps](#container-apps); choosing an
image tag beyond `latest` and `develop`; updating an installed app to a newer
image; apps that need a second container, such as anything wanting its own
database; and reconciling the app list against containers created or destroyed
behind WebDesk's back.

Native desktop applications are not on this list because they are not on this
path at all. A browser cannot host a native window, so the only way to show one
is to stream its pixels — a different architecture, described in
`docs/architecture.html`.

For the update path specifically: rollback to the previous build and a
scheduled check are both missing and both worth having.
