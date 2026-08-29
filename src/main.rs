mod apps;
mod auth;
mod catalog;
mod engine;
mod helper;
mod proto;
mod origin;
mod proxy;
mod pty;
mod tls;
mod update;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use helper::Helper;
use proto::Request as HReq;
use rand::Rng;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(RustEmbed)]
#[folder = "ui/"]
struct Ui;

pub const COOKIE: &str = "wd_session";

/// The default listen port.
///
/// Five digits, and above 60999 on purpose: Linux hands out 32768-60999 for
/// outbound sockets, so a fixed port inside that range is one that something
/// else on the host may already be holding when the service starts. 61443 is
/// registered to nothing, and the tail reads as what it is.
pub const DEFAULT_PORT: u16 = 61443;

pub struct Session {
    pub ident: auth::Identity,
    helper: Mutex<Helper>,
}

#[derive(Clone)]
pub struct AppState {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    /// Whether this process is the one terminating TLS, which decides whether
    /// the session cookie may carry `Secure`. Not guessed from the request:
    /// behind a proxy that terminates TLS the operator sets `WD_SECURE=on`,
    /// and a forwarded header is something a client can also send.
    secure: bool,
    /// Whether this process terminates TLS itself. Distinct from `secure`: an
    /// operator with something in front sets `WD_SECURE=on` while this process
    /// speaks plaintext, and an extra listener has to match what this process
    /// does rather than what is true at the far end.
    tls_on: bool,
    /// Listeners serving a single app at the root of a port of its own. Empty
    /// unless something in the catalog sets `needs_origin`. See `origin.rs`.
    origins: origin::Origins,
}

impl AppState {
    pub fn tls_on(&self) -> bool {
        self.tls_on
    }
    pub fn origins(&self) -> &origin::Origins {
        &self.origins
    }
}

fn main() {
    // The helper re-executes this binary. Handle that before anything else --
    // it must not start a runtime, bind a port, or touch shared state.
    if std::env::args().nth(1).as_deref() == Some("--helper") {
        helper::run_child(3);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "webdesk=info,tower_http=warn".into()),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    if let Err(e) = rt.block_on(serve()) {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    if !nix::unistd::Uid::effective().is_root() {
        tracing::warn!("not running as root; PAM authentication will fail");
    }

    // https unless told otherwise. `WD_TLS=off` is for a host where something
    // in front is already terminating TLS -- that operator also wants
    // `WD_SECURE=on`, since the browser is on https even though this process
    // is not.
    let tls_on = !off(&std::env::var("WD_TLS").unwrap_or_default());
    let secure = tls_on || !off(&std::env::var("WD_SECURE").unwrap_or_else(|_| "off".into()));

    let state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        secure,
        tls_on,
        origins: Default::default(),
    };

    // Anything already installed that wants a port of its own gets it back
    // before the main listener opens, so a restart does not leave one of them
    // unreachable while the desk looks healthy.
    origin::start_installed(&state).await;

    let app = Router::new()
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/fs/list", get(fs_list))
        .route("/api/fs/read", get(fs_read))
        .route("/api/fs/write", put(fs_write))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/fs/remove", post(fs_remove))
        .route("/api/fs/rename", post(fs_rename))
        .route("/api/system/info", get(update::system_info))
        .route("/api/update/check", post(update::update_check))
        .route("/api/update/apply", post(update::update_apply))
        .route("/api/update/status", get(update::update_status))
        .route("/ws/term", get(pty::ws_term))
        .route("/api/apps/catalog", get(apps::catalog_list))
        .route("/api/apps/list", get(apps::list))
        .route("/api/apps/status", get(apps::status))
        .route("/api/apps/install", post(apps::install))
        .route("/api/apps/start", post(apps::start))
        .route("/api/apps/stop", post(apps::stop))
        .route("/api/apps/remove", post(apps::remove))
        // Container apps, on this origin so they can share the session and sit
        // in an iframe. `any` rather than `get`: an app behind here serves the
        // whole method surface, uploads and websockets included.
        .route("/app/{slug}", any(proxy::handle_root))
        .route("/app/{slug}/", any(proxy::handle_index))
        .route("/app/{slug}/{*rest}", any(proxy::handle))
        .fallback(get(static_asset))
        .with_state(state);

    let addr = std::env::var("WD_LISTEN").unwrap_or_else(|_| format!("0.0.0.0:{DEFAULT_PORT}"));

    if tls_on {
        let listener = tls::bind(&addr, tls::config()?).await?;
        tracing::info!("webdesk listening on https://{addr}");
        axum::serve(listener, app).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::warn!("WD_TLS=off: serving plain http on {addr}");
        if !secure {
            tracing::warn!("passwords and session cookies will cross the network in the clear");
        }
        axum::serve(listener, app).await?;
    }
    Ok(())
}

/// Read one of the on/off knobs. Anything that plainly means no is no; an
/// unset variable is not an answer and leaves the caller's default standing.
fn off(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "no" | "false")
}

// ------------------------------------------------------------------ sessions

pub fn session_of(state: &AppState, headers: &HeaderMap) -> Option<Arc<Session>> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let token = raw
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v)?;
    state.sessions.lock().ok()?.get(token).cloned()
}

