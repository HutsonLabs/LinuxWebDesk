//! `cockpit-bridge`, used as a library rather than as a web console.
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
//!
//! **Not having the bridge is an answer, not a failure.** `cockpit-bridge` is
//! no dependency of WebDesk and most hosts will not have it, so its absence is
//! an ordinary state of the machine -- the same argument `systemd::state` makes
//! for `absent`. Every handler below refuses an absent bridge with `503` and
//! `"reason": "not-installed"`, naming the package that would fix it, and keeps
//! `502` and `"reason": "bridge-failed"` for a bridge that is installed and did
//! not work. Those two must never be confused: they are what the dependency
//! installer reads to decide whether to offer the install, and offering to
//! install a package that is already there is how a button becomes a loop.

use crate::auth::Identity;
use crate::catalog::Prereq;
use crate::{session_of, unauthorized, AppState};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What provides `cockpit-bridge` on each host package manager WebDesk knows.
///
/// Public because this is the one fact about the package that belongs next to
/// the code that needs it: `deps::RUNTIME` should point at this value rather
/// than carry a second copy of it that can drift.
///
/// Arch is the honest wart. There is no split-out bridge package there -- the
/// whole of Cockpit is in `cockpit`, including the web server this module was
/// written to avoid. Naming it anyway is better than reporting that WebDesk
/// does not know, because it is true and an operator can decide about it.
pub const BRIDGE_PREREQ: Prereq = Prereq {
    bin: "cockpit-bridge",
    dnf: Some("cockpit-bridge"),
    apt: Some("cockpit-bridge"),
    pacman: Some("cockpit"),
    zypper: Some("cockpit-bridge"),
};

/// Where to look for the bridge beyond `PATH`.
///
/// `PATH` alone would do on every distribution that ships it today, but the
/// daemon's `PATH` is systemd's rather than a login shell's, and Cockpit has
/// moved its own binaries between `/usr/bin` and `/usr/libexec` before. Two
/// extra `stat` calls are cheaper than a panel that reports "not installed" on
/// a host where the package is installed.
const BRIDGE_PATHS: &[&str] =
    &["/usr/bin/cockpit-bridge", "/usr/local/bin/cockpit-bridge", "/usr/libexec/cockpit-bridge"];

/// The one protocol version this speaks.
///
/// Cockpit has never shipped another, which is exactly why the check is worth
/// writing down: a client that ignores the version field will one day decode a
/// protocol it does not know using the parser for the one it does, and produce
/// wrong answers rather than an error. See [`check_init`].
const PROTOCOL_VERSION: i64 = 1;

/// Refuse a frame larger than this rather than allocate for it. Without a cap,
/// a peer speaking the wrong protocol gets to name the size of our heap with
/// four digits and a newline.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// A length prefix is a handful of decimal digits. More bytes than this with no
/// newline in them are not a length prefix, and failing immediately is far
/// better than buffering to `MAX_FRAME` before finding out.
const MAX_LENGTH_LINE: usize = 16;

/// How long to wait for the bridge's `init` before giving up on it.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long one operation may take. Generous, because a journal read on a busy
/// unit is genuinely slow; finite, because a handler blocked on a pipe is a
/// thread that never comes back.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Metrics get their own, shorter deadline: the sample is paced by the
/// bridge's own interval and two of them is the whole of the wait, so anything
/// approaching `CALL_TIMEOUT` here means the sampler is not running at all.
const METRICS_TIMEOUT: Duration = Duration::from_secs(8);

/// A bridge nobody has used for this long is closed. See [`BRIDGES`] for why
/// that has to be a timer rather than the session dropping it.
const IDLE_MAX: Duration = Duration::from_secs(30 * 60);

/// How much of the bridge's stderr to keep for a diagnostic. Enough for the
/// two lines that explain a failed spawn, not enough to grow without bound on a
/// bridge that has decided to complain once a second.
const STDERR_TAIL: usize = 4096;

const SYSTEMD_BUS_NAME: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// Default and ceiling for `?lines=`. The ceiling is not about the bridge; it
/// is about the browser, which is handed the result as one JSON array.
const JOURNAL_DEFAULT: usize = 200;
const JOURNAL_MAX: usize = 5000;

// --------------------------------------------------------------- the framing
//
// A frame is a decimal byte count, a newline, then that many bytes of payload,
// whose own first line is the channel id. The count covers the channel line and
// its newline as well as the data -- the detail that is easy to get wrong in
// both directions, since a decoder that excludes it desynchronises after the
// first frame and an encoder that excludes it writes frames the bridge reads as
// truncated.

/// One decoded frame: the channel it arrived on, and its bytes. The control
/// channel is the one with an empty id, and it is the only channel whose
/// contents this module ever reads as commands.
#[derive(Debug, PartialEq)]
struct Frame {
    channel: String,
    data: Vec<u8>,
}

impl Frame {
    fn is_control(&self) -> bool {
        self.channel.is_empty()
    }
}

fn encode(channel: &str, data: &[u8]) -> Vec<u8> {
    let len = channel.len() + 1 + data.len();
    let mut out = Vec::with_capacity(len + MAX_LENGTH_LINE);
    out.extend_from_slice(len.to_string().as_bytes());
    out.push(b'\n');
    out.extend_from_slice(channel.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(data);
    out
}

/// The read half of the framing, kept apart from the process so it can be
/// tested without one.
///
/// A state machine rather than a read-then-parse function, for one reason: a
/// length-prefixed protocol arriving down a pipe has no relationship between
/// reads and frames. A decoder that assumes one read is one frame works
/// perfectly on a developer's machine, where every message is small and
/// unhurried, and then corrupts the stream the first time the bridge writes a
/// long journal or two replies land inside one 8 KiB read.
#[derive(Default)]
struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, or `None` when more bytes are needed.
    ///
    /// `Err` is deliberately terminal. Once a length prefix has failed to parse
    /// there is no way to know where the next frame begins, so failing the
    /// connection is the only honest move -- resynchronising would mean
    /// guessing, and a wrong guess turns a broken stream into wrong data.
    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        let scan = self.buf.len().min(MAX_LENGTH_LINE);
        let Some(nl) = self.buf[..scan].iter().position(|b| *b == b'\n') else {
            if self.buf.len() >= MAX_LENGTH_LINE {
                // Nearly always a login profile that printed something. `su -`
                // runs a login shell, so an `echo` in /etc/profile lands in
                // front of the bridge's first frame, and nothing in the
                // protocol distinguishes that from a corrupt stream.
                return Err(format!(
                    "the bridge's output does not start with a frame length -- something wrote \
                     {:?} to stdout in front of it, most likely a login profile that prints on \
                     sign-in",
                    String::from_utf8_lossy(&self.buf[..scan])
                ));
            }
            return Ok(None);
        };
        let text = std::str::from_utf8(&self.buf[..nl])
            .map_err(|_| "the frame length is not text".to_string())?;
        let len: usize = text.parse().map_err(|_| format!("{text:?} is not a frame length"))?;
        if len > MAX_FRAME {
            return Err(format!(
                "the bridge announced a {len} byte frame; the limit here is {MAX_FRAME}"
            ));
        }
        let end = nl + 1 + len;
        if self.buf.len() < end {
            return Ok(None);
        }
        let payload = &self.buf[nl + 1..end];
        // The channel id is the payload's first line. A control frame has an
        // empty one, so this newline is there even with nothing before it.
        let Some(cnl) = payload.iter().position(|b| *b == b'\n') else {
            return Err("a frame arrived with no channel line".into());
        };
        let frame = Frame {
            channel: String::from_utf8_lossy(&payload[..cnl]).into_owned(),
            data: payload[cnl + 1..].to_vec(),
        };
        self.buf.drain(..end);
        Ok(Some(frame))
    }
}

