// BrokerClient — the daemon's ONLY door to the filesystem (Atlas C1/C2).
// The daemon has no ambient fs (Seatbelt denies it), so every file op is an
// RPC to the Rust path broker, which returns opaque HANDLES, never re-openable
// paths (kills TOCTOU). Contract: docs/CONTRACTS.md §2.
//
// Phase 0: the RPC transport (over the Tauri/WS bridge to Rust) is wired in
// M0.2. This class defines the frozen shape now so tools build against it.

export type Handle = { readonly __brand: "handle"; id: string };
export type Mode = "r" | "w" | "rw";

export interface DirEntry { name: string; kind: "file" | "dir"; }
export interface StatInfo { size: number; kind: "file" | "dir"; mtimeMs: number; }
export type LockToken = { readonly __brand: "lock"; id: string };

export class BrokerClient {
  // TODO(M0.2): inject the Rust RPC channel (Tauri command or WS bridge).

  async open(_agentId: string, _requestedPath: string, _mode: Mode): Promise<Handle> {
    throw new Error("broker.open not wired until M0.2");
  }
  async read(_h: Handle): Promise<Uint8Array> {
    throw new Error("broker.read not wired until M0.2");
  }
  async write(_h: Handle, _data: Uint8Array): Promise<void> {
    throw new Error("broker.write not wired until M0.2");
  }
  async close(_h: Handle): Promise<void> {
    throw new Error("broker.close not wired until M0.2");
  }
  async list(_agentId: string, _requestedPath: string): Promise<DirEntry[]> {
    throw new Error("broker.list not wired until M0.2");
  }
  async stat(_agentId: string, _requestedPath: string): Promise<StatInfo> {
    throw new Error("broker.stat not wired until M0.2");
  }
  // Folder-level RW lock (Atlas S3): safe-open, checkpoint quiesce, 1-writer-per-note.
  async acquireLock(_folderId: string, _kind: "read" | "write", _path?: string): Promise<LockToken> {
    throw new Error("broker.acquireLock not wired until M0.2");
  }
  async releaseLock(_token: LockToken): Promise<void> {
    throw new Error("broker.releaseLock not wired until M0.2");
  }
}
