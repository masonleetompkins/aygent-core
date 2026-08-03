// Chat — the primary agent surface. Streaming-first: tokens render live, tool
// calls appear as inline cards as they fire, multi-turn history persists.
// Consumes normalized StreamEvents from the Rust streaming agent loop over a
// Tauri event channel. Falls back to a thinking animation if no text streams.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/ui";
import { Icon, type IconName } from "../components/Icon";
import { Markdown } from "../components/Markdown";
import { runTurn, isRunning, setHistory, getAgentTurnSnapshot, useAgentTurn, getInbound, useConvVersion, stopTurn } from "../lib/turns";
import type { AgentProfile } from "../components/AgentSwitcher";

type ToolLine = { name: string; path: string; ok?: boolean; detail?: string };
type Msg =
  | { role: "user"; text: string; memory?: string }
  | { role: "assistant"; text: string; tools: ToolLine[]; streaming?: boolean };

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

type ConvMeta = { id: string; title: string; updated: number; pinned: boolean; order: number };

// MULTI-AGENT container: renders one ChatPane per open agent, side by side, each
// with its own name header. Focus (click a pane) sets it as the active agent so
// This-Agent screens (Tools/SavePoints/etc) + the folder scope track it.
export function Chat({
  keySet, paneIds, activeId, onClosePane, onFocusPane,
}: {
  keySet: boolean;
  paneIds: string[];
  activeId: string | null;
  onClosePane: (id: string) => void;
  onFocusPane: (id: string) => void;
}) {
  const [roster, setRoster] = useState<Record<string, AgentProfile>>({});
  useEffect(() => {
    invoke<{ agents: AgentProfile[] }>("agents_list")
      .then((r) => { const m: Record<string, AgentProfile> = {}; for (const a of (r.agents || [])) m[a.id] = a; setRoster(m); })
      .catch(() => {});
  }, [paneIds.join(",")]);

  if (paneIds.length === 0) {
    return <p style={hint}>No agent open. Pick one from the rail on the left, or create one in Agents.</p>;
  }

  return (
    <div style={{ display: "flex", height: "100%", minHeight: 0, gap: "var(--space-4)" }}>
      {paneIds.map((id) => (
        <div
          key={id}
          onPointerDownCapture={() => { if (id !== activeId) onFocusPane(id); }}
          style={{
            flex: 1, minWidth: 0, display: "flex", flexDirection: "column", minHeight: 0,
            // multi-pane: divider + subtle active outline so it's clear which
            // agent is focused (drives This-Agent screens + folder scope).
            border: paneIds.length > 1
              ? `var(--border-width) solid ${id === activeId ? "var(--accent)" : "var(--line)"}`
              : "none",
            borderRadius: paneIds.length > 1 ? "var(--radius-card)" : 0,
            padding: paneIds.length > 1 ? "var(--space-3)" : 0,
          }}
        >
          <ChatPane
            agent={roster[id] ?? null}
            agentId={id}
            folder={roster[id]?.folder_path ?? null}
            keySet={keySet}
            multi={paneIds.length > 1}
            closable={paneIds.length > 1}
            onClose={() => onClosePane(id)}
          />
        </div>
      ))}
    </div>
  );
}

