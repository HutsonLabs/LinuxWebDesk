# Giving a container app the host

*What it takes to make a containerised application behave as though it were
installed on the machine, which of those things are worth doing, and the
measurements behind each answer.*

Everything below was measured on the deployment host — AlmaLinux 10.2,
SELinux enforcing, Docker 29.7.2, AMD Radeon Vega 11 (Picasso), 8 cores —
against the images the catalog actually names. Where something could not be
measured it says so.

## The short version

| Mechanism | Verdict | Why |
| --- | --- | --- |
| The host's `/home` | **shipped, already** | An app with no `/home` has nothing to open |
| A render node | **shipped** | Without it, every frame is encoded on the CPU |
| The host's fonts | **shipped** | Missing scripts render as empty boxes |
| Host audio (`/dev/snd`) | **rejected** | Would send sound to the wrong machine |
| Host printing (CUPS socket) | **rejected, for now** | Mechanism works; the host has no printers |
| Host fonts *over* `/usr/share/fonts` | **rejected** | Measurably worse than doing nothing |
| The host's D-Bus | **rejected** | Collides with the image's own bus |
| Host supplementary groups | **rejected** | Too blunt for what it buys |
| The engine socket | **rejected, permanently** | It is root on the host |
| DNS / name resolution | **nothing to do** | Already identical to the host |

The pattern worth noticing: the things worth doing are the ones where the
container is *missing* something the host has. The things not worth doing are
the ones where the container already has its own, better answer — and where
"integrating" means overwriting it.

## What "as if installed directly" actually means here

It is worth being precise, because the phrase can mean two different things and
only one of them is achievable.

It does **not** mean the app runs on the host. Five of the six container entries
are a real browser or IDE rendering into a framebuffer, and the whole
proposition is that they are *contained*. Removing the container is not
integration; it is deletion of the feature.

It means the app should not be **surprised** by its surroundings. A natively
installed application finds the user's files where it expects them, the fonts
the rest of the system uses, and the hardware the machine has. A container
finds an empty home directory, its own private font set, and no GPU. Each of
those is a specific, closeable gap, and each is closed by a specific flag.

## 1. The render node — the largest single win

**The gap.** The desktop images do two expensive things: render an application's
UI, and encode every frame of it as H.264 to send to the browser. On this host
both were running in software, on a machine with a hardware video encoder that
was idle.

**Evidence, before.** From the running `webdesk-firefox`:

```
$ docker inspect webdesk-firefox --format '{{json .HostConfig.Devices}}'
[]
$ docker logs webdesk-firefox | grep -E 'CPU encoding|Mode:'
[Wayland] CPU encoding selected (use_cpu=true or encode_node_index=-1).
Stream settings active -> ... | Mode: H264 (CPU) FullFrame | ...
```

**Evidence, after**, same image and environment, with one render node added:

| | no device | render node |
| --- | --- | --- |
| `GL_VENDOR` / `GL_RENDERER` | `Mesa` / `llvmpipe (LLVM 21.1.8, 256 bits)` | `AMD` / `AMD Radeon Vega 11 Graphics (radeonsi, raven, ACO)` |
| encoder | `H264 (CPU) FullFrame` | `H264 (VAAPI) FullFrame` |
| capture path | `Readback path (encode thread) active` | `Zero-Copy path active` |

The renderer figures came from a surfaceless-EGL probe run inside both
containers, because the images ship no `glxinfo`. The encoder figures required
driving a real client: the decision is logged only once a client sends
`SETTINGS` with a `displayId` and then `START_VIDEO`.

The zero-copy line matters independently of the encoder. It removes a
GPU-to-CPU copy of the framebuffer on every frame, not just the encode cost.

**Why it was failing, traced rather than guessed.** In the image:

- `init-selkies-config/run:274` writes `DRI_NODE` only if a render node is
  present.
- `selkies.py:3247` sets `encode_node_index` from it, or **`-1`** when unset.
- `selkies.py:437` documents `-1` as disabling VA-API in the capture module.

So the CPU fallback was a direct consequence of the missing device. The image
default `SELKIES_ENCODER=x264enc,jpeg` is **not** the cause — it is unchanged
between the two runs.

**Render nodes only, never the card node.** `/dev/dri/cardN` is the modesetting
device; a container holding it can change what is on the physical monitor.
`/dev/dri/renderDN` is the compute and video half. Passing only the render node
gives results byte-for-byte identical to passing the whole directory, because
the compositor is headless and never does modesetting. The card node is dead
weight, so it is not passed.

**Exactly one node, and deliberately not named.** This is the subtlest finding,
and the one that changed the implementation twice — the second time by
overturning the first. The image's detection reads:

```sh
if [[ "${PIXELFLUX_WAYLAND}" == "true" ]] && [ -e "/dev/dri/renderD128" ] \
   && [ ! -e "/dev/dri/renderD129" ] && [ -z ${DRI_NODE+x} ]; then
```

