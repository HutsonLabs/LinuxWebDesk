//! The host's service manager, for the applications WebDesk does not run itself.
//!
//! `engine.rs` is the twin of this file: both wrap a program that starts and
//! stops something, and both are asked the same three questions -- is it there,
//! what state is it in, make it the other state. The difference is what is on
//! the other end. The engine runs an image WebDesk chose and created, so
//! WebDesk owns its whole life. A unit here was written by the operator, lives
//! in `/etc/systemd/system`, and would go on running if WebDesk were
//! uninstalled. So this module can ask systemd to start or stop one, and can
//! never bring one into existence.
//!
//! **Why that is the boundary.** A host service is a process running as a real
//! user on the real machine -- which is the entire point of one, and also why
//! it must not be describable from the browser. If WebDesk could write a unit
//! file it would be a way to run arbitrary code as root, which is a strictly
//! larger hole than the engine socket, and the catalog exists precisely so that
//! the set of things that may be run is a property of the build. So the unit
//! name comes from a `&'static str` in `catalog.rs` and from nowhere else: no
//! request can name a unit, and installing a host app means adopting a service
//! the operator has already put on the machine, not creating one.
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
    #[test]
    fn every_state_is_one_the_dock_has_a_name_for() {
        const KNOWN: &[&str] =
            &["running", "exited", "restarting", "failed", "absent", "unknown"];
        assert!(KNOWN.contains(&state("webdesk-not-a-unit.service").as_str()));
    }
}
