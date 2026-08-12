# Shared-context (read-only mounts): why an agent can't SEE a mounted folder

Mason gave Muse read-access to Cleo's memory folder; Muse can't see it. Not a
DB/registration bug — a **design gap** in how mounts are surfaced.

## What works today (and what doesn't)
Mounts (`broker.rs` `Mount`, `resolve()` read-fallback) were built for ONE
motion: **read a file in the mount whose relative path you already know**, and
only when that path does NOT exist in your own root. Proof: the only mount test
is `mount_read_is_admitted` (reads `notes.md` from the mount). There is NO code
or test for *listing* or *discovering* a mount.

The read-fallback rule (verified in `broker.rs::resolve`):
1. primary = resolve against the agent's OWN root
2. if primary path EXISTS → return it (own root always wins)
3. else, first mount whose SAME relative path exists → return it
4. else → primary (so errors never leak a mount path)

## The three reasons Muse "can't see it"
1. **No discovery.** `list_files` (`broker_ws.rs` "list" op) calls
   `broker.resolve(agent, dir, Read)` and lists THAT directory only.
   `list_files(".")` → the agent's own root EXISTS → step 2 returns it → the
   listing contains only the agent's own entries. A mount is NEVER enumerated.
   So Muse cannot learn the folder is mounted or what filenames are in it.
2. **The agent isn't even told.** `register_all_agent_mounts` wires the broker,
   but NOTHING injects "you have read-only access to these folders" into the
   agent's system prompt (grep: no mount text in `agent_stream`/headless).
   `agent_mounts_list` exists for the UI, but the model never sees it. Muse has
   no reason to try.
3. **Ambiguous addressing even if it guessed.** A mount is addressed by a path
   RELATIVE TO THE MOUNT ROOT, and only resolves when that path does NOT exist
   in the agent's own root (own root wins, rule 2). If both Muse and Cleo have
   `memory/`, `list_files("memory")` always hits MUSE's `memory`. There's no
   namespace (`@Cleo/memory`) to say "the mounted one."

Net: a mounted folder is invisible and, for any colliding name, unreachable.

## The fix — give mounts a NAMESPACE (discoverable + unambiguous), jail unchanged

Introduce a reserved virtual prefix so mounts have their own addressable space
that can't collide with the agent's own files and CAN be listed.

### Convention: `@shared/<label>/…`
- `list_files("@shared")` → lists the mount LABELS (one dir per mount, e.g.
  `Cleo`). This is the discovery surface.
- `list_files("@shared/Cleo")` / `read_file("@shared/Cleo/memory/foo.md")` →
  resolved against THAT mount's root (`resolve_within(mount.root, rest, Read)`).
- Writes to `@shared/…` are REFUSED (mounts are read-only — unchanged rule).
- `@` can't collide with a real relative path the agent owns (no agent file is
  addressed starting `@shared/`), so no shadowing, and own-root-wins is intact
  for every NON-`@shared` path.

### Where the code changes go
1. **`broker.rs`** — in `resolve()`, detect a leading `@shared/` segment:
   strip it, split off `<label>`, find the matching mount by label, then
   `resolve_within(mount.root, rest, mode)` with `mode.is_write()` → refuse.
   Keep the EXISTING implicit read-fallback too (back-compat), but `@shared` is
   the explicit, unambiguous path. Add `mounts_for` is already public.
2. **`broker_ws.rs` "list"** — special-case `@shared` (no trailing label):
   return one `{name:<label>, kind:"dir"}` per mount from `broker.mounts_for`.
   For `@shared/<label>[/…]`, resolve through the broker as above and read_dir.
   (Same for the Rust-side `exec_tool` `list_files` in `lib.rs` so the agent
   loop path matches the daemon path — TWO callers, per the cwd-oscillation
   lesson; fix both or they drift.)
3. **`lib.rs` agent_stream + run_headless_turn** — inject a MOUNTS block into
   the system prompt when the agent has mounts: "You have READ-ONLY access to
   shared folders. List them with list_files('@shared'); read e.g.
   read_file('@shared/Cleo/memory/…'). You cannot write to them." Assemble from
   `broker.mounts_for(agent_id)` (labels) — the same source the resolver uses,
   so it can't drift. THIS is what makes Muse actually try.
4. **Tests** (`broker.rs`): `@shared` lists labels; `@shared/<label>/file` reads
   the mount; a colliding own-file is NOT shadowed by `@shared`; write to
   `@shared` refused; unknown label → NotFound (no leak).

## Smaller alternative (if we want the 15-min version first)
Just do #3 alone — inject the mount block into the prompt AND tell the model the
implicit rule ("to read a shared file, use its path relative to the shared
folder; it resolves if you don't have that same path yourself"). That makes
Muse *try* and works for non-colliding names immediately, with zero jail change.
But it doesn't fix discovery (can't list what's there) or collisions. The
`@shared` namespace is the real fix; #3's prompt is a prerequisite either way.

## Security note
Nothing here widens the jail. `@shared` resolves ONLY to a registered mount
root, through the same `resolve_within` kernel (traversal/symlink/forbidden all
apply), and writes are refused. It's purely an ADDRESSING + DISCOVERY layer over
the existing read-only mount that's already proven by the escape suite.
