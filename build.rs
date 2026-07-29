//! Stamp the commit into the binary. The crate version moves slowly and the
//! machine moves nightly, so "adhd 0.1.0" alone cannot answer the only
//! question `--version` is ever asked: which build is this?

use std::process::Command;

fn main() {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=ADHD_BUILD={hash}");
    // Re-stamp when the checked-out commit changes, not on every build.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}
