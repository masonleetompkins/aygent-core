// AYGENT — Capability model + gating (M0.2b, Atlas C3).
// The tier boundary. Folder Mode grants a safe set; Pro Mode adds execution.
// This is enforced at the tool registry (below) AND by the OS (Seatbelt denies
// file+exec regardless) — defense in depth. Contract: docs/CONTRACTS.md §3.
//
// The critical split (Atlas C3): connecting an MCP server is NOT uniform.
//   - mcp.net       = network HTTP/SSE MCP endpoint      -> Folder Mode OK
//   - mcp.local-exec = stdio MCP server we must SPAWN    -> Pro Mode ONLY
// because spawning a local MCP server IS local execution. Gating only the
// "shell" tool would let an MCP server smuggle execution in through the front
// door. So the gate lives on the TRANSPORT, not just the tool name.

export type Capability =
  | "fs.read"
  | "fs.write"
  | "net.http"
  | "mcp.net"          // Folder Mode grants these
  | "shell.exec"
  | "hooks.script"
  | "mcp.local-exec";  // Pro Mode adds these

export type AgentMode = "folder" | "pro";

/** Capabilities granted for a given mode. Folder Mode is the safe baseline. */
export function grantsFor(mode: AgentMode): ReadonlySet<Capability> {
  const folder: Capability[] = ["fs.read", "fs.write", "net.http", "mcp.net"];
  if (mode === "folder") return new Set(folder);
  // Pro Mode = Folder grants + execution grants (opt-in, scary consent screen).
  return new Set<Capability>([...folder, "shell.exec", "hooks.script", "mcp.local-exec"]);
}

/** A tool/connection declares the capabilities it requires to run. */
export interface CapabilityGated {
  name: string;
  caps: Capability[];
}

/**
 * The gate. Returns null if allowed, or a refusal reason string if not.
 * Enforced BEFORE a tool is exposed to the model or executed. The OS-level
 * Seatbelt jail is the authoritative backstop; this makes refusals explicit
 * and early (and covers cases the OS can't see, like which MCP transport).
 */
export function gate(mode: AgentMode, tool: CapabilityGated): string | null {
  const granted = grantsFor(mode);
  const missing = tool.caps.filter((c) => !granted.has(c));
  if (missing.length === 0) return null;
  return `"${tool.name}" needs [${missing.join(", ")}] not granted in ${mode} mode` +
    (missing.some((m) => m === "shell.exec" || m === "mcp.local-exec" || m === "hooks.script")
      ? " — enable Pro Mode to allow execution."
      : "");
}

/** Classify an MCP server config into the correct capability by its transport. */
export function mcpCapabilityForTransport(transport: "http" | "sse" | "stdio"): Capability {
  // stdio servers are LOCAL PROCESSES we spawn -> execution -> Pro-only.
  return transport === "stdio" ? "mcp.local-exec" : "mcp.net";
}
