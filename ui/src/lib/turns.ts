// AYGENT — per-agent live-turn store (M1.4 #2 fix, Atlas A).
//
// THE PROBLEM: everything that makes a "live turn" — the event channel, the
// token accumulator, the listen() subscription, the busy flag, the running
// history — was born and died inside Chat's send() closure. When you switched
// which agent you view, Chat re-rendered, openConv wiped msgs/history, and the
// prior turn's stream had nowhere to land. So: you couldn't send to a 2nd agent
// while the 1st ran (one pane-level busy), and returning to a running agent
// showed no live stream (the accumulator was orphaned).
//
// THE FIX: a module-scope store keyed by agentId that lives ABOVE the pane (it
// never unmounts). It owns each agent's turn state — liveText, tool cards,
// running history, status — and the event listener writes into it regardless of
// what's on screen. Any pane can subscribe to "the agent I'm viewing" and read
// its live stream, mid-flight. Sending to agent B is gated by B's own status,
// not a global flag. Backend needs ZERO changes (agent_stream runs to completion
// independently once invoked).
//
// No Redux — a tiny external store + useSyncExternalStore.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";

export type ToolCard = {
  name: string;
  path?: string;
  ok?: boolean;
  detail?: string;      // bounded tool output (error text, or a success preview)
  running?: boolean;
  summary?: string;     // one-line human context: "$ cargo build", "✎ src/main.rs · 142 lines"
  body?: string;        // expandable content: written file text, full command, fetched URL…
  id?: string;          // tool_use id — routes streaming deltas to this card
  rawArgs?: string;     // accumulating partial-JSON args while streaming
  spark?: { slug: string; title: string; html: string }; // SPARKS: inline mini-app preview payload (rendered as a sandboxed iframe in Chat)
};

// LIVE tool-arg streaming (Mason 08-03): the args of an in-flight tool call
// arrive as partial JSON. For write_file we surface the growing "content"
// string (i.e. the code being written, live); anything else shows raw args.
// Cheap incremental extraction — find `"content":"` then unescape what follows.
function liveBodyFromPartialArgs(name: string, raw: string): string | undefined {
  if (name === "write_file" || name === "generate_pdf") {
    const key = '"content":';
    const at = raw.indexOf(key);
    if (at < 0) return undefined;
    let s = raw.slice(at + key.length).trimStart();
    if (!s.startsWith('"')) return undefined;
    s = s.slice(1);
    // Trim a trailing partial escape so we never render half a \u sequence.
    const m = s.match(/\\+(u[0-9a-fA-F]{0,3})?$/);
    if (m) s = s.slice(0, -m[0].length);
    // The value may already be closed — cut at the terminating quote.
    let out = "";
    for (let i = 0; i < s.length; i++) {
      if (s[i] === '\\') { i++; const c = s[i];
        out += c === "n" ? "\n" : c === "t" ? "\t" : c === "r" ? "" : c === "u" ? "" : (c ?? "");
        if (c === "u") i += 4;
      } else if (s[i] === '"') { break; } else { out += s[i]; }
    }
    return out || undefined;
  }
  return raw.length > 3 ? raw : undefined;
}

// Bound an expandable card body: head + tail of anything huge, so a 10k-line
// write_file can't bloat the conversation store or the DOM. The full content
// is on disk regardless — render limits are not data loss.
const BODY_CAP = 16000;
function safeJson(v: unknown): string {
  try { return JSON.stringify(v, null, 2); } catch { return ""; }
}

function capBody(s: string | undefined): string | undefined {
  if (!s) return undefined;
  if (s.length <= BODY_CAP) return s;
  const head = s.slice(0, BODY_CAP * 0.75);
  const tail = s.slice(-BODY_CAP * 0.2);
  return `${head}\n\n… [${s.length.toLocaleString()} chars total — middle truncated for display; full content is in the file] …\n\n${tail}`;
}

