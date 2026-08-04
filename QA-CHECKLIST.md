# Stage build QA — Aug 4, 14:01 bundle

Build: `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
Result: 0 errors, 54 cosmetic warnings (unused fns, non-snake-case). Prod untouched.

**Important:** the currently-open AYGENT window is still running the OLD build.
Quit and reopen from the Stage bundle to test any of this.

## Test in this order

1. **Connect Notion or Vercel with a REAL credential.** ← highest risk.
   My setup steps and endpoints were written from docs, never fired against a
   live API. Expect this to be where breakage shows up.
2. **Add a 2nd GitHub account + nickname.** Radio switching should show
   "Switched to X".
3. **Write-toggle blast-radius text** — read it, tell me if the wording
   actually conveys what a write can touch.
4. **Tools & Skills tabs** — origin badges render, core file tools now visible.
5. **Google service account** — only if you have a Cloud project handy.

## Known-untested
- Live OAuth/token refresh paths for Notion + Vercel.
- Multi-account switching beyond 2 accounts.
