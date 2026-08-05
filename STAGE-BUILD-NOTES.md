# Stage build — Aug 4, 2026, 14:01 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, 0 errors, 55 warnings (all dead-code / snake_case — pre-existing, harmless).

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
- binary: `Contents/MacOS/aygent`, 37,014,920 bytes, mtime Aug 4 14:01
- release profile finished in 44.42s (warm cache)

**Prod is untouched.** Any AYGENT window currently open is running the OLD build — it must be quit and reopened from the Stage bundle to pick these changes up.

## What needs live QA (untested against real APIs)
1. **Notion or Vercel connect with a REAL credential** — highest value. My setup steps + endpoints were written from docs, never round-tripped against a live API. If one of these works end-to-end, the whole connector pattern is validated.
2. **Second GitHub account + nickname** — add it, then flip the radio; expect the toast/label to read "Switched to X".
3. **Write-toggle blast-radius text** — read it as a stranger would. Does it make the danger obvious?
4. **Tools & Skills tabs** — origin badges should render, and the core file tools should now be visible in the list.
5. **Google service account** — only if there's an existing Cloud project to point at.

## Next
Fold findings back into the connector setup copy; anything that fails live is a docs bug, not a code bug, until proven otherwise.
