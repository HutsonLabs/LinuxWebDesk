//! `cockpit-bridge`, used as a library rather than as a web console.
//!
//! CONTRACT ONLY -- the bodies belong to the Cockpit workstream.
//!
//! **Only the bridge, never `cockpit-ws`.** `cockpit-bridge` is a separate
//! package from Cockpit's web server and its pages: one binary that speaks a
//! newline-framed JSON channel protocol over stdio, with no port, no HTTP, no
//! login page and no UI. Installing it surfaces none of Cockpit, which is why
//! this is the shape chosen rather than an iframe with a second sign-in behind
//! it.
//!
//! **It runs as the signed-in user.** Spawned the way `pty.rs` spawns a shell,
//! so the kernel enforces what this session may read and write -- the same
//! property that makes Cockpit's own bridge safe, and the reason none of this
//! needs a privileged daemon of its own.
//!
//! **The protocol is terminated here and never tunnelled to the browser.**
//! Cockpit's own client opens channels from JavaScript because Cockpit's UI
//! *is* JavaScript. Handing that to a browser would hand it a `stream` channel,
//! which is a shell by another name, and it would break the rule the rest of
//! this program is built on: a request may choose *which* of the operations the
//! build contains runs, never *what* the operation is. So every endpoint here is
//! a named, fixed channel with a validated parameter, and the bridge stays an
//! implementation detail.

use crate::auth::Identity;
use crate::AppState;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::Value;
use std::collections::HashMap;

/// A live `cockpit-bridge` for one session.
pub struct Bridge;

impl Bridge {
    /// Spawn the bridge as this identity. `Err` when the package is absent --
    /// which is not fatal to WebDesk, only to the panels that need it.
    pub fn open(_ident: &Identity) -> Result<Bridge, String> {
        unimplemented!("cockpit workstream")
    }
}

/// Whether this host has a bridge to open at all.
pub fn available() -> bool {
    crate::engine::which("cockpit-bridge").is_some()
}

/// `GET /api/host/services` -- the units this user may see, via a `dbus`
/// channel against `org.freedesktop.systemd1`.
pub fn services(_b: &mut Bridge) -> Result<Value, String> {
    unimplemented!("cockpit workstream")
}

/// `GET /api/host/journal` -- log lines for one unit.
pub fn journal(_b: &mut Bridge, _unit: &str, _lines: usize) -> Result<Value, String> {
    unimplemented!("cockpit workstream")
}

/// `GET /api/host/metrics` -- one sample of CPU, memory and disk.
pub fn metrics(_b: &mut Bridge) -> Result<Value, String> {
    unimplemented!("cockpit workstream")
}

// --- handlers -------------------------------------------------------------
//
// Narrow and named, one per thing the desk can show. None of them takes a
// channel type, a payload shape or a bus name from the request -- that is the
// difference between using the bridge and exposing it.

/// `GET /api/host/services`
pub async fn host_services(State(_s): State<AppState>, _h: HeaderMap) -> Response {
    unimplemented!("cockpit workstream")
}

/// `GET /api/host/journal?unit=&lines=`
pub async fn host_journal(
    State(_s): State<AppState>,
    Query(_q): Query<HashMap<String, String>>,
    _h: HeaderMap,
) -> Response {
    unimplemented!("cockpit workstream")
}

/// `GET /api/host/metrics`
pub async fn host_metrics(State(_s): State<AppState>, _h: HeaderMap) -> Response {
    unimplemented!("cockpit workstream")
}

/// `POST /api/host/services/action` -- `{"unit":"…","verb":"start|stop|restart"}`.
///
/// The verb is matched against a fixed set and the unit against what
/// `host_services` already listed, so this can only act on something the desk
/// has shown you. It is not a way to name a unit.
pub async fn host_service_action(
    State(_s): State<AppState>,
    _h: HeaderMap,
    _body: axum::body::Bytes,
) -> Response {
    unimplemented!("cockpit workstream")
}
