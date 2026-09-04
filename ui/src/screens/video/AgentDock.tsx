// AYGENT — VIDEO v0.3 agent dock. A real chat with the selected agent, scoped to
// the open project.
//
// ISOLATION: runs in its own turn-store slot (`<agentId>::video`, lib/turns.ts)
// so the Chat screen never shows or steals this conversation, and its history
// lives in Video/<project>/chat.json — not the agent's conversation list.
// RESUME: the live stream is captured by the store (not this component), so
// switching to Inspector / another tab and back re-attaches to the running turn
// with every tool card intact. A mid-turn draft is also persisted so a hard
// reload shows what happened.
// LIVE TIMELINE: after EVERY completed video_* tool the composition is reloaded
// (and the backend emits video-project-changed too), so titles/graphics land on
// the timeline one by one while the agent works.
// AUTO-COMPACT: when the model's context input passes 50 % of its window, the
// history is summarized (history_compact) before the next send.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Send, Square, Trash2, Bot, ClipboardList, Check } from "lucide-react";
import { Markdown } from "../../components/Markdown";
import { runTurn, useAgentTurn, isRunning, stopTurn, setHistory, getHistory, getAgentTurnSnapshot, turnSlotKey, type TurnItem } from "../../lib/turns";
import { useVideo, useChat, chatPush, chatReplaceLast, chatPersist, chatGetHistory, chatClear, flushSave, reload, get, toast, type ChatMsg } from "./store";
import { fmtTime } from "./model";

const QUICK: { l: string; p: string }[] = [
  { l: "Rough cut", p: "Run video_auto_cut on the A-roll: drop silences and dead air, keep the LAST take when I repeat a line. Then tell me what you removed." },
  { l: "Transcribe", p: "Transcribe the A-roll (video_transcribe) and give me a 5-bullet summary of what I say with timestamps." },
  { l: "Captions", p: "Turn on animated captions with the pop preset, 4 words per line, and pick 6 key words to highlight from the transcript." },
  { l: "Graphics plan", p: "Read the transcript and write a PLAN for on-screen graphics (title cards / callouts) where extra explanation helps: numbered list, one per line with timecode range, exact on-screen text, placement, and why. Do NOT add anything yet — wait for my approval or revision notes." },
  { l: "Grade", p: "Apply my LUT (pick the .cube in luts/) on the adjustment layer with a 35% S-curve." },
  { l: "Duck music", p: "Make sure anything on A2 ducks under my voice: music -18 dB, ducked -30 dB." },
  { l: "9:16 version", p: "Add a vertical export preset (1080x1920) and tell me which clips would need reframing (x offsets) to keep me centered." },
];
const COMPACT_AT = 0.5; // fraction of the context window

