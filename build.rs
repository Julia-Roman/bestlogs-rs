use std::process::Command;

fn main() {
    // rust-embed doesn't register this directory with cargo's change tracking
    // on its own, so a `cargo build` after a frontend rebuild silently keeps
    // serving the stale embedded assets unless we do it ourselves.
    println!("cargo:rerun-if-changed=frontend/build");

    // The Nix build's source has no `.git` in it (see `backendSrc` in
    // flake.nix), so asking git there yields nothing and the commit shows as
    // "unknown" in /meta and the site footer. Nix passes the revision in
    // through the environment instead, so prefer that when it's set and only
    // shell out to git for local `cargo build`s.
    println!("cargo:rerun-if-env-changed=GIT_COMMIT_HASH");
    let commit = std::env::var("GIT_COMMIT_HASH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(git_short_rev)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_COMMIT_HASH={commit}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}

fn git_short_rev() -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
