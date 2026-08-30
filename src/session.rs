//! Running one streamed application, in the session of the person who opened it.
//!
//! This is what `webdesk-app@<slug>.service` starts, and it runs as an ordinary
//! user with no privilege at all: a systemd *user* unit in their own manager,
//! reached the way `systemd::user_act` describes.
//!
//! **Why the binary and not a script.** The unit is one template for every
//! streamed app, and its `ExecStart` is handed `%i` -- a slug. A slug has to
//! become a Flatpak application id somewhere, and the only place that may happen
//! is inside a program that carries the catalog, because the whole rule this
//! project is built on is that the set of things which may be run is a property
//! of the build. A shell script taking an id from an argument would be exactly
//! the hole `catalog.rs` and `systemd.rs` are written to avoid, wearing a
//! systemd unit as a hat. So the unit runs `webdesk app-session <slug>`, this
//! resolves it against `CATALOG`, and anything not in there is refused before a
//! process is spawned.
//!
//! What it then does is start a headless `cage` holding exactly one application
//! and a `wayvnc` serving it on a socket only this user can open. Nothing here
//! listens on the network.
//!
//! **Three roles, one argv.** `webdesk app-session <slug>` is the unit's
//! `ExecStart`; `--kill` is its `ExecStop`; `--inside-cage` is what `cage`
//! itself is given to run, and is never typed by anybody. They are one file and
//! one catalog lookup because they have to agree about what a slug means, and
//! the cheapest way for two programs to agree is to be the same program -- the
//! argument `helper.rs` already makes for `--helper`.

use crate::catalog::{App, Streamed};
use std::process::Command;

/// The argument `cage` is given so that this binary can be its own child.
///
/// Not a documented interface and not meant for a person: it is how the
/// compositor's environment is handed to the RFB server, which is the whole of
/// the ordering problem -- see `inside_cage`.
const INSIDE: &str = "--inside-cage";

/// The catalog entry a slug names, refused unless it is one this host draws.
///
/// Split out of `run` so that the refusal can be tested without spawning
/// anything, which is the only part of this file a test can reach: everything
/// past it is `cage`, `wayvnc` and `flatpak`, and none of the three is on a
/// machine this is developed on.
///
/// Both refusals matter and they are different mistakes. An unknown slug is a
/// unit file naming something this build has never heard of -- a stale template
/// instance left behind by a downgrade, or a hand-typed `systemctl --user
/// start`. A known slug that is a container is a wiring error: somebody pointed
/// the streamed path at an entry `apps.rs` serves through the proxy, and
/// starting a compositor for it would produce an empty window rather than an
/// error anyone could read.
pub fn resolve(slug: &str) -> Result<(&'static App, &'static Streamed), String> {
    let Some(app) = crate::catalog::find(slug) else {
        return Err(format!("{slug} is not an application in this build"));
    };
    match app.streamed.as_ref() {
        Some(streamed) => Ok((app, streamed)),
        None if app.host.is_some() => {
            Err(format!("{slug} is a service adopted on this host, not one it draws"))
        }
        None => Err(format!("{slug} is a container app, not one this host draws")),
    }
}

/// Run the session for `slug`, replacing this process. Never returns on success.
///
/// Called from `main` before the runtime starts, for the same reason `--helper`
/// is: this must not bind a port or touch shared state.
pub fn run(slug: &str) -> ! {
    let (_app, streamed) = match resolve(slug) {
        Ok(pair) => pair,
        Err(why) => die(&why),
    };
    // `main` has read the slug out of argv and stops there. The rest of argv is
    // read here rather than there because what those words mean is a fact about
    // this file: they are the other two lines of the unit `systemd.rs` writes,
    // and neither is a thing a person is expected to type.
    match std::env::args().nth(3).as_deref() {
        None => start(streamed, slug),
        Some("--kill") => stop(streamed, slug),
        Some(INSIDE) => inside_cage(streamed, slug),
        Some(other) => die(&format!("unknown argument {other}")),
    }
}