/// Check the `init` the bridge opens with, before anything else is sent.
///
/// Two very different failures hide in this one frame. A bridge that refuses
/// the session -- a locked account, PAM saying no -- says so with `problem` and
/// no version, and that word is the only description of the refusal there will
/// ever be, so it goes straight into the message. A bridge announcing a version
/// this does not speak is a *working* bridge whose frames would be misread, and
/// carrying on would yield nonsense rather than an error.
fn check_init(v: &Value) -> Result<(), String> {
    if v["command"].as_str() != Some("init") {
        return Err(format!(
            "the bridge opened with {:?} rather than init",
            v["command"].as_str().unwrap_or("nothing")
        ));
    }
    if let Some(problem) = v["problem"].as_str() {
        return Err(format!("the bridge refused the session: {problem}"));
    }
    match v["version"].as_i64() {
        Some(PROTOCOL_VERSION) => Ok(()),
        Some(other) => Err(format!(
            "this cockpit-bridge speaks protocol version {other}; WebDesk speaks {PROTOCOL_VERSION}"
        )),
        None => Err("the bridge's init named no protocol version".into()),
    }
}

/// Our half of the handshake.
///
/// `host` is required, and it is always this machine. The bridge can be asked
/// to proxy to another host, and this field is the one place where "which
/// machine am I managing" could have become something a request decides.
fn our_init() -> Value {
    json!({ "command": "init", "version": PROTOCOL_VERSION, "host": "localhost" })
}

// --------------------------------------------------------------- the process

/// A live `cockpit-bridge` for one session.
pub struct Bridge {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    /// Decoded frames from the reader thread. Blocking reads live on their own
    /// thread exactly as they do in `pty.rs`, and that is also what gives every
    /// read here a deadline: a pipe has no read timeout of its own, and a
    /// handler blocked on one never returns.
    frames: std::sync::mpsc::Receiver<Result<Frame, String>>,
    stderr: Arc<Mutex<String>>,
    next_channel: u64,
}

fn find_bridge() -> Option<PathBuf> {
    if let Some(p) = crate::engine::which(BRIDGE_PREREQ.bin) {
        return Some(p);
    }
    BRIDGE_PATHS.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Whether this host has a bridge to open at all.
pub fn available() -> bool {
    find_bridge().is_some()
}

impl Bridge {
    /// Spawn the bridge as this identity. `Err` when the package is absent --
    /// which is not fatal to WebDesk, only to the panels that need it.
    pub fn open(ident: &Identity) -> Result<Bridge, String> {
        let Some(bridge) = find_bridge() else {
            return Err(format!("{} is not installed on this host", BRIDGE_PREREQ.bin));
        };
        let Some(su) =
            ["/bin/su", "/usr/bin/su"].into_iter().find(|p| std::path::Path::new(p).exists())
        else {
            return Err("su not found on this system".into());
        };

        // `su - <user> -c <bridge>`, for the reasons pty.rs gives for the
        // shell: su is setuid-aware, runs its own PAM session, sets up the
        // environment and lands in the user's home, and the daemon is root so
        // nothing asks for a password. The PAM session matters more here than
        // it does for a terminal -- it is what gives the bridge an
        // XDG_RUNTIME_DIR and a session bus that belong to this user.
        //
        // The username is from the authenticated identity and is its own argv
        // element; the command is the absolute path resolved a few lines above.
        // Nothing from a request reaches this call, which is why having a login
        // shell in the middle of it is not a hole -- there is no untrusted
        // string for that shell to re-read.
        let mut cmd = std::process::Command::new(su);
        cmd.arg("-")
            .arg(&ident.username)
            .arg("-c")
            .arg(&bridge)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child =
            cmd.spawn().map_err(|e| format!("could not start {}: {e}", bridge.display()))?;
        let stdin = child.stdin.take().ok_or("the bridge has no stdin")?;
        let stdout = child.stdout.take().ok_or("the bridge has no stdout")?;
        let stderr = child.stderr.take().ok_or("the bridge has no stderr")?;

        // stderr has to be drained by somebody. Piped and never read, it fills,
        // and the bridge then blocks writing a log line -- which presents as a
        // bridge that answered twice and then hung. It is kept rather than sent
        // to /dev/null because the difference between "installed but failed"
        // and a sentence somebody can act on is usually one line the bridge has
        // already written.
        let tail = Arc::new(Mutex::new(String::new()));
        {
            let tail = tail.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    if let Ok(mut s) = tail.lock() {
                        s.push_str(&line);
                        if s.len() > STDERR_TAIL {
                            let mut cut = s.len() - STDERR_TAIL;
                            while cut < s.len() && !s.is_char_boundary(cut) {
                                cut += 1;
                            }
                            *s = s[cut..].to_string();
                        }
                    }
                }
            });
        }

        let (tx, frames) = std::sync::mpsc::channel::<Result<Frame, String>>();
        std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut dec = Decoder::default();
            let mut buf = [0u8; 8192];
            loop {
                let n = match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                dec.feed(&buf[..n]);
                loop {
                    match dec.next_frame() {
                        Ok(Some(f)) => {
                            if tx.send(Ok(f)).is_err() {
                                return; // the Bridge went away
                            }
                        }
                        Ok(None) => break,
                        // A desynchronised stream cannot be recovered, so the
                        // error goes out once and this thread ends. The
                        // receiver then sees a closed channel, and the bridge
                        // is reaped and reopened on the next request.
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            return;
                        }
                    }
                }
            }
        });

        let mut b = Bridge { child, stdin, frames, stderr: tail, next_channel: 0 };

        let hello = b.recv(Instant::now() + HANDSHAKE_TIMEOUT)?;
        if !hello.is_control() {
            return Err("the bridge's first frame was not on the control channel".into());
        }
        let v: Value = serde_json::from_slice(&hello.data)
            .map_err(|e| format!("the bridge's init was not JSON: {e}"))?;
        check_init(&v)?;
        b.control(&our_init())?;
        Ok(b)
    }

    /// Whether the child is still running. The only thing that tells a bridge
    /// which died in the background from one that is merely idle.
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn stderr_tail(&self) -> String {
        self.stderr.lock().map(|s| s.trim().to_string()).unwrap_or_default()
    }

    /// A channel id no channel on this bridge has used before. Ids are never
    /// reused, so a late frame from a channel we have closed cannot be read as
    /// the answer to a later question.
    fn alloc_channel(&mut self) -> String {
        self.next_channel += 1;
        format!("c{}", self.next_channel)
    }

    fn send(&mut self, channel: &str, data: &[u8]) -> Result<(), String> {
        let frame = encode(channel, data);
        self.stdin
            .write_all(&frame)
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("could not write to the bridge: {e}"))
    }

    /// A command on the control channel, which is the one with an empty id.
    fn control(&mut self, v: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        self.send("", &body)
    }

    fn recv(&mut self, deadline: Instant) -> Result<Frame, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        match self.frames.recv_timeout(left) {
            Ok(Ok(f)) => Ok(f),
            Ok(Err(e)) => Err(e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err("the bridge did not answer in time".into())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let tail = self.stderr_tail();
                Err(if tail.is_empty() {
                    "the bridge closed without answering".into()
                } else {
                    format!("the bridge closed without answering: {tail}")
                })
            }
        }
    }

    /// Ask for a channel and get back its id. `opts` names the payload and
    /// whatever that payload needs; `command` and `channel` are set here so no
    /// caller can set them to anything else.
    fn open_channel(&mut self, mut opts: Value) -> Result<String, String> {
        let ch = self.alloc_channel();
        opts["command"] = json!("open");
        opts["channel"] = json!(ch.clone());
        self.control(&opts)?;
        Ok(ch)
    }

    /// Best effort. If the bridge has already gone this fails, and the failure
    /// of a close tells the caller nothing it wants to know.
    fn close_channel(&mut self, ch: &str) {
        let _ = self.control(&json!({ "command": "close", "channel": ch }));
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What a `close` on the control channel means when it arrives instead of an
/// answer: the channel could not be opened, and `problem` is the whole of the
/// explanation.
fn channel_closed(v: &Value) -> String {
    match v["problem"].as_str() {
        Some(p) if !p.is_empty() => format!("the bridge refused the channel: {p}"),
        _ => "the bridge closed the channel without answering".into(),
    }
}

// ------------------------------------------------------------ one per session

/// The open bridges, keyed by session token.
///
/// This is the wrong home for them and it is worth saying why it is theirs. A
/// bridge belongs to a session: one per signed-in user, opened lazily on first
/// use and closed at sign-out. A `cockpit: Mutex<Option<Bridge>>` field on
/// `main::Session` would give all of that for nothing -- `Drop` would run when
/// the last `Arc` to the session went, and none of the code below would need to
/// exist. That field is not mine to add, so the lifetime is reconstructed here,
/// and reconstructing it costs three things worth naming.
///
/// It repeats the cookie parse `main::session_of` already does, because the
/// token *is* the session's identity and `session_of` returns the session
/// without it. It cannot see a sign-out, so a bridge outlives the session that
/// opened it until the sweep reaps it -- an idle `cockpit-bridge` in the
/// meantime, which is a few megabytes and no privilege it did not already have.
/// And it is process-wide mutable state in a program that otherwise keeps
/// everything per-session behind `AppState`.
///
/// The sweep is what keeps that from being a leak: every access drops bridges
/// whose process has died and bridges nobody has touched for [`IDLE_MAX`].
static BRIDGES: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();

struct Entry {
    bridge: Arc<Mutex<Bridge>>,
    last_used: Instant,
}

fn bridges() -> &'static Mutex<HashMap<String, Entry>> {
    BRIDGES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The session token from the request's cookies.
///
/// The same parse `main::session_of` does, for the reason given at [`BRIDGES`].
/// It is only ever a map key: `session_of` has already decided whether this
/// token names a session, and a token that names none never reaches here.
fn session_key(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == crate::COOKIE)
        .map(|(_, v)| v.to_string())
}

