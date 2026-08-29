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

pub(crate) fn which(prog: &str) -> Option<PathBuf> {
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

/// Where the host keeps its fonts, or `off`.
const DEFAULT_FONT_DIR: &str = "/usr/share/fonts";

/// Where they are mounted inside, which is deliberately **not** the path they
/// have outside.
///
/// Every other mount WebDesk adds appears at the path it has on the host, and
/// this is the one exception, because here the path is the whole point. Binding
/// the host's fonts over `/usr/share/fonts` *replaces* the image's own set and
/// makes document rendering worse, not better: measured on the OnlyOffice
/// image, `fc-match Arial` degrades from `Arial.ttf` to `NimbusSans-Regular.otf`
/// and `Times New Roman` to `NimbusRoman-Regular.otf`, because the image ships
/// the metric-compatible originals and a typical Linux host does not.
///
/// `/usr/local/share/fonts` is the path fontconfig already scans *in addition*
/// to the system one -- both image families list it in `/etc/fonts/fonts.conf`
/// -- so the host's fonts are added to the image's rather than put in front of
/// them. Measured on the same image: 507 families become 1071, and `fc-match
/// Arial` still answers `Arial.ttf`.
const FONTS_AT: &str = "/usr/local/share/fonts";

fn font_dir_setting() -> String {
    std::env::var("WD_FONT_MOUNT")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_FONT_DIR.to_string())
}

/// The host's fonts, added to those of an application that draws.
///
/// A natively installed application can use every font on the machine. A
/// container can use only what its image shipped, and the two sets are not
/// nested in either direction -- on the deployment host the image carries 239
/// families the host has not got, and the host carries families the image has
/// not, including the Droid script coverage that decides whether Hebrew, Thai,
/// Devanagari and Japanese render as text or as empty boxes.
///
/// Read-only, because this is the host's font directory and an application has
/// no business writing to it. `WD_FONT_MOUNT` names another directory, or `off`
/// to share none.
pub fn font_mount() -> Option<(String, String, bool)> {
    let want = font_dir_setting();
    if want == "off" {
        return None;
    }
    if !std::path::Path::new(&want).is_dir() {
        tracing::debug!("{want} is not a directory; no fonts will be shared");
        return None;
    }
    Some((want, FONTS_AT.to_string(), true))
}

/// The host's graphics devices, or `off`.
///
/// A directory rather than a device, because which node is which differs per
/// machine -- `renderD128` on a host with one GPU, `renderD129` on one where
/// something else enumerated first.
const DEFAULT_GPU_DIR: &str = "/dev/dri";

fn gpu_dir_setting() -> String {
    std::env::var("WD_GPU")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_GPU_DIR.to_string())
}

/// The graphics device to give an application that draws, and the group it
/// needs in order to open it.
pub struct Gpu {
    /// The one render node, passed as `--device <path>:<path>`.
    ///
    /// **One, never several, even on a host that has several.** The Selkies
    /// images detect their own node with, at
    /// `init-selkies-config/run:274`:
    ///
    /// ```sh
    /// if [[ "${PIXELFLUX_WAYLAND}" == "true" ]] && [ -e "/dev/dri/renderD128" ] \
    ///    && [ ! -e "/dev/dri/renderD129" ] && [ -z ${DRI_NODE+x} ]; then
    /// ```
    ///
    /// -- so a *second* node visible inside the container fails that guard,
    /// leaves `DRI_NODE` unset, and drops the app back to CPU encoding. Passing
    /// everything found would therefore make a two-GPU host slower than a
    /// one-GPU host, silently. See `node_key` for which one is chosen.
    pub node: String,
    /// Supplementary gids, passed as `--group-add`. Only the ones that are
    /// actually load-bearing: a node anybody may open contributes none.
    pub groups: Vec<u32>,
}