/// Say why, where a person will find it, and stop.
///
/// stderr because a user unit's stderr is its journal: `journalctl --user -u
/// webdesk-app@gimp.service` is where somebody looking at a tile that will not
/// open ends up, and an exit code on its own would put them nowhere.
fn die(why: &str) -> ! {
    eprintln!("webdesk app-session: {why}");
    std::process::exit(2);
}

/// `ExecStart`: the compositor, with this binary inside it.
///
/// The last thing it does is `exec`, so the process systemd supervises as the
/// unit's main process is `cage` itself. One process fewer, and more usefully:
/// when `cage` exits -- which it does the moment its single client exits -- the
/// unit goes inactive by itself, with no wrapper left waiting to be reaped or
/// to be mistaken for a still-running app.
fn start(streamed: &'static Streamed, slug: &str) -> ! {
    use std::os::unix::process::CommandExt;

    let uid = nix::unistd::Uid::current().as_raw();
    if let Err(why) = prepare_socket(uid, slug) {
        die(&why);
    }
    clear_running(streamed);

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => die(&format!("could not find my own path: {e}")),
    };

    let Some(comp) = compositor() else {
        die("no headless compositor on this host -- install sway, or cage");
    };

    let mut child = Command::new(comp.bin());
    match comp {
        // Sway is configured rather than argued with, so the client it starts
        // and the way it is dressed both come out of one constant -- see
        // `SWAY_CONFIG`.
        Compositor::Sway => {
            let cfg = match write_sway_config(uid, slug, &exe) {
                Ok(p) => p,
                Err(e) => die(&e),
            };
            child.arg("--config").arg(cfg);
        }
        Compositor::Cage => {
            child
                // No client-side decorations. There is already a WebDesk window
                // around this with a title bar and a close button in it, and a
                // second frame inside the first is the giveaway that something
                // is being streamed rather than run.
                .arg("-d")
                // Everything after `--` is the client and its arguments.
                // Without it `--inside-cage` reads as an option of cage's, and
                // cage exits on it.
                .arg("--")
                .arg(&exe)
                .arg("app-session")
                .arg(slug)
                .arg(INSIDE);
        }
    }
    child
        // Without this, the compositor picks its backend from the environment
        // and would nest itself inside whatever session this user already has
        // -- a window on the machine's own screen, invisible here -- or fail on
        // a host with no seat at all. Headless is not a fallback for us; it is
        // the only backend that makes sense for an output nobody is sitting in
        // front of.
        .env("WLR_BACKENDS", "headless")
        // One output. wlroots defaults to one already, but the default is a
        // default and this is a requirement: `wayvnc` serves a single output,
        // and a second one would be a screen with no way to reach it.
        //
        // Its size is not set here and cannot be. wlroots brings a headless
        // output up at a hardcoded 1280x720; `WLR_HEADLESS_OUTPUTS` sets the
        // count and nothing sets the size, and neither compositor has a flag
        // for it. Under Sway it is changed afterwards, from inside the session
        // -- see `apply_size`. Under cage it is never changed at all.
        .env("WLR_HEADLESS_OUTPUTS", "1")
        // Belt for the same braces. `WAYLAND_DISPLAY` and `DISPLAY` in the
        // manager's environment are a live session of this user's on the
        // machine's own screen; leaving them set is how a compositor ends up
        // drawing there instead of here.
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY");

    // `exec` returns only on failure.
    let e = child.exec();
    die(&format!("could not run {}: {e}", comp.bin()));
}

/// Write the Sway configuration for this session and hand back its path.
///
/// Beside the sockets, in the same `0700` directory, and rewritten on every
/// open: it is derived entirely from the catalog entry and this binary's own
/// path, so a stale one is never worth keeping and a shared one would be a file
/// two sessions could disagree about.
fn write_sway_config(uid: u32, slug: &str, exe: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let path = crate::rfb::socket_dir(uid).join(format!("{slug}.sway"));
    let body = SWAY_CONFIG
        .replace("{exe}", &exe.to_string_lossy())
        .replace("{slug}", slug);
    std::fs::write(&path, body).map_err(|e| format!("could not write a sway config: {e}"))?;
    Ok(path)
}