/// This session's bridge, opening one if there is not already a usable one.
///
/// A bridge whose process has died is dropped and replaced rather than handed
/// back. The alternative is a session that receives the same dead handle for as
/// long as it stays signed in, every panel reporting the same failure, and no
/// way to clear it short of signing out and in again.
fn bridge_for(key: &str, ident: &Identity) -> Result<Arc<Mutex<Bridge>>, String> {
    let mut map = bridges().lock().map_err(|_| "the bridge table is poisoned".to_string())?;

    let now = Instant::now();
    map.retain(|_, e| {
        if now.duration_since(e.last_used) > IDLE_MAX {
            return false;
        }
        // `try_lock` rather than `lock`: an entry busy on another request is
        // alive by definition, and blocking on it here would hold the whole
        // table shut for the length of somebody else's journal read.
        match e.bridge.try_lock() {
            Ok(mut b) => b.alive(),
            Err(_) => true,
        }
    });

    if let Some(e) = map.get_mut(key) {
        let usable = match e.bridge.try_lock() {
            Ok(mut b) => b.alive(),
            Err(_) => true,
        };
        if usable {
            e.last_used = now;
            return Ok(e.bridge.clone());
        }
        map.remove(key);
    }

    // The spawn happens with the table locked, which serialises everybody's
    // first use. That is deliberate: it is the only thing stopping two requests
    // from one session racing into two bridges, and a spawn takes milliseconds.
    let bridge = Arc::new(Mutex::new(Bridge::open(ident)?));
    map.insert(key.to_string(), Entry { bridge: bridge.clone(), last_used: now });
    Ok(bridge)
}

/// Forget this session's bridge, so the next request opens a fresh one.
fn evict(key: &str) {
    if let Ok(mut map) = bridges().lock() {
        map.remove(key);
    }
}

// ----------------------------------------------------------------- validation

/// The unit types a panel here could show or act on.
const UNIT_SUFFIXES: &[&str] = &[
    ".service", ".socket", ".timer", ".target", ".mount", ".automount", ".path", ".slice",
    ".scope", ".swap", ".device",
];

/// systemd's own ceiling. Longer than any real unit name, and short enough that
/// refusing on it costs nothing.
const MAX_UNIT_NAME: usize = 256;

/// Whether this is a name systemd could have a unit for.
///
/// **Why the check is here.** The unit name is the only value in this module
/// that arrives from a request, and this is the last point at which it is still
/// a request parameter. One line further on it is a JSON option on a `journal`
/// channel or an argument to a D-Bus method, and after that it is the bridge's
/// business and then journald's -- neither of which has any way of knowing it
/// came from a browser, and neither of which will refuse it on those grounds.
/// A check belongs at the edge where a string stops being data and becomes an
/// argument, and this is that edge.
///
/// **What counts as legal.** systemd's own alphabet: ASCII letters and digits
/// plus `:-_.\@`, ending in a known unit type, non-empty, with no `..`. That is
/// narrower than systemd itself would accept -- escaped names can carry more --
/// and narrow is the right side to be wrong on, since everything it turns away
/// is something no panel would have listed. Without enumerating them, it
/// excludes every separator a shell would notice, both spellings of path
/// traversal, and the whitespace and newlines that would let one name pretend
/// to be two.
fn valid_unit(unit: &str) -> bool {
    if unit.is_empty() || unit.len() > MAX_UNIT_NAME {
        return false;
    }
    if unit.contains("..") {
        return false;
    }
    if !UNIT_SUFFIXES.iter().any(|s| unit.ends_with(s)) {
        return false;
    }
    // A name that is nothing but its suffix has no unit in it.
    if unit.starts_with('.') {
        return false;
    }
    unit.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.' | '\\' | '@'))
}

/// The verbs this can run, and the manager method each one is.
///
/// A table rather than a check on a string that is then passed along: a request
/// selects a row, and what gets called is the `&'static str` in that row, so
/// there is no path by which a verb becomes anything other than one of three
/// method names compiled into the binary. Matching is exact and
/// case-sensitive -- `Restart` is not a verb here. Folding case would only be a
/// second, looser spelling of the same list, and the only thing it could ever
/// admit is a word that the interface this serves did not send.
const VERBS: &[(&str, &str)] =
    &[("start", "StartUnit"), ("stop", "StopUnit"), ("restart", "RestartUnit")];

