# AYGENT

_The AI agent you actually own._ A native macOS app: a capable agent that works out of the
box, touches only folders you choose, runs on your own API keys, and is configured entirely
from a real UI — no terminal.

- Bundle ID: `build.masonlee.aygent`
- Stack: **Tauri v2 (Rust)** shell + **Node/TS** daemon + **React** UI
- Security: **Folder Mode** (OS-enforced jail via macOS Seatbelt) · **Pro Mode** relaxes it
- Full spec: `../BUILD-SPEC.md` · Scope: `../BUILD-SCOPE-macos.md` · Red-team: `../ATLAS-REVIEW.md`

## ⚠️ Build host
The **source** (Rust/Node/React) scaffolds & type-checks on any OS. But AYGENT is a **macOS
app** — it must be **compiled, run, signed, and notarized on a Mac.** Phase 0's escape-suite
gate only *executes* on macOS (it verifies the Seatbelt jail). Windows = author code; Mac =
run/verify/ship.

## Layout
```
aygent/
  src-tauri/     Rust shell: window, tray, keychain broker, PATH BROKER, Seatbelt supervisor
  daemon/        Node/TS brain: agent loop, tools, providers, scheduler, memory (NO ambient fs)
  ui/            React + Vite (Tauri WebView), local WS client (token-authed)
  seatbelt/      macOS sandbox-exec profiles (deny file+exec by default)
  docs/          contracts + build notes
```

## Phase 0 gate (must pass before committing to Phase 1)
The escape suite (`daemon/test/`) runs against the daemon **under its real Seatbelt profile**:
direct `fs`/`spawn` from the daemon is OS-denied; TOCTOU/hardlink/symlink/firmlink/case/stale
all refused; WS rejects unauthenticated connections. We test the door, not the lock.

## Status
Phase 0 scaffolding — see `docs/PHASE0.md`.
