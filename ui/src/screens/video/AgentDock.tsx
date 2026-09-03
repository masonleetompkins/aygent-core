// AYGENT — VIDEO v0.3 agent dock. A real chat with the selected agent, scoped to
// the open project: streams tokens + tool cards through the SAME per-agent turn
// store Chat uses (lib/turns.ts), persists to Video/<project>/chat.json, and
// reloads the composition after every turn (and live, as each video_* tool
// finishes) so the timeline moves while the agent works.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Send, Square, Trash2, Bot } from "lucide-react";
import { Markdown } from "../../components/Markdown";
import { runTurn, useAgentTurn, isRunning, stopTurn, setHistory, getAgentTurnSnapshot, type TurnItem } from "../../lib/turns";
import { useVideo, useChat, chatPush, chatReplaceLast, chatPersist, chatGetHistory, chatClear, flushSave, reload, get, type ChatMsg } from "./store";
import { fmtTime } from "./model";

const QUICK: { l: string; p: string }[] = [
  { l: "Rough cut", p: "Run video_auto_cut on the A-roll: drop silences and dead air, keep the LAST take when I repeat a line. Then tell me what you removed." },
  { l: "Transcribe", p: "Transcribe the A-roll (video_transcribe) and give me a 5-bullet summary of what I say with timestamps." },
  { l: "Captions", p: "Turn on animated captions with the pop preset, 4 words per line, and pick 6 key words to highlight from the transcript." },
  { l: "Graphics ideas", p: "Read the transcript and propose 3 on-screen graphics (title cards or callouts) where extra explanation helps, with timestamps. Don't add them yet — wait for my approval." },
  { l: "Grade", p: "Apply my LUT (pick the .cube in luts/) on the adjustment layer with a 35% S-curve." },
  { l: "Duck music", p: "Make sure anything on A2 ducks under my voice: music -18 dB, ducked -30 dB." },
  { l: "9:16 version", p: "Add a vertical export preset (1080x1920) and tell me which clips would need reframing (x offsets) to keep me centered." },
];

export function AgentDock({ agentName }: { agentName?: string }) {
  const s = useVideo();
  const msgs = useChat();
  const turn = useAgentTurn(s.agentId);
  const [text, setText] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const runningHere = useRef(false);
  const running = turn.status === "running" && runningHere.current;
  const channelRef = useRef<string | null>(null);
  const seenToolsRef = useRef(0);

  useEffect(() => { const el = scrollRef.current; if (el) el.scrollTop = el.scrollHeight; }, [msgs, turn.liveText, turn.liveTools.length]);

  // live reload: when a video_* tool completes mid-turn, refresh the timeline
  useEffect(() => {
    if (!running) return;
    const done = turn.liveTools.filter((t) => !t.running && t.name.startsWith("video_")).length;
    if (done > seenToolsRef.current) { seenToolsRef.current = done; void reload(); }
  }, [turn.liveTools, running]);

  async function send(prompt?: string) {
    const p = (prompt ?? text).trim();
    const { agentId, project, folder, comp, selection, playhead } = get();
    if (!p || !agentId || !project) return;
    if (isRunning(agentId)) return;
    setText("");
    await flushSave();
    const ctx = [
      `[Video editor context — project "${project}" · Video/${project}/ · scene ${comp.scene.width}x${comp.scene.height}@${comp.scene.fps} · ${comp.clips.length} clips · playhead ${fmtTime(playhead, comp.scene.fps)} (${playhead.toFixed(3)}s)` +
      (selection.length ? ` · selected clip ids: ${selection.join(", ")}` : "") + `]`,
      `Use the video_* tools (start with video_project if you need the current state). Keep the reply short: what changed, clip ids, times.`,
    ].join("\n");
    chatPush({ role: "user", text: p, at: Date.now() });
    chatPush({ role: "assistant", text: "", at: Date.now() });
    runningHere.current = true; seenToolsRef.current = 0;
    setHistory(agentId, chatGetHistory());
    const channel = `video-${project}-${Date.now()}`; channelRef.current = channel;
    let history: unknown[] = chatGetHistory();
    try {
      let model: string | null = null, provider: string | null = null;
      if (folder) { try { const sel = await invoke<{ provider: string; model: string }>("get_selection", { folder }); model = sel.model || null; provider = sel.provider || null; } catch { /* default */ } }
      history = await runTurn({ agentId, channel, prompt: `${ctx}\n\n${p}`, model, provider, folder, sessionId: `video-${agentId}-${project}`, attachments: [] });
    } catch (e) {
      chatReplaceLast({ role: "assistant", text: `✗ ${String(e)}`, at: Date.now() });
    } finally {
      runningHere.current = false; channelRef.current = null;
      const done = getAgentTurnSnapshot(agentId);
      const final: ChatMsg = { role: "assistant", text: done.liveText || (done.error ? `✗ ${done.error}` : "(no reply)"), at: Date.now(), tools: done.liveTools.map((t) => ({ name: t.name, summary: t.summary, ok: t.ok, detail: t.detail?.slice(0, 1200) })) };
      chatReplaceLast(final);
      void chatPersist(history);
      void reload();
    }
  }

  const disabled = !s.agentId || !s.project;
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
      <div className="ve-quick">{QUICK.map((q) => <button key={q.l} disabled={disabled || running} onClick={() => void send(q.p)}>{q.l}</button>)}</div>
      <div className="ve-compose">
        <textarea value={text} placeholder={disabled ? "Open a project to chat with your agent" : "Tell the agent what to do to this edit… (⏎ to send, ⇧⏎ newline)"} disabled={disabled}
          onChange={(e) => setText(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); } }} />
        <div className="row">
          <button className="ve-icon-btn" title="Clear conversation" disabled={!msgs.length || running} onClick={() => { if (confirm("Clear this project's conversation?")) chatClear(); }}><Trash2 size={14} /></button>
          <span className="ve-faint" style={{ fontSize: 11 }}>{agentName ? `as ${agentName}` : ""}{turn.info && running ? ` · ${turn.info}` : ""}</span>
          <span className="spacer" />
          {running ? <button className="ve-btn sm" onClick={() => channelRef.current && void stopTurn(channelRef.current)}><Square size={12} /> Stop</button>
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
