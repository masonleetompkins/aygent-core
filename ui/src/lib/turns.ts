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

export type ToolCard = { name: string; path?: string; ok?: boolean; detail?: string; running?: boolean };
export type TurnMsg = { role: "user" | "assistant"; text: string; tools?: ToolCard[]; streaming?: boolean; from?: string };

export type TurnState = {
  status: "idle" | "running";
  liveText: string;          // accumulated streamed tokens for the in-flight assistant msg
  liveTools: ToolCard[];     // tool cards for the in-flight turn
  info?: string;             // latest info line (model, save point…)
  memory?: string;           // 🧠 auto-capture note for THIS turn (shown under the user msg)
  error?: string;
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

/** Subscribe to one agent's live turn state. Re-renders when it changes. */
export function useAgentTurn(agentId: string | null): TurnState {
  return useSyncExternalStore(
    subscribe,
    () => (agentId ? slots.get(agentId)?.turn ?? IDLE : IDLE),
  );
}

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
    const kind = m.kind || (m.TextDelta ? "TextDelta" : m.Info ? "Info" : m.ToolUse ? "ToolUse" : m.ToolResult ? "ToolResult" : m.InboundMessage ? "InboundMessage" : m.Done ? "Done" : null);
    const text = m.text ?? m.TextDelta?.text ?? m.Info?.text ?? "";
    if (kind === "InboundMessage") {
      // The peer's message arriving — surfaced as a live inbound bubble.
      cur.inbound = { fromName: m.fromName || m.from, text: m.text };
      cur.turn = { ...cur.turn, status: "running", liveText: "", liveTools: [] };
      emit();
    } else if (kind === "TextDelta") { cur.turn = { ...cur.turn, status: "running", liveText: cur.turn.liveText + text }; emit(); }
    else if (kind === "Info") { cur.turn = { ...cur.turn, status: "running", info: text }; emit(); }
    else if (kind === "ToolUse") { cur.turn = { ...cur.turn, status: "running", liveTools: [...cur.turn.liveTools, { name: m.name, path: m.input?.path, running: true }] }; emit(); }
    else if (kind === "ToolResult") {
      const tools = [...cur.turn.liveTools];
      for (let i = tools.length - 1; i >= 0; i--) { if (tools[i].name === m.name && tools[i].running) { tools[i] = { name: m.name, path: m.path, ok: m.ok, detail: m.detail, running: false }; break; } }
      cur.turn = { ...cur.turn, liveTools: tools }; emit();
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
  const s = slot(a.agentId);
  // Reset this agent's live turn.
  s.turn = { status: "running", liveText: "", liveTools: [] };
  emit();

  // Tear down any stale listener, then attach a fresh one that writes into the store.
  if (s.unlisten) { try { s.unlisten(); } catch { /* ignore */ } s.unlisten = undefined; }
  const un = await listen<any>(a.channel, (ev) => {
    const m = ev.payload;
    const cur = slot(a.agentId);
    if (!m) return;
    // Anthropic-style: {TextDelta:{text}} or {kind:"TextDelta"} — accept both.
    const kind = m.kind || (m.TextDelta ? "TextDelta" : m.Info ? "Info" : m.ToolUse ? "ToolUse" : m.ToolResult ? "ToolResult" : m.Done ? "Done" : null);
    const text = m.text ?? m.TextDelta?.text ?? m.Info?.text ?? "";
    if (kind === "TextDelta") { cur.turn = { ...cur.turn, liveText: cur.turn.liveText + text }; emit(); }
    else if (kind === "Info") { cur.turn = { ...cur.turn, info: text }; emit(); }
    else if (kind === "MemoryCaptured") { cur.turn = { ...cur.turn, memory: m.text }; emit(); }
    else if (kind === "ToolUse") {
      cur.turn = { ...cur.turn, liveTools: [...cur.turn.liveTools, { name: m.name, path: m.input?.path, running: true }] }; emit();
    } else if (kind === "ToolResult") {
      const tools = [...cur.turn.liveTools];
      // mark the last matching running tool done
      for (let i = tools.length - 1; i >= 0; i--) { if (tools[i].name === m.name && tools[i].running) { tools[i] = { name: m.name, path: m.path, ok: m.ok, detail: m.detail, running: false }; break; } }
      cur.turn = { ...cur.turn, liveTools: tools }; emit();
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
    slot(a.agentId).history = Array.isArray(updated) ? updated : s.history;
    return slot(a.agentId).history;
  } finally {
    const cur = slot(a.agentId);
    if (cur.unlisten) { try { cur.unlisten(); } catch { /* ignore */ } cur.unlisten = undefined; }
    cur.turn = { ...cur.turn, status: "idle" };
    emit();
  }
}