/// `ExecStop`: take the application out of the scope it escaped into.
///
/// Everything else in the unit's cgroup -- the compositor, this binary's copy
/// inside it, the RFB server -- systemd kills on its own. This is the one
/// process it cannot see, for the reason `APP_UNIT` sets out at length, and
/// `flatpak kill` by id is the only handle that reaches it.
fn stop(streamed: &'static Streamed, slug: &str) -> ! {
    kill_app(streamed);
    // The compositor is being killed rather than asked to leave, so the RFB
    // server may not get as far as unlinking its own socket. A path in `/run`
    // with nothing behind it is what the next `wayvnc` would refuse to bind.
    let uid = nix::unistd::Uid::current().as_raw();
    let _ = std::fs::remove_file(crate::rfb::socket_path(uid, slug));
    std::process::exit(0);
}

/// `cage`'s one client: the RFB server, then the application.
///
/// **The ordering, and what was tried.** `wayvnc` attaches to the compositor
/// named by `WAYLAND_DISPLAY`, so it cannot be started first -- there is nothing
/// to attach to. `cage` exits when its client exits, so the application has to
/// be what it supervises, which appears to leave no room for a second process at
/// all.
///
/// The arrangement that was tried first was to start `cage` with the application
/// as its client, then poll `$XDG_RUNTIME_DIR` from outside for a new
/// `wayland-N` socket and start `wayvnc` against it. It is wrong twice over.
/// `cage` names its socket with `wl_display_add_socket_auto`, so the name is the
/// first free one and is not knowable in advance; and this user very plausibly
/// already has `wayland-0` from a session on the machine's own screen, so a
/// wrong guess does not fail -- it serves somebody's real desktop over a socket
/// WebDesk hands to a browser. That is not a race worth winning.
///
/// So `cage`'s client is this binary again. `cage` sets `WAYLAND_DISPLAY` in its
/// own environment before it spawns that client, so the display name arrives by
/// inheritance: no polling, no timeout, no guess, and no way to attach to a
/// compositor that is not the one just started. `wayvnc` then goes first, so
/// that a browser connecting while the application is still loading gets a
/// blank screen rather than a refused connection, and the application is run in
/// The Sway configuration a streamed session runs under.
///
/// **Two substitutions and no others**, for the reason `systemd::APP_UNIT` has
/// the same rule: a configuration assembled out of a request is a way to run
/// anything as anybody. `{exe}` is this binary's own path and `{slug}` has
/// already been matched against the catalog by the time anything gets here, so
/// neither is a name a request chose.
///
/// Sway is a tiling window manager and this is what makes it behave like the
/// kiosk `cage` is: no bar, no borders, nothing drawn that a WebDesk window
/// does not already draw around it. What it keeps that cage does not is
/// somewhere sensible to put a second window, which is the thing an application
/// opening a file dialog needs.
///
/// The trailing `swaymsg exit` is what `cage` did for free. cage ends when its
/// one client ends; Sway is a session and will happily sit there with nothing
/// in it, which would leave the unit active and the dock saying an application
/// is open after it has gone.
const SWAY_CONFIG: &str = "\
# Written by WebDesk for one application. Not a file to edit -- it is rewritten
# every time the app is opened.
default_border none
default_floating_border none
output * bg #000000 solid_color
exec \"{exe} app-session {slug} --inside-cage; swaymsg exit\"
";

/// The bounds a requested desktop size has to fall inside.
///
/// Not taste. A compositor asked for a zero-width output has undefined
/// behaviour and a browser that has just been minimised reports exactly that.
/// The upper bound is there because the size comes from a window somebody can
/// drag, and a framebuffer is width times height times four bytes.
const MIN_EDGE: u32 = 320;
const MAX_EDGE: u32 = 7680;

