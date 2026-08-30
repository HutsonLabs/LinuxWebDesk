//! Flatpak, for the host services that are packaged as one.
//!
//! `engine.rs` runs containers and `systemd.rs` runs units; this is the third
//! thing a host application can need, and it sits between them. The Flatpak is
//! *what* runs -- the binary the unit's `ExecStart` names -- so it has to be on
//! the machine before the unit is ever started, or the service is written,
//! enabled, and dead within a second, with nothing in the Apps window saying
//! why.
//!
//! **Nothing here takes a name from a request.** The application id, the
//! repository the bundle comes from and the host packages that may be
//! installed are all `&'static str` in `catalog.rs`, for the same reason a unit
//! name is: a request that could name a package is a request that can install
//! anything, which is a larger hole than any container in this catalog.
//!
//! **There is no remote to update from.** term.hut's bundles are built with
//! `flatpak build-bundle` and no `--runtime-repo`, so the installed app reports
//! an origin that `flatpak remotes` has never heard of and `flatpak update`
//! answers "Nothing to do" forever. Installing is downloading a file, and so is
//! upgrading -- which is why `newest_bundle` exists rather than a one-line
//! `flatpak install` against a remote.

use crate::catalog::{Flatpak, FlatpakSource, Prereq};
use crate::engine::which;
use std::path::Path;
use std::process::{Command, Stdio};

/// Whether this host already has the application.
///
/// `flatpak info` rather than parsing `flatpak list`: it exits non-zero for an
/// id that is not installed, so the answer is the exit code and there is no
/// output to misread.
pub fn installed(id: &str) -> bool {
    Command::new("flatpak")
        .args(["info", id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// This host's architecture in the words a release asset is named with.
///
/// The two the bundles are built for. Anything else returns `None` and the
/// install refuses rather than downloading a bundle that cannot run here.
pub fn arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

/// The host package managers this knows how to install a package with.
#[derive(Clone, Copy, PartialEq)]
pub enum Manager {
    Dnf,
    Apt,
    Pacman,
    Zypper,
}

impl Manager {
    pub fn bin(&self) -> &'static str {
        match self {
            Manager::Dnf => "dnf",
            Manager::Apt => "apt-get",
            Manager::Pacman => "pacman",
            Manager::Zypper => "zypper",
        }
    }

    /// The argv that installs packages without asking anybody anything. This
    /// runs with no terminal attached, so a manager that stops to confirm would
    /// hang the install rather than fail it.
    fn args(&self) -> &'static [&'static str] {
        match self {
            Manager::Dnf => &["install", "-y"],
            Manager::Apt => &["install", "-y"],
            Manager::Pacman => &["-S", "--noconfirm"],
            Manager::Zypper => &["install", "-y"],
        }
    }

    /// What this entry calls the package providing a prerequisite, or `None`
    /// where nobody has checked. See `catalog::Prereq`.
    fn package<'a>(&self, p: &'a Prereq) -> Option<&'a str> {
        match self {
            Manager::Dnf => p.dnf,
            Manager::Apt => p.apt,
            Manager::Pacman => p.pacman,
            Manager::Zypper => p.zypper,
        }
    }
}

/// Which manager this host has, in the order the target distributions are
/// listed in the README. `None` is a host WebDesk will not install packages on,
/// which is reported rather than guessed at.
pub fn manager() -> Option<Manager> {
    for m in [Manager::Dnf, Manager::Apt, Manager::Pacman, Manager::Zypper] {
        if which(m.bin()).is_some() {
            return Some(m);
        }
    }
    None
}

/// What is missing before this service could start: flatpak itself, and any
/// host program the unit's `ExecStart` names.
///
/// Returned as the package names to install, so the caller can put them in a
/// sentence before installing anything. An empty vector means nothing is
/// needed; `Err` means something is needed that this host has no known package
/// for, and the entry's `provision` text is the answer instead.
pub fn missing_packages(needs: &[Prereq]) -> Result<Vec<String>, String> {
    let flatpak = Prereq {
        bin: "flatpak",
        dnf: Some("flatpak"),
        apt: Some("flatpak"),
        pacman: Some("flatpak"),
        zypper: Some("flatpak"),
    };
    let wanted: Vec<&Prereq> = std::iter::once(&flatpak)
        .chain(needs.iter())
        .filter(|p| which(p.bin).is_none())
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let Some(m) = manager() else {
        return Err(format!(
            "this host is missing {} and has no package manager WebDesk knows",
            names(&wanted)
        ));
    };
    let mut out = Vec::new();
    for p in &wanted {
        match m.package(p) {
            Some(pkg) => out.push(pkg.to_string()),
            None => {
                return Err(format!(
                    "this host is missing {}, and WebDesk does not know which {} package \
                     provides it",
                    p.bin,
                    m.bin()
                ))
            }
        }
    }
    Ok(out)
}

