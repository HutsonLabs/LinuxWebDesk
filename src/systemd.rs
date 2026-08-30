//! The host's service manager, for the applications WebDesk does not run itself.
//!
//! `engine.rs` is the twin of this file: both wrap a program that starts and
//! stops something, and both are asked the same three questions -- is it there,
//! what state is it in, make it the other state. The difference is what is on
//! the other end. The engine runs an image WebDesk chose and created, so
//! WebDesk owns its whole life. A unit here lives in `/etc/systemd/system` and
//! would go on running if WebDesk were uninstalled -- it may have been written
//! by the operator, and if it was, this module only ever starts and stops it.
//!
//! There is a second manager below the halfway line of this file. A streamed
//! application runs in the *user's* systemd manager rather than the machine's,
//! and that half is the same three questions asked one bus over -- see the
//! banner above `APP_UNIT`. The rule is not relaxed for it: the template is
//! still a constant, and all a request contributes is a slug that has to be in
//! the catalog before anything is spawned.
//!
//! **Where the boundary is, and where it is not.** A host service is a process
//! running as a real user on the real machine -- which is the entire point of
//! one, and also why it must not be *describable* from the browser. A unit file
//! assembled out of a request would be a way to run arbitrary code as root,
//! which is a strictly larger hole than the engine socket.
//!
//! This file does now write a unit, which it did not before, and the line it
//! holds is the one that was always doing the work: **the unit is a constant.**
//! Its name and its entire body are `&'static str` in `catalog.rs`, so the set
//! of units that can exist is a property of the build, exactly as the set of
//! images is. `write_unit` interpolates two values and no others -- the user
//! and uid the service runs as -- and takes them from the caller's
//! authenticated session rather than from the request body, so the most a
//! request can decide is *whether* a unit the build already contains is
//! written, and never what is in it. A unit already on the machine is adopted
//! untouched, so an operator who wrote their own keeps it.
//!
//! Everything here degrades to a report rather than an error. A host without
//! systemd at all is a host where these entries simply cannot be installed, and
//! saying so is more use than a failure that reads like a bug.

use std::process::Command;

/// Whether this host has systemd to talk to.
///
/// Checked rather than assumed: every target distribution has it, but the
/// development machines this is built on do not, and a missing binary should
/// read as "not that kind of host" rather than as a crash.
pub fn available() -> bool {
    crate::engine::which("systemctl").is_some()
}