Read alone, that looks like it gives up on any host whose nodes are not exactly
one `renderD128` — which argues for naming the node explicitly in `DRI_NODE`.
That argument is wrong, and only reading the *second* script shows why:
`init-video/run:38-45` globs `/dev/dri/renderD*` and sets `AUTO_GPU=true`
whenever `DRI_NODE` and `DRINODE` are both unset, and it runs after the first
(`init-video/dependencies.d/` contains `init-selkies-config`). Verified through
`/run/s6/container_environment/`: one node gives `DRI_NODE=/dev/dri/renderD128`
with `AUTO_GPU` unset; two nodes, or a `renderD129`-only container, gives
`DRI_NODE` unset and `AUTO_GPU=true`.

So the multi-GPU case was never unhandled — and naming the variable *suppresses
the scan that handles it*, via the `[ -z ${DRI_NODE+x} ]` guard. Setting it to a
node that is not right produces:

```
Failed to allocate GBM buffer. Falling back to Software Renderer (Pixman)
Failed to derive VAAPI device: Invalid argument. Falling back to CPU
```

WebDesk therefore passes the lowest-numbered node and sets **neither** variable.
Passing exactly one is what makes that safe: the container sees a single render
node, which is the case both the hardcoded path and the scan handle well.
Handing over the device and letting the image choose beats replacing a routine
that looks at what is really there with a value that can be wrong.

The general lesson is worth keeping: an init script that looks broken in
isolation may be one of several, and the guard that seems to be a bug may be a
handoff to the script that runs next.

**The group.** A device the container may reach but may not open is the same as
no device. On this host the render node happens to be `0666`, so nothing is
needed; on the ordinary `0660 root:render` host the gid is required. The
LinuxServer images arrange this themselves — `init-video/run` stats each node,
creates a matching group and adds `abc` to it, as root, before dropping
privileges — but WebDesk passes `--group-add` for a node that is not
world-accessible anyway, so that the device is usable without depending on the
image to run a fixup script.

**What this is not.** `--device` adds one character device to the container's
allowlist. It is not `--privileged`, which adds every device; not
`--device-cgroup-rule`, which names a whole major number; and not
`--security-opt`. A test enumerates those and fails if any ever appears.

## 2. The host's fonts — additive, never substitutive

**The gap.** A container can use only the fonts its image shipped. Neither set
contains the other: the Firefox image carries 415 families, the host 577, and
the host has script coverage the image lacks.

**The measurement that decides the mount point.** Binding the host's fonts *over*
`/usr/share/fonts` is worse than doing nothing. On the OnlyOffice image:

| | `fc-match Arial` | `fc-match "Times New Roman"` |
| --- | --- | --- |
| image alone | `Arial.ttf` | `Times_New_Roman.ttf` |
| host fonts over `/usr/share/fonts` | `NimbusSans-Regular.otf` | `NimbusRoman-Regular.otf` |

The image ships the metric-compatible originals and a typical Linux host does
not, so the "integration" destroys exactly the document fidelity it was meant to
improve.

`/usr/local/share/fonts` is listed in both image families' `/etc/fonts/fonts.conf`
and is scanned *in addition* to the system directory. Mounted there, on the
Firefox image, with no `fc-cache` run:

```
without mount: 415 families
with mount:    979 families
fc-match "Droid Sans Japanese"  ->  DroidSansJapanese.ttf   (host-only family)
fc-match "PT Sans"              ->  PTS55F.ttf              (host-only family)
fc-match Arial                  ->  NimbusSans-Regular.otf  (unchanged)
fc-match "Times New Roman"      ->  NimbusRoman-Regular.otf (unchanged)
```

Additive and non-destructive: the host's families become available, and the
image's own answers do not move. Fontconfig picks the directory up on first use,
so no command has to be run inside the container after it is created — which
matters, because WebDesk has no way to run one.

Read-only, because an application has no business writing the host's font
directory.

**And it must not be relabelled.** `z` relabels the *source* on the host,
recursively — read-only on the mount is no defence, because the relabelling
happens to the source and not to the view inside. Applied here it would rewrite
the labels on `/usr/share/fonts`, a system directory WebDesk does not own. It is
excluded by path, alongside `/home` and the engine socket, under a rule those
three were always instances of: **a mount WebDesk adds unasked never relabels
the host.**

## 3. SELinux: the kernel is only half the question

WebDesk decided whether to append `z`/`Z` by testing for
`/sys/fs/selinux/enforce`. That is the kernel's answer to a question only the
engine settles, and here they disagree:

```
$ getenforce
Enforcing
$ docker info --format '{{json .SecurityOptions}}'
["name=seccomp,profile=builtin","name=cgroupns"]
$ docker inspect webdesk-firefox --format 'label={{.ProcessLabel}}'
label=
```