// Build the one-line summary + expandable body for a tool call from its input.
// This is what turns "shell run" into "$ cargo build --release" (Mason 08-03).
export function describeToolUse(name: string, input: any): { summary: string; body?: string; spark?: { slug: string; title: string; html: string } } {
  const inp = input ?? {};
  if (name === "spark_preview") {
    const slug = typeof inp.slug === "string" ? inp.slug : "spark";
    const title = typeof inp.title === "string" && inp.title ? inp.title : slug;
    const html = typeof inp.html === "string" ? inp.html : "";
    return { summary: `\u26A1 ${title}`, spark: { slug, title, html } };
  }
  switch (name) {
    case "shell_run": case "shell_spawn": {
      const cmd = [inp.program, ...(Array.isArray(inp.args) ? inp.args : [])].filter(Boolean).join(" ");
      return { summary: `$ ${cmd}`.slice(0, 200), body: cmd.length > 200 ? cmd : undefined };
    }
    case "write_file": {
      const content = typeof inp.content === "string" ? inp.content : "";
      const lines = content ? content.split("\n").length : 0;
      return { summary: `✎ ${inp.path ?? "?"} · ${lines} line${lines === 1 ? "" : "s"}`, body: capBody(content) };
    }
    case "read_file": return { summary: `📄 ${inp.path ?? "?"}` };
    case "list_files": return { summary: `📁 ${inp.path ?? "."}` };
    case "rename_file": return { summary: `➜ ${inp.from ?? "?"} → ${inp.to ?? "?"}` };
    case "delete_file": return { summary: `🗑 ${inp.path ?? "?"}` };
    case "fetch_url": return { summary: `🌐 ${(inp.url ?? "?").slice(0, 160)}` };
    case "web_search": return { summary: `🔍 ${(inp.query ?? "?").slice(0, 160)}` };
    case "generate_pdf": return { summary: `📕 ${inp.output_path ?? inp.path ?? "document.pdf"}`, body: capBody(typeof inp.content === "string" ? inp.content : undefined) };
    case "send_message": return { summary: `✉ → ${inp.to_agent ?? "?"}`, body: capBody(typeof inp.message === "string" ? inp.message : undefined) };
    case "task_continue": return { summary: `⏰ wake in ${inp.delay_secs ?? 60}s`, body: capBody(typeof inp.note === "string" ? inp.note : undefined) };
    case "shell_poll": return { summary: `⟳ poll ${inp.proc_handle ?? "?"}` };
    case "shell_kill": return { summary: `⏹ kill ${inp.proc_handle ?? "?"}` };
    case "shell_write": return { summary: `⌨ stdin → ${inp.proc_handle ?? "?"}`, body: capBody(typeof inp.data === "string" ? inp.data : undefined) };
    case "transcribe_audio": return { summary: `🎙 ${inp.path ?? "?"}` };
    // DASHBOARD (M2): the generic fallback would dump raw spec JSON, which is
    // both noisy and the most interesting thing to read — so summarize the
    // human-meaningful part (what module, called what) and keep the spec as the
    // expandable body.
    case "dashboard_get": return { summary: "▦ read dashboard" };
    case "dashboard_add_module":
      return { summary: `▦ + ${inp.kind ?? "module"} “${inp.title ?? "untitled"}”`, body: capBody(safeJson(inp)) };
    case "dashboard_update_module":
      return { summary: `▦ update ${inp.title ? `“${inp.title}”` : (inp.id ?? "module")}`, body: capBody(safeJson(inp)) };
    case "dashboard_remove_module":
      return { summary: `▦ remove ${inp.id ?? "module"}` };
    case "dashboard_arrange":
      return { summary: `▦ rearrange ${Array.isArray(inp.moves) ? inp.moves.length : 0} module${Array.isArray(inp.moves) && inp.moves.length === 1 ? "" : "s"}`, body: capBody(safeJson(inp)) };
    default: {
      // Unknown/registry tool: show its args compactly instead of nothing.
      let args = ""; try { args = JSON.stringify(inp); } catch { /* ignore */ }
      return { summary: `⚙ ${name}${args && args !== "{}" ? " " + args.slice(0, 140) : ""}`, body: args.length > 140 ? args : undefined };
    }
  }
}
export type TurnMsg = { role: "user" | "assistant"; text: string; tools?: ToolCard[]; streaming?: boolean; from?: string };

/** One ordered entry in a turn. Text segments and tool calls interleave in the
 *  order they actually arrived, so a note sits with the calls it belongs to. */
export type TurnItem =
  | { kind: "text"; text: string }
  | { kind: "tool"; tool: ToolCard };

