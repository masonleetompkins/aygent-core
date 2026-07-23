// BrokerWsTransport — the daemon's authed client connection to the Rust broker
// WS (M0.2b). Rust is the server (privileged); the daemon is a pure client that
// can only ASK. Request/reply correlation by id. This is the jail-boundary
// crossing — see docs/BROKER-RPC-DECISION.md.

import { WebSocket } from "ws";
import type { BrokerTransport } from "./client.js";

export class BrokerWsTransport implements BrokerTransport {
  private ws: WebSocket | null = null;
  private ready: Promise<void>;
  private nextId = 1;
  private pending = new Map<number, (v: any) => void>();

  constructor(port: number, private token: string) {
    this.ws = new WebSocket(`ws://127.0.0.1:${port}`);
    this.ready = new Promise((resolve, reject) => {
      const ws = this.ws!;
      ws.on("open", () => ws.send(JSON.stringify({ type: "auth", token: this.token })));
      ws.on("message", (raw) => {
        let msg: any;
        try { msg = JSON.parse(raw.toString()); } catch { return; }
        if (msg.type === "auth:ok") { resolve(); return; }
        if (msg.type === "auth:err") { reject(new Error("broker auth rejected")); return; }
        if (msg.type === "broker:reply" && typeof msg.id === "number") {
          const cb = this.pending.get(msg.id);
          if (cb) { this.pending.delete(msg.id); cb(msg); }
        }
      });
      ws.on("error", (e) => reject(e));
      ws.on("close", () => {
        // reject any in-flight requests so tools fail closed
        for (const [, cb] of this.pending) cb({ ok: false, error: "broker channel closed" });
        this.pending.clear();
      });
    });
  }

  async request(payload: Record<string, unknown>): Promise<any> {
    await this.ready;
    const id = this.nextId++;
    return new Promise((resolve) => {
      this.pending.set(id, resolve);
      this.ws!.send(JSON.stringify({ ...payload, id }));
    });
  }
}
