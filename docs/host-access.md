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

**Exactly one node, named explicitly.** This is the subtlest finding, and the
one that changed the implementation twice. The image's detection reads:

```sh
if [[ "${PIXELFLUX_WAYLAND}" == "true" ]] && [ -e "/dev/dri/renderD128" ] \
   && [ ! -e "/dev/dri/renderD129" ] && [ -z ${DRI_NODE+x} ]; then
```

Two silent failures follow. Passing *every* render node found means a two-GPU
host fails the `! -e renderD129` guard, leaves `DRI_NODE` unset, and encodes on
the CPU — so a machine with more hardware would be slower than one with less,
with nothing saying why. And a host whose only node is `renderD129` fails the
first guard, so it would be handed a working GPU and go on ignoring it.

WebDesk therefore passes the lowest-numbered node only, and sets `DRI_NODE` and
`DRINODE` to name it. Neither variable is sent when there is no device.

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

### Host supplementary groups — rejected

A natively installed app runs with all of the user's groups; the container user
gets its `PGID` plus the image's own. `--group-add` would close that, but it
grants group reach across the whole shared `/home` for every app at once, to buy
access to files a user could equally reach by fixing the permissions on them.
The render-node group is passed because it is scoped to one device; a general
adoption of the user's groups is not.

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
`/home/<you>/Downloads` — but the default is wrong.

It is left alone deliberately, because every fix picks a user, and that
contradicts the principle the shared `/home` was built on: an installed app is
part of the host, not a possession of whoever installed it. Binding one user's
`~/Downloads` into an app that everyone with a WebDesk session can open would
put everybody's downloads in one person's folder. The alternative — a per-session
download directory — is not something this architecture can express today.

Worth revisiting if the apps ever become per-user rather than per-host.

## Sources

Every figure above came from the deployment host or from the images themselves.
No claim here is taken from documentation alone; where a documented knob was
checked, it was checked against the script that reads it.