/** Token accounting accumulated across a turn's provider rounds (context meter
 *  + $ cost). `input` is the LAST round's input token count = the current
 *  context-window fill; the others sum across rounds. */
/** Token accounting for a turn.
 *  - `input`/`cacheRead`/`cacheWrite`/`output`: components for COST (summed as
 *    appropriate across the turn's tool-loop rounds).
 *  - `contextInput`: the TOTAL input the model processed on the LATEST round =
 *    input + cache_read + cache_creation. THIS is the real context-window fill
 *    (with prompt caching on, plain `input` is tiny because most tokens are
 *    billed as cache read/creation — that was the "2 / 1M" bug). */
export type TurnUsage = { input: number; output: number; cacheRead: number; cacheWrite: number; contextInput: number; contextWindow?: number };

export type TurnState = {
  status: "idle" | "running";
  liveText: string;          // accumulated streamed tokens (back-compat: full prose)
  liveTools: ToolCard[];     // tool cards for the in-flight turn (back-compat: all cards)
  /** ORDERED view of the same data — what the chat actually renders. */
  timeline?: TurnItem[];
  info?: string;             // latest info line (model, save point…)
  memory?: string;           // 🧠 auto-capture note for THIS turn (shown under the user msg)
  error?: string;
  usage?: TurnUsage;         // token counts for THIS turn (context meter + $ cost)
};

type AgentSlot = {
  turn: TurnState;
  history: unknown[];        // provider-format running history (per agent!)
  unlisten?: UnlistenFn;     // active stream subscription for this agent's turn
  inbound?: { fromName: string; text: string }; // live inter-agent inbound msg
};

const slots = new Map<string, AgentSlot>();
const listeners = new Set<() => void>();

function emit() { listeners.forEach((l) => l()); }
function subscribe(l: () => void) { listeners.add(l); return () => { listeners.delete(l); }; }

function slot(agentId: string): AgentSlot {
  let s = slots.get(agentId);
  if (!s) { s = { turn: { status: "idle", liveText: "", liveTools: [] }, history: [] }; slots.set(agentId, s); }
  return s;
}

// --- public read API (React) ------------------------------------------------

const IDLE: TurnState = { status: "idle", liveText: "", liveTools: [] };

// Handle ToolUseStart/ToolUseDelta for a slot (shared by the human-turn and
// headless listeners). Returns true if the event was one of ours.
function handleToolStream(cur: AgentSlot, kind: string | null, m: any): boolean {
  if (kind === "ToolUseStart") {
    cur.turn = { ...appendTool(cur.turn, { id: m.id, name: m.name, running: true, summary: `⚙ ${m.name}…`, rawArgs: "" }), status: "running" };
    emit();
    return true;
  }
  if (kind === "ToolUseDelta") {
    const tools = cur.turn.liveTools;
    for (let i = tools.length - 1; i >= 0; i--) {
      if (tools[i].id === m.id && tools[i].running) {
        const rawArgs = (tools[i].rawArgs ?? "") + (m.text ?? "");
        const body = capBody(liveBodyFromPartialArgs(tools[i].name, rawArgs));
        cur.turn = patchTool(cur.turn, i, { rawArgs, body: body ?? tools[i].body });
        break;
      }
    }
    emit();
    return true;
  }
  return false;
}

// --- TIMELINE MAINTENANCE --------------------------------------------------
// Every mutation goes through these so the ordered timeline can never drift
// out of sync with liveText/liveTools.

/** Append streamed tokens to the CURRENT text segment (or open a new one). */
function appendText(t: TurnState, chunk: string): TurnState {
  const timeline = [...(t.timeline ?? [])];
  const last = timeline[timeline.length - 1];
  if (last && last.kind === "text") {
    timeline[timeline.length - 1] = { kind: "text", text: last.text + chunk };
  } else {
    timeline.push({ kind: "text", text: chunk });
  }
  return { ...t, liveText: t.liveText + chunk, timeline };
}

/** Accumulate a Usage event into the turn. `input` is the LAST round's input
 *  token count (the current context-window fill, since each round re-sends the
 *  whole history); output + cache SUM across the turn's rounds. */
