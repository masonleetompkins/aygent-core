//! aygent_runner — headless Cloud Shell / Cloud Runs worker (1.1.0 M4).
//!
//! Same runtime as the desktop app, no Tauri shell: polls its leases/jobs,
//! executes with the shared broker, pushes snapshots back to R2.
//!
//! STATUS: plumbing stub. The runtime modules (broker, exec, …) are still
//! private to the lib — each gets exposed behind `pub` incrementally, and this
//! main() grows a job loop as they land. A plain build never compiles this
//! file (see Cargo.toml `[[bin]]` required-features, same pattern as
//! aygent_helper).

fn main() {
    println!("aygent_runner 1.1.0-headless-stub (no job loop yet)");
}