/// One `systemctl show` property, or `None` when systemd would not answer.
///
/// `show` rather than `is-active`/`status`: it exits 0 even for a unit that
/// does not exist, so the answer to "is this unit here" arrives as a value to
/// read instead of an exit code to interpret. The two states this file cares
/// about are then plain string comparisons.
fn property(unit: &str, name: &str) -> Option<String> {
    let out = Command::new("systemctl")
        .args(["show", unit, "--property", name, "--value"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Whether systemd has a unit by this name loaded at all.
///
/// The check an install makes before recording anything: an entry whose service
/// is not on the machine yet would otherwise install cleanly and then answer
/// 502 from the dock, with nothing anywhere saying why.
pub fn known(unit: &str) -> bool {
    matches!(property(unit, "LoadState").as_deref(), Some("loaded"))
}

/// The unit's state, in the same words `engine::state` uses for a container, so
/// that one dock can paint both without knowing which it is looking at.
///
/// `absent` is the one word this adds, and it is the one the container
/// vocabulary has no equivalent for: a container WebDesk created and then lost
/// is `missing`, which is a fault, while a unit that was never installed is an
/// ordinary thing to find and is what the entry's `provision` text is for.
///
/// Never an error. A service stopped behind our back is a state to report.
pub fn state(unit: &str) -> String {
    word_for(property(unit, "LoadState").as_deref(), || property(unit, "ActiveState"))
}

/// The two `systemctl show` answers turned into one of the dock's words.
///
/// Split out of `state` when `user_state` appeared, because the two ask the
/// same question of different managers and the vocabulary has to be the same
/// word for word. Kept apart they would drift by one state at a time, and the
/// symptom of that is a raw systemd token painted in the Apps window.
///
/// The active state is a closure rather than a value: on a `LoadState` that
/// already settles the answer there is no reason to spend a second `systemctl`
/// on it, and a user manager reached over the bus is the expensive one.
fn word_for(load: Option<&str>, active: impl FnOnce() -> Option<String>) -> String {
    match load {
        Some("loaded") => {}
        // `not-found`, `masked`, `bad-setting`, or no answer at all.
        _ => return "absent".into(),
    }
    match active().as_deref() {
        Some("active") => "running".into(),
        Some("activating" | "deactivating" | "reloading") => "restarting".into(),
        Some("failed") => "failed".into(),
        Some("inactive") => "exited".into(),
        _ => "unknown".into(),
    }
}

/// Run one `systemctl` verb against a unit, reporting what it said on failure.
///
/// stderr rather than the exit code, because the exit code of a refused start
/// is the same as the exit code of a unit that failed to come up, and only one
/// of those is worth showing somebody.
fn act(verb: &str, unit: &str) -> Result<(), String> {
    let out = Command::new("systemctl")
        .args([verb, unit])
        .output()
        .map_err(|e| format!("could not run systemctl: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.is_empty() { format!("systemctl {verb} {unit} failed") } else { err })
}

pub fn start(unit: &str) -> Result<(), String> {
    act("start", unit)
}

/// Where a system unit lives. `/etc` rather than `/usr/lib`: this is local
/// configuration, and an operator editing it afterwards should find it in the
/// directory that belongs to them.
fn unit_path(unit: &str) -> std::path::PathBuf {
    std::path::Path::new("/etc/systemd/system").join(unit)
}

/// Write a unit from the catalog, substituting the identity it runs as.
///
/// `unit` and `body` are both `&'static str` from `catalog.rs` -- see the
/// module docs for why that is the whole of the security argument here. `user`
/// and `uid` come from the session of whoever pressed Install.
///
/// Refuses rather than overwrites. A unit already on this host was put there by
/// somebody, may not say what this one says, and is very likely serving the app
/// right now; replacing it silently would be the one way this could take a
/// working host service away from its operator.
pub fn write_unit(
    unit: &'static str,
    body: &'static str,
    user: &str,
    uid: u32,
) -> Result<(), String> {
    let path = unit_path(unit);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let text = body.replace("{user}", user).replace("{uid}", &uid.to_string());
    std::fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    // Without this systemd goes on believing what it read at boot, and the
    // enable that follows fails on a unit that is sitting right there.
    reload()
}

/// Remove a unit WebDesk wrote. Best effort, and only ever called for the unit
/// named in the entry being removed.
pub fn remove_unit(unit: &'static str) {
    let _ = std::fs::remove_file(unit_path(unit));
    let _ = reload();
}

pub fn reload() -> Result<(), String> {
    let out = Command::new("systemctl")
        .arg("daemon-reload")
        .output()
        .map_err(|e| format!("could not run systemctl: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// Start it now and at every boot. One call because the two are never wanted
/// apart here: a terminal that is in the Apps window but gone after a reboot
/// is a bug report, not a feature.
pub fn enable_now(unit: &str) -> Result<(), String> {
    let out = Command::new("systemctl")
        .args(["enable", "--now", unit])
        .output()
        .map_err(|e| format!("could not run systemctl: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.is_empty() { format!("systemctl enable --now {unit} failed") } else { err })
}

pub fn stop(unit: &str) -> Result<(), String> {
    act("stop", unit)
}

// ---------------------------------------------------------------------------
// User units, for the applications that are drawn on this host.
//
// Everything above this line manages *system* units, which is right for a
// service whose subject is the machine. A streamed application is the opposite:
// its subject is your home directory, so one host-wide instance shared by
// everyone would be the wrong machine's files by exactly the argument that put
// term.hut on the host in the first place. These run in the signed-in user's
// own systemd manager instead.
// ---------------------------------------------------------------------------

/// The template unit every streamed application is started from.
///
/// **One unit for all of them, and the instance name is a slug.** That is the
/// whole of how this keeps the rule the catalog is built on. A unit whose
/// `ExecStart` interpolated a Flatpak id would be a way to run any Flatpak on
/// this host; instead `%i` is handed to `webdesk app-session`, which resolves it
/// against the catalog compiled into the binary and refuses anything that is not
/// there. A request still decides only *whether* something the build already
/// contains runs.
///
/// A user unit, so `User=` is absent and `HOME`, `XDG_RUNTIME_DIR` and the
/// session bus are whatever the manager already has -- which is the point. Note
/// how much shorter this is than `TERM_HUT_UNIT` for that one reason: the three
/// `Environment=` lines that unit needs are three a user manager has already set
/// correctly, and cannot set wrongly.
///
/// `{exe}` is the only substitution, and it is not a value anybody sends us --
/// see `exe_path`. There is deliberately no `{user}` and no `{uid}`: a template
/// that named an identity would be a template that could name the wrong one,
/// and in the manager this is installed into there is only ever the one.
pub const APP_UNIT: &str = r#"[Unit]
Description=%i, drawn on this host and streamed into a WebDesk window

# Nothing to order after. A system service waits for the network because
# something out there dials it; this waits for a person to click an icon, and by
# then everything it uses is long up.

[Service]
Type=simple

# No User=, and that absence is the entry rather than an omission. term.hut is a
# system unit running as one named person because its subject is the machine;
# this is the opposite kind of application. Started in your own manager it is
# your files it opens, which is the only version of it worth having.
#
# The argument is a slug and never an application id -- see the doc comment.
ExecStart={exe} app-session %i

# The failure TERM_HUT_UNIT documents, in a unit that is not allowed to say the
# words. `flatpak run` hands the application to the session helper, which puts
# it in a systemd *scope* of its own outside this service's cgroup, so stopping
# the service kills the compositor and the launcher and leaves the application
# running -- holding this user's files, with nothing left anywhere to draw it.
# `flatpak kill` by id is the one handle that reaches into that scope.
#
# The id may not appear here, so the kill is spelled the only way this unit is
# allowed to spell anything: the same binary, the same slug, and the id looked
# up in the catalog it was built with. The leading `-` for the reason term.hut
# has one -- nothing to kill is the ordinary case and is not a failed stop.
# There is no matching ExecStartPre because ExecStart is ours and clears the
# ground itself; term.hut needed one only because its ExecStart was `flatpak
# run` directly.
ExecStop=-{exe} app-session %i --kill

# Said out loud because the sibling unit says on-failure and a reader arriving
# from it will assume this one does too. Quitting the application is how you
# close it. `on-failure` would not fire on that, but an application that dies on
# startup would loop for as long as the window is open, and the dock would paint
# `restarting` forever instead of `failed` once -- and only one of those is a
# fact somebody can act on.
Restart=no

# The stop above is one `flatpak kill` and a socket to unlink. If that has not
# finished in fifteen seconds it is wedged, and the default ninety would leave
# the dock showing an app that is closing for a minute and a half.
TimeoutStopSec=15

# No [Install] section, deliberately. This is started when somebody opens the
# app and by nothing else; enabled, every app anyone had ever opened would bring
# up a compositor at login for a window nobody asked for.
"#;

/// The file the template is written as. `webdesk-app@<slug>.service` is one
/// instance of it, which is what the trailing `@` means.
const APP_TEMPLATE: &str = "webdesk-app@.service";

/// The binary the template's `ExecStart` names.
///
/// `current_exe` rather than the `/usr/local/bin/webdesk` that `install.sh`
/// defaults to, because the binary that is running is by definition the one
/// that has to run again, and this way the two need no agreement at all.
/// `install.sh` takes `PREFIX` from the operator and records it in
/// `/etc/webdesk/install.conf`; reading that back would be a second copy of the
/// same fact, and wrong on any host where the binary was put somewhere by hand.
///
/// The fallback is not for an unusual prefix -- it is for the minutes after an
/// in-place upgrade, when `/proc/self/exe` still resolves to a path that no
/// longer exists and comes back as `/usr/local/bin/webdesk (deleted)`. A unit
/// written with that in it fails complaining about a file that is plainly
/// there, which is a bad hour for whoever has to read the message.
fn exe_path() -> String {
    match std::env::current_exe() {
        Ok(p) if p.is_file() => p.to_string_lossy().into_owned(),
        _ => "/usr/local/bin/webdesk".into(),
    }
}

/// `--machine=<user>@.host`: this host's copy of that person's systemd manager.
fn machine(user: &str) -> String {
    format!("--machine={user}@.host")
}

/// Put the template in the user's own unit directory, and make the runtime
/// directory its sessions will need.
///
/// Called before every open rather than once at install, which is why nothing
/// here may churn: the socket directory is under `/run` and is gone after a
/// reboot, so something has to make it again, and this is the one privileged
/// per-user step on the way to starting an app. An identical template is left
/// entirely alone -- no write, no `daemon-reload` -- because rewriting the file
/// under a running instance is how a manager comes to say `changed on disk`.
///
/// **Ownership is the part that fails silently.** WebDesk is root, so a file it
/// creates in somebody's home is root's, and the manager that has to read it is
/// theirs. A user manager that cannot read its own unit says so to a journal
/// nobody is reading, and the Apps window shows an app that will not start with
/// no reason attached. Everything created here is handed over.
///
/// Lingering is enabled here and nowhere else on this path, and it has to be
/// first: without it there is no `/run/user/<uid>`, no user bus and so no
/// manager for `--machine=<user>@.host` to reach, and the `daemon-reload` at
/// the end of this function is the first thing that would fail. It does not
/// belong in `user_act`, which the dock calls for every poll of every tile,
/// where it would be a `loginctl` per poll for a fact that changes once.
pub fn install_app_template(uid: u32, user: &str) -> Result<(), String> {
    // First, and see above. Best effort in `flatpak.rs`, which is right here
    // too: a host where this fails may still have a live session for this user,
    // and the reload below returns the real answer either way.
    crate::flatpak::enable_linger(user);

    let Some(account) = users::get_user_by_name(user) else {
        return Err(format!("{user} is not a user on this host"));
    };
    let gid = account.primary_group_id();

    // Where the session will serve. `/run/webdesk/rfb` stays root's, because the
    // parent of a per-user directory must not be writable by the users it is
    // separating; `<uid>` under it is theirs and 0700, which is the whole of why
    // two people with the same app open cannot open each other's screen.
    let per_user = crate::rfb::socket_dir(uid);
    if let Some(parent) = per_user.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    make_dir(&per_user, 0o700, uid, gid)?;

    // One level at a time rather than `create_dir_all`, so each directory this
    // brings into existence can be given away as it is made. An account that has
    // never logged in graphically has no `~/.config` -- and that is exactly the
    // account a web desktop is for -- so it would otherwise get a root-owned one
    // and lose the ability to write anything else into it ever again.
    let home = {
        use users::os::unix::UserExt;
        account.home_dir().to_path_buf()
    };
    let mut dir = home.clone();
    for part in [".config", "systemd", "user"] {
        dir.push(part);
        make_dir(&dir, 0o755, uid, gid)?;
    }

    let path = dir.join(APP_TEMPLATE);
    let text = APP_UNIT.replace("{exe}", &exe_path());
    if std::fs::read_to_string(&path).map(|old| old == text).unwrap_or(false) {
        return Ok(());
    }
    std::fs::write(&path, &text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let _ = std::fs::set_permissions(
        &path,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o644),
    );
    own(&path, uid, gid)?;

    // Without this the manager goes on believing what it read at login, and the
    // start that follows fails on a unit that is sitting right there -- what
    // `write_unit` says about the system manager, one bus over.
    user_act("daemon-reload", user, "")
}

/// Create a directory if it is missing, with the mode and the owner it needs.
///
/// Already there is not an error and is the ordinary case. Already there and
/// owned by somebody else is left exactly as it is: a directory in a user's
/// home that is not theirs is a fact about that host, and not ours to correct
/// on the way to opening an image editor.
fn make_dir(path: &std::path::Path, mode: u32, uid: u32, gid: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if path.is_dir() {
        return Ok(());
    }
    std::fs::create_dir(path).map_err(|e| format!("could not create {}: {e}", path.display()))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    own(path, uid, gid)
}

fn own(path: &std::path::Path, uid: u32, gid: u32) -> Result<(), String> {
    nix::unistd::chown(
        path,
        Some(nix::unistd::Uid::from_raw(uid)),
        Some(nix::unistd::Gid::from_raw(gid)),
    )
    .map_err(|e| format!("could not give {} to its user: {e}", path.display()))
}

/// `systemctl --user --machine=<user>@.host <verb> webdesk-app@<slug>.service`.
///
/// Root reaching a user's manager over the bus, which is the standard way to do
/// this and avoids both a setuid helper and a second privileged path: the
/// manager on the other end is the one that already exists for that person, so
/// everything it starts is theirs without anybody dropping privilege by hand.
///
/// The same shape as `act`, including the part that matters -- the message comes
/// off stderr, because "no such unit", "no bus to talk to" and "it came up and
/// died" all exit non-zero and only the words tell them apart. The second is the
/// common one on a host where this user has never logged in, and it says so.
///
/// `unit` may be empty, for the verbs that address the manager rather than one
/// of its units. `daemon-reload` is the only one of those used here.
pub fn user_act(verb: &str, user: &str, unit: &str) -> Result<(), String> {
    let mut args = vec!["--user".to_string(), machine(user), verb.to_string()];
    if !unit.is_empty() {
        args.push(unit.to_string());
    }
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .map_err(|e| format!("could not run systemctl: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if err.is_empty() {
        format!("systemctl --user {verb} {unit} failed for {user}")
    } else {
        err
    })
}

/// One `systemctl show` property out of a user's manager, or `None`.
///
/// The twin of `property`, absent for one more reason than that one has: a user
/// with no session and no lingering has no manager at all, and that has to
/// answer nothing rather than answer wrongly.
fn user_property(user: &str, unit: &str, name: &str) -> Option<String> {
    let out = Command::new("systemctl")
        .args(["--user", &machine(user), "show", unit, "--property", name, "--value"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// The state of a user unit, in the words the dock already paints.
///
/// Never an error, for the reason `state` is not, and `absent` covers one more
/// case here: a user whose manager is not running has no unit to report, which
/// is exactly what an app nobody has opened is.
pub fn user_state(user: &str, unit: &str) -> String {
    word_for(user_property(user, unit, "LoadState").as_deref(), || {
        user_property(user, unit, "ActiveState")
    })
}

/// The unit name for a slug. The only place the two are joined.
pub fn app_unit(slug: &str) -> String {
    format!("webdesk-app@{slug}.service")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name systemd cannot have a unit for reads as `absent`, on a host with
    /// systemd and on one without. The second half is what makes this test
    /// runnable on the machines this is developed on.
    #[test]
    fn a_unit_that_is_not_there_is_absent_rather_than_an_error() {
        assert_eq!(state("webdesk-definitely-not-a-real-unit.service"), "absent");
        assert!(!known("webdesk-definitely-not-a-real-unit.service"));
    }

    /// The words this file returns are the words the dock already knows. A
    /// state invented here would paint as a raw systemd token in the Apps
    /// window, which is how `deactivating` would reach a user.
    /// The substitutions are the only ones, and they reach every place the unit
    /// names the identity. A template that interpolated nothing would run the
    /// terminal as root; one that missed the uid would point the service at a
    /// runtime directory that is not the user's.
    #[test]
    fn a_written_unit_names_the_user_and_nothing_else_is_substituted() {
        let body = crate::catalog::TERM_HUT_UNIT;
        let text = body.replace("{user}", "someone").replace("{uid}", "1234");
        assert!(!text.contains('{'), "a placeholder survived: {text}");
        assert!(text.contains("User=someone"));
        assert!(text.contains("XDG_RUNTIME_DIR=/run/user/1234"));
        assert!(text.contains("unix:path=/run/user/1234/bus"));
        // The two flags that decide how many doors this terminal has. Loopback
        // so WebDesk's sign-in is the only way in, and no token of its own
        // because reaching it already means getting past that sign-in -- the
        // same argument the container entry used to make with HUT_NO_TOKEN.
        assert!(text.contains("--host 127.0.0.1"));
        assert!(text.contains("--no-token"));
    }

    /// Every host entry that writes a unit must have a body to write, and it
    /// must be a unit file rather than whatever else a `&'static str` could be.
    #[test]
    fn every_host_entry_carries_a_unit_it_could_write() {
        for app in crate::catalog::CATALOG.iter().filter(|a| a.host.is_some()) {
            let host = app.host.as_ref().unwrap();
            assert!(
                host.unit_body.contains("[Service]") && host.unit_body.contains("ExecStart="),
                "{} has no unit body to write",
                app.slug
            );
            // A unit that never starts at boot would vanish from the Apps
            // window after a reboot with nothing saying why.
            assert!(host.unit_body.contains("[Install]"), "{} would not survive a reboot", app.slug);
        }
    }

    #[test]
    fn every_state_is_one_the_dock_has_a_name_for() {
        const KNOWN: &[&str] =
            &["running", "exited", "restarting", "failed", "absent", "unknown"];
        assert!(KNOWN.contains(&state("webdesk-not-a-unit.service").as_str()));
        // The user manager answers in the same six words or in none of them.
        // `root` because it is the one account every host has; on a host where
        // its manager is not running this is `absent`, which is the point.
        assert!(KNOWN.contains(&user_state("root", "webdesk-not-a-unit.service").as_str()));
    }

    /// A user unit nobody has installed reads as `absent`, on a host with
    /// systemd and on one without, and on a host where root may not reach that
    /// person's bus. Three ways to get no answer and one word for all of them:
    /// anything else here paints a raw systemd token, or a failure, in a tile
    /// for an app whose ordinary condition is "not open".
    #[test]
    fn a_user_unit_that_is_not_there_is_absent_rather_than_an_error() {
        assert_eq!(user_state("root", &app_unit("not-a-real-slug")), "absent");
        // Not a user at all, which is the same answer by a different road.
        assert_eq!(user_state("webdesk-nobody", &app_unit("not-a-real-slug")), "absent");
    }

    /// The instance name is the slug and nothing has to be escaped to get it
    /// back. `%i` in the unit is what systemd read out of the file name, so a
    /// slug that needed escaping would arrive at `app-session` as `\x2d`-ridden
    /// nonsense and resolve against nothing. Every catalog slug is a URL key
    /// already, which is why this holds -- and this test is what says so.
    #[test]
    fn a_slug_survives_the_trip_through_a_unit_name() {
        for app in crate::catalog::CATALOG {
            let unit = app_unit(app.slug);
            let back = unit
                .strip_prefix("webdesk-app@")
                .and_then(|s| s.strip_suffix(".service"))
                .unwrap_or("");
            assert_eq!(back, app.slug, "{} does not survive its own unit name", app.slug);
            assert!(
                app.slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{} would be escaped by systemd on the way into %i",
                app.slug
            );
        }
        // And one instance of the template is the template with the slug in it,
        // so a rename of either has to be a rename of both.
        assert_eq!(app_unit("x"), APP_TEMPLATE.replace('@', "@x"));
    }

    /// **The security property, as a test.** The template may say *whether*
    /// something the build contains runs, never *what* it is. Concretely: it
    /// hands `%i` -- a slug -- to a binary that owns the catalog, it names no
    /// application, and it substitutes nothing but the path of that binary.
    ///
    /// This is the test that survives a future edit. A well-meaning change that
    /// put the Flatpak id in `ExecStart` to save a lookup, or a `{slug}` where
    /// `%i` was, would turn one template into a way to run anything on this host
    /// as whoever is signed in, and nothing else in the tree would notice.
    /// The template with its commentary taken out.
    ///
    /// The unit carries as much explanation as the code around it does, and a
    /// comment saying what systemd must never be told is not systemd being told
    /// it. Tests about what the unit *does* read this; tests about what it may
    /// not contain at all read `APP_UNIT` itself.
    fn directives() -> String {
        APP_UNIT
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_template_can_name_the_slug_but_never_the_application() {
        assert!(APP_UNIT.contains("app-session %i"), "the unit does not pass the instance name");

        // Not one application id, from any entry, anywhere in it -- and not the
        // word `flatpak` either. This unit's whole knowledge of Flatpak is that
        // the binary it starts has some.
        for app in crate::catalog::CATALOG {
            if let Some(s) = &app.streamed {
                assert!(!APP_UNIT.contains(s.flatpak.id), "the unit names {}", s.flatpak.id);
            }
            if let Some(fp) = app.host.as_ref().and_then(|h| h.flatpak.as_ref()) {
                assert!(!APP_UNIT.contains(fp.id), "the unit names {}", fp.id);
            }
        }
        assert!(!directives().contains("flatpak"), "the unit runs flatpak itself");

        // A user unit. `User=` would run somebody else's application against
        // somebody else's home directory, which is the one thing this kind of
        // entry exists not to do.
        assert!(!directives().contains("User="), "the unit names an identity");

        // `{exe}` is the only interpolation, and after it there is nothing left
        // for a later `.replace` to reach. A `{user}` or `{slug}` added here
        // would be substituted by nobody and reach systemd as a literal brace.
        let text = APP_UNIT.replace("{exe}", "/usr/local/bin/webdesk");
        assert!(!text.contains('{'), "a placeholder survived: {text}");
        assert!(text.contains("ExecStart=/usr/local/bin/webdesk app-session %i"));
    }

    /// The unit stops what it starts. `flatpak run` puts the application in a
    /// scope of its own, so a template with no `ExecStop` would leave it running
    /// after the unit went inactive -- the failure `TERM_HUT_UNIT` documents,
    /// arriving in a file that cannot name the application to kill it.
    #[test]
    fn stopping_the_unit_reaches_the_application_it_started() {
        assert!(APP_UNIT.contains("ExecStop=-{exe} app-session %i --kill"));
        // A restarting compositor is a window that flickers back after somebody
        // closed it. Quitting is how you close a streamed app.
        assert!(directives().contains("Restart=no"));
        // Enabled, this would start at login for everyone who ever opened it.
        assert!(!directives().contains("[Install]"));
    }

    /// The path in `ExecStart` is absolute and is a file. systemd refuses a
    /// relative `ExecStart` outright, and `/proc/self/exe` reads as
    /// `... (deleted)` for a binary that has been replaced in place -- which is
    /// every host, a few seconds after an update.
    #[test]
    fn the_unit_names_a_binary_that_is_actually_there() {
        let exe = exe_path();
        assert!(exe.starts_with('/'), "{exe} is not an absolute path");
        assert!(!exe.contains("(deleted)"), "{exe} is a binary that has been replaced");
    }
}