fn verb_method(verb: &str) -> Option<&'static str> {
    VERBS.iter().find(|(v, _)| *v == verb).map(|(_, m)| *m)
}

// ---------------------------------------------------------------------- D-Bus

fn dbus_error(v: &Value) -> String {
    let name = v.get(0).and_then(|n| n.as_str()).unwrap_or("an unnamed D-Bus error");
    let msg = v.get(1).and_then(|m| m.get(0)).and_then(|m| m.as_str()).unwrap_or("");
    if msg.is_empty() {
        name.to_string()
    } else {
        format!("{name}: {msg}")
    }
}

/// One method call on systemd's manager, over a `dbus-json3` channel on the
/// system bus.
///
/// The channel is opened per call and closed after it. Keeping one open across
/// calls would save a round trip and buy a whole class of bug in exchange: an
/// answer to a question asked a minute ago, read as the answer to this one.
/// `id` exists to make that impossible even within a call, and is checked.
fn systemd_call(b: &mut Bridge, method: &'static str, args: Value) -> Result<Value, String> {
    let ch = b.open_channel(json!({
        "payload": "dbus-json3",
        "bus": "system",
        "name": SYSTEMD_BUS_NAME,
    }))?;
    let call = serde_json::to_vec(&json!({
        "call": [SYSTEMD_PATH, SYSTEMD_MANAGER, method, args],
        "id": "1",
    }))
    .map_err(|e| e.to_string())?;

    // Sent without waiting for `ready`. One ordered stream carries both, the
    // bridge processes `open` before the data that follows it, and waiting for
    // a control frame that a future bridge might reasonably stop sending would
    // be a deadlock with no upside.
    let out = match b.send(&ch, &call) {
        Ok(()) => read_call_reply(b, &ch),
        Err(e) => Err(e),
    };
    b.close_channel(&ch);
    out
}

fn read_call_reply(b: &mut Bridge, ch: &str) -> Result<Value, String> {
    let deadline = Instant::now() + CALL_TIMEOUT;
    loop {
        let frame = b.recv(deadline)?;
        if frame.is_control() {
            let v: Value = serde_json::from_slice(&frame.data).unwrap_or(Value::Null);
            if v["channel"].as_str() != Some(ch) {
                continue;
            }
            if v["command"].as_str() == Some("close") {
                return Err(channel_closed(&v));
            }
            continue;
        }
        if frame.channel != ch {
            continue;
        }
        let v: Value = serde_json::from_slice(&frame.data)
            .map_err(|e| format!("the bridge sent malformed JSON: {e}"))?;
        if v["id"].as_str() != Some("1") {
            // A signal or a notify, which this channel gets whether it asked
            // for one or not. Not an answer, so not ours.
            continue;
        }
        if let Some(reply) = v.get("reply") {
            return Ok(reply.clone());
        }
        if let Some(err) = v.get("error") {
            return Err(dbus_error(err));
        }
    }
}

// ------------------------------------------------------------------ the units

fn s(v: Option<&Value>) -> String {
    v.and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// `GET /api/host/services` -- the units this user may see, via a `dbus`
/// channel against `org.freedesktop.systemd1`.
///
/// Two calls rather than one, merged on the unit name. `ListUnits` answers with
/// what systemd has *loaded*, which is the only source of a live active and sub
/// state but silently omits every installed unit that has never been started --
/// a panel built on it alone shows nothing to start. `ListUnitFiles` answers
/// with what is on disk and whether it is enabled, and knows nothing about
/// state. Each is half the question, so both are asked.
///
/// Only `.service` units are returned. The route is called services and a timer
/// or a mount wants different columns; listing them here would mean a panel
/// that has to decide what a socket's "restart" button means, which is a
/// different panel.
pub fn services(b: &mut Bridge) -> Result<Value, String> {
    let loaded = systemd_call(b, "ListUnits", json!([]))?;
    let files = systemd_call(b, "ListUnitFiles", json!([]))?;

    // `ListUnitFiles` returns full paths; the unit's name is the last segment.
    let mut state_of: BTreeMap<String, String> = BTreeMap::new();
    if let Some(rows) = files.get(0).and_then(|v| v.as_array()) {
        for row in rows {
            let path = s(row.get(0));
            let Some(name) = path.rsplit('/').next() else { continue };
            if name.is_empty() {
                continue;
            }
            state_of.insert(name.to_string(), s(row.get(1)));
        }
    }

    // (name, description, load, active, sub, followed, path, job id, ...) --
    // the first five are the whole of what a list needs, and the rest is job
    // bookkeeping that means nothing outside systemd.
    let mut units: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(rows) = loaded.get(0).and_then(|v| v.as_array()) {
        for row in rows {
            let name = s(row.get(0));
            if !name.ends_with(".service") {
                continue;
            }
            let file_state = state_of.get(&name).cloned();
            units.insert(
                name.clone(),
                unit_json(&name, &s(row.get(1)), &s(row.get(2)), &s(row.get(3)), &s(row.get(4)), file_state),
            );
        }
    }

    // Everything on disk that systemd has not loaded. `inactive`/`dead` is what
    // systemd itself reports for these once they are loaded, so a panel does
    // not need to know which of the two calls a row came from.
    for (name, file_state) in &state_of {
        if !name.ends_with(".service") || units.contains_key(name) {
            continue;
        }
        units.insert(
            name.clone(),
            unit_json(name, "", "not-found", "inactive", "dead", Some(file_state.clone())),
        );
    }

    let list: Vec<Value> = units.into_values().collect();
    Ok(json!({ "units": list, "count": list.len() }))
}

/// One row, flat and stable.
///
/// `enabled` is the plain answer a checkbox wants; `unit_file_state` is the one
/// systemd gave, because `static`, `masked` and `generated` are all "not
/// enabled" to a boolean and mean three different things to whoever is looking
/// at the list. Both, rather than one, so the panel need not choose between a
/// simple control and a truthful one.
fn unit_json(
    name: &str,
    description: &str,
    load: &str,
    active: &str,
    sub: &str,
    file_state: Option<String>,
) -> Value {
    let enabled = matches!(file_state.as_deref(), Some("enabled") | Some("enabled-runtime"));
    json!({
        "name": name,
        "description": description,
        "load": load,
        "active": active,
        "sub": sub,
        "enabled": enabled,
        "unit_file_state": file_state,
    })
}

/// The set of units `services` would list, for the action to check itself
/// against. One extra round trip per action, which buys the property the
/// endpoint's contract claims: it can only act on something the desk has shown.
fn listed_units(b: &mut Bridge) -> Result<BTreeSet<String>, String> {
    let v = services(b)?;
    Ok(v["units"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|r| r["name"].as_str().map(String::from)).collect())
        .unwrap_or_default())
}

// ---------------------------------------------------------------- the journal

/// `GET /api/host/journal` -- log lines for one unit.
///
/// A `journal` channel with `follow` off, which is the difference between a
/// request that ends and a subscription. The bridge sends the entries and then
/// `done`, and this returns what it collected.
///
/// The entries are flattened on the way through rather than passed on as
/// journald wrote them. A journal record has upwards of thirty fields, almost
/// all of them about the machine rather than the message, and forwarding the
/// lot would make the browser's job the parsing of a format that is not its
/// business -- and would pass on fields nobody chose to publish.
pub fn journal(b: &mut Bridge, unit: &str, lines: usize) -> Result<Value, String> {
    if !valid_unit(unit) {
        return Err(format!("{unit:?} is not a unit name"));
    }
    let lines = lines.clamp(1, JOURNAL_MAX);
    let ch = b.open_channel(json!({
        "payload": "journal",
        "unit": unit,
        "count": lines,
        "follow": false,
    }))?;
    let out = read_journal(b, &ch);
    b.close_channel(&ch);
    let entries = out?;
    Ok(json!({ "unit": unit, "count": entries.len(), "lines": entries }))
}

