//! What the host needs before an app will open, and installing it on one press.
//!
//! Three kinds of app in the catalog need three different things on the host: a
//! container engine, a compositor and an RFB server, and `flatpak` itself. None
//! of them is a build dependency, so `install.sh` cannot simply require them --
//! a host that will only ever run the file manager and the terminal should not
//! be made to carry Docker.
//!
//! So they are *probed* and *offered*. `report` says what is missing and what it
//! would cost to fix; `install` fixes it, streaming the package manager's output
//! into the same log the Apps window is already watching. That is the whole
//! "single click" -- the same "offered, not installed" shape `flatpak.rs`
//! already uses for a prerequisite, moved in front of the install rather than
//! inside it.
//!
//! **A package name is not always a fact about a manager.** `catalog::Prereq`
//! has one field per manager and says `None` where nobody has checked. That
//! covers most of what is below, and three rows need more than it can express:
//!
//! - **Docker on the RHEL family is not a package name at all.** Docker CE comes
//!   from Docker's own repository, which is a decision an operator makes and not
//!   one an installer makes for them. `None`, and Podman beside it as the answer
//!   that needs no third-party repository.
//! - **`dnf` is three package universes, not one.** Fedora has `cage` and
//!   `wayvnc` in its base repositories. Enterprise Linux has neither: they are in
//!   EPEL, `wayvnc` from EPEL 9 and `cage` only from EPEL 10 -- there is no EPEL
//!   9 build of `cage` at any version, so the streamed half of the catalog is
//!   simply unavailable on that generation. One `dnf` field cannot say "yes,
//!   after you enable EPEL, and not at all before EL10", so `ElFacts` says it
//!   instead, and `Dep::provides_here` reads the host rather than guessing.
//!   Enabling EPEL is the same category of act as adding Docker's repository and
//!   WebDesk does neither: it reports that EPEL is what is missing and leaves the
//!   decision where it belongs.
//! - **On Arch, the bridge cannot be had without the console.** There is no
//!   split package: `cockpit` is all of Cockpit, web server included. The name is
//!   given rather than withheld -- it is true, and an operator can weigh it --
//!   but the `why` says out loud what comes with it, because "only the bridge,
//!   never `cockpit-ws`" is the premise of `cockpit.rs` and on that one
//!   distribution package selection cannot keep the promise.
//!
//! **One fact, one home.** The bridge's package names live in
//! `cockpit::BRIDGE_PREREQ` and this table points at them, so the row here and
//! the `503 not-installed` refusal the host panels return cannot disagree about
//! what would fix the host. That refusal's `missing.bin` is this row's `key`, on
//! purpose: the window that receives it can turn it into an install without a
//! third table in between.
//!
//! **Nothing here takes a package name from a request.** `install` matches keys
//! against `RUNTIME` and drops everything else, so the widest thing a browser
//! can ask for is one of the six rows below. That is the same rule the catalog
//! is built on: a request may choose *which* of the operations the build
//! contains runs, never *what* the operation is.

use crate::catalog::Prereq;
use crate::cockpit;
use crate::engine::which;
use crate::flatpak::{self, Manager};
use crate::{session_of, unauthorized, AppState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// Which part of the catalog a dependency unlocks.
#[derive(Clone, Copy, PartialEq)]
pub enum Need {
    /// Container entries -- the LinuxServer desktops and the editor.
    Containers,
    /// Streamed entries -- a Flatpak drawn on the host.
    Streamed,
    /// The host panels, which speak to `cockpit-bridge`.
    Host,
}

impl Need {
    /// The word this goes over the wire as, and the word `install.sh` takes in
    /// `WD_APPS`.
    ///
    /// One vocabulary rather than two. An operator who reads "streamed" in the
    /// Apps window and types `WD_APPS=streamed` on the next host has to be
    /// right, and the only way to guarantee that is for the browser and the
    /// installer to be reading the same three words -- which a test checks
    /// against `install.sh` itself, because two hand-kept lists of the same
    /// three names drift.
    pub fn as_str(self) -> &'static str {
        match self {
            Need::Containers => "containers",
            Need::Streamed => "streamed",
            Need::Host => "host",
        }
    }

    /// Every need there is, so anything that walks them cannot quietly miss
    /// one when a fourth is added.
    ///
    /// Only the tests walk it today, which is why it carries an allow rather
    /// than being deleted: the tests are the thing that keeps this file and
    /// `install.sh` agreeing about the three words, and they can only do that
    /// against a list of all of them.
    #[allow(dead_code)]
    pub const ALL: [Need; 3] = [Need::Containers, Need::Streamed, Need::Host];

}

/// What `Prereq`'s single `dnf` field cannot say.
///
/// `Prereq` assumes a manager is a package universe. For three of the four it
/// is. `dnf` is not: Fedora, Enterprise Linux 9 and Enterprise Linux 10 run the
/// same binary against different repositories, and for `cage` the same name is
/// correct on the first, correct-after-a-decision on the third, and does not
/// exist on the second. The two rows that need that said are listed in `EL`;
/// every other row means exactly what `Prereq` says.
pub struct ElFacts {
    /// The repository the RHEL family gets this from, when a stock host has not
    /// got it enabled. Named rather than enabled: adding a third-party
    /// repository to somebody's machine is the same act as adding Docker's, and
    /// WebDesk does neither on its own.
    pub repo: &'static str,
    /// The earliest Enterprise Linux generation that has a build at all.
    ///
    /// `None` where every generation the repository serves has one. `Some(10)`
    /// is `cage`, and it is the difference between "enable EPEL" and "this is
    /// not available on this release" -- two sentences an operator acts on very
    /// differently.
    pub since: Option<u32>,
}

/// Which of the three package universes a `dnf` on this host is looking at.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DnfHost {
    Fedora,
    /// Enterprise Linux, with the major version, because it decides whether a
    /// build exists rather than merely which repository it is in.
    El(u32),
    /// A `dnf` this could not place. Treated as the strictest case: something
    /// unrecognised is not a licence to guess.
    Unknown,
}

/// Read the host's identity, once, from the file every target agrees on.
///
/// `/etc/os-release` rather than `rpm -q` or `dnf`: it is a text file, it is
/// there on every distribution this targets, and reading it cannot block on a
/// network or a metadata refresh -- which matters because this is on the path
/// that paints a window.
fn dnf_host() -> DnfHost {
    match std::fs::read_to_string("/etc/os-release") {
        Ok(text) => dnf_host_from(&text),
        Err(_) => DnfHost::Unknown,
    }
}

/// The parsing, apart from the reading, so every release can be asserted rather
/// than only whichever one the build machine happens to be.
fn dnf_host_from(text: &str) -> DnfHost {
    let field = |key: &str| -> Option<String> {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim_matches('"').trim().to_string())
    };
    let id = field("ID=").unwrap_or_default();
    if id == "fedora" {
        return DnfHost::Fedora;
    }
    // `ID_LIKE` as well as `ID`, because Rocky, Alma, CentOS Stream and Oracle
    // are each their own `ID` and all of them say `rhel` here. Matching on the
    // family is the check that does not need a new arm per rebuild.
    let like = field("ID_LIKE=").unwrap_or_default();
    let is_el = matches!(id.as_str(), "rhel" | "rocky" | "almalinux" | "centos" | "ol")
        || like.split_whitespace().any(|w| w == "rhel");
    if !is_el {
        return DnfHost::Unknown;
    }
    // `9.4` and `10` both appear; only the generation decides what is built.
    match field("VERSION_ID=").and_then(|v| v.split('.').next()?.parse().ok()) {
        Some(major) => DnfHost::El(major),
        None => DnfHost::Unknown,
    }
}

