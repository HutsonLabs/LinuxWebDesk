//! Running one streamed application, in the session of the person who opened it.
//!
//! CONTRACT ONLY -- the bodies belong to the host-session workstream.
//!
//! This is what `webdesk-app@<slug>.service` starts, and it runs as an ordinary
//! user with no privilege at all: a systemd *user* unit in their own manager,
//! reached the way `systemd::user_act` describes.
//!
//! **Why the binary and not a script.** The unit is one template for every
//! streamed app, and its `ExecStart` is handed `%i` -- a slug. A slug has to
//! become a Flatpak application id somewhere, and the only place that may happen
//! is inside a program that carries the catalog, because the whole rule this
//! project is built on is that the set of things which may be run is a property
//! of the build. A shell script taking an id from an argument would be exactly
//! the hole `catalog.rs` and `systemd.rs` are written to avoid, wearing a
//! systemd unit as a hat. So the unit runs `webdesk app-session <slug>`, this
//! resolves it against `CATALOG`, and anything not in there is refused before a
//! process is spawned.
//!
//! What it then does is start a headless `cage` holding exactly one application
//! and a `wayvnc` serving it on a socket only this user can open. Nothing here
//! listens on the network.

/// Run the session for `slug`, replacing this process. Never returns on success.
///
/// Called from `main` before the runtime starts, for the same reason `--helper`
/// is: this must not bind a port or touch shared state.
pub fn run(_slug: &str) -> ! {
    unimplemented!("host-session workstream")
}
