// WS server (Atlas C6): token + Origin authenticated. localhost is NOT access
// control on its own — any local process/tab can otherwise connect and drive
// the agent. First frame MUST carry the per-session token minted by Rust.
//
// Phase 0 (M0.1): loopback + token. M0.2 may switch to a unix domain socket
// (not port-scannable) if the Tauri sidecar bridge allows.

import { WebSocketServer, WebSocket } from "ws";

export interface WsClient {
  on(event: "message", cb: (data: string) => void): void;
  send(data: string): void;
}

export interface WsOptions {
  token: string;
  socketPath?: string;
  onConnection: (client: WsClient) => void;
}

export async function startWsServer(opts: WsOptions) {
  // Bind loopback only; port 0 => ephemeral (not a fixed well-known port).
  const wss = new WebSocketServer({ host: "127.0.0.1", port: 0 });

  wss.on("connection", (sock: WebSocket, req) => {
    // Origin check: reject browser tabs / foreign origins.
    const origin = req.headers.origin;
    if (origin && !/^tauri:\/\/|^https?:\/\/localhost/.test(origin)) {
      sock.close(1008, "bad origin");
      return;
    }

    // Require the token in the FIRST frame, else drop.
    let authed = false;
    sock.once("message", (raw) => {
      let ok = false;
      try {
        const msg = JSON.parse(raw.toString());
        ok = msg?.type === "auth" && msg?.token === opts.token;
      } catch {
        ok = false;
      }
      if (!ok) {
        sock.close(1008, "unauthorized");
        return;
      }
      authed = true;
      sock.send(JSON.stringify({ type: "auth:ok" }));

      const client: WsClient = {
        on: (_e, cb) => sock.on("message", (d) => cb(d.toString())),
        send: (d) => sock.send(d),
      };
      opts.onConnection(client);
    });

    // Safety: if no auth frame quickly, close.
    setTimeout(() => { if (!authed) sock.close(1008, "auth timeout"); }, 3000);
  });

  const addr = wss.address();
  const port = typeof addr === "object" && addr ? addr.port : 0;
  // Rust reads this line to learn the port to hand the WebView.
  console.error(`[aygent] ws listening 127.0.0.1:${port}`);
  return { wss, port };
}