/// Whether this host already has EPEL configured.
///
/// A repository file, not `dnf repolist`: this is called to paint a window, and
/// `dnf` would go to the network. A host with EPEL set up has a file for it
/// under `/etc/yum.repos.d`, whether that came from the `epel-release` package
/// or from an operator writing one.
fn epel_configured() -> bool {
    let Ok(entries) = std::fs::read_dir("/etc/yum.repos.d") else { return false };
    entries.flatten().any(|e| e.file_name().to_string_lossy().starts_with("epel"))
}

/// Whether the name in `prereq.dnf` would actually resolve on this host.
///
/// A pure function of the three things that decide it, so the whole matrix can
/// be asserted rather than only whichever row the build machine happens to be.
fn resolvable(el: &ElFacts, host: DnfHost, epel: bool) -> bool {
    match host {
        // Base repositories, nothing to enable, nothing to explain.
        DnfHost::Fedora => true,
        // A generation with no build is not a repository problem and enabling
        // one would not fix it.
        DnfHost::El(major) if el.since.is_some_and(|s| major < s) => false,
        DnfHost::El(_) => epel,
        DnfHost::Unknown => false,
    }
}

pub struct Dep {
    pub key: &'static str,
    pub label: &'static str,
    /// One sentence saying what stops working without it, shown next to the
    /// button that installs it.
    pub why: &'static str,
    pub need: Need,
    pub prereq: Prereq,
    /// Whether WebDesk will ever put this on a host.
    ///
    /// Every row is *detected*; only an offered row is proposed and only an
    /// offered row can be installed. The distinction exists for exactly one
    /// entry and it is worth the field: Podman satisfies the container group
    /// when a host already has it, and WebDesk still will not be the thing that
    /// installs it. Recommending an engine is a claim about having run it, and
    /// the README says plainly that nobody has -- so the honest position is to
    /// use what is there and offer the one this project was written against.
    ///
    /// `absent_for` and `plan` both filter on this, so a row that is not offered
    /// cannot be reached by a request even by name.
    pub offered: bool,
    /// Rows sharing a name are alternatives; one of them is enough.
    ///
    /// `None` is a requirement -- `wayvnc` with no compositor serves nothing,
    /// and half of that is worth exactly as much as none of it. A group is a
    /// question with more than one right answer: "a container engine", "a
    /// headless compositor". A host needs one and it does not matter which,
    /// which is not the same as WebDesk having no opinion about which one it
    /// would put there itself -- the order rows appear in `RUNTIME` is that
    /// opinion, and the first one this host can install is the one offered.
    pub group: Option<&'static str>,
    /// The repository this comes from when the distribution does not ship it,
    /// named for the refusal.
    ///
    /// `Prereq` can say "no package name here" and cannot say why. For `docker`
    /// on the RHEL family the why is the whole answer: there is nothing missing
    /// from a mirror, the software is simply published by its vendor and getting
    /// it means adding their repository and their signing key. That is a
    /// supply-chain decision an operator makes deliberately, and WebDesk names
    /// it for the same reason it names EPEL rather than enabling it.
    pub vendor_repo: Option<&'static str>,
}

/// The rows where `prereq.dnf` is not the whole story, by key.
///
/// Beside `RUNTIME` rather than a field inside `Dep`, and that is a deliberate
/// shape rather than laziness. `Dep` is constructed outside this file --
/// `apps.rs` builds one to test the refusal a streamed install gives -- so every
/// field added here is a field every other caller must write out, and four of
/// the six rows would be writing `None` to say nothing at all. Two rows need
/// this; a two-row table says exactly that and nothing more.
///
/// The join is by `key`, which a test checks, because a key that matched nothing
/// would silently take an EPEL row back to "installs anywhere".
static EL: &[(&str, ElFacts)] = &[
    // EPEL built cage 0.2.0-3 for 10.2, 10.3 and 10.4 and never back to 9, so
    // an EL9 host has no compositor package under any name and the streamed
    // entries are not available there at all -- which is a different sentence
    // from "enable EPEL", and the one an operator on that generation needs.
    ("cage", ElFacts { repo: "EPEL", since: Some(10) }),
    // The same repository, not the same story, and recording both is the point:
    // EPEL has wayvnc for 9 (0.7.2) as well as 10 (0.9.0), so there is no
    // generation floor. On an EL9 host this one installs and `cage` cannot,
    // which is precisely the half a feature that is worth nothing.
    ("wayvnc", ElFacts { repo: "EPEL", since: None }),
];

fn el_facts(key: &str) -> Option<&'static ElFacts> {
    EL.iter().find(|(k, _)| *k == key).map(|(_, f)| f)
}

impl Dep {
    /// Where `dnf` alone cannot say what provides this. See `EL`.
    fn el(&self) -> Option<&'static ElFacts> {
        el_facts(self.key)
    }

    /// Whether this host already has it.
    ///
    /// By binary on `PATH`, like `flatpak::missing_packages`, and not by asking
    /// the package manager: the binary is what the compositor or the engine
    /// actually needs, it is spelled the same on every distribution, and an
    /// operator who built one from source has it even though no package
    /// database mentions it.
    pub fn present(&self) -> bool {
        which(self.prereq.bin).is_some()
    }

    /// What a host of this manager can install *sight unseen* -- with only the
    /// repositories it shipped with, and without anybody having looked at which
    /// distribution it is.
    ///
    /// The answer `install.sh` needs, because it runs before anything has
    /// probed anything, and the answer the drift test compares against, because
    /// it must not depend on the machine the tests run on. A row with `ElFacts`
    /// has no such answer for `dnf`: the name is right on Fedora and wrong on
    /// every stock Enterprise Linux, and this function may not pick one.
    ///
    /// The match itself is the same one as `flatpak::Manager::package`, which is
    /// private to that file. Repeated rather than reached for, because widening
    /// it belongs to whoever owns `flatpak.rs`; the *names* still live in one
    /// place, which is the half that would actually hurt to duplicate.
    pub fn package(&self, m: Manager) -> Option<&'static str> {
        match m {
            Manager::Dnf if self.el().is_some() => None,
            Manager::Dnf => self.prereq.dnf,
            Manager::Apt => self.prereq.apt,
            Manager::Pacman => self.prereq.pacman,
            Manager::Zypper => self.prereq.zypper,
        }
    }

    /// What *this* host would install, having looked at it.
    ///
    /// The answer `report` shows and `plan` acts on. It differs from `package`
    /// only on the RHEL family and only for the two rows that come from EPEL,
    /// where the generation and whether EPEL is already configured decide
    /// whether there is a name to give at all.
    pub fn provides_here(&self) -> Option<&'static str> {
        let m = flatpak::manager()?;
        if m != Manager::Dnf {
            return self.package(m);
        }
        let Some(el) = self.el() else { return self.prereq.dnf };
        resolvable(el, dnf_host(), epel_configured()).then_some(self.prereq.dnf).flatten()
    }

    /// Why this host cannot install it, in a sentence somebody can act on.
    ///
    /// Only reached when `provides_here` said no, and it says which of the four
    /// reasons it was -- because "enable EPEL and press this again" and "this
    /// release has no build and never will" are the same `null` in the report
    /// and completely different news.
    fn refusal(&self, m: Manager) -> String {
        if let (Manager::Dnf, Some(el)) = (m, self.el()) {
            return match dnf_host() {
                DnfHost::El(major) if el.since.is_some_and(|s| major < s) => format!(
                    "{} has no build for Enterprise Linux {major} at any version -- {} carries \
                     it only from {} onwards -- so it cannot be installed on this release, and \
                     the apps that need it are not available here.",
                    self.prereq.bin,
                    el.repo,
                    el.since.map(|s| s.to_string()).unwrap_or_default(),
                ),
                DnfHost::El(_) => format!(
                    "{} comes from {} on this family rather than from any base repository, and \
                     this host has not got {} configured. WebDesk will not add a third-party \
                     repository to your machine; enable it yourself and this becomes one press.",
                    self.prereq.bin, el.repo, el.repo,
                ),
                // Fedora resolves in `provides_here`, so this is the `dnf` we
                // could not place -- which is not a licence to try a name that
                // is right on only some of them.
                _ => format!(
                    "WebDesk could not tell which dnf distribution this is, and {} is in a base \
                     repository on some of them and in {} on others.",
                    self.prereq.bin, el.repo,
                ),
            };
        }
        if let Some(repo) = self.vendor_repo {
            return format!(
                "{} is not in this distribution's repositories -- it is published through {}, \
                 and installing it means adding that repository and its signing key. WebDesk \
                 will not do that to your host. Add it yourself and this becomes one press.",
                self.prereq.bin, repo,
            );
        }
        format!(
            "WebDesk does not know which {} package provides {}. {}",
            m.bin(),
            self.prereq.bin,
            self.why
        )
    }
}

