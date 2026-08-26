//! The catalog of installable applications.
//!
//! Every application WebDesk can install is described here, in the binary. It
//! is deliberately not a file on disk and not something the browser can add to:
//! a container is a way to run arbitrary code as whoever owns the engine, so
//! the set of things that may be run is a property of the build, reviewed like
//! any other code. "Install an app" means "choose one of these and fill in its
//! blanks", and nothing else.
//!
//! **Subpath tolerance decides membership.** Apps are served from `/app/<slug>/`
//! on WebDesk's own origin (see `proxy.rs`), which is what lets a container app
//! share the session cookie and sit in an iframe at all. An application that
//! assumes it owns `/` emits root-absolute links that escape its prefix and
//! renders as a blank frame. So an entry earns its place by working under a
//! prefix, one of two ways:
//!
//! - **On its own.** The LinuxServer desktop images are Selkies underneath, and
//!   the Selkies client derives everything from `location.pathname` -- assets as
//!   `./assets/...` and its socket as `<base>websockets`. Nothing to configure.
//! - **By being told.** `base` names an environment variable and the template to
//!   put the prefix into. `vscodium-web` needs
//!   `CODE_ARGS=--server-base-path=/app/vscodium-web` or its assets come out
//!   rooted at `/stable-<hash>/...`; `term.hut` takes `HUT_BASE_PATH` directly.
//!
//! Each entry's port, volume and prefix behaviour below was read from the image
//! or observed by running it, not taken from documentation.
//!
//! **Two shapes, not one.** Most entries are LinuxServer images and share that
//! contract -- `/config` for state, `PUID`/`PGID` to own it, `TZ` for the clock.
//! `term.hut` is not one: it runs as a fixed user `hut`, keeps its state in
//! `/home/hut`, and would ignore `PUID`/`PGID` if we sent them. `lsio` is what
//! distinguishes them, so the installer stops pretending there is one contract.

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

pub struct App {
    pub slug: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    /// Image reference without a tag; the tag is appended at install time.
    pub image: &'static str,
    /// The port the application listens on *inside* the container.
    pub port: u16,
    pub icon: &'static str,
    /// `None` when the application works out its own prefix.
    pub base: Option<Base>,
    /// Where this application keeps state, to be mounted from the host.
    /// `/config` for a LinuxServer image; `term.hut` uses its home directory.
    pub config_at: &'static str,
    /// Follows the LinuxServer contract, so `PUID`/`PGID`/`TZ` mean something.
    pub lsio: bool,
    /// `--shm-size`. The Selkies desktop images run a real browser or IDE, and
    /// the 64 MB default `/dev/shm` is not enough for one -- Firefox tabs die
    /// and IntelliJ fails to start. This is a tmpfs size, not a relaxation of
    /// the sandbox; nothing here ever loosens seccomp or drops a capability.
    pub shm: Option<&'static str>,
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
            base: None,
            config_at: "/config",
            lsio: true,
            shm: Some("1g"),
            // What the app calls itself in its own title bar. Fixed to the slug
            // rather than offered, which is what makes the install form empty.
            title: Some($slug),
            notes: "A desktop application, drawn in the browser. Its state lives in the app \
                    directory, so it is still there next time.",
            params: &[],
        }
    };
}

pub static CATALOG: &[App] = &[
    desktop!(
        "firefox",
        "Firefox",
        "lscr.io/linuxserver/firefox",
        "a-firefox",
        "The browser, running on this host rather than on your machine.",
    ),
    desktop!(
        "helium",
        "Helium",
        "lscr.io/linuxserver/helium",
        "a-helium",
        "A quieter Chromium, with the tracking taken out.",
    ),
    desktop!(
        "onlyoffice",
        "OnlyOffice",
        "lscr.io/linuxserver/onlyoffice",
        "a-onlyoffice",
        "Documents, spreadsheets and slides, close to the shapes Office makes.",
    ),
    desktop!(
        "inkscape",
        "Inkscape",
        "lscr.io/linuxserver/inkscape",
        "a-inkscape",
        "Vector drawing, for the SVGs this desktop is drawn with.",
    ),
    desktop!(
        "intellij-idea",
        "IntelliJ IDEA",
        "lscr.io/linuxserver/intellij-idea",
        "a-intellij",
        "The JetBrains IDE, with its indexes kept on the host.",
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
        base: Some(Base { key: "CODE_ARGS", template: "--server-base-path={prefix}" }),
        config_at: "/config",
        lsio: true,
        shm: None,
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
        slug: "term-hut",
        name: "term.hut",
        tagline: "An agent-aware terminal, served to the browser.",
        image: "ghcr.io/hutsonlabs/term.hut",
        // From docker/entrypoint.sh: --port "${HUT_PORT:-6767}". The other
        // exposed port, 6768, is mDNS workspace sync, which wants host
        // networking and so cannot work behind this proxy at all.
        port: 6767,
        icon: "a-termhut",
        base: Some(Base { key: "HUT_BASE_PATH", template: "{prefix}" }),
        // Runs as a fixed user `hut`; its home is the volume, not /config.
        config_at: "/home/hut",
        lsio: false,
        shm: None,
        title: None,
        tls: false,
        notes: "Signs in with a token by default. It is minted on first run and printed in the \
                container log; set one below to choose it yourself, or turn the token off.",
        params: &[
            Param {
                key: "HUT_TOKEN",
                label: "Access token",
                help: "Optional. Leave empty and one is generated on first run.",
                kind: Kind::Secret,
                default: "",
                required: false,
            },
            Param {
                key: "HUT_NO_TOKEN",
                label: "No token at all",
                help: "Rely only on WebDesk's own sign-in. Anyone with a session gets a shell.",
                kind: Kind::Toggle,
                default: "false",
                required: false,
            },
            Param {
                key: "HUT_DEFAULT_FOLDER",
                label: "Folder to open in",
                help: "Optional. A directory on the host, mounted and opened at start.",
                kind: Kind::HostPath { at: "/workspace", ro: false },
                default: "",
                required: false,
            },
            Param {
                key: "HUT_NAME",
                label: "Name",
                help: "Optional. What this terminal calls itself.",
                kind: Kind::Text,
                default: "",
                required: false,
            },
        ],
    },
];

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
            })
        })
        .collect();
    serde_json::json!({ "apps": apps })
}