fn read_journal(b: &mut Bridge, ch: &str) -> Result<Vec<Value>, String> {
    let deadline = Instant::now() + CALL_TIMEOUT;
    let mut entries: Vec<Value> = Vec::new();
    loop {
        let frame = b.recv(deadline)?;
        if frame.is_control() {
            let v: Value = serde_json::from_slice(&frame.data).unwrap_or(Value::Null);
            if v["channel"].as_str() != Some(ch) {
                continue;
            }
            match v["command"].as_str() {
                // `done` is the end of the backlog. With `follow` off it is
                // followed by a `close`, but waiting for that would mean a
                // second deadline for nothing.
                Some("done") => return Ok(entries),
                Some("close") => {
                    // A close after entries have arrived is the ordinary end of
                    // a channel; a close before any is a refusal, and the only
                    // one of the two worth reporting as a failure.
                    return if entries.is_empty() { Err(channel_closed(&v)) } else { Ok(entries) };
                }
                _ => continue,
            }
        }
        if frame.channel != ch {
            continue;
        }
        // The bridge has sent a batch of entries as one array and a single
        // entry as one object across different versions. Both are accepted
        // because guessing wrong means an empty log with no error on it.
        match serde_json::from_slice::<Value>(&frame.data) {
            Ok(Value::Array(rows)) => entries.extend(rows.iter().map(entry_json)),
            Ok(v @ Value::Object(_)) => entries.push(entry_json(&v)),
            _ => {}
        }
    }
}

/// One journal record, reduced to what a log panel paints.
fn entry_json(v: &Value) -> Value {
    json!({
        "ts": v["__REALTIME_TIMESTAMP"].as_str().and_then(|t| t.parse::<u64>().ok()),
        "priority": v["PRIORITY"].as_str().and_then(|p| p.parse::<u8>().ok()),
        "identifier": v["SYSLOG_IDENTIFIER"].as_str(),
        "pid": v["_PID"].as_str().and_then(|p| p.parse::<u32>().ok()),
        "message": message_text(&v["MESSAGE"]),
    })
}

