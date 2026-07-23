// BrokerClient — the daemon's ONLY door to the filesystem (Atlas C1/C2).
// The daemon has no ambient fs (Seatbelt denies it), so every file op is an
// RPC to the Rust path broker over the WS bridge. The broker resolves + admits
// or refuses; on read it returns CONTENT, on write it takes content. The daemon
// never receives a re-openable path (kills TOCTOU). Contract: CONTRACTS.md §2.
//
// M0.2(b): WS-RPC transport. The daemon sends {type:"broker", op, ...} frames
// and awaits a matching {type:"broker:reply", id, ...}. Rust answers via the
// broker we proved in the escape suite.

export type Mode = "r" | "w" | "rw";
export interface DirEntry { name: string; kind: "file" | "dir"; }
export interface StatInfo { size: number; kind: "file" | "dir"; mtimeMs: number; }

/// A transport the client uses to reach Rust. In the daemon this is the WS
/// connection back to the shell; injected so the client is testable.
export interface BrokerTransport {
  request(payload: Record<string, unknown>): Promise<any>;
}

export class BrokerError extends Error {
  constructor(public code: string) { super(`broker refused: ${code}`); }
}

export class BrokerClient {
  constructor(private transport: BrokerTransport | null = null) {}

  attach(transport: BrokerTransport) { this.transport = transport; }

  private async call(op: string, args: Record<string, unknown>): Promise<any> {
    if (!this.transport) throw new Error("broker transport not attached");
    const reply = await this.transport.request({ type: "broker", op, ...args });
    if (reply?.ok === false) throw new BrokerError(reply.error ?? "unknown");
    return reply;
  }

  /// Resolve+read a file's content (broker admits/refuses first).
  async readFile(agentId: string, path: string): Promise<string> {
    const r = await this.call("read", { agentId, path });
    return r.content as string;
  }
  /// Resolve+write content to a path (write-mode checks incl. hardlink refusal).
  async writeFile(agentId: string, path: string, content: string): Promise<void> {
    await this.call("write", { agentId, path, content });
  }
  /// List a directory inside the jail.
  async list(agentId: string, path: string): Promise<DirEntry[]> {
    const r = await this.call("list", { agentId, path });
    return r.entries as DirEntry[];
  }
  /// Stat a path inside the jail.
  async stat(agentId: string, path: string): Promise<StatInfo> {
    const r = await this.call("stat", { agentId, path });
    return r.stat as StatInfo;
  }
  /// Probe admit/refuse WITHOUT reading (used by tools to check before acting).
  async resolve(agentId: string, path: string, mode: Mode = "r"): Promise<{ ok: boolean; error?: string }> {
    if (!this.transport) throw new Error("broker transport not attached");
    return this.transport.request({ type: "broker", op: "resolve", agentId, path, mode });
  }
}