fn names(ps: &[&Prereq]) -> String {
    let v: Vec<&str> = ps.iter().map(|p| p.bin).collect();
    v.join(" and ")
}

/// Install host packages, with everything the manager says going to the log the
/// Apps window is already streaming.
pub fn install_packages(packages: &[String], log: &Path) -> Result<(), String> {
    let Some(m) = manager() else {
        return Err("no package manager on this host".into());
    };
    let mut args: Vec<String> = m.args().iter().map(|s| s.to_string()).collect();
    args.extend(packages.iter().cloned());
    logged(m.bin(), &args, log)
}

/// The newest release carrying a bundle for this architecture, as
/// `(version, download url)`.
///
/// Walks the releases rather than taking `/releases/latest`, and that is not
/// caution for its own sake: term.hut's newest release at the time of writing
/// is a macOS-only build with no `.flatpak` asset at all, so `latest` would
/// have this refuse to install on a host where twelve usable bundles are one
/// page down.
pub fn newest_bundle(repo: &str) -> Result<(String, String), String> {
    let Some(arch) = arch() else {
        return Err(format!("no term.hut bundle is built for {}", std::env::consts::ARCH));
    };
    let suffix = format!("_{arch}.flatpak");
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    let releases = crate::update::github_json(&url)?;
    let list = releases.as_array().ok_or("unexpected release list")?;

    for release in list {
        let Some(assets) = release["assets"].as_array() else { continue };
        for asset in assets {
            let name = asset["name"].as_str().unwrap_or_default();
            if !name.ends_with(&suffix) {
                continue;
            }
            let Some(url) = asset["browser_download_url"].as_str() else { continue };
            let version = release["tag_name"]
                .as_str()
                .unwrap_or_default()
                .trim_start_matches('v')
                .to_string();
            return Ok((version, url.to_string()));
        }
    }
    Err(format!("no {arch} bundle in the last 30 releases of {repo}"))
}

/// Flathub's repository description, as `flatpak remote-add` takes it.
///
/// A constant here rather than a field on an entry. A remote is where code comes
/// from, so a remote an entry could name is a remote a request could name one
/// refactor later -- the same rule that keeps unit bodies and application ids in
/// `catalog.rs` out of reach of the browser.
pub const FLATHUB_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// Add the Flathub remote if this host has not got it. Idempotent by the flag,
/// so this is safe to call before every install and cheap when it is a no-op.
pub fn ensure_flathub(log: &Path) -> Result<(), String> {
    logged(
        "flatpak",
        &[
            "remote-add".into(),
            "--if-not-exists".into(),
            "--system".into(),
            "flathub".into(),
            FLATHUB_URL.into(),
        ],
        log,
    )
}

/// Put the application on the host, whichever way this entry gets one.
///
/// The one entry point the installer calls, so that "where does this Flatpak
/// come from" is answered here and not in `apps.rs`. Already-installed is the
/// ordinary case on a host that had the app before this entry did, and
/// reinstalling over it would cost minutes to arrive where it already is.
pub fn provide(fp: &Flatpak, log: &Path) -> Result<(), String> {
    if installed(fp.id) {
        return Ok(());
    }
    match fp.source {
        FlatpakSource::Flathub => {
            ensure_flathub(log)?;
            logged(
                "flatpak",
                &[
                    "install".into(),
                    "-y".into(),
                    "--system".into(),
                    "--noninteractive".into(),
                    "flathub".into(),
                    fp.id.into(),
                ],
                log,
            )
        }
        FlatpakSource::Bundle { repo } => {
            let (version, url) = newest_bundle(repo)?;
            tracing::info!(id = %fp.id, %version, "installing a bundle");
            install_bundle(&url, log)
        }
    }
}

/// Bring the application up to date, which is two different operations.
///
/// A remote has a repository behind it, so this is one command. A bundle has
/// none -- `flatpak update` answers "Nothing to do" forever against an origin no
/// remote knows -- so upgrading is downloading the newest file again.
pub fn update(fp: &Flatpak, log: &Path) -> Result<(), String> {
    match fp.source {
        FlatpakSource::Flathub => logged(
            "flatpak",
            &[
                "update".into(),
                "-y".into(),
                "--system".into(),
                "--noninteractive".into(),
                fp.id.into(),
            ],
            log,
        ),
        FlatpakSource::Bundle { repo } => {
            let (_, url) = newest_bundle(repo)?;
            install_bundle(&url, log)
        }
    }
}

/// Download a bundle and install it system-wide.
///
/// `--system` rather than `--user` because the unit is a system unit: a user
/// installation lives under the installing user's home and would not be there
/// for the service. `--reinstall` so that installing over the same version is
/// an upgrade path rather than an error.
pub fn install_bundle(url: &str, log: &Path) -> Result<(), String> {
    let file = std::env::temp_dir().join("webdesk-termhut.flatpak");
    let path = file.to_string_lossy().to_string();
    logged(
        "curl",
        &["-fsSL".into(), "--max-time".into(), "900".into(), "-o".into(), path.clone(), url.into()],
        log,
    )?;
    let result = logged(
        "flatpak",
        &["install".into(), "-y".into(), "--system".into(), "--reinstall".into(), path.clone()],
        log,
    );
    // The bundle is a few hundred megabytes and nothing reads it again.
    let _ = std::fs::remove_file(&file);
    result
}

