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
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Send, Square, Trash2, Bot, ClipboardList, Check } from "lucide-react";
import { Markdown } from "../../components/Markdown";
import { runTurn, useAgentTurn, isRunning, stopTurn, setHistory, getHistory, getAgentTurnSnapshot, turnSlotKey, type TurnItem } from "../../lib/turns";
import { useVideo, useChat, chatPush, chatReplaceLast, chatPersist, chatGetHistory, chatClear, flushSave, reload, get, toast, type ChatMsg } from "./store";
import { fmtTime } from "./model";

const QUICK: { l: string; p: string }[] = [
  { l: "Rough cut", p: "Run video_auto_cut on the A-roll: drop silences and dead air, keep the LAST take when I repeat a line. Then tell me what you removed." },
  { l: "Transcribe", p: "Transcribe the A-roll (video_transcribe) and give me a 5-bullet summary of what I say with timestamps." },
  { l: "Captions", p: "Transcribe the A-roll if needed, then build the Hyperframes caption overlay (video_build_captions) with 6 key words to highlight. Style comes from my Graphics panel." },
  { l: "Graphics plan", p: "Read the transcript and my Graphics panel style, then write a PLAN for Hyperframes overlay graphics (title cards / callouts) where extra explanation helps: numbered list, one per line with timecode range, exact on-screen text, placement, and why. Do NOT build anything yet — wait for my approval or revision notes, then build with video_render_overlay." },
  { l: "Grade", p: "Apply my LUT (pick the .cube in luts/) on the adjustment layer with a 35% S-curve." },
  { l: "Duck music", p: "Make sure anything on A2 ducks under my voice: music -18 dB, ducked -30 dB." },
  { l: "9:16 version", p: "Add a vertical export preset (1080x1920) and tell me which clips would need reframing (x offsets) to keep me centered." },
];
const COMPACT_AT = 0.5; // fraction of the context window

/** One-shot instruction behind Inspector's "Apply Feedback" button. */
export const APPLY_FEEDBACK_PROMPT =
  "Apply my open R1 review notes now: read each note with its timecode, make the requested change to the edit with the video_* tools, and mark every note you actioned resolved via video_edit update_clip {hidden:true}. Reply with one line per note: what you changed, clip ids, times.";

// Live channel for the in-flight video turn (module-level so Inspector's
// Apply Feedback button and the dock share one turn lifecycle).
let videoChannel: string | null = null;

/** Send a prompt to the video agent from anywhere (Inspector, Timeline…).
 *  Same context/history lifecycle as the dock composer. Returns false when
 *  there is nothing to send or a turn is already running. */
export async function sendVideoPrompt(prompt: string): Promise<boolean> {
  const p = prompt.trim();
  const { agentId, project, folder, comp, selection, playhead } = get();
  const slotKey = agentId ? turnSlotKey(agentId, "video") : null;
  if (!p || !agentId || !project || !slotKey) return false;
  if (isRunning(slotKey)) return false;
  await flushSave();
  const snap = getAgentTurnSnapshot(slotKey);
  const pct = snap.usage?.contextWindow ? (snap.usage.contextInput || 0) / snap.usage.contextWindow : 0;
  let history = chatGetHistory();
  if (pct >= COMPACT_AT && history.length >= 4) {
    try {
      history = await invoke<unknown[]>("history_compact", { agentId, history });
      await chatPersist(history);
      chatPush({ role: "assistant", text: `\u{1F5DC}\uFE0F Context compacted at ${Math.round(pct * 100)}% — earlier turns summarized. The visible chat is unchanged.`, at: Date.now() });
    } catch (e) { toast(`compact failed: ${String(e)}`, "err"); }
  }
  const notes = comp.clips.filter((c) => c.type === "review" && !c.hidden && c.text.content.trim());
  const notesTxt = notes.length
    ? `\nOPEN REVIEW NOTES from the user (R1 lane — read each, act on it, mark resolved via video_edit update_clip {hidden:true}):\n` +
      notes.map((c) => `- [${fmtTime(c.start, comp.scene.fps)} \u2192 ${fmtTime(c.end, comp.scene.fps)}] ${c.text.content.trim()}`).join("\n")
    : "";
  const ctx = [
    `[Video editor context — project "${project}" · Video/${project}/ · scene ${comp.scene.width}x${comp.scene.height}@${comp.scene.fps} · ${comp.clips.length} clips · playhead ${fmtTime(playhead, comp.scene.fps)} (${playhead.toFixed(3)}s)` +
    (selection.length ? ` · selected clip ids: ${selection.join(", ")}` : "") + `]${notesTxt}`,
    `Use the video_* tools (start with video_project if you need the current state). Keep the reply short: what changed, clip ids, times. Captions/graphics are Hyperframes transparent overlays (video_build_captions / video_render_overlay), styled by the Graphics panel — never drawtext/ASS. Overlays: PLAN first, build only after approval, one tool call per overlay in timeline order.`,
  ].join("\n");
  chatPush({ role: "user", text: p, at: Date.now() });
  chatPush({ role: "assistant", text: "", at: Date.now() });
  void chatPersist(history);
  setHistory(slotKey, history);
  const channel = `video-${project}-${Date.now()}`; videoChannel = channel;
  let out: unknown[] = history;
  try {
    let model: string | null = null, provider: string | null = null;
    if (folder) { try { const sel = await invoke<{ provider: string; model: string }>("get_selection", { folder }); model = sel.model || null; provider = sel.provider || null; } catch { /* default */ } }
    out = await runTurn({ agentId, channel, prompt: `${ctx}\n\n${p}`, model, provider, folder, sessionId: `video-${agentId}-${project}`, attachments: [], slot: slotKey });
  } catch (e) {
    chatReplaceLast({ role: "assistant", text: `\u2717 ${String(e)}`, at: Date.now() });
  } finally {
    videoChannel = null;
    const done = getAgentTurnSnapshot(slotKey);
    const final: ChatMsg = { role: "assistant", text: done.liveText || (done.error ? `\u2717 ${done.error}` : "(no reply)"), at: Date.now(), tools: done.liveTools.map((t) => ({ name: t.name, summary: t.summary, ok: t.ok, detail: t.detail?.slice(0, 1200) })) };
    chatReplaceLast(final);
    void chatPersist(out.length ? out : getHistory(slotKey));
    void reload();
  }
  return true;
}
export function stopVideoTurn() { if (videoChannel) void stopTurn(videoChannel); }

