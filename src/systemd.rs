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
    match property(unit, "LoadState").as_deref() {
        Some("loaded") => {}
        // `not-found`, `masked`, `bad-setting`, or no answer at all.
        _ => return "absent".into(),
    }
    match property(unit, "ActiveState").as_deref() {
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
    }
}