/// The host's render nodes, for the applications that draw.
///
/// **Render nodes only, never the card node.** `/dev/dri/cardN` is the
/// modesetting device: it drives the physical display, and a container holding
/// it can change what is on the monitor attached to the machine.
/// `/dev/dri/renderDN` is the other half -- the compute and video engines, with
/// no display attached -- and it is the half a headless application needs. Mesa
/// selects a hardware driver from a render node alone and the video encoder
/// takes its VA-API device from one, so passing the card node as well would buy
/// nothing and cost the display.
///
/// `WD_GPU=off` declines the whole thing; `WD_GPU=<dir>` names the directory to
/// look in, for a host that keeps its nodes somewhere else.
///
/// `None` on a host with no graphics device, which is an ordinary thing for a
/// server to be rather than a misconfiguration -- so unlike a missing
/// `WD_HOME_MOUNT` directory it is not warned about.
pub fn gpu() -> Option<Gpu> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let want = gpu_dir_setting();
    if want == "off" {
        return None;
    }
    let dir = std::path::Path::new(&want);
    if !dir.is_dir() {
        tracing::debug!("{want} is not a directory; no graphics device will be shared");
        return None;
    }

    let mut found: Vec<(u32, String, std::fs::Metadata)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(n) = node_key(name) else { continue };
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.file_type().is_char_device() {
            continue;
        }
        found.push((n, path.to_string_lossy().to_string(), meta));
    }
    // Lowest-numbered, so that a host with two cards makes the same choice on
    // every install rather than whichever `read_dir` happened to yield first.
    found.sort_by_key(|(n, _, _)| *n);
    let (_, node, meta) = found.into_iter().next()?;

    // The group is only worth adding when the mode says it is doing work. A
    // node anybody may open -- 0666, which is what a `uaccess` rule leaves
    // behind on a desktop -- needs none, and adding one would put a gid in
    // `docker inspect` that explains nothing.
    let groups = if meta.mode() & 0o006 == 0o006 { Vec::new() } else { vec![meta.gid()] };
    Some(Gpu { node, groups })
}

/// The number in `renderD128`, or `None` for anything that is not a render
/// node.
///
/// Parsed rather than matched as a prefix so that the nodes sort numerically:
/// a host with ten cards would otherwise put `renderD1280` before `renderD129`,
/// and the choice of which GPU an app gets should not turn on string order.
fn node_key(name: &str) -> Option<u32> {
    name.strip_prefix("renderD")?.parse().ok()
}

/// Is SELinux in the picture? On the RHEL side of the target list it usually
/// is, and a bind mount that has not been relabelled is simply unreadable to
/// the container -- which presents as an app that starts and then behaves as
/// though its data directory were empty.
///
/// Checked by the presence of the filesystem rather than by running
/// `getenforce`, which is not installed everywhere it applies.
///
/// **The kernel having SELinux is only half the question.** The other half is
/// whether the engine was built and configured to act on it, and the two
/// disagree more often than is comfortable: the deployment host runs AlmaLinux
/// 10 in enforcing mode, and its Docker reports `SecurityOptions` of exactly
/// `seccomp` and `cgroupns` -- no `selinux` -- so every container on it runs
/// with an empty process label. On such a host a `z` suffix relabels the host's
/// files to suit a confinement that is not being applied. That is the worst of
/// both: the cost is paid on the host and the benefit is not collected.
/// See `honours_labels`.
fn selinux() -> bool {
    std::path::Path::new("/sys/fs/selinux/enforce").exists()
}

