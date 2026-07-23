# The Four Frozen Contracts (Atlas S2)

_**FROZEN at the Phase-0 gate (M0.4 — 2026-07-23).** These are what we live with; churning them
later reworks every tool. Do not change without a migration plan._

## M0.4 freeze status (verified against shipped code)
| Contract | Shape frozen | Impl status (2026-07-23) |
|---|---|---|
| 1. `ToolDef` | ✅ | Shape frozen. Agent loop currently registers builtin fs tools inline (Rust); the generic registry that consumes `ToolDef` lands with MCP/connectors in Phase 1. |
| 2. Broker RPC | ✅ | Resolution FROZEN + LIVE (atomic openat/O_NOFOLLOW, component-compare, nlink, firmlink/tmp, gate-tested). Daemon-facing WS currently returns *content* (proven end-to-end). Opaque `Handle` object API + `acquireLock` + bookmark-stale re-acquire land P1 (checkpoints/multi-agent). |
| 3. `Capability` enum | ✅ | FROZEN + enforced (M0.2b): `grantsFor(mode)` + gate + MCP transport split. `daemon/src/core/capabilities.ts`. |
| 4. Folder-lock protocol | ✅ | Design FROZEN. Implementation lands with checkpoints (C4) + shared pools (C5) in Phase 1 — single-writer file ops don't need it yet. |

_Rule for changes: if a change touches a frozen SHAPE (type/enum/RPC signature), it needs a
migration note here first. Filling in deferred IMPL against the frozen shape is normal Phase-1
work, not a contract change._

---

## 1. `ToolDef` — the universal tool currency
Every capability the agent has (builtin fs, work tools, web, MCP tools, API connectors) is a
`ToolDef`. The agent loop only ever knows "tools."

```ts
type Capability =
  | "fs.read" | "fs.write" | "net.http" | "mcp.net"        // Folder Mode grants
  | "shell.exec" | "hooks.script" | "mcp.local-exec";      // Pro Mode adds

type ToolDef = {
  name: string;                        // unique, stable
  description: string;
  args: JSONSchema;
  caps: Capability[];                  // gated against the active agent's grants
  handler: (args: unknown, ctx: ToolCtx) => Promise<ToolResult>;
};

type ToolCtx = {
  agentId: string;
  broker: BrokerClient;                // the ONLY way to touch files (handle-based)
  emit: (event: LifecycleEvent) => void;   // hooks bus
  signal: AbortSignal;
};

type ToolResult =
  | { ok: true; content: string; meta?: Record<string, unknown> }
  | { ok: false; error: string };
```

Rule: a tool NEVER imports `fs`/`child_process`/`net` directly. It uses `ctx.broker`. The
Seatbelt profile makes a violation fail at the OS anyway (defense in depth).

---

## 2. Broker RPC — handle-based, never path-based (Atlas C2)
The daemon has NO ambient fs (Seatbelt denies it). All file I/O crosses to the Rust broker.
The broker returns opaque handles / content, never a path the daemon re-opens (kills TOCTOU).

```ts
interface BrokerClient {
  // resolve+open atomically in Rust; returns an opaque handle, not a path
  open(agentId: string, requestedPath: string, mode: "r" | "w" | "rw"): Promise<Handle>;
  read(h: Handle): Promise<Uint8Array>;
  write(h: Handle, data: Uint8Array): Promise<void>;
  close(h: Handle): Promise<void>;
  list(agentId: string, requestedPath: string): Promise<DirEntry[]>;
  stat(agentId: string, requestedPath: string): Promise<StatInfo>;
  // folder-level RW lock (A.2 #5) — safe-open, checkpoint quiesce, single-writer-per-note
  acquireLock(folderId: string, kind: "read" | "write", path?: string): Promise<LockToken>;
  releaseLock(token: LockToken): Promise<void>;
}
type Handle = { __brand: "handle"; id: string };  // opaque; NOT a path
```

Rust-side resolution (must be atomic): `openat` component walk + `O_NOFOLLOW` on final
component + per-component symlink check; component-boundary root compare (NOT string prefix);
canonicalize against the data volume; case via inode; refuse `st_nlink > 1` on write; reject
`/tmp`,`/private/tmp`,`/var`/firmlinks; handle stale bookmarks (re-acquire or fail closed).

---

## 3. `Capability` enum — the tier boundary (Atlas C3)
See the enum in contract #1. Enforcement is TWO layers:
- **Registry:** refuses to expose a tool whose `caps` aren't granted to the active agent.
- **OS (authoritative):** the Seatbelt profile denies `file-*`/`process-exec*` in Folder Mode,
  so even a bug/compromise can't exceed the grant.

MCP transport split:
- `mcp.net` — network HTTP/SSE MCP endpoints → **Folder Mode OK**.
- `mcp.local-exec` — stdio MCP servers the app must *spawn* → **Pro Mode only** (spawning a
  local server IS local execution). The catalog labels each server's transport.

---

## 4. Folder-lock protocol (Atlas S3/C4/C5)
ONE per-folder RW lock lives in the Rust broker. Everything acquires it:
- **Safe open** (C2): read/write lock on the path.
- **Checkpoint quiesce** (C4): exclusive folder-wide write lock across ALL agents scoped to
  the folder → `git add`/commit → release. Never snapshot a moving tree.
- **Single-writer-per-note** (C5): pooled agents editing the same file serialize; second
  writer queues or is refused with a surfaced conflict.

Subagents share the parent's folder lock + checkpoint stream (same scope) by default.
