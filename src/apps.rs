//! Installing, running and removing the applications in the catalog.
//!
//! **Two managers, one book.** Almost every entry is a container, and this file
//! creates it, starts it and removes it through `engine.rs`. One is a systemd
//! unit on the host, which `systemd.rs` can only start and stop -- it was
//! installed by the operator and outlives WebDesk. The record written at
//! install time says which, and everything downstream of that record is the
//! same for both: one loopback port, one prefix, one dock icon. See
//! `catalog::HostService` for why the second kind is worth having.
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
//! directory, the shared `/home`, `PUID`/`PGID`, the labels, the restart
//! policy -- is computed here and cannot be reached from the browser. A user
//! picks an app and answers its questions; they do not describe a container.

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

/// The host's own timezone, as an IANA name.
///
/// Read rather than asked. A container whose clock disagrees with the host it
/// runs on timestamps everything wrong, and the correct answer is already on
/// the machine -- so making somebody retype it is only an invitation to typo
/// it. `/etc/localtime` is a symlink into the zoneinfo tree on every target
/// distribution; `timedatectl` is the fallback for a host where it is a copy
/// instead, and `Etc/UTC` the last resort.
pub fn host_timezone() -> String {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let path = target.to_string_lossy();
        if let Some((_, zone)) = path.split_once("/zoneinfo/") {
            if !zone.is_empty() {
                return zone.to_string();
            }
        }
    }
    if let Ok(out) = std::process::Command::new("timedatectl")
        .args(["show", "-p", "Timezone", "--value"])
        .output()
    {
        let zone = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !zone.is_empty() {
            return zone;
        }
    }
    "Etc/UTC".to_string()
}

// ------------------------------------------------------------------- state

/// One installed application, as recorded on disk.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Installed {
    pub slug: String,
    pub image: String,
    /// The loopback port the container publishes on.
    pub port: u16,
    /// Whether that port speaks TLS. Recorded here rather than looked up in the
    /// catalog so the record stays true to the container that actually exists,
    /// even if the entry it came from changes underneath it.
    #[serde(default)]
    pub tls: bool,
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
    /// The port WebDesk itself listens on to serve this app at the root of an
    /// origin, for an entry with `needs_origin`. `None` for everything reached
    /// at `/app/<slug>/`, which is almost everything. See `origin.rs`.
    #[serde(default)]
    pub origin_port: Option<u16>,
    /// The systemd unit serving this app, for one that runs on the host rather
    /// than in a container. `None` -- and absent from every record written
    /// before this existed -- means a container, which is the ordinary case.
    ///
    /// This is the field that decides which of the two managers a start, a stop
    /// or a state enquiry goes to. Copied from the catalog at install time
    /// rather than looked up each time for the same reason `tls` is: the record
    /// should stay true to the thing that actually exists, even if the entry it
    /// came from is edited underneath it.
    #[serde(default)]
    pub unit: Option<String>,
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

/// Every installed app that is served at the root of a port of its own, and
/// which port that is. Read at start by `origin::start_installed`.
pub fn origin_ports() -> Vec<(String, u16)> {
    read_book()
        .apps
        .iter()
        .filter_map(|(slug, a)| a.origin_port.map(|p| (slug.clone(), p)))
        .collect()
}

/// The one lookup the proxy needs: which loopback port serves this slug, and
/// whether that port expects TLS.
pub fn upstream_of(slug: &str) -> Option<(u16, bool)> {
    read_book().apps.get(slug).map(|a| (a.port, a.tls))
}

/// Where this app really lives, for one that has an origin of its own.
///
/// `None` for everything reached under `/app/<slug>/`, which is the ordinary
/// case. The proxy asks before forwarding: an entry with `needs_origin` cannot
/// be served from a prefix at all, so `/app/<slug>/` has to hand the browser
/// the real address rather than relay a page that will never populate.
pub fn origin_url_of(tls_on: bool, headers: &HeaderMap, slug: &str) -> Option<String> {
    let book = read_book();
    let a = book.apps.get(slug)?;
    a.origin_port?;
    Some(app_url(tls_on, headers, a))
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
    /// Set by a `Kind::Port` answer. Not an environment variable and never sent
    /// to the container: the app inside goes on serving the port it always did.
    origin_port: Option<u16>,
}

