//! The catalog of installable applications.
//!
//! Every application WebDesk can install is described here, in the binary. It
//! is deliberately not a file on disk and not something the browser can add to:
//! a container is a way to run arbitrary code as whoever owns the engine, so
//! the set of things that may be run is a property of the build, reviewed like
//! any other code. "Install an app" means "choose one of these and fill in its
//! blanks", and nothing else.
//!
//! **Three kinds of entry, and only two of them are served over HTTP.** Most
//! apps are containers, reached under a path prefix; one entry is a service
//! adopted on the host and reached exactly the same way; and a third kind is
//! not served over HTTP at all, but drawn on this host and streamed into a
//! window here. Everything immediately below is about the first two, because a
//! prefix is only a question for something sitting behind `proxy.rs`.
//!
//! **Most apps live under a prefix; one owns an origin.** Apps are served from
//! `/app/<slug>/` on WebDesk's own origin (see `proxy.rs`), which is what lets a
//! container app share the session cookie and sit in an iframe at all. An
//! application that assumes it owns `/` emits root-absolute links that escape
//! its prefix and renders as a blank frame. So an entry either works under a
//! prefix, one of three ways --
//!
//! - **On its own.** The LinuxServer desktop images are Selkies underneath, and
//!   the Selkies client derives everything from `location.pathname` -- assets as
//!   `./assets/...` and its socket as `<base>websockets`. Nothing to configure.
//! - **By being told, in a variable.** `base` names an environment variable and
//!   the template to put the prefix into. `vscodium-web` needs
//!   `CODE_ARGS=--server-base-path=/app/vscodium-web` or its assets come out
//!   rooted at `/stable-<hash>/...`.
//! - **By being told, in a header.** `proxy.rs` sends `X-Forwarded-Prefix` on
//!   every request regardless. An app that reads it needs no entry-specific
//!   configuration at all. No entry relies on this today, but it is the cheapest
//!   of the three to satisfy, so it is worth checking for before reaching for
//!   `base`.
//!
//! -- or it sets `needs_origin` and is given a port of its own.
//!
//! A base path is safe to send to an app that treats it as *what to write into
//! the links it generates* and goes on answering at `/`. It is fatal to send to
//! an app that *routes* on it, because the proxy strips `/app/<slug>` before
//! forwarding, so the app is told to answer at a prefix it will never be sent.
//! `term.hut` is the second kind, which is why it is told nothing.
//!
//! **`needs_origin` is for the apps none of that reaches.** No entry ships with
//! it today -- `dockhand` did, and was the reason it exists. That app hardcoded
//! `/api/...` into every call its client made -- `fetch`, an `EventSource`, and
//! a WebSocket built from `location.host` -- with no base path to set and no
//! interest in `X-Forwarded-Prefix`. Under a prefix those all land on WebDesk's
//! own `/api/*` and the frame stays empty. There is nothing to configure for an
//! app of that shape, because it is not asking a question; it simply requires
//! the root of an origin. So it is given one: a second listener, on a port the
//! operator picks, serving that app at `/` and refusing anyone without a
//! WebDesk session.
//!
//! That costs an open port, which is a real cost and is why it is opt-in per
//! entry rather than the default. What it buys is that "can this app live under
//! a prefix" stops deciding what may be in the catalog at all.
//!
//! **And one entry is not a container at all.** `host` marks an application
//! that runs as a systemd unit on the machine, which WebDesk adopts rather than
//! installs: it is not pulled, not created, and not given a port, because all
//! of that happened before WebDesk heard of it. Everything downstream of the
//! loopback port is identical -- the proxy cannot tell the two apart, and does
//! not need to.
//!
//! This exists for the applications whose subject is the host. A terminal in a
//! container is a terminal into that container, which is the correct answer for
//! an editor and the wrong one for a shell. The cost is that the isolation is
//! gone, so it is arranged deliberately and by hand: see `HostService`, and
//! `systemd.rs` for why a unit name may only ever come from this file.
//!
//! **And a third kind is not served at all.** `streamed` marks a Flatpak that
//! runs on this host, as the person who opened it, and is drawn into a WebDesk
//! window: a headless `cage` holding exactly one application, `wayvnc` turning
//! that into RFB on a socket nothing but this process can open, and a canvas in
//! the browser at the other end. None of it goes through `proxy.rs`, so the
//! whole of the prefix argument above simply does not apply.
//!
//! What is striking about these entries is how little is left in them. There is
//! no port, because nothing listens on one -- the transport is a Unix socket
//! and `/ws/rfb/<slug>` is the entire address. There is no prefix, because
//! there is no proxy to be served under. There is no `/config`, because the
//! application keeps its state in `~/.var/app/<id>`, where Flatpak already put
//! it, per user. There is no `PUID`/`PGID`, because the process *is* the
//! signed-in user rather than a container being told to impersonate them. There
//! is no `TZ`, because it reads the host's clock, which is the clock the answer
//! would have been copied off. There is no shm size, because there is no
//! container whose `/dev/shm` was capped at 64 MB. And there is no render-node
//! flag, because a device on the host is not something you hand to a process on
//! the host -- it is simply already open to it.
//!
//! Those are not seven simplifications. They are one, said seven times. Every
//! one of them is a question about how to make a container resemble this
//! machine and this user closely enough to be useful, and an application that
//! is already running on this machine as this user has no such question to
//! answer. That is what makes this kind easy and repeatable in a way the other
//! two are not: adding one is a name, an id, an icon and a first window size,
//! `flathub!` is short because there is genuinely nothing else to say, and the
//! reviewing that matters is about whether the application is worth having
//! rather than about whether it can be made to work.
//!
//! It also settles four of the README's own Known limits outright, rather than
//! promising to. The container desktops put downloads in
//! `/var/lib/webdesk/appdata/<slug>/Downloads` instead of `~/Downloads`; every
//! one of the Selkies images ships a passwordless root shell; they mount
//! `/home` read-write; and each of them needs a gigabyte of `/dev/shm`. None of
//! those is a fix somebody has not got round to -- they are what a container
//! has to do to approximate a desktop session. A Flatpak on the host is not
//! approximating one. The user's real home, their fonts, their theme, the GPU
//! and a working `xdg-desktop-portal` are already there, so there is nothing to
//! bind in and nothing to loosen.
//!
//! The cost is the same one the host service pays, and it should be said in the
//! same breath: the container boundary is gone. A streamed application can
//! reach whatever its Flatpak sandbox permits, in the account of whoever
//! pressed open. That is the point of it rather than a flaw in it -- an
//! application whose subject is your files is worth nothing pointed at somebody
//! else's -- and it is why installing one is gated on the administrative group
//! while merely opening one is not.
//!
//! Each entry's port, volume and prefix behaviour below was read from the image
//! or observed by running it, not taken from documentation.
//!
//! **Two shapes, not one.** Most entries are LinuxServer images and share that
//! contract -- `/config` for state, `PUID`/`PGID` to own it, `TZ` for the clock.
//! `term.hut` is not one: it runs as a fixed user `hut`, keeps its state in
//! `/home/hut`, and would ignore `PUID`/`PGID` if we sent them. `lsio` is what
//! distinguishes them, so the installer stops pretending there is one contract.
//!
//! **An entry can cost too much to keep.** `intellij-idea` was here and is not
//! any more. Nothing about it was broken: the image is current, the port and
//! prefix behaviour were right, and it drew as well as the others. It unpacks to
//! roughly 9 GB, which on the deployment host was more than the free space on
//! the filesystem the engine stores images in -- so the one entry in the catalog
//! most likely to fail an install was also the one whose failure would take the
//! rest of the machine down with it, by filling the disk other services were
//! writing to. Size is a property of an entry like any other, and this is what
//! it looks like when it decides the answer.

