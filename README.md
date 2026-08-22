# LinuxWebDesk

A web desktop for Linux servers. Sign in with your system account, get a file
manager and a real terminal in the browser. One binary, no runtime, no build
step, no npm.

Targets Debian and RHEL/Fedora/Rocky.

## Try it

```sh
./deploy.sh 10.1.2.40          # or user@host
```

That rsyncs the tree, installs build dependencies, builds, installs a systemd
unit and a PAM service file, opens the firewall port, and starts the service.
Then open **http://10.1.2.40:7788** and sign in with any normal account on
that box.

Re-running `deploy.sh` is also the upgrade path. This project used to be called
`rockywebde`; upgrading from a host that ran that version retires the old unit,
PAM file and binary automatically. Everyone is signed out once, because the
session cookie was renamed with everything else.

To build and run by hand on the target instead:

```sh
sudo bash install.sh
sudo systemctl status linuxwebdesk
journalctl -u linuxwebdesk -f
```

## What it does

- **Files** — browse, open, edit and save text files, upload, download, create
  folders, rename, delete.
- **Terminal** — a real login shell via `su - <user>`, xterm.js in the browser,
  resize-aware.
- **Windows** — draggable and resizable panes with focus, minimize and a dock.
  Not a compositor; they are positioned divs, which is all a shell like this
  needs.

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

## Layout

```
src/main.rs     axum server, sessions, filesystem API
src/auth.rs     PAM authentication and NSS lookup
src/helper.rs   the privilege-dropping child and its file operations
src/proto.rs    JSON-line + binary-payload framing for the helper channel
src/pty.rs      terminal sessions over WebSocket
ui/             the whole frontend — vanilla JS, no build step
install.sh      runs on the target: deps, build, PAM, systemd, firewall
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

## Not built yet

Deliberately out of scope for this pass: service and log viewers, storage,
networking, users, containers, multi-window terminals per session, drag-and-drop
upload, and an app launcher for other web UIs on the host.