Docker on this host has no SELinux support compiled in or enabled, so every
container runs with an empty process label. The live container nonetheless
carries `/var/lib/webdesk/appdata/firefox:/config:Z` — a relabelling of the
host's files for a confinement that is not being applied. The cost is paid and
the benefit is not collected.

Two consequences beyond the fix:

- `container_use_dri_devices` was **not** what permitted the GPU passthrough,
  and `container_use_devices=off` would **not** have blocked audio. Those
  booleans are inert while the engine ignores labels. Any argument resting on
  them is unsound on this host.
- The documented worry that the unrelabelled `/home` share might be unreadable
  does not apply here. Verified directly: a container sees `/home`, and
  `touch /home/homelab/.wd-probe` succeeds.

WebDesk now asks the engine, once per install. An engine that cannot be asked is
treated as not labelling: the failure that avoids is permanent and touches files
outside WebDesk, while the failure it risks is an app that cannot read its own
state directory, which shows up immediately.

## 4. What was rejected, and why

### Host audio — rejected on correctness

Not on security. Selkies already captures the application's audio and streams it
to the browser over the data websocket:

```
$ docker exec webdesk-firefox pactl list short sinks
1  output  module-null-sink.c  s16le 2ch 48000Hz
$ docker logs webdesk-firefox | grep -i pcmflux
[pcmflux] First non-silent audio chunk detected! Encoding...
```

The sinks are null-sinks whose monitors feed the encoder. Passing `/dev/snd`
would route sound to the **host's** speakers — the wrong room, for a user who is
sitting at a browser somewhere else. It would make the product worse.

### Host printing — mechanism proven, nothing behind it

The mechanism works with a single bind and no configuration:

```
$ docker run --rm -v /run/cups/cups.sock:/run/cups/cups.sock alpine:3 ... lpstat -r
scheduler is running
```

The images carry what would use it — `libcups.so.2` and the GTK
`libprintbackend-cups.so` are present in the Firefox, OnlyOffice and Inkscape
images — so a print dialog really would find the host's printers.

But the host has none:

```
$ lpstat -a
lpstat: No destinations added.
```

Shipping this would add a mount to every drawing app in exchange for reaching a
print spooler with no printers in it — a line in `docker inspect` that reads
like a fact and is not one. It is written down here rather than implemented, so
that a host that *does* have printers is one small change away.

### The host's D-Bus — rejected

The image runs its own system bus at the same path
(`/run/dbus/system_bus_socket`, with `dbus-daemon --system` inside), so binding
the host's would collide with it. The security cost is real and the benefit is
unclear, which is the wrong side of both trades.

### Host supplementary groups — not adopted, but the case is stronger than expected

A natively installed app runs with all of the user's groups; the container user
gets its `PGID` plus the image's own. The gap is real and reproducible — a
`root:wheel 0770` directory the host user can read is unreadable in the
container, and `--group-add 10` closes it exactly.

And it is **not** a sandbox loosening, which was worth measuring rather than
assuming:

| | `CapBnd` |
| --- | --- |
| default | `00000000a80425fb` |
| `--group-add 10` | `00000000a80425fb` (identical) |
| `--cap-add SYS_ADMIN` | `00000000a82425fb` |
| `--privileged` | `000001ffffffffff` |

Only `groups=` changes; no kernel privilege is granted. It is exactly the DAC
membership `initgroups()` gives a native login.

It is still not adopted, on a policy rather than a technical objection: these
apps are shared by everyone who can sign in to WebDesk, so adopting *one* user's
groups grants that user's group reach to every one of them. The render-node
group is passed because it is scoped to a single device. This is the same
reasoning that leaves the download directory alone, and it would change if apps
ever became per-user.

One loose end worth tidying separately: the images ship `abc` in `27(sudo)`,
`100(users)` and `990(docker)`, which are unearned memberships rather than
anything WebDesk grants. Nothing on this host is exposed by them — no file under
`/home` carries those gids, and gid 990 is `geoclue` here with no engine socket
mounted — but they are not doing any work either.

### The engine socket — rejected permanently

Unchanged, and worth restating in this context because it is the one thing here
that would undo all of it: a process that can talk to the engine can start a
container that bind-mounts `/`, so it is root. No shipping entry holds it and a
test keeps it that way.

### DNS — nothing to fix

Checked symmetrically, which is the only way this question has a meaningful
answer. The container inherits the host's resolver (`nameserver 100.100.100.100`,
Tailscale) and resolves public names. The short tailnet names it cannot resolve,
the **host cannot resolve either**. The container is not worse off than the
machine it runs on, so there is no gap.

## 5. The one gap left open

