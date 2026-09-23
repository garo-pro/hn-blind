//! Updating hn-blind in place from its GitHub releases, through Trypsynth's `ship-shape`.
//!
//! ship-shape drives the whole flow with its own native dialogs — the release notes in a read-only text field, a progress dialog that can be cancelled, then a hidden PowerShell that unpacks the zip over the executable once this process has exited and starts the new version. A screen reader reads all of it like any other dialog. What this module adds is configuration, and one thing ship-shape leaves to the application: making sure the version that replaces this one opens with focus.
//!
//! Windows only. The other platforms' releases are tarballs, which ship-shape has no way to install, and on macOS it would want a signed, notarised disk image first.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ship_shape::{UpdateChannel, UpdaterConfig};
use wxdragon::prelude::*;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit this binary was built from, as the seven-character short hash the development channel compares. Empty when `build.rs` could not find one. See `build.rs`.
const CURRENT_COMMIT: &str = env!("HN_BLIND_COMMIT");

const GITHUB_REPO: &str = "garo-pro/hn-blind";

/// The public half of the key the release and dev workflows sign the Windows zip with (the `MINISIGN_KEY` secret). A download that does not verify against it is deleted, never unpacked, so this line is what stands between a compromised release and every user's machine — change it only together with that secret.
const MINISIGN_PUBLIC_KEY: &str = "RWRMdwNGi4KYZEDEtn+bnjgM6ZDCemzo1TKkEdaEqhvzXFnFp+pjJXpE";

/// The release workflow names each asset after its platform, and this is the part ship-shape inserts between `hn-blind` and `.zip` to find the one for this build. There is no aarch64 Windows release yet; asking for one anyway gets an honest "no matching download" rather than the x86_64 zip, which would not run.
#[cfg(target_arch = "aarch64")]
const ASSET_SUFFIX: &str = "-windows-aarch64";
#[cfg(not(target_arch = "aarch64"))]
const ASSET_SUFFIX: &str = "-windows-x86_64";

/// Check for a newer release and, if there is one, let ship-shape offer it, download it and install it. Returns at once; the check runs on ship-shape's own thread, and a second call while one is under way does nothing.
///
/// `dev` checks the rolling `latest` pre-release that the dev workflow rebuilds from every push to main, and compares commits rather than versions; otherwise only tagged releases count. `silent` is for the check at startup: ship-shape then shows nothing unless there is an update to offer, not even an error, so being offline never greets anyone with a dialog.
pub fn check(frame: &Frame, dev: bool, silent: bool) {
    let config = UpdaterConfig::new(
        GITHUB_REPO,
        "hn-blind",
        "hn-blind",
        MINISIGN_PUBLIC_KEY,
        format!("hn-blind/{CURRENT_VERSION}"),
    )
    .with_asset_suffix(ASSET_SUFFIX);

    let channel = if dev { UpdateChannel::Dev } else { UpdateChannel::Stable };

    keep_granting_foreground(frame.get_handle() as usize);
    // Not an installer build: releases are a bare zip.
    ship_shape::ui::run_update_check(
        Arc::new(config),
        frame.handle_ptr() as usize,
        CURRENT_VERSION,
        CURRENT_COMMIT,
        false,
        channel,
        silent,
    );
}

#[link(name = "user32")]
unsafe extern "system" {
    fn AllowSetForegroundWindow(process_id: u32) -> i32;
    fn IsWindowEnabled(window: *mut c_void) -> i32;
}

/// `ASFW_ANY`: any process may take the foreground, not just one named by id.
const ASFW_ANY: u32 = u32::MAX;

/// Keep passing this process's right to the foreground on for as long as ship-shape's dialogs are up, so the version that replaces this one can open with focus.
///
/// The new version is started by a windowless PowerShell rather than by anything the user touched, so Windows would open its window behind whatever else is running — and to someone who cannot see the screen, an application that restarted without focus is one that silently vanished. This process has the foreground while the user is answering its dialogs, and `AllowSetForegroundWindow` hands that right on; `ASFW_ANY` because the process that spends it is the PowerShell's child, whose id is not known here.
///
/// Once is not enough. The grant is revoked by the next input, and the download runs for seconds with the user free to press anything, so it is re-issued every second until the end. ship-shape exits the process straight from its install step with no hook for the application, so "the end" is inferred: every one of its dialogs is modal, and a modal dialog disables the main window, which is something another thread can ask Windows about without touching wxWidgets. This is the approach Paperback takes with the same crate.
fn keep_granting_foreground(frame_hwnd: usize) {
    const BEAT: Duration = Duration::from_secs(1);
    /// Long enough to bridge the one moment in a real update when no dialog is up: between the release notes closing and the progress dialog opening.
    const QUIET_BEATS: u32 = 3;
    /// How long to wait for a first dialog. A check has to go to GitHub before it shows anything, and giving up before it does would mean no grant at all.
    const START_TIMEOUT: Duration = Duration::from_mins(3);

    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    thread::spawn(move || {
        let started = Instant::now();
        let mut in_update = false;
        let mut quiet = 0;
        loop {
            thread::sleep(BEAT);
            // SAFETY: `IsWindowEnabled` only reads the window's state, is safe from any thread, and returns false rather than failing for a handle that no longer names a window.
            if unsafe { IsWindowEnabled(frame_hwnd as *mut c_void) } == 0 {
                in_update = true;
                quiet = 0;
            } else if in_update {
                quiet += 1;
                if quiet >= QUIET_BEATS {
                    break;
                }
            } else if started.elapsed() >= START_TIMEOUT {
                break;
            }
            if in_update {
                // SAFETY: takes no pointers. A refusal only means the new window may open without focus; there is nothing better to do about it here.
                unsafe {
                    AllowSetForegroundWindow(ASFW_ANY);
                }
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}