/// Turn what the browser sent into the exact set of environment variables and
/// mounts this entry allows -- and nothing else. Keys the catalog does not
/// declare are dropped rather than rejected, so a stale UI cannot wedge an
/// install, but they can never reach the engine.
fn validate(app: &catalog::App, given: &BTreeMap<String, String>) -> Result<Answers, String> {
    let mut out =
        Answers { env: BTreeMap::new(), mounts: Vec::new(), secrets: Vec::new(), origin_port: None };

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
            Kind::Port => {
                // The entry has to have declared that it needs one. Without
                // this the flag would be decorative and the parameter alone
                // would open a listener -- so an entry that grew a Port field
                // by a bad merge would quietly start publishing a port. The
                // catalog test keeps the two in step; this is what makes the
                // flag mean something at run time.
                if !app.needs_origin {
                    continue;
                }
                // Unprivileged only: WebDesk drops to an unprivileged child for
                // filesystem work and has no business asking for a reserved
                // port, and an operator who wants one has a proxy for that.
                let n: u16 = value
                    .parse()
                    .map_err(|_| format!("{} must be a number", p.label))?;
                if n < 1024 {
                    return Err(format!("{} must be 1024 or above", p.label));
                }
                out.origin_port = Some(n);
            }
        }
    }
    Ok(out)
}

/// Where the browser should be sent for this app.
///
/// `/app/<slug>/` for almost everything, and a relative path is the right answer
/// there: it works whatever name the desk was reached by. An app on an origin of
/// its own needs an absolute URL, and the only honest source for the host part
/// is the `Host` header of the request asking -- WebDesk is never told its own
/// public name, and guessing one from an interface address would be wrong for
/// every operator who reaches it by a domain.
///
/// The port is replaced, not appended, so the answer is correct whether the desk
/// is on `:61443` or behind something on `:443`.
fn app_url(tls_on: bool, headers: &HeaderMap, a: &Installed) -> String {
    let Some(port) = a.origin_port else { return format!("/app/{}/", a.slug) };
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let bare = host_without_port(host);
    if bare.is_empty() {
        // Nothing to build a URL from. A relative path is wrong for this app,
        // but it is better than a URL pointing at nowhere.
        return format!("/app/{}/", a.slug);
    }
    let scheme = if tls_on { "https" } else { "http" };
    format!("{scheme}://{bare}:{port}/")
}

/// Strip `:port` from a Host header, leaving an IPv6 literal's brackets intact.
///
/// `[::1]:443` and `[::1]` both have colons in the host part, so the port can
/// only be the tail after the closing bracket.
fn host_without_port(host: &str) -> &str {
    match host.rfind(']') {
        Some(close) => &host[..=close],
        None => host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host),
    }
}

/// The port this process listens on, if it can be told from `WD_LISTEN`.
///
/// Only used to refuse an app that would try to take it. `None` when the
/// variable is unparseable, which is not worth failing an install over -- the
/// bind would fail loudly enough on its own.
fn listen_port() -> Option<u16> {
    let listen = std::env::var("WD_LISTEN").unwrap_or_else(|_| format!("0.0.0.0:{}", crate::DEFAULT_PORT));
    listen.rsplit(':').next()?.parse().ok()
}

