//! Self-update: report the running build, ask the remote what the latest one
//! is, and hand the actual work to a script running outside this service.
//!
//! Two things shape this module.
//!
//! **It is the one privileged action in the program.** Everywhere else the
//! kernel decides what a session may do, because the helper has already become
//! the user (see `helper.rs`). An update cannot work that way: it replaces a
//! root-owned binary and restarts a root service, so it runs with the daemon's
//! own privileges. That makes it the one place where authorisation has to be
//! checked in code rather than delegated -- so it is checked against the host's
//! administrative group, and it is checked on every entry point. A session that
//! is not in that group can reach none of this.
//!
//! **The updater must outlive the service it updates.** Installing a new binary
//! ends with `systemctl restart webdesk`, and systemd's default KillMode
//! would take any child of ours down with it -- mid-update, having already
//! overwritten the binary. So the work is launched as a separate transient unit
//! and reports progress through files on disk, which the next daemon can read
//! once it comes back up.

use crate::auth;
use crate::{session_of, unauthorized, AppState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Baked in at build time by `build.rs`.
const COMMIT: &str = env!("WD_COMMIT");
const BUILT: &str = env!("WD_BUILT");
const BUILT_REF: &str = env!("WD_REF");
const VERSION: &str = env!("WD_VERSION");

const DEFAULT_REPO: &str = "HutsonLabs/WebDesk";
const DEFAULT_STATE_DIR: &str = "/var/lib/webdesk";
const DEFAULT_UPDATER: &str = "/usr/local/libexec/webdesk-update";

/// The transient unit the updater runs as. systemd refuses to start a unit that
/// already exists, which is exactly the mutex we want against a second update.
const UNIT: &str = "webdesk-update";

/// Cap on how much of the update log is handed to the browser at once.
const LOG_TAIL: usize = 64 * 1024;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn state_dir() -> PathBuf {
    PathBuf::from(env_or("WD_STATE_DIR", DEFAULT_STATE_DIR))
}

fn updater_path() -> PathBuf {
    PathBuf::from(env_or("WD_UPDATER", DEFAULT_UPDATER))
}

fn repo() -> String {
    env_or("WD_REPO", DEFAULT_REPO)
}

/// The branch or tag this install follows. `build.rs` records what the source
/// was fetched as; the unit file can override it to track something else.
fn tracked_ref() -> String {
    env_or("WD_REF", BUILT_REF)
}

fn status_file() -> PathBuf {
    state_dir().join("update.status")
}

fn log_file() -> PathBuf {
    state_dir().join("update.log")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------ availability

/// Why this host cannot self-update, if it cannot. `None` means it can.
fn unavailable() -> Option<String> {
    if env_or("WD_UPDATE", "on").eq_ignore_ascii_case("off") {
        return Some("updates are disabled on this host (WD_UPDATE=off)".into());
    }
    if !nix::unistd::Uid::effective().is_root() {
        return Some("the service is not running as root".into());
    }
    let updater = updater_path();
    if !updater.is_file() {
        return Some(format!("no updater installed at {}", updater.display()));
    }
    None
}

fn build_info() -> Value {
    json!({
        "version": VERSION,
        "commit": COMMIT,
        "ref": tracked_ref(),
        "built": BUILT.parse::<u64>().unwrap_or(0),
        "repo": repo(),
    })
}

/// Resolve the session, then require it to be administrative. Every update
/// entry point goes through here.
fn admin_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Arc<crate::Session>, Response> {
    let Some(session) = session_of(state, headers) else { return Err(unauthorized()) };
    if !session.ident.admin {
        tracing::warn!(
            user = %session.ident.username,
            "denied an update action: not in {:?}",
            auth::admin_groups()
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": format!(
                    "updating requires membership of {}",
                    auth::admin_groups().join(" or ")
                )
            })),
        )
            .into_response());
    }
    Ok(session)
}

fn bad(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

// ------------------------------------------------------------------ status

fn read_status() -> Value {
    match std::fs::read_to_string(status_file()) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| json!({"state": "idle"})),
        Err(_) => json!({"state": "idle"}),
    }
}

/// Replace the status file in one step, so a poll never reads a half-written
/// one. The updater script does the same thing for the same reason.
fn write_status(v: &Value) -> std::io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("update.status.new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(tmp, status_file())
}

/// Last `LOG_TAIL` bytes of the update log, trimmed to a character boundary.
fn log_tail() -> String {
    let Ok(bytes) = std::fs::read(log_file()) else { return String::new() };
    let start = bytes.len().saturating_sub(LOG_TAIL);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

// ---------------------------------------------------------------- handlers

/// Build and host facts. Available to any session -- it is what the About box
/// shows -- but the `updates.allowed` flag is what the UI gates the controls on.
pub async fn system_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    let reason = unavailable();
    Json(json!({
        "build": build_info(),
        "hostname": hostname(),
        "user": {
            "username": session.ident.username,
            "admin": session.ident.admin,
        },
        "updates": {
            // Both must hold: the host supports it and this user may do it.
            "allowed": session.ident.admin && reason.is_none(),
            "supported": reason.is_none(),
            "reason": reason,
            "admin_groups": auth::admin_groups(),
        },
    }))
    .into_response()
}

