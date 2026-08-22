fn main() {
    println!("cargo:rerun-if-changed=build.rs");
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
    .any(|p| std::path::Path::new(p).exists());

    if devel {
        println!("cargo:rustc-link-lib=dylib=pam");
    } else {
        println!("cargo:rustc-link-lib=dylib:+verbatim=libpam.so.0");
    }
}