/// The `; Secure` half of a cookie, or nothing.
fn secure(state: &AppState) -> &'static str {
    if state.secure {
        "; Secure"
    } else {
        ""
    }
}

pub fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "not signed in"}))).into_response()
}

/// Run one helper round-trip off the async runtime. The helper is strictly
/// sequential, so the mutex also serialises access to it.
async fn ask(
    session: Arc<Session>,
    req: HReq,
    payload: Vec<u8>,
) -> Result<(proto::Response, Vec<u8>), String> {
    tokio::task::spawn_blocking(move || {
        let mut h = session.helper.lock().map_err(|_| "helper lock poisoned".to_string())?;
        h.request(&req, &payload).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// --------------------------------------------------------------------- login

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> Response {
    let ident = match auth::authenticate(&body.username, &body.password) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(user = %body.username, "login rejected: {e}");
            // Deliberately vague to the client; the detail stays in the log.
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid username or password"})))
                .into_response();
        }
    };

    let helper = match Helper::spawn(&ident.username, ident.uid, ident.gid, &ident.home) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("helper spawn failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "could not open session"})))
                .into_response();
        }
    };

    let token: String = {
        let mut b = [0u8; 32];
        rand::thread_rng().fill(&mut b);
        b.iter().map(|x| format!("{x:02x}")).collect()
    };

    let username = ident.username.clone();
    let home = ident.home.clone();
    let admin = ident.admin;
    let session = Arc::new(Session { ident, helper: Mutex::new(helper) });
    state.sessions.lock().unwrap().insert(token.clone(), session);
    tracing::info!(user = %username, "session opened");

    // `Secure` whenever the browser is on https, which is the default. It is
    // left off under `WD_TLS=off` because a cookie a browser refuses to send
    // over http is a login that silently never sticks.
    let cookie = format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/{}", secure(&state));
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({"username": username, "home": home, "admin": admin})),
    )
        .into_response()
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(raw) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        if let Some(token) = raw
            .split(';')
            .filter_map(|c| c.trim().split_once('='))
            .find(|(k, _)| *k == COOKIE)
            .map(|(_, v)| v.to_string())
        {
            // Dropping the Session kills its helper process.
            state.sessions.lock().unwrap().remove(&token);
        }
    }
    // Same attributes as the one that was set, or the browser keeps the old
    // cookie alongside the expired one.
    let cleared =
        format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/{}; Max-Age=0", secure(&state));
    (StatusCode::OK, [(header::SET_COOKIE, cleared)], Json(json!({"ok": true}))).into_response()
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match session_of(&state, &headers) {
        Some(s) => Json(json!({
            "username": s.ident.username,
            "home": s.ident.home,
            "uid": s.ident.uid,
            "admin": s.ident.admin,
        }))
        .into_response(),
        None => unauthorized(),
    }
}

// ---------------------------------------------------------------- filesystem

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
    #[serde(default)]
    to: String,
}

fn hreq(op: &str, path: &str, to: &str, len: usize) -> HReq {
    HReq { op: op.into(), path: path.into(), to: to.into(), len }
}

async fn simple_op(state: AppState, headers: HeaderMap, op: &str, path: &str, to: &str) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    match ask(session, hreq(op, path, to, 0), Vec::new()).await {
        Ok((r, _)) if r.ok => Json(r.data.unwrap_or(json!({}))).into_response(),
        Ok((r, _)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": r.error.unwrap_or_else(|| "failed".into())})),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    }
}

async fn fs_list(State(s): State<AppState>, h: HeaderMap, Query(q): Query<PathQuery>) -> Response {
    simple_op(s, h, "list", &q.path, "").await
}

async fn fs_mkdir(State(s): State<AppState>, h: HeaderMap, Json(b): Json<PathBody>) -> Response {
    simple_op(s, h, "mkdir", &b.path, "").await
}

async fn fs_remove(State(s): State<AppState>, h: HeaderMap, Json(b): Json<PathBody>) -> Response {
    simple_op(s, h, "remove", &b.path, "").await
}

async fn fs_rename(State(s): State<AppState>, h: HeaderMap, Json(b): Json<PathBody>) -> Response {
    simple_op(s, h, "rename", &b.path, &b.to).await
}

async fn fs_read(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<PathQuery>) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    match ask(session, hreq("read", &q.path, "", 0), Vec::new()).await {
        Ok((r, bytes)) if r.ok => {
            let name = q.path.rsplit('/').next().unwrap_or("download");
            let mime = mime_guess::from_path(&q.path).first_or_octet_stream();
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    (header::CONTENT_DISPOSITION, format!("inline; filename=\"{name}\"")),
                ],
                bytes,
            )
                .into_response()
        }
        Ok((r, _)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": r.error.unwrap_or_else(|| "failed".into())})),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    }
}

async fn fs_write(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
    body: Bytes,
) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    let len = body.len();
    match ask(session, hreq("write", &q.path, "", len), body.to_vec()).await {
        Ok((r, _)) if r.ok => Json(json!({"ok": true, "bytes": len})).into_response(),
        Ok((r, _)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": r.error.unwrap_or_else(|| "failed".into())})),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(),
    }
}

// -------------------------------------------------------------------- assets

async fn static_asset(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Ui::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.to_string())], f.data.into_owned()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