export function AgentDock({ agentName }: { agentName?: string }) {
  const s = useVideo();
  const msgs = useChat();
  const slotKey = s.agentId ? turnSlotKey(s.agentId, "video") : null;
  const turn = useAgentTurn(slotKey);
  const running = turn.status === "running";
  const [text, setText] = useState("");
  const [compacting, setCompacting] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const channelRef = useRef<string | null>(null);
  const seenToolsRef = useRef(0);

  useEffect(() => { const el = scrollRef.current; if (el) el.scrollTop = el.scrollHeight; }, [msgs, turn.liveText, turn.liveTools.length, turn.timeline?.length]);

  // Live timeline: reload after each video_* tool completes (works even if this
  // component mounted mid-turn — it just catches up on the ones it hasn't seen).
  useEffect(() => {
    if (!running) { seenToolsRef.current = 0; return; }
    const done = turn.liveTools.filter((t) => !t.running && t.name.startsWith("video_")).length;
    if (done > seenToolsRef.current) { seenToolsRef.current = done; void reload(); }
  }, [turn.liveTools, running]);

  // Mid-turn draft: keep the last assistant bubble updated with live progress so
  // a remount (view switch) or hard reload shows the tools that already ran.
  useEffect(() => {
    if (!running) return;
    const last = msgs[msgs.length - 1];
    if (!last || last.role !== "assistant") return;
    const draft: ChatMsg = { ...last, text: turn.liveText, tools: turn.liveTools.map((t) => ({ name: t.name, summary: t.summary, ok: t.ok, detail: t.detail?.slice(0, 1200) })) };
    chatReplaceLast(draft);
  }, [turn.liveTools.length, turn.liveText, running]);

  const ctxPct = turn.usage?.contextWindow ? (turn.usage.contextInput || 0) / turn.usage.contextWindow : 0;

  async function maybeCompact(agentId: string): Promise<unknown[]> {
    const hist = chatGetHistory();
    if (ctxPct < COMPACT_AT || hist.length < 4) return hist;
    setCompacting(true);
    try {
      const seed = await invoke<unknown[]>("history_compact", { agentId, history: hist });
      await chatPersist(seed);
      chatPush({ role: "assistant", text: `🗜️ Context compacted at ${Math.round(ctxPct * 100)}% — earlier turns summarized. The visible chat is unchanged.`, at: Date.now() });
      return seed;
    } catch (e) { toast(`compact failed: ${String(e)}`, "err"); return hist; }
    finally { setCompacting(false); }
  }

  async function send(prompt?: string) {
    const p = (prompt ?? text).trim();
    const { agentId, project, folder, comp, selection, playhead } = get();
    if (!p || !agentId || !project || !slotKey) return;
    if (isRunning(slotKey)) return;
    setText("");
    await flushSave();
    const history = await maybeCompact(agentId);
    const ctx = [
      `[Video editor context — project "${project}" · Video/${project}/ · scene ${comp.scene.width}x${comp.scene.height}@${comp.scene.fps} · ${comp.clips.length} clips · playhead ${fmtTime(playhead, comp.scene.fps)} (${playhead.toFixed(3)}s)` +
      (selection.length ? ` · selected clip ids: ${selection.join(", ")}` : "") + `]`,
      `Use the video_* tools (start with video_project if you need the current state). Keep the reply short: what changed, clip ids, times. Graphics/titles: PLAN first, build only after approval, one video_edit per graphic.`,
    ].join("\n");
    chatPush({ role: "user", text: p, at: Date.now() });
    chatPush({ role: "assistant", text: "", at: Date.now() });
    void chatPersist(history);
    setHistory(slotKey, history);
    const channel = `video-${project}-${Date.now()}`; channelRef.current = channel;
    let out: unknown[] = history;
    try {
      let model: string | null = null, provider: string | null = null;
      if (folder) { try { const sel = await invoke<{ provider: string; model: string }>("get_selection", { folder }); model = sel.model || null; provider = sel.provider || null; } catch { /* default */ } }
      out = await runTurn({ agentId, channel, prompt: `${ctx}\n\n${p}`, model, provider, folder, sessionId: `video-${agentId}-${project}`, attachments: [], slot: slotKey });
    } catch (e) {
      chatReplaceLast({ role: "assistant", text: `✗ ${String(e)}`, at: Date.now() });
    } finally {
      channelRef.current = null;
      const done = getAgentTurnSnapshot(slotKey);
      const final: ChatMsg = { role: "assistant", text: done.liveText || (done.error ? `✗ ${done.error}` : "(no reply)"), at: Date.now(), tools: done.liveTools.map((t) => ({ name: t.name, summary: t.summary, ok: t.ok, detail: t.detail?.slice(0, 1200) })) };
      chatReplaceLast(final);
      void chatPersist(out.length ? out : getHistory(slotKey));
      void reload();
    }
  }

  const disabled = !s.agentId || !s.project;
  const lastAssistant = [...msgs].reverse().find((m) => m.role === "assistant");
  const awaitingApproval = !running && !!lastAssistant && /approve|revision notes/i.test(lastAssistant.text) && /^\s*(\d+[.)]|[-•*])\s/m.test(lastAssistant.text);

  return (
    <div className="ve-dock-body">
      <div className="ve-chat aygent-scroll" ref={scrollRef}>
        {msgs.length === 0 && !running && (
          <div className="ve-empty-chat">
            <Bot size={28} style={{ opacity: .5 }} />
            <p style={{ marginTop: 8 }}><b>{agentName ?? "Your agent"}</b> edits this project with you.<br />Everything it does lands in the timeline as it happens — same file you're editing.</p>
            <p>Try <b>Rough cut</b> after importing your A-roll.</p>
          </div>
        )}
        {msgs.map((m, i) => {
          const isLast = i === msgs.length - 1;
          if (m.role === "user") return <div key={i} className="ve-msg user">{m.text}</div>;
          if (isLast && running) return <LiveBubble key={i} timeline={turn.timeline ?? []} text={turn.liveText} info={turn.info} />;
          return (
            <div key={i} className="ve-msg assistant">
              {m.text ? <Markdown text={m.text} /> : <span className="ve-faint">(no reply)</span>}
              {m.tools && m.tools.length > 0 && <div className="tools">{m.tools.map((t, j) => <ToolCard key={j} name={t.name} summary={t.summary} ok={t.ok} detail={t.detail} />)}</div>}
            </div>
          );
        })}
      </div>
      {awaitingApproval ? (
        <div className="ve-quick approve">
          <span className="ve-faint" style={{ fontSize: 11.5, display: "inline-flex", alignItems: "center", gap: 5 }}><ClipboardList size={13} /> Plan ready</span>
          <button className="go" disabled={disabled} onClick={() => void send("Approved — build it. One video_edit per graphic, in timeline order.")}><Check size={12} /> Approve &amp; build</button>
          <span className="ve-faint" style={{ fontSize: 11.5 }}>or type revision notes below</span>
        </div>
      ) : (
        <div className="ve-quick">{QUICK.map((q) => <button key={q.l} disabled={disabled || running} onClick={() => void send(q.p)}>{q.l}</button>)}</div>
      )}
      <div className="ve-compose">
        <textarea value={text} placeholder={disabled ? "Open a project to chat with your agent" : awaitingApproval ? "Revision notes… (⏎ to send)" : "Tell the agent what to do to this edit… (⏎ to send, ⇧⏎ newline)"} disabled={disabled}
          onChange={(e) => setText(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); } }} />
        <div className="row">
          <button className="ve-icon-btn" title="Clear conversation" disabled={!msgs.length || running} onClick={() => { if (confirm("Clear this project's conversation?")) chatClear(); }}><Trash2 size={14} /></button>
          <span className="ve-faint" style={{ fontSize: 11 }}>
            {agentName ? `as ${agentName}` : ""}
            {turn.usage?.contextWindow ? <span title="Context used · auto-compacts at 50%" style={{ marginLeft: 8, color: ctxPct >= COMPACT_AT ? "var(--warn)" : undefined }}>· ctx {Math.round(ctxPct * 100)}%</span> : null}
            {compacting ? " · compacting…" : turn.info && running ? ` · ${turn.info}` : ""}
          </span>
          <span className="spacer" />
          {running ? <button className="ve-btn sm" onClick={() => channelRef.current && void stopTurn(channelRef.current)}><Square size={12} /> Stop</button>
            : <button className="ve-btn sm primary" disabled={disabled || !text.trim() || compacting} onClick={() => void send()}><Send size={12} /> Send</button>}
        </div>
      </div>
    </div>
  );
}