/// Does this engine actually label containers, or merely accept the suffix?
///
/// Asked of the engine rather than assumed from the kernel, for the reason
/// `selinux` gives. Answered once per install and carried on the `RunSpec`, so
/// that building the command line stays a pure function of it.
///
/// A engine that cannot be asked is treated as not labelling. The failure that
/// avoids -- relabelling a host directory for confinement nobody applies -- is
/// permanent and touches files outside WebDesk; the failure it risks is an app
/// that cannot read its own state directory, which is visible immediately and
/// fixed by an operator who knows their host better than this guess does.
pub fn honours_labels(engine: Engine) -> bool {
    if !selinux() {
        return false;
    }
    match capture(engine, &["info", "--format", "{{json .SecurityOptions}}"]) {
        Ok(s) => s.contains("selinux"),
        Err(e) => {
            tracing::warn!("could not ask {} about SELinux ({e}); not relabelling", engine.bin());
            false
        }
    }
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
/// The host's fonts get neither, for the same reason and a sharper one. `z`
/// would relabel `/usr/share/fonts` on the host -- a system directory this
/// program does not own, shared read-only with every app that draws -- to suit
/// a container. The mount is read-only, which is not a defence: the relabelling
/// happens to the source on the host, not to the view inside.
///
/// The engine socket is left alone for the same reason and more sharply. It is
/// not WebDesk's file: the daemon and every other client on the host are using
/// it right now, and relabelling it to suit one container is a change to
/// something the machine depends on to run containers at all. An operator who
/// wants that made to work on an enforcing host should say so in policy, where
/// it is visible and reversible, rather than have an install quietly do it.
///
/// The rule underneath all three: **a mount WebDesk adds unasked never
/// relabels the host.** What is left is the directories WebDesk made itself
/// and the ones a user named on a form, which is where a label change is
/// theirs to expect.
fn relabel_for(relabel: bool, container_path: &str) -> Option<char> {
    if !relabel
        || container_path == home_dir_setting()
        || container_path == FONTS_AT
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
    /// Host device nodes to pass through, at the same path inside as out.
    ///
    /// Only ever the render nodes `gpu()` found, and only for an entry whose
    /// catalog record says it draws. Empty is the ordinary answer -- for every
    /// app that does not draw, and on every host with no graphics device.
    pub devices: Vec<String>,
    /// Supplementary gids the container needs to open those devices.
    ///
    /// Numeric, and computed from the mode and ownership of the nodes
    /// themselves rather than from a group name: `render` and `video` are the
    /// usual names but not universal ones, and the gid behind a name differs
    /// per host anyway.
    pub groups: Vec<u32>,
    /// Whether a `z`/`Z` suffix on a mount means anything on this host.
    ///
    /// Decided once by `honours_labels`, which asks the engine rather than the
    /// kernel, and carried here so that building the command line stays a pure
    /// function of this struct.
    pub relabel: bool,
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

        // The one widening in this function, and it is a narrow one: a named
        // character device is added to the container's device allowlist, at the
        // same path inside as out. It is not `--privileged`, which would add
        // every device on the machine; it is not `--device-cgroup-rule`, which
        // would name a whole major number; and the paths come from `gpu()`
        // reading the host's own directory, never from a request.
        for dev in &self.devices {
            a.push("--device".into());
            a.push(format!("{dev}:{dev}"));
        }
        // A device the container may reach but may not open is the same as no
        // device, so these travel together. A supplementary gid grants exactly
        // what that group already grants on the host and nothing else -- it
        // adds no capability, and it cannot reach a file the group does not
        // already own.
        for gid in &self.groups {
            a.push("--group-add".into());
            a.push(gid.to_string());
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
            if let Some(z) = relabel_for(self.relabel, at) {
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
            relabel: true,
            devices: vec!["/dev/dri/renderD128".into()],
            groups: vec![105],
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
        // --shm-size is a tmpfs size and --device names one character device.
        // These are the things that are neither -- a capability, a whole
        // namespace, or the seccomp profile -- and this program has no code
        // path that emits any of them.
        for forbidden in [
            "--security-opt",
            "--privileged",
            "--cap-add",
            "--network=host",
            "--pid=host",
            "--ipc=host",
            "--device-cgroup-rule",
        ] {
            assert!(!args.contains(forbidden), "{forbidden} appeared in {args}");
        }
    }

    #[test]
    fn a_device_is_passed_at_the_same_path_inside_as_out() {
        let args = spec().args();
        let at = args.iter().position(|a| a == "--device").expect("no --device");
        // Same path both sides: an application looks for /dev/dri/renderD128,
        // and a node that arrived under another name is one it will not find.
        assert_eq!(args[at + 1], "/dev/dri/renderD128:/dev/dri/renderD128");
    }

    #[test]
    fn the_group_that_opens_the_device_travels_with_it() {
        let args = spec().args();
        let at = args.iter().position(|a| a == "--group-add").expect("no --group-add");
        // Numeric, and only the gids `gpu()` found load-bearing. A device the
        // container may reach but may not open is the same as no device.
        assert_eq!(args[at + 1], "105");
    }

    #[test]
    fn an_app_that_does_not_draw_is_given_no_device_and_no_group() {
        let mut s = spec();
        s.devices.clear();
        s.groups.clear();
        let args = s.args().join(" ");
        // The ordinary case, and the one every non-drawing entry takes: the
        // flags are absent rather than empty.
        assert!(!args.contains("--device"), "{args}");
        assert!(!args.contains("--group-add"), "{args}");
    }

    #[test]
    fn the_card_node_is_never_offered() {
        // `gpu()` reads the host, so this asserts about the policy rather than
        // about a fixture: whatever this machine has, only render nodes come
        // back. The card node drives the physical display.
        if let Some(g) = gpu() {
            assert!(g.node.contains("renderD"), "{} is not a render node", g.node);
            assert!(!g.node.contains("/card"), "{} is the modesetting node", g.node);
        }
    }

    #[test]
    fn render_nodes_are_ordered_by_number_and_not_by_name() {
        // The choice of which GPU an app gets must not turn on string order:
        // `renderD1280` sorts before `renderD129` as text and after it as a
        // number, and the lowest-numbered node is the one that gets passed.
        assert_eq!(node_key("renderD128"), Some(128));
        assert_eq!(node_key("renderD129"), Some(129));
        assert_eq!(node_key("renderD1280"), Some(1280));
        assert!(node_key("card0").is_none());
        assert!(node_key("renderD").is_none());
        assert!(node_key("by-path").is_none());
        let mut nodes = ["renderD1280", "renderD129", "renderD128"];
        nodes.sort_by_key(|n| node_key(n).unwrap());
        assert_eq!(nodes[0], "renderD128");
    }

    #[test]
    fn exactly_one_render_node_is_ever_offered() {
        // Not a stylistic preference. The Selkies images detect their node with
        // `[ -e renderD128 ] && [ ! -e renderD129 ]`, so a second node visible
        // inside the container fails that guard and drops the app back to CPU
        // encoding -- making a two-GPU host slower than a one-GPU host. The
        // type says one; this says the type is the point.
        if let Some(g) = gpu() {
            assert!(!g.node.is_empty());
            assert!(g.groups.len() <= 1, "one node cannot need two groups");
        }
    }

    #[test]
    fn host_fonts_are_added_to_the_images_rather_than_put_in_front_of_them() {
        // Skipped on a host with no font directory, which is what the option
        // returning `None` there means.
        if let Some((host, at, ro)) = font_mount() {
            assert!(ro, "an application has no business writing the host's fonts");
            // The one mount that deliberately does not appear at the path it
            // has outside. Landing on /usr/share/fonts replaces the image's own
            // set, and the metric-compatible faces a document needs are in the
            // image, not on a typical host.
            assert_eq!(at, "/usr/local/share/fonts");
            assert_ne!(at, host, "the host path is the one path this must not use");
        }
    }

    #[test]
    fn declining_the_fonts_is_honoured() {
        let before = std::env::var("WD_FONT_MOUNT").ok();
        unsafe { std::env::set_var("WD_FONT_MOUNT", "off") };
        let off = font_mount().is_none();
        match before {
            Some(v) => unsafe { std::env::set_var("WD_FONT_MOUNT", v) },
            None => unsafe { std::env::remove_var("WD_FONT_MOUNT") },
        }
        assert!(off, "WD_FONT_MOUNT=off still produced a mount");
    }

    #[test]
    fn declining_the_gpu_is_honoured() {
        // Safe to set: this is the only test that reads it, and it is restored
        // before the assertion that would notice.
        let before = std::env::var("WD_GPU").ok();
        unsafe { std::env::set_var("WD_GPU", "off") };
        let off = gpu().is_none();
        match before {
            Some(v) => unsafe { std::env::set_var("WD_GPU", v) },
            None => unsafe { std::env::remove_var("WD_GPU") },
        }
        assert!(off, "WD_GPU=off still produced a device");
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
        // A directory WebDesk made gets the private label; one the user named
        // on a form gets the shared one, because they may still want to reach
        // it from outside.
        assert_eq!(relabel_for(true, "/config"), Some('Z'));
        assert_eq!(relabel_for(true, "/media"), Some('z'));
    }

    #[test]
    fn an_engine_that_does_not_label_is_sent_no_suffix() {
        // The deployment host is exactly this shape: AlmaLinux 10 enforcing,
        // with a Docker whose SecurityOptions are seccomp and cgroupns and no
        // selinux. Relabelling there pays the cost on the host and collects no
        // benefit, so nothing is emitted at all.
        assert_eq!(relabel_for(false, "/config"), None);
        assert_eq!(relabel_for(false, "/media"), None);
    }

    #[test]
    fn a_mount_webdesk_adds_unasked_never_relabels_the_host() {
        // The rule the next such mount has to obey too. Each of these is a
        // directory or socket the host already had and still uses: relabelling
        // /home stops sshd reading ~/.ssh, relabelling the font directory
        // rewrites a system path shared with every drawing app, and the engine
        // socket is what the machine runs containers with.
        //
        // Asserted with relabelling *on*, which is the only setting where the
        // question can fail.
        assert_eq!(relabel_for(true, "/home"), None);
        assert_eq!(relabel_for(true, FONTS_AT), None);
        assert_eq!(relabel_for(true, "/var/run/docker.sock"), None);
    }

    #[test]
    fn read_only_is_no_defence_against_relabelling() {
        // Worth an assertion because the instinct is that `ro` makes it safe.
        // It does not: the relabelling happens to the source on the host, not
        // to the view inside, so the font mount has to be excluded by path.
        let (_, at, ro) = ("/usr/share/fonts", FONTS_AT, true);
        assert!(ro);
        assert_eq!(relabel_for(true, at), None);
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
