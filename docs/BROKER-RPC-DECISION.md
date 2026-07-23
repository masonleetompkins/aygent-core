# M0.2(b) — Broker RPC transport: the architecture fork (2026-07-23)

## The problem
The daemon has no ambient fs (Seatbelt). It must ask the Rust broker for every
file op. But our current topology is:

- **Rust shell** spawns the **daemon**.
- **Daemon** hosts the WS server; the **UI** connects to it as a client.
- The **broker lives in Rust** (it must — it's the privileged, un-jailed side).

So the daemon can't reach the broker over its *own* WS server — the broker
isn't on that server. We need a channel FROM the daemon (jailed) TO Rust
(privileged).

## Options considered
1. **Rust hosts a second WS server; daemon connects as a client.**
   - Clean separation: broker RPC is its own privileged channel.
   - Daemon gets the broker WS URL + token via env (same pattern as the UI token).
   - Rust answers `{op: read|write|list|stat|resolve}` by calling the broker
     we already proved.
   - ✅ Matches CONTRACTS.md; the jailed side is a pure client of the privileged side.
2. Daemon calls Rust via Tauri IPC.
   - ❌ Tauri IPC is WebView↔Rust; the daemon is a separate Node process, not the
     WebView. Doesn't apply.
3. Unix domain socket, Rust-hosted.
   - ✅ Strongest (not port-scannable, fs-permission gated). Slightly more plumbing
     on Node side. Good Phase-1 hardening; for M0.2 the token'd loopback WS is fine
     and we can swap to UDS later behind the same BrokerTransport interface.

## Decision
**Option 1 for M0.2** (Rust-hosted broker WS, daemon connects as authed client),
with the `BrokerTransport` interface in `daemon/src/broker/client.ts` abstracting
it so we can swap to a Unix domain socket (Option 3) in Phase 1 without touching
tools.

### Channels summary (post-decision)
- **UI ↔ daemon WS** (daemon = server): chat, streaming, status. (Built, M0.1.)
- **daemon ↔ Rust broker WS** (Rust = server): every filesystem op, handle/content
  based, token-authed. (Building now, M0.2(b).)

### Why two channels, not one
The trust levels differ. The UI is semi-trusted presentation. The broker channel
is the crossing of the jail boundary — it must originate in the privileged Rust
side, and the jailed daemon must be a pure client that can only *ask*, never
*reach*. Separate servers = separate trust domains = no accidental privilege
bleed.

## Next build steps (M0.2(b))
1. Rust: host a second WS server (loopback, ephemeral port, per-session token).
   Answer `broker` ops via `Broker::resolve` + real fd read/write.
2. Rust: hand the daemon the broker-WS `{port, token}` via env at spawn.
3. Daemon: a `BrokerTransport` impl that connects to that WS, authenticates,
   and does request/reply correlation by id. Attach it to `BrokerClient`.
4. Prove end-to-end: daemon reads a file INSIDE the chosen folder (admitted) and
   is refused a file OUTSIDE it — asserted from the daemon side.