pub async fn update_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = admin_session(&state, &headers) {
        return r;
    }
    Json(json!({
        "status": read_status(),
        "log": log_tail(),
        "build": build_info(),
    }))
    .into_response()
}

/// Ask GitHub what the tracked ref points at now, and compare.
pub async fn update_check(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = admin_session(&state, &headers) {
        return r;
    }

    let (repo, git_ref) = (repo(), tracked_ref());
    let url = format!("https://api.github.com/repos/{repo}/commits/{git_ref}");

    let fetched = tokio::task::spawn_blocking(move || github_json(&url)).await;
    let latest = match fetched {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return bad(StatusCode::BAD_GATEWAY, e),
        Err(e) => return bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    };

    let sha = latest["sha"].as_str().unwrap_or_default().to_string();
    if sha.is_empty() {
        return bad(StatusCode::BAD_GATEWAY, "the remote did not return a commit");
    }
    let message = latest["commit"]["message"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let date = latest["commit"]["committer"]["date"].as_str().unwrap_or_default().to_string();

    // A dirty or unknown local build cannot be compared honestly, so say that
    // rather than claiming it is up to date.
    let comparable = COMMIT != "unknown" && !COMMIT.ends_with("-dirty");
    let behind = comparable && COMMIT != sha;

    Json(json!({
        "current": COMMIT,
        "latest": sha,
        "behind": behind,
        "comparable": comparable,
        "ref": git_ref,
        "repo": repo,
        "message": message,
        "date": date,
        "checked": now(),
    }))
    .into_response()
}

/// Start an update. Returns as soon as the work is handed off; progress is
/// polled from `/api/update/status`.
pub async fn update_apply(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let session = match admin_session(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Some(reason) = unavailable() {
        return bad(StatusCode::CONFLICT, reason);
    }
    if read_status()["state"] == "running" {
        return bad(StatusCode::CONFLICT, "an update is already running");
    }

    let actor = session.ident.username.clone();

    // Claim the slot before spawning. If the script dies before it writes its
    // own first status, the UI still sees that something was started.
    let initial = json!({
        "state": "running",
        "phase": "starting",
        "started": now(),
        "actor": actor,
        "from": COMMIT,
    });
    if let Err(e) = write_status(&initial) {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, format!("could not write update state: {e}"));
    }
    // Start the log fresh so the browser is not shown the previous run's output.
    let _ = std::fs::write(log_file(), b"");

    tracing::warn!(user = %actor, "self-update requested; launching {UNIT}");

    match spawn_updater(&actor) {
        Ok(()) => Json(json!({"ok": true, "started": true})).into_response(),
        Err(e) => {
            let _ = write_status(&json!({
                "state": "failed",
                "phase": "launch",
                "started": now(),
                "finished": now(),
                "actor": actor,
                "from": COMMIT,
                "error": e,
            }));
            bad(StatusCode::INTERNAL_SERVER_ERROR, e)
        }
    }
}

// ----------------------------------------------------------------- plumbing

/// One HTTPS GET, via curl.
///
/// `install.sh` already guarantees curl on the host, and borrowing it keeps an
/// HTTP client and a TLS stack -- easily ten times the size of this program --
/// out of the binary for the sake of one request a user makes by hand.
pub(crate) fn github_json(url: &str) -> Result<Value, String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: webdesk",
            url,
        ])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("update check failed ({})", out.status)
        } else {
            err
        });
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("unexpected response: {e}"))
}

fn spawn_updater(actor: &str) -> Result<(), String> {
    let updater = updater_path();

    if which("systemd-run") {
        // --collect reaps the unit once it exits, so a previous run does not
        // block the next one; without it a failed unit lingers and the name
        // stays taken.
        let out = std::process::Command::new("systemd-run")
            .args(["--unit", UNIT, "--collect", "--quiet", "--description", "WebDesk self-update"])
            .arg("--setenv")
            .arg(format!("WD_UPDATE_ACTOR={actor}"))
            .arg("--setenv")
            .arg(format!("WD_UPDATE_FROM={COMMIT}"))
            .arg(&updater)
            .output()
            .map_err(|e| format!("could not run systemd-run: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "systemd-run failed".into() } else { err });
    }

    // No systemd: nothing is going to kill the child on restart either, so a
    // detached process is enough.
    std::process::Command::new("setsid")
        .arg(&updater)
        .env("WD_UPDATE_ACTOR", actor)
        .env("WD_UPDATE_FROM", COMMIT)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start {}: {e}", updater.display()))
}

fn which(prog: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into())
        .split(':')
        .filter(|d| !d.is_empty())
        .any(|d| std::path::Path::new(d).join(prog).is_file())
}

fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default()
}