/// What kind of blank a parameter is, and how it reaches the container.
///
/// `Choice` and `Toggle` are unused by the entries below and kept deliberately:
/// they are the vocabulary an entry is written in, and an entry that needs one
/// should not have to add the plumbing as well as itself.
#[allow(dead_code)]
pub enum Kind {
    /// A plain string, passed as `-e KEY=value`.
    Text,
    /// The same, but never echoed back to the browser once stored.
    Secret,
    /// One of a fixed set of strings.
    Choice(&'static [&'static str]),
    /// `"true"` or `"false"`.
    Toggle,
    /// A directory on the host, passed as `-v value:at[:ro]` rather than as an
    /// environment variable. Validated hard -- see `apps::validate`.
    HostPath { at: &'static str, ro: bool },
    /// A TCP port for WebDesk itself to listen on, for an entry with
    /// `needs_origin`. Never reaches the container: the app inside goes on
    /// serving the port it always did, and this is the public one in front of
    /// it. Validated as a number in the unprivileged range.
    Port,
}

pub struct Param {
    /// Environment variable name, or -- for `HostPath` -- just an identifier.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub default: &'static str,
    pub required: bool,
}

/// How an application is told what prefix it is being served under.
///
/// A template rather than a bare value because the two apps that need it want
/// different things in the same place: one takes the path on its own, the other
/// wants it inside a command-line argument.
pub struct Base {
    pub key: &'static str,
    /// `{prefix}` is replaced with `/app/<slug>` -- no trailing slash.
    pub template: &'static str,
}

/// A service that runs on the host itself rather than in a container.
///
/// The whole of this struct is the operator's side of a contract, written down
/// so both halves can be checked against each other. WebDesk does not install
/// the software, does not write the unit, and does not choose the port: it
/// adopts a service that is already on the machine and serves it under
/// `/app/<slug>/` like any other app. `provision` is the other half in prose --
/// what an operator has to have done before the entry will install.
///
/// **Why a host service exists at all.** A container is the right shape for an
/// application that only needs itself: the desktops draw a window, the editor
/// edits files in a volume, and the isolation costs them nothing. It is the
/// wrong shape for an application whose subject *is* the machine. A terminal in
/// a container is a terminal into that container -- the host's package manager,
/// its services and its filesystem are all on the other side of a boundary that
/// exists to keep them there. Run the same program as a unit on the host and
/// the shell it hands out is a shell on the host, which is what somebody
/// opening a terminal asked for.
///
/// That is a real transfer of power and it is deliberately awkward to arrange:
/// an operator has to install the software and write a unit as root, by hand,
/// before WebDesk will show the entry as installable at all.
pub struct HostService {
    /// The systemd unit that serves it, as `systemctl` would be given it.
    ///
    /// A `&'static str` so that it comes from the build and can never come from
    /// a request. `systemd.rs` explains why that is the line: a unit name that
    /// could be named by the browser would be a way to start anything on the
    /// host, which is a larger hole than any container in this catalog.
    pub unit: &'static str,
    /// The unit file WebDesk writes when this host has no such unit yet.
    ///
    /// A `&'static str` for exactly the reason `unit` is one, and it is the
    /// same rule doing more work: the *contents* of a unit decide what runs as
    /// whom, so a body assembled from a request would be a way to run anything
    /// on the host. Nothing here is interpolated except `{user}` and `{uid}`,
    /// and those come from the authenticated session rather than from the
    /// request body -- see `systemd::write_unit`.
    pub unit_body: &'static str,
    /// The Flatpak this service runs, if it is packaged as one.
    ///
    /// `None` for a host service WebDesk can only adopt. `Some` means the
    /// install may also *provide* it: the bundle is fetched and installed
    /// before the unit is written, so the service is never started against a
    /// binary that is not there.
    pub flatpak: Option<Flatpak>,
    /// What an operator must do when WebDesk cannot do it for them.
    ///
    /// Shown by the install refusal rather than buried in documentation,
    /// because the refusal is the exact moment somebody wants to read it. Now
    /// the last resort rather than the first: it is what a host that has no
    /// package manager we know, or whose operator declined, is told.
    pub provision: &'static str,
}

