use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    version_metadata();
    link_pam();
}

/// Bake in what the running binary needs to answer "which build am I, and what
/// am I tracking?" -- the update check compares these against the remote.
///
/// A release tarball has no `.git`, so `.lwd-source` (written by bootstrap.sh
/// and by the updater before it rebuilds) is the authoritative source when it
/// exists; git is only the fallback for a working copy.
fn version_metadata() {
    println!("cargo:rerun-if-changed=.lwd-source");
    println!("cargo:rerun-if-changed=.git/HEAD");

    let (mut commit, mut git_ref) = (None, None);
    if let Ok(text) = std::fs::read_to_string(".lwd-source") {
        for line in text.lines() {
            match line.split_once('=') {
                Some(("commit", v)) => commit = Some(v.trim().to_string()),
                Some(("ref", v)) => git_ref = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }

    let commit = commit.filter(|c| !c.is_empty()).unwrap_or_else(git_commit);
    let git_ref = git_ref.filter(|r| !r.is_empty()).unwrap_or_else(|| "main".into());

    // Seconds since the epoch. Formatting a date properly would mean a date
    // crate; the browser can do it from this for free.
    let built = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    println!("cargo:rustc-env=LWD_COMMIT={commit}");
    println!("cargo:rustc-env=LWD_REF={git_ref}");
    println!("cargo:rustc-env=LWD_BUILT={built}");
}

fn git_commit() -> String {
    let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).output();
    let Ok(out) = out else { return "unknown".into() };
    if !out.status.success() {
        return "unknown".into();
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        return "unknown".into();
    }
    // A dirty tree is not the commit it claims to be; say so, so the update
    // check does not report a local build as up to date with the remote.
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty {
        format!("{sha}-dirty")
    } else {
        sha
    }
}

fn link_pam() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    // Prefer the -devel symlink when it exists. When it does not, link the
    // versioned soname verbatim -- every PAM-enabled Linux ships libpam.so.0,
    // so this builds on a stock host with no extra packages.
    let devel = [
        "/usr/lib64/libpam.so",
        "/usr/lib/libpam.so",
        "/usr/lib/x86_64-linux-gnu/libpam.so",
        "/usr/lib/aarch64-linux-gnu/libpam.so",
    ]
    .iter()
    .any(|p| Path::new(p).exists());

    if devel {
        println!("cargo:rustc-link-lib=dylib=pam");
    } else {
        println!("cargo:rustc-link-lib=dylib:+verbatim=libpam.so.0");
    }
}