function ChatPane({ agent, folder, keySet, agentId, multi, closable, onClose }: {
  agent: AgentProfile | null;
  folder: string | null; keySet: boolean; agentId: string | null;
  multi: boolean; closable: boolean; onClose: () => void;
}) {
  const [msgs, setMsgs] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  // `busy` is now DERIVED from the per-agent turn store (see `running` below),
  // not a local pane flag — so gating is per-agent (send to B while A runs).
  // Kept as a name for the sidebar's new-chat gate.

  const [convs, setConvs] = useState<ConvMeta[]>([]);
  const [convId, setConvId] = useState<string | null>(null);
  const historyRef = useRef<any>([]); // provider-format running history
  // convId in a REF too: `persist()` runs inside async closures that would
  // otherwise capture a STALE convId (the save-bug that dropped the first
  // chat). The ref is always the current thread id.
  const convIdRef = useRef<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // #4 @mention: the list of agents to offer in the picker, loaded once.
  const [allAgents, setAllAgents] = useState<Array<{ id: string; name: string; icon: string; color: string }>>([]);
  useEffect(() => {
    invoke<{ agents: Array<{ id: string; name: string; icon: string; color: string; archived: boolean }> }>("agents_list")
      .then((r) => setAllAgents((r.agents || []).filter((a) => !a.archived)))
      .catch(() => {});
  }, []);
  // Active @mention query state: { query, matches, sel, start } or null.
  const [mention, setMention] = useState<{ query: string; matches: typeof allAgents; sel: number; start: number } | null>(null);

  // ---- Task #5: attachments (+ button). ANY file becomes agent context ----
  const [attachments, setAttachments] = useState<Array<{ name: string; rel?: string; pending: boolean }>>([]);
  const fileRef = useRef<HTMLInputElement>(null);
  async function onFilesPicked(files: FileList | null) {
    if (!files || !agentId) return;
    for (const file of Array.from(files)) {
      setAttachments((a) => [...a, { name: file.name, pending: true }]);
      try {
        const buf = await file.arrayBuffer();
        // chunked btoa — String.fromCharCode(...bigArray) blows the stack
        const u8 = new Uint8Array(buf);
        let bin = "";
        for (let i = 0; i < u8.length; i += 0x8000) bin += String.fromCharCode(...u8.subarray(i, i + 0x8000));
        const b64 = btoa(bin);
        const rel = await invoke<string>("chat_attach_file", { agentId, filename: file.name, bytesB64: b64 });
        setAttachments((a) => a.map((x) => x.name === file.name ? { ...x, rel, pending: false } : x));
      } catch (err) {
        setAttachments((a) => a.filter((x) => x.name !== file.name));
        alert("Attach failed: " + String(err));
      }
    }
  }

  // ---- Task #7: mic button -> record -> Whisper -> input ----
  const [rec, setRec] = useState<"idle" | "recording" | "transcribing">("idle");
  const recRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  async function toggleMic() {
    if (rec === "recording") { recRef.current?.stop(); return; }
    if (rec !== "idle") return;
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const mime = ["audio/webm", "audio/mp4", "audio/mpeg", ""].find((m) => !m || MediaRecorder.isTypeSupported(m)) ?? "";
      const mr = mime ? new MediaRecorder(stream, { mimeType: mime }) : new MediaRecorder(stream);
      chunksRef.current = [];
      mr.ondataavailable = (e) => { if (e.data.size > 0) chunksRef.current.push(e.data); };
      mr.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());
        setRec("transcribing");
        try {
          const blob = new Blob(chunksRef.current, { type: "audio/webm" });
          const buf = await blob.arrayBuffer();
          const u8 = new Uint8Array(buf);
          let bin = "";
          for (let i = 0; i < u8.length; i += 0x8000) bin += String.fromCharCode(...u8.subarray(i, i + 0x8000));
          const ext = (recRef.current?.mimeType || "audio/webm").includes("mp4") ? "m4a"
            : (recRef.current?.mimeType || "").includes("mpeg") ? "mp3" : "webm";
          const text = await invoke<string>("transcribe_audio_b64", { b64: btoa(bin), filename: `recording.${ext}` });
          setInput((prev) => (prev ? prev + " " : "") + text);
          taRef.current?.focus();
        } catch (err) { alert("Transcription failed: " + String(err)); }
        setRec("idle");
      };
      mr.start();
      recRef.current = mr;
      setRec("recording");
    } catch (err) {
      const e = err as DOMException;
      const why = e?.name === "NotAllowedError"
        ? "macOS blocked the microphone. Check System Settings → Privacy & Security → Microphone → AYGENT."
        : e?.name === "NotFoundError" ? "No microphone found."
        : String(err);
      alert("Mic unavailable: " + why);
    }
  }

  // ---- Task #8: #tool tagging (mirrors @mentions) ----
  const [toolNames, setToolNames] = useState<string[]>([]);
  useEffect(() => {
    const base = ["read_file", "write_file", "list_files", "rename_file", "delete_file", "task_continue"];
    if (!folder) { setToolNames(base); return; }
    invoke<Array<{ name: string; enabled: boolean }>>("tools_list", { folder })
      .then((ts) => setToolNames([...base, ...ts.filter((t) => t.enabled && t.name).map((t) => t.name)]))
      .catch(() => setToolNames(base));
  }, [folder]);
  const [toolTag, setToolTag] = useState<{ query: string; matches: string[]; sel: number; start: number } | null>(null);

  // #3 auto-grow: single-line by default, grows with content up to a sane cap.
  // Reset to auto first so it can SHRINK too; when empty, scrollHeight collapses
  // to one line. Cap ~200px (~8 lines), not 50vh (that let an empty box balloon
  // to half the window inside the flex column). Mason 07-28.
  useEffect(() => {
    const ta = taRef.current; if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = Math.min(ta.scrollHeight, 200) + "px";
    // Task #2 (Mason 08-01): a growing input was COVERING the last message —
    // the messages column doesn't reflow on its own. Pin to bottom as we grow.
    scrollRef.current?.scrollTo({ top: 1e9 });
  }, [input]);

  // #4 detect an @mention token at the caret and surface matching agents.
  function onInputChange(value: string, caret: number) {
    // Find the @token immediately before the caret (letters/digits/_-, no space).
    const upto = value.slice(0, caret);
    const m = upto.match(/@([\w-]*)$/);
    if (m) {
      const q = m[1].toLowerCase();
      const matches = allAgents.filter((a) => a.name.toLowerCase().includes(q)).slice(0, 6);
      setMention({ query: m[1], matches, sel: 0, start: caret - m[0].length });
    } else {
      setMention(null);
    }
    // Task #8: #tool token at the caret — offer enabled tools.
    const t = upto.match(/#([\w-]*)$/);
    if (t) {
      const q = t[1].toLowerCase();
      const matches = toolNames.filter((n) => n.toLowerCase().includes(q)).slice(0, 6);
      setToolTag({ query: t[1], matches, sel: 0, start: caret - t[0].length });
    } else {
      setToolTag(null);
    }
  }

  function pickToolTag(name: string) {
    if (!toolTag) return;
    const before = input.slice(0, toolTag.start);
    const after = input.slice(toolTag.start + 1 + toolTag.query.length);
    const inserted = `#${name} `;
    setInput(before + inserted + after);
    setToolTag(null);
    requestAnimationFrame(() => {
      const ta = taRef.current; if (!ta) return;
      const pos = (before + inserted).length;
      ta.focus(); ta.setSelectionRange(pos, pos);
    });
  }

  // Insert the picked agent's @Name into the input, replacing the partial token.
  function pickMention(a: { id: string; name: string }) {
    if (!mention) return;
    const before = input.slice(0, mention.start);
    const after = input.slice(mention.start + 1 + mention.query.length);
    const inserted = `@${a.name} `;
    const next = before + inserted + after;
    setInput(next);
    setMention(null);
    // Restore focus + caret after the inserted mention.
    requestAnimationFrame(() => {
      const ta = taRef.current; if (!ta) return;
      const pos = (before + inserted).length;
      ta.focus(); ta.setSelectionRange(pos, pos);
    });
  }

  // #3 + #4 key handling: mention nav when open; else Enter=send, Shift+Enter=newline.
  function onInputKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (toolTag && toolTag.matches.length > 0) {
      if (e.key === "ArrowDown") { e.preventDefault(); setToolTag({ ...toolTag, sel: (toolTag.sel + 1) % toolTag.matches.length }); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); setToolTag({ ...toolTag, sel: (toolTag.sel - 1 + toolTag.matches.length) % toolTag.matches.length }); return; }
      if (e.key === "Enter" || e.key === "Tab") { e.preventDefault(); pickToolTag(toolTag.matches[toolTag.sel]); return; }
      if (e.key === "Escape") { e.preventDefault(); setToolTag(null); return; }
    }
    if (mention && mention.matches.length > 0) {
      if (e.key === "ArrowDown") { e.preventDefault(); setMention({ ...mention, sel: (mention.sel + 1) % mention.matches.length }); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); setMention({ ...mention, sel: (mention.sel - 1 + mention.matches.length) % mention.matches.length }); return; }
      if (e.key === "Enter" || e.key === "Tab") { e.preventDefault(); pickMention(mention.matches[mention.sel]); return; }
      if (e.key === "Escape") { e.preventDefault(); setMention(null); return; }
    }
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
    // Shift+Enter falls through -> newline (default textarea behavior).
  }
  // POINTER-BASED reorder. Native HTML5 draggable is broken in Tauri's macOS
  // WebKit webview (the row grays out on drag-start but `drop`/`dragend` never
  // fire, so the item stays stuck grey and nothing persists). We implement drag
  // with pointer events instead — works identically in every webview. `dragId`
  // = the row being dragged (for the grayed-out style); `overId` = the row the
  // pointer is currently over (drop target). A small move threshold keeps a
  // plain click from starting a drag.
  const [dragId, setDragId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);
  const dragRef = useRef<{ id: string; startY: number; active: boolean } | null>(null);
  const listElRef = useRef<HTMLDivElement>(null);
  // Per-folder selection (provider + model). "" model = auto/haiku; provider
  // "local" routes to the in-app llama.cpp engine. Loaded on folder change and
  // re-checked on each send so a Settings change applies without a reload.
  const modelRef = useRef<string>("");
  // STOP BUTTON: the channel id of the turn currently in flight on this pane
  // (set right before runTurn, cleared after) so the Stop button -- rendered
  // outside send()'s closure -- knows exactly which turn to cancel.
  const runningChannelRef = useRef<string | null>(null);
  const providerRef = useRef<string>("");

  useEffect(() => { scrollRef.current?.scrollTo({ top: 1e9, behavior: "smooth" }); }, [msgs]);

  function setConv(id: string | null) { convIdRef.current = id; setConvId(id); }

  // Set msgs in BOTH the ref (source of truth for send/persist) and state (render).
  // Keeping these in lockstep is critical: send() builds from msgsRef, so if a
  // switch/new only cleared state, the next send would append to the OLD thread
  // (the "previous chat bled into the new one" bug).
  function setMessages(next: Msg[]) { msgsRef.current = next; setMsgs(next); }

  // On folder change: load the list and open the most recent one (or a fresh one).
  useEffect(() => {
    if (!folder) { setConvs([]); setConv(null); setMessages([]); historyRef.current = []; return; }
    invoke<{ provider: string; model: string }>("get_selection", { folder })
      .then((s) => { providerRef.current = s.provider; modelRef.current = s.model; }).catch(() => {});
    (async () => {
      try {
        const list = await invoke<ConvMeta[]>("conv_list", { folder });
        setConvs(list);
        if (list.length > 0) await openConv(list[0].id);
        else newConv();
      } catch { newConv(); }
    })();
    // eslint-disable-next-line
  }, [folder]);

  async function refreshList() {
    if (!folder) return;
    try { setConvs(await invoke<ConvMeta[]>("conv_list", { folder })); } catch { /* ignore */ }
  }

  function newConv() {
    const id = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    setConv(id); setMessages([]); historyRef.current = [];
  }

  async function openConv(id: string) {
    // VIEWING is never blocked by `busy` — you can always read any conversation,
    // even while an agent is mid-turn. `busy` only gates SENDING (the lane
    // serializes turns; it must not lock the whole pane). This was bug #2.
    if (!folder) return;
    try {
      const c = await invoke<any>("conv_load", { folder, id });
      setConv(c.id);
      setMessages(Array.isArray(c.msgs) ? c.msgs : []);
      historyRef.current = Array.isArray(c.history) ? c.history : [];
    } catch { newConv(); }
  }

  async function deleteConv(id: string) {
    if (!folder) return;
    try { await invoke("conv_delete", { folder, id }); } catch { /* ignore */ }
    const list = await invoke<ConvMeta[]>("conv_list", { folder }).catch(() => [] as ConvMeta[]);
    setConvs(list);
    if (id === convIdRef.current) { if (list.length > 0) openConv(list[0].id); else newConv(); }
  }

  async function togglePin(id: string) {
    const c = convs.find((x) => x.id === id);
    if (!c || !folder) return;
    try {
      await invoke("conv_reorder", { folder, updates: [{ id, pinned: !c.pinned, order: c.order }] });
      await refreshList();
    } catch { /* ignore */ }
  }

  // Rename a chat by id: persist a new title via conv_save (loads the full
  // conversation first so we don't clobber msgs/history) then refresh. Used by
  // both the sidebar pencil (prompt) and the inline header field (direct value).
  async function saveConvTitle(id: string, next: string) {
    if (!folder) return;
    const title = next.trim();
    if (!title) return;
    try {
      const c = await invoke<any>("conv_load", { folder, id });
      await invoke("conv_save", { folder, conv: { ...c, title } });
      await refreshList();
    } catch { /* ignore */ }
  }
  async function renameConv(id: string) {
    const cur = convs.find((x) => x.id === id);
    const next = window.prompt("Rename chat", cur?.title || "");
    if (next == null) return;
    await saveConvTitle(id, next);
  }
  // Rename the CURRENTLY OPEN chat (from the inline header field).
  async function renameCurrent(next: string) {
    if (!convId) return;
    await saveConvTitle(convId, next);
  }

  // Persist a drop: move `srcId` to `targetId`'s slot, recompute a dense order
  // (1..n) for the whole list, and save it in one batch.
  async function commitReorder(srcId: string, targetId: string) {
    if (!folder || srcId === targetId) return;
    const ids = convs.map((c) => c.id);
    const from = ids.indexOf(srcId);
    const to = ids.indexOf(targetId);
    if (from < 0 || to < 0) return;
    const reordered = [...convs];
    const [moved] = reordered.splice(from, 1);
    reordered.splice(to, 0, moved);
    const updates = reordered.map((c, i) => ({ id: c.id, pinned: c.pinned, order: i + 1 }));
    setConvs(reordered.map((c, i) => ({ ...c, order: i + 1 })));
    try { await invoke("conv_reorder", { folder, updates }); await refreshList(); } catch { /* ignore */ }
  }

  // Hit-test: which row id is under this Y coordinate? Uses the row DOM nodes
  // (each tagged with data-conv-id) inside the scrollable list.
  function rowIdAtY(y: number): string | null {
    const container = listElRef.current;
    if (!container) return null;
    const rows = container.querySelectorAll<HTMLElement>("[data-conv-id]");
    for (const row of Array.from(rows)) {
      const r = row.getBoundingClientRect();
      if (y >= r.top && y <= r.bottom) return row.dataset.convId || null;
    }
    return null;
  }

  // Pointer drag lifecycle (replaces native DnD). Bound at the row level via
  // onPointerDown; move/up are tracked on window so the drag survives leaving
  // the row. A ~5px threshold distinguishes a drag from a click.
  function startPointerDrag(id: string, e: React.PointerEvent) {
    dragRef.current = { id, startY: e.clientY, active: false };
    const onMove = (ev: PointerEvent) => {
      const d = dragRef.current;
      if (!d) return;
      if (!d.active && Math.abs(ev.clientY - d.startY) > 5) {
        d.active = true;
        setDragId(d.id);
      }
      if (d.active) {
        setOverId(rowIdAtY(ev.clientY));
      }
    };
    const onUp = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      const d = dragRef.current;
      dragRef.current = null;
      setDragId(null);
      setOverId(null);
      if (d && d.active) {
        const target = rowIdAtY(ev.clientY);
        if (target) void commitReorder(d.id, target);
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  // Persist a conversation snapshot. Reads msgs from the passed array (source of
  // truth), derives the title from the first user message, uses convIdRef so it
  // never saves against a stale id. Called both on SEND (so the thread appears
  // immediately) and after the turn COMPLETES (to store the reply + history).
  const msgsRef = useRef<Msg[]>([]);
  async function persist(nextMsgs: Msg[]) {
    await persistFor(convIdRef.current, nextMsgs, historyRef.current);
  }

  // Persist against a SPECIFIC conv id + history (not the currently-viewed pane's
  // — avoids the stale-capture bug when a turn finishes after you've navigated
  // away). Atlas: persist against the turn's captured convId.
  async function persistFor(id: string | null, nextMsgs: Msg[], hist: unknown[]) {
    if (!folder || !id) return;
    const firstUser = nextMsgs.find((m) => m.role === "user") as { text: string } | undefined;
    const title = (firstUser?.text?.trim() || "New chat").slice(0, 60);
    try {
      await invoke("conv_save", {
        folder,
        conv: { id, title, updated: 0, pinned: false, order: 0, msgs: nextMsgs, history: hist },
      });
      await refreshList();
    } catch { /* non-fatal: chat still works even if save fails */ }
  }

  async function send() {
    let prompt = input.trim();
    // Task #8: #tool tags become an explicit instruction the model honors.
    const tagged = [...new Set((prompt.match(/#([\w-]+)/g) || []).map((x) => x.slice(1)).filter((n) => toolNames.includes(n)))];
    if (tagged.length > 0) prompt += `\n\n(Use the ${tagged.join(", ")} tool${tagged.length > 1 ? "s" : ""} for this.)`;
    // Attachments ride into the model as real content blocks (images are SEEN).
    const attRels = attachments.filter((a) => !a.pending && a.rel).map((a) => a.rel!);
    const attNames = attachments.filter((a) => !a.pending).map((a) => a.name);
    if (attNames.length > 0) {
      prompt += `\n\n(Attached: ${attNames.join(", ")})`;
      setAttachments([]);
    }
    // Gate on THIS agent's status (per-agent), not a global pane flag — so you
    // can send to a second agent while the first still runs (Atlas #2).
    if (!prompt || !agentId || isRunning(agentId)) return;
    setInput("");

    const myAgent = agentId;
    const myConvId = convIdRef.current || `agent://${Date.now()}`;
    const channel = myConvId; // per-conversation channel = the stable session id

    // Optimistic user bubble ONLY. Do NOT append a static assistant placeholder:
    // live tokens land in the store (turns.ts) and render via the trailing
    // streaming bubble below — a placeholder here matched that bubble's
    // suppression condition and blocked live streaming entirely (bug: responses
    // appeared all-at-once). Cleo 2026-07-31.
    const withUser: Msg[] = [...msgsRef.current, { role: "user", text: prompt }];
    msgsRef.current = withUser;
    setMsgs(withUser);
    void persist(withUser); // thread appears in the sidebar immediately

    // Seed the store's per-agent history from this conversation so a follow-up
    // continues the thread.
    setHistory(myAgent, historyRef.current);

    // Refresh model/provider selection just before the call.
    if (folder) {
      try {
        const s = await invoke<{ provider: string; model: string }>("get_selection", { folder });
        providerRef.current = s.provider; modelRef.current = s.model;
      } catch { /* keep last */ }
    }

    runningChannelRef.current = channel;
    try {
      // runTurn OWNS the listener + accumulator in the App-level store, so the
      // stream keeps landing even if you navigate away. It resolves with the
      // final history. The live text/tools render via the subscribed `turn`
      // slice below (see the streaming bubble), so no local mirror needed.
      const updated = await runTurn({
        agentId: myAgent, channel, prompt,
        model: modelRef.current || null,
        provider: providerRef.current || null,
        folder: folder || null,
        sessionId: myConvId,
        attachments: attRels,
      });
      historyRef.current = updated;
      // Compose the final saved msgs from the store's completed live slice.
      const done = getAgentTurnSnapshot(myAgent);
      // Attach the 🧠 auto-capture note to the USER message that triggered it, so
      // it renders as a small badge under that bubble AND persists (the live
      // turn.memory is discarded on finalize otherwise). Mason 07-28.
      const withUserMem: Msg[] = withUser.map((mm, idx) =>
        idx === withUser.length - 1 && mm.role === "user" && done.memory
          ? { ...mm, memory: done.memory } : mm
      );
      // BUG FIX (Mason 08-02): a genuinely empty final answer (no crash, no loop
      // exhaustion -- the model just returned no text, e.g. after only viewing an
      // attachment) rendered as a bare "(done)", indistinguishable from the OLD
      // silent-failure bug this session already fixed once. Make the fallback
      // say what actually happened instead of a cryptic placeholder, and nudge
      // toward the fix (ask a follow-up) rather than leaving it a dead end.
      const emptyReplyText = done.liveTools.length > 0
        ? "(ran " + done.liveTools.length + " tool" + (done.liveTools.length > 1 ? "s" : "") + " but sent no written reply — try asking a follow-up, e.g. ‘what did you find?’)"
        : "(no reply text came back from the model this turn — try asking a follow-up)";
      const finalMsgs: Msg[] = [
        ...withUserMem,
        { role: "assistant", text: done.liveText || emptyReplyText, tools: done.liveTools as ToolLine[], streaming: false },
      ];
      // Only overwrite the visible pane if we're STILL viewing this agent+conv.
      if (agentId === myAgent && convIdRef.current === myConvId) {
        msgsRef.current = finalMsgs; setMsgs(finalMsgs);
      }
      void persistFor(myConvId, finalMsgs, historyRef.current);
      runningChannelRef.current = null;
    } catch (err) {
      const errMsgs: Msg[] = [...withUser, { role: "assistant", text: `✗ ${String(err)}`, tools: [], streaming: false }];
      if (agentId === myAgent && convIdRef.current === myConvId) { msgsRef.current = errMsgs; setMsgs(errMsgs); }
      void persistFor(myConvId, errMsgs, historyRef.current);
      runningChannelRef.current = null;
    }
  }

  // STOP BUTTON: cancel whatever turn is currently in flight on THIS pane.
  // Fire-and-forget -- the turn winds down on its own (the backend notices the
  // cancel flag on the next network chunk and finishes cleanly with a
  // "stopped by user" message), which flows through the normal runTurn/finally
  // path exactly like any other turn ending. Nothing to await here.
  function stop() {
    const ch = runningChannelRef.current;
    if (ch) void stopTurn(ch);
  }

  const blocked = !folder || !keySet;

  // Per-agent live turn (from the App-level store). This is what makes switching
  // TO a running agent show its live stream + thinking dots — the store never
  // unmounts, so the stream is always captured and any pane can reattach.
  const turn = useAgentTurn(agentId);
  const running = turn.status === "running";
  // Follow the live stream: msgs is static mid-turn now, so scroll on liveText.
  useEffect(() => { scrollRef.current?.scrollTo({ top: 1e9 }); }, [turn.liveText]);
  // UI task #1: reload the viewed conv when a headless/continuation turn
  // persists, so the report STAYS on screen instead of vanishing.
  const convVersion = useConvVersion();
  useEffect(() => {
    if (convVersion > 0 && convIdRef.current && !isRunning(agentId)) { void openConv(convIdRef.current); }
    // eslint-disable-next-line
  }, [convVersion]);

  return (
    <div style={{ display: "flex", height: "100%", minHeight: 0, gap: "var(--space-4)" }}>
      {/* MAIN CHAT COLUMN. In multi-pane mode it flexes to share width; solo it
         stays centered. height:100% + flex so the input pins to the bottom. */}
      <div style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 0, minHeight: 0, maxWidth: multi ? "none" : 720, margin: multi ? 0 : "0 auto" }}>
        {/* Header: agent name (multi) or "Chat" + editable chat name underneath.
           In multi-pane, each pane is labeled with its AGENT so you always know
           who you're talking to; a close button removes just this pane. */}
        <div style={{ margin: "0 0 var(--space-3)", flexShrink: 0, display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 8 }}>
          <div style={{ minWidth: 0 }}>
            <h2 style={{ fontSize: "var(--text-h1)", fontWeight: "var(--weight-heading)", margin: 0, display: "flex", alignItems: "center", gap: 8, overflow: "hidden" }}>
              {multi && <span style={{ color: "var(--accent)", display: "flex" }}><Icon name={(agent?.icon as IconName) || "sparkles"} size={20} /></span>}
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{multi ? (agent?.name || "Agent") : "Chat"}</span>
            </h2>
            <ChatTitle
              title={convs.find((c) => c.id === convId)?.title || ""}
              disabled={!folder || !convId}
              onRename={(next) => renameCurrent(next)}
            />
          </div>
          {closable && (
            <button
              onClick={onClose}
              title="Close this pane"
              style={{ background: "none", border: "none", cursor: "pointer", padding: 4, display: "flex", color: "var(--text-muted)", flexShrink: 0 }}
            ><Icon name="close" size={16} /></button>
          )}
        </div>

        {blocked && (
          <p style={{ ...hint, marginBottom: 12 }}>
            {!folder ? "Pick an Agent Folder in Settings, " : ""}{!keySet ? "add an Anthropic key in Settings" : ""} to start.
          </p>
        )}

        {/* Messages bottom-align: newest sits just above the input, older scroll
           up (justifyContent flex-end + margin-top auto on the list wrapper). */}
        <div ref={scrollRef} className="aygent-scroll" style={{ flex: 1, overflowY: "auto", overflowX: "hidden", display: "flex", flexDirection: "column", padding: "6px 28px 36px 28px" }}>
          <div style={{ marginTop: "auto", display: "flex", flexDirection: "column", gap: 14 }}>
          {msgs.length === 0 && !running && !blocked && (
            <p style={hint}>Say hello, or ask your agent to work with files in your folder.</p>
          )}
          {msgs.map((m, i) => <Bubble key={i} m={m} />)}
          {/* LIVE inter-agent inbound message: when a peer dispatches a message
              to the agent you're viewing, show it as a user bubble immediately
              (before the reply streams) so you WATCH the conversation arrive. */}
          {running && getInbound(agentId) && (
            <Bubble m={{ role: "user", text: `from ${getInbound(agentId)!.fromName}: ${getInbound(agentId)!.text}` }} />
          )}
          {/* LIVE turn for the agent being viewed: render a trailing streaming
              bubble fed by the store, so switching to a running agent shows its
              tokens + tool cards arriving mid-flight (Atlas #2), for BOTH human
              turns and headless inter-agent turns (same store slot). */}
          {running && !(msgs.length > 0 && msgs[msgs.length - 1].role === "assistant" && (msgs[msgs.length - 1] as { streaming?: boolean }).streaming) && (
            <Bubble m={{
              role: "assistant",
              text: turn.liveText,
              tools: turn.liveTools.map((t) => ({ name: t.name, path: t.path ?? "", ok: t.ok, detail: t.detail })),
              streaming: true,
            }} />
          )}
          </div>
        </div>

        {/* Task #5: attachment chips above the input */}
        {attachments.length > 0 && (
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 8 }}>
            {attachments.map((a) => (
              <span key={a.name} style={{
                display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12,
                padding: "4px 10px", borderRadius: 999, border: "var(--border-width) solid var(--line)",
                background: "var(--surface)", color: a.pending ? "var(--text-faint)" : "var(--text)",
              }}>
                📎 {a.name}{a.pending ? "…" : ""}
                {!a.pending && (
                  <button onClick={() => setAttachments((x) => x.filter((y) => y.name !== a.name))}
                    style={{ background: "none", border: "none", cursor: "pointer", padding: 0, color: "var(--text-muted)" }}>✕</button>
                )}
              </span>
            ))}
          </div>
        )}
        <div style={{ display: "flex", gap: 8, marginTop: "var(--space-3)", flexShrink: 0, alignItems: "flex-end", position: "relative" }}>
          {/* Task #8: #tool picker (mirrors the @ picker) */}
          {toolTag && toolTag.matches.length > 0 && (
            <div style={{
              position: "absolute", bottom: "calc(100% + 6px)", left: 0, minWidth: 220,
              background: "var(--surface)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", boxShadow: "var(--elevation)", overflow: "hidden", zIndex: 21,
            }}>
              {toolTag.matches.map((n, i) => (
                <button key={n} onMouseDown={(e) => { e.preventDefault(); pickToolTag(n); }}
                  style={{
                    display: "flex", alignItems: "center", gap: 8, width: "100%", textAlign: "left",
                    padding: "8px 12px", border: "none", cursor: "pointer", fontSize: 13,
                    fontFamily: "ui-monospace, monospace",
                    background: i === toolTag.sel ? "var(--bg)" : "transparent", color: "var(--text)",
                  }}>⚙ {n}</button>
              ))}
            </div>
          )}
          {/* Task #7: mic — record voice, Whisper transcribes into the input */}
          <button onClick={toggleMic} disabled={blocked} title={rec === "recording" ? "Stop recording" : "Record voice"}
            style={{
              width: 40, height: 44, flexShrink: 0, cursor: "pointer",
              // center the SVG glyph (Mason's screenshot: it sat top-left; a raw
              // button only centers TEXT, not inline SVG).
              display: "flex", alignItems: "center", justifyContent: "center", padding: 0,
              background: rec === "recording" ? "var(--danger)" : "var(--bg)",
              border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
              color: rec === "recording" ? "#fff" : "var(--text-muted)", fontSize: 16,
            }}>{rec === "transcribing" ? "…" : <Icon name={rec === "recording" ? "stop" : "mic"} size={18} />}</button>
          {/* Task #5: + attach any file as context */}
          <input ref={fileRef} type="file" multiple style={{ display: "none" }}
            onChange={(e) => { void onFilesPicked(e.target.files); e.target.value = ""; }} />
          <button onClick={() => fileRef.current?.click()} disabled={blocked} title="Attach files as context"
            style={{
              width: 40, height: 44, flexShrink: 0, cursor: "pointer",
              display: "flex", alignItems: "center", justifyContent: "center", padding: 0,
              background: "var(--bg)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", color: "var(--text-muted)", fontSize: 20,
            }}>+</button>
          {/* #4: @mention picker — shows matching agents as you type @Name. */}
          {mention && mention.matches.length > 0 && (
            <div style={{
              position: "absolute", bottom: "calc(100% + 6px)", left: 0, minWidth: 220,
              background: "var(--surface)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", boxShadow: "var(--elevation)", overflow: "hidden", zIndex: 20,
            }}>
              {mention.matches.map((a, i) => (
                <button key={a.id} onMouseDown={(e) => { e.preventDefault(); pickMention(a); }}
                  style={{
                    display: "flex", alignItems: "center", gap: 8, width: "100%", textAlign: "left",
                    padding: "8px 12px", border: "none", cursor: "pointer", fontSize: 14,
                    background: i === mention.sel ? "var(--bg)" : "transparent", color: "var(--text)",
                  }}>
                  <span style={{ width: 22, height: 22, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--accent)" }}><Icon name={(a.icon as IconName) || "sparkles"} size={18} /></span>
                  <span>{a.name}</span>
                </button>
              ))}
            </div>
          )}
          {/* #3: auto-growing multiline textarea; Shift+Enter = newline, Enter = send. */}
          <textarea
            ref={taRef}
            value={input} disabled={blocked || running}
            rows={1}
            onChange={(e) => { setInput(e.target.value); onInputChange(e.target.value, e.target.selectionStart); }}
            onKeyDown={onInputKeyDown}
            placeholder={blocked ? "Set up folder + key in Settings first…" : "Message your agent…  (@ agent · # tool)"}
            style={{
              flex: 1, resize: "none", overflowY: "auto",
              // fixed single-line start; JS auto-grow adjusts height up to 200px.
              // NO flex-stretch on height: alignItems:flex-end on the row + a set
              // height keep it compact instead of filling the column.
              height: 44, maxHeight: 200, lineHeight: 1.5,
              boxSizing: "border-box",
              background: "var(--bg)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", color: "var(--text)", padding: "10px 12px",
              fontSize: 15, fontFamily: "inherit",
            }} />
          <Button onClick={running ? stop : send} disabled={blocked || (!running && !input.trim())}>{running ? "Stop" : "Send"}</Button>
        </div>
      </div>

      {/* HISTORY SIDEBAR — right-hand side, so the active chat stays centered */}
      {!blocked && (
        <HistorySidebar
          convs={convs} activeId={convId} busy={running} dragId={dragId} overId={overId}
          listElRef={listElRef}
          onNew={newConv} onOpen={openConv} onDelete={deleteConv} onRename={renameConv}
          onPin={togglePin} onPointerDragStart={startPointerDrag}
        />
      )}
    </div>
  );
}

function HistorySidebar({
  convs, activeId, busy, dragId, overId, listElRef, onNew, onOpen, onDelete, onRename, onPin, onPointerDragStart,
}: {
  convs: ConvMeta[]; activeId: string | null; busy: boolean;
  dragId: string | null; overId: string | null;
  listElRef: React.RefObject<HTMLDivElement>;
  onNew: () => void; onOpen: (id: string) => void; onDelete: (id: string) => void;
  onRename: (id: string) => void;
  onPin: (id: string) => void;
  onPointerDragStart: (id: string, e: React.PointerEvent) => void;
}) {
  return (
    <div style={{
      width: 230, flexShrink: 0, display: "flex", flexDirection: "column", gap: 8,
      // Bleed past App.tsx's 28px top/bottom content padding so the divider
      // reaches the literal top and bottom of the window. height:100% alone
      // does NOT do this with negative margins -- a negative margin SHIFTS a
      // box, it doesn't stretch it, so height:100% + marginTop:-28 moved the
      // top up 28px but left the bottom 28px short (the exact bug Mason
      // caught). Grow the height by the full bled amount (28 top + 28 bottom)
      // so the box actually stretches past both edges instead of relocating.
      height: "calc(100% + 56px)",
      marginTop: -28, marginBottom: -28, paddingTop: 28, paddingBottom: 28,
      borderLeft: "var(--border-width) solid var(--line)", paddingLeft: 14,
    }}>
      <Button onClick={onNew} disabled={busy}>+ New chat</Button>
      <div ref={listElRef} className="aygent-scroll" style={{ overflowY: "auto", display: "flex", flexDirection: "column", gap: 4, marginTop: 4, flex: 1, minHeight: 0 }}>
        {convs.length === 0 && (
          <p style={{ ...hint, fontSize: 13, color: "var(--text-faint)" }}>No chats yet.</p>
        )}
        {convs.map((c) => (
          <HistoryItem
            key={c.id} c={c} active={c.id === activeId}
            dragging={dragId === c.id} isOver={overId === c.id && dragId !== null && dragId !== c.id}
            onOpen={() => onOpen(c.id)} onDelete={() => onDelete(c.id)} onRename={() => onRename(c.id)} onPin={() => onPin(c.id)}
            onPointerDown={(e) => onPointerDragStart(c.id, e)}
          />
        ))}
      </div>
    </div>
  );
}

function HistoryItem({
  c, active, dragging, isOver, onOpen, onDelete, onRename, onPin, onPointerDown,
}: {
  c: ConvMeta; active: boolean; dragging: boolean; isOver: boolean;
  onOpen: () => void; onDelete: () => void; onRename: () => void; onPin: () => void;
  onPointerDown: (e: React.PointerEvent) => void;
}) {
  const [hover, setHover] = useState(false);
  return (
    <div
      data-conv-id={c.id}
      onPointerDown={onPointerDown}
      onClick={onOpen}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      title={c.title || "Untitled"}
      style={{
        display: "flex", alignItems: "center", gap: 6, cursor: "pointer",
        padding: "8px 10px", borderRadius: "var(--radius-control)", fontSize: 13,
        userSelect: "none", touchAction: "none",
        border: `var(--border-width) solid ${isOver ? "var(--accent)" : active ? "var(--line)" : "transparent"}`,
        background: active ? "var(--bg)" : hover ? "var(--surface)" : "transparent",
        boxShadow: active ? "var(--elevation)" : "none",
        opacity: dragging ? 0.4 : 1,
      }}
    >
      <button
        onClick={(e) => { e.stopPropagation(); onPin(); }}
        title={c.pinned ? "Unpin" : "Pin to top"}
        style={{ background: "none", border: "none", cursor: "pointer", padding: 0, display: "flex", color: c.pinned ? "var(--accent)" : "var(--text-muted)", opacity: c.pinned ? 1 : hover ? 0.6 : 0 }}
      ><Icon name={c.pinned ? "pin-fill" : "pin"} size={13} /></button>
      <span style={{ flex: 1, minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", color: "var(--text)" }}>
        {c.title || "Untitled"}
      </span>
      {hover && (
        <>
          <button
            onClick={(e) => { e.stopPropagation(); onRename(); }}
            title="Rename chat"
            style={{ background: "none", border: "none", cursor: "pointer", padding: 0, display: "flex", color: "var(--text-muted)" }}
          ><Icon name="pencil" size={13} /></button>
          <button
            onClick={(e) => { e.stopPropagation(); onDelete(); }}
            title="Delete chat"
            style={{ background: "none", border: "none", cursor: "pointer", padding: 0, display: "flex", color: "var(--danger)" }}
          ><Icon name="trash" size={13} /></button>
        </>
      )}
    </div>
  );
}

// Inline-editable chat name shown under the "Chat" header. Click to edit; Enter
// or blur commits, Escape cancels. Empty renders a muted "Untitled" prompt.
function ChatTitle({ title, disabled, onRename }: { title: string; disabled: boolean; onRename: (next: string) => void; }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const inRef = useRef<HTMLInputElement>(null);
  useEffect(() => { if (!editing) setDraft(title); }, [title, editing]);
  useEffect(() => { if (editing) inRef.current?.focus(); }, [editing]);
  function commit() { const t = draft.trim(); if (t && t !== title) onRename(t); setEditing(false); }
  if (disabled) return null;
  if (editing) {
    return (
      <input
        ref={inRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); commit(); } else if (e.key === "Escape") { setDraft(title); setEditing(false); } }}
        placeholder="Untitled"
        style={{
          marginTop: 2, background: "var(--bg)", border: "var(--border-width) solid var(--accent)",
          borderRadius: "var(--radius-control)", color: "var(--text)", padding: "2px 8px",
          fontSize: "var(--text-body)", fontFamily: "inherit", maxWidth: 360, width: "100%",
        }}
      />
    );
  }
  return (
    <button
      onClick={() => setEditing(true)}
      title="Rename this chat"
      style={{
        marginTop: 2, display: "flex", alignItems: "center", gap: 6, background: "none", border: "none",
        cursor: "text", padding: "2px 0", color: title ? "var(--text-muted)" : "var(--text-faint)",
        fontSize: "var(--text-body)", fontFamily: "inherit", maxWidth: 360,
      }}
    >
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{title || "Untitled"}</span>
      <Icon name="pencil" size={13} style={{ opacity: 0.6 }} />
    </button>
  );
}

function Bubble({ m }: { m: Msg }) {
  const isUser = m.role === "user";
  const memory = isUser && m.role === "user" ? m.memory : undefined;
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: isUser ? "flex-end" : "flex-start" }}>
      <div style={{
        maxWidth: "82%",
        minWidth: 0,
        overflowWrap: "anywhere",
        background: isUser ? "var(--accent)" : "var(--surface)",
        color: isUser ? "var(--bg)" : "var(--text)",
        border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-card)",
        boxShadow: "var(--elevation)",
        padding: "12px 15px",
        display: "flex", flexDirection: "column", gap: 8,
      }}>
        {!isUser && m.role === "assistant" && m.tools.map((t, i) => <ToolCard key={i} t={t} />)}
        {m.text && (isUser
          ? <span style={{ whiteSpace: "pre-wrap", lineHeight: 1.55, fontSize: 15 }}>{m.text}</span>
          : <Markdown text={m.text} />)}
        {!isUser && m.role === "assistant" && m.streaming && !m.text && <Thinking />}
      </div>
      {memory && (
        <span style={{ marginTop: 3, marginRight: 4, fontSize: 12, color: "#3fa46a" }}>{memory}</span>
      )}
    </div>
  );
}

