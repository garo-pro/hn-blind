// SPDX-License-Identifier: MPL-2.0

//! Forward the delay-load list that `prism-sys` publishes into link arguments for our binary.
//!
//! This only has anything to do for a static MSVC build — the release workflow, which sets `PRISM_STATIC=1`. A statically linked Prism puts the screen-reader import libraries into our link, and those DLLs ship with the screen readers rather than with Windows, so without `/delayload` the executable hard-imports them and will not start on a machine that has none of them installed. Cargo cannot propagate link arguments through a dependency, so `prism-sys` publishes the names as `links` metadata and every crate that links a binary repeats these few lines; see `crates/prism-sys/README.md` in prism2rust. `DEP_PRISM_DELAYLOAD` reaches only *direct* dependents of `prism-sys`, which is why `Cargo.toml` depends on it alongside the safe `prism` wrapper.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

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