/// Keep the service user's runtime directory and bus alive with nobody logged
/// in, which is what `flatpak-spawn --host` needs to reach the portal.
///
/// Best effort: a host where this fails is one where the service may still come
/// up, and a hard failure here would refuse an install over something only some
/// of the app's features need.
pub fn enable_linger(user: &str) {
    let _ = Command::new("loginctl")
        .args(["enable-linger", user])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// One command, with its output appended to the install log.
///
/// The same shape as `engine::run_logged` and for the same reason: the Apps
/// window is already polling this file, so anything written here is on screen
/// while it happens rather than summarised after it fails.
fn logged(bin: &str, args: &[String], log: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| format!("could not open the install log: {e}"))?;
    let _ = writeln!(file, "$ {bin} {}", args.join(" "));

    let err_file = file.try_clone().map_err(|e| format!("could not open the install log: {e}"))?;
    let status = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .status()
        .map_err(|e| format!("could not run {bin}: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{bin} {} failed ({status})", args.first().map(String::as_str).unwrap_or("")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An id nothing could have installed reads as absent rather than as an
    /// error, on a host with flatpak and on one without -- the second half is
    /// what keeps this runnable on the machines this is developed on.
    #[test]
    fn an_app_that_is_not_installed_is_not_an_error() {
        assert!(!installed("com.example.definitely-not-installed"));
    }

    /// Every prerequisite a shipping entry names must be installable on the
    /// managers the README claims to target, or the entry has to say so by
    /// leaving the name `None` -- which is a refusal with instructions, not a
    /// wrong package. This catches a `Prereq` added with the field forgotten.
    #[test]
    fn a_prerequisite_is_either_named_or_deliberately_not() {
        for app in crate::catalog::CATALOG {
            let Some(host) = &app.host else { continue };
            let Some(fp) = &host.flatpak else { continue };
            for p in fp.needs {
                assert!(!p.bin.is_empty(), "{} names a prerequisite with no binary", app.slug);
                assert!(
                    p.dnf.is_some() || p.apt.is_some() || p.pacman.is_some() || p.zypper.is_some(),
                    "{}: {} is installable nowhere, so it can never be provided",
                    app.slug,
                    p.bin
                );
            }
        }
    }

    /// The unit's `ExecStart` and the `Flatpak.id` beside it name the same
    /// application, and they have to: `ExecStartPre` and `ExecStop` kill *by
    /// id*, so an id that drifted from the command would leave the service
    /// unable to stop the thing it just started -- which is the exact failure
    /// the leading `flatpak kill` was added to prevent, wearing a new hat.
    #[test]
    fn the_unit_runs_the_flatpak_the_entry_names() {
        for app in crate::catalog::CATALOG {
            let Some(host) = &app.host else { continue };
            let Some(fp) = &host.flatpak else { continue };
            assert!(
                host.unit_body.contains(&format!("flatpak run {}", fp.id)),
                "{} would start something other than {}",
                app.slug,
                fp.id
            );
            assert!(
                host.unit_body.contains(&format!("flatpak kill {}", fp.id)),
                "{} could not stop {}",
                app.slug,
                fp.id
            );
            // Every host program the unit needs must be one the entry declares,
            // or the install checks for something the unit never uses while the
            // thing it does use goes unchecked.
            for p in fp.needs {
                assert!(
                    host.unit_body.contains(p.bin),
                    "{} declares {} but never runs it",
                    app.slug,
                    p.bin
                );
            }
        }
    }

    /// The constraint that came with removing the container entry: an
    /// application served from the host must not also be in the catalog as an
    /// image. If it were, a host install that could not be provided would have
    /// somewhere to quietly fall back to -- and "install this terminal" would
    /// hand back a shell on the wrong machine.
    #[test]
    fn nothing_served_from_the_host_is_also_offered_as_a_container() {
        for app in crate::catalog::CATALOG.iter().filter(|a| a.host.is_some()) {
            let twin = crate::catalog::CATALOG
                .iter()
                .find(|o| o.slug != app.slug && o.name == app.name && !o.image.is_empty());
            assert!(
                twin.is_none(),
                "{} is also in the catalog as a container, which is a fallback nobody asked for",
                app.name
            );
        }
    }

    /// The architectures the bundles are built for. A host that is neither is
    /// told so instead of being handed a bundle that cannot run.
    #[test]
    fn only_the_two_built_architectures_are_offered() {
        assert!(matches!(arch(), Some("x86_64") | Some("aarch64") | None));
    }
}
