// Chat — the primary agent surface. Streaming-first: tokens render live, tool
// calls appear as inline cards as they fire, multi-turn history persists.
// Consumes normalized StreamEvents from the Rust streaming agent loop over a
// Tauri event channel. Falls back to a thinking animation if no text streams.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/ui";
import { Markdown } from "../components/Markdown";
import { runTurn, isRunning, setHistory, getAgentTurnSnapshot, useAgentTurn, getInbound } from "../lib/turns";

type ToolLine = { name: string; path: string; ok?: boolean; detail?: string };
type Msg =
  | { role: "user"; text: string; memory?: string }
  | { role: "assistant"; text: string; tools: ToolLine[]; streaming?: boolean };

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

type ConvMeta = { id: string; title: string; updated: number; pinned: boolean; order: number };

export function Chat({ folder, keySet, agentId }: { folder: string | null; keySet: boolean; agentId: string | null }) {
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

  // #3 auto-grow: resize the textarea to fit its content, capped at 50vh.
  useEffect(() => {
    const ta = taRef.current; if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = Math.min(ta.scrollHeight, Math.floor(window.innerHeight * 0.5)) + "px";
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
    const prompt = input.trim();
    // Gate on THIS agent's status (per-agent), not a global pane flag — so you
    // can send to a second agent while the first still runs (Atlas #2).
    if (!prompt || !agentId || isRunning(agentId)) return;
    setInput("");

    const myAgent = agentId;
    const myConvId = convIdRef.current || `agent://${Date.now()}`;
    const channel = myConvId; // per-conversation channel = the stable session id

    // Optimistic user bubble + a streaming assistant placeholder in the pane.
    const withUser: Msg[] = [...msgsRef.current, { role: "user", text: prompt }];
    msgsRef.current = [...withUser, { role: "assistant", text: "", tools: [], streaming: true }];
    setMsgs(msgsRef.current);
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
      const finalMsgs: Msg[] = [
        ...withUserMem,
        { role: "assistant", text: done.liveText || "(done)", tools: done.liveTools as ToolLine[], streaming: false },
      ];
      // Only overwrite the visible pane if we're STILL viewing this agent+conv.
      if (agentId === myAgent && convIdRef.current === myConvId) {
        msgsRef.current = finalMsgs; setMsgs(finalMsgs);
      }
      void persistFor(myConvId, finalMsgs, historyRef.current);
    } catch (err) {
      const errMsgs: Msg[] = [...withUser, { role: "assistant", text: `✗ ${String(err)}`, tools: [], streaming: false }];
      if (agentId === myAgent && convIdRef.current === myConvId) { msgsRef.current = errMsgs; setMsgs(errMsgs); }
      void persistFor(myConvId, errMsgs, historyRef.current);
    }
  }

  const blocked = !folder || !keySet;

  // Per-agent live turn (from the App-level store). This is what makes switching
  // TO a running agent show its live stream + thinking dots — the store never
  // unmounts, so the stream is always captured and any pane can reattach.
  const turn = useAgentTurn(agentId);
  const running = turn.status === "running";

  return (
    <div style={{ display: "flex", height: "calc(100vh - 130px)", gap: 16 }}>
      {/* MAIN CHAT COLUMN (stays centered/left; history lives on the RIGHT) */}
      <div style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 0, maxWidth: 720, margin: "0 auto" }}>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: "0 0 12px" }}>Chat</h2>

        {blocked && (
          <p style={{ ...hint, marginBottom: 12 }}>
            {!folder ? "Pick an Agent Folder in Settings, " : ""}{!keySet ? "add an Anthropic key in Settings" : ""} to start.
          </p>
        )}

        <div ref={scrollRef} style={{ flex: 1, overflowY: "auto", display: "flex", flexDirection: "column", gap: 14, paddingRight: 6 }}>
          {msgs.length === 0 && !running && !blocked && (
            <p style={hint}>Say hello, or ask your agent to work with files in your folder.</p>
          )}
          {msgs.map((m, i) => <Bubble key={i} m={m} />)}
          {/* LIVE inter-agent inbound message: when a peer dispatches a message
              to the agent you're viewing, show it as a user bubble immediately
              (before the reply streams) so you WATCH the conversation arrive. */}
          {running && getInbound(agentId) && (
            <Bubble m={{ role: "user", text: `📨 from ${getInbound(agentId)!.fromName}: ${getInbound(agentId)!.text}` }} />
          )}
          {/* LIVE turn for the agent being viewed: render a trailing streaming
              bubble fed by the store, so switching to a running agent shows its
              tokens + tool cards arriving mid-flight (Atlas #2), for BOTH human
              turns and headless inter-agent turns (same store slot). */}
          {running && (
            <Bubble m={{
              role: "assistant",
              text: turn.liveText,
              tools: turn.liveTools.map((t) => ({ name: t.name, path: t.path ?? "", ok: t.ok, detail: t.detail })),
              streaming: true,
            }} />
          )}
        </div>

        <div style={{ display: "flex", gap: 8, marginTop: 14, alignItems: "flex-end", position: "relative" }}>
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
                  <span style={{ width: 22, height: 22, borderRadius: 6, background: a.color, color: "#fff", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 13 }}>{a.icon || "🤖"}</span>
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
            placeholder={blocked ? "Set up folder + key in Settings first…" : "Message your agent…  (@ to call another agent)"}
            style={{
              flex: 1, resize: "none", overflowY: "auto",
              maxHeight: "50vh", minHeight: 42, lineHeight: 1.5,
              background: "var(--bg)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", color: "var(--text)", padding: "10px 12px",
              fontSize: 15, fontFamily: "inherit",
            }} />
          <Button onClick={send} disabled={blocked || running}>{running ? "…" : "Send"}</Button>
        </div>
      </div>

      {/* HISTORY SIDEBAR — right-hand side, so the active chat stays centered */}
      {!blocked && (
        <HistorySidebar
          convs={convs} activeId={convId} busy={running} dragId={dragId} overId={overId}
          listElRef={listElRef}
          onNew={newConv} onOpen={openConv} onDelete={deleteConv}
          onPin={togglePin} onPointerDragStart={startPointerDrag}
        />
      )}
    </div>
  );
}