export function AgentDock({ agentName }: { agentName?: string }) {
  const s = useVideo();
  const msgs = useChat();
  const slotKey = s.agentId ? turnSlotKey(s.agentId, "video") : null;
  const turn = useAgentTurn(slotKey);
  const running = turn.status === "running";
  const [text, setText] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const taH = useRef(68);
  // Auto-grow the composer (68–200px) and shift the chat up by exactly the delta
  // so new lines never cover the latest message (same approach as Chat.tsx).
  useLayoutEffect(() => {
    const ta = taRef.current; if (!ta) return;
    ta.style.height = "auto";
    const next = Math.max(68, Math.min(ta.scrollHeight, 200));
    ta.style.height = next + "px";
    const prev = taH.current; if (next === prev) return; taH.current = next;
    const el = scrollRef.current; if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 160 + Math.max(0, next - prev)) el.scrollTop += next - prev;
  }, [text]);
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

  async function send(prompt?: string) {
    const ok = await sendVideoPrompt(prompt ?? text);
    if (ok) setText("");
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
          <button className="go" disabled={disabled} onClick={() => void send("Approved — build it. One overlay tool call per graphic, in timeline order.")}><Check size={12} /> Approve &amp; build</button>
          <span className="ve-faint" style={{ fontSize: 11.5 }}>or type revision notes below</span>
        </div>
      ) : (
        <div className="ve-quick">{QUICK.map((q) => <button key={q.l} disabled={disabled || running} onClick={() => void send(q.p)}>{q.l}</button>)}</div>
      )}
      <div className="ve-compose">
        <textarea ref={taRef} rows={1} value={text} placeholder={disabled ? "Open a project to chat with your agent" : awaitingApproval ? "Revision notes… (⏎ to send)" : "Tell the agent what to do to this edit… (⏎ to send, ⇧⏎ newline)"} disabled={disabled}
          onChange={(e) => setText(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); } }} />
        <div className="row">
          <button className="ve-icon-btn" title="Clear conversation" disabled={!msgs.length || running} onClick={() => { if (confirm("Clear this project's conversation?")) chatClear(); }}><Trash2 size={14} /></button>
          <span className="ve-faint" style={{ fontSize: 11 }}>
            {agentName ? `as ${agentName}` : ""}
            {turn.usage?.contextWindow ? <span title="Context used · auto-compacts at 50%" style={{ marginLeft: 8, color: ctxPct >= COMPACT_AT ? "var(--warn)" : undefined }}>· ctx {Math.round(ctxPct * 100)}%</span> : null}
            {turn.info && running ? ` · ${turn.info}` : ""}
          </span>
          <span className="spacer" />
          {running ? <button className="ve-btn sm" onClick={() => stopVideoTurn()}><Square size={12} /> Stop</button>
            : <button className="ve-btn sm primary" disabled={disabled || !text.trim()} onClick={() => void send()}><Send size={12} /> Send</button>}
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
