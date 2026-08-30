# WebDesk
A web desktop for Linux servers. Sign in with your system account, get a file manager and a real terminal in the browser. One binary, no runtime, no buildstep, no npm.

Targets Debian/Ubuntu/Mint, RHEL/Fedora/Rocky, and Arch, on x86_64 and aarch64.

## Install
On the target host:

```sh
curl -fsSL https://raw.githubusercontent.com/HutsonLabs/WebDesk/main/bootstrap.sh | sudo sh
```

That fetches the source, installs build dependencies, builds, installs a systemd unit and a PAM service file, opens the firewall port, and starts the service. Then open **https://\<host\>:61443** and sign in with any normal account on that box. The certificate is self-signed until you give it a real one, so the browser asks about it once per host — see [HTTPS by default](#https-by-default).

The first build takes a few minutes; it is compiling Rust with fat LTO on the host. Nothing is left running that was not asked for, and everything it writes is listed under [What gets installed](#what-gets-installed).

Knobs, all optional:

```sh
curl -fsSL .../bootstrap.sh | sudo PORT=9000 WD_REF=v0.2.0 sh
```

| | |
| --- | --- |
| `PORT` | listen port (default 61443) |
| `WD_TLS=off` | serve plain http, for a host with a TLS proxy in front |
| `WD_TLS_CERT` / `WD_TLS_KEY` | PEM paths, instead of the self-signed pair |
| `PREFIX` | where the binary goes (default `/usr/local/bin`) |
| `WD_REF` | branch, tag or commit to install (default `main`) |
| `WD_REPO` | source repository, for a fork |
| `WD_ADMIN_GROUPS` | who may update from the browser (default `wheel,sudo`) |
| `WD_UPDATE=off` | build without the update capability at all |
| `WD_APPS` | provision what the app kinds need — `containers`, `streamed`, `host`, or `all`. **Empty by default**, so a plain install still adds nothing but WebDesk. `containers` installs Docker, and does nothing on the RHEL family — see [Container apps](#container-apps) |

### What apps need, and when to install it

None of the three kinds of app works on a bare host. Containers need an engine,
[apps drawn on this host](#apps-drawn-on-this-host) need `flatpak`, `cage` and
`wayvnc`, and the [host panels](#host-panels-without-the-cockpit-interface) need
`cockpit-bridge`. **None of it is installed by default**, because a host that
will only ever run the file manager and the terminal should not be made to carry
Docker — what gets written is still exactly [what is listed
below](#what-gets-installed).

There are two moments to fix that, and the second one is the good one.

**At install time**, if you already know what the machine is for:

```sh
curl -fsSL .../bootstrap.sh | sudo WD_APPS=containers,streamed sh
```

`all` and `none` also work, the value is validated before anything is
downloaded, and it is recorded in `/etc/webdesk/install.conf` so an update does
not quietly revert it. Whatever is still missing is named in the closing
summary, along with what it costs and the exact `WD_APPS=` to re-run with.

**Or from the browser, later.** Open **Apps** and anything missing is listed
above the catalog: what it is, one sentence on what stops working without it,
the package that would provide it, and a button that installs the lot. A blocked
install offers the same button in place rather than sending you elsewhere —
pressing it installs the dependency and then continues the install you wanted.
It is gated on the same administrative group as every other install, and a
non-admin is told which groups can rather than shown a button that will fail.

Three answers are refusals rather than buttons, and they say so: you are not in
the group; this host has no package manager WebDesk knows; or the package does
not exist here. The last one is real and specific — **`cage` is in EPEL 10 and
was never built for Enterprise Linux 9**, so an EL9 host is told that streamed
apps are not available on that release rather than being sent to install
something that does not exist. On EL10 it names EPEL and stops: enabling a
third-party repository is not something WebDesk will do to your machine on your
behalf.

### From a checkout instead

```sh
./deploy.sh 10.1.2.40          # or user@host
PORT=61443 ./deploy.sh 10.1.2.40
```

rsyncs the working tree to the host and runs `install.sh` there, which is useful for testing a change you have not pushed. The settings in the table above are forwarded to that remote `install.sh`, so it is also how you move an existing host onto a different port. It rebuilds on the host rather than reusing what is already in that tree's `target/`, which the sync leaves untouched and would otherwise be a binary from the previous deploy. Or, already on the target:

```sh
sudo bash install.sh
sudo systemctl status webdesk
journalctl -u webdesk -f
```

Every one of these is also an upgrade path, and re-running any of them is safe.

### Coming from `linuxwebdesk`

This project was called `linuxwebdesk` until August 2026, and before that `rockywebde`. **There is no in-place upgrade from either.** Everything moved at once -- the binary, the systemd unit, the PAM service, `/etc/`, `/var/lib/`, `/usr/local/src/`, the release asset names, the `WD_*` environment prefix and the session cookie -- so an old install and a new one share no paths and would simply coexist, with the old unit still holding the port.

Remove the old install first, then install as above:

```sh
sudo systemctl disable --now linuxwebdesk
sudo rm -f /etc/systemd/system/linuxwebdesk.service /etc/pam.d/linuxwebdesk \
           /usr/local/bin/linuxwebdesk /usr/local/bin/linuxwebdesk-update \
           /usr/local/libexec/linuxwebdesk-update
sudo rm -rf /usr/local/src/linuxwebdesk /etc/linuxwebdesk /var/lib/linuxwebdesk
sudo systemctl daemon-reload
```

Settings are not carried over, so pass `PORT` and any other knobs again if the old install used something other than the defaults.

## Updating

Click the account button in the upper left of the desktop — the one whose tooltip is your `you@host` — and take the top row of the menu, which says the same name, to open **System**. It shows the running build and, for a member of `wheel` or `sudo`, checks the tracked ref for a newer commit and updates on a button, streaming the build log into the window as it goes. Everyone else sees the build information and a note saying why the update controls are not theirs to use.

It opens as a single window: taking that row again raises the one already open rather than stacking another copy. The second row of the menu signs you out.

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
/var/lib/webdesk/tls/               the self-signed certificate and key (0700/0600)
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
- **Auto-hiding title bars** — a window can give its bar back to whatever is
  inside it. The circle in the title bar is the switch: a filled dot while the
  bar is staying put, an empty ring once it has gone. Thrown, it takes the bar
  off the layout and hangs it on the outside of the window's top edge instead;
  touching that edge brings it back, and it goes again when the pointer
  leaves. This is for the streamed desktops, which draw a bar of their own at
  the top of the screen they are streaming: two bars stacked one on the other
  is what gives away that you are looking at a desktop inside a desktop. The
  bar arrives *above* the window rather than over its content, and the body
  keeps its size throughout, so a streamed canvas is never asked to
  renegotiate its resolution just because the pointer crossed an edge. A
  window snapped against the top of the desktop has no room above it and steps
  down while the bar is out. The cost is the top five pixels of the window,
  which answer to the desktop rather than to the app while the bar is away. The
  setting is remembered per app in the browser's own storage — nothing about
  it reaches the host.
- **Dock** — a floating frosted bar rather than a panel. Windows grow out of
  their icon and shrink back into it, and an icon carries a dot while its app
  has a window open, minimised or not. Clicking an icon raises what is already
  there — the minimised window first, otherwise the next one, so a second click
  on Terminal walks through the terminals; alt- or middle-click opens another.
  Open editors get a dock item each, drawn with the file's own icon. The dock
  is applications and open windows and nothing else.
- **Apps** — a short catalog of container applications, and one service that
  runs on the host itself. Pick one, answer its questions, and it is pulled,
  started and given a dock icon of its own; it opens in a window like
  everything else. See [Container apps](#container-apps).
- **Account** — one button in the upper left, carrying no text: the username is
  its tooltip. It drops a two-row menu — the username again, which opens
  System, and Sign out.
- **No browser dialogs** — nothing calls `prompt()`, `confirm()` or `alert()`.
  Questions are asked in an in-page modal, complaints arrive as a toast above
  the dock, and a file the editor will not take is downloaded rather than
  opened in a tab of its own. Every button names itself in a styled tooltip;
  there is not a `title` attribute in the UI.

## HTTPS by default

WebDesk asks for a system password and hands back a root-capable shell. A login
form on plain http puts that one passive listener away, so **https is the
default and plaintext is now the thing you have to ask for.**

| | |
| --- | --- |
| Port | **61443** — five digits, and above the 32768–60999 range Linux hands out for outbound sockets, so nothing else on the host is already holding it |
| Certificate | self-signed, written to `/var/lib/webdesk/tls/` on first start (`0700`, key `0600`) and reused after that |
| Names on it | the host's own hostname, `localhost`, `127.0.0.1`, `::1` |
| Stack | `rustls` with `ring` — the same one the app proxy already carried, turned around. No OpenSSL headers, no cmake, nothing new to install on the host |
| Protocol | HTTP/1.1 only. h2 is deliberately not offered: a websocket over h2 needs extended CONNECT, and the terminal is a websocket |
| Session cookie | now `HttpOnly; SameSite=Strict; Secure` |

**What the self-signed certificate does and does not buy.** It encrypts, which
is the part that matters for a password crossing a LAN. It authenticates
nothing — there is no authority to check it against — so the browser shows its
interstitial once per host, and a machine-in-the-middle is not detectable. That
is a real limit and it is the honest default: the alternative was http, which
is worse in every respect and has no warning at all.

Give it a real certificate and both the warning and the limit go away:

```sh
sudo curl -fsSL .../bootstrap.sh | sudo \
  WD_TLS_CERT=/etc/ssl/certs/desk.pem WD_TLS_KEY=/etc/ssl/private/desk.key sh
```

Both must be PEM, both must be set together, and the certificate file may be a
chain. Already installed? The same two names in `/etc/webdesk/install.conf` are
what the unit reads, and `systemctl restart webdesk` picks them up.

**Behind a reverse proxy that already terminates TLS**, set `WD_TLS=off` to go
back to plain http on the listen port, and `WD_SECURE=on` so the session cookie
is still marked `Secure` — the browser is on https even though this process is
not. The scheme is not inferred from a forwarded header, because a client can
send one of those too.

**Typing http:// by mistake** gets a `308` to the same URL over https rather
than a TLS parse error and a blank page. The port sniffs the first byte of each
connection — a TLS `ClientHello` starts `0x16`, an HTTP method does not.

**Upgrading an existing install** keeps the port it was installed with; only the
scheme changes. A host on 6767 stays on 6767 and starts answering https there.
Pass `PORT=` explicitly to move it -- `PORT=61443 ./deploy.sh host` -- which
rewrites the recorded port and reopens the firewall on the new one.

## Container apps

WebDesk can install a small, fixed set of applications as containers and show
each one in a window. This section is about the container kind; there are two
others, and both are worth knowing before you read what a container costs — one
is [a service on the host](#a-service-on-the-host-is-not-a-container), and one is
[a Flatpak drawn on this host](#apps-drawn-on-this-host), which answers four of
the limits below by not being a container at all. Open **Apps** from the dock, choose something from
*Available*, fill in its blanks, and press Install. The image is pulled — the
log streams into the window as it goes — the container is created, and the app
appears in the dock with its own icon. Clicking it opens a window.

It needs a container engine on the host, and the two are not treated the same
way. **Docker is what WebDesk offers**: it is what this was written against and
tested with, and it is what the Apps window and `WD_APPS=containers` install.
**Podman is used but never offered.** A host that already has it runs the
container entries on it perfectly well — every command used here takes the same
arguments in both — but WebDesk will not put it there, because installing an
engine is recommending one and nothing has been run against Podman end to end.
Using what an operator already chose claims nothing; choosing for them would.

`WD_CONTAINER_ENGINE=docker` or `=podman` overrides the guess. Without it Docker
wins where both are present, which is also what makes the spare-engine decision
below possible.

**If you end up with both**, the Apps window says so: Docker is in use, Podman is
installed and no longer doing anything, and you can leave it or remove it. Remove
is a real removal and is refused rather than risked — if Podman is holding any
container at all, yours or anyone's, running or stopped, WebDesk will not take
the engine out from under it, and it says how many it found. The check is made
again when you press, not when the panel was drawn, because a container can have
started in between. On the RHEL and Fedora families Podman is part of the
distribution's own tooling, and the confirmation says so before you agree.

WebDesk will not remove a package on its own initiative, and this is the only
package it will remove at all.

### What is in it

| | | |
| --- | --- | --- |
| **Firefox** | `linuxserver/firefox` | the browser, running on this host |
| **Helium** | `linuxserver/helium` | a quieter Chromium |
| **OnlyOffice** | `linuxserver/onlyoffice` | documents, spreadsheets, slides |
| **Inkscape** | `linuxserver/inkscape` | vector drawing |
| **VSCodium** | `linuxserver/vscodium-web` | VS Code without the telemetry |
| **term.hut** | a Flatpak and a systemd unit, not an image | an agent-aware terminal, with the host's own shell — **[read this first](#a-service-on-the-host-is-not-a-container)** |

The first four are **desktop applications, not web apps** — real GTK and C++
programs running headless on the host, drawn into the browser by
[Selkies](https://github.com/selkies-project). That is the same "stream the
pixels in" trade `docs/architecture.html` describes, arrived at by installing a
container rather than by building a streaming stack. It is why they want
[more shared memory](#what-you-choose-and-what-webdesk-chooses) than a server
process does, and why they feel like a remote desktop rather than a web page.

The rest are ordinary web servers: VSCodium serves an editor, and term.hut
serves a terminal from the host itself rather than from a container.

### A service on the host is not a container

One entry is not an image at all. **term.hut** is a Flatpak and a systemd unit
on the host. Everything after the loopback port is identical — the proxy cannot
tell it from a container, and does not need to.

It exists because a terminal in a container is a terminal *into that container*.
That is the right answer for an editor and the wrong one for a shell: the host's
packages, services and files are all on the other side of a boundary that exists
to keep them there. Run the same program as a unit on the host and the shell it
hands out is a shell on the host, which is what opening a terminal meant. There
is no container version of this entry to fall back on — there was one, and it
was removed, because a terminal that silently gave you the wrong machine's shell
is worse than one that will not install.

**Install does the whole thing.** In order, because the order is the point:

1. Checks for `flatpak` and for `xwfb-run`, which is what the unit's
   `ExecStart` uses for a display — GTK wants one even though web mode never
   opens a window, and EL10 ships no Xvfb at all. Anything missing is **offered,
   not installed**: the install refuses, naming the exact packages and the
   manager, and only proceeds if you say yes.
2. Downloads the newest term.hut bundle for this architecture and installs it
   system-wide.
3. Writes `/etc/systemd/system/term-hut-web.service` and enables it.

A host that already has that unit skips all of it: the unit is **adopted exactly
as it is**, never overwritten. That is how this entry used to work and all it
used to be able to do, and it is still the right answer for an operator who
arranged something of their own.

`--host 127.0.0.1` in that unit is the part that matters. WebDesk's sign-in is
the only door to this terminal, and a service bound to every interface has a
second one with no lock on it.

**Writing a unit file is a real widening, and it is worth saying where the line
is.** A unit assembled out of a request would be a way to run anything on this
host as anyone. So the unit's name *and its entire body* are constants in
`src/catalog.rs`; `systemd::write_unit` substitutes the user and uid the service
runs as, takes both from the session of whoever pressed Install, and substitutes
nothing else. A request decides *whether* a unit the build already contains gets
written, never what is in it.

Two more consequences worth knowing. **Everyone who can sign in to WebDesk can
open it**, so that set of people should be the set you would give an SSH
account. And **Remove forgets it rather than deletes it** — uninstalling stops
it being served here and leaves the service and the Flatpak exactly as they are,
which is the safe half of a decision it should not be making for you.

The bundles come from GitHub releases rather than from a Flatpak remote, because
term.hut publishes no repository to add: `flatpak build-bundle` is called with
no `--runtime-repo`, so the installed app reports an origin `flatpak remotes`
has never heard of and `flatpak update` answers "Nothing to do" forever.
Installing means downloading a file, and so does upgrading. WebDesk walks the
release list for the newest one carrying a bundle for this architecture rather
than taking `/releases/latest` — at the time of writing the latest release is a
macOS-only build with no `.flatpak` asset at all, and taking it at its word
would refuse to install on a host where a dozen usable bundles are one page
down.

### What you choose, and what WebDesk chooses

You answer the questions the catalog entry asks, if it asks any — a workspace
folder, a token. WebDesk decides everything else, and none of it is reachable
from the browser:

| | |
| --- | --- |
| Container name | `webdesk-<slug>` |
| Published port | assigned from 47000–47999, **bound to `127.0.0.1`** — a [host service](#a-service-on-the-host-is-not-a-container) is already listening on a port of its own instead, well outside that range |
| State directory | `/var/lib/webdesk/appdata/<slug>`, owned by whoever installed it, mounted where the entry says (`/config`) — an entry that keeps no state, or that runs on the host, is given no directory at all |
| Home directories | the host's `/home`, at `/home` inside, read-write, for **every** app — see below |
| Render node | one `/dev/dri/renderDN`, for an app that draws, when the host has one — never the card node, and left for the image to select rather than named |
| Fonts | the host's `/usr/share/fonts`, read-only at `/usr/local/share/fonts`, for an app that draws — added to the image's own set, never over it |
| `PUID` / `PGID` | the installing user's — but only for images that read them |
| Engine socket | never — no shipping entry is given it, and [a test keeps it that way](#the-engine-socket-and-why-nothing-holds-it) |
| `TZ` | **read off the host**, not asked for — see below |
| `TITLE` | the app's own name, for the desktop applications |
| `--shm-size` | `1g` for the desktop applications; a browser or IDE dies on the 64 MB default |
| Upstream scheme | https for the desktop applications (port 3001), http for the rest |
| Base path | set for the one app that must be told: `CODE_ARGS=--server-base-path=…`. `X-Forwarded-Prefix` is sent to every app besides, for the ones that read it — see below for why telling the others would break them |
| Its own port | only for an app that [cannot live under a prefix](#apps-that-cannot-live-under-a-prefix). You choose the number; WebDesk listens on it and serves that one app at `/` |
| Restart policy | `unless-stopped` |
| Image tag | `latest`, or `develop` — not free text |

**The four desktop applications ask nothing at all.** Every blank they might
have had has one obviously-right answer, so Install is a single press. The
clock comes from `/etc/localtime` (falling back to `timedatectl`, then
`Etc/UTC`), because a container whose clock disagrees with its host timestamps
everything wrong and the correct answer is already on the machine — retyping it
is only an invitation to get it wrong. The images do accept a `PASSWORD` for a
second sign-in of their own, and that is deliberately not offered: reaching one
already means getting past WebDesk's session, so it would be a second lock on
the same door and one more thing to lose.

VSCodium still asks, because a workspace folder has no default worth guessing.
**term.hut asks nothing at all**, and cannot: everything a host service is told
lives in its unit file, so a form here could only collect settings and then drop
them. That includes the token it once minted on first run and printed into a log
the person installing it had no way to read — the unit passes `--no-token`, on
the reasoning already applied to the desktop images' `PASSWORD`: reaching the
terminal means getting past WebDesk's session, so a second lock on the same door
buys nothing and can be lost.

### Every app can see `/home`

Every container gets the host's `/home` bound at `/home`, read-write, without
being asked. A packaged application expects that path to mean what it means
everywhere else — without it an app sees only its own state directory, "open a
file" has nothing to open, and a path copied out of a terminal resolves to
nothing.

**This is a real widening, and worth saying plainly: every container app can
read and write every home directory on the machine.** Installing one is a
decision about all of them, made by the administrative group that is already
trusted to choose what the engine runs. Set `WD_HOME_MOUNT` to another
directory to share that instead, or to `off` to share none.

Two details follow from doing it for every app. Where an app keeps its state
*inside* the shared home — which is where term.hut's container entry kept it,
at `/home/hut`, before that entry was removed — the engine mounts the deeper
path second, so that app still gets its own private state directory rather than
the host's copy; the engine creates that mountpoint if it is missing, so an
empty directory may appear under `/home` on the host, shadowed from the
container's side and holding nothing. No shipping entry does this today; the
ordering is kept because the alternative is an app silently sharing its state
with every user on the machine. And on an SELinux host this mount is **not**
relabelled: `z` rewrites the whole tree it is given, and relabelling `/home`
would stop sshd reading `~/.ssh`. An app that cannot read the share is the
smaller failure, and the recoverable one.

The mount is added when an app is installed. **Apps installed before this
existed do not have it** until they are removed and installed again.

### The apps that draw get the GPU and the host's fonts

The four desktop applications are a real browser or office suite: they render an
interface here and encode every frame of it as H.264 to send to your browser.
Both halves were running in software. On the deployment host, with and without
a render node — same image, same settings:

| | no device | render node |
| --- | --- | --- |
| renderer | `llvmpipe` | `AMD Radeon Vega 11 Graphics (radeonsi)` |
| encoder | `H264 (CPU)` | `H264 (VAAPI)` |
| capture | readback path | zero-copy path |

So an entry that draws is given **one render node** — `/dev/dri/renderDN`, never
`/dev/dri/cardN`, which is the modesetting device that drives the physical
monitor and which a headless app never opens. One and not several, because a
container that sees a single node is the case these images handle best; and
deliberately *not* told which one it is, because naming `DRI_NODE` suppresses
the scan the image runs when that variable is unset. A host with no graphics
device installs exactly as before. `WD_GPU=off` declines it.

The same entries get **the host's fonts**, read-only, at
`/usr/local/share/fonts` — a path fontconfig already scans *in addition* to the
image's own. Where they land is the whole point: mounted over `/usr/share/fonts`
instead, they *replace* the image's set and make things worse, taking
OnlyOffice's `Arial` from the real `Arial.ttf` to a Nimbus Sans substitute.
Mounted additively, Firefox goes from 415 font families to 979 — picking up the
host's Japanese, Thai and Devanagari coverage — with its existing matches
unchanged. `WD_FONT_MOUNT=off` declines it.

Neither is applied to VSCodium, which draws in *your* browser on *your* machine,
with your GPU and your fonts.

**These are widenings, and here is exactly how far they go.** `--device` adds
one character device to the container's allowlist, and `--group-add` grants what
that group already grants on the host. Neither adds a capability. Nothing here
emits `--security-opt`, `--privileged`, `--cap-add`, `--device-cgroup-rule`, or
host networking, and there is a test that fails if one ever does.

`docs/host-access.md` has the full measurements, including the mechanisms that
were tried and rejected — host audio, printing, D-Bus — and why.

### SELinux relabelling asks the engine, not the kernel

A `z`/`Z` suffix is only added when the **engine** actually labels containers,
which is not the same question as whether the host is enforcing. The deployment
host is enforcing, and its Docker reports security options of `seccomp` and
`cgroupns` only — no `selinux` — so containers there run with an empty process
label and a relabelling would change the host's files for a confinement nobody
applies. An engine that cannot be asked is treated as not labelling.

Three mounts are never relabelled whatever the answer: the shared `/home`
(relabelling it stops sshd reading `~/.ssh`), the shared fonts (it would rewrite
a system directory WebDesk does not own — and read-only is no defence, since the
relabelling happens to the source), and the engine socket. The rule they are all
instances of: **a mount WebDesk adds unasked never relabels the host.**

### Apps that cannot live under a prefix

Almost everything in the catalog is reached at `/app/<slug>/` on WebDesk's own
origin, and that is the arrangement to prefer: one port to open, one
certificate, one origin, and cookies the proxy pins per app so they cannot leak
between them.

Some applications cannot be served that way at all. They compile `/api/...` into
their own client — in `fetch`, in an `EventSource`, in a WebSocket built from
`location.host` — with no base path to configure and no interest in
`X-Forwarded-Prefix`. Under a prefix those calls land on WebDesk's own `/api/*`
and the frame stays empty. Dockhand was one of these, and is why this exists:
without it, "does this application tolerate a path prefix" was quietly deciding
what WebDesk is allowed to install. No entry ships with `needs_origin` today,
so nothing asks you for a port — the machinery stays because the next such app
is a catalog entry away, not a rewrite.

Such an entry sets `needs_origin` and asks you for a port. WebDesk opens a
second listener there and serves that one app at the root of it — still
refusing anyone without a WebDesk session, exactly as the prefixed route does,
and with the container still bound to `127.0.0.1` and unreachable from the
network on its own.

**Reach it at the same hostname you already use for WebDesk.** A certificate
does not name a port, so whatever address you trust today — a real domain with
a real certificate, or an IP with the self-signed one you clicked through once —
keeps working at `:<port>` with no new warning, no new DNS record, and no
configuration WebDesk has to be told. It arrives signed in for the same reason:
[cookies are not isolated by port](https://www.rfc-editor.org/rfc/rfc6265#section-8.5),
and `SameSite=Strict` is evaluated per site rather than per origin. The URL is
built from the `Host` header of the request that asked for it, because WebDesk
is never told its own public name and guessing one from an interface address
would be wrong for everyone who reaches it by a domain.

It costs two things, and neither is hidden:

- **An open port per such app**, which you have to allow through your firewall.
  That is why it is opt-in per entry and not the default.
- **That app's cookies stop being isolated from WebDesk's.** The proxy pins a
  prefixed app's cookies to `/app/<slug>/`; an app at `/` sets `Path=/`, and
  since cookies ignore ports those now reach WebDesk's port too. Nothing can
  forge a session — `wd_session` is refused from an app on both paths — but the
  privacy boundary between apps is weaker here than under a prefix.

### The engine socket, and why nothing holds it

One field in the catalog would undo every flag above: `socket`, which binds the
host's `/var/run/docker.sock` into a container read-write. A process that can
talk to the engine can ask it to start a container that bind-mounts `/`. There
is no sandbox left to speak of — it is root on the host, and no seccomp profile
or capability set inside the container changes it.

**No shipping entry sets it.** Dockhand did, as the one engine manager in the
catalog, and it has been removed; a test now asserts the list is empty, so an
entry acquiring the socket fails in CI rather than in somebody's install. The
field stays because an engine manager with no engine to manage is not an
application — but it is a field on the entry, never a question on a form, so no
request from a browser can ask for it.

Two consequences are worth writing down before anyone adds one back, because
neither is hypothetical:

- **Installing such an app is a decision about every session, not just yours.**
  Only the administrative group may *install* an app, but [any signed-in user
  may open one](#why-every-app-is-on-this-origin). So installing an engine
  manager promotes every WebDesk account on this host to root on it.
- **Its own sign-in has to go on immediately.** Dockhand started with
  authentication *disabled*. Everywhere else in this project a second sign-in is
  argued against as a second lock on the same door; that argument depends on the
  door being worth one lock, and here what is behind it is the machine.

On an SELinux host the socket would deliberately **not** be relabelled: it
belongs to the daemon and every other client on the machine, and quietly
re-labelling it to suit one container is a change to something the host needs to
run containers at all. Make that a policy decision if you want it, where it is
visible and reversible.

A folder you name is mounted where the catalog entry says, read-only where that
makes sense, and it is checked first: it must be an absolute path to a real
directory, and it may not be `/` or anything under `/etc`, `/proc`, `/sys`,
`/dev`, `/boot`, `/run`, `/usr`, `/bin`, `/sbin`, `/lib` — or under WebDesk's
own state directory, since an app that could write there could rewrite the list
of what gets run. The check resolves symlinks before testing, so a link into
`/etc` is refused as if it had been typed.

On a host with SELinux, mounts are relabelled: `Z` for `/config`, which is ours
alone, and the shared `z` for a directory you named and may still want to reach
yourself. The shared `/home` is the exception and gets neither, for the reason
given above.

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
- **There is one port to open, for everything served this way.** The firewall
  story is unchanged: the one port WebDesk listens on. An app that
  [cannot live under a prefix](#apps-that-cannot-live-under-a-prefix) is the
  exception and asks for a port of its own, which is exactly why it is opt-in
  per entry rather than how every app is served.
- **Every app inherits WebDesk's TLS.** The browser's connection is to WebDesk,
  which terminates TLS itself, so an app speaking plaintext over loopback still
  reaches the user encrypted and inside a secure context.

Two rules keep an app inside its own prefix. The session cookie is stripped
from every request before the app sees it, and cookies coming back are pinned
to `/app/<slug>/` — with any attempt to set `wd_session` dropped outright.
Redirects are moved back under the prefix, and `frame-ancestors` is removed
from any policy the app sends.

### The loopback hop to a desktop app is TLS

The four desktop applications are proxied to their **https** port, 3001, so the
proxy carries a TLS client — `rustls`, chosen over `native-tls` because that one
wants OpenSSL headers at build time and this program builds on a stock host with
nothing beyond gcc. It costs about 0.55 MB of binary.

**The certificate is not verified, and verifying it would mean nothing.** The
image generates its own self-signed certificate with `CN=*` at first start;
there is no authority to check it against and no name to match. What makes the
hop private is that it is a socket to a port bound on `127.0.0.1` and never
leaves the machine.

Be clear about what this does and does not buy. It encrypts a hop that was
already private, and it changes nothing a browser can observe: the browser talks
to WebDesk's origin, so whether the page is a *secure context* — which is what
the clipboard, microphone and WebRTC actually check — is decided by WebDesk's
own listener, which is https by default, not by this. Port 3000 serves
byte-identical content over plain http and works, websocket included. This is
the https port by request rather than by necessity.

### Why the catalog is fixed

Creating a container chooses what code the engine runs, and the engine runs as
root. That is not something to hand to a browser, so the catalog lives in the
binary and is reviewed like any other code. **You cannot define your own
container yet**, by design.

Most entries are [LinuxServer.io](https://www.linuxserver.io/) images from
`lscr.io`, which share one contract — `/config`, `PUID`/`PGID`, `TZ` — so the
installer has one shape to implement rather than one per application. Entries
say whether they are one (`lsio`) rather than the installer assuming, because an
image that is not one would be sent `PUID`/`PGID` and `TZ` it never reads —
noise in `docker inspect` that reads like a fact about the image. term.hut is
the entry that made that distinction necessary and no longer tests it, having
left the engine entirely for a Flatpak on the host.

**The requirement that decides membership** — for a container entry — is that
it must work when served from `/app/<slug>/` instead of `/`. It does not apply
to an app [drawn on this host](#apps-drawn-on-this-host): that kind is never
served through the proxy, so there is no prefix for it to survive and nothing
here to test. If you are adding a Flatpak entry, this whole subsection is about
a problem you do not have. An application that assumes it owns
the root emits links that escape its prefix and renders as a blank frame. There
are two ways to satisfy it:

- **It works it out itself.** The Selkies desktop images derive everything from
  `location.pathname` — assets as `./assets/…`, their socket as
  `<base>websockets`. Nothing to configure.
- **It is told.** VSCodium needs `--server-base-path` or its assets come out
  rooted at `/stable-<hash>/…`. The entry names the variable and the shape to
  put the prefix in.

**Telling one is not the safe default — it is the dangerous one.** The proxy
strips `/app/<slug>` before forwarding, so an app only ever sees paths from its
own root. A base path is harmless to an app that treats it as *what prefix to
write into the links it generates* and goes on answering at `/`, which is what
VSCodium does. It is fatal to an app that *routes* on it: that app is now
waiting at a prefix the proxy guarantees it will never be sent, so every real
request arrives as `/` and 404s.

term.hut is the second kind, and was told anyway — which is exactly the blank
frame this section warns about, arrived at by trying to prevent it. It is now
told nothing, and works: its hrefs are already relative, so they resolve under
whatever prefix the browser is on. **A blank frame is as likely to mean you
configured a prefix as that you forgot one.**

Check this first before adding anything, and check it by running the image
rather than by reading its documentation — every port, volume and prefix
behaviour recorded in `src/catalog.rs` was observed, not looked up. The cheap
check is three curls straight at the published port: `/` should answer, and if
it only answers at `/app/<slug>` then a base path is being routed on and must
not be set.

Installing, starting, stopping and removing require an admin group, the same
one the self-updater uses and for the same reason — see
[How privileges work](#how-privileges-work). Anyone signed in can see what is
installed and open it: an installed app is part of the host, like a package,
not the property of whoever installed it.

Removing deletes the container. Its data is kept unless you tick the box.

## Apps drawn on this host

A third kind of entry is neither a container nor an adopted service. It is a
**Flatpak that runs on this machine as you**, under a headless compositor, with
its pixels carried into a WebDesk window. Open it from the dock and you are
running the program locally in every sense that matters — it has your home
directory, your files, your fonts, your theme, the machine's GPU and a working
`xdg-desktop-portal`, because it is an ordinary application in an ordinary
session and not a guest in a container that had to be handed each of those one
at a time.

That is the whole argument for it, and it is easiest to see in what such an
entry does *not* have to answer for:

| | container entry | drawn on this host |
| --- | --- | --- |
| published port | assigned from 47000–47999 | none |
| state directory | `/var/lib/webdesk/appdata/<slug>`, mounted at `/config` | none — Flatpak already keeps it in `~/.var/app/<id>`, per user |
| `PUID`/`PGID` | the installer's | none — it runs *as* you |
| `TZ` | read off the host and passed in | none — it is on the host |
| `--shm-size` | `1g`, or a browser dies | none |
| render node and fonts | bound in, when the host has them | none — every device is simply present |
| path prefix | must survive `/app/<slug>/` | none — it never touches the proxy |

Seven absences, but they are one simplification said seven times: none of those
questions exist when the application is already running as the right user on the
right machine.

**It answers four of the [Known limits](#known-limits) the container desktops
have.** A download lands in `~/Downloads` rather than in an app directory. There
is no passwordless root shell behind it. `/home` is not mounted read-write for
every app on the box, because nothing is mounted anywhere. And it does not hold
a gigabyte of shared memory to run a browser.

**What it costs.** The container boundary is gone. A drawn app is confined by
Flatpak's own sandbox, which is finer-grained than "this container can read
every home directory on the machine" but is the *app's* sandbox rather than
ours — and an app that asks for the whole of `home` in its manifest has one no
narrower than a container's on that question. It is still the right user's
files, which is the point, but do not read this as tighter confinement across
the board. That is why installing one is still gated on the administrative
group, and why `scripts/flathub-entry.py` prints an app's requested permissions
before you add it.

### Installed once, run per user

The two halves are deliberately different, and they line up with the split this
project already had between who may install and who may open.

**Installing is host-wide.** `flatpak install --system`, one copy on disk, part
of the machine like a package, and gated on `wheel`/`sudo` exactly like every
other install here. **Running is per user.** Opening one starts a systemd *user*
unit in your own session, because an application whose subject is your home
directory is worth nothing pointed at somebody else's — the same reasoning that
put [term.hut on the host](#a-service-on-the-host-is-not-a-container) rather
than in a container, applied one step further.

So two people can have the same app open at once and they are two compositors,
two sockets and two sets of files, neither able to reach the other's. The socket
is a unix socket under `/run/webdesk/rfb/<uid>/`, mode `0700`: unreachable from
the network as a property of the filesystem rather than of a `--bind` somebody
had to remember to write.

### How the pixels get out

`cage` — a kiosk compositor holding exactly one application, which is already
WebDesk's window model — with `wayvnc` serving RFB on that socket, and noVNC
drawing it on a canvas in the browser. WebDesk's own proxy terminates the
WebSocket, so there is no `websockify` and no second daemon per app.

Two consequences worth knowing:

- **The unit is one template for every app, and its instance name is a slug.**
  A unit whose `ExecStart` interpolated an application id would be a way to run
  any Flatpak on this host as anyone. Instead the unit runs
  `webdesk app-session <slug>`, which resolves the slug against the catalog
  compiled into the binary and refuses anything that is not in it. A request
  still decides only *whether* something the build already contains runs.
- **Stopping a session kills the Flatpak by application id**, because that is
  the only handle that reaches into the scope `flatpak run` escapes into — the
  same problem term.hut's unit documents. It follows that quitting an app here
  also closes that app if you happen to have it open on the machine's own
  screen. That is announced in the unit's journal rather than done silently.

### Adding one

`scripts/flathub-entry.py <application-id>` prints a ready-to-paste catalog
entry, along with the things that actually decide whether it belongs: installed
size, licence, whether Flathub has verified the publisher, and which sandbox
permissions it asks for. It also says plainly what it cannot answer — whether
the app is usable at a fixed initial resolution, whether its dialogs behave
under a one-surface compositor, and whether it wants audio it will not get.

Entries still live in the binary and are still reviewed like code. What changed
is what the review is *about*: for a container the deciding question was
mechanical — does this application tolerate living under `/app/<slug>/` — and
for a drawn app that question does not exist. The gate is now judgement rather
than compatibility. See [docs/flathub.md](docs/flathub.md).

## Host panels, without the Cockpit interface

Service, log and metric views talk to **`cockpit-bridge`**, and to nothing else
of Cockpit. The bridge is a separate package from `cockpit-ws`: one binary that
speaks a JSON channel protocol over stdio, with no port, no web server, no login
page and no interface. Installing it surfaces none of Cockpit, which is why this
was chosen over putting the web console in an iframe — that would have meant a
second PAM sign-in and a second visual language inside the desk.

It runs **as the signed-in user**, spawned the way `pty.rs` spawns a shell, so
the kernel enforces what a session may read and write. That is the property that
makes it safe, and it is why none of this needed a privileged daemon of its own —
[architecture.html](docs/architecture.html) prices the alternative at Umbrel's
~35,000-line root daemon.

**The protocol is terminated in WebDesk and never handed to the browser.**
Cockpit's own client opens channels from JavaScript because Cockpit's interface
*is* JavaScript; doing the same here would hand a browser a `stream` channel,
which is a shell by another name. Every endpoint is instead a named operation
with a validated parameter — `/api/host/services`, `/api/host/journal`,
`/api/host/metrics`, and a start/stop/restart whose verb is matched against a
fixed table and whose unit must be one the desk has already listed.

On a host without the bridge every one of those answers `503` naming the package
to install rather than failing, which is what the dependency check reads. On
Arch there is no split package, so installing the bridge there installs the web
console too — that is said out loud before the button rather than discovered
afterwards.

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

The desktop's own icons — the Files toolbar, the dock, the window controls —
are hand-drawn in `ui/ui-icons.svg`, hairline strokes in `currentColor` so a
button colours its own icon on hover.

**Application marks are the exception**, and they come from
[Simple Icons](https://simpleicons.org) (CC0 1.0), vendored into the same
sprite by `scripts/brand-icons.py`. They are solid glyphs, not hairline
strokes, because that is what a brand mark is; what makes them belong is that
they share the 24 grid, take their colour from `currentColor` like everything
else, and are scaled a little under full size so a filled shape does not
outweigh a stroked one beside it in the dock. Brand colours are deliberately
dropped. The marks remain their owners' trademarks; they are used here to label
the application they belong to and nothing else.

```sh
scripts/brand-icons.py            # refresh the marks from upstream
scripts/brand-icons.py --check    # fail if the committed sprite is stale
```

Two things that catch people out. **Some upstream marks are a filled square with
the shape knocked out of them**, which at dock size is a black block rather than
an icon — `STRIP_LEADING` names those, so the script drops the square and keeps
the shape, and refuses to run if upstream changes that path out from under it.
It is empty right now, since the one entry that needed it left the catalog. And
**an icon id the catalog names but the sprite does not have is a blank square in
the Apps window**, so a test asserts every one of them resolves.

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
src/proxy.rs    the reverse proxy that puts an app on this origin, http or TLS
src/tls.rs      WebDesk's own https listener and its self-signed certificate
ui/             the whole frontend — vanilla JS, no build step
ui/icons.svg    vendored Catppuccin icon sprite, injected at boot
ui/ui-icons.svg hand-drawn sprite for the Files toolbar's own actions
scripts/        vendor-icons.py, run by hand to refresh the vendored sprite
scripts/brand-icons.py  the application marks, from Simple Icons — see Icons
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

The app entries are the one piece of mock data with a source of truth elsewhere,
so they are not trusted to stay in step on their own: `scripts/preview.py` reads
the real ones out of `src/catalog.rs` on each load and the shim takes each
entry's name, icon, image and prose from there, keeping only the install-form
blanks of its own. Drift is corrected rather than drawn, and named in the console
so the stale copy in `mock.js` can be fixed. Editing an entry reloads the tab the
same way editing `ui/` does.

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

- **The certificate is self-signed unless you supply one.** That is a real
  limit, not a formality: it encrypts, but it authenticates nothing, so a
  machine-in-the-middle on the path is not detectable. On a LAN it is strictly
  better than the plaintext it replaced; across anything less trusted, give it
  a certificate with `WD_TLS_CERT` and `WD_TLS_KEY`. See
  [HTTPS by default](#https-by-default).
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
  loopback binding and the fixed catalog, not a hardened runtime. No capability
  is *added* either — the two flags that reach outside the container are one
  render node and the group that opens it.
- **Nothing checks for disk space before pulling an image.** The desktop images
  are multi-gigabyte unpacked — OnlyOffice is around 6 GB on disk — and
  `docker pull` is run with no free-space precondition, so an install onto a
  nearly-full filesystem fails partway rather than being refused up front. Check
  `df` before installing a desktop app you have not installed before.
- **A signed-in user gets a root shell inside any desktop app's container.**
  The LinuxServer desktop images ship a terminal with passwordless sudo, so
  anyone who can open one of these apps can install software in that container
  and reach the network from it — and `/home` is mounted read-write. The blast
  radius is the container, not the host, but the set of people who can sign in
  to WebDesk should be chosen with that in mind. The images offer
  `HARDEN_DESKTOP` and `DISABLE_SUDO`; WebDesk does not set them, because
  the trade differs per app — see
  `docs/host-access.md`.
- **A download goes to the app's state directory, not to a home directory.**
  The images set `HOME=/config`, so a file saved in the containerised Firefox
  lands in `/var/lib/webdesk/appdata/firefox/Downloads` rather than in
  `~/Downloads`. It is reachable — `/home` is mounted, so the save dialog can be
  pointed at `/home/<you>/Downloads` — but the default is wrong. It is left that
  way deliberately: every fix picks one user's home, and these apps are shared
  by everyone who can sign in. See `docs/host-access.md`.
- **The desktop applications are heavy.** Each one is a real browser or IDE with
  an X server behind it, holding a gigabyte of shared memory and a CPU budget
  for encoding frames. They are not the same kind of thing as a web app that
  idles at a few megabytes, and a host that runs several at once will feel it.
- **term.hut's workspace sync is not proxied.** It is mDNS on port 6768; only
  the web interface on 6767 is served here. Running on the host rather than in a
  container removes the networking objection — the service can see the LAN like
  any other host process — but WebDesk still puts nothing but 6767 in front of
  the browser.
- **Podman is used but untested.** Every command used takes the same arguments
  in both engines, which is why a host that has it works; nothing has been run
  against it, which is why WebDesk will not install it. What would change that
  is the install, start, stop and remove path exercised end to end on one host
  of each family with the desktop entries actually drawing.
- **Docker is not installable from here on the RHEL or Fedora families.**
  Fedora's `moby-engine` is a fork rather than Docker, and Enterprise Linux has
  neither it nor Docker CE, so both roads end at Docker's own repository and
  signing key. WebDesk names it and stops, the same as it does for EPEL. Install
  Docker yourself, or install Podman — which WebDesk will then use.
- **An app must tolerate living under a path prefix.** This is a property of
  the application, not something the proxy can fix for it, and it is why the
  catalog is curated rather than open.
- **Installs are one at a time**, host-wide. A second one is refused while the
  first is running rather than queued.
- **An app drawn on this host has no sound.** RFB carries none. That is fine
  for an editor and disqualifying for a media player, and
  `scripts/flathub-entry.py` flags an application that asks for PulseAudio
  before you add it.
- **Its clipboard is text only**, both directions. Images and file lists do not
  cross.
- **Resize is the roughest edge.** wlroots brings its headless output up at a
  hardcoded 1280×720 and there is no compositor flag for anything else, so the
  size in a catalog entry is a request the *browser* makes on connecting, which
  `wayvnc` then applies. When that request does not arrive the window scales a
  720p bitmap instead of being crisp.
- **Latency is VNC latency.** Fine on a LAN; worse than a purpose-built encoder
  across a WAN.
- **Drawn apps do not work on the EL9 generation at all.** `cage` is packaged in
  EPEL 10 and there is no EPEL 9 build of it by any name — `wayvnc` is in both,
  but the compositor is the half that is missing. Rocky and Alma 10 are one
  `dnf` away once EPEL is enabled; enabling EPEL is a third-party repository and
  WebDesk will not do it behind your back.
- **A drawn app is confined by its own Flatpak manifest, not by ours.** An
  application that asks for the whole of `home` has a filesystem sandbox no
  narrower than a container's. It is still *your* files rather than everyone's,
  which is the improvement, but it is not blanket confinement.
- **`cockpit-bridge` is not split from the web console on Arch**, so installing
  it there installs Cockpit's interface too — the one thing the host panels are
  arranged to avoid.

## Not built yet

Deliberately out of scope for this pass: service and log viewers, storage,
networking, users, multi-window terminals per session, and drag-and-drop
upload.

For container apps specifically: **defining your own container**, which is the
deliberate omission described in [Container apps](#container-apps); choosing an
image tag beyond `latest` and `develop`; updating an installed app to a newer
image, and updating a Flatpak entry — the two are not the same gap, and it is
worth saying which. An image has no update path at all here. A Flatpak has one
that works, `flatpak::update` in the binary, with nothing calling it yet: no
button, no route, no scheduled check. That is a wire to run, not a mechanism to
invent; apps that need a second container, such as anything wanting its own
database; and reconciling the app list against containers created or destroyed
behind WebDesk's back.

Native desktop applications are not on this list because they are not on this
path at all. A browser cannot host a native window, so the only way to show one
is to stream its pixels — a different architecture, described in
`docs/architecture.html`.

For the update path specifically: rollback to the previous build and a
scheduled check are both missing and both worth having.