function LiveBubble({ timeline, text, info }: { timeline: TurnItem[]; text: string; info?: string }) {
  return (
    <div className="ve-msg assistant">
      {timeline.length === 0 && <div className="ve-thinking"><i /><i /><i /></div>}
      {timeline.map((it, i) => it.kind === "text" ? <Markdown key={i} text={it.text} /> : <div key={i} className="tools"><ToolCard name={it.tool.name} summary={it.tool.summary} ok={it.tool.ok} detail={it.tool.detail} running={it.tool.running} /></div>)}
      {!text && timeline.length > 0 && timeline[timeline.length - 1].kind === "tool" && <div className="ve-thinking" style={{ marginTop: 6 }}><i /><i /><i /></div>}
      {info && <div className="ve-faint" style={{ fontSize: 11, marginTop: 4 }}>{info}</div>}
    </div>
  );
}

function ToolCard({ name, summary, ok, detail, running }: { name: string; summary?: string; ok?: boolean; detail?: string; running?: boolean }) {
  const [open, setOpen] = useState(false);
  const label = summary?.replace(/^⚙ /, "") ?? name;
  return (
    <div className="ve-toolcard" style={{ flexDirection: "column", alignItems: "stretch", cursor: detail ? "pointer" : "default" }} onClick={() => detail && setOpen(!open)}>
      <div style={{ display: "flex", alignItems: "center", gap: 7 }}><span className={`dot ${running ? "run" : ok === false ? "err" : ok ? "ok" : ""}`} /><span className="s" title={label}>{label}</span>{detail && <span className="ve-faint">{open ? "▾" : "▸"}</span>}</div>
      {open && detail && <pre>{detail}</pre>}
    </div>
  );
}
