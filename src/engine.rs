//! The container engine, as a thin wrapper over its command line.
//!
//! Docker is what the target hosts have, so Docker is what this is written
//! against. Podman is accepted because the subcommands used here -- `pull`,
//! `run`, `start`, `stop`, `rm`, `inspect` -- take the same arguments in both,
//! so supporting it costs one extra name in a lookup table. **It is untested.**
//! `WD_CONTAINER_ENGINE` names one explicitly if the guess is wrong.
//!
//! The CLI rather than the socket API: it is one dependency instead of an HTTP
//! client speaking a versioned JSON protocol over a Unix socket, and it is the
//! same trade update.rs makes when it shells out to curl. Nothing here builds a
//! shell command from a string -- every argument is a separate argv element, so
//! a value a user typed cannot become an argument, let alone a command.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Prefix for every container this program creates. Also how it recognises its
/// own containers when reconciling state against reality.
pub const PREFIX: &str = "webdesk-";

/// Marks a container as ours, so `docker ps` can be filtered to it.
pub const LABEL: &str = "webdesk.managed=1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Docker,
    Podman,
}

impl Engine {
    pub fn bin(&self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman (untested)",
        }
    }
}

fn which(prog: &str) -> Option<PathBuf> {
    std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into())
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| std::path::Path::new(d).join(prog))
        .find(|p| p.is_file())
}

/// Which engine this host has, preferring Docker. `None` means neither is
/// installed, which is the one condition the Apps window reports as fatal.
pub fn detect() -> Option<Engine> {
    if let Ok(forced) = std::env::var("WD_CONTAINER_ENGINE") {
        return match forced.trim() {
            "docker" => Some(Engine::Docker),
            "podman" => Some(Engine::Podman),
            "" => None,
            other => {
                tracing::warn!("WD_CONTAINER_ENGINE={other} is not a known engine");
                None
            }
        };
    }
    if which("docker").is_some() {
        return Some(Engine::Docker);
    }
    if which("podman").is_some() {
        return Some(Engine::Podman);
    }
    None
}

/// Is the engine actually usable, not merely installed? A Docker binary with a
/// daemon that is not running fails here rather than at install time.
pub fn probe(engine: Engine) -> Result<String, String> {
    let out = Command::new(engine.bin())
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map_err(|e| format!("could not run {}: {e}", engine.bin()))?;
    if out.status.success() {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok(if v.is_empty() { "unknown".into() } else { v });
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.is_empty() {
        format!("{} is installed but not responding", engine.bin())
    } else {
        err
    })
}

pub fn container_name(slug: &str) -> String {
    format!("{PREFIX}{slug}")
}

/// Shared with every container app, at the same path inside as out.
const DEFAULT_HOME_DIR: &str = "/home";

/// The host directory holding home directories, or `off`.
fn home_dir_setting() -> String {
    std::env::var("WD_HOME_MOUNT")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_HOME_DIR.to_string())
}

/// The host's home directories, mounted into every container at the same path.
///
/// A packaged application expects `/home` to mean what it means everywhere
/// else: the place a user's files live, at a path their documents already name.
/// Without it an app sees only its own state directory, so "open a file" has
/// nothing to open and a path copied from a terminal resolves to nothing. This
/// is the one bind mount WebDesk adds on its own, and it is deliberately the
/// same directory for every app -- an installed app is part of the host, not a
/// possession of whoever installed it.
///
/// **This is a real widening.** Every container app can read and write every
/// home directory on the machine, so installing one is a decision about all of
/// them. `WD_HOME_MOUNT` names a different directory to share, or `off` to
/// share none, for a host where that trade is the wrong one.
pub fn home_mount() -> Option<(String, String, bool)> {
    let want = home_dir_setting();
    if want == "off" {
        return None;
    }
    // Named rather than assumed: binding a path the host does not have gets an
    // empty directory created under it by the engine, which is a confusing way
    // to discover a typo.
    if !std::path::Path::new(&want).is_dir() {
        tracing::warn!("{want} is not a directory; no home directories will be shared");
        return None;
    }
    Some((want.clone(), want, false))
}

/// Is SELinux in the picture? On the RHEL side of the target list it usually
/// is, and a bind mount that has not been relabelled is simply unreadable to
/// the container -- which presents as an app that starts and then behaves as
/// though its data directory were empty.
///
/// Checked by the presence of the filesystem rather than by running
/// `getenforce`, which is not installed everywhere it applies.
fn selinux() -> bool {
    std::path::Path::new("/sys/fs/selinux/enforce").exists()
}