function accumulateUsage(t: TurnState, m: any): TurnState {
  const prev = t.usage ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, contextInput: 0 };
  const nIn = Number(m.input ?? 0), nOut = Number(m.output ?? 0);
  const nCr = Number(m.cache_read ?? m.cacheRead ?? 0), nCw = Number(m.cache_write ?? m.cacheWrite ?? 0);
  const win = Number(m.context_window ?? m.contextWindow ?? 0) || prev.contextWindow || undefined;
  // CONTEXT FILL = the TOTAL input the model saw on THIS round: fresh input +
  // cache reads + cache creation. With caching on, `input` alone is tiny; the
  // real prompt size lives in the cache fields. Last round wins (it reflects the
  // full history at turn's end). For COST we still keep per-component sums.
  const roundContextInput = nIn + nCr + nCw;
  return { ...t, usage: {
    // Cost components: sum output + cache_creation across rounds; input +
    // cache_read take the latest round (each round re-sends the whole prompt, so
    // summing them would multiply the history — the $74 bug). Latest is correct.
    input: nIn || prev.input,
    output: prev.output + nOut,
    cacheRead: nCr || prev.cacheRead,
    cacheWrite: prev.cacheWrite + nCw,
    contextInput: roundContextInput || prev.contextInput,
    contextWindow: win,
  } };
}

/** Append a NEW tool card. This also closes the open text segment, which is
 *  what keeps "summary, then the calls it describes" readable. */
function appendTool(t: TurnState, tool: ToolCard): TurnState {
  return {
    ...t,
    liveTools: [...t.liveTools, tool],
    timeline: [...(t.timeline ?? []), { kind: "tool", tool }],
  };
}

/** Update an existing card in place (streaming args, ToolResult) in BOTH views. */
function patchTool(t: TurnState, idx: number, patch: Partial<ToolCard>): TurnState {
  const liveTools = [...t.liveTools];
  if (idx < 0 || idx >= liveTools.length) return t;
  const updated = { ...liveTools[idx], ...patch };
  liveTools[idx] = updated;

  // Find the matching timeline entry: prefer id, fall back to positional match
  // among tool entries (ids are absent for some local providers).
  const timeline = [...(t.timeline ?? [])];
  let seen = -1;
  for (let i = 0; i < timeline.length; i++) {
    const e = timeline[i];
    if (e.kind !== "tool") continue;
    seen++;
    const idMatch = updated.id && e.tool.id === updated.id;
    if (idMatch || seen === idx) { timeline[i] = { kind: "tool", tool: updated }; break; }
  }
  return { ...t, liveTools, timeline };
}

// Finalize the streaming card when the assembled ToolUse arrives: replace the
// placeholder summary/body with the accurate ones from the FULL input. Falls
// back to appending a new card if no streaming card was opened (non-streaming
// providers emit ToolUse only).
function upsertToolUse(cur: AgentSlot, m: any): void {
  const d = describeToolUse(m.name, m.input);
  const tools = cur.turn.liveTools;
  for (let i = tools.length - 1; i >= 0; i--) {
    if (tools[i].id === m.id && tools[i].running) {
      cur.turn = { ...patchTool(cur.turn, i, { path: m.input?.path, summary: d.summary, body: d.body, spark: d.spark, rawArgs: undefined }), status: "running" };
      emit();
      return;
    }
  }
  cur.turn = { ...appendTool(cur.turn, { id: m.id, name: m.name, path: m.input?.path, running: true, summary: d.summary, body: d.body, spark: d.spark }), status: "running" };
  emit();
}

/** Subscribe to one agent's live turn state. Re-renders when it changes. */
export function useAgentTurn(agentId: string | null): TurnState {
  return useSyncExternalStore(
    subscribe,
    () => (agentId ? slots.get(agentId)?.turn ?? IDLE : IDLE),
  );
}

// SCOPED SLOTS: a surface that runs its own conversation with an agent (the
// Video editor dock) passes `slot: \`${agentId}::video\`` so its live stream and
// history live in a separate slot. The Chat screen (keyed by bare agentId) never
// sees it, and vice-versa. The backend still serializes per sessionId.
export const turnSlotKey = (agentId: string, scope: string) => `${agentId}::${scope}`;

/** The running history for an agent (used to seed a follow-up turn). */
export function getHistory(agentId: string): unknown[] {
  return slot(agentId).history;
}
export function setHistory(agentId: string, h: unknown[]) { slot(agentId).history = h; emit(); }