/// Where a Flatpak comes from, which decides what installing and updating mean.
///
/// Two shapes, and the difference is not cosmetic. A remote has a repository
/// behind it, so `flatpak update` is a real upgrade path and the whole install
/// is one command with no version to work out. A bundle has none, so installing
/// is downloading a file and so is upgrading -- which is why `newest_bundle`
/// exists at all.
pub enum FlatpakSource {
    /// Flathub, the remote nearly every desktop Flatpak is published to.
    ///
    /// `flatpak install --system flathub <id>` is the entire install, and
    /// `flatpak update --system <id>` the entire update. There is nothing
    /// per-entry to configure, which is the point: an entry naming a Flathub id
    /// is a name and an icon and nothing else.
    ///
    /// The remote URL is a constant in `flatpak.rs`, not a field here. A remote
    /// that could be named by an entry would be a remote that could be named by
    /// a request one refactor later, and the rule this file is built on is that
    /// the set of things that may be run is a property of the build.
    Flathub,
    /// A bundle from a GitHub repository's releases.
    ///
    /// For an application that publishes no remote to add. term.hut is built
    /// with `flatpak build-bundle` and no `--runtime-repo`, so the installed app
    /// reports an origin no `flatpak remotes` knows and `flatpak update` answers
    /// "Nothing to do" forever.
    Bundle { repo: &'static str },
}

/// An application that draws on this host and is streamed into a window here.
///
/// The third kind of entry, after the container and the adopted host service,
/// and the one that gets closest to running the application locally -- because
/// it *is* running locally. A Flatpak on the host has the signed-in user's real
/// home directory, their fonts, their theme, the machine's GPU and a working
/// `xdg-desktop-portal`, none of which a container can be given without being
/// handed the host.
///
/// What WebDesk adds is a way to see it: a headless `cage` holding exactly one
/// application, `wayvnc` turning that into RFB on a socket nothing but this
/// process can open, and `rfb.rs` carrying those bytes to a canvas in the
/// browser. No port is published, no image is pulled, no prefix is negotiated,
/// and no state directory is invented -- the app keeps its state where Flatpak
/// already puts it, in `~/.var/app/<id>`, per user.
///
/// **Installed once, run per user.** Installing is `--system`, host-wide, and
/// gated on the administrative group like every other install here: one copy on
/// disk, part of the machine like a package. Running is a systemd *user* unit in
/// the session of whoever opened it, because an application whose subject is
/// your files is worth nothing pointed at somebody else's. See
/// `systemd::APP_UNIT`.
pub struct Streamed {
    /// The Flatpak this entry runs.
    pub flatpak: Flatpak,
    /// The size of the headless output `cage` is started with, in pixels.
    ///
    /// A starting point rather than a limit: it is what the compositor comes up
    /// at before the browser has said how big its window is. Chosen per entry
    /// because a terminal and an image editor do not want the same first
    /// impression.
    pub width: u16,
    pub height: u16,
}

/// A Flatpak-packaged application WebDesk installs before starting its unit.
pub struct Flatpak {
    /// The application id, as `flatpak info` would be given it.
    pub id: &'static str,
    /// Where it comes from, and so what installing and updating mean.
    pub source: FlatpakSource,
    /// Host programs the unit's `ExecStart` needs, which are not the Flatpak.
    ///
    /// Probed by binary name rather than by package name, because the package
    /// that provides one differs per distribution and the binary does not.
    pub needs: &'static [Prereq],
}

/// A host program the service cannot start without, and how to get it.
///
/// The package name is per manager and deliberately incomplete: a manager with
/// no name here is one where nobody has checked what provides this, and
/// guessing would install the wrong thing or fail with a message about a
/// package that never existed. `None` there means the install refuses and
/// prints `provision` instead, which is the honest answer.
pub struct Prereq {
    /// The binary to look for on `PATH`.
    pub bin: &'static str,
    /// `dnf`/`yum` package name.
    pub dnf: Option<&'static str>,
    /// `apt-get` package name.
    pub apt: Option<&'static str>,
    /// `pacman` package name.
    pub pacman: Option<&'static str>,
    /// `zypper` package name.
    pub zypper: Option<&'static str>,
}

pub struct App {
    pub slug: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    /// Image reference without a tag; the tag is appended at install time.
    ///
    /// Empty for a `host` entry, which is not an image and is never pulled.
    pub image: &'static str,
    /// The port the application listens on: inside the container for an ordinary
    /// entry, on the host's loopback for a `host` one.
    ///
    /// The two are told apart by who chooses. A container's published port is
    /// picked by the installer out of `apps::PORT_LOW..=PORT_HIGH`, because
    /// nothing outside the container ever names it. A host service is already
    /// listening by the time WebDesk hears of it, so its port is fixed here and
    /// the operator's unit file has to agree -- which is why it is written into
    /// `provision` as well, in the command that has to match it.
    pub port: u16,
    pub icon: &'static str,
    /// `Some` when this entry is a service on the host rather than a container.
    ///
    /// The field that decides which half of `apps.rs` an entry goes through:
    /// with it set, nothing is pulled, nothing is created, no port is allocated
    /// and no directory is made, because all of that already happened without
    /// us. See `HostService`.
    pub host: Option<HostService>,
    /// `Some` when this entry is a Flatpak drawn on the host and streamed here.
    ///
    /// The third answer to the same question `host` answers, and mutually
    /// exclusive with both it and `image` -- a test keeps it that way. Nothing
    /// is pulled, no port is published and no prefix applies: this entry is
    /// reached over `/ws/rfb/<slug>`, not through `proxy.rs` at all.
    pub streamed: Option<Streamed>,
    /// `None` when the application works out its own prefix.
    pub base: Option<Base>,
    /// This application cannot live under a path prefix and must be served at
    /// the root of an origin of its own.
    ///
    /// `false` for everything that can be reached at `/app/<slug>/`, which is
    /// the cheaper arrangement and stays the default: one port to open, one
    /// origin, and cookies pinned per app by the proxy. `true` buys a listener
    /// of its own for an app that would otherwise be unusable -- see the module
    /// docs for what that costs, and `origin.rs` for how it is served.
    ///
    /// An entry that sets this must ask for a `Kind::Port`, since only the
    /// operator knows which port is free and reachable on their machine. A test
    /// keeps the two together.
    pub needs_origin: bool,
    /// Where this application keeps state, to be mounted from the host.
    /// `/config` for a LinuxServer image; `term.hut` uses its home directory.
    ///
    /// `None` for an application that keeps none. Mounting a directory an image
    /// never writes to is the same kind of noise as sending it a variable it
    /// ignores: it shows up in `docker inspect` reading like a fact about the
    /// image, and it is not one. No entry answers `None` today, but an image
    /// that declares no volume and writes no file is an ordinary thing to meet,
    /// and the alternative is inventing a state directory it will never use.
    pub config_at: Option<&'static str>,
    /// Settings that are ours to make rather than questions to ask.
    ///
    /// Distinct from `params`, which the browser fills in, and from the
    /// variables the installer already decides on its own (`PUID`, `TZ`,
    /// `TITLE`). These are entry-specific and have exactly one right answer, so
    /// putting them on a form would only be an invitation to get them wrong.
    pub env: &'static [(&'static str, &'static str)],
    /// Variables filled with a fresh random value at install time.
    ///
    /// For the keys an application needs but nobody should choose or retype: a
    /// session-signing secret is not a password, and a human-picked one is
    /// strictly worse than 32 bytes from the system generator. Recorded as a
    /// secret, so it is never echoed back to the browser.
    pub generated: &'static [&'static str],
    /// Follows the LinuxServer contract, so `TZ` means something.
    pub lsio: bool,
    /// Reads `PUID`/`PGID` to decide who owns the files it writes.
    ///
    /// Split from `lsio` because they are two different questions. Every
    /// LinuxServer image reads both, so no entry currently answers them
    /// differently -- but an image that reads the ids and has no opinion about
    /// the clock is an ordinary thing to meet, and sending a variable an image
    /// ignores is noise in `docker inspect` that reads like a fact about the
    /// image. Collapsing the two would mean lying about one of them.
    pub ids: bool,
    /// The host's container socket, bound into the container at this path.
    ///
    /// **This is the one thing in the catalog that hands out the host.** A
    /// process that can talk to the engine socket can start a container that
    /// mounts `/`, so it is root, and no amount of seccomp or user-namespace
    /// care inside the container changes that. It is here because an engine
    /// manager with no engine to manage is not an application, and it is a
    /// field on the entry rather than a question on a form so that it can only
    /// ever be true of an entry someone wrote it into deliberately.
    ///
    /// `None` for everything else, and a test keeps it that way.
    pub socket: Option<&'static str>,
    /// `--shm-size`. The Selkies desktop images run a real browser or office
    /// suite, and the 64 MB default `/dev/shm` is not enough for one -- Firefox
    /// tabs die, and the images ship `1gb` in their own documented examples.
    /// This is a tmpfs size, not a relaxation of the sandbox; nothing here ever
    /// loosens seccomp or drops a capability.
    pub shm: Option<&'static str>,
    /// This application renders its own interface on this host, rather than
    /// sending a web page for your browser to render on yours.
    ///
    /// One property with two consequences, which is why it is one field rather
    /// than a `gpu` and a `fonts`: an application drawing pixels here wants the
    /// hardware that draws them and the typefaces to draw them *with*, and an
    /// application that draws nothing here wants neither. Both are supplied by
    /// `engine::gpu` and `engine::font_mount`, and both check the host as well,
    /// so an entry that draws on a machine with no render node -- or no font
    /// directory -- installs exactly as it did before.
    ///
    /// True for the Selkies desktop applications, which are a real browser or
    /// IDE rendering into a framebuffer and then encoding every frame of it as
    /// H.264. Measured on the deployment host, with and without the device:
    /// `GL_RENDERER` moves from `llvmpipe` to `AMD Radeon Vega 11 Graphics
    /// (radeonsi)`, the encoder from `H264 (CPU)` to `H264 (VAAPI)`, and the
    /// capture from a readback path to a zero-copy one. Both arrangements work;
    /// one of them spends several cores of a machine that has silicon for
    /// exactly this sitting idle.
    ///
    /// False for an application that serves a web page. VSCodium draws in
    /// *your* browser, on your machine, with your GPU and your fonts; the
    /// container is a Node process holding a file tree, and a render node would
    /// be a device it never opens.
    pub draws: bool,
    /// `TITLE`, when the app should be told what to call itself rather than
    /// asked. `None` leaves whatever the image defaults to.
    pub title: Option<&'static str>,
    /// Speak TLS to this app's port.
    ///
    /// Only the loopback hop between the proxy and the container is encrypted
    /// by this, and the certificate is the self-signed one the image makes, so
    /// it is not verified and could not be -- what makes the hop private is
    /// that it never leaves the machine. It buys nothing a browser can observe:
    /// the browser talks to WebDesk's origin, and whether *that* is a secure
    /// context is decided by WebDesk's own listener (`tls.rs`), not here.
    pub tls: bool,
    pub notes: &'static str,
    pub params: &'static [Param],
}