/// Everything WebDesk can check for and offer to install.
pub static RUNTIME: &[Dep] = &[
    // Two rows for one question, and they are not symmetrical. Either engine
    // satisfies the `engine` group when it is already here; only one of them is
    // ever offered.
    Dep {
        key: "docker",
        label: "Docker",
        why: "Without a container engine the desktop and editor entries have nothing to run \
              in and will not install; Docker is the one WebDesk was written against and \
              tested with.",
        need: Need::Containers,
        offered: true,
        group: Some("engine"),
        // Fedora's `moby-engine` is a fork and not what somebody asking for
        // Docker means, and Enterprise Linux has neither it nor Docker CE. Both
        // roads there end at Docker's own repository, so `dnf` has no honest
        // name and says so through `vendor_repo` rather than through silence.
        vendor_repo: Some("Docker's own repository at download.docker.com"),
        prereq: Prereq {
            bin: "docker",
            dnf: None,
            // Real, and in these distributions' own repositories.
            apt: Some("docker.io"),
            pacman: Some("docker"),
            zypper: Some("docker"),
        },
    },
    Dep {
        key: "podman",
        label: "Podman",
        why: "A container engine this host already has. WebDesk will use it for the desktop \
              and editor entries, and does not install it.",
        need: Need::Containers,
        // Detected, never offered, and that is a deliberate asymmetry rather
        // than a slight. Every command `engine.rs` runs takes the same arguments
        // in both engines, which is an argument from reading the manuals and not
        // from running anything -- the README still says "Podman is accepted but
        // untested". Using what an operator already chose costs nothing and
        // takes nothing back; putting it there ourselves would be recommending
        // an engine on the strength of a comparison nobody has made.
        //
        // What would change this: the install, start, stop and remove path
        // exercised end to end against Podman on one host of each family, with
        // the desktop entries actually drawing. Then the README line changes,
        // this becomes `true`, and this paragraph goes.
        offered: false,
        group: Some("engine"),
        vendor_repo: None,
        prereq: Prereq {
            bin: "podman",
            // No package names, because there is no case in which WebDesk
            // installs it, and a name here would be an offer with the button
            // filed off. It is in the base repositories of all four under this
            // same name, which is a fact for an operator's shell and not for
            // this table.
            dnf: None,
            apt: None,
            pacman: None,
            zypper: None,
        },
    },
    Dep {
        key: "flatpak",
        label: "Flatpak",
        why: "Without it there is nothing to install a streamed application with, and every \
              entry in that half of the catalog refuses before it starts.",
        need: Need::Streamed,
        offered: true,
        group: None,
        vendor_repo: None,
        prereq: Prereq {
            // The same four names `flatpak::missing_packages` hardcodes for the
            // host-service path. It is one word on every target and has been
            // for years.
            bin: "flatpak",
            dnf: Some("flatpak"),
            apt: Some("flatpak"),
            pacman: Some("flatpak"),
            zypper: Some("flatpak"),
        },
    },
    Dep {
        key: "sway",
        label: "Sway",
        why: "The compositor a drawn app runs inside. Sway's output can be resized, so with it \
              an application's resolution follows the WebDesk window instead of being scaled \
              up from a fixed 1280x720.",
        need: Need::Streamed,
        group: Some("compositor"),
        offered: true,
        vendor_repo: None,
        prereq: Prereq {
            bin: "sway",
            // In Fedora but in no EPEL generation, which is exactly why `cage`
            // stays below rather than being replaced: on Enterprise Linux 10
            // cage is the only compositor there is to install, and on 9 there is
            // neither.
            dnf: Some("sway"),
            apt: Some("sway"),
            pacman: Some("sway"),
            zypper: Some("sway"),
        },
    },
    Dep {
        key: "cage",
        label: "cage",
        why: "A headless compositor for drawn apps, and the fallback where Sway is not packaged \
              -- Enterprise Linux 10 is the case. cage cannot resize its output: asking \
              it to crashes it, so an app running under cage is fixed at 1280x720 and \
              the browser scales it to fit.",
        need: Need::Streamed,
        offered: true,
        group: Some("compositor"),
        vendor_repo: None,
        prereq: Prereq {
            // The name is `cage` wherever `cage` exists -- Fedora's base
            // repositories, Debian and Ubuntu `main` from bookworm (0.1.4, and
            // 0.2.0 in trixie), Arch, openSUSE, and EPEL. What differs is
            // whether it exists at all, which `EL` above says and this field
            // cannot -- the row this whole mechanism was written for.
            bin: "cage",
            dnf: Some("cage"),
            apt: Some("cage"),
            pacman: Some("cage"),
            zypper: Some("cage"),
        },
    },
    Dep {
        key: "wayvnc",
        label: "wayvnc",
        why: "Without it a streamed application draws and nothing carries the pixels to this \
              browser; on the RHEL family it comes from EPEL rather than from any base \
              repository.",
        need: Need::Streamed,
        offered: true,
        group: None,
        vendor_repo: None,
        prereq: Prereq {
            // Debian bookworm has 0.5.0 and trixie 0.9.1, both in `main`.
            bin: "wayvnc",
            dnf: Some("wayvnc"),
            apt: Some("wayvnc"),
            pacman: Some("wayvnc"),
            zypper: Some("wayvnc"),
        },
    },
    Dep {
        // The key is `BRIDGE_PREREQ.bin`, and that is load-bearing rather than a
        // coincidence: the host panels refuse with `503 not-installed` and a
        // `missing.bin` field, and a window holding that refusal can post it
        // straight back here as a key. One name, one row, no table in between.
        key: "cockpit-bridge",
        label: "Cockpit bridge",
        why: "Without it the Services, Logs and Metrics panels have nobody to ask and stay \
              empty; on Arch there is no bridge package apart from `cockpit`, so installing it \
              there installs the Cockpit web console with it.",
        need: Need::Host,
        offered: true,
        group: None,
        vendor_repo: None,
        // Pointed at rather than copied. `cockpit.rs` owns what provides its own
        // bridge, and it is the same value its `not-installed` refusal names --
        // so the package this offers to install and the package that refusal
        // says would fix the host cannot come apart.
        //
        // Its `pacman` name is `cockpit`, not a bridge, and that is deliberate
        // there: withholding the name would be less true than giving it. What
        // this row adds is that the cost is in the sentence beside the button,
        // where somebody sees it before pressing rather than after finding a web
        // server listening. `install.sh` declines it altogether -- a knob in a
        // shell script is not somebody deciding.
        prereq: cockpit::BRIDGE_PREREQ,
    },
];