export function isRunning(agentId: string | null): boolean {
  return !!(agentId && slots.get(agentId)?.turn.status === "running");
}

/** Non-reactive read of an agent's current turn slice (for finalizing a save). */
export function getAgentTurnSnapshot(agentId: string): TurnState {
  return slots.get(agentId)?.turn ?? IDLE;
}

// --- HEADLESS (inter-agent) turn capture -----------------------------------
// Human turns call runTurn() which owns its own listener. But INTER-AGENT turns
// are started by the Rust drainer, not the UI — so nothing is subscribed to
// their stream channel. This standing watcher listens to the global
// `agent-activity` broadcast; when an agent's headless turn starts, it attaches
// a listener on that agent's `inbox-<id>` channel and writes the live stream
// into the SAME per-agent store slot. Result: open the recipient's pane (or the
// sender's) and you WATCH the inter-agent conversation happen, token by token.
// Called once at App mount.

let headlessWatcherStarted = false;
const headlessUnlisten = new Map<string, UnlistenFn>();

function inboxChannel(agentId: string): string { return `inbox-${agentId}`; }

async function attachHeadless(agentId: string) {
  if (headlessUnlisten.has(agentId)) return;
  const un = await listen<any>(inboxChannel(agentId), (ev) => {
    const m = ev.payload; if (!m) return;
    const cur = slot(agentId);
    const kind = m.kind || (m.TextDelta ? "TextDelta" : m.Info ? "Info" : m.ToolUseStart ? "ToolUseStart" : m.ToolUseDelta ? "ToolUseDelta" : m.ToolUse ? "ToolUse" : m.ToolResult ? "ToolResult" : m.InboundMessage ? "InboundMessage" : m.Done ? "Done" : null);
    const text = m.text ?? m.TextDelta?.text ?? m.Info?.text ?? "";
    if (handleToolStream(cur, kind, m)) return;
    if (kind === "InboundMessage") {
      // The peer's message arriving — surfaced as a live inbound bubble.
      cur.inbound = { fromName: m.fromName || m.from, text: m.text };
      cur.turn = { ...cur.turn, status: "running", liveText: "", liveTools: [], timeline: [] };
      emit();
    } else if (kind === "TextDelta") { cur.turn = { ...appendText(cur.turn, text), status: "running" }; emit(); }
    else if (kind === "Info") { cur.turn = { ...cur.turn, status: "running", info: text }; emit(); }
    else if (kind === "ToolUse") { upsertToolUse(cur, m); }
    else if (kind === "ToolResult") {
      const tools = cur.turn.liveTools;
      for (let i = tools.length - 1; i >= 0; i--) { if ((m.id ? tools[i].id === m.id : tools[i].name === m.name) && tools[i].running) { cur.turn = patchTool(cur.turn, i, { path: m.path || tools[i].path, ok: m.ok, detail: m.detail, running: false }); break; } }
      emit();
    }
  });
  headlessUnlisten.set(agentId, un);
}

/** Start the standing watcher for inter-agent (headless) turns. Idempotent. */
export async function startHeadlessWatcher() {
  if (headlessWatcherStarted) return;
  headlessWatcherStarted = true;
  await listen<any>("agent-activity", (ev) => {
    const a = ev.payload; if (!a?.agentId) return;
    if (a.kind === "turn_start") {
      // Don't clobber a HUMAN turn already running on this agent (that has its
      // own listener). Only attach for a headless turn.
      const cur = slot(a.agentId);
      if (cur.turn.status !== "running") { void attachHeadless(a.agentId); }
    } else if (a.kind === "turn_done") {
      const cur = slot(a.agentId);
      cur.turn = { ...cur.turn, status: "idle" };
      // UI task #1: headless/continuation turn persisted messages — bump so
      // viewing panes reload from disk (Chat.tsx effect).
      convVersion++;
      const un = headlessUnlisten.get(a.agentId);
      if (un) { try { un(); } catch { /* ignore */ } headlessUnlisten.delete(a.agentId); }
      emit();
    }
  });
}

// Bumped when a headless turn persists; panes subscribe to reload their conv.
let convVersion = 0;
export function useConvVersion(): number {
  return useSyncExternalStore(subscribe, () => convVersion);
}