/// The clock every LinuxServer image reads.
///
/// Not a question. The right answer is whatever the host is already set to, and
/// asking a user to retype it is asking them to get it wrong -- so the
/// installer reads it off the host and sends that. See `apps::host_timezone`.
pub const TZ_KEY: &str = "TZ";

/// One Selkies desktop application.
///
/// These take no parameters at all. Everything they would have asked has one
/// obviously-right answer: the title is the app's own name, the clock is the
/// host's, and the identity is the installer's. So installing one is a single
/// press with nothing to fill in.
///
/// The images do accept `PASSWORD`/`CUSTOM_USER` for a second sign-in of their
/// own, and it is deliberately not offered: reaching one of these already means
/// getting past WebDesk's session, so it would be a second lock on the same
/// door and one more thing to lose.
macro_rules! desktop {
    ($slug:literal, $name:literal, $image:literal, $icon:literal, $tagline:literal $(,)?) => {
        App {
            slug: $slug,
            name: $name,
            tagline: $tagline,
            image: $image,
            // 3001 is the https port. 3000 serves byte-identical content over
            // plain http and works, websocket included -- this is the https
            // port by request, not by necessity.
            port: 3001,
            tls: true,
            icon: $icon,
            // Selkies derives its own base from location.pathname.
            host: None,
            streamed: None,
            base: None,
            needs_origin: false,
            config_at: Some("/config"),
            env: &[],
            generated: &[],
            lsio: true,
            ids: true,
            socket: None,
            shm: Some("1g"),
            // A real browser or IDE, rendered into a framebuffer and encoded
            // frame by frame. Both halves of that want the render node.
            draws: true,
            // What the app calls itself in its own title bar. Fixed rather than
            // offered, which is what makes the install form empty -- and the
            // *name*, not the slug. The slug is a key: lowercase, hyphenated,
            // and chosen to be safe in a URL and a container name. Sending it
            // put `intellij-idea` in a title bar whose image would have said
            // `IntelliJ IDEA` on its own, so the one thing this variable exists
            // to improve, it made worse.
            title: Some($name),
            notes: "A desktop application, drawn in the browser. Its state lives in the app \
                    directory, so it is still there next time.",
            params: &[],
        }
    };
}

