//! The pixels of a streamed app, carried to the browser.
//!
//! A streamed entry (`catalog::Streamed`) runs under `cage` with `wayvnc`
//! serving RFB on a unix socket. This module is the one hop between that socket
//! and a WebSocket the browser's noVNC client speaks: it checks the session,
//! works out which socket the person asking is entitled to, opens it, and then
//! copies bytes until one of the two ends stops.
//!
//! **It is deliberately not `proxy.rs`.** Nearly everything that file does is
//! about HTTP -- a request line to rewrite into origin form, a prefix to
//! announce, hop-by-hop headers to drop, `Set-Cookie` pinned to a path so one
//! app cannot read another's, `X-Frame-Options` taken away so the frame renders
//! at all. None of it exists here. There is no request to forward, because RFB
//! is not request/response and the server speaks first; no header to rewrite,
//! because the far end has never heard of HTTP; and no cookie to pin, because
//! nothing behind this socket knows a browser is involved. Take all of that
//! away and what is left is a byte pump, so routing this through the proxy
//! would mean carrying the whole of that machinery to reach the one call that
//! opens a socket -- and inviting a later change to the proxy to arrive here
//! without anyone meaning it to.
//!
//! What this file does have that the proxy does not is the rule in `ws_rfb`:
//! the uid is taken from the session and never from the request.

use crate::{catalog, session_of, AppState};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path as AxPath, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::io::ErrorKind;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

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

/// The catalog entry this slug names, if it is one that is drawn on this host.
///
/// A function of its own because it is the only thing standing between a string
/// out of a URL and a path on the filesystem, and a check that cannot be called
/// from a test is a check nobody reads twice.
///
/// Note what is not here. Nothing strips `..`, nothing rejects a `/`, and
/// nothing normalises anything, because a slug is never cleaned up until it is
/// safe -- it either equals one of the `&'static str`s compiled into `CATALOG`
/// or it is refused. That is the same rule `systemd.rs` states for unit names
/// and `session.rs` for application ids, and it is worth more than a filter: a
/// filter has to anticipate what an attacker will send, and this does not have
/// to anticipate anything.
fn streamed(slug: &str) -> Option<&'static catalog::App> {
    catalog::find(slug).filter(|a| a.streamed.is_some())
}

/// Whether a failure to connect means "that application is not running".
///
/// `NotFound` is the ordinary case and the expected one: nothing has created the
/// socket because no unit has started. `ConnectionRefused` is the same answer
/// reached from a crash -- a unix socket's inode outlives the process that bound
/// it, so a `wayvnc` that was killed leaves behind a file in exactly the place a
/// running one would have.
///
/// Everything else deliberately misses. A permission error on a path built from
/// your own uid means something about the session directory is wrong, and
/// answering "not running" would send someone to press Open, watch it succeed,
/// and arrive back at this same message with nothing having changed.
fn not_running(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused)
}

/// How much is read from the socket at a time, per open window.
///
/// Not the 8 KiB `tokio::io::copy` would have used. RFB is not a stream of small
/// messages in the direction that matters: a framebuffer update covering a 1080p
/// window is megabytes raw and still hundreds of kilobytes once Tight or ZRLE
/// has had it, and it arrives all at once when a menu opens or a page scrolls.
/// At 8 KiB that single repaint becomes dozens of reads, dozens of WebSocket
/// frames and dozens of wakeups -- work that scales with the size of the window
/// rather than with how much of it changed.
///
/// 64 KiB is roughly where the return stops. A read cannot hand back more than
/// the socket has buffered, and the default send buffer for a unix stream is a
/// couple of hundred kilobytes, so a much larger figure would mostly be memory
/// held open per window against a ceiling it cannot reach. What it costs is
/// 64 KiB per streamed window, reused for every read, with the copy out of it
/// sized to what actually arrived.
const READ_BUF: usize = 64 * 1024;

/// Where this session listens for the one thing the browser can tell it.
///
/// Beside the RFB socket, in the same `0700` directory and owned by the same
/// user, because it carries the same authority: whoever may see the pixels may
/// decide how many of them there are.
///
/// It exists because the obvious route does not work. A VNC client asks for a
/// desktop size with `SetDesktopSize`, and `wayvnc` up to and including 0.7.2 --
/// which is what Debian, Ubuntu and EPEL 9 ship -- never registers a handler for
/// it, so the request is received and discarded with nothing logged. `cage` can
/// resize perfectly well; it implements `wlr-output-management`, which is the
/// protocol `wlr-randr` drives. So the resize is applied from inside the session
/// instead of asked for through the stream, and this socket is how the size gets
/// there.
pub fn control_path(uid: u32, slug: &str) -> PathBuf {
    PathBuf::from(format!("/run/webdesk/rfb/{uid}/{slug}.ctl"))
}

