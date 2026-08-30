//! The pixels of a streamed app, carried to the browser.
//!
//! CONTRACT ONLY -- the bodies belong to the RFB workstream.
//!
//! A streamed entry (`catalog::Streamed`) runs under `cage` with `wayvnc`
//! serving RFB on a unix socket. This module is the one hop between that socket
//! and a WebSocket the browser's noVNC client speaks. It is deliberately *not*
//! `proxy.rs`: there is no HTTP here, no prefix to rewrite, no cookie to pin and
//! no upstream URL -- only a byte pump between two sockets, with the session
//! check in front of it.

use crate::AppState;
use axum::extract::{Path as AxPath, State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use std::path::PathBuf;

/// Where `wayvnc` for this user's copy of this app listens.
///
/// Per uid as well as per slug, because the session is per user: two people with
/// the same app open are two compositors and two sockets, and neither can open
/// the other's -- the directory is `0700` and owned by the user.
///
/// A unix socket rather than a loopback port on purpose. It keeps this off the
/// 47000-47999 pool entirely, and "unreachable from the network" becomes a
/// property of the filesystem rather than a `--bind 127.0.0.1` somebody has to
/// remember to write.
pub fn socket_path(uid: u32, slug: &str) -> PathBuf {
    PathBuf::from(format!("/run/webdesk/rfb/{uid}/{slug}.sock"))
}

/// The directory `socket_path` lives in, created `0700` for the session user
/// before the unit starts.
pub fn socket_dir(uid: u32) -> PathBuf {
    PathBuf::from(format!("/run/webdesk/rfb/{uid}"))
}

/// `GET /ws/rfb/{slug}` -- a WebSocket carrying raw RFB in binary frames.
pub async fn ws_rfb(
    _ws: WebSocketUpgrade,
    State(_state): State<AppState>,
    AxPath(_slug): AxPath<String>,
    _headers: HeaderMap,
) -> Response {
    unimplemented!("rfb workstream")
}
