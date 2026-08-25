//! The catalog of installable applications.
//!
//! Every application WebDesk can install is described here, in the binary. It
//! is deliberately not a file on disk and not something the browser can add to:
//! a container is a way to run arbitrary code as whoever owns the engine, so
//! the set of things that may be run is a property of the build, reviewed like
//! any other code. "Install an app" means "choose one of these and fill in its
//! blanks", and nothing else.
//!
//! **Why every entry is a LinuxServer.io image.** They share one contract --
//! `/config` for state, `PUID`/`PGID` to own it, `TZ` for the clock -- so the
//! installer has one shape to implement rather than one per application. The
//! registry is `lscr.io`.
//!
//! **Why subpath support decides membership.** Apps are served from
//! `/app/<slug>/` on WebDesk's own origin (see `proxy.rs`), which is what lets
//! a container app share the session cookie and sit in an iframe at all. An
//! application that assumes it owns `/` emits absolute links that escape its
//! prefix and breaks. So an entry earns its place by working under a prefix --
//! either natively, or by being told its prefix through `base_env`. That is the
//! single hardest requirement to satisfy, and the reason this list is short.

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

pub struct App {
    pub slug: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    /// Image reference without a tag; the tag is appended at install time.
    pub image: &'static str,
    /// The port the application listens on *inside* the container.
    pub port: u16,
    pub icon: &'static str,
    /// The environment variable this application reads its base URL from, for
    /// those that must be told rather than working it out from
    /// `X-Forwarded-Prefix`. `None` means it needs no telling.
    pub base_env: Option<&'static str>,
    /// Set when the application has a first-run setup step that decides its own
    /// URLs, so being moved later would strand it.
    pub notes: &'static str,
    pub params: &'static [Param],
}

/// The universal clock parameter. Every LinuxServer image reads it, so rather
/// than repeating it in each entry the installer appends it to all of them.
pub const TZ: Param = Param {
    key: "TZ",
    label: "Timezone",
    help: "IANA name, such as Europe/London. Sets the clock inside the container.",
    kind: Kind::Text,
    default: "Etc/UTC",
    required: false,
};

pub static CATALOG: &[App] = &[
    App {
        slug: "code-server",
        name: "Code Server",
        tagline: "VS Code in the browser, running on this host.",
        image: "lscr.io/linuxserver/code-server",
        port: 8443,
        icon: "a-code",
        base_env: None,
        notes: "Serves itself correctly from a subpath with no extra configuration.",
        params: &[
            Param {
                key: "PASSWORD",
                label: "Password",
                help: "Asked for when the editor opens. Leave empty for no password.",
                kind: Kind::Secret,
                default: "",
                required: false,
            },
            Param {
                key: "SUDO_PASSWORD",
                label: "sudo password",
                help: "Lets the editor's terminal use sudo inside the container only.",
                kind: Kind::Secret,
                default: "",
                required: false,
            },
            Param {
                key: "DEFAULT_WORKSPACE",
                label: "Workspace folder",
                help: "Directory on the host to open. Mounted into the container.",
                kind: Kind::HostPath { at: "/workspace", ro: false },
                default: "",
                required: false,
            },
        ],
    },
    App {
        slug: "freshrss",
        name: "FreshRSS",
        tagline: "A feed reader that keeps your subscriptions on your own box.",
        image: "lscr.io/linuxserver/freshrss",
        port: 80,
        icon: "a-rss",
        base_env: None,
        notes: "Honours X-Forwarded-Prefix, so it follows wherever it is mounted.",
        params: &[],
    },
    App {
        slug: "dokuwiki",
        name: "DokuWiki",
        tagline: "A wiki that stores its pages as plain files, with no database.",
        image: "lscr.io/linuxserver/dokuwiki",
        port: 80,
        icon: "a-book",
        base_env: None,
        notes: "Finish setup at /install.php the first time it is opened.",
        params: &[],
    },
    App {
        slug: "calibre-web",
        name: "Calibre-Web",
        tagline: "Browse and read an existing Calibre library.",
        image: "lscr.io/linuxserver/calibre-web",
        port: 8083,
        icon: "a-book",
        base_env: None,
        notes: "Point it at a folder that already contains a Calibre metadata.db.",
        params: &[
            Param {
                key: "LIBRARY",
                label: "Calibre library",
                help: "The folder holding metadata.db. Mounted read-only.",
                kind: Kind::HostPath { at: "/books", ro: true },
                default: "",
                required: true,
            },
        ],
    },
    App {
        slug: "audiobookshelf",
        name: "Audiobookshelf",
        tagline: "A server for audiobooks and podcasts.",
        image: "lscr.io/linuxserver/audiobookshelf",
        port: 80,
        icon: "a-audio",
        base_env: None,
        notes: "",
        params: &[
            Param {
                key: "AUDIOBOOKS",
                label: "Audiobooks folder",
                help: "Mounted read-only.",
                kind: Kind::HostPath { at: "/audiobooks", ro: true },
                default: "",
                required: true,
            },
            Param {
                key: "PODCASTS",
                label: "Podcasts folder",
                help: "Optional, and writable so new episodes can be saved.",
                kind: Kind::HostPath { at: "/podcasts", ro: false },
                default: "",
                required: false,
            },
        ],
    },
    App {
        slug: "syncthing",
        name: "Syncthing",
        tagline: "Continuous file sync between your own machines.",
        image: "lscr.io/linuxserver/syncthing",
        port: 8384,
        icon: "a-sync",
        base_env: None,
        notes: "Only the web interface is proxied. Sync traffic needs port 22000 \
                opened separately, which WebDesk does not do for you.",
        params: &[
            Param {
                key: "DATA",
                label: "Folder to sync",
                help: "Mounted writable, since syncing means writing.",
                kind: Kind::HostPath { at: "/data", ro: false },
                default: "",
                required: true,
            },
        ],
    },
    App {
        slug: "grocy",
        name: "Grocy",
        tagline: "Household stock, shopping lists and chores.",
        image: "lscr.io/linuxserver/grocy",
        port: 80,
        icon: "a-list",
        base_env: None,
        notes: "First sign-in is admin / admin, and it will ask you to change it.",
        params: &[],
    },
    App {
        slug: "qbittorrent",
        name: "qBittorrent",
        tagline: "A BitTorrent client with a web interface.",
        image: "lscr.io/linuxserver/qbittorrent",
        port: 8080,
        icon: "a-download",
        base_env: None,
        notes: "The temporary first-run password is printed in the install log below.",
        params: &[
            Param {
                key: "DOWNLOADS",
                label: "Downloads folder",
                help: "Where completed files are written.",
                kind: Kind::HostPath { at: "/downloads", ro: false },
                default: "",
                required: true,
            },
        ],
    },
];

pub fn find(slug: &str) -> Option<&'static App> {
    CATALOG.iter().find(|a| a.slug == slug)
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
                .params
                .iter()
                .chain(std::iter::once(&TZ))
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