/** The live inbound peer-message for an agent (if a headless turn is mid-flight). */
export function getInbound(agentId: string | null): { fromName: string; text: string } | undefined {
  return agentId ? slots.get(agentId)?.inbound : undefined;
}

// --- the turn runner --------------------------------------------------------

export type RunArgs = {
  agentId: string;
  channel: string;      // per-conversation event channel
  prompt: string;
  model: string | null;
  provider: string | null;
  folder: string | null;
  sessionId: string;
  attachments?: string[];  // jail-relative paths from chat_attach_file
  slot?: string;           // store slot key (default agentId) — see turnSlotKey
};

/**
 * STOP BUTTON: ask the backend to cancel the in-flight turn on this channel.
 * Fire-and-forget from the UI's perspective -- the actual turn winds down when
 * the Rust side notices the flag (next network chunk) and pushes a final
 * "stopped by user" message through the SAME event channel, so the UI's normal
 * finalize path (runTurn's `finally`) handles cleanup exactly like any other
 * turn ending. No local status flip here: that would race the real one.
 */
export async function stopTurn(channel: string): Promise<boolean> {
  try { return await invoke<boolean>("agent_stop", { channel }); }
  catch { return false; }
}

/**
 * Start a turn for an agent. Owns the event listener + accumulator in the store
 * so the stream is captured whether or not the agent is being viewed. Resolves
 * with the final provider-format history (also written into the store).
 */
export async function runTurn(a: RunArgs): Promise<unknown[]> {
  const key = a.slot ?? a.agentId;
  const s = slot(key);
  // Reset this agent's live turn.
  s.turn = { status: "running", liveText: "", liveTools: [], timeline: [] };
  emit();

  // Tear down any stale listener, then attach a fresh one that writes into the store.
  if (s.unlisten) { try { s.unlisten(); } catch { /* ignore */ } s.unlisten = undefined; }
  const un = await listen<any>(a.channel, (ev) => {
    const m = ev.payload;
    const cur = slot(key);
    if (!m) return;
    // Anthropic-style: {TextDelta:{text}} or {kind:"TextDelta"} — accept both.
    const kind = m.kind || (m.TextDelta ? "TextDelta" : m.Info ? "Info" : m.ToolUseStart ? "ToolUseStart" : m.ToolUseDelta ? "ToolUseDelta" : m.ToolUse ? "ToolUse" : m.ToolResult ? "ToolResult" : m.Usage ? "Usage" : m.Done ? "Done" : null);
    const text = m.text ?? m.TextDelta?.text ?? m.Info?.text ?? "";
    if (handleToolStream(cur, kind, m)) return;
    if (kind === "Usage") { cur.turn = accumulateUsage(cur.turn, m); emit(); }
    else if (kind === "TextDelta") { cur.turn = appendText(cur.turn, text); emit(); }
    else if (kind === "Info") { cur.turn = { ...cur.turn, info: text }; emit(); }
    else if (kind === "MemoryCaptured") { cur.turn = { ...cur.turn, memory: m.text }; emit(); }
    else if (kind === "ToolUse") { upsertToolUse(cur, m); }
    else if (kind === "ToolResult") {
      const tools = cur.turn.liveTools;
      // mark the last matching running tool done (id-matched when available;
      // spread keeps summary/body)
      for (let i = tools.length - 1; i >= 0; i--) { if ((m.id ? tools[i].id === m.id : tools[i].name === m.name) && tools[i].running) { cur.turn = patchTool(cur.turn, i, { path: m.path || tools[i].path, ok: m.ok, detail: m.detail, running: false }); break; } }
      emit();
    }
  });
  s.unlisten = un;

  try {
    const updated = await invoke<unknown[]>("agent_stream", {
      channel: a.channel, prompt: a.prompt, history: s.history,
      model: a.model, provider: a.provider, folder: a.folder,
      agentId: a.agentId, sessionId: a.sessionId,
      attachments: a.attachments ?? [],
    });
    slot(key).history = Array.isArray(updated) ? updated : s.history;
    return slot(key).history;
  } finally {
    const cur = slot(key);
    if (cur.unlisten) { try { cur.unlisten(); } catch { /* ignore */ } cur.unlisten = undefined; }
    cur.turn = { ...cur.turn, status: "idle" };
    emit();
  }
}
