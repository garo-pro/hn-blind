// SPDX-License-Identifier: MPL-2.0

//! Stamp the build with its commit and update channel, and forward the delay-load list that `prism-sys` publishes into link arguments for our binary.
//!
//! The delay-load forwarding only has anything to do for a static MSVC build — the release workflow, which sets `PRISM_STATIC=1`. A statically linked Prism puts the screen-reader import libraries into our link, and those DLLs ship with the screen readers rather than with Windows, so without `/delayload` the executable hard-imports them and will not start on a machine that has none of them installed. Cargo cannot propagate link arguments through a dependency, so `prism-sys` publishes the names as `links` metadata and every crate that links a binary repeats these few lines; see `crates/prism-sys/README.md` in prism2rust. `DEP_PRISM_DELAYLOAD` reaches only *direct* dependents of `prism-sys`, which is why `Cargo.toml` depends on it alongside the safe `prism` wrapper.

use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    stamp_build();

    let Ok(dlls) = env::var("DEP_PRISM_DELAYLOAD") else {
        return;
    };

    for dll in dlls.split(';').filter(|d| !d.is_empty()) {
        println!("cargo:rustc-link-arg=/delayload:{dll}");
    }
    // Match upstream: unloading a delay-loaded module is allowed.
    println!("cargo:rustc-link-arg=/DELAY:unload");
    // The Orca and speech-dispatcher bridges are Unix-only, so some /DELAYLOAD entries go unreferenced; LNK4199 for those is expected rather than a problem.
    println!("cargo:rustc-link-arg=/ignore:4199");
}

/// Set `HN_BLIND_COMMIT` and `HN_BLIND_CHANNEL` for the crate, both always defined so the code reading them can use `env!` and never has to guess at a missing one.
///
/// The development update channel tells builds apart by commit, not version: ship-shape compares the first seven characters of this hash against the `- <hash> <subject>` lines of the rolling `latest` release's notes, which the dev workflow writes. `GITHUB_SHA` wins over asking git because CI knows the commit even when a checkout is shallow; a build with neither (a source tarball) gets an empty hash, which ship-shape treats as already up to date rather than offering an endless loop of "updates".
fn stamp_build() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=HN_BLIND_CHANNEL");

    let commit = env::var("GITHUB_SHA")
        .ok()
        .filter(|sha| !sha.is_empty())
        .or_else(git_head)
        .unwrap_or_default();
    println!("cargo:rustc-env=HN_BLIND_COMMIT={}", commit.get(..7).unwrap_or(&commit));

    let channel = env::var("HN_BLIND_CHANNEL").unwrap_or_default();
    println!("cargo:rustc-env=HN_BLIND_CHANNEL={channel}");
}

/// The checked-out commit, and a rerun whenever it moves: `.git/HEAD` changes on a checkout, the branch's own ref file on a commit, and `packed-refs` when git has packed that ref away.
fn git_head() -> Option<String> {
    let git = Path::new(".git");
    if git.is_dir() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        println!("cargo:rerun-if-changed=.git/packed-refs");
        let head = std::fs::read_to_string(git.join("HEAD")).unwrap_or_default();
        if let Some(branch) = head.strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=.git/{}", branch.trim());
        }
    }
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !sha.is_empty()).then_some(sha)
}
