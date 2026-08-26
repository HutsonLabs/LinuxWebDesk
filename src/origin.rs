//! Serving one application at the root of a port of its own.
//!
//! Most of the catalog is reached at `/app/<slug>/` and shares WebDesk's
//! origin, which is the cheaper arrangement in every way: one port to open, one
//! certificate, and cookies the proxy can pin per app. This module is for the
//! entries that cannot be reached that way at all -- see `needs_origin` in
//! `catalog.rs` -- and it exists because "does this app tolerate a path prefix"
//! was otherwise deciding what WebDesk is allowed to install.
//!
//! **It is still WebDesk's door.** A listener here refuses anyone without a
//! session, exactly as `proxy.rs` does; the app behind it is not published to
//! the network, and its container still binds `127.0.0.1`. What changes is the
//! shape of the URL the browser uses, not who may open it.
//!
//! **Why a port and not a name.** A second hostname would keep the single-port
//! promise, but it needs a DNS record and a certificate that covers it, and
//! neither is something WebDesk can arrange on an arbitrary machine. A port
//! needs nothing: a certificate does not name a port, so whatever address the
//! operator already trusts for WebDesk -- a real domain with a real
//! certificate, or an IP with a self-signed one they clicked through once --
//! goes on being trusted at `:<port>` with no new warning and no new record.
//! That is what makes this portable rather than an arrangement that happens to
//! suit one machine.
//!
//! **What it costs, stated where it is implemented.** An open port per such
//! app, which the operator has to allow through their firewall. And the app's
//! own cookies stop being isolated from WebDesk's: `proxy.rs` pins them to
//! `/app/<slug>/`, and an app at `/` sets `Path=/` instead. Cookies are not
//! isolated by port (RFC 6265 section 8.5), so those now reach WebDesk's port
//! too. Nothing here can forge a session -- `wd_session` is refused from an app
//! in both paths -- but the privacy boundary between apps is weaker than the
//! prefixed arrangement, and an entry should only ask for this if it truly
//! cannot work without it.

use crate::{apps, session_of, tls, unauthorized, AppState};
use axum::extract::{Path, Request, State};
use axum::routing::any;
use axum::Router;
use axum::response::Response;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A running origin listener, and the way to stop it.
pub struct Origin {
    pub port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Origin {
    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

/// Every origin listener this process is running, by slug.
pub type Origins = Arc<Mutex<HashMap<String, Origin>>>;

/// Bring the listeners up for everything installed that wants one.
///
/// Called once at start. A port that will not bind is logged and skipped rather
/// than fatal: one app whose port was taken while WebDesk was down must not
/// stop the rest of the desk from serving.
pub async fn start_installed(state: &AppState) {
    for (slug, port) in apps::origin_ports() {
        if let Err(e) = start(state, &slug, port).await {
            tracing::error!(slug = %slug, port, "could not serve app on its own port: {e}");
        }
    }
}

/// Bind `port` and serve `slug` at the root of it until told to stop.
pub async fn start(state: &AppState, slug: &str, port: u16) -> Result<(), String> {
    // Replacing an existing one is a reinstall, not a conflict; drop the old
    // listener first so the bind below is not fighting it.
    stop(state, slug);

    let addr = format!("0.0.0.0:{port}");
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = Router::new()
        // One wildcard and one bare root. `any` for the same reason as the
        // prefixed routes: an app back here serves the whole method surface,
        // uploads and websockets included.
        .route("/", any(root))
        .route("/{*rest}", any(rest))
        .with_state((state.clone(), slug.to_string()));

    if state.tls_on() {
        let config = tls::config()?;
        let listener = tls::bind(&addr, config).await.map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async { let _ = rx.await; })
                .await;
        });
    } else {
        let listener =
            tokio::net::TcpListener::bind(&addr).await.map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async { let _ = rx.await; })
                .await;
        });
    }

    let scheme = if state.tls_on() { "https" } else { "http" };
    tracing::info!(slug = %slug, "serving on its own origin at {scheme}://<this host>:{port}/");
    state.origins().lock().unwrap().insert(slug.to_string(), Origin { port, stop: Some(tx) });
    Ok(())
}

/// Stop serving `slug` on its own port, if it was.
pub fn stop(state: &AppState, slug: &str) {
    if let Some(mut o) = state.origins().lock().unwrap().remove(slug) {
        tracing::info!(slug = %slug, port = o.port, "no longer serving on its own origin");
        o.stop();
    }
}

async fn root(state: State<(AppState, String)>, req: Request) -> Response {
    serve(state, String::new(), req).await
}

async fn rest(
    state: State<(AppState, String)>,
    Path(rest): Path<String>,
    req: Request,
) -> Response {
    serve(state, rest, req).await
}

/// The same relay `proxy.rs` performs, with an empty prefix.
///
/// An empty prefix is not a special case there, it is the honest description of
/// this arrangement: nothing to strip from the path, cookies pinned to `/`
/// because `/` is all this origin serves, and redirects left as the app wrote
/// them. So the cookie stripping, the `wd_session` refusal, the frame-ancestors
/// handling and the upgrade path are shared rather than reimplemented.
async fn serve(
    State((state, slug)): State<(AppState, String)>,
    rest: String,
    req: Request,
) -> Response {
    if session_of(&state, req.headers()).is_none() {
        return unauthorized();
    }
    let Some((port, tls)) = apps::upstream_of(&slug) else {
        return crate::proxy::not_installed(&slug);
    };
    crate::proxy::forward(port, tls, "", &slug, rest, req).await
}