/// The dependencies for one part of the catalog that this host has not got.
///
/// The typed half of `report`, for the installer: an install that is about to
/// fail for want of a compositor should refuse and say so, in the same sentence
/// as what would fix it, rather than start and die.
///
/// Empty means the need is met. For `Containers` the list that comes back is
/// *alternatives* -- "docker or podman", any one of which is enough -- and for
/// the other two it is a shopping list, all of which is needed. `Need` is what
/// says which, so a caller building a sentence asks it rather than guessing
/// from the length.
///
pub fn absent_for(need: Need) -> Vec<&'static Dep> {
    let group: Vec<&'static Dep> = RUNTIME.iter().filter(|d| d.need == need).collect();
    // Satisfaction is judged across every row, and only then is the answer
    // narrowed to what WebDesk would actually put there. The order matters: a
    // host with Podman and no Docker is *not* missing a container engine, and
    // filtering to offered rows first would have said it was and offered to fix
    // a machine that was already working.
    let mut out: Vec<&'static Dep> = Vec::new();
    for d in &group {
        let Some(name) = d.group else {
            // A requirement is missing or it is not.
            if !d.present() && d.offered {
                out.push(d);
            }
            continue;
        };
        let peers: Vec<&&'static Dep> = group.iter().filter(|o| o.group == Some(name)).collect();
        // An alternative is missing only when none of its alternatives is here.
        // Asking about offered rows first would tell a host with Podman and no
        // Docker that it has no container engine, and offer to fix a machine
        // that was already working.
        if peers.iter().any(|o| o.present()) {
            continue;
        }
        // Nothing in the group is here, so one of them is proposed: the first
        // this host can actually install, which is what makes the order of
        // `RUNTIME` a preference rather than a list. Naming all of them would
        // have somebody install two compositors to run one application.
        match peers.iter().find(|o| o.offered && o.provides_here().is_some()) {
            Some(p) if p.key == d.key => out.push(d),
            Some(_) => {}
            // None of the alternatives can be installed here, so every offered
            // one is named and all the refusals are visible rather than one
            // arbitrary one.
            None if d.offered => out.push(d),
            None => {}
        }
    }
    out
}

/// What is present, what is missing, and what package would provide it here.
///
/// `package` is what *this* host would install, so it is `null` for a row with
/// no known name here, for every row on a host with no package manager WebDesk
/// knows, and for an EPEL row on an Enterprise Linux that has not got EPEL or is
/// too old to have a build. Those are different stories behind one `null`: the
/// window tells the first two apart by `manager`, which is `null` only for a
/// host with no manager, and gets the rest of the sentence from `why` -- and
/// from the refusal, if the button is pressed anyway.
pub fn report() -> Value {
    let m = flatpak::manager();
    let deps: Vec<Value> = RUNTIME
        .iter()
        .map(|d| {
            json!({
                "key": d.key,
                "label": d.label,
                "why": d.why,
                "need": d.need.as_str(),
                "present": d.present(),
                "package": d.provides_here(),
                // Whether the window may draw a button at all. A row that is
                // detected but never installed by us -- Podman -- is reported
                // like any other and has nothing to press.
                "offered": d.offered,
                "group": d.group,
            })
        })
        .collect();
    json!({ "deps": deps, "manager": m.map(|m| m.bin()), "engine": engine_report() })
}

/// What this host runs containers with, and what choice that leaves.
///
/// Split out of the dependency list because it is not a list question. The rows
/// answer "is a container engine here"; this answers "which one, and is there a
/// decision outstanding" -- and the second only ever has an answer when both are
/// on the machine at once.
fn engine_report() -> Value {
    let docker = which("docker").is_some();
    let podman = which("podman").is_some();
    // `engine::detect` is the authority rather than a rule repeated here. It
    // prefers Docker and honours `WD_CONTAINER_ENGINE`, so a window that showed
    // its own opinion would eventually disagree with the thing actually running
    // the containers.
    let in_use = crate::engine::detect().map(|e| e.bin());

    // The decision only exists in one arrangement: both engines present, Docker
    // doing the work, Podman sitting there installed and no longer used for
    // anything of ours. Anything else is a machine with nothing to decide.
    let spare = docker && podman && in_use == Some("docker");
    json!({
        "in_use": in_use,
        "docker": docker,
        "podman": podman,
        // Present, installed by somebody else, and now doing nothing for
        // WebDesk. The window offers Keep or Remove; `podman_removal` decides
        // whether Remove is allowed to be more than a button.
        "podman_spare": spare,
        "removal": if spare { podman_removal() } else { Value::Null },
    })
}

/// Whether Podman can be removed, and the command it would take.
///
/// Every answer here is a refusal or a plan, never an action. The refusals are
/// the point of the feature: WebDesk did not install Podman, so the bar for
/// taking it off a host is higher than the bar for putting Docker on one.
fn podman_removal() -> Value {
    let Some(m) = flatpak::manager() else {
        return json!({
            "allowed": false,
            "reason": "this host has no package manager WebDesk knows how to remove with",
        });
    };
    let command = format!("{} {} podman", m.bin(), remove_verb(m).join(" "));

    // The containers are asked about first, because this is the refusal that
    // protects work that is not ours. A container somebody else created is
    // exactly what must not be destroyed by a press in this window, and a
    // stopped one is no less theirs for being idle.
    match crate::engine::containers(crate::engine::Engine::Podman) {
        Err(e) => json!({
            "allowed": false,
            "command": command,
            "reason": format!(
                "WebDesk could not ask Podman what it is holding ({e}), and will not remove an \
                 engine whose containers it could not count. `podman ps -a` will say."
            ),
        }),
        Ok(names) if !names.is_empty() => json!({
            "allowed": false,
            "command": command,
            "containers": names,
            "reason": format!(
                "Podman still holds {} container{}. Removing it would destroy {}, and none of \
                 them is WebDesk's to destroy.",
                names.len(),
                if names.len() == 1 { "" } else { "s" },
                if names.len() == 1 { "it" } else { "them" },
            ),
        }),
        Ok(_) => json!({
            "allowed": true,
            "command": command,
            "warning": "Removing a package can take others that depend on it. On the RHEL and \
                        Fedora families Podman is part of the distribution's own tooling and \
                        other things may expect it.",
        }),
    }
}

/// The argv that removes a package, per manager.
///
/// Deliberately the narrow verb in each: `remove` and not `purge`, `-R` and not
/// `-Rns`. WebDesk is taking off a package somebody else put on, and the
/// configuration and dependencies of it are theirs to keep -- the wide verbs are
/// available in their shell if that is what they meant.
fn remove_verb(m: Manager) -> &'static [&'static str] {
    match m {
        Manager::Dnf => &["remove", "-y"],
        Manager::Apt => &["remove", "-y"],
        Manager::Pacman => &["-R", "--noconfirm"],
        Manager::Zypper => &["remove", "-y"],
    }
}

/// Requested keys narrowed to the rows `RUNTIME` names, in table order.
///
/// Everything else is dropped rather than rejected. A key that is not a row is
/// not an attack to report, it is a client and a build that disagree about what
/// exists -- and the safe reading of "install docker and this other thing" is
/// to install docker.
fn chosen(keys: &[String]) -> Vec<&'static Dep> {
    RUNTIME.iter().filter(|d| keys.iter().any(|k| k == d.key)).collect()
}

