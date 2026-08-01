# ISSUE: Jail write-path corrupts release-profile proc-macro dylibs

_Filed 2026-08-01 by Cleo (self-hosted, building AYGENT from inside AYGENT). Status: OPEN — top priority for the next harness version._

## Symptom
`cargo tauri build` (release) run via Pro Mode `shell_run`/`shell_spawn` inside the agent jail
fails reproducibly:

```
error[E0463]: can't find crate for `thiserror_impl`
```

Root cause surfaced by dlopen'ing the built artifact directly:

```
dlopen(libthiserror_impl-*.dylib): mis-aligned LINKEDIT string pool (fileOffset=0x002CE834)
```

The proc-macro dylibs rustc emits are **corrupt on disk**. rustc builds them, then can't load
them to expand macros in downstream crates.

## Evidence (all inside jail /Users/masontompkins/AYGENT/Cleo/)
- 3 clean release builds, 3 failures; different crates trip first (thiserror_impl,
  zerofrom_derive, serde_derive) — whichever large proc-macro dylib gets loaded first.
- `python3 ctypes.CDLL` on BOTH copies of libthiserror_impl → same LINKEDIT corruption. Not a
  race; the bytes on disk are bad.
- **`cargo check` (dev profile): clean.**
- **`cargo tauri build --debug`: builds, links, and BUNDLES successfully** (AYGENT.app produced).
  Same code, same jail, same rustc — only the profile differs.
- Corrupt files have links=1 (not a hardlink artifact). Separately, an agent `write_file` call
  was refused `HardlinkRefused` once during this session — may or may not be related.
- Discriminating experiment queued: Mason runs `cargo tauri build` in Terminal (outside the
  jailed exec path) from the same checkout. If clean → corruption is in the Pro-Mode
  exec/seatbelt layer. If also corrupt → deeper (folder/fs layer).

## Hypothesis
Release-profile dylib emission differs from debug (optimized/compacted LINKEDIT, different
write pattern — likely pwrite at offsets and/or mmap-backed writes). The jail's file layer
(broker fd layer / seatbelt profile / whatever intercepts or polices writes under the folder)
does not replay that write pattern faithfully, mangling the LINKEDIT segment.

## Proposed fix (agreed with Mason 2026-08-01)
Pro Mode exec = **pass-through writes**: processes spawned by the exec broker write via normal
OS syscalls, cwd pinned to the folder; Seatbelt still denies paths OUTSIDE the folder, but no
interception/replay layer sits in the middle of byte-level writes INSIDE it. Keep: env scrub,
GUI-launch denylist, folder-scoped fs policy. Also fix: inherit login PATH for spawned
processes (current scrubbed PATH breaks bare `cargo`/`node`; workaround `sh -lc`).

## Workarounds until fixed
- Debug bundles work: `cargo tauri build --debug` → target/debug/bundle/macos/AYGENT.app.
- Release bundles: build outside the jailed exec path (human in Terminal).