function ToolCard({ t }: { t: ToolLine }) {
  const pending = t.ok === undefined;
  const color = pending ? "var(--text-muted)" : t.ok ? "var(--ok)" : "var(--danger)";
  // A path is "revealable" once the call succeeded and points at a real file
  // (list_files on '.' or a refused call has nothing useful to reveal).
  const revealable = t.ok === true && !!t.path && t.path !== ".";

  async function reveal() {
    try { await invoke("reveal_in_finder", { path: t.path }); }
    catch (err) { console.warn("reveal failed:", err); }
  }

  return (
    <div style={{
      display: "flex", alignItems: "center", gap: 8,
      fontFamily: "ui-monospace, monospace", fontSize: 12.5,
      border: `var(--border-width) solid ${color}`, color,
      borderRadius: "var(--radius-control)", padding: "6px 10px",
      background: "var(--bg)",
    }}>
      <span>
        ⚙ {t.name}(
        {revealable ? (
          <button
            onClick={reveal}
            title="Reveal in Finder"
            style={{
              font: "inherit", color: "inherit", background: "none", border: "none",
              padding: 0, cursor: "pointer", textDecoration: "underline",
              textUnderlineOffset: 2,
            }}
          >{t.path}</button>
        ) : t.path}
        )
      </span>
      <span style={{ marginLeft: "auto" }}>{pending ? "…" : t.ok ? "✓" : `✗ ${t.detail || "refused"}`}</span>
    </div>
  );
}

function Thinking() {
  return (
    <span style={{ display: "inline-flex", gap: 4, alignItems: "center", color: "var(--text-muted)" }}>
      {[0, 1, 2].map((i) => (
        <span key={i} style={{
          width: 6, height: 6, borderRadius: 999, background: "currentColor",
          animation: `aygentPulse 1s ${i * 0.15}s infinite ease-in-out`,
        }} />
      ))}
      <style>{`@keyframes aygentPulse{0%,100%{opacity:.25}50%{opacity:1}}`}</style>
    </span>
  );
}