function HistorySidebar({
  convs, activeId, busy, dragId, overId, listElRef, onNew, onOpen, onDelete, onPin, onPointerDragStart,
}: {
  convs: ConvMeta[]; activeId: string | null; busy: boolean;
  dragId: string | null; overId: string | null;
  listElRef: React.RefObject<HTMLDivElement>;
  onNew: () => void; onOpen: (id: string) => void; onDelete: (id: string) => void;
  onPin: (id: string) => void;
  onPointerDragStart: (id: string, e: React.PointerEvent) => void;
}) {
  return (
    <div style={{
      width: 230, flexShrink: 0, display: "flex", flexDirection: "column", gap: 8,
      borderLeft: "var(--border-width) solid var(--line)", paddingLeft: 14,
    }}>
      <Button onClick={onNew} disabled={busy}>+ New chat</Button>
      <div ref={listElRef} style={{ overflowY: "auto", display: "flex", flexDirection: "column", gap: 4, marginTop: 4 }}>
        {convs.length === 0 && (
          <p style={{ ...hint, fontSize: 13, color: "var(--text-faint)" }}>No chats yet.</p>
        )}
        {convs.map((c) => (
          <HistoryItem
            key={c.id} c={c} active={c.id === activeId}
            dragging={dragId === c.id} isOver={overId === c.id && dragId !== null && dragId !== c.id}
            onOpen={() => onOpen(c.id)} onDelete={() => onDelete(c.id)} onPin={() => onPin(c.id)}
            onPointerDown={(e) => onPointerDragStart(c.id, e)}
          />
        ))}
      </div>
    </div>
  );
}

function HistoryItem({
  c, active, dragging, isOver, onOpen, onDelete, onPin, onPointerDown,
}: {
  c: ConvMeta; active: boolean; dragging: boolean; isOver: boolean;
  onOpen: () => void; onDelete: () => void; onPin: () => void;
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
        style={{ background: "none", border: "none", cursor: "pointer", padding: 0, fontSize: 12, opacity: c.pinned ? 1 : hover ? 0.5 : 0 }}
      >{c.pinned ? "★" : "☆"}</button>
      <span style={{ flex: 1, minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", color: "var(--text)" }}>
        {c.title || "Untitled"}
      </span>
      {hover && (
        <button
          onClick={(e) => { e.stopPropagation(); onDelete(); }}
          title="Delete chat"
          style={{ background: "none", border: "none", cursor: "pointer", padding: 0, fontSize: 13, color: "var(--danger)" }}
        >✕</button>
      )}
    </div>
  );
}

function Bubble({ m }: { m: Msg }) {
  const isUser = m.role === "user";
  const memory = isUser && m.role === "user" ? m.memory : undefined;
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: isUser ? "flex-end" : "flex-start" }}>
      <div style={{
        maxWidth: "82%",
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
