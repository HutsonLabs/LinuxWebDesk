//! Installing, running and removing container applications.
//!
//! **Who may do this.** Creating a container means choosing what code the
//! engine runs, and the engine runs as root; there is no way to hand that to a
//! session without handing it the host. So install, remove, start and stop are
//! checked against the same administrative group as the self-updater, for the
//! same reason and by the same rule. Everyone signed in may *list* the
//! installed apps and open them -- an installed app is part of the host, like a
//! package, not a possession of whoever installed it.
//!
//! This is the second place in the program where authorisation lives in code
//! rather than in the kernel. The first is `update.rs`, which explains why that
//! is a cost worth naming out loud.
//!
//! **What a user may actually choose.** Only the blanks a catalog entry
//! declares. Everything WebDesk depends on to keep working -- the container
//! name, the published port and the address it binds to, the `/config`
//! directory, `PUID`/`PGID`, the labels, the restart policy -- is computed
//! here and cannot be reached from the browser. A user picks an app and answers
//! its questions; they do not describe a container.

use crate::catalog::{self, Kind};
use crate::engine::{self, Engine, RunSpec};
use crate::{auth, session_of, unauthorized, AppState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_STATE_DIR: &str = "/var/lib/webdesk";

/// Host ports for container apps. Loopback only, and well clear of both the
/// default listen port and the ephemeral range.
const PORT_LOW: u16 = 47000;
const PORT_HIGH: u16 = 47999;

const LOG_TAIL: usize = 64 * 1024;

/// Refuse a typed value longer than this. Nothing in the catalog wants more,
/// and it keeps a pathological value out of an argv list.
const MAX_VALUE: usize = 1024;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn state_dir() -> PathBuf {
    PathBuf::from(env_or("WD_STATE_DIR", DEFAULT_STATE_DIR))
}

/// Where the `/config` directory of every app lives, one subdirectory each.
fn appdata_dir() -> PathBuf {
    match std::env::var("WD_APPDATA").ok().filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => state_dir().join("appdata"),
    }
}

fn apps_file() -> PathBuf {
    state_dir().join("apps.json")
}

fn status_file() -> PathBuf {
    state_dir().join("apps.status")
}

fn log_file() -> PathBuf {
    state_dir().join("apps.log")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn bad(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

// ------------------------------------------------------------------- state

/// One installed application, as recorded on disk.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Installed {
    pub slug: String,
    pub image: String,
    /// The loopback port the container publishes on. The proxy's only input.
    pub port: u16,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `(host, container, read-only)`
    #[serde(default)]
    pub mounts: Vec<(String, String, bool)>,
    /// Keys of `env` that came from a secret field, so they are never sent back.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub installed: u64,
    #[serde(default)]
    pub actor: String,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Book {
    #[serde(default)]
    apps: BTreeMap<String, Installed>,
}

fn read_book() -> Book {
    match std::fs::read_to_string(apps_file()) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_default(),
        Err(_) => Book::default(),
    }
}

/// Replace the book in one step, so a concurrent read never sees half of it.
fn write_book(b: &Book) -> std::io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("apps.json.new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(b)?)?;
    std::fs::rename(tmp, apps_file())
}

/// The one lookup the proxy needs: which loopback port serves this slug.
pub fn port_of(slug: &str) -> Option<u16> {
    read_book().apps.get(slug).map(|a| a.port)
}

fn read_status() -> Value {
    match std::fs::read_to_string(status_file()) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|_| json!({"state": "idle"})),
        Err(_) => json!({"state": "idle"}),
    }
}

fn write_status(v: &Value) -> std::io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("apps.status.new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(tmp, status_file())
}

