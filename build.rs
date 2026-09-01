use std::process::Command;

fn main() {
    // rust-embed doesn't register this directory with cargo's change tracking
    // on its own, so a `cargo build` after a frontend rebuild silently keeps
    // serving the stale embedded assets unless we do it ourselves.
    println!("cargo:rerun-if-changed=frontend/build");

    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_COMMIT_HASH={commit}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