/// The relabelling suffix for one mount, if any.
///
/// `Z` is a private label -- this container and nothing else -- and is right
/// for the `/config` directory, which WebDesk created and no one else uses.
/// Anywhere else is a directory the user already had and may still want to
/// reach, so it gets the shared `z` instead. Relabelling somebody's media
/// library exclusively to one container would be a surprising thing to do on
/// their behalf.
///
/// The shared home directories get neither. `z` relabels the whole tree it is
/// given, and rewriting the labels under `/home` breaks what reads it from
/// outside a container -- sshd stops reading `~/.ssh`. A mount WebDesk adds to
/// every app on its own must not do that to the host, so it is left alone. On
/// an enforcing host that may mean an app cannot read it, which is the smaller
/// failure and the recoverable one.
///
/// The engine socket is left alone for the same reason and more sharply. It is
/// not WebDesk's file: the daemon and every other client on the host are using
/// it right now, and relabelling it to suit one container is a change to
/// something the machine depends on to run containers at all. An operator who
/// wants that made to work on an enforcing host should say so in policy, where
/// it is visible and reversible, rather than have an install quietly do it.
fn relabel_for(container_path: &str) -> Option<char> {
    if !selinux()
        || container_path == home_dir_setting()
        || container_path.ends_with(".sock")
    {
        return None;
    }
    Some(if container_path == "/config" { 'Z' } else { 'z' })
}

/// Everything needed to create one container. Assembled by `apps.rs` from a
/// catalog entry plus the answers a user gave.
pub struct RunSpec {
    pub slug: String,
    pub image: String,
    pub host_port: u16,
    pub container_port: u16,
    pub env: Vec<(String, String)>,
    /// `(host path, container path, read-only)`
    pub mounts: Vec<(String, String, bool)>,
    /// `--shm-size`, for the desktop images that run a real browser or IDE.
    pub shm: Option<String>,
}

impl RunSpec {
    fn args(&self) -> Vec<String> {
        let name = container_name(&self.slug);
        let mut a: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            "--name".into(),
            name,
            "--label".into(),
            LABEL.into(),
            "--label".into(),
            format!("webdesk.slug={}", self.slug),
            "--restart".into(),
            "unless-stopped".into(),
        ];

        // Bound to the loopback interface on purpose. The only route to a
        // container app is through WebDesk's proxy, which means through the
        // session cookie -- publishing on 0.0.0.0 would quietly put an
        // unauthenticated copy of every app on the network.
        a.push("-p".into());
        a.push(format!("127.0.0.1:{}:{}", self.host_port, self.container_port));

        // Only ever a tmpfs size. Worth saying plainly because "--shm-size" and
        // "--security-opt" get mentioned in the same breath in image docs, and
        // this program passes the first and never the second.
        if let Some(shm) = &self.shm {
            a.push("--shm-size".into());
            a.push(shm.clone());
        }

        for (k, v) in &self.env {
            a.push("-e".into());
            a.push(format!("{k}={v}"));
        }
        for (host, at, ro) in &self.mounts {
            let mut opts: Vec<String> = Vec::new();
            if *ro {
                opts.push("ro".into());
            }
            if let Some(z) = relabel_for(at) {
                opts.push(z.to_string());
            }
            a.push("-v".into());
            a.push(if opts.is_empty() {
                format!("{host}:{at}")
            } else {
                format!("{host}:{at}:{}", opts.join(","))
            });
        }
        a.push(self.image.clone());
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RunSpec {
        RunSpec {
            slug: "demo".into(),
            image: "lscr.io/linuxserver/demo:latest".into(),
            host_port: 47000,
            container_port: 8080,
            env: vec![("TZ".into(), "Etc/UTC".into())],
            mounts: vec![
                ("/srv/media".into(), "/media".into(), true),
                ("/var/lib/webdesk/appdata/demo".into(), "/config".into(), false),
            ],
            shm: Some("1g".into()),
        }
    }

    #[test]
    fn shm_size_is_passed_when_an_entry_asks_for_it() {
        let args = spec().args();
        let at = args.iter().position(|a| a == "--shm-size").expect("no --shm-size");
        assert_eq!(args[at + 1], "1g");
    }

    #[test]
    fn nothing_ever_loosens_the_sandbox() {
        let args = spec().args().join(" ");
        // --shm-size is a tmpfs size. These are the things that are not, and
        // this program has no code path that emits any of them.
        for forbidden in ["--security-opt", "--privileged", "--cap-add", "--network=host", "--pid=host"] {
            assert!(!args.contains(forbidden), "{forbidden} appeared in {args}");
        }
    }

    #[test]
    fn no_shm_means_no_flag() {
        let mut s = spec();
        s.shm = None;
        assert!(!s.args().contains(&"--shm-size".to_string()));
    }

    #[test]
    fn the_published_port_is_bound_to_loopback_only() {
        let args = spec().args();
        let at = args.iter().position(|a| a == "-p").expect("no -p");
        // The whole access-control story rests on this one string: anything
        // other than 127.0.0.1 puts an unauthenticated app on the network.
        assert_eq!(args[at + 1], "127.0.0.1:47000:8080");
    }