/// What a scale may be.
///
/// Below 1 an application is drawn smaller than the pixels it is given, which is
/// a way to make a screen useless that nobody asks for on purpose. The ceiling
/// is where a window large enough to be worth scaling has already run out of
/// logical room for anything to sit in.
const MIN_SCALE: f32 = 1.0;
const MAX_SCALE: f32 = 4.0;

/// Which headless compositor this host has, in order of preference.
///
/// **They are not equivalent, and the difference is the resolution.** Sway's
/// output can be resized while it runs, so an application follows the WebDesk
/// window. cage's cannot: `wlr-randr --custom-mode` against cage 0.1.5 trips
/// `wlr_scene_output_layout_add_output`'s assertion and the compositor dumps
/// core, taking the application with it -- measured on the deployment host, not
/// inferred, and the reason no version of this asks cage to resize anything.
///
/// cage stays because on Enterprise Linux 10 it is the only one packaged; Sway
/// is in no EPEL generation. A host with only cage runs drawn apps at the
/// compositor's default 1280x720 with the browser scaling them, which is what
/// this did before any of it could resize at all.
#[derive(Clone, Copy, PartialEq)]
pub enum Compositor {
    Sway,
    Cage,
}

impl Compositor {
    fn bin(self) -> &'static str {
        match self {
            Compositor::Sway => "sway",
            Compositor::Cage => "cage",
        }
    }

    /// Whether an application under this one can be given a size.
    fn resizes(self) -> bool {
        matches!(self, Compositor::Sway)
    }
}

/// The compositor to run, or `None` on a host with neither.
pub fn compositor() -> Option<Compositor> {
    for c in [Compositor::Sway, Compositor::Cage] {
        if crate::engine::which(c.bin()).is_some() {
            return Some(c);
        }
    }
    None
}

/// Set the one output to this size, through Sway's own IPC.
///
/// `swaymsg` rather than `wlr-randr`: it ships with Sway, so a host that can run
/// this at all can already do it, and it speaks to the compositor that is
/// running rather than to whichever one a Wayland socket happens to belong to.
///
/// `Err` is a sentence for the journal rather than something to act on. A
/// session whose output could not be resized is still a session; it draws at
/// what it came up at and the browser scales it, which is the outcome every
/// cage host gets all the time.
fn apply_size(w: u32, h: u32, scale: f32) -> Result<(), String> {
    if !(MIN_EDGE..=MAX_EDGE).contains(&w) || !(MIN_EDGE..=MAX_EDGE).contains(&h) {
        return Err(format!("{w}x{h} is outside what this will ask a compositor for"));
    }
    if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return Err(format!("a scale of {scale} is outside what this will ask for"));
    }
    // The framebuffer is what the browser draws one-for-one; the *logical* size
    // is what the application lays itself out in, and it is the framebuffer
    // divided by the scale. That is the number that can become unusable without
    // anything looking wrong from here -- 200% in a small window leaves an
    // application a few hundred points wide, and what it does then is hide its
    // toolbars and clip its dialogs rather than complain.
    let (lw, lh) = ((w as f32 / scale) as u32, (h as f32 / scale) as u32);
    if lw < MIN_EDGE || lh < MIN_EDGE {
        return Err(format!(
            "at {scale}x that window leaves the application {lw}x{lh} to lay out in, \
             which is smaller than anything usable -- make the window bigger or the \
             scale smaller"
        ));
    }
    // One command for both, so the output is never briefly at a new size with an
    // old scale. Sway applies them together and answers once.
    let out = Command::new("swaymsg")
        .args([
            "output",
            "*",
            "resolution",
            &format!("{w}x{h}"),
            "scale",
            &format!("{scale}"),
        ])
        .output()
        .map_err(|e| format!("could not run swaymsg: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// Read one `<width>x<height>` and nothing else.
///
/// The parsing is the validation. Two integers with one `x` between them is the
/// whole language this socket speaks, so anything else is refused before it
/// reaches an argument list -- which matters more than it looks, because the far
/// end of this socket is reachable by anything running as this user.
/// Read one `<width>x<height>` with an optional `@<scale>` after it.
///
/// The parsing is the validation. Two integers with an `x` between them, and at
/// most one decimal after an `@`, is the whole language this socket speaks --
/// anything else is refused before it reaches an argument list, which matters
/// more than it looks because the far end is reachable by anything running as
/// this user.
///
/// The scale is optional and defaults to 1 so that the older two-number form
/// still means what it always meant.
fn parse_size(line: &str) -> Option<(u32, u32, f32)> {
    let line = line.trim();
    let (dims, scale) = match line.split_once('@') {
        Some((d, s)) => (d, s.parse::<f32>().ok()?),
        None => (line, 1.0),
    };
    if !scale.is_finite() {
        return None;
    }
    let (w, h) = dims.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, scale))
}