/// `GET /ws/rfb/{slug}` -- a WebSocket carrying raw RFB in binary frames.
///
/// The three checks below run in this order for a reason, and it is the whole
/// of what this route is trusted with:
///
/// **The session comes first, before the slug is looked at.** Anyone without one
/// is told the same thing whatever they asked for, so a request from outside
/// cannot be used to find out what this host is running. A 404 for one name and
/// a 503 for another would answer that question exactly, and answering it is
/// worse than it sounds: the names are the names of applications, and which ones
/// a machine has open is a fact about the person using it.
///
/// **The uid comes from the session and never from the request.** This is the
/// security property of this file. The browser supplies a slug and nothing else;
/// the path is assembled from the authenticated identity's uid, so there is no
/// string a signed-in user can send that names another user's socket -- not by
/// asking for their app by name, because the name is all they get to choose, and
/// the name is not the part that selects whose it is. Everything else about a
/// streamed app follows from this: it is why the unit is a *user* unit, why the
/// directory is `0700`, and why the app has the right home directory to begin
/// with.
///
/// **And the slug must name a streamed entry.** See `streamed` for why the check
/// is a catalog lookup and not a sanitiser.
pub async fn ws_rfb(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    AxPath(slug): AxPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(session) = session_of(&state, &headers) else {
        return (StatusCode::UNAUTHORIZED, "not signed in").into_response();
    };

    let Some(app) = streamed(&slug) else {
        return (StatusCode::NOT_FOUND, "no application by that name is drawn on this host")
            .into_response();
    };

    // `app.slug` rather than the `slug` that arrived, though the two are equal
    // by construction. The string that becomes a path component is then plainly
    // a `&'static str` out of the build, visible right here, rather than a fact
    // about `catalog::find` that a reader has to go elsewhere to confirm.
    let path = socket_path(session.ident.uid, app.slug);

    // Connected before the upgrade rather than after it, which is the one place
    // this differs in shape from `pty::ws_term`. A PTY that will not open has
    // nothing to say for itself, so that file answers 101 and writes the reason
    // down the socket; here the ordinary failure is that the app has not been
    // opened yet, and that has an HTTP status which says so. It is worth far
    // more than a close frame: a browser tells a script nothing about a
    // WebSocket that opened and then closed except that it closed.
    let sock = match UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(e) if not_running(&e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "that application is not running yet -- open it first",
            )
                .into_response();
        }
        Err(e) => {
            // Not "not running", and the reason is in the log rather than in
            // the answer: the person at the browser cannot act on it, and the
            // operator who can is reading the journal.
            tracing::warn!(
                user = %session.ident.username,
                slug = %app.slug,
                "could not open the rfb socket: {e}"
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, "could not reach that application")
                .into_response();
        }
    };

    ws.on_upgrade(move |socket| pump(socket, sock))
}