fn log_tail() -> String {
    let Ok(bytes) = std::fs::read(log_file()) else { return String::new() };
    let start = bytes.len().saturating_sub(LOG_TAIL);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

// ----------------------------------------------------------- authorisation

fn admin_session(state: &AppState, headers: &HeaderMap) -> Result<Arc<crate::Session>, Response> {
    let Some(session) = session_of(state, headers) else { return Err(unauthorized()) };
    if !session.ident.admin {
        tracing::warn!(
            user = %session.ident.username,
            "denied an app action: not in {:?}",
            auth::admin_groups()
        );
        return Err(bad(
            StatusCode::FORBIDDEN,
            format!(
                "installing apps requires membership of {}",
                auth::admin_groups().join(" or ")
            ),
        ));
    }
    Ok(session)
}

// -------------------------------------------------------------- validation

/// Directories a bind mount may never name, whatever else is true. Matching is
/// by prefix, so `/etc` also rejects `/etc/ssh`.
const FORBIDDEN_MOUNTS: &[&str] = &[
    "/", "/etc", "/proc", "/sys", "/dev", "/boot", "/run", "/usr", "/bin", "/sbin", "/lib", "/lib64",
];

fn check_path(value: &str) -> Result<String, String> {
    let p = std::path::Path::new(value);
    if !p.is_absolute() {
        return Err(format!("{value} is not an absolute path"));
    }
    if value.contains("..") {
        return Err("a path may not contain ..".into());
    }
    // A colon would be read by the engine as the start of the next field.
    if value.contains(':') || value.contains('\0') {
        return Err("a path may not contain : or a null byte".into());
    }
    let canon = p
        .canonicalize()
        .map_err(|e| format!("{value} cannot be used: {e}"))?;
    if !canon.is_dir() {
        return Err(format!("{value} is not a directory"));
    }
    let s = canon.to_string_lossy().to_string();

    for bad in FORBIDDEN_MOUNTS {
        let hit = if *bad == "/" { s == "/" } else { s == *bad || s.starts_with(&format!("{bad}/")) };
        if hit {
            return Err(format!("{s} is part of the system and cannot be shared with a container"));
        }
    }
    // WebDesk's own state is off limits: an app that could write there could
    // rewrite the book that decides what gets run.
    let own = state_dir().to_string_lossy().to_string();
    if s == own || s.starts_with(&format!("{own}/")) {
        return Err(format!("{s} belongs to WebDesk and cannot be shared with a container"));
    }
    Ok(s)
}

fn check_scalar(value: &str) -> Result<(), String> {
    if value.len() > MAX_VALUE {
        return Err(format!("that value is longer than {MAX_VALUE} characters"));
    }
    if value.chars().any(|c| c == '\0' || (c.is_control() && c != '\t')) {
        return Err("that value contains a control character".into());
    }
    Ok(())
}

struct Answers {
    env: BTreeMap<String, String>,
    mounts: Vec<(String, String, bool)>,
    secrets: Vec<String>,
}

/// Turn what the browser sent into the exact set of environment variables and
/// mounts this entry allows -- and nothing else. Keys the catalog does not
/// declare are dropped rather than rejected, so a stale UI cannot wedge an
/// install, but they can never reach the engine.
fn validate(app: &catalog::App, given: &BTreeMap<String, String>) -> Result<Answers, String> {
    let mut out =
        Answers { env: BTreeMap::new(), mounts: Vec::new(), secrets: Vec::new() };

    for p in app.all_params() {
        let raw = given.get(p.key).map(|s| s.trim()).unwrap_or("");
        let value = if raw.is_empty() { p.default } else { raw };

        if value.is_empty() {
            if p.required {
                return Err(format!("{} is required", p.label));
            }
            continue;
        }
        check_scalar(value)?;

        match p.kind {
            Kind::HostPath { at, ro } => {
                let host = check_path(value)?;
                out.mounts.push((host, at.to_string(), ro));
            }
            Kind::Choice(opts) => {
                if !opts.contains(&value) {
                    return Err(format!("{} must be one of {}", p.label, opts.join(", ")));
                }
                out.env.insert(p.key.to_string(), value.to_string());
            }
            Kind::Toggle => {
                let v = match value {
                    "true" | "false" => value,
                    _ => return Err(format!("{} must be true or false", p.label)),
                };
                out.env.insert(p.key.to_string(), v.to_string());
            }
            Kind::Secret => {
                out.secrets.push(p.key.to_string());
                out.env.insert(p.key.to_string(), value.to_string());
            }
            Kind::Text => {
                out.env.insert(p.key.to_string(), value.to_string());
            }
        }
    }
    Ok(out)
}

/// The lowest unused port in the range. Checked against the book rather than
/// against the kernel, because a port a container has published is in use by
/// the engine and would not bind here anyway.
fn free_port(book: &Book) -> Result<u16, String> {
    let taken: Vec<u16> = book.apps.values().map(|a| a.port).collect();
    (PORT_LOW..=PORT_HIGH)
        .find(|p| !taken.contains(p))
        .ok_or_else(|| "no free port left for another app".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_relative_path_is_refused() {
        assert!(check_path("etc/passwd").is_err());
        assert!(check_path("").is_err());
    }

    #[test]
    fn a_path_with_a_parent_step_is_refused() {
        assert!(check_path("/var/tmp/../../etc").is_err());
    }

    #[test]
    fn a_path_with_a_colon_is_refused() {
        // A colon would be read by the engine as the start of the next field,
        // which is how a read-only mount would become writable.
        assert!(check_path("/tmp/x:/etc").is_err());
    }

    #[test]
    fn a_path_that_does_not_exist_is_refused() {
        assert!(check_path("/definitely/not/here/at/all").is_err());
    }

    #[test]
    fn the_root_of_the_filesystem_is_refused() {
        assert!(check_path("/").is_err());
    }

    #[test]
    fn a_control_character_is_refused() {
        assert!(check_scalar("two\nlines").is_err());
        assert!(check_scalar(&"x".repeat(MAX_VALUE + 1)).is_err());
        assert!(check_scalar("Europe/London").is_ok());
    }

    #[test]
    fn a_key_the_catalog_did_not_declare_never_reaches_the_engine() {
        let app = catalog::find("firefox").unwrap();
        // Every one of these would change what the container is, which is
        // exactly what a user does not get to do.
        let got = validate(
            app,
            &answers(&[
                ("TZ", "Europe/London"),
                ("PUID", "0"),
                ("PATH", "/evil"),
                ("LD_PRELOAD", "/evil.so"),
                ("SELKIES_ENCODER", "nonsense"),
            ]),
        )
        .unwrap();
        assert_eq!(got.env.get("TZ").map(String::as_str), Some("Europe/London"));
        for key in ["PUID", "PATH", "LD_PRELOAD", "SELKIES_ENCODER"] {
            assert!(!got.env.contains_key(key), "{key} got through");
        }
    }

    #[test]
    fn an_unanswered_optional_parameter_is_simply_absent() {
        let app = catalog::find("firefox").unwrap();
        let got = validate(app, &answers(&[])).unwrap();
        assert!(!got.env.contains_key("PASSWORD"));
        // ...but the default that does exist is applied.
        assert_eq!(got.env.get("TZ").map(String::as_str), Some("Etc/UTC"));
    }

    #[test]
    fn a_missing_required_parameter_stops_the_install() {
        // No shipping entry demands an answer, so this rule needs a subject of
        // its own rather than going untested until one does.
        let app = catalog::App {
            slug: "demo",
            name: "Demo",
            tagline: "",
            image: "example/demo",
            port: 80,
            icon: "a-box",
            base: None,
            config_at: "/config",
            lsio: true,
            shm: None,
            notes: "",
            params: &[catalog::Param {
                key: "NEEDED",
                label: "Needed",
                help: "",
                kind: catalog::Kind::Text,
                default: "",
                required: true,
            }],
        };
        // Matched rather than unwrap_err'd so that `Answers`, which holds
        // secrets, never needs a Debug impl.
        match validate(&app, &answers(&[])) {
            Ok(_) => panic!("a missing required parameter was accepted"),
            Err(e) => assert!(e.contains("required"), "{e}"),
        }
    }

    #[test]
    fn a_secret_is_recorded_as_one_so_it_is_never_echoed_back() {
        let app = catalog::find("firefox").unwrap();
        let got = validate(app, &answers(&[("PASSWORD", "hunter2")])).unwrap();
        assert!(got.secrets.contains(&"PASSWORD".to_string()));
        assert_eq!(got.env.get("PASSWORD").map(String::as_str), Some("hunter2"));
    }

    #[test]
    fn an_app_that_must_be_told_its_prefix_is_told_it_correctly() {
        // The two that need telling want the same path in different shapes.
        let vsc = catalog::find("vscodium-web").unwrap();
        assert_eq!(
            vsc.base_value("/app/vscodium-web"),
            Some(("CODE_ARGS", "--server-base-path=/app/vscodium-web".to_string()))
        );
        let hut = catalog::find("term-hut").unwrap();
        assert_eq!(hut.base_value("/app/term-hut"), Some(("HUT_BASE_PATH", "/app/term-hut".to_string())));
        // The Selkies desktops work it out from the page URL and are told nothing.
        assert_eq!(catalog::find("firefox").unwrap().base_value("/app/firefox"), None);
    }

    #[test]
    fn a_non_linuxserver_image_is_not_offered_linuxserver_settings() {
        let hut = catalog::find("term-hut").unwrap();
        assert!(!hut.lsio);
        // TZ is a LinuxServer convention; term.hut would ignore it.
        assert!(!hut.all_params().any(|p| p.key == "TZ"));
        assert_eq!(hut.config_at, "/home/hut");
    }

    #[test]
    fn the_desktop_apps_get_enough_shared_memory() {
        // A browser or IDE on the 64 MB default dies in ways that look like the
        // app being broken rather than the container being starved.
        for slug in ["firefox", "helium", "onlyoffice", "inkscape", "intellij-idea"] {
            let a = catalog::find(slug).unwrap_or_else(|| panic!("{slug} missing"));
            assert_eq!(a.shm, Some("1g"), "{slug}");
            assert_eq!(a.port, 3000, "{slug}");
        }
    }

    #[test]
    fn ports_are_handed_out_without_collision() {
        let mut book = Book::default();
        let first = free_port(&book).unwrap();
        book.apps.insert(
            "a".into(),
            Installed {
                slug: "a".into(),
                image: String::new(),
                port: first,
                env: BTreeMap::new(),
                mounts: Vec::new(),
                secrets: Vec::new(),
                installed: 0,
                actor: String::new(),
            },
        );
        assert_ne!(free_port(&book).unwrap(), first);
    }

    #[test]
    fn every_catalog_entry_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for a in catalog::CATALOG {
            assert!(seen.insert(a.slug), "duplicate slug {}", a.slug);
            // The slug reaches a URL path, a container name and a directory
            // name, so it has to be safe in all three.
            assert!(
                a.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not a safe slug",
                a.slug
            );
            assert!(!a.image.contains(':'), "{} should carry no tag", a.slug);
            assert!(a.port > 0, "{} has no port", a.slug);
            assert!(a.config_at.starts_with('/'), "{} has no state directory", a.slug);
            // The state directory is mounted by the installer; an entry that
            // also claimed it would produce two mounts at the same place.
            for p in a.params {
                if let Kind::HostPath { at, .. } = p.kind {
                    assert_ne!(at, a.config_at, "{} must not mount its own state dir", a.slug);
                }
            }
            // A template that never substitutes would silently hand the app an
            // empty prefix and look like the app ignoring it.
            if let Some(b) = &a.base {
                assert!(b.template.contains("{prefix}"), "{} has a base with no {{prefix}}", a.slug);
            }
        }
    }
}

// ---------------------------------------------------------------- handlers

/// What may be installed. Any session may read it; the UI uses `allowed` to
/// decide whether to offer the button, and every route re-checks regardless.
pub async fn catalog_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };

    let (engine_name, engine_error) = match engine::detect() {
        Some(e) => match engine::probe(e) {
            Ok(v) => (Some(format!("{} {}", e.name(), v)), None),
            Err(err) => (Some(e.name().to_string()), Some(err)),
        },
        None => (None, Some("no container engine found on this host".to_string())),
    };

    let mut body = catalog::as_json();
    body["engine"] = json!({
        "name": engine_name,
        "error": engine_error,
        "ready": engine_error.is_none(),
    });
    body["allowed"] = json!(session.ident.admin && engine_error.is_none());
    body["admin"] = json!(session.ident.admin);
    body["admin_groups"] = json!(auth::admin_groups());
    Json(body).into_response()
}

