//! What the host needs before an app will open, and installing it on one press.
//!
//! CONTRACT ONLY -- the bodies belong to the dependencies workstream.
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

use crate::catalog::Prereq;
use crate::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
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

pub struct Dep {
    pub key: &'static str,
    pub label: &'static str,
    /// One sentence saying what stops working without it, shown next to the
    /// button that installs it.
    pub why: &'static str,
    pub need: Need,
    pub prereq: Prereq,
}

/// Everything WebDesk can check for and offer to install.
pub static RUNTIME: &[Dep] = &[];

/// The dependencies for one part of the catalog that this host has not got.
///
/// The typed half of `report`, for the installer: an install that is about to
/// fail for want of a compositor should refuse and say so, in the same sentence
/// as what would fix it, rather than start and die.
pub fn absent_for(_need: Need) -> Vec<&'static Dep> {
    unimplemented!("dependencies workstream")
}

/// What is present, what is missing, and what package would provide it here.
pub fn report() -> serde_json::Value {
    unimplemented!("dependencies workstream")
}

/// Install the named dependencies. Keys are matched against `RUNTIME`; anything
/// not in it is dropped, so a request can never name a package.
pub fn install(_keys: &[String], _log: &Path) -> Result<(), String> {
    unimplemented!("dependencies workstream")
}

/// `GET /api/deps` -- what is here, what is not, and what would fix it.
pub async fn deps_report(State(_s): State<AppState>, _h: HeaderMap) -> Response {
    unimplemented!("dependencies workstream")
}

/// `POST /api/deps/install` -- `{"keys":["docker","cage",…]}`, the single click.
///
/// Admin-gated like every other install, and streamed into the same log the
/// Apps window already polls, so the button that starts it needs no new UI to
/// report what it is doing.
pub async fn deps_install(
    State(_s): State<AppState>,
    _h: HeaderMap,
    _body: axum::body::Bytes,
) -> Response {
    unimplemented!("dependencies workstream")
}
