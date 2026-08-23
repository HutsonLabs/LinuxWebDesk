# LinuxWebDesk

A web desktop for Linux servers. Sign in with your system account, get a file
manager and a real terminal in the browser. One binary, no runtime, no build
step, no npm.

Targets Debian/Ubuntu/Mint, RHEL/Fedora/Rocky, and Arch, on x86_64 and
aarch64.

## Install

On the target host:

```sh
curl -fsSL https://raw.githubusercontent.com/HutsonLabs/LinuxWebDesk/main/bootstrap.sh | sudo sh
```

That fetches the source, installs build dependencies, builds, installs a systemd
unit and a PAM service file, opens the firewall port, and starts the service.
Then open **http://<host>:7788** and sign in with any normal account on that box.

The first build takes a few minutes; it is compiling Rust with fat LTO on the
host. Nothing is left running that was not asked for, and everything it writes
is listed under [What gets installed](#what-gets-installed).

Knobs, all optional:

```sh
curl -fsSL .../bootstrap.sh | sudo PORT=9000 LWD_REF=v0.2.0 sh
```

| | |
| --- | --- |
| `PORT` | listen port (default 7788) |
| `PREFIX` | where the binary goes (default `/usr/local/bin`) |
| `LWD_REF` | branch, tag or commit to install (default `main`) |
| `LWD_REPO` | source repository, for a fork |
| `LWD_ADMIN_GROUPS` | who may update from the browser (default `wheel,sudo`) |
| `LWD_UPDATE=off` | build without the update capability at all |

### From a checkout instead

```sh
./deploy.sh 10.1.2.40          # or user@host
```

rsyncs the working tree to the host and runs `install.sh` there, which is
useful for testing a change you have not pushed. Or, already on the target:

```sh
sudo bash install.sh
sudo systemctl status linuxwebdesk
journalctl -u linuxwebdesk -f
```

Every one of these is also an upgrade path, and re-running any of them is safe.
This project used to be called `rockywebde`; upgrading from a host that ran that
version retires the old unit, PAM file and binary automatically. Everyone is
signed out once, because the session cookie was renamed with everything else.

## Updating

Click the account chip at the right of the dock — the `you@host` one — to open
**System**. It shows the running build and, for a member of `wheel` or `sudo`,
checks the tracked ref for a newer commit and updates on a button, streaming the
build log into the window as it goes. Everyone else sees the build information
and a note saying why the update controls are not theirs to use.

It opens as a single window: clicking the chip again raises the one already
open rather than stacking another copy.

The same thing from a shell, doing exactly the same work:

```sh
sudo linuxwebdesk-update
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
  stays in the System window and in `journalctl -u linuxwebdesk-update`.
- **Settings survive.** Port, prefix and tracked ref are recorded in
  `/etc/linuxwebdesk/install.conf` at install time and read back on update, so a
  host installed on port 9000 comes back on port 9000.
- **The installer comes from the tracked ref**, not from the copy the last
  update left on disk, so a change to how installing works takes effect on the
  next update rather than the one after it. If it cannot be fetched, the copy on
  disk is used instead, which still installs the source already there.

To follow something other than `main`, edit `LWD_REF` in that file and restart.
To remove the capability from a host entirely, set `LWD_UPDATE=off` there and in
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
reports the real number, and `bootstrap.sh` records it in `.lwd-source` so a
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
| `LWD_PREBUILT=off` | never use a release binary; always compile on the host |
| `LWD_REQUIRE_ATTESTATION=1` | refuse to install unless provenance is verified — implies `gh` 2.49+ must be present |
| `LWD_RELEASE_TAG=tag` | take the binary from a specific release |

Be clear about what the checksum does and does not buy you: it comes from the
same origin as the binary, so it catches corruption, not a compromised release.
The attestation is the part that establishes where the binary came from, and it
is only checked if `gh` is on the host. Verify one by hand with:

```sh
gh attestation verify linuxwebdesk-x86_64-rhel --repo HutsonLabs/LinuxWebDesk
```

## What gets installed

```
/usr/local/bin/linuxwebdesk              the binary
/usr/local/bin/linuxwebdesk-update       symlink to the updater
/usr/local/libexec/linuxwebdesk-update   the updater
/usr/local/src/linuxwebdesk/             source, kept for incremental rebuilds
/etc/linuxwebdesk/install.conf           settings, so updates preserve them
/etc/pam.d/linuxwebdesk                  PAM service
/etc/systemd/system/linuxwebdesk.service the unit
/var/lib/linuxwebdesk/                   update log and status (root, 0700)
```

`/usr/local/src/linuxwebdesk/target` is a Rust build directory and is the large
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

### The one exception

Updating cannot work that way. It replaces a root-owned binary and restarts a
root service, so it runs with the daemon's privileges rather than the user's —
which makes it the only place in the program where authorisation is a decision
in code instead of a question for the kernel.

So it is gated, and gated on something the host already decided: membership of
`wheel` or `sudo`, resolved through `getgrouplist` exactly as `sudo` resolves
it. An SSSD- or LDAP-provided `wheel` works without being told about. A session
outside those groups gets `403` from every update route, and the check is on
each route rather than on the button — the dock item is hidden for non-admins,
but hiding a button is not an access control.

Be clear-eyed about what this trusts. Pressing update runs code fetched from
GitHub as root on your host. The trust anchor is TLS to `codeload.github.com`
plus whoever can push to the tracked ref — there is no signature check, and a
compromised upstream is a compromised host. That is the same bargain as any
`curl | sh` installer, which is what this is; it is just wearing a button. If
that is not a bargain you want on a particular box, `LWD_UPDATE=off` removes it
and `sudo bash install.sh` from a checkout you control still works.

## Layout

```
src/main.rs     axum server, sessions, filesystem API
src/auth.rs     PAM authentication, NSS lookup, the admin-group check
src/helper.rs   the privilege-dropping child and its file operations
src/proto.rs    JSON-line + binary-payload framing for the helper channel
src/pty.rs      terminal sessions over WebSocket
src/update.rs   version reporting, update check, launching the updater
ui/             the whole frontend — vanilla JS, no build step
ui/icons.svg    vendored Catppuccin icon sprite, injected at boot
scripts/        vendor-icons.py, run by hand to refresh that sprite
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

## API

All endpoints require the session cookie set by `/api/login`.

| Method | Path | |
| --- | --- | --- |
| POST | `/api/login` | `{username, password}` → sets `lwd_session` |
| POST | `/api/logout` | ends the session, kills its helper |
| GET | `/api/me` | current identity |
| GET | `/api/fs/list?path=` | directory listing |
| GET | `/api/fs/read?path=` | file contents (64 MB cap) |
| PUT | `/api/fs/write?path=` | body is written verbatim |
| POST | `/api/fs/mkdir` | `{path}` |
| POST | `/api/fs/remove` | `{path}` — directories must be empty |
| POST | `/api/fs/rename` | `{path, to}` |
| GET | `/ws/term` | WebSocket: binary = I/O, text = `{"t":"resize",...}` |

These additionally require the session to be in an admin group, and return
`403` otherwise:

| Method | Path | |
| --- | --- | --- |
| GET | `/api/system/info` | build, host, and whether this session may update |
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
  `LWD_REQUIRE_ATTESTATION=1` to make it mandatory.
- **No static musl build.** PAM `dlopen`s its modules, so the binary cannot be
  statically linked; that is why artifacts are per libc family rather than one
  universal build.
- **An update signs everyone out**, because sessions are in memory.

## Not built yet

Deliberately out of scope for this pass: service and log viewers, storage,
networking, users, containers, multi-window terminals per session, drag-and-drop
upload, and an app launcher for other web UIs on the host.

For the update path specifically: rollback to the previous build and a
scheduled check are both missing and both worth having.