/// One Flathub application, drawn on this host and streamed into a window.
///
/// This is what the third kind of entry costs to write, and the shortness is
/// the whole argument for it. A container entry has to answer for a published
/// port, a state directory, `PUID`/`PGID`, a shared memory size, a clock, a
/// render node and whether the application tolerates a path prefix. None of
/// those questions exist here. The app runs on the host as the person who
/// opened it, so its state, its identity, its fonts, its GPU and its clock are
/// already the right ones, and there is no prefix because there is no proxy.
///
/// What is left is a name, an id, an icon and a first window size -- and of
/// those only the id is load-bearing. `scripts/flathub-entry.py` writes one of
/// these from an application id, which is the intended way to add an app.
macro_rules! flathub {
    ($slug:literal, $name:literal, $id:literal, $icon:literal, $tagline:literal,
     $w:literal x $h:literal $(,)?) => {
        App {
            slug: $slug,
            name: $name,
            tagline: $tagline,
            streamed: Some(Streamed {
                flatpak: Flatpak {
                    id: $id,
                    source: FlatpakSource::Flathub,
                    // Nothing beyond the compositor and the RFB server, and
                    // those are host-wide rather than per entry -- see
                    // `deps::RUNTIME`. An entry here needs no prerequisite of
                    // its own, which is the other half of why it is this short.
                    needs: &[],
                },
                width: $w,
                height: $h,
            }),
            icon: $icon,
            // Not an image, so nothing to pull and no port to publish. The
            // browser reaches this over `/ws/rfb/<slug>`, never through the
            // proxy, so every field the proxy reads is the empty answer.
            image: "",
            port: 0,
            host: None,
            base: None,
            needs_origin: false,
            config_at: None,
            env: &[],
            generated: &[],
            lsio: false,
            ids: false,
            socket: None,
            shm: None,
            // There is no container to give a device to. This runs on the host,
            // where the render node is simply present -- and `cage` will find
            // it the same way any other session compositor does.
            draws: false,
            title: None,
            tls: false,
            notes: "Runs on this host as you, with your home directory, your fonts and your \
                    GPU, and is drawn into this window. Its files are your files.",
            params: &[],
        }
    };
}

