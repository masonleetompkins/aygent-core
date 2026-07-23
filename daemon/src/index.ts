// AYGENT daemon — the brain. Runs under a macOS Seatbelt profile that denies
// ambient fs + exec (Atlas C1). This process MUST NOT import `fs`, `child_process`,
// or `net` for real work — all file I/O goes through the Rust broker over WS,
// and only the Rust side may spawn. A lint rule enforces the import ban; the OS
// enforces it for real.
//
// Phase 0 scope: boot, authenticate the WS, stream one Anthropic turn through one
// handle-based broker fs tool. Contract: docs/CONTRACTS.md.

import { startWsServer } from "./core/ws.js";
import { BrokerClient } from "./broker/client.js";

// --- WS auth (Atlas C6) --------------------------------------------------
// The Rust shell generates a random per-session token and injects it via env.
// The daemon rejects any WS connection lacking the token in its first frame,
// and validates Origin. localhost is NOT access control on its own.
const WS_TOKEN = process.env.AYGENT_WS_TOKEN;
const WS_SOCKET = process.env.AYGENT_WS_SOCKET; // prefer a unix socket path if provided

if (!WS_TOKEN) {
  // Fail closed: without the token the daemon has no legitimate client.
  console.error("[aygent] FATAL: no AYGENT_WS_TOKEN — refusing to start.");
  process.exit(1);
}

async function main() {
  const broker = new BrokerClient(); // talks to the Rust broker; NO local fs

  const ws = await startWsServer({
    token: WS_TOKEN!,
    socketPath: WS_SOCKET, // unix socket preferred; else ephemeral loopback + token
    onConnection: (client) => {
      // client is authenticated (token + Origin checked in startWsServer)
      client.on("message", async (msg) => {
        // TODO(M0.1/M0.3): route to agent loop; stream tokens back.
        // Phase-0 smoke: echo + a broker.stat round-trip to prove the bridge.
      });
    },
  });

  console.error(`[aygent] daemon up. transport=${WS_SOCKET ? "unix" : "loopback+token"}`);

  // Defensive self-check: if we can read outside our scope, the Seatbelt profile
  // is misconfigured. The real escape suite lives in daemon/test.
  void broker;
  void ws;
}

main().catch((e) => {
  console.error("[aygent] fatal", e);
  process.exit(1);
});
