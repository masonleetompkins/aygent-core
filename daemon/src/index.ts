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
import { ExecBrokerClient } from "./broker/exec-client.js";

// --- WS auth (Atlas C6) --------------------------------------------------
// The Rust shell generates a random per-session token and injects it via env.
// The daemon rejects any WS connection lacking the token in its first frame,
// and validates Origin. localhost is NOT access control on its own.
const WS_TOKEN = process.env.AYGENT_WS_TOKEN;
const WS_SOCKET = process.env.AYGENT_WS_SOCKET; // prefer a unix socket if provided
const BROKER_PORT = process.env.AYGENT_BROKER_PORT;   // Rust-hosted broker WS (M0.2b)
const BROKER_TOKEN = process.env.AYGENT_BROKER_TOKEN;
// PRO MODE: the session's granted capabilities, comma-separated, injected by the
// supervisor for the active agent (e.g. "shell.exec"). Bound on the Rust broker
// at connect; the daemon can't self-grant. Empty/unset = Folder Mode (no exec).
const AGENT_CAPS = (process.env.AYGENT_AGENT_CAPS ?? "").split(",").map((s) => s.trim()).filter(Boolean);

if (!WS_TOKEN) {
  // Fail closed: without the token the daemon has no legitimate client.
  console.error("[aygent] FATAL: no AYGENT_WS_TOKEN — refusing to start.");
  process.exit(1);
}

async function main() {
  const broker = new BrokerClient(); // talks to the Rust broker; NO local fs
  const exec = new ExecBrokerClient(); // PRO MODE: talks to the Rust exec broker

  // M0.2b: connect to the Rust-hosted broker WS as an authed client, then
  // self-test the jail from the DAEMON side (admit inside, refuse outside).
  if (BROKER_PORT && BROKER_TOKEN) {
    try {
      const { BrokerWsTransport } = await import("./broker/transport.js");
      // Send the session caps in the auth frame so the broker binds shell.exec
      // (or not) for this connection. The file broker + exec broker share this
      // one authed transport — one channel, one cap binding.
      const transport = new BrokerWsTransport(Number(BROKER_PORT), BROKER_TOKEN, AGENT_CAPS);
      broker.attach(transport);
      exec.attach(transport);
      console.error(`[aygent] session caps: [${AGENT_CAPS.join(", ") || "folder-mode"}]`);

      const inside = await broker.resolve("default", "aygent-selftest.txt", "w");
      const outside = await broker.resolve("default", "/etc/passwd", "r");
      const insideStr = inside.ok ? "ADMIT" : "refuse:" + inside.error;
      const outsideStr = outside.ok ? "ADMIT(!!)" : "refuse:" + outside.error;
      console.error(`[aygent] broker self-test: inside=${insideStr} outside=${outsideStr}`);
    } catch (e) {
      console.error("[aygent] broker connect failed:", e);
    }
  } else {
    console.error("[aygent] no broker WS coords — broker calls will fail (dev without folder).");
  }

  const { port } = await startWsServer({
    token: WS_TOKEN!,
    socketPath: WS_SOCKET,
    onConnection: (client) => {
      // client is already authenticated (token + Origin checked in ws.ts).
      client.send(JSON.stringify({ type: "hello", from: "daemon", version: "0.0.1" }));

      client.on("message", async (raw) => {
        let msg: any;
        try { msg = JSON.parse(raw); } catch { return; }
        switch (msg?.type) {
          case "ping":
            client.send(JSON.stringify({ type: "pong", t: Date.now() }));
            break;
          case "selftest": {
            // Re-run the jail probe from the DAEMON side, on demand (after the
            // user picks a folder). Proves the daemon→broker channel enforces
            // the jail live: admit inside, refuse outside.
            const inside = await broker.resolve("default", "aygent-selftest.txt", "w");
            const outside = await broker.resolve("default", "/etc/passwd", "r");
            client.send(JSON.stringify({ type: "selftest:result", inside, outside }));
            break;
          }
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
