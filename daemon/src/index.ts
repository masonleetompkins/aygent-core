// AYGENT daemon — the brain. Runs under a macOS Seatbelt profile that denies
// ambient fs + exec (Atlas C1). This process MUST NOT import `fs`,
// `child_process`, or `net` for real work — all file I/O goes through the Rust
// broker over WS, and only the Rust side may spawn. A lint rule enforces the
// import ban; the OS enforces it for real.
//
// M0.1 scope: boot, authenticate the WS (per-session token + Origin — Atlas C6),
// and round-trip a hello + ping. Agent loop + broker wiring land in M0.2/M0.3.

import { startWsServer } from "./core/ws.js";
import { BrokerClient } from "./broker/client.js";

// --- WS auth (Atlas C6) --------------------------------------------------
// The Rust shell generates a random per-session token and injects it via env.
// The daemon rejects any WS connection lacking the token in its first frame,
// and validates Origin. localhost is NOT access control on its own.
const WS_TOKEN = process.env.AYGENT_WS_TOKEN;
const WS_SOCKET = process.env.AYGENT_WS_SOCKET; // prefer a unix socket if provided

if (!WS_TOKEN) {
  // Fail closed: without the token the daemon has no legitimate client.
  console.error("[aygent] FATAL: no AYGENT_WS_TOKEN — refusing to start.");
  process.exit(1);
}

async function main() {
  const broker = new BrokerClient(); // talks to the Rust broker; NO local fs
  void broker; // wired in M0.2

  const { port } = await startWsServer({
    token: WS_TOKEN!,
    socketPath: WS_SOCKET,
    onConnection: (client) => {
      // client is already authenticated (token + Origin checked in ws.ts).
      client.send(JSON.stringify({ type: "hello", from: "daemon", version: "0.0.1" }));

      client.on("message", (raw) => {
        let msg: any;
        try { msg = JSON.parse(raw); } catch { return; }
        switch (msg?.type) {
          case "ping":
            client.send(JSON.stringify({ type: "pong", t: Date.now() }));
            break;
          // M0.3: case "chat" -> agent loop -> stream tokens back.
          default:
            client.send(JSON.stringify({ type: "ack", echo: msg?.type ?? null }));
        }
      });
    },
  });

  // The Rust supervisor reads this exact line to learn the port to hand the UI.
  console.error(`[aygent] daemon up. AYGENT_WS_PORT=${port} transport=${WS_SOCKET ? "unix" : "loopback+token"}`);
}

main().catch((e) => {
  console.error("[aygent] fatal", e);
  process.exit(1);
});
