# Adding applications from Flathub

*Why a Flatpak on the host is the cheapest entry this catalog has, how to add
ten of them in an afternoon, and which ones to say no to.*

Where something below was measured or run, it says so. Where it describes
intended behaviour it says that too. Neither `cage` nor `wayvnc` is installed on
the machine this was written on, so nothing here that begins "it draws" or "it
opens" has been watched happening — the Flathub metadata, the package
availability and the tool's own output were all checked; the streamed transport
was not.

## The short version

| | |
| --- | --- |
| Find an app | `scripts/flathub-entry.py --search "photo editor"` |
| Draft its entry | `scripts/flathub-entry.py org.gimp.GIMP` |
| Add it | paste the `flathub!(…)` into `CATALOG` in `src/catalog.rs` |
| Ship it | `cargo build --release`, install, open |
| Say no when | it wants audio, a tray, or several gigabytes |

## Why Flathub pairs well with this project

A streamed entry is a Flatpak that runs on this host as the signed-in user,
under a headless `cage` compositor, with `wayvnc` serving RFB into a WebDesk
window. It is the third kind of entry in the catalog, after the container and
the adopted host service, and it exists because of four of the README's own
[Known limits](../README.md#known-limits) — downloads landing in an app
directory instead of `~/Downloads`, a passwordless root shell inside every
desktop container, `/home` mounted read-write into all of them, and a gigabyte
of shared memory each. A Flatpak on the host has none of those, and not because
they were fixed: because it is not a container.

What Flathub adds on top of that is five things this project would otherwise
have had to build for itself.

**One remote, one format, one command.** `flatpak install --system flathub
<id>`. There is no per-application packaging question — no tag to choose, no
`/config` convention to check for, no `PUID` to pass. `flatpak::provide()` is
the whole install path and it does not branch per entry.

**Per-app sandboxing that is finer-grained than the container's.** A container
desktop gets all of `/home`, read-write, shared with every other one. A Flatpak
gets what its manifest asked for, and the manifest is published metadata you can
read before you add the entry — `scripts/flathub-entry.py` prints it. Be honest
about the limit of this, though: plenty of applications ask for `filesystems:
host`, and one that does is no more confined on the filesystem question than the
containers were. The tool says so when it sees it. The difference that always
holds is *whose* files they are: the streamed app runs as the person who opened
it, so `~/Downloads` is theirs and not a shared app directory.

**Real per-user state.** `~/.var/app/<id>`, per user, created by Flatpak,
untouched by WebDesk. No state directory is invented, no volume is mounted, no
`config_at` is filled in, and two people with the same app open are not sharing
a profile.

**A genuine update path.** This is the one worth dwelling on. The README lists
"updating an installed app to a newer image" under
[Not built yet](../README.md#not-built-yet), and that is accurate: an installed
container entry stays on the image it was pulled with. The Flathub path simply
does not have that gap — `flatpak::update()` runs `flatpak update --system <id>`
and that is the entire feature. Note the contrast with the *other* Flatpak in
this catalog: term.hut ships as a bundle with no `--runtime-repo`, so its
installed origin is one no remote knows and `flatpak update` answers "Nothing to
do" forever, which is why `newest_bundle` exists. `FlatpakSource` is the field
that tells those two apart, and Flathub is the half that gets the cheap answer.

**Adding an app adds no dependency.** A Flathub entry sets `needs: &[]`. `cage`
and `wayvnc` are host-wide, probed once by `deps.rs`, and shared by every
streamed entry — so the tenth app you add this afternoon costs exactly what the
first one did, which is eight lines in `src/catalog.rs`.

## The workflow, end to end

### 1. Find the application

If you know the id, skip this. If you know what you want it to *do*:

```
$ scripts/flathub-entry.py --search "photo editor" --limit 5
"photo editor" -- the 5 desktop applications Flathub ranks highest.
This is a ranking and not a filter: it answers even when nothing matches.

  me.ahola.aphototoollibre
    A Photo Tool (Libre) -- Photo editor for Linux
      10 MiB  verified  org.kde.Platform/x86_64/6.10
  org.filmulator.Filmulator
    Filmulator -- Simple raw photo editor based on film development
      17 MiB  unverified  org.kde.Platform/x86_64/5.15-25.08
  org.gnome.Shotwell
    Shotwell -- Digital photo organizer
      28 MiB  verified  org.gnome.Platform/x86_64/50
  com.xnview.XnViewMP
    XnView MP -- View, convert and organize your images
     196 MiB  verified  org.freedesktop.Platform/x86_64/25.08
  io.github.ktgw0316.LightZone
    LightZone -- Camera RAW image processor
     215 MiB  unverified  org.freedesktop.Platform/x86_64/25.08

Sizes here are the application alone. Run this again with an id for the
runtime it sits on, the licence, the sandbox it asks for and an entry to paste.
```

The size on each line is the application without its runtime, which is the
number that matters when you are choosing between five of them: the runtime is
shared, so the second app on `org.gnome.Platform` costs its own size and nothing
else.

Two things about this search are worth knowing before you trust it. It **ranks
rather than filters** — measured: the endpoint reports 3315 hits for every
query, and a search for `zzzqqq` comes back with twenty-one perfectly real
applications. A result is the best match Flathub could find, not evidence that
anything matched. And the *first* result is frequently not the one you want; the
top hit for "photo editor" above is a 10 MiB application nobody has heard of,
while GIMP is not on the page at all. Search to discover an id, then judge the
application.

### 2. Draft the entry

```
$ scripts/flathub-entry.py org.gimp.GIMP
org.gimp.GIMP
  GNU Image Manipulation Program -- High-end image creation and manipulation
  licence    GPL-3.0+ AND LGPL-3.0+ (free)
  verified   yes -- the publisher controls gimp.org
  runtime    GNOME Application Platform version 50
  size       256 MiB of application, on 1023 MiB of runtime that every other
             app built on it shares
  window     1280 x 720 -- the shape of its own 1920x1080 screenshot
  slug       gimp   (the other candidate was `gnu-image-manipulation`)
  name       30 characters, which is a dock label nobody can read. The
             catalog's names are what an app is called, not what it is
             registered as -- shorten it.

    flathub!(
        "gimp",
        "GNU Image Manipulation Program",
        "org.gimp.GIMP",
        "a-box",
        "High-end image creation and manipulation.",
        1280 x 720,
    ),

  this tool cannot answer, and somebody has to by running it:
    - whether it is usable at a fixed initial resolution over RFB. Resize is
      the roughest edge of the streamed transport, and an app that lays itself
      out once will look wrong the moment the window changes.
    - whether its dialogs behave. `cage` holds one surface, so a second
      top-level window is a question about the compositor rather than about the
      app.
    - whether the tagline above reads like the rest of the catalog. It is
      Flathub's own summary with a full stop added, which is a starting point
      and not a sentence anybody wrote for this dock.
    - asks for the whole of `home` or `host`. Its sandbox is then no narrower
      than the containers' on the filesystem question -- it is still the *right
      user's* files, which is the point, but do not claim finer-grained
      confinement for this one.
    - asks for `devices=all`, which is every device node including input and
      USB, not just the render node.
    - asks to talk to 5 session bus names, starting with
      com.canonical.AppMenu.Registrar. A streamed session has a bus of its own
      but not the desktop services behind those names, so check what the app
      does when nothing answers.
    - the icon is `a-box`. If it deserves a mark of its own, add the Simple
      Icons slug to scripts/brand-icons.py, re-run it, and change the one line.

1 entry drafted. Nothing was written -- paste what you want into src/catalog.rs.
```

Nothing was written, deliberately. A script that edited `src/catalog.rs` would
be a script that could add an entry nobody read, which is the one property the
whole [fixed catalog](../README.md#why-the-catalog-is-fixed) argument depends
on.

The tool takes several ids at once, which is what makes ten in an afternoon
plausible:

```
$ scripts/flathub-entry.py org.kde.krita org.inkscape.Inkscape md.obsidian.Obsidian
```

### 3. Edit the two things it got approximately right

The draft is not the entry. Two fields always want a human:

- **The name** is Flathub's registered `<name>`, which for GIMP is *GNU Image
  Manipulation Program*. The catalog's names are dock labels — `Firefox`,
  `OnlyOffice`, `VSCodium`. Cut it to `GIMP`. The tool flags anything over
  twenty characters rather than truncating it, because where to cut is a
  judgement.
- **The tagline** is Flathub's `<summary>` with a full stop added. The catalog's
  taglines are sentences somebody wrote for this dock — "The browser, running on
  this host rather than on your machine." — not marketing copy lifted from a
  store page. Rewrite it.

The **window size** is a guess with a stated basis: 1280 wide, at the aspect
ratio of the application's own default screenshot, clamped to a height between
720 and 1000. It is what `cage` comes up at before the browser has said how big
its window is, so a wrong guess is a cosmetic wrong guess, not a broken entry.

### 4. Paste, build, install

```rust
    flathub!(
        "gimp",
        "GIMP",
        "org.gimp.GIMP",
        "a-box",
        "Image editing, running on this host as you.",
        1280 x 720,
    ),
```

Into `pub static CATALOG` in `src/catalog.rs`, then `cargo build --release` and
install as usual. From WebDesk: **Apps**, *Available*, the new entry, Install.
There is nothing to fill in — a streamed entry has no parameters, for the same
reason the Selkies desktops have none: every question it could ask has one
obviously right answer, and the answers come from the host and the session
rather than from a form.

*Intended behaviour, not observed here:* the install runs
`flatpak install --system flathub org.gimp.GIMP`, the app appears in the dock,
and opening it starts `webdesk-app@gimp.service` in your own systemd user
manager and streams the result into a window.

### 5. Give it a mark, if it deserves one

Every generated entry says `"a-box"`, and that is a real icon, not a
placeholder-shaped hole — an app can ship with it. If the application has a
recognisable mark, add its [Simple Icons](https://simpleicons.org) slug to
`ICONS` in `scripts/brand-icons.py`, re-run the script, commit the sprite, and
change the one line in the entry. Check the slug by eye: `brand-icons.py` has a
comment about `heliumbrowser` versus `helium` that exists because picking by
name alone ships the wrong company's logo.

## What makes a good candidate

The honest test is: *would you rather have this than the tab you already have
open?* Streaming an application costs a compositor, an RFB server and a video
connection per user. It has to be buying something.

**Good.** A single-window desktop application, under a gigabyte, that operates
on files in your home directory, that has no web version worth using, and that
you would otherwise SSH in and run over X forwarding. Image and vector editors,
document editors, RAW processors, CAD, IDEs — the things that are still
genuinely native in 2026. GIMP, Inkscape and Krita are the shape of this.

**No, and here is what to check.**

- **Size.** The catalog has a precedent and it is not theoretical: `intellij-idea`
  was removed on size alone. Nothing about it was broken; it unpacks to roughly
  9 GB, which was more than the free space on the filesystem the engine stores
  images in, so the entry most likely to fail its install was also the one whose
  failure would fill the disk every other service on the host writes to. The
  tool warns above 4 GiB counting the runtime once, and mentions `df` above 1
  GiB. `com.jetbrains.IntelliJ-IDEA-Community` on Flathub is 2.5 GiB of
  application on a 1.6 GiB runtime — 4.1 GiB — and the tool says out loud that
  this is the same decision.
- **Audio.** RFB carries none. A Flatpak that asks for `pulseaudio` will run
  here and be silent. For an image editor that is a beep nobody misses; for a
  media player it is the entire application. The tool reports the declared
  socket, but *declared* is not *needed* — the judgement is yours.
- **Dialogs and extra windows.** `cage` is a kiosk compositor holding one
  surface. An application whose real workflow is three floating palettes and a
  modal file chooser is asking the compositor a question this arrangement
  answers badly. This is the check that most needs running the thing.
- **A system tray.** There is no tray. An application that minimises to one
  instead of exiting will look like it vanished. The tool flags a manifest that
  asks for a status-notifier bus name.
- **X11 only.** An app with no `wayland` socket needs Xwayland under `cage`,
  which is one more component that has to be present and working. Krita declares
  `x11` and not `wayland`; that is not a refusal, it is a thing to test.
- **It is already a web app.** The clearest no of all. If the application ships
  a web interface, put *that* behind the proxy as a container entry or a host
  service and get a real HTML document, a working clipboard, sound, and no video
  encoder. term.hut is on the host precisely because it serves a web interface;
  streaming it would have been strictly worse.

`org.videolan.VLC` is a useful example of the tool refusing to be enthusiastic:
it flags PulseAudio, X11-only, *and* a status-notifier name in one run. VLC is
excellent software and a bad streamed entry.

## Why the entry still lives in the binary

Adding an app is still a code change, a build and a deploy. That is the same
gate the rest of the catalog has, and the reasoning is unchanged: installing
chooses what software lands on this host and what runs in a user's session, and
that is not a decision to hand to a browser. `flatpak::provide` takes an id from
a `&'static str` on a catalog entry, never from a request; the unit that starts
a streamed app is one template whose `ExecStart` gets a slug, and
`webdesk app-session <slug>` resolves that slug against the compiled-in catalog
and refuses anything else. A shell script doing that resolution would have been
a way to run any Flatpak on this host as anyone — the exact hole the fixed
catalog exists to close, arrived at from a new direction.

**But one thing genuinely changes, and it is worth being precise about it.** The
README says the requirement that decides catalog membership is *"an entry must
work when served from `/app/<slug>/` instead of `/`"*. That requirement does not
apply to a streamed entry at all. There is no proxy, no prefix, no
`X-Forwarded-Prefix`, no `base` template and no blank frame to diagnose; the
browser reaches the app over `/ws/rfb/<slug>` and what arrives is pixels. The
three-curls check the README asks you to run before adding a container entry has
nothing to test here.

So the gate stops being about compatibility and becomes purely about judgement:
is this worth streaming, is it small enough, and do you trust the publisher.
That is a real widening, and it is worth saying where the line now is. It is not
"anything on Flathub" — it is "anything on Flathub that somebody read the
metadata for, ran once, and committed". The build is still what decides.

## What it costs

The streamed transport is a real improvement on the container desktops for
identity, state and isolation. It is not free, and these are its edges.

- **No audio.** RFB carries pixels and input. There is no sound channel and
  adding one is not a configuration change.
- **Text-only clipboard.** The clipboard in RFB is a cut-text message and
  nothing else; there is no type negotiation in the base protocol. Copying an
  image out of a streamed editor and into something on your own machine is not a
  thing this transport does.
- **Resize is the roughest edge.** The compositor comes up at the size in the
  entry. Following the browser window means renegotiating the framebuffer, and
  applications differ enormously in how gracefully they take that. This is the
  first thing to try when evaluating a candidate.
- **Latency is VNC latency.** On a LAN it is fine. Over a WAN it is worse than a
  purpose-built encoder would be — the Selkies containers encode H.264 with
  VAAPI on this host (measured; see `docs/host-access.md`), and RFB is not
  competing with that on a slow link. The trade is deliberate: RFB is a much
  smaller thing to run and it removes four Known limits, and if you are on the
  LAN you will not notice.
- **`cage` and `wayvnc` are EPEL on the RHEL family, not base.** This is the
  practical risk on this project's primary deployment target. Checked against
  the Fedora package index rather than against a host: `wayvnc` is in EPEL 9
  (0.7.2) and EPEL 10 (0.9.0); **`cage` is in EPEL 10 only** (0.2.0, for 10.2
  through 10.4) — there is no EPEL 9 build of it at all. So on AlmaLinux or
  Rocky 10 both are one `dnf` away once EPEL is enabled, and on anything in the
  EL9 generation the compositor has to be built from source. Debian carries both
  in `main` from bookworm onward — `cage` 0.1.4 and `wayvnc` 0.5.0 in bookworm,
  0.2.0 and 0.9.1 in trixie, checked against `sources.debian.org`. Ubuntu
  inherits from Debian and is presumably the same; that was not checked.
  `deps.rs` probes for the binaries rather than for package names, which is the
  right shape because the package that provides one differs per distribution and
  the binary does not — but on EL9 there is no package to offer at all, and the
  honest answer there is a refusal with instructions, the same shape term.hut's
  `provision` already uses.
- **`flatpak` itself is not a build dependency.** A host that will only ever run
  container apps should not be made to carry it, so it is probed and offered
  like the rest.

## Reading list

- [`src/catalog.rs`](../src/catalog.rs) — the `flathub!` macro, `Streamed`, and
  `FlatpakSource`.
- [`scripts/flathub-entry.py`](../scripts/flathub-entry.py) — the tool, and its
  header comment on what it refuses to do.
- [README, *Why the catalog is fixed*](../README.md#why-the-catalog-is-fixed) —
  the gate this widens without removing.
- [`docs/host-access.md`](host-access.md) — what it took to make a *container*
  behave as though it were installed, which is the problem a streamed entry does
  not have.