/// A value for a variable an application needs but nobody should pick: 32 bytes
/// from the system generator, hex-encoded. A human-chosen signing key is
/// strictly worse than this, and one retyped from somewhere else is worse still.
fn fresh_secret() -> String {
    use rand::Rng;
    let mut b = [0u8; 32];
    rand::thread_rng().fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
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

    /// A catalog entry with nothing interesting in it, for tests about a rule
    /// rather than about a particular application.
    fn demo_app() -> catalog::App {
        catalog::App {
            slug: "demo",
            name: "Demo",
            tagline: "",
            image: "example/demo",
            port: 80,
            icon: "a-box",
            host: None,
            base: None,
            needs_origin: false,
            config_at: Some("/config"),
            env: &[],
            generated: &[],
            lsio: true,
            ids: true,
            socket: None,
            shm: None,
            title: None,
            tls: false,
            notes: "",
            params: &[],
        }
    }

    /// An install record with nothing in it, for tests that care about one field.
    fn blank(slug: &str) -> Installed {
        Installed {
            slug: slug.into(),
            image: String::new(),
            port: 0,
            tls: false,
            env: BTreeMap::new(),
            mounts: Vec::new(),
            secrets: Vec::new(),
            installed: 0,
            actor: String::new(),
            origin_port: None,
            unit: None,
        }
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
        // A desktop entry declares no blanks at all, so *nothing* a browser
        // sends should survive -- including the settings the installer itself
        // sets, which are applied after this and must not be forgeable here.
        let got = validate(
            app,
            &answers(&[
                ("TZ", "Europe/London"),
                ("TITLE", "not-firefox"),
                ("PUID", "0"),
                ("PATH", "/evil"),
                ("LD_PRELOAD", "/evil.so"),
                ("SELKIES_ENCODER", "nonsense"),
                ("PASSWORD", "sneaky"),
            ]),
        )
        .unwrap();
        assert!(got.env.is_empty(), "a desktop entry accepted {:?}", got.env.keys());
        assert!(got.mounts.is_empty());
    }

    #[test]
    fn an_unanswered_optional_parameter_is_simply_absent() {
        let app = catalog::find("vscodium-web").unwrap();
        let got = validate(app, &answers(&[])).unwrap();
        assert!(!got.env.contains_key("CONNECTION_TOKEN"));
        assert!(!got.env.contains_key("SUDO_PASSWORD"));
    }

    #[test]
    fn a_desktop_app_installs_without_asking_anything() {
        for slug in ["firefox", "helium", "onlyoffice", "inkscape", "intellij-idea"] {
            let a = catalog::find(slug).unwrap();
            assert_eq!(a.all_params().count(), 0, "{slug} still asks something");
            // Told what to call itself rather than asked.
            assert_eq!(a.title, Some(slug), "{slug}");
        }
    }

    #[test]
    fn the_timezone_comes_off_the_host() {
        let tz = host_timezone();
        assert!(!tz.is_empty());
        // Either a real zone name or the documented last resort -- never a
        // path, and never the empty string that an unset variable would be.
        assert!(!tz.starts_with('/'), "{tz} looks like a path, not a zone");
        assert!(tz == "Etc/UTC" || tz.contains('/'), "{tz} is not an IANA name");
    }

    #[test]
    fn a_missing_required_parameter_stops_the_install() {
        // A subject of its own rather than leaning on a shipping entry, so the
        // rule stays tested whatever the catalog happens to ask for.
        let app = catalog::App {
            params: &[catalog::Param {
                key: "NEEDED",
                label: "Needed",
                help: "",
                kind: catalog::Kind::Text,
                default: "",
                required: true,
            }],
            ..demo_app()
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
        let app = catalog::find("vscodium-web").unwrap();
        let got = validate(app, &answers(&[("CONNECTION_TOKEN", "hunter2")])).unwrap();
        assert!(got.secrets.contains(&"CONNECTION_TOKEN".to_string()));
        assert_eq!(got.env.get("CONNECTION_TOKEN").map(String::as_str), Some("hunter2"));
    }

    #[test]
    fn an_app_that_must_be_told_its_prefix_is_told_it_correctly() {
        // Exactly one entry needs telling: VSCodium roots its assets at
        // /stable-<hash>/ otherwise.
        let vsc = catalog::find("vscodium-web").unwrap();
        assert_eq!(
            vsc.base_value("/app/vscodium-web"),
            Some(("CODE_ARGS", "--server-base-path=/app/vscodium-web".to_string()))
        );
        // The Selkies desktops work it out from the page URL and are told nothing.
        assert_eq!(catalog::find("firefox").unwrap().base_value("/app/firefox"), None);
    }

    #[test]
    fn the_terminal_is_never_told_a_prefix_the_proxy_has_already_stripped() {
        // Regression: HUT_BASE_PATH made term.hut route on /app/term-hut, but
        // `forward` strips that prefix, so every real request arrived as `/`
        // and 404'd -- a tile that opened on a blank frame. Its assets are all
        // relative, so it needs no telling.
        let hut = catalog::find("term-hut").unwrap();
        assert_eq!(hut.base_value("/app/term-hut"), None);
        let got = validate(hut, &answers(&[])).unwrap();
        assert!(
            !got.env.contains_key("HUT_BASE_PATH"),
            "a base path reached term.hut after all: {:?}",
            got.env.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_non_linuxserver_image_is_not_offered_linuxserver_settings() {
        let hut = catalog::find("term-hut").unwrap();
        assert!(!hut.lsio);
        // TZ is a LinuxServer convention; term.hut would ignore it.
        assert!(!hut.all_params().any(|p| p.key == "TZ"));
        assert_eq!(hut.config_at, Some("/home/hut"));
    }

    #[test]
    fn the_terminal_asks_for_no_second_token_of_its_own() {
        // Answering nothing is the common case -- Install is one press -- and
        // it must produce a terminal that opens rather than one that asks for
        // a token minted into a container log the user cannot see. Reaching it
        // already means getting past WebDesk's session.
        let hut = catalog::find("term-hut").unwrap();
        let got = validate(hut, &answers(&[])).unwrap();
        assert_eq!(got.env.get("HUT_NO_TOKEN").map(String::as_str), Some("true"));
        // Still a choice, not a removal: unticking it puts the token back.
        let on = validate(hut, &answers(&[("HUT_NO_TOKEN", "false")])).unwrap();
        assert_eq!(on.env.get("HUT_NO_TOKEN").map(String::as_str), Some("false"));
    }

    /// The host entry is the same application as the container one and takes
    /// the same lesson: term.hut *routes* on a base path and the proxy strips
    /// `/app/<slug>` before forwarding, so being told one is a guaranteed 404.
    /// Running on the host changes nothing about that -- it is a property of
    /// how the app reads the flag, not of where it runs.
    #[test]
    fn the_terminal_on_the_host_is_told_no_prefix_either() {
        let hut = catalog::find("term-hut-host").unwrap();
        assert_eq!(hut.base_value("/app/term-hut-host"), None);
    }

    /// A host entry is described by what it is *not*: no image, no state
    /// directory, no shared memory, no engine socket, no identity or clock
    /// variables and no questions. Every one of those is something the
    /// installer would do to a container and there is no container here -- and
    /// a field left set by a bad merge would have the installer quietly try.
    #[test]
    fn a_host_entry_carries_nothing_a_container_would_need() {
        for a in catalog::CATALOG.iter().filter(|a| a.host.is_some()) {
            assert!(a.image.is_empty(), "{} is a host service with an image", a.slug);
            assert!(a.config_at.is_none(), "{} would be given a mount", a.slug);
            assert!(a.socket.is_none(), "{} would be given the engine socket", a.slug);
            assert!(a.shm.is_none(), "{}'s shm size would go nowhere", a.slug);
            assert!(!a.lsio && !a.ids, "{} would be sent settings nothing reads", a.slug);
            assert!(a.env.is_empty(), "{}'s environment would go nowhere", a.slug);
            assert!(a.generated.is_empty(), "{} would generate a key for nobody", a.slug);
            assert!(a.title.is_none(), "{} cannot be told what to call itself", a.slug);
            assert!(!a.needs_origin, "{} would ask for a port it does not choose", a.slug);
            // Every answer would have to reach the process through its unit
            // file, which WebDesk does not write. A form here could only
            // collect settings and then drop them.
            assert_eq!(a.all_params().count(), 0, "{} asks a question it cannot apply", a.slug);
            assert!(!a.host.as_ref().unwrap().unit.is_empty(), "{} names no unit", a.slug);
        }
    }

    /// The two allocators must never meet. A container's port is handed out of
    /// `PORT_LOW..=PORT_HIGH` by `free_port`, which knows only about the book;
    /// a host service was already listening on a port fixed in the catalog and
    /// written into somebody's unit file. If the ranges overlapped, installing
    /// a container could be handed the port a host service is on -- and the
    /// engine's bind would fail with a message about an address in use, which
    /// names neither app.
    #[test]
    fn a_host_service_port_is_out_of_reach_of_the_container_allocator() {
        for a in catalog::CATALOG.iter().filter(|a| a.host.is_some()) {
            assert!(
                !(PORT_LOW..=PORT_HIGH).contains(&a.port),
                "{} sits on {} inside the range free_port hands out",
                a.slug,
                a.port
            );
        }
    }

    /// Two host entries on one port would have whichever was installed second
    /// served by the first one's application, under its own name and icon.
    /// Nothing downstream could notice: the proxy is given a port and dials it.
    #[test]
    fn no_two_host_entries_claim_the_same_port() {
        let mut seen = std::collections::HashMap::new();
        for a in catalog::CATALOG.iter().filter(|a| a.host.is_some()) {
            if let Some(other) = seen.insert(a.port, a.slug) {
                panic!("{} and {} both expect port {}", other, a.slug, a.port);
            }
        }
    }

    #[test]
    fn the_desktop_apps_get_enough_shared_memory() {
        // A browser or IDE on the 64 MB default dies in ways that look like the
        // app being broken rather than the container being starved.
        for slug in ["firefox", "helium", "onlyoffice", "inkscape", "intellij-idea"] {
            let a = catalog::find(slug).unwrap_or_else(|| panic!("{slug} missing"));
            assert_eq!(a.shm, Some("1g"), "{slug}");
            // The https port, which means the proxy must speak TLS to it.
            assert_eq!(a.port, 3001, "{slug}");
            assert!(a.tls, "{slug} is on 3001 but not marked tls");
        }
    }

    #[test]
    fn every_icon_the_catalog_names_is_in_the_sprite() {
        // A renamed or mistyped id is invisible until someone opens the Apps
        // window and finds a blank square, so it is checked at build time
        // against the file that actually ships.
        const SPRITE: &str = include_str!("../ui/ui-icons.svg");
        for a in catalog::CATALOG {
            assert!(
                SPRITE.contains(&format!("id=\"{}\"", a.icon)),
                "{} names icon {}, which is not in ui/ui-icons.svg",
                a.slug,
                a.icon
            );
        }
        // The fallback the UI draws for an app whose entry has gone away.
        assert!(SPRITE.contains("id=\"a-box\""));
    }

    #[test]
    fn only_the_desktop_apps_expect_tls() {
        // A mismatch either way is a connection that fails in a way that looks
        // like the container being down: plaintext into a TLS port, or a
        // handshake against something that speaks none.
        for a in catalog::CATALOG {
            assert_eq!(a.tls, a.port == 3001, "{} disagrees about its scheme", a.slug);
        }
    }

    #[test]
    fn ports_are_handed_out_without_collision() {
        let mut book = Book::default();
        let first = free_port(&book).unwrap();
        book.apps.insert(
            "a".into(),
            Installed { port: first, ..blank("a") },
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
            // Only a host service may name no image. For anything else an
            // empty one would reach the engine as a `pull ":latest"`.
            assert_eq!(
                a.image.is_empty(),
                a.host.is_some(),
                "{} is a container with no image, or a host service with one",
                a.slug
            );
            assert!(a.port > 0, "{} has no port", a.slug);
            if let Some(at) = a.config_at {
                assert!(at.starts_with('/'), "{}'s state directory is not absolute", a.slug);
            }
            // The state directory is mounted by the installer; an entry that
            // also claimed it would produce two mounts at the same place.
            for p in a.params {
                if let Kind::HostPath { at, .. } = p.kind {
                    assert_ne!(Some(at), a.config_at, "{} must not mount its own state dir", a.slug);
                }
            }
            // A template that never substitutes would silently hand the app an
            // empty prefix and look like the app ignoring it.
            if let Some(b) = &a.base {
                assert!(b.template.contains("{prefix}"), "{} has a base with no {{prefix}}", a.slug);
            }
            // Every LinuxServer image reads the ids as well as the clock. The
            // reverse does not hold, which is why they are separate fields.
            if a.lsio {
                assert!(a.ids, "{} follows the lsio contract but is not sent PUID/PGID", a.slug);
            }
        }
    }

    #[test]
    fn only_the_engine_manager_is_given_the_engine_socket() {
        // The socket is root on this host in one bind: a process that reaches
        // the engine can start a container that mounts /. Exactly one entry may
        // have it, and it must be the one whose whole purpose is the engine --
        // so a second entry quietly acquiring it fails here rather than in
        // somebody's install.
        let with: Vec<&str> =
            catalog::CATALOG.iter().filter(|a| a.socket.is_some()).map(|a| a.slug).collect();
        assert_eq!(with, vec!["dockhand"], "the engine socket escaped its one entry");
        assert_eq!(catalog::find("dockhand").unwrap().socket, Some("/var/run/docker.sock"));
    }

    #[test]
    fn the_engine_manager_is_given_an_origin_rather_than_a_prefix() {
        // It cannot be told a prefix, because there is nothing in it that would
        // read one: `/api/...` is compiled into its client. So it gets a port
        // of its own instead, and is told no prefix at all.
        let dh = catalog::find("dockhand").unwrap();
        assert!(dh.needs_origin);
        assert_eq!(dh.base_value("/app/dockhand"), None);
        // It reads the ids without being a LinuxServer image; the clock it has
        // no opinion about, so it is not sent one.
        assert!(dh.ids && !dh.lsio);
    }

    #[test]
    fn an_app_that_needs_an_origin_asks_which_port_and_nothing_else_does() {
        // The two halves are useless apart. Without the question there is no
        // port to listen on, and only the operator knows which one is free and
        // allowed through their firewall -- WebDesk picking one would be a
        // guess about a machine it cannot see. And a Port answer on an entry
        // that is served under a prefix would be silently ignored.
        for a in catalog::CATALOG {
            let asks = a.params.iter().any(|p| matches!(p.kind, Kind::Port));
            assert_eq!(
                asks, a.needs_origin,
                "{}: needs_origin={} but asks for a port={}",
                a.slug, a.needs_origin, asks
            );
        }
    }

    #[test]
    fn a_port_is_ignored_on_an_entry_that_did_not_ask_for_an_origin() {
        // No shipping entry disagrees -- a test upstairs enforces that -- so
        // this needs a subject of its own rather than going untested until one
        // does. The failure it guards is an entry that grows a Port field
        // without the flag and starts opening a port nobody decided on.
        let app = catalog::App {
            needs_origin: false,
            params: &[catalog::Param {
                key: "WD_ORIGIN_PORT",
                label: "Port",
                help: "",
                kind: catalog::Kind::Port,
                default: "61444",
                required: false,
            }],
            ..demo_app()
        };
        let got = validate(&app, &answers(&[("WD_ORIGIN_PORT", "61444")])).unwrap();
        assert_eq!(got.origin_port, None, "a port was taken from an entry that never asked");
    }

    #[test]
    fn a_port_answer_is_a_number_above_the_reserved_range() {
        let app = catalog::find("dockhand").unwrap();
        // The default is offered because a form with an empty required field
        // is a worse first run than one with a sensible number in it.
        let got = validate(app, &answers(&[])).unwrap();
        assert_eq!(got.origin_port, Some(61444));
        // Never an environment variable: the container is not listening here.
        assert!(got.env.is_empty(), "the port reached the container");

        assert_eq!(validate(app, &answers(&[("WD_ORIGIN_PORT", "8443")])).unwrap().origin_port, Some(8443));
        for bad in ["80", "443", "1023", "0"] {
            assert!(
                validate(app, &answers(&[("WD_ORIGIN_PORT", bad)])).is_err(),
                "{bad} was accepted"
            );
        }
        assert!(validate(app, &answers(&[("WD_ORIGIN_PORT", "not-a-port")])).is_err());
        assert!(validate(app, &answers(&[("WD_ORIGIN_PORT", "70000")])).is_err());
    }

    #[test]
    fn an_origin_app_is_opened_at_the_host_the_browser_used() {
        // The whole portability argument in one assertion: WebDesk never knows
        // its own public name, so the URL is built from the Host header of the
        // request asking. Whatever address reaches the desk reaches the app,
        // which is why an existing certificate keeps working -- it does not
        // name a port.
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::HOST, "desk.example.net:61443".parse().unwrap());
        let rec = Installed { origin_port: Some(61444), ..blank("dockhand") };

        assert_eq!(app_url(true, &h, &rec), "https://desk.example.net:61444/");

        // The desk's own port is replaced, not appended, so being behind
        // something on :443 does not produce desk.example.net:443:61444.
        h.insert(axum::http::header::HOST, "desk.example.net".parse().unwrap());
        assert_eq!(app_url(true, &h, &rec), "https://desk.example.net:61444/");

        // An IPv6 literal keeps its brackets: the colons in it are not a port.
        h.insert(axum::http::header::HOST, "[::1]:61443".parse().unwrap());
        assert_eq!(app_url(true, &h, &rec), "https://[::1]:61444/");

        // Plaintext when this process is not the one terminating TLS.
        assert_eq!(app_url(false, &h, &rec), "http://[::1]:61444/");

        // Everything else stays relative, which is what makes it work under
        // any name at all.
        let plain = Installed { origin_port: None, ..blank("firefox") };
        assert_eq!(app_url(true, &h, &plain), "/app/firefox/");
    }

    #[test]
    fn a_generated_key_is_never_also_a_question() {
        // The two would fight: `validate` fills the parameter from the browser
        // and the installer then overwrites it, so the answer would vanish
        // without a word. The same holds for the fixed settings.
        for a in catalog::CATALOG {
            for key in a.generated {
                assert!(
                    !a.params.iter().any(|p| p.key == *key),
                    "{} both asks for and generates {key}",
                    a.slug
                );
            }
            for (key, _) in a.env {
                assert!(
                    !a.params.iter().any(|p| p.key == *key),
                    "{} both asks for and fixes {key}",
                    a.slug
                );
            }
        }
    }

    #[test]
    fn the_signing_key_is_random_and_not_a_constant() {
        // A fixed default would be worse than no key at all: every install on
        // every host would sign its cookies with the same secret.
        let (a, b) = (fresh_secret(), fresh_secret());
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
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
            // Whichever manager owns this one. A host app has no container to
            // inspect, and asking the engine about it would report `missing`
            // for a service that is running perfectly well.
            let status = match (&a.unit, engine) {
                (Some(unit), _) => crate::systemd::state(unit),
                (None, Some(e)) => engine::state(e, &a.slug),
                (None, None) => "unknown".to_string(),
            };
            json!({
                "slug": a.slug,
                "name": entry.map(|c| c.name).unwrap_or(&a.slug),
                "tagline": entry.map(|c| c.tagline).unwrap_or(""),
                "icon": entry.map(|c| c.icon).unwrap_or("a-box"),
                "notes": entry.map(|c| c.notes).unwrap_or(""),
                "image": a.image,
                // Empty for a host app, which has no image; the unit is what
                // the Apps window names in its place.
                "unit": a.unit,
                "state": status,
                "url": app_url(state.tls_on(), &headers, a),
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
    // A service on the host takes none of the rest of this: there is no engine
    // in the story, no image to pull, and nothing to create.
    if let Some(host) = &app.host {
        return install_host(app, host, &session.ident.username);
    }
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
    // container will run as. An entry that keeps no state gets no directory,
    // rather than an empty one that nothing will ever write to.
    let (uid, gid) = (session.ident.uid, session.ident.gid);
    let config = match app.config_at {
        Some(_) => {
            let dir = appdata_dir().join(&req.slug);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return bad(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("could not create {}: {e}", dir.display()),
                );
            }
            let _ = nix::unistd::chown(
                &dir,
                Some(nix::unistd::Uid::from_raw(uid)),
                Some(nix::unistd::Gid::from_raw(gid)),
            );
            Some(dir)
        }
        None => None,
    };

    // A port two apps both claim would leave one of them silently unserved:
    // the second bind fails, and nothing in the UI would say why. Refuse it
    // here, where there is somebody to tell.
    if let Some(want) = answers.origin_port {
        if let Some((other, _)) =
            book.apps.iter().find(|(_, a)| a.origin_port == Some(want))
        {
            return bad(
                StatusCode::CONFLICT,
                format!("port {want} is already serving {other}"),
            );
        }
        // The desk's own listener is the one collision the operator cannot see
        // in the app list at all.
        if listen_port() == Some(want) {
            return bad(
                StatusCode::CONFLICT,
                format!("port {want} is the one WebDesk itself listens on"),
            );
        }
    }

    let mut env = answers.env.clone();
    // Fixed by us, not offered: these are what make the container's files
    // readable by the person who installed it. Only sent to images that read
    // them -- term.hut runs as its own fixed user and would ignore them, so
    // sending them would just be noise in `docker inspect`.
    if app.ids {
        env.insert("PUID".into(), uid.to_string());
        env.insert("PGID".into(), gid.to_string());
    }
    // Asked separately because they are two different questions -- an image
    // may well read the ids and have no opinion about the clock. See `ids`.
    if app.lsio {
        // The host's clock, not a blank on a form. See `host_timezone`.
        env.insert(catalog::TZ_KEY.into(), host_timezone());
    }
    if let Some(title) = app.title {
        env.insert("TITLE".into(), title.to_string());
    }
    if let Some((key, value)) = app.base_value(&format!("/app/{}", app.slug)) {
        env.insert(key.to_string(), value);
    }
    // Entry-specific settings with one right answer. Applied after the answers
    // so that a parameter can never quietly redefine one.
    for (key, value) in app.env {
        env.insert((*key).to_string(), (*value).to_string());
    }
    // The keys nobody should choose or retype. Generated here, once, and
    // recorded as secrets so they are never echoed back to the browser.
    let mut secrets = answers.secrets.clone();
    for key in app.generated {
        env.insert((*key).to_string(), fresh_secret());
        secrets.push((*key).to_string());
    }

    let mut mounts = answers.mounts.clone();
    // Ordered parent-first for anyone reading `docker inspect`; the engine
    // sorts by depth regardless, so an app whose state directory sits *inside*
    // the shared home -- term.hut's `/home/hut` -- still gets its own state
    // there rather than the host's copy.
    if let Some(home) = engine::home_mount() {
        mounts.push(home);
    }
    if let (Some(dir), Some(at)) = (&config, app.config_at) {
        mounts.push((dir.to_string_lossy().to_string(), at.to_string(), false));
    }
    // The engine socket, for the one entry that manages the engine. Read-write
    // because the whole point is to act, and `ro` on a socket buys nothing
    // anyway -- the writes that matter are the ones sent *through* it, not to
    // the inode. Taken from the catalog rather than from anything the browser
    // sent, so no request can ask for it.
    if let Some(sock) = app.socket {
        if std::path::Path::new(sock).exists() {
            mounts.push((sock.to_string(), sock.to_string(), false));
        } else {
            return bad(
                StatusCode::CONFLICT,
                format!("{} needs the engine socket at {sock}, which is not there", app.name),
            );
        }
    }

    let image = format!("{}:{}", app.image, tag);
    let record = Installed {
        slug: app.slug.to_string(),
        image: image.clone(),
        port,
        tls: app.tls,
        env: env.clone(),
        mounts: mounts.clone(),
        secrets,
        installed: now(),
        actor: session.ident.username.clone(),
        origin_port: answers.origin_port,
        // A container. `install_host` is the only place that writes a unit.
        unit: None,
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
    let origin_port = answers.origin_port;
    let origin_state = state.clone();
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
                // Only now, and only if the entry asked for one: a listener
                // in front of a container that failed to be created would
                // answer 502 to anyone who found it.
                if let Some(p) = origin_port {
                    let st = origin_state.clone();
                    let sl = slug.clone();
                    tokio::runtime::Handle::current().spawn(async move {
                        if let Err(e) = crate::origin::start(&st, &sl, p).await {
                            tracing::error!(slug = %sl, port = p, "could not open its port: {e}");
                        }
                    });
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

/// Adopt a service the operator has already put on this host.
///
/// Almost the whole of an install is missing here, and that absence is the
/// feature: nothing is pulled, nothing is created, no port is allocated, no
/// state directory is made and no environment is composed, because all of it
/// happened in a unit file before WebDesk was involved. What is left is a
/// record saying which loopback port serves this slug -- which is the only
/// thing `upstream_of` ever asks, and so the only thing the proxy needs.
///
/// Synchronous, unlike the container path, because there is nothing slow to do.
/// It writes the same status file regardless: the browser polls for it either
/// way, and an install that returned without reporting would spin forever.
fn install_host(app: &catalog::App, host: &catalog::HostService, actor: &str) -> Response {
    let book = read_book();
    if book.apps.contains_key(app.slug) {
        return bad(StatusCode::CONFLICT, format!("{} is already installed", app.name));
    }
    // Two apps on one loopback port would leave whichever lost the race being
    // served somebody else's application under its own name. The catalog test
    // keeps the shipping entries apart; this catches the pair that only exist
    // together on a particular host.
    if let Some((other, _)) = book.apps.iter().find(|(_, a)| a.port == app.port) {
        return bad(
            StatusCode::CONFLICT,
            format!("port {} is already serving {other}", app.port),
        );
    }
    if !crate::systemd::available() {
        return bad(
            StatusCode::CONFLICT,
            format!("{} runs as a service on the host, and this host has no systemd", app.name),
        );
    }
    // The check that turns a blank frame into a sentence. Without it the entry
    // would install cleanly against a service that is not there, and the only
    // symptom would be a 502 from the dock with nothing anywhere saying why.
    if !crate::systemd::known(host.unit) {
        return bad(
            StatusCode::CONFLICT,
            format!("this host has no {}. {}", host.unit, host.provision),
        );
    }
    if read_status()["state"] == "running" {
        return bad(StatusCode::CONFLICT, "another install is already running");
    }

    let record = Installed {
        slug: app.slug.to_string(),
        image: String::new(),
        port: app.port,
        tls: app.tls,
        env: BTreeMap::new(),
        mounts: Vec::new(),
        secrets: Vec::new(),
        installed: now(),
        actor: actor.to_string(),
        origin_port: None,
        unit: Some(host.unit.to_string()),
    };

    let mut book = book;
    book.apps.insert(app.slug.to_string(), record);
    if let Err(e) = write_book(&book) {
        return bad(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not record the app: {e}"),
        );
    }

    tracing::warn!(user = %actor, slug = %app.slug, unit = %host.unit, "adopted a host service");
    let _ = write_status(&json!({
        "state": "done", "phase": "installed", "slug": app.slug,
        "name": app.name, "finished": now(), "actor": actor,
    }));
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
    act(&state, &headers, &req.slug, "start").await
}

pub async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SlugReq>,
) -> Response {
    act(&state, &headers, &req.slug, "stop").await
}

/// Start or stop one installed app, whichever manager owns it.
///
/// The record decides, not the catalog: an app that was adopted as a service is
/// started by systemd and one that was created as a container by the engine,
/// and the unit name comes from what was written at install time. The two paths
/// differ only in which program is run -- everything around them, the
/// authorisation and the reporting, is the same because it is the same act.
async fn act(state: &AppState, headers: &HeaderMap, slug: &str, what: &str) -> Response {
    let session = match admin_session(state, headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(record) = read_book().apps.get(slug).cloned() else {
        return bad(StatusCode::NOT_FOUND, format!("{slug} is not installed"));
    };

    tracing::info!(user = %session.ident.username, slug, "app {what}");
    let done = match record.unit {
        Some(unit) => {
            let f = if what == "start" { crate::systemd::start } else { crate::systemd::stop };
            tokio::task::spawn_blocking(move || f(&unit)).await
        }
        None => {
            let Some(eng) = engine::detect() else {
                return bad(StatusCode::CONFLICT, "no container engine found on this host");
            };
            let f: fn(Engine, &str) -> Result<(), String> =
                if what == "start" { engine::start } else { engine::stop };
            let slug = slug.to_string();
            tokio::task::spawn_blocking(move || f(eng, &slug)).await
        }
    };
    match done {
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
    let Some(record) = book.apps.get(&req.slug).cloned() else {
        return bad(StatusCode::NOT_FOUND, format!("{} is not installed", req.slug));
    };

    // A host service is forgotten, not deleted. WebDesk did not install the
    // software and did not write the unit, so removing the entry means it stops
    // being served here -- and stopping the service as well would be reaching
    // past what was ever handed over. The unit is left exactly as it was, still
    // running if it was running, and can be adopted again.
    if record.unit.is_none() {
        let Some(eng) = engine::detect() else {
            return bad(StatusCode::CONFLICT, "no container engine found on this host");
        };
        let slug = req.slug.clone();
        let removed = tokio::task::spawn_blocking(move || engine::remove(eng, &slug)).await;
        if let Ok(Err(e)) = removed {
            // Reported, not fatal: the container may already be gone, and
            // refusing to forget it would leave an app that can never be
            // uninstalled.
            tracing::warn!(slug = %req.slug, "could not remove the container: {e}");
        }
    }

    // Before the record goes: a listener left running would keep a port open
    // and answer 502 on it for as long as this process lives.
    crate::origin::stop(&state, &req.slug);

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

    let kind = if record.unit.is_some() { "host service" } else { "container app" };
    tracing::warn!(user = %session.ident.username, slug = %req.slug, purge = req.purge, "removed a {kind}");
    Json(json!({ "ok": true, "purged": purged })).into_response()
}
