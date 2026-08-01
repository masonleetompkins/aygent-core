# ISSUE: Release proc-macro dylibs corrupt — ROOT CAUSE: cargo strip pass, NOT the jail

_Filed 2026-08-01 by Cleo. Status: RESOLVED with workaround (`strip = "none"`). Original
jail hypothesis was WRONG — corrected here; kept for the record._

## Symptom
`cargo tauri build` (release) failed reproducibly with `error[E0463]: can't find crate for
<proc-macro>`; dlopen on the emitted dylibs showed they were corrupt on disk:

```
dlopen(libthiserror_impl-*.dylib): mis-aligned LINKEDIT string pool
```

## Evidence chain (and the wrong turn)
1. 3 clean release builds inside the agent jail → 3 failures, different proc-macro crates
   first each time. `cargo check` and `cargo tauri build --debug` were always clean.
2. **Wrong hypothesis (mine):** the Pro-Mode jail's write path mangles release dylib writes.
3. **Discriminating experiment:** Mason ran the same build in Terminal, outside the jailed
   exec path. First run was contaminated (Cargo reused my stale corrupt artifacts — same
   file hash + corruption offset `0x002CE834` as my jailed run). After `rm -rf target/release`,
   his clean run **failed identically**. → The jail is INNOCENT.
4. **Actual discriminator:** debug vs release. Cargo's release default strips debuginfo via a
   post-link strip pass that rewrites the Mach-O LINKEDIT segment. Debug doesn't strip.
5. **Test:** `[profile.release] strip = "none"` + clean rebuild → **entire workspace compiled
   green first try** (previously 0-for-3), and `cargo tauri build` produced a release
   AYGENT.app — built from inside the jail, no special handling.

## Root cause
The strip pass on this toolchain (rustc 1.97.1, 2026-07-14, macOS) corrupts the LINKEDIT
string pool of proc-macro dylibs; dyld then refuses to load them mid-build. Toolchain bug,
not an AYGENT bug.

## Fix in repo
`src-tauri/Cargo.toml`: `[profile.release] strip = "none"` (committed). Cost: larger binary
(debuginfo retained). Ship pipeline can strip the FINAL app binary later with safe tooling
(`strip -S` / codesign-aware), which never round-trips proc-macro dylibs through dlopen.
Revisit when rustc updates.

## Still-valid harness findings from this hunt (real, keep)
- `shell_run` spawns with a scrubbed PATH — bare `cargo`/`node` fail; workaround `sh -lc`.
  Fix: inherit login PATH (or configurable) for Pro-Mode spawns.
- One agent `write_file` was refused `HardlinkRefused` mid-session (transient, unreproduced).
  Watch for recurrence.

## Lesson (Cleo)
I pattern-matched "corruption + jail = jail bug" and filed it as fact. The discriminating
experiment existed and was cheap — run it BEFORE writing the issue doc, not after. Precise
beats clever; evidence beats theory.