/// Copy bytes between the browser and `wayvnc` until one of them stops.
///
/// Two tasks rather than `tokio::io::copy_bidirectional`, because one side is
/// not an `AsyncRead` at all: a WebSocket is a sequence of messages, and putting
/// those back into a stream of bytes -- and deciding what to do with the ones
/// that are not bytes -- is most of what this function is.
async fn pump(socket: WebSocket, sock: UnixStream) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (mut app_rx, mut app_tx) = sock.into_split();

    // browser -> wayvnc. Pointer motion, keys, clipboard, and the client half of
    // the handshake: small and frequent, and never the thing that is slow.
    let mut to_app = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(data) => {
                    if app_tx.write_all(&data).await.is_err() {
                        break;
                    }
                }
                // RFB has no text in it anywhere, so a text frame is not a
                // message this could relay badly -- it is a client that is not
                // speaking the protocol. Hanging up beats writing the bytes on
                // and letting `wayvnc` fail somewhere further along, where the
                // reason is no longer visible.
                Message::Text(_) => break,
                Message::Close(_) => break,
                // The WebSocket layer talking to itself. axum answers a ping on
                // its own, and forwarding either of these would splice bytes
                // into the middle of an RFB message -- a protocol with no
                // framing of its own, so nothing downstream could ever get back
                // into step.
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
    });

    // wayvnc -> browser. Nearly all of the traffic, and the direction READ_BUF
    // was chosen for.
    let mut to_browser = tokio::spawn(async move {
        let mut buf = vec![0u8; READ_BUF];
        loop {
            match app_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let frame = Message::Binary(Bytes::copy_from_slice(&buf[..n]));
                    if ws_tx.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
        // The application quit, or the compositor did. Saying so is the
        // difference between noVNC reporting a disconnection and reporting a
        // network error, and only one of those is true.
        let _ = ws_tx.close().await;
    });

    // Whichever side ends first takes the other with it. Without this the
    // survivor would sit on a read that can never complete, holding its half of
    // both sockets for as long as the process runs -- once for every window
    // anybody ever closed, on a daemon that is expected to stay up for months.
    tokio::select! {
        _ = &mut to_app => to_browser.abort(),
        _ = &mut to_browser => to_app.abort(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two people with the same application open are two compositors and two
    /// sockets. A path that left the uid out would put the second person on the
    /// first one's screen -- and it would look to both of them like it worked.
    #[test]
    fn a_socket_belongs_to_one_user_and_one_app() {
        assert_ne!(socket_path(1000, "gimp"), socket_path(1001, "gimp"));
        assert_ne!(socket_path(1000, "gimp"), socket_path(1000, "inkscape"));
        assert_ne!(socket_dir(1000), socket_dir(1001));
        assert_eq!(socket_path(1000, "gimp").parent().unwrap(), socket_dir(1000));
    }

    /// The slug is a path component, and this is what makes that safe: every
    /// name the catalog carries stays inside its own user's directory, and the
    /// names that would not are not names the catalog carries.
    #[test]
    fn a_slug_cannot_climb_out_of_its_own_directory() {
        for app in catalog::CATALOG {
            let path = socket_path(4242, app.slug);
            assert_eq!(path.parent().unwrap(), socket_dir(4242), "{} escapes", app.slug);
        }

        // Not because the formatting is careful -- it is not. Joined on, `..`
        // does exactly what it says it does:
        assert_ne!(socket_path(4242, "../1001/gimp").parent().unwrap(), socket_dir(4242));

        // -- which is why the lookup is what carries the weight. None of these
        // is a name in the catalog, so none of them reaches a path at all.
        for bad in ["..", "../..", "../1001/gimp", "a/b", "/etc/passwd", "firefox/../../root", ""] {
            assert!(streamed(bad).is_none(), "{bad:?} was accepted as an application");
        }
    }

    /// Only an application drawn on this host has a socket here. The entry a
    /// looser check gets wrong is `term-hut-host`: it is a Flatpak as well, so
    /// anything asking "does this entry have a Flatpak in it" says yes, and the
    /// browser then waits on a socket no unit will ever create.
    #[test]
    fn only_a_streamed_entry_is_served_here() {
        for app in catalog::CATALOG {
            assert_eq!(
                streamed(app.slug).is_some(),
                app.streamed.is_some(),
                "{} is served by the wrong half of this daemon",
                app.slug
            );
        }
        // Named rather than derived, and safe to name: the catalog says this
        // slug cannot move, because it is the key of the record in `apps.json`.
        assert!(streamed("term-hut-host").is_none(), "a host service has no socket here");
        assert!(streamed("no-such-app").is_none());
    }

    /// Refusing to call a permission error "not running". It cannot happen on a
    /// path built from your own uid, and if it ever does the honest answer is
    /// that something here is wrong -- not an invitation to press Open, watch it
    /// succeed, and come back to the same message.
    #[test]
    fn a_permission_error_is_not_reported_as_not_running() {
        use std::io::Error;
        assert!(not_running(&Error::from(ErrorKind::NotFound)));
        assert!(not_running(&Error::from(ErrorKind::ConnectionRefused)));
        assert!(!not_running(&Error::from(ErrorKind::PermissionDenied)));
        assert!(!not_running(&Error::from(ErrorKind::InvalidInput)));
    }

    /// A directory of our own under `/tmp`. The real one is under `/run`, which
    /// a test is not root enough to write to and should not be.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("webdesk-rfb-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The ordinary state of this route: nothing is there because nothing has
    /// been opened yet. It has to read as "not running", because the fix is a
    /// press of Open and the browser has to be able to tell that from a failure
    /// it should give up on.
    #[tokio::test]
    async fn nothing_listening_reads_as_not_running() {
        let path = scratch("absent").join("gimp.sock");
        let err = UnixStream::connect(&path).await.unwrap_err();
        assert!(not_running(&err), "{err} did not read as not running");
    }

    /// The same answer arrived at from a crash. A unix socket's inode outlives
    /// the process that bound it, so a `wayvnc` that was killed leaves its file
    /// exactly where a live one would have -- which is why this connects rather
    /// than asking whether the file is there. A check on the file would report
    /// the application as up and then hand the browser a socket that answers
    /// nothing.
    #[tokio::test]
    async fn a_socket_left_behind_by_a_crash_is_also_not_running() {
        let path = scratch("stale").join("gimp.sock");
        drop(tokio::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists(), "the socket file was cleaned up; this test proves nothing");

        let err = UnixStream::connect(&path).await.unwrap_err();
        assert!(not_running(&err), "{err} did not read as not running");
    }

    /// And a socket something is listening on is connected to, with the server
    /// speaking first -- which is the reason the connection is made before the
    /// upgrade rather than after it. This stands in for `wayvnc`, which is not
    /// installed on the machines this is developed on.
    #[tokio::test]
    async fn a_listening_socket_is_connected_to_and_the_server_speaks_first() {
        let path = scratch("live").join("gimp.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (mut server, _) = listener.accept().await.unwrap();
            server.write_all(b"RFB 003.008\n").await.unwrap();
        });

        let mut sock = UnixStream::connect(&path).await.expect("a listening socket refused us");
        let mut hello = [0u8; 12];
        sock.read_exact(&mut hello).await.unwrap();
        assert_eq!(&hello, b"RFB 003.008\n");
    }
}