/// Serve the control socket for as long as the session lives.
///
/// A thread rather than a task: this process has no async runtime and wants
/// none. It is one blocking accept in a loop and the work behind it finishes in
/// milliseconds.
///
/// Every failure is swallowed deliberately. The application is running and
/// visible by the time this exists, and a resize going wrong must not take down
/// the session drawing it.
fn serve_control(uid: u32, slug: &str, resizes: bool) {
    use std::io::{BufRead, BufReader, Write};

    let path = crate::rfb::control_path(uid, slug);
    let _ = std::fs::remove_file(&path);
    let listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("webdesk app-session: no control socket ({e}); this app will not resize");
            return;
        }
    };
    for stream in listener.incoming().flatten() {
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        });
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let mut stream = stream;
        let reply = match parse_size(&line) {
            None => "bad size".to_string(),
            Some(_) if !resizes => {
                "this compositor cannot resize its output; install sway".to_string()
            }
            Some((w, h, scale)) => match apply_size(w, h, scale) {
                Ok(()) => "ok".to_string(),
                Err(e) => e,
            },
        };
        let _ = writeln!(stream, "{reply}");
    }
}

/// the foreground here so that its exit is something this process can act on.
fn inside_cage(streamed: &'static Streamed, slug: &str) -> ! {
    let uid = nix::unistd::Uid::current().as_raw();
    let socket = crate::rfb::socket_path(uid, slug);
    // Asked again rather than passed down. `start` picked from the same list a
    // moment ago and nothing between the two can change what is installed, so
    // the answer is the same one -- and a second argument threaded through
    // `cage --` would have to be parsed back out of an argv that already
    // carries a sentinel.
    let comp = match compositor() {
        Some(c) => c,
        None => die("no headless compositor on this host"),
    };

    // `--unix-socket` makes the positional address a path instead of a host.
    // Verified against wayvnc's own option table rather than assumed: it has
    // been `-u, --unix-socket` since v0.5.0, so there is no version this host
    // could plausibly have where the flag is missing and a loopback port would
    // be needed instead. That matters because a port would put this back in
    // reach of the network, and `rfb.rs` chose a socket precisely so that
    // "unreachable from outside" is a fact about the filesystem.
    // `-S` is the difference between one streamed app working and two working.
    // See `rfb::wayvnc_control_path`.
    let ctl = crate::rfb::wayvnc_control_path(uid, slug);
    let _ = std::fs::remove_file(&ctl);
    let mut vnc = match Command::new("wayvnc")
        .arg("--socket")
        .arg(&ctl)
        .arg("--unix-socket")
        .arg(&socket)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => die(&format!("could not run wayvnc: {e} -- there is no way to see this app")),
    };

    // Sized before the application starts, so the first frame anybody sees is
    // already the right shape -- a window that arrives at 1280x720 and jumps a
    // moment later reads as a fault even when the second size is correct.
    //
    // A compositor that cannot resize is not an error and not silent. The
    // session runs at whatever the output came up at, which is exactly how this
    // behaved before any of it could resize, and the reason is in the journal
    // for whoever wonders why the window will not follow.
    let resizes = comp.resizes();
    if resizes {
        let (w, h) = (streamed.width as u32, streamed.height as u32);
        // Scale 1 to begin with. The browser is the only thing that knows what
        // somebody set last time, and it says so as soon as it connects.
        if let Err(e) = apply_size(w, h, 1.0) {
            eprintln!("webdesk app-session: could not set the output to {w}x{h}: {e}");
        }
    } else {
        eprintln!(
            "webdesk app-session: {} cannot resize its output, so this app is fixed at the \
             compositor's default and the browser will scale it. Install sway for a resolution \
             that follows the window.",
            comp.bin()
        );
    }

    // From here the browser owns the size. It knows how big its window is and
    // this process does not.
    {
        let slug = slug.to_string();
        std::thread::spawn(move || serve_control(uid, &slug, resizes));
    }

    // `flatpak run` and not the application's own command line: the id is the
    // only thing this file knows about it, which is the point of the id being
    // the only thing an entry has to write down.
    let status = Command::new("flatpak").args(["run", streamed.flatpak.id]).status();

    // The application has gone, so nothing may be left holding the socket.
    // SIGTERM rather than a kill: wayvnc unlinks its own socket and drops its
    // clients on the way out, so the browser sees a closed connection instead of
    // a stall, and the `remove_file` below is only there for the case where it
    // did not get that far.
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(vnc.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    );
    let _ = vnc.wait();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(crate::rfb::control_path(uid, slug));
    let _ = std::fs::remove_file(&ctl);

    // Exiting is what makes `cage` exit, which is what makes the unit inactive
    // and the tile stop saying the app is open. The application's own code is
    // carried through so that a crash reads as `failed` in the dock and a quit
    // reads as `exited`.
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(0)),
        Err(e) => die(&format!("could not run flatpak: {e}")),
    }
}