/// A journal message is usually a string, and is an array of byte values when
/// the line was not valid UTF-8. Rendering the second as a list of numbers is
/// how a panel ends up showing `[104,105]` to somebody, so it is decoded here.
fn message_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(bytes) => {
            let raw: Vec<u8> = bytes.iter().filter_map(|b| b.as_u64()).map(|b| b as u8).collect();
            String::from_utf8_lossy(&raw).into_owned()
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- the metrics

/// The internal metrics one sample is made of. `cpu.core.user` is asked for
/// only so that its instance list can be counted: the basic CPU counters are
/// summed across every core, and without knowing how many there are they cannot
/// be turned into a proportion of anything.
const METRICS: &[&str] = &[
    "cpu.basic.user",
    "cpu.basic.system",
    "cpu.basic.nice",
    "cpu.basic.iowait",
    "cpu.core.user",
    "memory.used",
    "memory.free",
    "memory.cached",
    "memory.swap-used",
    "mount.used",
    "mount.total",
];

/// How many rows to merge before answering. Two, because the CPU counters are
/// rates: the first row of a rate has nothing to have been a rate *since*.
const METRIC_ROWS: usize = 2;

/// `GET /api/host/metrics` -- one sample of CPU, memory and disk.
///
/// **One sample, and then the channel closes.** A `metrics1` channel with an
/// interval is a subscription, and a subscription needs somebody to end it --
/// which in a web desk means noticing that a panel was closed, or a tab was
/// hidden, or a laptop was shut, and getting every one of those right or else
/// leaking a sampler per visit. A panel that polls this endpoint has none of
/// that: the request is the lifetime. It costs a round trip per refresh and
/// removes the entire question of who closes the stream.
///
/// The channel still has to be opened with an interval, and two rows still have
/// to be read, because the CPU figures are rates -- see [`METRIC_ROWS`]. So one
/// sample takes about a second of wall clock, which is a poll interval's worth
/// of latency and no more.
pub fn metrics(b: &mut Bridge) -> Result<Value, String> {
    let ch = b.open_channel(json!({
        "payload": "metrics1",
        "source": "internal",
        "interval": 1000,
        "metrics": METRICS.iter().map(|m| json!({ "name": m })).collect::<Vec<_>>(),
    }))?;
    let out = read_sample(b, &ch);
    b.close_channel(&ch);
    let (meta, row) = out?;

    let at = |name: &str| -> Option<&Value> {
        meta.names.iter().position(|n| n == name).and_then(|i| row.get(i))
    };
    let num = |name: &str| -> Option<f64> { at(name).and_then(|v| v.as_f64()) };

    // The basic counters are milliseconds of CPU time per second, summed over
    // every core, so a fully busy machine reports 1000 per core. Reported as
    // null rather than guessed at when the core count is missing: a percentage
    // computed against the wrong denominator is worse than no percentage.
    let cores = meta.instances_of("cpu.core.user").map(|i| i.len()).filter(|n| *n > 0);
    let busy = match (num("cpu.basic.user"), num("cpu.basic.system"), num("cpu.basic.nice"), cores) {
        (Some(u), Some(sy), Some(n), Some(c)) => Some((u + sy + n) / (10.0 * c as f64)),
        _ => None,
    };

    let used = num("memory.used");
    let free = num("memory.free");
    let cached = num("memory.cached");
    // The bridge reports no total, so it is the sum of the parts -- the same
    // identity Cockpit's own memory graph is drawn from. Only offered when all
    // three arrived, since a total missing one of them reads as a machine with
    // less memory than it has.
    let total = match (used, free, cached) {
        (Some(u), Some(f), Some(c)) => Some(u + f + c),
        _ => None,
    };

    let (mount, disk_used, disk_total) = disk_of(&meta, &row);

    Ok(json!({
        "cpu": {
            "busy_percent": busy,
            "user": num("cpu.basic.user"),
            "system": num("cpu.basic.system"),
            "nice": num("cpu.basic.nice"),
            "iowait": num("cpu.basic.iowait"),
            "cores": cores,
        },
        "memory": {
            "used": used,
            "free": free,
            "cached": cached,
            "swap_used": num("memory.swap-used"),
            "total": total,
        },
        "disk": { "mount": mount, "used": disk_used, "total": disk_total },
        "sampled_at": now(),
    }))
}

/// What the `metrics1` channel says it is about to send: one entry per metric,
/// in the order the rows will use, with an instance list for the metrics that
/// have one.
struct MetricMeta {
    names: Vec<String>,
    instances: Vec<Option<Vec<String>>>,
}

impl MetricMeta {
    fn parse(v: &Value) -> Option<MetricMeta> {
        let rows = v["metrics"].as_array()?;
        let mut names = Vec::with_capacity(rows.len());
        let mut instances = Vec::with_capacity(rows.len());
        for m in rows {
            names.push(m["name"].as_str()?.to_string());
            instances.push(
                m["instances"]
                    .as_array()
                    .map(|i| i.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()),
            );
        }
        Some(MetricMeta { names, instances })
    }

    fn instances_of(&self, name: &str) -> Option<&Vec<String>> {
        let i = self.names.iter().position(|n| n == name)?;
        self.instances.get(i)?.as_ref()
    }
}

/// Read the meta and enough rows to answer with, merging as it goes.
fn read_sample(b: &mut Bridge, ch: &str) -> Result<(MetricMeta, Vec<Value>), String> {
    let deadline = Instant::now() + METRICS_TIMEOUT;
    let mut meta: Option<MetricMeta> = None;
    let mut row: Vec<Value> = Vec::new();
    let mut merged = 0usize;

    loop {
        let frame = b.recv(deadline)?;
        if frame.is_control() {
            let v: Value = serde_json::from_slice(&frame.data).unwrap_or(Value::Null);
            if v["channel"].as_str() != Some(ch) {
                continue;
            }
            if matches!(v["command"].as_str(), Some("close")) {
                return match meta {
                    // Whatever arrived before the close is still a sample, and
                    // a partial one beats a panel of dashes.
                    Some(m) if merged > 0 => Ok((m, row)),
                    _ => Err(channel_closed(&v)),
                };
            }
            continue;
        }
        if frame.channel != ch {
            continue;
        }
        let v: Value = match serde_json::from_slice(&frame.data) {
            Ok(v) => v,
            Err(e) => return Err(format!("the bridge sent malformed metrics: {e}")),
        };
        match v {
            // The first data message describes the rows that follow. A second
            // one means the shape changed underneath us -- a filesystem was
            // mounted -- and the rows already merged no longer line up, so the
            // count starts again against the new shape.
            Value::Object(_) => {
                let m = MetricMeta::parse(&v)
                    .ok_or("the bridge's metrics meta named no metrics".to_string())?;
                row = vec![Value::Null; m.names.len()];
                merged = 0;
                meta = Some(m);
            }
            // Rows before the meta cannot be lined up with anything, so they
            // are dropped rather than merged into a shape we are guessing at.
            Value::Array(samples) if meta.is_some() => {
                for sample in samples {
                    let Some(values) = sample.as_array() else { continue };
                    merge_row(&mut row, values);
                    merged += 1;
                    if merged >= METRIC_ROWS {
                        return Ok((meta.expect("checked just above"), row));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Fold one row into the running sample.
///
/// `null` in a `metrics1` row means "the same as last time" rather than "no
/// value" -- the channel only sends what changed. A reader that takes a row at
/// face value therefore shows a machine whose memory keeps vanishing, because
/// the number that did not move is the number that is not there.
fn merge_row(into: &mut [Value], row: &[Value]) {
    for (i, v) in row.iter().enumerate() {
        let Some(slot) = into.get_mut(i) else { break };
        match v {
            Value::Null => {}
            // An instanced metric is an array parallel to its instance list,
            // and its elements are elided one at a time on the same rule.
            Value::Array(items) => match slot {
                Value::Array(kept) if kept.len() == items.len() => {
                    for (k, item) in kept.iter_mut().zip(items) {
                        if !item.is_null() {
                            *k = item.clone();
                        }
                    }
                }
                _ => *slot = v.clone(),
            },
            _ => *slot = v.clone(),
        }
    }
}

/// The root filesystem's usage out of the instanced mount metrics.
///
/// `/` rather than every mount: a panel showing one number wants the one
/// everybody means by "disk", and a host with a dozen bind mounts would
/// otherwise get a dozen rows nobody asked for. The mount that was actually
/// read is named in the answer, so a host with no `/` instance reports which
/// one it fell back to instead of quietly meaning something else.
fn disk_of(meta: &MetricMeta, row: &[Value]) -> (Option<String>, Option<f64>, Option<f64>) {
    let Some(mounts) = meta.instances_of("mount.used") else {
        return (None, None, None);
    };
    let idx = mounts.iter().position(|m| m == "/").or(if mounts.is_empty() { None } else { Some(0) });
    let Some(idx) = idx else { return (None, None, None) };

    let value = |name: &str| -> Option<f64> {
        let i = meta.names.iter().position(|n| n == name)?;
        row.get(i)?.as_array()?.get(idx)?.as_f64()
    };
    (Some(mounts[idx].clone()), value("mount.used"), value("mount.total"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// --- handlers -------------------------------------------------------------
//
// Narrow and named, one per thing the desk can show. None of them takes a
// channel type, a payload shape or a bus name from the request -- that is the
// difference between using the bridge and exposing it.

fn bad(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// The refusal a host without the package gets, and the one signal the
/// dependency installer keys on.
///
/// `503` and `"reason": "not-installed"`, with the package this host's manager
/// would install. Never a `500`: nothing has gone wrong, the machine simply has
/// not got the thing yet, and a panel that says so with the fix in the same
/// sentence is the difference between a feature that looks broken and one that
/// looks optional.
fn not_installed() -> Response {
    let manager = crate::flatpak::manager();
    let package = manager.and_then(|m| match m {
        crate::flatpak::Manager::Dnf => BRIDGE_PREREQ.dnf,
        crate::flatpak::Manager::Apt => BRIDGE_PREREQ.apt,
        crate::flatpak::Manager::Pacman => BRIDGE_PREREQ.pacman,
        crate::flatpak::Manager::Zypper => BRIDGE_PREREQ.zypper,
    });
    let detail = match (manager.map(|m| m.bin()), package) {
        (Some(mgr), Some(pkg)) => {
            format!("WebDesk can install it here with {mgr}, as the {pkg} package.")
        }
        _ => format!(
            "Install the {} package for this distribution and the host panels will work.",
            BRIDGE_PREREQ.bin
        ),
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": format!(
                "The host panels need {}, which is not installed on this host.",
                BRIDGE_PREREQ.bin
            ),
            "reason": "not-installed",
            "detail": detail,
            "missing": { "bin": BRIDGE_PREREQ.bin, "package": package, "manager": manager.map(|m| m.bin()) },
        })),
    )
        .into_response()
}

/// The bridge is here and did not work. Distinct from [`not_installed`] on
/// purpose: an installer that read this as "install it" would install a package
/// that is already present, and go round again.
fn bridge_failed(msg: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": msg.to_string(), "reason": "bridge-failed" })),
    )
        .into_response()
}

/// The bridge answered and the answer was no -- a unit that does not exist,
/// polkit declining. Not a fault in the bridge, so it does not wear the
/// bridge's status code.
fn refused(msg: impl std::fmt::Display) -> Response {
    (StatusCode::CONFLICT, Json(json!({ "error": msg.to_string(), "reason": "refused" })))
        .into_response()
}

/// The shape every handler here has: signed in, a bridge, one operation on it.
///
/// The operation runs on a blocking task. Every read inside it parks a thread
/// on a pipe, and doing that on a runtime worker would stall every other
/// request on this executor for as long as journald took to answer.
async fn with_bridge<F>(state: AppState, headers: HeaderMap, f: F) -> Response
where
    F: FnOnce(&mut Bridge) -> Result<Value, String> + Send + 'static,
{
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    let Some(key) = session_key(&headers) else { return unauthorized() };
    if !available() {
        return not_installed();
    }
    // The whole session moves onto the blocking task rather than a copy of the
    // identity: `auth::Identity` is not `Clone` and is not mine to make so, and
    // `main::ask` already carries an `Arc<Session>` across the same boundary.
    let result = tokio::task::spawn_blocking(move || {
        let bridge = bridge_for(&key, &session.ident).map_err(|e| (true, e))?;
        let mut b = bridge.lock().map_err(|_| (true, "the bridge lock is poisoned".to_string()))?;
        match f(&mut b) {
            Ok(v) => Ok(v),
            // A bridge that died mid-call is a transport failure however the
            // error reads, and leaving it in the table would hand the same
            // corpse to every request until the sweep noticed.
            Err(e) => {
                let dead = !b.alive();
                drop(b);
                if dead {
                    evict(&key);
                }
                Err((dead, e))
            }
        }
    })
    .await;

    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err((true, e))) => bridge_failed(e),
        Ok(Err((false, e))) => refused(e),
        Err(e) => bridge_failed(format!("the host panel task did not finish: {e}")),
    }
}

/// `GET /api/host/services`
pub async fn host_services(State(s): State<AppState>, h: HeaderMap) -> Response {
    with_bridge(s, h, services).await
}

/// `GET /api/host/journal?unit=&lines=`
pub async fn host_journal(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Response {
    let unit = q.get("unit").cloned().unwrap_or_default();
    if !valid_unit(&unit) {
        return bad(StatusCode::BAD_REQUEST, format!("{unit:?} is not a unit name"));
    }
    let lines = q
        .get("lines")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(JOURNAL_DEFAULT)
        .clamp(1, JOURNAL_MAX);
    with_bridge(s, h, move |b| journal(b, &unit, lines)).await
}

/// `GET /api/host/metrics`
pub async fn host_metrics(State(s): State<AppState>, h: HeaderMap) -> Response {
    with_bridge(s, h, metrics).await
}

#[derive(serde::Deserialize)]
struct ActionReq {
    unit: String,
    verb: String,
}

/// `POST /api/host/services/action` -- `{"unit":"…","verb":"start|stop|restart"}`.
///
/// The verb is matched against a fixed set and the unit against what
/// `host_services` already listed, so this can only act on something the desk
/// has shown you. It is not a way to name a unit.
///
/// **Who may do this.** The same administrative group that gates installing an
/// app and updating the binary, checked in code here for the same reason
/// `update.rs` gives: this is a decision about the host rather than about the
/// signed-in user's own files, and there is nothing for the kernel to decide it
/// with. Everyone signed in may read the lists and the logs; stopping a service
/// is not reading.
///
/// **The wrinkle, stated honestly.** The bridge already runs as the signed-in
/// user, so a restart from an unprivileged session would meet polkit and be
/// refused anyway -- the kernel and polkit are, between them, the real answer
/// to this question and they would give it without any help from this file. The
/// gate here is a second and earlier answer to the same one. It earns its place
/// on two counts and no more: it refuses in WebDesk's own words instead of a
/// D-Bus error name, and it does not depend on how any particular host has
/// configured polkit -- which on a host where an administrator has made systemd
/// permissive is the difference between a rule and a hope. It is not a
/// substitute for either, and nothing here should be written as though it were.
pub async fn host_service_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(session) = session_of(&state, &headers) else { return unauthorized() };
    if !session.ident.admin {
        tracing::warn!(
            user = %session.ident.username,
            "denied a host service action: not in {:?}",
            crate::auth::admin_groups()
        );
        return bad(
            StatusCode::FORBIDDEN,
            format!(
                "starting and stopping host services requires membership of {}",
                crate::auth::admin_groups().join(" or ")
            ),
        );
    }

    let Ok(req) = serde_json::from_slice::<ActionReq>(&body) else {
        return bad(StatusCode::BAD_REQUEST, "expected a unit and a verb");
    };
    let Some(method) = verb_method(&req.verb) else {
        return bad(
            StatusCode::BAD_REQUEST,
            format!(
                "{:?} is not something WebDesk does to a service; it does {}",
                req.verb,
                VERBS.iter().map(|(v, _)| *v).collect::<Vec<_>>().join(", ")
            ),
        );
    };
    if !valid_unit(&req.unit) {
        return bad(StatusCode::BAD_REQUEST, format!("{:?} is not a unit name", req.unit));
    }

    let unit = req.unit.clone();
    let verb = req.verb.clone();
    let actor = session.ident.username.clone();
    with_bridge(state, headers, move |b| {
        if !listed_units(b)?.contains(&unit) {
            return Err(format!("{unit} is not a service on this host"));
        }
        tracing::info!(user = %actor, %verb, %unit, "host service action");
        // `replace` is systemd's own default: a start queued behind a stop
        // finishes the stop rather than failing on it, which is what somebody
        // pressing Restart twice means and not what `fail` would do.
        systemd_call(b, method, json!([unit, "replace"]))?;
        Ok(json!({ "ok": true, "unit": unit, "verb": verb }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(chunks: &[&[u8]]) -> Vec<Frame> {
        let mut dec = Decoder::default();
        let mut out = Vec::new();
        for c in chunks {
            dec.feed(c);
            while let Some(f) = dec.next_frame().expect("decode") {
                out.push(f);
            }
        }
        out
    }

    /// The length prefix counts the channel line as well as the data. An
    /// encoder that leaves it out writes frames the bridge reads as truncated,
    /// and the mistake is invisible until something on the other end complains
    /// in a language nobody here speaks.
    #[test]
    fn a_frames_length_covers_its_channel_line() {
        assert_eq!(encode("", b"{}"), b"3\n\n{}".to_vec());
        assert_eq!(encode("c1", b"{}"), b"5\nc1\n{}".to_vec());
    }

    /// What goes out comes back, on the control channel and on a named one.
    #[test]
    fn a_frame_survives_a_round_trip() {
        let frames = decode_all(&[&encode("", b"{\"command\":\"init\"}"), &encode("c7", b"hello")]);
        assert_eq!(
            frames,
            vec![
                Frame { channel: String::new(), data: b"{\"command\":\"init\"}".to_vec() },
                Frame { channel: "c7".into(), data: b"hello".to_vec() },
            ]
        );
        assert!(frames[0].is_control());
        assert!(!frames[1].is_control());
    }

    /// The test this file exists for. A pipe gives back whatever it has, so a
    /// frame arrives in as many pieces as the kernel felt like -- including a
    /// length prefix split down the middle. A decoder that assumes one read is
    /// one frame passes every other test here and corrupts the stream in
    /// production, where the journal is long and the reads are 8 KiB.
    #[test]
    fn a_frame_split_across_reads_is_decoded_whole() {
        let payload = "x".repeat(9000);
        let whole = encode("c1", payload.as_bytes());
        for cut in [1usize, 2, 3, 4, 5, 9, 100, 8192, whole.len() - 1] {
            let (a, b) = whole.split_at(cut);
            let frames = decode_all(&[a, b]);
            assert_eq!(frames.len(), 1, "one frame, split at {cut}");
            assert_eq!(frames[0].channel, "c1");
            assert_eq!(frames[0].data, payload.as_bytes());
        }
        // And in as many pieces as there are bytes, which is the same failure
        // taken to its limit.
        let byte_at_a_time: Vec<&[u8]> = whole.chunks(1).collect();
        let frames = decode_all(&byte_at_a_time);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, payload.as_bytes());
    }

    /// The other half of the same failure: two replies inside one read. A
    /// decoder that returns after one frame leaves the second in the buffer
    /// until a third arrives, so every answer is off by one -- which reads as a
    /// bridge that is answering the previous question.
    #[test]
    fn two_frames_in_one_read_are_both_decoded() {
        let mut both = encode("c1", b"first");
        both.extend_from_slice(&encode("c2", b"second"));
        both.extend_from_slice(&encode("", b"third"));
        let frames = decode_all(&[&both]);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].data, b"first");
        assert_eq!(frames[1].channel, "c2");
        assert_eq!(frames[2].channel, "");
        // Nothing is left over: a decoder that miscounts by one byte per frame
        // still passes the assertions above and desynchronises on the fourth.
        let mut dec = Decoder::default();
        dec.feed(&both);
        while dec.next_frame().unwrap().is_some() {}
        assert!(dec.buf.is_empty());
    }

    /// An incomplete frame is not an error and must not be consumed. Returning
    /// `Err` here would tear down a healthy bridge every time a read landed
    /// mid-frame, which is most of them.
    #[test]
    fn an_incomplete_frame_waits_rather_than_fails() {
        // Seven bytes of payload -- "c1", its newline, and four of data.
        let mut dec = Decoder::default();
        dec.feed(b"7\nc1\nhal");
        assert_eq!(dec.next_frame().expect("not an error"), None);
        dec.feed(b"f");
        assert_eq!(dec.next_frame().unwrap().map(|f| f.data), Some(b"half".to_vec()));
    }

    /// `su -` runs a login shell, so a profile that prints lands in front of
    /// the first frame. The decoder must say that rather than hang: the failure
    /// otherwise looks like a bridge that never answered, and the host it
    /// happens on is the one where nobody suspects /etc/profile.
    #[test]
    fn output_that_is_not_a_frame_is_reported_rather_than_buffered() {
        let mut dec = Decoder::default();
        dec.feed(b"Welcome to this machine, please behave yourself");
        let err = dec.next_frame().expect_err("garbage is not a frame");
        assert!(err.contains("login profile"), "unhelpful message: {err}");
    }

    /// A bridge speaking a protocol this does not would otherwise be decoded
    /// with the wrong parser and produce wrong answers instead of an error.
    #[test]
    fn only_the_protocol_version_this_speaks_is_accepted() {
        assert!(check_init(&json!({"command": "init", "version": 1})).is_ok());
        assert!(check_init(&json!({"command": "init", "version": 2})).is_err());
        assert!(check_init(&json!({"command": "init"})).is_err());
        assert!(check_init(&json!({"command": "close"})).is_err());
        // A refusal carries `problem` and no version, and that word is the only
        // account of it there will be, so it has to reach the message.
        let err = check_init(&json!({"command": "init", "problem": "access-denied"}))
            .expect_err("a problem is a refusal");
        assert!(err.contains("access-denied"), "the refusal was swallowed: {err}");
        assert_eq!(our_init()["version"], json!(PROTOCOL_VERSION));
    }

    /// The verb can never be free text. Anything outside the table is refused,
    /// including the spellings that a looser match would let through -- a case
    /// variant, a leading space, and a word with a shell metacharacter in it,
    /// which is what an attempt to append a second command looks like.
    #[test]
    fn only_the_three_verbs_in_the_table_are_verbs() {
        for verb in ["start", "stop", "restart"] {
            assert!(verb_method(verb).is_some(), "{verb} should be a verb");
        }
        for verb in [
            "Start",
            "START",
            "sTaRt",
            " start",
            "start ",
            "start;reboot",
            "start && reboot",
            "restart; rm -rf /",
            "restart\nstop",
            "reload",
            "mask",
            "kill",
            "enable",
            "",
            "*",
        ] {
            assert!(verb_method(verb).is_none(), "{verb:?} must not be a verb");
        }
        // What is called is the constant in the table, never the request.
        assert_eq!(verb_method("restart"), Some("RestartUnit"));
    }

    /// The unit name is the only thing here that comes from a request, and
    /// everything downstream of this check treats it as an argument. Traversal,
    /// whitespace, shell punctuation and a name with no unit type in it are all
    /// things no panel would have listed and this must not pass on.
    #[test]
    fn a_unit_name_is_a_unit_name_or_it_is_refused() {
        for unit in [
            "sshd.service",
            "getty@tty1.service",
            "systemd-journald.service",
            "dbus.socket",
            "logrotate.timer",
            "multi-user.target",
            "var-lib-machines.mount",
            "user-1000.slice",
            "home\\x2duser.mount",
        ] {
            assert!(valid_unit(unit), "{unit} is a real unit name");
        }
        for unit in [
            "",
            "sshd",
            ".service",
            "../../etc/passwd",
            "../sshd.service",
            "/etc/systemd/system/evil.service",
            "sshd.service/../../root",
            "my service.service",
            "sshd.service ",
            "sshd.service\n",
            "sshd.service\nstop.service",
            "sshd.service;reboot.service",
            "$(id).service",
            "`id`.service",
            "sshd.service|tee.service",
            "*.service",
            "sshd.service\0.service",
            "nul\u{0}.service",
            "unité.service",
        ] {
            assert!(!valid_unit(unit), "{unit:?} must not pass as a unit name");
        }
        // Length, which is the one bound a character check cannot express.
        assert!(!valid_unit(&format!("{}.service", "a".repeat(MAX_UNIT_NAME))));
    }

    /// `cockpit-bridge` is not installed on the machines this is developed on,
    /// and that is not an error -- it is the case every handler has to answer
    /// well, since it is the case nearly every host is in. The refusal it
    /// produces must carry the signal the dependency installer reads, and must
    /// not be a 500.
    #[test]
    fn an_absent_bridge_is_a_state_of_the_host_and_not_a_failure() {
        assert!(!available(), "this machine is not expected to have cockpit-bridge");
        assert!(find_bridge().is_none());

        let refusal = not_installed();
        assert_eq!(refusal.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_ne!(refusal.status(), StatusCode::INTERNAL_SERVER_ERROR);
        // The two words the installer tells apart. They must differ, and they
        // must differ in the field it reads rather than only in the prose.
        assert_eq!(bridge_failed("x").status(), StatusCode::BAD_GATEWAY);
    }

    /// A `metrics1` row elides what has not changed, so a `null` means "as
    /// before" and not "gone". Taking a row at face value shows a machine whose
    /// memory keeps disappearing, because the number that did not move is
    /// exactly the number that is not there.
    #[test]
    fn an_unchanged_metric_keeps_its_last_value() {
        let mut row = vec![Value::Null; 3];
        merge_row(&mut row, &[json!(10), json!(20), json!([1, 2])]);
        merge_row(&mut row, &[Value::Null, json!(21), json!([Value::Null, 3])]);
        assert_eq!(row[0], json!(10), "an elided value was lost");
        assert_eq!(row[1], json!(21));
        assert_eq!(row[2], json!([1, 3]), "an elided instance was lost");
    }

    /// A binary log line arrives as a list of byte values, and passing it on
    /// unchanged is how a panel comes to show somebody `[104,105]`.
    #[test]
    fn a_journal_message_is_text_however_journald_wrote_it() {
        assert_eq!(message_text(&json!("started")), "started");
        assert_eq!(message_text(&json!([104, 105])), "hi");
        assert_eq!(message_text(&Value::Null), "");
    }
}