/// The installed apps, each with the container's live state folded in. Any
/// session may read this -- it is what paints the dock.
pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    let book = read_book();
    let engine = engine::detect();

    let apps: Vec<Value> = book
        .apps
        .values()
        .map(|a| {
            let entry = catalog::find(&a.slug);
            let status = match engine {
                Some(e) => engine::state(e, &a.slug),
                None => "unknown".to_string(),
            };
            json!({
                "slug": a.slug,
                "name": entry.map(|c| c.name).unwrap_or(&a.slug),
                "tagline": entry.map(|c| c.tagline).unwrap_or(""),
                "icon": entry.map(|c| c.icon).unwrap_or("a-box"),
                "notes": entry.map(|c| c.notes).unwrap_or(""),
                "image": a.image,
                "state": status,
                "url": format!("/app/{}/", a.slug),
                "installed": a.installed,
                "actor": a.actor,
                // Values are echoed back so the Apps window can show what was
                // chosen -- except the secret ones, which are only ever named.
                "env": a.env.iter()
                    .filter(|(k, _)| !a.secrets.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<BTreeMap<_, _>>(),
                "secrets": a.secrets,
                "mounts": a.mounts,
            })
        })
        .collect();

    Json(json!({
        "apps": apps,
        "admin": session.ident.admin,
        "engine": engine.map(|e| e.name()),
    }))
    .into_response()
}

pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = admin_session(&state, &headers) {
        return r;
    }
    Json(json!({ "status": read_status(), "log": log_tail() })).into_response()
}

#[derive(Deserialize)]
pub struct InstallReq {
    slug: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
    /// Image tag. Constrained to a short allow-list rather than taken as typed,
    /// since this half of the reference decides what code runs.
    #[serde(default)]
    tag: String,
}

const TAGS: &[&str] = &["latest", "develop"];

pub async fn install(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<InstallReq>,
) -> Response {
    let session = match admin_session(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let Some(app) = catalog::find(&req.slug) else {
        return bad(StatusCode::NOT_FOUND, format!("{} is not in the catalog", req.slug));
    };
    let Some(eng) = engine::detect() else {
        return bad(StatusCode::CONFLICT, "no container engine found on this host");
    };
    if let Err(e) = engine::probe(eng) {
        return bad(StatusCode::CONFLICT, e);
    }
    if read_status()["state"] == "running" {
        return bad(StatusCode::CONFLICT, "another install is already running");
    }

    let tag = if req.tag.is_empty() { "latest" } else { req.tag.as_str() };
    if !TAGS.contains(&tag) {
        return bad(StatusCode::BAD_REQUEST, format!("{tag} is not an offered tag"));
    }

    let book = read_book();
    if book.apps.contains_key(&req.slug) {
        return bad(StatusCode::CONFLICT, format!("{} is already installed", app.name));
    }

    let answers = match validate(app, &req.params) {
        Ok(a) => a,
        Err(e) => return bad(StatusCode::BAD_REQUEST, e),
    };
    let port = match free_port(&book) {
        Ok(p) => p,
        Err(e) => return bad(StatusCode::CONFLICT, e),
    };

    // The directory this app keeps its state in, owned by the identity the
    // container will run as.
    let config = appdata_dir().join(&req.slug);
    let (uid, gid) = (session.ident.uid, session.ident.gid);
    if let Err(e) = std::fs::create_dir_all(&config) {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, format!("could not create {}: {e}", config.display()));
    }
    let _ = nix::unistd::chown(
        &config,
        Some(nix::unistd::Uid::from_raw(uid)),
        Some(nix::unistd::Gid::from_raw(gid)),
    );

    let mut env = answers.env.clone();
    // Fixed by us, not offered: these are what make the container's files
    // readable by the person who installed it. Only sent to images that read
    // them -- term.hut runs as its own fixed user and would ignore them, so
    // sending them would just be noise in `docker inspect`.
    if app.lsio {
        env.insert("PUID".into(), uid.to_string());
        env.insert("PGID".into(), gid.to_string());
    }
    if let Some((key, value)) = app.base_value(&format!("/app/{}", app.slug)) {
        env.insert(key.to_string(), value);
    }

    let mut mounts = answers.mounts.clone();
    mounts.push((config.to_string_lossy().to_string(), app.config_at.to_string(), false));

    let image = format!("{}:{}", app.image, tag);
    let record = Installed {
        slug: app.slug.to_string(),
        image: image.clone(),
        port,
        env: env.clone(),
        mounts: mounts.clone(),
        secrets: answers.secrets,
        installed: now(),
        actor: session.ident.username.clone(),
    };

    let spec = RunSpec {
        slug: app.slug.to_string(),
        image: image.clone(),
        host_port: port,
        container_port: app.port,
        env: env.into_iter().collect(),
        mounts,
        shm: app.shm.map(str::to_string),
    };

    let actor = session.ident.username.clone();
    let _ = write_status(&json!({
        "state": "running",
        "phase": "pulling",
        "slug": app.slug,
        "name": app.name,
        "started": now(),
        "actor": actor,
    }));
    let _ = std::fs::write(log_file(), b"");

    tracing::warn!(user = %actor, slug = %app.slug, image = %image, "installing a container app");

    // The pull is the long part, so the request returns now and the browser
    // polls /api/apps/status -- the same shape the self-updater uses.
    let name = app.name.to_string();
    let slug = app.slug.to_string();
    tokio::task::spawn_blocking(move || {
        let log = log_file();
        let result = engine::pull(eng, &image, &log).and_then(|_| {
            let _ = write_status(&json!({
                "state": "running", "phase": "creating", "slug": slug,
                "name": name, "started": now(), "actor": actor,
            }));
            engine::create(eng, &spec, &log)
        });

        match result {
            Ok(()) => {
                // Only recorded once the container exists, so a failed install
                // leaves nothing behind to clean up.
                let mut book = read_book();
                book.apps.insert(slug.clone(), record);
                if let Err(e) = write_book(&book) {
                    let _ = engine::remove(eng, &slug);
                    let _ = write_status(&json!({
                        "state": "failed", "phase": "recording", "slug": slug, "name": name,
                        "finished": now(), "actor": actor,
                        "error": format!("the container was created but could not be recorded: {e}"),
                    }));
                    return;
                }
                let _ = write_status(&json!({
                    "state": "done", "phase": "installed", "slug": slug, "name": name,
                    "finished": now(), "actor": actor,
                }));
            }
            Err(e) => {
                // A half-created container would hold the name against a retry.
                let _ = engine::remove(eng, &slug);
                let _ = write_status(&json!({
                    "state": "failed", "phase": "install", "slug": slug, "name": name,
                    "finished": now(), "actor": actor, "error": e,
                }));
            }
        }
    });

    Json(json!({ "ok": true, "started": true, "slug": app.slug })).into_response()
}

