//! aygent_runner — headless Cloud Shell / Cloud Runs worker (1.1.0 M4).
//!
//! Same runtime as the desktop app, no Tauri shell: polls its leases/jobs,
//! executes with the shared broker, pushes snapshots back to R2.
//!
//! STATUS: links the runtime (broker, exec, … now pub) — job loop lands next.

use aygent_lib::broker::Broker;
use aygent_lib::exec::ExecBroker;

fn main() {
    // Linkage proof: these types come from the SAME modules the app uses.
    let _ = std::mem::size_of::<Broker>();
    let _ = std::mem::size_of::<ExecBroker>();
    println!("aygent_runner 1.1.0 (runtime linked, job loop TODO)");
}