pub static CATALOG: &[App] = &[
    desktop!(
        "helium",
        "Helium",
        "lscr.io/linuxserver/helium",
        "a-helium",
        "A quieter Chromium, with the tracking taken out.",
    ),
    App {
        slug: "vscodium-web",
        name: "VSCodium",
        tagline: "VS Code without the telemetry, as a web editor rather than a drawn desktop.",
        image: "lscr.io/linuxserver/vscodium-web",
        // Verified: 8000, and no /config volume is declared even though the
        // application writes there -- so the mount matters more here, not less.
        port: 8000,
        icon: "a-vscodium",
        // Without this its assets come out rooted at /stable-<hash>/..., which
        // escapes the prefix and leaves a blank frame. Observed, not assumed.
        host: None,
        streamed: None,
        base: Some(Base { key: "CODE_ARGS", template: "--server-base-path={prefix}" }),
        needs_origin: false,
        config_at: Some("/config"),
        env: &[],
        generated: &[],
        lsio: true,
        ids: true,
        socket: None,
        shm: None,
        // A web editor, so the drawing happens in your browser on your machine.
        // This container is a Node process holding a file tree; a render node
        // is a device it would never open.
        draws: false,
        title: None,
        tls: false,
        notes: "Its extensions and settings live in the app directory.",
        params: &[
            Param {
                key: "DEFAULT_WORKSPACE",
                label: "Workspace folder",
                help: "Directory on the host to open. Mounted into the editor.",
                kind: Kind::HostPath { at: "/config/workspace", ro: false },
                default: "",
                required: false,
            },
            Param {
                key: "CONNECTION_TOKEN",
                label: "Connection token",
                help: "Optional. A secret the editor asks for; leave empty to run without one.",
                kind: Kind::Secret,
                default: "",
                required: false,
            },
            Param {
                key: "SUDO_PASSWORD",
                label: "sudo password",
                help: "Optional. Lets the editor's terminal use sudo inside the container only.",
                kind: Kind::Secret,
                default: "",
                required: false,
            },
        ],
    },
    App {
        slug: "term-hut-host",
        // The slug still says `-host` and the name no longer does. There is no
        // longer a container entry to be distinguished from -- it was removed
        // once this one could install itself -- so the name is just the
        // application's. The slug stays as it is because it is the key of the
        // record in `apps.json` and the name of the directory beside it: a host
        // that installed this yesterday would read as having nothing installed
        // if the key moved, and would then refuse to install it again over the
        // unit that is already running.
        name: "term.hut",
        tagline: "An agent-aware terminal, run as a service on this host -- so its shell is the host's.",
        // Not an image. Nothing is pulled and no container is created; what
        // this installs is a Flatpak and the unit that serves it.
        image: "",
        // Fixed, because a service that is already listening cannot be handed a
        // port by whoever adopts it. 6767 is `hut web`'s own default, so the
        // unit file in `provision` is the command anybody would have typed
        // anyway -- and it sits well outside the range `apps::free_port` hands
        // to containers, so the two allocators can never meet. Note that
        // `hut web` moves to a random port if this one is taken, and then
        // nothing here would reach it: `hut web status` is what says so.
        port: 6767,
        icon: "a-termhut",
        host: Some(HostService {
            unit: "term-hut-web.service",
            flatpak: Some(Flatpak {
                id: "com.hutsonlabs.termhut",
                source: FlatpakSource::Bundle { repo: "HutsonLabs/termhut.hutsonlabs.com" },
                // `xwfb-run`, from `xwayland-run`. GTK insists on a display
                // even though web mode never opens a window, and EL10 ships no
                // Xvfb at all -- `dnf provides */Xvfb` finds nothing -- so this
                // is the replacement rather than a preference. Probed because
                // the unit's ExecStart names it: without it the service is
                // written, started, and dies immediately.
                needs: &[Prereq {
                    bin: "xwfb-run",
                    dnf: Some("xwayland-run"),
                    apt: Some("xwayland-run"),
                    // Arch splits it differently and nobody has checked which
                    // package carries a headless Xwayland there. Refusing with
                    // instructions beats installing something that is not it.
                    pacman: None,
                    zypper: None,
                }],
            }),
            // Verified as the unit running on the deployment host, comments and
            // all -- it is written here rather than pasted into a doc so that
            // the thing that runs and the thing that is explained cannot drift.
            unit_body: TERM_HUT_UNIT,
            provision: "Install the term.hut Flatpak and add a system unit named \
                        term-hut-web.service that runs `hut web --host 127.0.0.1 --port 6767 \
                        --no-token` as the user whose shell this should be, then \
                        `systemctl enable --now term-hut-web.service`. --host 127.0.0.1 is \
                        the part that matters: WebDesk's sign-in is the only door to this \
                        terminal, and a service bound to every interface has a second one \
                        with no lock on it.",
        }),
        // Adopted, not drawn here: it serves its own web interface.
        streamed: None,
        // The same reason as the container entry above, and for the same
        // measured reason: term.hut *routes* on a base path, and the proxy
        // strips `/app/<slug>` before forwarding, so telling it one guarantees
        // a 404. Its hrefs are relative, so it needs no telling.
        base: None,
        needs_origin: false,
        // Its state is in the home directory of whoever the unit runs as, on
        // the host, where it already was. Nothing here to mount.
        config_at: None,
        env: &[],
        generated: &[],
        lsio: false,
        ids: false,
        socket: None,
        shm: None,
        // There is no container to give a device to. This one already runs on
        // the host, where every device is simply present.
        draws: false,
        title: None,
        tls: false,
        notes: "A desktop application designed for editing and terminal.",
        params: &[],
    },
    // The streamed shelf. Everything from here down runs on this host as the
    // person who opened it and is drawn into a window here, so each entry is a
    // name, an id, an icon and a first window size and nothing else -- see
    // `flathub!` for why there is nothing else to say.
    //
    // Size still decides what may be here, exactly as it did for
    // `intellij-idea`, but it decides on a different arithmetic. A Flathub
    // install is an application plus a runtime, and the runtime is shared: the
    // first GNOME entry pays about 900 MB for `org.gnome.Platform` and every
    // GNOME entry after it pays only for itself. So the shelf costs far less
    // than the sum of its parts, and the entries worth arguing over are the
    // ones that bring a runtime -- or a Java runtime -- nothing else will use.
    //
    // Every one of these now names its own mark rather than `a-box`. Seven come
    // from Simple Icons through `scripts/brand-icons.py`; Remmina and Disk
    // Usage Analyzer are not in that set and sit in the sprite by hand. `a-box`
    // stays as the fallback the UI reaches for when an installed app's catalog
    // entry has gone away, which is the only case left that needs it. An entry
    // naming an icon that is not in `ui/ui-icons.svg` would draw a blank
    // square, and `every_icon_the_catalog_names_is_in_the_sprite` is what
    // catches that before somebody opens the Apps window.
    // The three that used to be LinuxServer images and are not any more. Each
    // one loses a multi-gigabyte pull, a passwordless root shell, a share of
    // `/home` handed to every container on the box, and a gigabyte of shared
    // memory -- four of the Known limits, gone by not being a container rather
    // than by being fixed. What each one gains is your actual home directory,
    // the resolution following the window, and a zoom.
    //
    // Helium is not among them because Flathub has no Helium. It is still an
    // image, and still the only entry here that draws without any of this.
    flathub!(
        "firefox",
        "Firefox",
        "org.mozilla.firefox",
        "a-firefox",
        "The browser, running on this host rather than on your machine.",
        // A browser is the one application where the window is the point, so it
        // opens at the size the rest of the working entries do rather than at
        // the 1280x720 its own screenshot suggests.
        1600 x 1000,
    ),
    flathub!(
        "inkscape",
        "Inkscape",
        "org.inkscape.Inkscape",
        "a-inkscape",
        "Vector drawing, for the SVGs this desktop is drawn with.",
        // 295 MB on the GNOME runtime the rest of this shelf already pays for,
        // against roughly 3 GB unpacked as an image. The cheapest of the three
        // to move and the one whose files most want to be the host's.
        1600 x 1000,
    ),
    flathub!(
        "onlyoffice",
        "OnlyOffice",
        "org.onlyoffice.desktopeditors",
        "a-onlyoffice",
        "Documents, spreadsheets and slides, close to the shapes Office makes.",
        // 1.2 GB on the Freedesktop runtime, where the image was around 6 GB
        // unpacked -- the entry the README singles out for checking `df` before
        // installing. It is still the largest thing here and no longer the kind
        // of large that decides anything.
        //
        // `org.onlyoffice.desktopeditors`, so the id's own tail would make the
        // slug `desktopeditors`. It is `onlyoffice` instead: a slug is what a
        // person types and what a container was once named, and the entry this
        // replaces was called that.
        1600 x 1000,
    ),
    flathub!(
        "gimp",
        "GIMP",
        "org.gimp.GIMP",
        "a-gimp",
        "Photo and image editing, on the machine the images are already on.",
        // Roughly 1.3 GB installed, most of which is the GNOME runtime the rest
        // of this shelf then reuses for nothing. It earns the space by being
        // the half of the drawing story Inkscape is not: that entry makes
        // vectors, this one edits pixels, and between them a screenshot taken
        // on this host can be cropped and the logo next to it redrawn without
        // either file being downloaded, edited elsewhere and uploaded back.
        1600 x 1000,
    ),
    flathub!(
        "dbeaver",
        "DBeaver",
        "io.dbeaver.DBeaverCommunity",
        "a-dbeaver",
        "A database client, on the side of the firewall the database is on.",
        // Roughly 800 MB, because it carries a Java runtime of its own and
        // shares nothing with the GNOME entries around it -- the most expensive
        // thing here, and still the least arguable. A Postgres or MySQL bound
        // to 127.0.0.1, which is how it ought to be bound, is reachable from
        // exactly one machine and this is that machine. What people do instead
        // is open an SSH tunnel from a laptop, which is the same access with a
        // second credential to manage and a step to forget.
        1600 x 1000,
    ),
    flathub!(
        "remmina",
        "Remmina",
        "org.remmina.Remmina",
        "a-remmina",
        "RDP, VNC and SSH out to the other machines this host can see.",
        // Small, and the only entry here whose subject is not this host. A
        // server usually sits on a segment a laptop cannot reach: the
        // hypervisor's management interface, a switch, the Windows box holding
        // the licence server. Streaming a remote-desktop client from inside
        // that segment makes WebDesk the jump host, which is a thing operators
        // otherwise build on purpose and then have to maintain.
        1280 x 800,
    ),
    flathub!(
        "baobab",
        "Disk Analyzer",
        "org.gnome.baobab",
        "a-baobab",
        "Where the disk went, as a picture rather than a column of numbers.",
        // A few megabytes, and it is on this shelf because of `intellij-idea`.
        // The paragraph above about size describes an install that could have
        // filled the filesystem other services were writing to; this is the
        // tool for the morning after one does. `du -sh` reaches the same answer
        // eventually, one directory at a time, and the difference is that this
        // shows the whole tree at once -- which matters most in the case where
        // you do not yet know where to look.
        1100 x 750,
    ),
    flathub!(
        "localsend-app",
        "LocalSend",
        "org.localsend.localsend_app",
        "a-localsend",
        "Send files to the machines beside this one, with no share to set up.",
        // 55 MB on the Freedesktop runtime OnlyOffice already paid for, which
        // makes it near enough free. It answers the question the Files window
        // raises and cannot answer itself: getting a file onto this host from
        // a laptop on the same segment, without a share to export, an upload
        // form to build or an `scp` incantation to get right.
        //
        // Note which way it looks. The peers it discovers are whatever *this
        // host* can see on its own network, not what the machine you are
        // sitting at can see. That is the useful direction here -- it is how a
        // file reaches a server that has no share -- and it is the surprising
        // one everywhere else, so it is worth knowing before the list of
        // devices is not the list you expected.
        //
        // It asks for a status-notifier bus name and there is no tray in this
        // compositor to give it. Closing the window ends the app rather than
        // hiding it, so it receives while it is open and not otherwise.
        1280 x 944,
    ),
    flathub!(
        "bitwarden",
        "Bitwarden",
        "com.bitwarden.desktop",
        "a-bitwarden",
        "The password vault, on the host where the passwords get used.",
        // 487 MB of Electron on the Freedesktop runtime: the largest thing
        // here after OnlyOffice, and the one whose real cost is not the disk.
        // A vault opened here is decrypted in a process on this machine, in
        // your own user session. That is the same trust already placed in the
        // host by having a shell on it, and it is still worth saying out loud,
        // because a password manager is exactly the application where "it runs
        // on the server" stops being an implementation detail.
        //
        // Two consequences to know before reaching for it. Its manifest asks
        // for `devices=all`, every device node this host has rather than the
        // render node, and a hardware key is plugged into *this* machine and
        // not into the one you are sitting at -- so FIDO2 unlock is the host's
        // key or it is nobody's. And it wants a tray it cannot have, so
        // closing the window ends it instead of minimising it, which for a
        // vault is the better of the two outcomes.
        1280 x 768,
    ),
];

