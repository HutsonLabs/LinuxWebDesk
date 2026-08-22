//! The per-session helper.
//!
//! The daemon runs as root so it can authenticate through PAM. It must never
//! touch a user's files with those privileges. Instead, each authenticated
//! session re-executes this binary as a child, permanently drops to the
//! authenticated user, and every filesystem operation for that session is
//! performed by that child. The kernel then enforces permissions for us, which
//! is the whole point -- there is no permission logic in this program to get
//! wrong.

use crate::proto::{Channel, Request, Response};
use serde_json::json;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Refuse to read a single file larger than this into memory.
const MAX_READ: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------- child side

/// Entry point for `rockywebde --helper`. Never returns to the caller's flow.
pub fn run_child(fd: i32) -> ! {
    use std::os::unix::io::FromRawFd;
    // Safety: fd 3 is the socketpair end the parent handed us across exec.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let mut ch = match Channel::new(stream) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("helper: channel setup failed: {e}");
            std::process::exit(1);
        }
    };

    loop {
        let (req, payload): (Request, Vec<u8>) = match ch.recv() {
            Ok(v) => v,
            Err(_) => std::process::exit(0), // parent went away
        };
        let (resp, out) = dispatch(&req, &payload);
        if ch.send(&resp, &out).is_err() {
            std::process::exit(0);
        }
    }
}

fn dispatch(req: &Request, payload: &[u8]) -> (Response, Vec<u8>) {
    match req.op.as_str() {
        "home" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            (Response::ok_data(json!({ "home": home })), Vec::new())
        }
        "list" => match list_dir(Path::new(&req.path)) {
            Ok(v) => (Response::ok_data(v), Vec::new()),
            Err(e) => (Response::err(e), Vec::new()),
        },
        "read" => match read_file(Path::new(&req.path)) {
            Ok(bytes) => (Response::ok_bytes(bytes.len()), bytes),
            Err(e) => (Response::err(e), Vec::new()),
        },
        "write" => match std::fs::write(&req.path, payload) {
            Ok(()) => (Response::ok_data(json!({})), Vec::new()),
            Err(e) => (Response::err(e), Vec::new()),
        },
        "mkdir" => match std::fs::create_dir(&req.path) {
            Ok(()) => (Response::ok_data(json!({})), Vec::new()),
            Err(e) => (Response::err(e), Vec::new()),
        },
        "remove" => match remove(Path::new(&req.path)) {
            Ok(()) => (Response::ok_data(json!({})), Vec::new()),
            Err(e) => (Response::err(e), Vec::new()),
        },
        "rename" => match std::fs::rename(&req.path, &req.to) {
            Ok(()) => (Response::ok_data(json!({})), Vec::new()),
            Err(e) => (Response::err(e), Vec::new()),
        },
        other => (Response::err(format!("unknown op {other}")), Vec::new()),
    }
}

fn remove(p: &Path) -> std::io::Result<()> {
    if p.is_dir() && !p.is_symlink() {
        std::fs::remove_dir(p) // deliberately non-recursive
    } else {
        std::fs::remove_file(p)
    }
}

fn read_file(p: &Path) -> std::io::Result<Vec<u8>> {
    let md = std::fs::metadata(p)?;
    if md.len() > MAX_READ {
        return Err(std::io::Error::other(format!(
            "file is {} bytes; limit is {MAX_READ}",
            md.len()
        )));
    }
    std::fs::read(p)
}

fn list_dir(p: &Path) -> std::io::Result<serde_json::Value> {
    let canon: PathBuf = p.canonicalize()?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&canon)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        // Symlinks are reported as links but not followed for metadata, so a
        // dangling link lists cleanly instead of erroring the whole directory.
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                entries.push(json!({"name": name, "kind": "unknown"}));
                continue;
            }
        };
        let kind = if md.is_dir() { "dir" } else if md.is_symlink() { "link" } else { "file" };
        entries.push(json!({
            "name": name,
            "kind": kind,
            "size": md.len(),
            "mode": format!("{:o}", md.permissions().mode() & 0o7777),
            "mtime": md.mtime(),
        }));
    }
    entries.sort_by(|a, b| {
        let ka = a["kind"].as_str().unwrap_or("");
        let kb = b["kind"].as_str().unwrap_or("");
        let da = (ka == "dir") as u8;
        let db = (kb == "dir") as u8;
        db.cmp(&da).then_with(|| {
            a["name"].as_str().unwrap_or("").to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    let parent = canon.parent().map(|p| p.to_string_lossy().to_string());
    Ok(json!({ "path": canon.to_string_lossy(), "parent": parent, "entries": entries }))
}

// --------------------------------------------------------------- parent side

pub struct Helper {
    ch: Channel,
    child: std::process::Child,
}

impl Helper {
    /// Fork this binary, drop to `uid`/`gid`, and keep the pipe open.
    pub fn spawn(username: &str, uid: u32, gid: u32, home: &str) -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::process::CommandExt;

        let (parent_end, child_end) = UnixStream::pair()?;
        let exe = std::env::current_exe()?;
        let cname = std::ffi::CString::new(username).map_err(std::io::Error::other)?;
        let child_fd = child_end.as_raw_fd();

        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--helper")
            .env_clear()
            .env("HOME", home)
            .env("USER", username)
            .env("LOGNAME", username)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir(home);

        unsafe {
            cmd.pre_exec(move || {
                // Order matters: supplementary groups and gid must be set while
                // still privileged, and setuid must come last.
                nix::unistd::setgid(nix::unistd::Gid::from_raw(gid))?;
                // Via libc: nix does not expose initgroups on every target.
                if libc::initgroups(cname.as_ptr(), gid as _) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                nix::unistd::setuid(nix::unistd::Uid::from_raw(uid))?;
                // Move the socket to fd 3 where the child expects it.
                if child_fd != 3 {
                    nix::unistd::dup2(child_fd, 3)?;
                }
                Ok(())
            });
        }

        let child = cmd.spawn()?;
        drop(child_end);
        Ok(Helper { ch: Channel::new(parent_end)?, child })
    }

    pub fn request(&mut self, req: &Request, payload: &[u8]) -> std::io::Result<(Response, Vec<u8>)> {
        self.ch.send(req, payload)?;
        self.ch.recv()
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
