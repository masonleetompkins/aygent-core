// ExecBrokerClient — the daemon's ONLY door to process execution (Pro Mode).
// The daemon runs under Seatbelt deny-exec, so it CANNOT spawn. Every process
// op is an RPC to the Rust exec broker over the SAME authed WS the file broker
// uses (envelope type:"exec"). Only Rust spawns. The daemon never receives a
// raw PID it can signal — it gets an opaque proc_handle (mirror of the file
// broker's handles-not-paths). Design: projects/aygent/PRO-MODE-SHELL-PLAN.md.
//
// Cap gate: shell.exec is bound on the broker at connect (the transport sends
// the session caps in its auth frame). If this agent isn't Pro Mode, EVERY exec
// call is refused by Rust — the daemon cannot self-grant.

import type { BrokerTransport } from "./client.js";

export interface ExecDigest {
  errors: number;
  warnings: number;
  lines_total: number;
  signal: string[]; // structured error lines (error[E...], --> file:line, …)
}

export interface ExecPoll {
  ok: boolean;
  running: boolean;
  exit_code: number | null;
  next_cursor: number;
  digest: ExecDigest;
  tail: string[];
  chunks?: Array<{ stream: "out" | "err"; text: string }>;
  error?: string;
}

export interface ExecRun extends ExecPoll {
  timed_out: boolean;
  cmd: string;
  log: string;
}

export interface ExecSpawn {
  ok: boolean;
  proc_handle: string;
  cmd: string;
  log: string;
  error?: string;
}

export class ExecBrokerError extends Error {
  constructor(msg: string) { super(`exec broker refused: ${msg}`); }
}

export class ExecBrokerClient {
  constructor(private transport: BrokerTransport | null = null) {}
  attach(transport: BrokerTransport) { this.transport = transport; }

  private async call(op: string, args: Record<string, unknown>): Promise<any> {
    if (!this.transport) throw new Error("exec transport not attached");
    const reply = await this.transport.request({ type: "exec", op, ...args });
    if (reply?.ok === false) throw new ExecBrokerError(reply.error ?? "unknown");
    return reply;
  }

  /// One-shot: spawn + wait-with-timeout + bounded digest. The 90% case
  /// (git pull, cargo build, git push, npm run build, tsc).
  async run(agentId: string, program: string, args: string[] = [], timeoutMs = 120_000): Promise<ExecRun> {
    return this.call("run", { agentId, program, args, timeout_ms: timeoutMs });
  }

  /// Spawn a long-running process (cargo tauri dev). Returns a proc_handle to
  /// poll/write/kill. cwd is pinned to the agent folder by the broker.
  async spawn(agentId: string, program: string, args: string[] = []): Promise<ExecSpawn> {
    return this.call("spawn", { agentId, program, args });
  }

  /// Read new output since `cursor` + the running digest. tailOnly drops the
  /// per-line chunks (agent just wants "how's it going").
  async poll(procHandle: string, cursor = 0, tailOnly = false): Promise<ExecPoll> {
    return this.call("poll", { proc_handle: procHandle, cursor, tail_only: tailOnly });
  }

  /// Write to the process's stdin.
  async write(procHandle: string, data: string): Promise<void> {
    await this.call("write", { proc_handle: procHandle, data });
  }

  /// Kill (TERM by default; pass "KILL" to escalate).
  async kill(procHandle: string, signal: "TERM" | "KILL" = "TERM"): Promise<void> {
    await this.call("kill", { proc_handle: procHandle, signal });
  }

  /// Block up to timeoutMs for exit.
  async wait(procHandle: string, timeoutMs = 120_000): Promise<{ exit_code: number | null; timed_out: boolean }> {
    return this.call("wait", { proc_handle: procHandle, timeout_ms: timeoutMs });
  }

  /// List known processes (for the UI process panel).
  async list(): Promise<{ procs: Array<{ proc_handle: string; cmd: string; running: boolean; uptime_ms: number }> }> {
    return this.call("list", {});
  }
}
