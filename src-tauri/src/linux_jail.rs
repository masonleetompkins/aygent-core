// AYGENT — Linux Folder Mode jail (Landlock), the Linux counterpart of
// seatbelt/folder-mode.sb.
//
// The daemon must not be able to touch the user's files or run programs on
// its own: every file op goes through the Rust broker, every process through
// the exec broker. On macOS sandbox-exec enforces that; on Linux, Landlock
// (a kernel LSM since 5.13, no install, no root) does.
//
// How it is applied: the supervisor launches THIS app binary with
// `--aygent-landlock-exec <daemon_dir> <node> <entry>`. main() calls
// maybe_launch() before anything else, which restricts the (still
// single-threaded) process and then execs node, so the daemon is confined
// from its first instruction, and nothing runs between fork and exec in the
// multi-threaded app.
//
// What the jail allows (mirroring the Seatbelt profile):
//   - execute: only the node binary (and its ELF loader, which exec opens)
//   - read: node's install prefix (its libs, ICU), the daemon's own code (dist/ + node_modules/), system libraries and
//     config (/usr, /lib, /lib64, /etc, /opt), /proc and /sys (node's runtime
//     probes)
//   - read + write: /dev/null, /dev/zero, /dev/random, /dev/urandom, /dev/tty
//   - nothing else: not the home folder, not /tmp, not the agent folders, and
//     no executing anything but node (Landlock's Execute right).
// Network is not restricted (Seatbelt allows it too: loopback to the broker,
// and the daemon's gated net.http calls).
//
// If the kernel has no Landlock (older than 5.13, or disabled), the daemon
// still starts, unconfined, and says so: the launcher prints
// `AYGENT_JAIL=none` and the app shows a warning (daemon_info → jail).

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use landlock::{
    Access, AccessFs, BitFlags, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
};

/// The launcher flag (first argument) the supervisor starts us with.
pub const FLAG: &str = "--aygent-landlock-exec";

/// If this process was started as the jail launcher, confine it and exec
/// node; never returns in that case. Otherwise returns immediately.
pub fn maybe_launch() {
    let mut args = std::env::args_os();
    let _exe = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return;
    }
    let rest: Vec<OsString> = args.collect();
    if rest.len() < 3 {
        eprintln!("[aygent] jail launcher: expected <daemon_dir> <node> <entry...>");
        std::process::exit(2);
    }
    let daemon_dir = PathBuf::from(&rest[0]);
    let node_bin = PathBuf::from(&rest[1]);

    match restrict(&daemon_dir, &node_bin) {
        Ok(RulesetStatus::FullyEnforced) => eprintln!("AYGENT_JAIL=landlock"),
        Ok(RulesetStatus::PartiallyEnforced) => eprintln!("AYGENT_JAIL=landlock (partial: older kernel ABI)"),
        Ok(RulesetStatus::NotEnforced) => eprintln!("AYGENT_JAIL=none (this kernel has no Landlock; the daemon runs unconfined)"),
        Err(e) => eprintln!("AYGENT_JAIL=none (Landlock failed: {e}; the daemon runs unconfined)"),
    }

    let err = Command::new(&node_bin).args(&rest[2..]).exec();
    eprintln!("[aygent] jail launcher: could not start node ({}): {err}", node_bin.display());
    std::process::exit(127);
}

/// The program interpreter (PT_INTERP, e.g. /lib64/ld-linux-x86-64.so.2) of a
/// 64-bit little-endian ELF binary, or None.
fn elf_interpreter(bin: &Path) -> Option<PathBuf> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(bin).ok()?;
    let mut h = [0u8; 64];
    f.read_exact(&mut h).ok()?;
    if &h[0..4] != b"\x7fELF" || h[4] != 2 || h[5] != 1 {
        return None; // not ELF64 little-endian
    }
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (phoff, phentsize, phnum) = (u64_at(&h, 0x20), u16_at(&h, 0x36), u16_at(&h, 0x38));
    for i in 0..phnum.min(64) {
        let mut ph = vec![0u8; phentsize as usize];
        f.seek(SeekFrom::Start(phoff + i * phentsize)).ok()?;
        f.read_exact(&mut ph).ok()?;
        if u32_at(&ph, 0) == 3 {
            // PT_INTERP: a NUL-terminated path.
            let (off, size) = (u64_at(&ph, 0x08), u64_at(&ph, 0x20).min(4096));
            let mut path = vec![0u8; size as usize];
            f.seek(SeekFrom::Start(off)).ok()?;
            f.read_exact(&mut path).ok()?;
            let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            return Some(PathBuf::from(String::from_utf8_lossy(&path[..end]).into_owned()));
        }
    }
    None
}

fn restrict(daemon_dir: &Path, node_bin: &Path) -> Result<RulesetStatus, landlock::RulesetError> {
    // Best effort: the newest ABI this crate knows, degraded to what the
    // running kernel supports (the status says how much was enforced).
    let abi = ABI::V5;
    let all = AccessFs::from_all(abi);
    let read: BitFlags<AccessFs> = AccessFs::ReadFile | AccessFs::ReadDir;
    let dev: BitFlags<AccessFs> = AccessFs::ReadFile | AccessFs::WriteFile;

    let mut rules: Vec<(PathBuf, BitFlags<AccessFs>)> = Vec::new();
    // The node runtime: the binary (and where its symlink points) may run;
    // its install prefix (…/bin/node -> …) is readable for shared libraries
    // and data, but nothing else in it may run (inside an AppImage the
    // prefix also holds the app itself).
    for bin in [Some(node_bin.to_path_buf()), std::fs::canonicalize(node_bin).ok()].into_iter().flatten() {
        rules.push((bin.clone(), AccessFs::Execute | AccessFs::ReadFile));
        if let Some(prefix) = bin.parent().and_then(Path::parent) {
            if !matches!(prefix.to_str(), Some("/" | "/usr" | "/usr/local")) {
                rules.push((prefix.to_path_buf(), read));
            }
        }
    }
    // Starting node also opens its dynamic loader (ld-linux), which Landlock
    // checks as an execution: allow exactly that file, not all of /usr.
    for interp in [elf_interpreter(node_bin), std::fs::canonicalize(node_bin).ok().and_then(|p| elf_interpreter(&p))].into_iter().flatten() {
        if let Ok(real) = std::fs::canonicalize(&interp) {
            rules.push((real, AccessFs::Execute | AccessFs::ReadFile));
        }
        rules.push((interp, AccessFs::Execute | AccessFs::ReadFile));
    }
    rules.push((daemon_dir.to_path_buf(), read));
    for sys in ["/usr", "/lib", "/lib64", "/lib32", "/etc", "/opt", "/proc", "/sys"] {
        rules.push((PathBuf::from(sys), read));
    }
    for d in ["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom", "/dev/tty"] {
        rules.push((PathBuf::from(d), dev));
    }

    let mut ruleset = Ruleset::default().handle_access(all)?.create()?;
    for (path, access) in rules {
        // Paths that do not exist on this system are simply not granted.
        if let Ok(fd) = PathFd::new(&path) {
            // A file (not a directory) only takes file rights.
            let access = if path.is_dir() { access } else { access & AccessFs::from_file(abi) };
            ruleset = ruleset.add_rule(PathBeneath::new(fd, access & all))?;
        }
    }
    Ok(ruleset.restrict_self()?.ruleset)
}