/// The directory this session serves in, and nothing left over in it.
///
/// `0700` is re-applied rather than assumed. The directory is made by the
/// privileged side in `systemd::install_app_template`, which is the only half
/// that can create anything under `/run/webdesk`; this end owns it by then, so
/// the mode is cheap to insist on and the cost of being wrong about it is one
/// user reading another's screen.
///
/// A leftover socket is the ordinary aftermath of a session that was killed
/// rather than stopped. `wayvnc` will not bind an address that already exists,
/// so without this a single hard kill would make the app unopenable until
/// somebody logged in and deleted a file.
fn prepare_socket(uid: u32, slug: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = crate::rfb::socket_dir(uid);
    if !dir.is_dir() {
        // Not created here on purpose. The parent is root's, so this is not a
        // directory an unprivileged process can make, and a session that
        // reaches this point was started by something other than WebDesk --
        // by hand, or by a unit left enabled. Saying which half is missing is
        // more use than a permission error on a path nobody recognises.
        return Err(format!(
            "{} is not there -- WebDesk makes it when the app is opened, so this session \
             was started by something else",
            dir.display()
        ));
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("could not make {} private: {e}", dir.display()))?;
    let _ = std::fs::remove_file(crate::rfb::socket_path(uid, slug));
    Ok(())
}