**Downloads land inside the app's state directory, not in a user's home.** The
images set `HOME=/config` and create `/config/Downloads`, and with no
`user-dirs.dirs` present that is where a browser saves. So a file downloaded in
the containerised Firefox lands in `/var/lib/webdesk/appdata/firefox/Downloads`
rather than in `~/Downloads`.

It is *reachable* — `/home` is mounted, so the save dialog can navigate to
`/home/<you>/Downloads` — but the default is wrong. Worse, `/config` is mode
0700 and root-owned on the host, so a downloaded file is somewhere the user
cannot get at without `sudo`.

**A fix exists and is proven**, so this is a decision rather than a limitation.
Measured, using GLib's special-directory lookup — the mechanism Firefox actually
uses:

| approach | result |
| --- | --- |
| default | `GLib DOWNLOAD = None` → falls back to `$HOME/Downloads` |
| `XDG_DOWNLOAD_DIR=…` | `None` — **the environment variable is ignored** |
| `~/.config/user-dirs.dirs` | resolves correctly |
| bind `~/Downloads` → `/config/Downloads` | works; a real headless download landed on the host as uid 1000 |

The image does not fight the bind mount: `init-adduser` runs a non-recursive
`lsiown abc:abc /config`, and `init-nginx` does `mkdir -p` on a directory that
already exists.

It is nonetheless left alone, on a policy objection rather than a technical one:
every fix picks a user, and that contradicts the principle the shared `/home`
was built on — an installed app is part of the host, not a possession of whoever
installed it. Binding one user's `~/Downloads` into an app that everyone with a
WebDesk session can open would put everybody's downloads in one person's folder.
A per-session download directory is not something this architecture can express
today.

Worth revisiting if the apps ever become per-user rather than per-host. If the
decision goes the other way, the bind mount is the option to take — it needs no
writes into the app's config tree, and cannot be silently overridden if the app
ever sets `browser.download.dir` itself.

## 6. Two things found on the way that are not host access

Both came out of auditing the catalog against the images, and both are somebody's
decision rather than a fact to record.

### An install can fill the root filesystem, and nothing stops it

The deployment host has **9.9 GB free on `/`**, which also carries the live
container stack's volumes. Compressed layer sizes from the amd64 manifests,
calibrated against the three images already resident (measured ratio ≈3.7×
compressed to on-disk):

| image | compressed | on disk |
| --- | --- | --- |
| firefox *(resident)* | 1.13 GB | 4.35 GB |
| onlyoffice *(resident)* | 1.69 GB | 6.09 GB |
| inkscape *(resident)* | 0.65 GB | 2.45 GB |
| **intellij-idea** *(absent)* | **2.47 GB** | **≈9.2 GB estimated** |
| helium *(absent)* | 1.18 GB | ≈4.4 GB estimated |
| vscodium-web *(absent)* | 0.20 GB | ≈0.75 GB estimated |

So pressing Install on IntelliJ IDEA today would try to write about 9.2 GB into
9.9 GB of headroom. There is no guard: `engine::pull` shells straight to
`docker pull`, and nothing in `src/` mentions `statvfs`, free space or `ENOSPC`.

A flat free-space precondition before the pull would turn a filesystem-full
outage into a readable refusal, in the shape `HostService::provision` already
uses for a missing prerequisite. The host is not actually short of space —
`docker system df` reports roughly 24 GB reclaimable across images, build cache
and volumes — so the host-side answer is a prune and the WebDesk-side answer is
the guard.

### Every desktop app hands a signed-in user a root shell in its container

LinuxServer documents this plainly for all five images: *"The web interface
includes a terminal with passwordless sudo access. Any user with access to the
GUI can gain root control within the container, install arbitrary software, and
probe your local network."*

`HARDEN_DESKTOP=true`, `DISABLE_SUDO` and `DISABLE_TERMINALS` are the documented
mitigations, and WebDesk sets none of them or mentions them anywhere — while the
term.hut entry goes out of its way to say the equivalent thing about the host.
The asymmetry is the part worth fixing.

It is a judgment call rather than an oversight, which is why it is written down
rather than changed: hardening costs Firefox, Helium, OnlyOffice and Inkscape
nothing anybody installs them for, but IntelliJ IDEA legitimately wants its
terminal. Note the blast radius is the container, not the host — but a container
with `/home` mounted read-write is not nothing.

Two related knobs look important and are **not**, both checked against the
scripts rather than the docs: `DISABLE_ZINK` is read only after
`which nvidia-smi` succeeds, and this host has no NVIDIA driver; `DISABLE_DRI3`
is read only by `svc-xorg`, whose first statement is `sleep infinity` under the
default `PIXELFLUX_WAYLAND=true`. Neither is worth setting here.

## Sources

Every figure above came from the deployment host or from the images themselves.
No claim here is taken from documentation alone; where a documented knob was
checked, it was checked against the script that reads it.