/// The argv tail that installing these keys would really run, or why it cannot.
///
/// Separated from `install` so the refusal happens while there is still a
/// request to answer with it. Anything absent from `RUNTIME`, and anything
/// already on the host, is gone by the time this returns a name.
fn plan(keys: &[String], m: Manager) -> Result<Vec<String>, String> {
    let mut packages = Vec::new();
    // `offered` is filtered here as well as in `chosen`'s callers, because this
    // is the last point before an argv: a row WebDesk does not put on hosts must
    // not become one because some future caller reached `plan` another way.
    for d in chosen(keys).into_iter().filter(|d| d.offered) {
        if d.present() {
            continue;
        }
        match d.provides_here() {
            Some(p) => packages.push(p.to_string()),
            None => return Err(d.refusal(m)),
        }
    }
    Ok(packages)
}

/// Install the named dependencies. Keys are matched against `RUNTIME`; anything
/// not in it is dropped, so a request can never name a package.
pub fn install(keys: &[String], log: &Path) -> Result<(), String> {
    let Some(m) = flatpak::manager() else {
        return Err("this host has no package manager WebDesk knows how to install with".into());
    };
    let packages = plan(keys, m)?;
    if packages.is_empty() {
        return Ok(());
    }
    flatpak::install_packages(&packages, log)
}

// ------------------------------------------- the log and the install flag
//
// The Apps window is already watching one log and one status file, and a
// dependency install belongs in them rather than in a second pair nobody polls.
// `apps.rs` owns both and now shares the four functions that name them, so the
// state directory is spelled in exactly one place -- it used to be spelled here
// as well, which would have meant the Apps window streaming an empty file while
// the packages installed perfectly well behind it, on the day somebody moved it.
use crate::apps::{admin_session, log_file, now, read_status, write_status};