/// Kill this application if it is already running for this user, and say so.
///
/// **This can take something away, and it is the lesser of two costs.**
/// `flatpak kill` names an application id, not an instance, so there is no way
/// to kill only the one a previous session left behind. If the person has this
/// same app open on the machine's own screen, opening it here closes it there.
///
/// The alternative is worse in two directions. A start that begins with an
/// instance already alive draws nothing at all: most desktop applications are
/// single-instance over the session bus, so `flatpak run` hands the request to
/// the copy that exists and returns, `cage`'s only client is gone within the
/// second, and the unit goes inactive behind an empty window. And an instance
/// orphaned by a stop that did not finish can never be cleared from the
/// browser, because systemd does not run `ExecStop` for a unit that is already
/// inactive -- so without this there is no way back at all short of a shell.
///
/// Same user, same files, and it is announced before it happens, into the
/// journal of the unit that did it.
fn clear_running(streamed: &'static Streamed) {
    if !kill_app(streamed) {
        return;
    }
    eprintln!(
        "webdesk app-session: {} was already running for you and has been closed, so that this \
         session has a window to draw",
        streamed.flatpak.id
    );
    // The next `flatpak run` asks the session bus whether the application is
    // already there, and the name is released when the process dies rather than
    // when `flatpak kill` returns. This is a guess at how long that takes and
    // not a measurement -- it is only ever paid on the path that just killed
    // something, which is the rare one.
    std::thread::sleep(std::time::Duration::from_millis(300));
}

/// `flatpak kill <id>`, reporting whether there was anything to kill.
///
/// Non-zero is the ordinary answer -- it is what "not running" looks like --
/// so it is a `false` here and never an error.
fn kill_app(streamed: &'static Streamed) -> bool {
    Command::new("flatpak")
        .args(["kill", streamed.flatpak.id])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slug that is in no catalog is refused before anything is spawned. This
    /// is the browser-facing half of the rule: `apps.rs` may decide *whether* a
    /// session starts, and this is where "for what" stops being negotiable.
    #[test]
    fn a_slug_that_is_not_in_the_catalog_is_refused() {
        assert!(resolve("definitely-not-an-app").is_err());
        // Including the shapes somebody would reach for if they were trying.
        assert!(resolve("").is_err());
        assert!(resolve("org.gimp.GIMP").is_err());
        assert!(resolve("../../etc/passwd").is_err());
    }

    /// A real catalog slug that is not a streamed entry is refused too, and it
    /// has to be: a compositor started for a container app would come up, find
    /// nothing to draw and exit, leaving a tile that says `failed` for a reason
    /// nobody could work out from the journal.
    #[test]
    fn a_catalog_entry_that_is_not_streamed_is_refused() {
        let mut checked = 0;
        for app in crate::catalog::CATALOG.iter().filter(|a| a.streamed.is_none()) {
            let Err(err) = resolve(app.slug) else {
                panic!("{} is not streamed and must not resolve", app.slug);
            };
            // The message says which kind it is, because the answer to "why did
            // my app not open" is different for the two.
            assert!(err.contains(app.slug), "{err} does not say what was asked for");
            checked += 1;
        }
        assert!(checked > 0, "the catalog has no container entries left to check against");
    }

    /// Every streamed entry resolves, and resolves to an application id there is
    /// something to run. An entry added with an empty id would install, start a
    /// compositor, and fail inside `flatpak run` where nothing is watching.
    #[test]
    fn every_streamed_entry_resolves_to_something_runnable() {
        for app in crate::catalog::CATALOG.iter().filter(|a| a.streamed.is_some()) {
            let Ok((found, streamed)) = resolve(app.slug) else {
                panic!("{} is a streamed entry and must resolve", app.slug);
            };
            assert_eq!(found.slug, app.slug);
            assert!(!streamed.flatpak.id.is_empty(), "{} names no application", app.slug);
            // Nothing else to run it: no image to pull and no port to publish.
            // A streamed entry that also had those would be two entries.
            assert!(app.image.is_empty(), "{} is a container as well", app.slug);
            assert_eq!(app.port, 0, "{} publishes a port nothing would serve", app.slug);
        }
    }

    /// The three roles are three distinct words, and none of them is a slug.
    /// `--inside-cage` colliding with a catalog slug would make `cage` run the
    /// start path again, forking compositors until the host gave up.
    #[test]
    fn the_argument_after_a_slug_can_never_be_mistaken_for_one() {
        for word in [INSIDE, "--kill"] {
            assert!(word.starts_with("--"));
            assert!(crate::catalog::find(word).is_none(), "{word} is also a catalog slug");
        }
    }
}