/// The unit `term-hut-host` installs, and the one already running on the
/// deployment host it was read back from.
///
/// `{user}` and `{uid}` are the only substitutions, and `systemd::write_unit`
/// is the only place they are made. Both describe the person who pressed
/// Install -- taken from their session, never from the request -- because the
/// whole proposition of this entry is a shell that is theirs.
///
/// One consequence is not obvious and is not a bug here: `xwfb-run` starts a
/// headless `mutter`, which claims `/run/user/{uid}/wayland-0`. GDK tries
/// Wayland first and falls back to that socket when `WAYLAND_DISPLAY` is
/// unset, so GTK apps launched in an *X11* session for this same user will
/// bind to this invisible monitor and their windows will never appear. A
/// GDM-started Wayland session is immune, since it exports an explicit
/// `WAYLAND_DISPLAY`. On a host where that matters, `GDK_BACKEND=x11` in the
/// X11 session is the fix -- not a change to this unit, which needs the
/// compositor it starts.
pub const TERM_HUT_UNIT: &str = r#"[Unit]
Description=term.hut in web mode, served to WebDesk on loopback
Documentation=https://term-hut.hutsonlabs.com
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User={user}

# A system unit gets no login session, but the Flatpak's whole value here is
# flatpak-spawn --host, which talks to the portal on the *user* bus. Lingering
# (loginctl enable-linger {user}) keeps /run/user/{uid} and its bus alive with
# nobody logged in; these two point the service at them.
Environment=XDG_RUNTIME_DIR=/run/user/{uid}
Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus

# GTK insists on a display even though web mode never opens a window, and
# EL10 ships no Xvfb at all. xwfb-run is the replacement -- a headless Wayland
# compositor plus Xwayland.
#
# --host 127.0.0.1 is the part that matters. WebDesk's sign-in is the only door
# to this terminal; bound to every interface it would have a second one with no
# lock on it. --no-token for the same reason: reaching this already means
# getting past WebDesk's session.
ExecStart=/usr/bin/xwfb-run -- /usr/bin/flatpak run com.hutsonlabs.termhut web --host 127.0.0.1 --port 6767 --no-token

# systemd cannot reach this app on its own. `flatpak run` hands the app to the
# session helper, which puts it in a systemd *scope* of its own -- outside this
# service's cgroup -- so stopping the service kills only xwfb-run and the
# launcher and leaves the terminal running and holding port 6767. The next
# start then finds a live instance, returns in under a second, and the unit
# goes `inactive (dead)` while the *old* build goes on serving. That is not a
# restart; it is a silent no-op that pins whatever version started first.
#
# `flatpak kill` reaches into the scope by app id, which is the one handle that
# does work. On Stop so the service really stops, and before Start (leading `-`
# so "nothing to kill" is not a failure) so a start always begins from nothing
# and `flatpak run` stays in the foreground where systemd can supervise it.
ExecStartPre=-/usr/bin/flatpak kill com.hutsonlabs.termhut
ExecStop=-/usr/bin/flatpak kill com.hutsonlabs.termhut

Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
"#;

pub fn find(slug: &str) -> Option<&'static App> {
    CATALOG.iter().find(|a| a.slug == slug)
}

impl App {
    /// The value this app's base variable should carry, given its prefix.
    pub fn base_value(&self, prefix: &str) -> Option<(&'static str, String)> {
        self.base.as_ref().map(|b| (b.key, b.template.replace("{prefix}", prefix)))
    }