fn bad(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// `GET /api/deps` -- what is here, what is not, and what would fix it.
///
/// Any session may read it, like the app list: this is an inventory of the
/// host, not a possession of whoever installed something. Not open to an
/// unauthenticated caller, though -- "this machine has no container engine and
/// no cockpit-bridge" is a description of what is not being watched.
pub async fn deps_report(State(s): State<AppState>, h: HeaderMap) -> Response {
    if session_of(&s, &h).is_none() {
        return unauthorized();
    }
    Json(report()).into_response()
}

#[derive(Deserialize)]
pub struct InstallReq {
    /// Keys from `RUNTIME`. Not package names, and there is nowhere in this
    /// request to put one.
    #[serde(default)]
    keys: Vec<String>,
}

/// `POST /api/deps/install` -- `{"keys":["docker","cage",…]}`, the single click.
///
/// Admin-gated like every other install, and streamed into the same log the
/// Apps window already polls, so the button that starts it needs no new UI to
/// report what it is doing.
///
/// It takes the same host-wide install flag `apps.rs` sets, which is what keeps
/// two package managers off the same lock file: an app install running now
/// refuses this, and this refuses an app install while it runs. The flag is a
/// file and the check is not atomic with the claim -- two requests in the same
/// millisecond can both pass it -- which is the race `apps.rs` already has with
/// itself, inherited rather than added.
pub async fn deps_install(
    State(s): State<AppState>,
    h: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let session = match admin_session(&s, &h) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let req: InstallReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad(StatusCode::BAD_REQUEST, format!("could not read the request: {e}")),
    };

    let wanted = chosen(&req.keys);
    if wanted.is_empty() {
        return bad(
            StatusCode::BAD_REQUEST,
            "none of those is a dependency WebDesk knows how to install",
        );
    }
    let Some(m) = flatpak::manager() else {
        return bad(
            StatusCode::CONFLICT,
            "this host has no package manager WebDesk knows how to install with",
        );
    };
    // Resolved here rather than on the worker so a host with no name for one of
    // these is told so in the response to the request that asked, instead of
    // finding out from a log line after the button has already gone busy.
    let packages = match plan(&req.keys, m) {
        Ok(p) => p,
        Err(e) => return bad(StatusCode::CONFLICT, e),
    };
    if packages.is_empty() {
        return Json(json!({ "ok": true, "started": false, "packages": [] })).into_response();
    }
    if read_status()["state"] == "running" {
        return bad(StatusCode::CONFLICT, "another install is already running");
    }

    let actor = session.ident.username.clone();
    let labels: Vec<&str> = wanted.iter().map(|d| d.label).collect();
    let name = labels.join(", ");
    let keys = req.keys.clone();

    // The Apps window is already polling this pair. `slug` is empty because
    // there is no catalog entry here -- this is the host being prepared, not an
    // application being installed -- and `name` is what the window has to show
    // in its place.
    let _ = write_status(&json!({
        "state": "running", "phase": "packages", "slug": "", "name": name,
        "started": now(), "actor": actor,
    }));
    let _ = std::fs::write(log_file(), b"");

    tracing::warn!(user = %actor, packages = ?packages, "installing host dependencies");

    // Package managers are slow and the browser polls, exactly as it does for
    // an app install. Same shape, so the window needs nothing new to watch it.
    tokio::task::spawn_blocking(move || {
        let done = match install(&keys, &log_file()) {
            Ok(()) => json!({
                "state": "done", "phase": "installed", "slug": "", "name": name,
                "finished": now(), "actor": actor,
            }),
            Err(e) => json!({
                "state": "failed", "phase": "packages", "slug": "", "name": name,
                "finished": now(), "actor": actor, "error": e,
            }),
        };
        let _ = write_status(&done);
    });

    Json(json!({ "ok": true, "started": true, "packages": packages })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row must be installable on at least one of the managers the README
    /// claims to target, or it is a button that can never work anywhere and the
    /// row should not exist. The mirror of
    /// `flatpak::a_prerequisite_is_either_named_or_deliberately_not`, and the
    /// same guarantee: `None` is a deliberate refusal with instructions, never
    /// a field somebody forgot.
    #[test]
    fn a_dependency_is_either_named_somewhere_or_should_not_be_offered() {
        for d in RUNTIME {
            assert!(!d.prereq.bin.is_empty(), "{} names no binary to probe for", d.key);
            assert!(!d.why.is_empty(), "{} has no sentence to show beside its button", d.key);
            assert!(!d.label.is_empty(), "{} has nothing to call itself", d.key);
            let named = d.prereq.dnf.is_some()
                || d.prereq.apt.is_some()
                || d.prereq.pacman.is_some()
                || d.prereq.zypper.is_some();
            if d.offered {
                assert!(
                    named || d.vendor_repo.is_some(),
                    "{} is offered, is installable nowhere, and names no repository to say why \
                     -- so the refusal would have nothing to tell anybody",
                    d.key
                );
            } else {
                // The other direction, and the one that keeps the policy true:
                // a row WebDesk does not put on hosts must not carry a package
                // name, because a name is an offer with the button filed off
                // and the next person to touch this file will wire it up.
                assert!(
                    !named,
                    "{} is not offered but names a package, which is an offer waiting to be \
                     made by accident",
                    d.key
                );
            }
        }
    }

    /// A key is an identifier in three places at once -- the JSON the Apps
    /// window matches on, the body of an install request, and this table. Two
    /// rows with one key would make an install request ambiguous and a
    /// duplicated row in the window; a key with a space or a slash in it would
    /// be a key nobody can put in a URL or a shell word.
    #[test]
    fn a_key_names_exactly_one_row_and_is_spelled_plainly() {
        for (i, d) in RUNTIME.iter().enumerate() {
            assert!(!d.key.is_empty(), "row {i} has no key");
            assert!(
                d.key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not spelled in lowercase and hyphens",
                d.key
            );
            assert!(
                RUNTIME.iter().filter(|o| o.key == d.key).count() == 1,
                "{} names more than one row",
                d.key
            );
        }
    }

    /// The three group names the browser is shown are the three `install.sh`
    /// accepts in `WD_APPS`. An operator reads one and types the other, so a
    /// rename on either side has to be a rename on both -- which is why this
    /// reads the script rather than a copy of its vocabulary.
    #[test]
    fn install_sh_takes_the_group_names_the_api_reports() {
        let script = include_str!("../install.sh");
        for need in Need::ALL {
            assert!(
                script.contains(need.as_str()),
                "install.sh has never heard of the {} group",
                need.as_str()
            );
        }
    }

    /// `install.sh` cannot read this table -- the binary that owns it is not
    /// built yet when the script runs -- so it carries its own copy of the
    /// package names, and a copy is a thing that drifts. Every name the script
    /// installs for a group must be a name this table gives for that group on
    /// that manager, and every group this table can provision on a manager must
    /// be one the script provisions.
    ///
    /// A subset in the first direction rather than an equality, because a host
    /// needs one container engine and the table offers two. `package` and not
    /// `provides_here` on purpose: `install.sh` runs before anything has looked
    /// at the host, and this test must say the same thing on every machine it
    /// runs on.
    #[test]
    fn install_sh_installs_only_packages_this_table_names() {
        let script = include_str!("../install.sh");
        // The families `install.sh` detects, against the managers they use.
        let families = [("debian", Manager::Apt), ("rhel", Manager::Dnf), ("arch", Manager::Pacman)];

        // An arm may be empty, but only where somebody decided it should be.
        // Anything else empty is a group that was forgotten, which is the whole
        // failure this test exists to catch.
        let deliberately_empty = [
            // The table names `cockpit` here, from `cockpit::BRIDGE_PREREQ`, and
            // installing it would put the Cockpit web console on the host. The
            // Apps window may offer that, because a person is reading the
            // sentence beside the button; `WD_APPS=host` in a shell script is
            // not a person reading anything.
            ("host", "arch"),
        ];

        for need in Need::ALL {
            for (family, m) in families {
                let known: Vec<&str> =
                    RUNTIME.iter().filter(|d| d.need == need).filter_map(|d| d.package(m)).collect();
                let arm = format!("{}:{}", need.as_str(), family);
                let line = script
                    .lines()
                    .find(|l| l.trim_start().starts_with(&format!("{arm})")))
                    .unwrap_or_else(|| panic!("install.sh has no {arm} arm"));
                let quoted = line
                    .split('"')
                    .nth(1)
                    .unwrap_or_else(|| panic!("install.sh's {arm} arm echoes nothing quoted"));

                for pkg in quoted.split_whitespace() {
                    assert!(
                        known.contains(&pkg),
                        "install.sh installs {pkg} for {arm}, which deps::RUNTIME does not name",
                    );
                }
                if quoted.split_whitespace().next().is_none() {
                    assert!(
                        known.is_empty() || deliberately_empty.contains(&(need.as_str(), family)),
                        "{arm}: the table can provision this and install.sh does not, with no \
                         decision recorded either here or in the script",
                    );
                }
            }
        }
    }

    /// The one reading that decides whether an EPEL row can be installed, taken
    /// against real `/etc/os-release` files rather than the one under this
    /// build. `VERSION_ID` is `9.4` on one rebuild and `10` on another, and only
    /// the generation in front of the dot decides what EPEL built; a host read
    /// as 9 when it is 10 is refused a compositor it has.
    #[test]
    fn a_dnf_host_is_placed_by_what_it_says_it_is() {
        let rocky9 = "NAME=\"Rocky Linux\"\nID=\"rocky\"\nVERSION_ID=\"9.4\"\n\
                      ID_LIKE=\"rhel centos fedora\"\n";
        let rocky10 = "ID=\"rocky\"\nVERSION_ID=\"10.0\"\nID_LIKE=\"rhel centos fedora\"\n";
        let rhel10 = "NAME=\"Red Hat Enterprise Linux\"\nID=\"rhel\"\nVERSION_ID=\"10.1\"\n";
        let fedora = "NAME=Fedora Linux\nID=fedora\nVERSION_ID=42\n";
        let ubuntu = "ID=ubuntu\nVERSION_ID=\"24.04\"\nID_LIKE=debian\n";

        assert_eq!(dnf_host_from(rocky9), DnfHost::El(9));
        assert_eq!(dnf_host_from(rocky10), DnfHost::El(10));
        assert_eq!(dnf_host_from(rhel10), DnfHost::El(10));
        assert_eq!(dnf_host_from(fedora), DnfHost::Fedora);
        // Not an EL, whatever its VERSION_ID looks like. Reading 24.04 as
        // "Enterprise Linux 24" would clear every generation floor there is.
        assert_eq!(dnf_host_from(ubuntu), DnfHost::Unknown);
        assert_eq!(dnf_host_from(""), DnfHost::Unknown);

        // A rebuild nobody has added an arm for still reads as EL, because
        // ID_LIKE is what they all agree on.
        assert_eq!(dnf_host_from("ID=circus\nID_LIKE=\"rhel\"\nVERSION_ID=\"10\"\n"), DnfHost::El(10));
    }

    /// The measured packaging facts, asserted for every generation rather than
    /// for whichever one this build happens to run on -- which is why the
    /// decision is a pure function and this test can drive it.
    ///
    /// What it prevents: an EL9 host being sent to a `cage` package that EPEL
    /// never built, and an EL10 host with EPEL already configured being refused
    /// something that is one `dnf` away.
    #[test]
    fn the_epel_rows_answer_for_the_release_they_are_asked_about() {
        let cage = &ElFacts { repo: "EPEL", since: Some(10) };
        let wayvnc = &ElFacts { repo: "EPEL", since: None };

        // Fedora has both in base repositories; EPEL does not come into it.
        assert!(resolvable(cage, DnfHost::Fedora, false));
        assert!(resolvable(wayvnc, DnfHost::Fedora, false));

        // EL9 has wayvnc in EPEL and no cage build at any version, so enabling
        // EPEL changes the answer for one of them and not the other.
        assert!(!resolvable(cage, DnfHost::El(9), true), "EPEL 9 has no cage to install");
        assert!(!resolvable(cage, DnfHost::El(9), false));
        assert!(resolvable(wayvnc, DnfHost::El(9), true));
        assert!(!resolvable(wayvnc, DnfHost::El(9), false), "EPEL is not enabled here");

        // EL10 has both, once EPEL is set up -- and WebDesk does not set it up.
        assert!(resolvable(cage, DnfHost::El(10), true));
        assert!(resolvable(wayvnc, DnfHost::El(10), true));
        assert!(!resolvable(cage, DnfHost::El(10), false));

        // A dnf nobody could place gets the strict answer, not a guess.
        assert!(!resolvable(cage, DnfHost::Unknown, true));
        assert!(!resolvable(wayvnc, DnfHost::Unknown, true));
    }

    /// `EL` is joined to `RUNTIME` by a string, which is the one thing about
    /// this arrangement that can rot silently: a key that matched no row would
    /// take an EPEL row back to "installs on any dnf host" with nothing failing
    /// to say so. And the other direction -- `ElFacts` says where a name comes
    /// from, so attaching it to a row with no `dnf` name would describe the
    /// provenance of nothing, and `provides_here` would refuse for a reason the
    /// refusal text never mentions.
    #[test]
    fn every_repository_note_belongs_to_a_row_that_names_a_package() {
        for (key, facts) in EL {
            let d = RUNTIME
                .iter()
                .find(|d| d.key == *key)
                .unwrap_or_else(|| panic!("EL names {key}, which is not a row in RUNTIME"));
            assert!(
                d.prereq.dnf.is_some(),
                "{key} says which repository provides it on dnf but not what to install",
            );
            assert!(!facts.repo.is_empty(), "{key}: unnamed repository");
            // Every one of them is a `dnf` story. A note on a row whose only
            // absent manager is somewhere else would never be read.
            assert!(d.el().is_some());
        }
    }

    /// The host panels refuse with `503 not-installed` and a `missing.bin`, and
    /// a window holding that refusal turns it into an install by posting it back
    /// as a key. If the two ever stopped being the same string, that round trip
    /// would silently install nothing -- `chosen` drops what it does not know.
    ///
    /// Also that the row points at `cockpit::BRIDGE_PREREQ` rather than a copy:
    /// the package this offers and the package that refusal names have to be one
    /// value, or a host can be told two different things about the same fix.
    #[test]
    fn the_host_row_is_the_key_the_cockpit_refusal_names() {
        let row = RUNTIME
            .iter()
            .find(|d| d.need == Need::Host)
            .expect("something has to answer for the host panels");
        assert_eq!(row.key, cockpit::BRIDGE_PREREQ.bin);
        assert_eq!(row.prereq.bin, cockpit::BRIDGE_PREREQ.bin);
        assert_eq!(row.prereq.dnf, cockpit::BRIDGE_PREREQ.dnf);
        assert_eq!(row.prereq.apt, cockpit::BRIDGE_PREREQ.apt);
        assert_eq!(row.prereq.pacman, cockpit::BRIDGE_PREREQ.pacman);
        assert_eq!(row.prereq.zypper, cockpit::BRIDGE_PREREQ.zypper);
        // The wart, said where somebody sees it before pressing rather than
        // after finding a web server listening.
        assert!(
            row.why.contains("cockpit") && row.why.contains("console"),
            "the Arch cost of installing the bridge is not in the sentence beside the button"
        );
    }

    /// The rule the catalog is built on, applied here: a request may choose
    /// which row runs, never what runs. A key that is not a row has to fall out
    /// silently, and a key that looks like a package name or a shell fragment
    /// has to fall out with it -- otherwise `/api/deps/install` is a way to
    /// A host with one compositor is not missing the other, and only one is
    /// ever proposed.
    ///
    /// The same rule the two engines follow, and the reason it is a `group` on
    /// the row rather than a property of the need: `Need::Streamed` also holds
    /// `flatpak` and `wayvnc`, which are requirements and not alternatives, so
    /// "one of these is enough" could not be answered per need without saying it
    /// about those two as well.
    #[test]
    fn one_compositor_is_enough_and_only_one_is_offered() {
        let comps: Vec<&Dep> =
            RUNTIME.iter().filter(|d| d.group == Some("compositor")).collect();
        assert!(comps.len() >= 2, "the compositor group is meant to have alternatives");
        assert_eq!(comps[0].key, "sway", "sway is preferred, and order is the preference");

        let absent = absent_for(Need::Streamed);
        let named: Vec<&str> = absent
            .iter()
            .filter(|d| d.group == Some("compositor"))
            .map(|d| d.key)
            .collect();
        if comps.iter().any(|d| d.present()) {
            assert!(named.is_empty(), "a compositor is here, so none is missing");
        } else {
            assert!(named.len() <= comps.len(), "never more than the alternatives");
        }
    }

    /// Podman is detected and never offered, which is the whole of the policy
    /// and the thing a well-meaning edit would undo first.
    ///
    /// Three properties, because losing any one of them re-opens it: it is in
    /// the table (so a host that has it is not told it has no engine), it is not
    /// offered (so no button proposes it), and it names no package (so there is
    /// nothing for a future `plan` to reach for).
    #[test]
    fn podman_is_used_where_it_is_found_and_never_put_there() {
        let podman = RUNTIME.iter().find(|d| d.key == "podman").expect("podman is still a row");
        assert!(podman.need == Need::Containers, "it has to count as an engine");
        assert!(!podman.offered, "podman must never be offered");
        assert!(
            podman.prereq.dnf.is_none()
                && podman.prereq.apt.is_none()
                && podman.prereq.pacman.is_none()
                && podman.prereq.zypper.is_none(),
            "a package name here is an offer waiting to be made by accident"
        );

        let docker = RUNTIME.iter().find(|d| d.key == "docker").expect("docker is a row");
        assert!(docker.offered, "docker is the engine WebDesk offers");
    }

    /// Naming podman in a request does not install it.
    ///
    /// The keys are filtered against the table and then again against `offered`,
    /// and this is the test for the second filter. Without it, "we do not offer
    /// podman" would be a property of the window rather than of the server, and
    /// a window is not where that decision can live.
    #[test]
    fn asking_for_podman_by_name_still_installs_nothing() {
        let m = Manager::Apt;
        let asked = vec!["podman".to_string()];
        assert_eq!(plan(&asked, m).unwrap(), Vec::<String>::new());
    }

    /// The removal verb is the narrow one on every manager.
    ///
    /// `purge` and `-Rns` take configuration and dependencies with them. WebDesk
    /// is taking off a package somebody else put on, so the wide verbs are
    /// theirs to type and not ours to choose -- and this is the assertion that
    /// notices when somebody "fixes" a removal that left files behind.
    #[test]
    fn removing_a_package_takes_only_that_package() {
        for m in [Manager::Dnf, Manager::Apt, Manager::Pacman, Manager::Zypper] {
            let v = remove_verb(m).join(" ");
            assert!(!v.contains("purge"), "{} would purge", m.bin());
            assert!(!v.contains("Rns") && !v.contains("Rs"), "{} would cascade", m.bin());
            assert!(
                v.contains("remove") || v.contains("-R"),
                "{} does not remove anything",
                m.bin()
            );
        }
    }

    /// A host with no spare podman is never asked to decide about one.
    ///
    /// `podman_spare` is the flag the window draws Keep and Remove from, and it
    /// is false on this machine, which has no podman at all. The paired
    /// assertion -- that a spare one carries a verdict -- lives in the report
    /// shape test, where the whole object is checked at once.
    #[test]
    fn there_is_no_decision_to_make_without_two_engines() {
        let e = report()["engine"].clone();
        if !e["docker"].as_bool().unwrap() || !e["podman"].as_bool().unwrap() {
            assert_eq!(e["podman_spare"], json!(false));
            assert!(e["removal"].is_null());
        }
    }

    /// install anything on the host.
    #[test]
    fn a_key_that_is_not_in_the_table_installs_nothing() {
        let junk: Vec<String> = ["cowsay", "docker; rm -rf /", "--allowerasing", "", "DOCKER"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(chosen(&junk).is_empty(), "a key outside the table survived the filter");

        // And the whole way through: with nothing left to install, `plan` has
        // no package name to hand a package manager whichever manager is here.
        for m in [Manager::Dnf, Manager::Apt, Manager::Pacman, Manager::Zypper] {
            assert_eq!(plan(&junk, m), Ok(Vec::new()), "{} was given something to install", m.bin());
        }

        // A real key alongside the junk still resolves to exactly one row, so
        // this is a filter and not a blanket refusal.
        let mixed = vec!["cowsay".to_string(), "flatpak".to_string()];
        let picked: Vec<&str> = chosen(&mixed).iter().map(|d| d.key).collect();
        assert_eq!(picked, vec!["flatpak"]);
    }

    /// The exact shape the Apps window is written against. Checked field by
    /// field rather than by comparing a whole blob, because the failure this
    /// prevents is one renamed key silently emptying a column in a window that
    /// lives in another file.
    #[test]
    fn report_is_the_shape_the_apps_window_reads() {
        let r = report();
        let deps = r["deps"].as_array().expect("deps is an array");
        assert_eq!(deps.len(), RUNTIME.len());

        // `manager` is a package manager's binary name or null, and never
        // anything else -- the window prints it in a sentence.
        let m = &r["manager"];
        assert!(
            m.is_null() || matches!(m.as_str(), Some("dnf" | "apt-get" | "pacman" | "zypper")),
            "manager came back as {m}"
        );

        for (got, want) in deps.iter().zip(RUNTIME.iter()) {
            assert_eq!(got["key"].as_str(), Some(want.key));
            assert_eq!(got["label"].as_str(), Some(want.label));
            assert_eq!(got["why"].as_str(), Some(want.why));
            assert_eq!(got["need"].as_str(), Some(want.need.as_str()));
            assert!(got["present"].is_boolean(), "{}: present is not a bool", want.key);
            assert!(
                got["package"].is_null() || got["package"].is_string(),
                "{}: package is neither a name nor null",
                want.key
            );
            assert!(got["offered"].is_boolean(), "{}: offered is not a bool", want.key);
            assert_eq!(got["group"].as_str(), want.group, "{}: group came back wrong", want.key);
            // Eight keys and no ninth: an extra field here is a field the
            // window will not be reading, which is how a UI and an API drift.
            assert_eq!(got.as_object().map(|o| o.len()), Some(8));
        }

        // The engine object is not one of the rows and is always present, so a
        // window can ask "which engine" without first working out which rows
        // happen to be about engines.
        let e = &r["engine"];
        assert!(e["docker"].is_boolean() && e["podman"].is_boolean());
        assert!(e["podman_spare"].is_boolean());
        assert!(e["in_use"].is_null() || matches!(e["in_use"].as_str(), Some("docker" | "podman")));
        // A decision is only ever offered where there is one to make: both
        // engines present and the spare no longer doing anything.
        if e["podman_spare"] == json!(true) {
            assert!(e["removal"]["allowed"].is_boolean(), "a spare podman needs a verdict");
        } else {
            assert!(e["removal"].is_null(), "no spare engine, so nothing to decide");
        }
        assert_eq!(r.as_object().map(|o| o.len()), Some(3));
    }

    /// `need` serialises as the three words and nothing else. The window groups
    /// on this string, so a fourth spelling would file a dependency under a
    /// heading that does not exist and hide it.
    #[test]
    fn a_need_has_exactly_three_spellings() {
        let mut seen: Vec<&str> = Need::ALL.iter().map(|n| n.as_str()).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec!["containers", "host", "streamed"]);
        for d in RUNTIME {
            assert!(Need::ALL.contains(&d.need), "{} is in no group", d.key);
        }
    }

    /// One engine is enough. A host with Docker must not be told it is missing
    /// Podman -- that refusal would stop an install that was about to work.
    /// Both halves are asserted, so this says something on a developer machine
    /// with an engine and on a build host with none.
    #[test]
    fn a_host_with_one_container_engine_is_not_missing_the_other() {
        let have_one = RUNTIME
            .iter()
            .filter(|d| d.need == Need::Containers)
            .any(|d| d.present());
        if have_one {
            assert!(absent_for(Need::Containers).is_empty());
        } else {
            let keys: Vec<&str> = absent_for(Need::Containers).iter().map(|d| d.key).collect();
            // Docker alone, though Podman would satisfy the group just as well
            // if it were here. Either engine *counts*; only one is ever *put*
            // on a host, and this is the assertion that keeps those two
            // sentences from collapsing into each other.
            assert_eq!(keys, vec!["docker"], "only the offered engine is proposed");
        }
    }
}

/// `POST /api/deps/podman/remove` -- `{"confirm": true}`.
///
/// The one place WebDesk takes a package off a host, and it is deliberately the
/// narrowest door in this file: one package, named here and not by the request,
/// admin-gated, and refused outright unless every condition `podman_removal`
/// checks still holds.
///
/// **The checks are made again here rather than trusted from the report.** The
/// window was painted at some point in the past; a container can have been
/// started since, by somebody who is not looking at this screen. A confirmation
/// says the operator meant it, not that the machine has stood still, and those
/// are different facts with different lifetimes.
///
/// It refuses while Podman is the engine in use. Removing the thing currently
/// running the desktop entries is not a decision anybody makes on purpose from
/// a dependency panel, and the honest order is Docker first.
pub async fn deps_remove_podman(
    State(s): State<AppState>,
    h: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let session = match admin_session(&s, &h) {
        Ok(v) => v,
        Err(r) => return r,
    };
    #[derive(Deserialize)]
    struct Req {
        #[serde(default)]
        confirm: bool,
    }
    let req: Req = serde_json::from_slice(&body).unwrap_or(Req { confirm: false });
    if !req.confirm {
        return bad(StatusCode::BAD_REQUEST, "removing podman has to be confirmed");
    }
    if which("podman").is_none() {
        return bad(StatusCode::NOT_FOUND, "this host has no podman to remove");
    }
    if crate::engine::detect().map(|e| e.bin()) == Some("podman") {
        return bad(
            StatusCode::CONFLICT,
            "podman is the engine WebDesk is using on this host. Install Docker first, which \
             takes over on its own, and then this becomes possible.",
        );
    }
    let plan = podman_removal();
    if plan["allowed"] != json!(true) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": plan["reason"].as_str().unwrap_or("podman cannot be removed here"),
                "removal": plan,
            })),
        )
            .into_response();
    }
    let Some(m) = flatpak::manager() else {
        return bad(StatusCode::CONFLICT, "no package manager on this host");
    };

    if read_status()["state"] == "running" {
        return bad(StatusCode::CONFLICT, "another install is running on this host");
    }
    let actor = session.ident.username.clone();
    let _ = write_status(&json!({
        "state": "running", "phase": "packages", "slug": "podman", "name": "Podman",
        "started": now(), "actor": actor,
    }));
    let _ = std::fs::write(log_file(), b"");

    tracing::warn!(user = %actor, "removing podman at an operator's request");

    let done = tokio::task::spawn_blocking(move || {
        let mut args: Vec<String> = remove_verb(m).iter().map(|a| a.to_string()).collect();
        args.push("podman".into());
        crate::flatpak::logged(m.bin(), &args, &log_file())
    })
    .await;

    let outcome = match done {
        Ok(Ok(())) => json!({
            "state": "done", "slug": "podman", "name": "Podman",
            "finished": now(), "actor": actor,
        }),
        Ok(Err(e)) => json!({
            "state": "failed", "slug": "podman", "name": "Podman",
            "finished": now(), "actor": actor, "error": e,
        }),
        Err(e) => json!({
            "state": "failed", "slug": "podman", "name": "Podman",
            "finished": now(), "actor": actor, "error": e.to_string(),
        }),
    };
    let failed = outcome["state"] == json!("failed");
    let _ = write_status(&outcome);
    if failed {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": outcome["error"].clone() })),
        )
            .into_response();
    }
    Json(json!({ "ok": true, "engine": engine_report() })).into_response()
}
