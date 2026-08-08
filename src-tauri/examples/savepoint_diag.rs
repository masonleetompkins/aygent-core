// DIAG: run the real snapshot on a real agent folder; print the exact result.
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: savepoint_diag <folder>"));
    match aygent_lib::savepoint::snapshot(&root, "diagnostic snapshot") {
        Ok(Some(sha)) => println!("OK: new save point {sha}"),
        Ok(None) => println!("OK: tree identical (no changes to capture)"),
        Err(e) => println!("ERR: {e}"),
    }
}
