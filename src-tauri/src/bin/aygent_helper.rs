//! AYGENT CEF helper subprocess (Phase 1, engine-cef only).
//!
//! CEF uses Chromium's multi-process model: the browser process (AYGENT itself)
//! spawns GPU / Renderer / Plugin / Alerts subprocesses. On macOS every one of
//! those subprocesses is a SEPARATE `.app` (Helper (GPU).app, etc.) whose
//! executable is THIS binary. Each helper does exactly one thing: load the CEF
//! framework as a helper, init the API version, and call `execute_process`,
//! which blocks running the subprocess's message loop until CEF tears it down.
//!
//! This is copied VERBATIM (structurally) from cef-rs's proven
//! `cefsimple_helper.rs` — the Phase-0 spike validated this exact shape. Do NOT
//! add app logic here; the helper must stay a pure CEF subprocess entry point.
//!
//! Built ONLY with `--features engine-cef` (see Cargo.toml `[[bin]]`
//! required-features). A plain build never compiles this file.

#[cfg(all(target_os = "macos", feature = "engine-cef"))]
fn main() {
    use cef::{args::Args, *};

    let args = Args::new();

    // Sandbox: the helper participates in the CEF macOS sandbox. Must be
    // constructed BEFORE the library loads and kept alive for the process life.
    #[cfg(feature = "sandbox")]
    let _sandbox = {
        let mut sandbox = cef::sandbox::Sandbox::new();
        sandbox.initialize(args.as_main_args());
        sandbox
    };

    // Load the framework via the HELPER resolver (../../.. up out of the
    // Helper.app/Contents/MacOS into Contents/Frameworks of the OUTER app).
    let _loader = {
        let loader = library_loader::LibraryLoader::new(&std::env::current_exe().unwrap(), true);
        assert!(loader.load(), "[aygent][cef][helper] failed to load CEF framework");
        loader
    };

    // Pin the API version (must match the browser process).
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    // Run the subprocess. Blocks until CEF exits this subprocess.
    execute_process(
        Some(args.as_main_args()),
        None::<&mut App>,
        std::ptr::null_mut(),
    );
}

// A non-CEF / non-macOS build still needs SOMETHING to compile if the target
// selection ever includes this bin. required-features gates it off in practice,
// but keep a trivial main so an accidental include is a clean no-op, not an
// error.
#[cfg(not(all(target_os = "macos", feature = "engine-cef")))]
fn main() {}