#[derive(Deserialize)]
pub struct SlugReq {
    slug: String,
    /// Only read by `remove`: delete the app's `/config` directory too.
    #[serde(default)]
    purge: bool,
}

pub async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SlugReq>,
) -> Response {
    act(&state, &headers, &req.slug, "start", engine::start).await
}

pub async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SlugReq>,
) -> Response {
    act(&state, &headers, &req.slug, "stop", engine::stop).await
}

async fn act(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
    what: &str,
    f: fn(Engine, &str) -> Result<(), String>,
) -> Response {
    let session = match admin_session(state, headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !read_book().apps.contains_key(slug) {
        return bad(StatusCode::NOT_FOUND, format!("{slug} is not installed"));
    }
    let Some(eng) = engine::detect() else {
        return bad(StatusCode::CONFLICT, "no container engine found on this host");
    };

    tracing::info!(user = %session.ident.username, slug, "app {what}");
    let slug = slug.to_string();
    match tokio::task::spawn_blocking(move || f(eng, &slug)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SlugReq>,
) -> Response {
    let session = match admin_session(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut book = read_book();
    if !book.apps.contains_key(&req.slug) {
        return bad(StatusCode::NOT_FOUND, format!("{} is not installed", req.slug));
    }
    let Some(eng) = engine::detect() else {
        return bad(StatusCode::CONFLICT, "no container engine found on this host");
    };

    let slug = req.slug.clone();
    let removed = tokio::task::spawn_blocking(move || engine::remove(eng, &slug)).await;
    if let Ok(Err(e)) = removed {
        // Reported, not fatal: the container may already be gone, and refusing
        // to forget it would leave an app that can never be uninstalled.
        tracing::warn!(slug = %req.slug, "could not remove the container: {e}");
    }

    book.apps.remove(&req.slug);
    if let Err(e) = write_book(&book) {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, format!("could not update the app list: {e}"));
    }

    let mut purged = false;
    if req.purge {
        let config = appdata_dir().join(&req.slug);
        // Guarded against a slug that is not a plain name, since this deletes.
        let sane = req.slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if sane && config.starts_with(appdata_dir()) && config.is_dir() {
            match std::fs::remove_dir_all(&config) {
                Ok(()) => purged = true,
                Err(e) => tracing::warn!(slug = %req.slug, "could not delete {}: {e}", config.display()),
            }
        }
    }

    tracing::warn!(user = %session.ident.username, slug = %req.slug, purge = req.purge, "removed a container app");
    Json(json!({ "ok": true, "purged": purged })).into_response()
}