    #[test]
    fn the_image_is_the_last_argument() {
        let args = spec().args();
        assert_eq!(args.last().unwrap(), "lscr.io/linuxserver/demo:latest");
    }

    #[test]
    fn a_read_only_mount_says_so() {
        let args = spec().args();
        let mounts: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0 && args[i - 1] == "-v")
            .map(|(_, v)| v)
            .collect();
        assert!(mounts.iter().any(|m| m.starts_with("/srv/media:/media:ro")), "{mounts:?}");
        assert!(
            mounts.iter().any(|m| m.starts_with("/var/lib/webdesk/appdata/demo:/config")
                && !m.contains(":ro")),
            "{mounts:?}"
        );
    }

    #[test]
    fn the_container_is_labelled_and_named_for_its_slug() {
        let args = spec().args();
        assert!(args.contains(&container_name("demo")));
        assert!(args.contains(&LABEL.to_string()));
        assert!(args.contains(&"webdesk.slug=demo".to_string()));
    }

    #[test]
    fn selinux_relabelling_follows_the_mount() {
        // Only meaningful where SELinux exists; elsewhere it must add nothing,
        // which is what the assertions below check on this machine.
        if selinux() {
            assert_eq!(relabel_for("/config"), Some('Z'));
            assert_eq!(relabel_for("/media"), Some('z'));
        } else {
            assert_eq!(relabel_for("/config"), None);
            assert_eq!(relabel_for("/media"), None);
        }
    }

    #[test]
    fn the_shared_home_is_never_relabelled() {
        // True on either kind of host, which is the point: relabelling /home
        // would rewrite the labels sshd and everything else outside a
        // container rely on, and WebDesk adds this mount unasked.
        assert_eq!(relabel_for("/home"), None);
    }

    #[test]
    fn the_engine_socket_is_never_relabelled() {
        // Relabelling it would change a file the daemon and every other client
        // on this host are using, to suit one container. Left alone on either
        // kind of host.
        assert_eq!(relabel_for("/var/run/docker.sock"), None);
    }

    #[test]
    fn a_shared_home_is_mounted_read_write_at_the_same_path() {
        // Skipped where the host has no /home to share -- the mount is only
        // ever added for a directory that is really there.
        if let Some((host, at, ro)) = home_mount() {
            assert_eq!(host, at, "the shared home must appear at the path it has outside");
            assert!(!ro, "an app that cannot write a home directory cannot save a file");
        }
    }
}

/// Run one engine command, appending everything it prints to `log`.
///
/// Output is merged and streamed to the file as it arrives rather than
/// collected, because `pull` on a slow link is the longest thing this program
/// does and a progress log that only appears at the end is not a progress log.
pub fn run_logged(engine: Engine, args: &[String], log: &std::path::Path) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| format!("could not open the install log: {e}"))?;

    let _ = writeln!(file, "$ {} {}", engine.bin(), args.join(" "));

    let err_file = file.try_clone().map_err(|e| format!("could not open the install log: {e}"))?;
    let status = Command::new(engine.bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .status()
        .map_err(|e| format!("could not run {}: {e}", engine.bin()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{} {} failed ({})", engine.bin(), args[0], status))
    }
}

pub fn pull(engine: Engine, image: &str, log: &std::path::Path) -> Result<(), String> {
    run_logged(engine, &["pull".to_string(), image.to_string()], log)
}

pub fn create(engine: Engine, spec: &RunSpec, log: &std::path::Path) -> Result<(), String> {
    run_logged(engine, &spec.args(), log)
}

/// One engine command whose output is wanted rather than logged.
fn capture(engine: Engine, args: &[&str]) -> Result<String, String> {
    let out = Command::new(engine.bin())
        .args(args)
        .output()
        .map_err(|e| format!("could not run {}: {e}", engine.bin()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("{} failed", args[0]) } else { err });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn start(engine: Engine, slug: &str) -> Result<(), String> {
    capture(engine, &["start", &container_name(slug)]).map(|_| ())
}

pub fn stop(engine: Engine, slug: &str) -> Result<(), String> {
    capture(engine, &["stop", &container_name(slug)]).map(|_| ())
}

/// Remove the container. Its `/config` directory is left alone -- see
/// `apps::remove`, which decides separately whether the data goes too.
pub fn remove(engine: Engine, slug: &str) -> Result<(), String> {
    capture(engine, &["rm", "-f", &container_name(slug)]).map(|_| ())
}

/// `running`, `exited`, `created`, ... or `missing` when there is no such
/// container. Never an error: a container that has been removed behind our back
/// is a state to report, not a failure to handle.
pub fn state(engine: Engine, slug: &str) -> String {
    match capture(engine, &["inspect", "-f", "{{.State.Status}}", &container_name(slug)]) {
        Ok(s) if !s.is_empty() => s,
        _ => "missing".into(),
    }
}