    /// The parameters this entry asks for. Everything else the container needs
    /// -- the clock, the title, the identity -- is decided by the installer, so
    /// an entry with nothing here installs on one press.
    pub fn all_params(&self) -> impl Iterator<Item = &Param> {
        self.params.iter()
    }
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Secret => "secret",
            Kind::Choice(_) => "choice",
            Kind::Toggle => "toggle",
            Kind::HostPath { .. } => "path",
            Kind::Port => "port",
        }
    }
}

/// The catalog as the browser sees it. Secrets carry no value, only a shape.
pub fn as_json() -> serde_json::Value {
    let apps: Vec<_> = CATALOG
        .iter()
        .map(|a| {
            let params: Vec<_> = a
                .all_params()
                .map(|p| {
                    let mut v = serde_json::json!({
                        "key": p.key,
                        "label": p.label,
                        "help": p.help,
                        "kind": p.kind.name(),
                        "default": p.default,
                        "required": p.required,
                    });
                    if let Kind::Choice(opts) = p.kind {
                        v["options"] = serde_json::json!(opts);
                    }
                    if let Kind::HostPath { ro, .. } = p.kind {
                        v["readonly_mount"] = serde_json::json!(ro);
                    }
                    v
                })
                .collect();
            serde_json::json!({
                "slug": a.slug,
                "name": a.name,
                "tagline": a.tagline,
                "image": a.image,
                "icon": a.icon,
                "notes": a.notes,
                "params": params,
                // What the row under the name says for an entry with no image
                // to name -- and, when the install is refused, what to do
                // about it. `null` for a container, which is most of them.
                "host": a.host.as_ref().map(|h| serde_json::json!({
                    "unit": h.unit,
                    "provision": h.provision,
                })),
                // Which transport this app uses, which the desk has to know
                // before it opens anything: a container or an adopted service
                // is an iframe at a URL, and a streamed entry is a canvas on a
                // WebSocket, and nothing about the two arrangements is shared
                // past the window frame. `null` means the ordinary kind, so the
                // browser needs no new branch for the entries that already
                // worked -- the same shape `host` uses, for the same reason.
                //
                // The id travels because the window's own chrome names it.
                // "This is org.gimp.GIMP, running on this host as you" is the
                // one fact about a streamed app that a person cannot see for
                // themselves once it is drawn, and it is the fact that decides
                // whether they should be typing anything into it. The size
                // travels because the canvas has to exist before the first
                // frame arrives, and creating it at a guessed size means the
                // first thing anybody sees is the window resizing itself.
                "streamed": a.streamed.as_ref().map(|s| serde_json::json!({
                    "flatpak": s.flatpak.id,
                    "width": s.width,
                    "height": s.height,
                })),
            })
        })
        .collect();
    serde_json::json!({ "apps": apps })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three kinds are three answers to one question -- how does this
    /// application get run -- so an entry gives exactly one of them. Both
    /// halves of that matter and they fail differently. An entry with two set
    /// is installed as whichever `apps.rs` checks for first and carries the
    /// other kind's fields as dead weight: a container that also named a
    /// Flatpak would pull the image, start it, and never mention that it had
    /// ignored the id sitting beside it. An entry with none is worse in the
    /// quiet way -- it appears in the Apps window, is offered as installable,
    /// and there is no path through the installer that could do anything at all
    /// with it.
    #[test]
    fn an_entry_is_exactly_one_of_the_three_kinds() {
        for a in CATALOG {
            let kinds = [!a.image.is_empty(), a.host.is_some(), a.streamed.is_some()];
            let n = kinds.iter().filter(|k| **k).count();
            assert!(
                n <= 1,
                "{} is {n} kinds of entry at once, and only one of them would run",
                a.slug
            );
            assert_eq!(n, 1, "{} is none of the three kinds, so nothing would install it", a.slug);
        }
    }

    /// A streamed entry is described by what it is *not*, the same way a host
    /// entry is, and the list is longer because a Flatpak on the host is
    /// further from a container than an adopted service is. Every field here is
    /// one the installer or the proxy would act on if it were set, and not one
    /// of them has anywhere to act: the port would be dialled and nothing is
    /// listening, the `/config` would be mounted into a container that does not
    /// exist, the `PUID` would be sent to a process that has a real uid
    /// already. All three failures are silent, which is why the empty answers
    /// are asserted here rather than discovered later.
    #[test]
    fn a_streamed_entry_carries_the_empty_answers() {
        for a in CATALOG.iter().filter(|a| a.streamed.is_some()) {
            assert!(a.image.is_empty(), "{} is drawn on the host but names an image", a.slug);
            assert_eq!(a.port, 0, "{} publishes a port nothing is listening on", a.slug);
            assert!(!a.needs_origin, "{} would be given a listener of its own", a.slug);
            assert!(a.config_at.is_none(), "{} would be given a mount", a.slug);
            assert!(a.env.is_empty(), "{}'s environment would go nowhere", a.slug);
            assert!(a.generated.is_empty(), "{} would generate a key for nobody", a.slug);
            assert!(a.params.is_empty(), "{} asks a question it cannot apply", a.slug);
            assert!(a.socket.is_none(), "{} would be given the engine socket", a.slug);
            assert!(a.shm.is_none(), "{}'s shm size would go nowhere", a.slug);
        }
    }

    /// The slug is the one string an entry contributes to places that are not
    /// Rust, and it reaches five of them: the path `/app/<slug>/`, the socket
    /// `/ws/rfb/<slug>`, the container's name, the directory under `appdata`,
    /// and the key of the record in `apps.json`. Two entries sharing one would
    /// collide in every one of those, and not by failing -- the second install
    /// would find the first's record and either refuse or adopt it, and the
    /// proxy would route on a key that names two applications. A character
    /// outside `[a-z0-9-]` is the same problem spread thinner, because each of
    /// the five would want it escaped differently and only some of them would
    /// say so when it was not.
    #[test]
    fn every_slug_is_unique_and_safe_everywhere_it_is_used() {
        let mut seen = std::collections::HashSet::new();
        for a in CATALOG {
            assert!(seen.insert(a.slug), "two entries are both called {}", a.slug);
            assert!(!a.slug.is_empty(), "{} has an empty slug", a.name);
            assert!(
                a.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not safe as a path, a container name and a directory at once",
                a.slug
            );
        }
    }
}
