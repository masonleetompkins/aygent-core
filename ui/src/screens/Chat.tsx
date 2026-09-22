// Chat — the primary agent surface. Streaming-first: tokens render live, tool
// calls appear as inline cards as they fire, multi-turn history persists.
// Consumes normalized StreamEvents from the Rust streaming agent loop over a
// Tauri event channel. Falls back to a thinking animation if no text streams.
import { useEffect, useMemo, useRef, useState, useLayoutEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/ui";
import { Icon, type IconName } from "../components/Icon";
import { Markdown } from "../components/Markdown";
import { useSparkBlobUrl, useSparkThemeSync, isSparkStateMsg } from "../lib/sparkChrome";
import { runTurn, isRunning, setHistory, getAgentTurnSnapshot, useAgentTurn, getInbound, useConvVersion, stopTurn, turnSlotKey } from "../lib/turns";
import { notifyThread, kindFor } from "../lib/notify";
import type { TurnItem, TurnUsage } from "../lib/turns";
import type { AgentProfile } from "../components/AgentSwitcher";
import { DiffView, DiffCounts, countDiff, isDiffable } from "../components/DiffView";

type ToolLine = { name: string; path: string; ok?: boolean; detail?: string; summary?: string; body?: string; running?: boolean; spark?: { slug: string; title: string; html: string }; before?: { exists: boolean; content: string; truncated: boolean; binary: boolean } | null };
type Msg =
  // `at` = epoch ms. For a USER message it's when they hit send; for an
  // ASSISTANT message it's when the turn COMPLETED (set at finalize, not at
  // first token), which is what the timestamp in the margin claims to mean.
  | { role: "user"; text: string; memory?: string; at?: number }
  | { role: "assistant"; text: string; tools: ToolLine[]; streaming?: boolean; at?: number; timeline?: TurnItem[]; usage?: TurnUsage };

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

/** Human label for the "add a key" hint — names the agent's real provider. */
function providerLabel(p: string | null | undefined): string {
  switch (p) {
    case "openai": return "an OpenAI";
    case "openrouter": return "an OpenRouter";
    case "meta": return "a Muse (Meta)";
    case "anthropic": case "": case null: case undefined: return "an Anthropic";
    default: return `a ${p}`;
  }
}

function ChatPane({ agent, folder, keySet, agentId, multi, closable, onClose }: {
  agent: AgentProfile | null;
  folder: string | null; keySet: boolean; agentId: string | null;
  multi: boolean; closable: boolean; onClose: () => void;
}) {
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyCollapsed, setHistoryCollapsed] = useState(false);
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

  // COMMAND PRO threads (B1-B6): N parallel sessions per agent. Tabs are conv
  // ids; each thread streams in its own store slot (turnSlotKey) on its own
  // backend lane (sessionId = convId), so there is no cap. Disk is the source
  // of truth for finalized msgs; the store holds only in-flight turns.
  const [tabs, setTabs] = useState<string[]>([]);
  const [, forceTabs] = useState(0);
  // Per-thread model override (B1): {provider, model} or null = agent default.
  // Persisted in localStorage per conv so it survives reloads without a DB migration.
  const [threadModels, setThreadModels] = useState<Record<string, { provider: string; model: string }>>({});
  // Herdr-style toasts (B3): done / needs-input for background threads.
  const [toasts, setToasts] = useState<Array<{ id: number; agentName: string; threadTitle: string; kind: "done" | "needs-input"; body: string }>>([]);
  const prevRunningRef = useRef<Record<string, boolean>>({});
  function threadTitle(id: string | null): string {
    if (!id) return "New thread";
    return convs.find((c) => c.id === id)?.title || "New thread";
  }
  function openThread(id: string) {
    setTabs((t) => (t.includes(id) ? t : [...t, id]));
    void openConv(id);
  }
  function closeTab(id: string) {
    setTabs((t) => {
      if (!t.includes(id) || t.length <= 1) return t;
      const next = t.filter((x) => x !== id);
      if (id === convIdRef.current) {
        const idx = t.indexOf(id);
        const fallback = next[Math.min(idx, next.length - 1)];
        if (fallback) setTimeout(() => void openConv(fallback), 0);
      }
      return next;
    });
  }
  function threadModelKey(id: string | null) { return `aygent.threadModel.${agentId ?? "?"}.${id ?? "?"}`; }
  function getThreadModel(id: string | null): { provider: string; model: string } | null {
    if (!id) return null;
    const m = threadModels[id];
    if (m) return m;
    try {
      const raw = localStorage.getItem(threadModelKey(id));
      if (raw) return JSON.parse(raw);
    } catch { /* ignore */ }
    return null;
  }
  function setThreadModel(id: string, v: { provider: string; model: string } | null) {
    setThreadModels((prev) => {
      const next = { ...prev };
      if (v) next[id] = v; else delete next[id];
      return next;
    });
    try {
      if (v) localStorage.setItem(threadModelKey(id), JSON.stringify(v));
      else localStorage.removeItem(threadModelKey(id));
    } catch { /* ignore */ }
  }
  const scrollRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const isNearBottom = (threshold = 80) => {
    const el = scrollRef.current; if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
  };
  const pinToBottom = (smooth = false) => {
    // Only double-rAF when actually pinning; use single scrollTop — scrollIntoView on an
    // inner anchor can cause layout thrash when called per-keystroke while typing.
    const el = scrollRef.current;
    if (!el) return;
    requestAnimationFrame(() => {
      el.scrollTop = el.scrollHeight;
      // Anchor as fallback for flex-column bottom gap; no smooth during typing.
      if (smooth) bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
    });
  };

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
  // DRAG-AND-DROP (Mason 09-09): drop files anywhere on the chat column to
  // attach them — same path as the + button (onFilesPicked), so behavior is
  // identical. dragDepth keeps the overlay from flickering across children.
  const [dragOver, setDragOver] = useState(false);
  const dragDepth = useRef(0);
  function dropHasFiles(e: React.DragEvent) {
    try { return Array.from(e.dataTransfer.types || []).includes("Files"); } catch { return false; }
  }
  function onDropZoneDragEnter(e: React.DragEvent) {
    if (blocked || !agentId || !dropHasFiles(e)) return;
    e.preventDefault();
    dragDepth.current += 1;
    setDragOver(true);
  }
  function onDropZoneDragOver(e: React.DragEvent) {
    if (blocked || !agentId || !dropHasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  }
  function onDropZoneDragLeave() {
    dragDepth.current = Math.max(0, dragDepth.current - 1);
    if (dragDepth.current === 0) setDragOver(false);
  }
  function onDropZoneDrop(e: React.DragEvent) {
    dragDepth.current = 0;
    setDragOver(false);
    if (blocked || !agentId) return;
    const files = e.dataTransfer.files;
    if (!files || files.length === 0) return;
    e.preventDefault();
    e.stopPropagation();
    void onFilesPicked(files);
  }
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
  // PUSH-TO-TALK (Mason 08-04): hold ` (or ~) to record, release to transcribe;
  // TAP it to send. Both the button and the key go through startRec/stopRec so
  // there is exactly one recording implementation to keep correct.
  // `ptt` tracks a held key so keydown auto-repeat doesn't start N recorders.
  const pttRef = useRef<{ down: boolean; start: number; recording: boolean }>({ down: false, start: 0, recording: false });
  const HOLD_MS = 220; // under this = a tap (send), over = a hold (record)

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

  // ---- Push-to-talk on ` / ~ -------------------------------------------
  // Hold to record (release -> Whisper -> input box). Tap to send.
  // Deliberately NOT active while the textarea has focus: a backtick typed
  // into a message (```code```) must stay a backtick. Use it from anywhere
  // else in the pane.
  useEffect(() => {
    if (blocked) return;
    const isBacktick = (e: KeyboardEvent) => e.code === "Backquote" || e.key === "`" || e.key === "~";
    const typing = () => document.activeElement === taRef.current;

    async function onDown(e: KeyboardEvent) {
      if (!isBacktick(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (typing()) return;              // let the user type a real backtick
      if (pttRef.current.down) return;   // ignore auto-repeat
      e.preventDefault();
      pttRef.current = { down: true, start: Date.now(), recording: false };
      // Only START recording once the key has been held past the tap window,
      // otherwise a quick tap would spin the mic up and immediately tear it
      // down (and on macOS that flashes the mic indicator for no reason).
      window.setTimeout(() => {
        if (pttRef.current.down && !pttRef.current.recording && rec === "idle") {
          pttRef.current.recording = true;
          void toggleMic(); // starts recording
        }
      }, HOLD_MS);
    }

    function onUp(e: KeyboardEvent) {
      if (!isBacktick(e) || !pttRef.current.down) return;
      if (typing()) { pttRef.current.down = false; return; }
      e.preventDefault();
      const held = Date.now() - pttRef.current.start;
      const wasRecording = pttRef.current.recording;
      pttRef.current = { down: false, start: 0, recording: false };

      if (wasRecording) {
        // Release ends the recording; onstop transcribes into the input box.
        recRef.current?.stop();
      } else if (held < HOLD_MS) {
        // A TAP: send whatever is in the box.
        if (!running && input.trim()) void send();
      }
    }

    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  });

  // ---- Task #8: #tool tagging (mirrors @mentions) ----
  const [toolNames, setToolNames] = useState<string[]>([]);
  useEffect(() => {
    const base = ["read_file", "write_file", "list_files", "rename_file", "delete_file", "task_continue"];
    if (!agentId) { setToolNames(base); return; }
    invoke<Array<{ name: string; enabled: boolean }>>("tools_list", { agentId, folder })
      .then((ts) => setToolNames([...base, ...ts.filter((t) => t.enabled && t.name).map((t) => t.name)]))
      .catch(() => setToolNames(base));
  }, [folder, agentId]);
  const [toolTag, setToolTag] = useState<{ query: string; matches: string[]; sel: number; start: number } | null>(null);

  // #3 auto-grow: single-line by default, grows with content up to a sane cap.
  // Reset to auto first so it can SHRINK too; when empty, scrollHeight collapses
  // to one line. Cap ~200px (~8 lines), not 50vh (that let an empty box balloon
  // to half the window inside the flex column). Mason 07-28.
  const prevTaHeightRef = useRef<number>(38);
  useLayoutEffect(() => {
    const ta = taRef.current; if (!ta) return;
    const prev = prevTaHeightRef.current;
    // Cheap early-out: on a same-line keystroke scrollHeight can't have changed
    // unless the line wrapped, and reading it is a layout read — fine, but we only
    // WRITE (height + scrollTop) when the value actually differs.
    ta.style.height = "auto";
    const next = Math.min(ta.scrollHeight, 200);
    ta.style.height = next + "px";
    if (next === prev) return;
    prevTaHeightRef.current = next;
    // GROWTH COMPENSATION (Mason 09-04): the composer sits below the message
    // list, so every extra line steals that many px from the list's viewport
    // and covered the newest message. Shift the list's scrollTop by EXACTLY the
    // delta, synchronously (useLayoutEffect — before paint), so the messages
    // appear to move up one line per line typed. No rAF, no full re-pin, no
    // "jump to bottom": nothing visible moves except the new line, so there's
    // nothing to stutter. Skipped when the user has scrolled up to read history.
    const el = scrollRef.current; if (!el) return;
    const wasNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 160 + Math.max(0, next - prev);
    if (wasNearBottom) el.scrollTop = el.scrollTop + (next - prev);
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
  // Reactive mirror of "this pane runs a local model" — gates the <think>
  // Thoughts-bar parsing so cloud chats that merely MENTION <think> in prose
  // or code are never chopped up (Mason 08-19). Refs don't re-render; this does.
  const [isLocal, setIsLocal] = useState(false);

  useEffect(() => { pinToBottom(true); }, [msgs]);

  function setConv(id: string | null) { convIdRef.current = id; setConvId(id); }

  // Set msgs in BOTH the ref (source of truth for send/persist) and state (render).
  // Keeping these in lockstep is critical: send() builds from msgsRef, so if a
  // switch/new only cleared state, the next send would append to the OLD thread
  // (the "previous chat bled into the new one" bug).
  function setMessages(next: Msg[]) { msgsRef.current = next; setMsgs(next); }

  // On folder change: load the list and open the most recent one (or a fresh one).
  useEffect(() => {
    if (!folder) { setConvs([]); setConv(null); setTabs([]); setMessages([]); historyRef.current = []; return; }
    invoke<{ provider: string; model: string }>("get_selection", { folder })
      .then((s) => { providerRef.current = s.provider; modelRef.current = s.model; setIsLocal(s.provider === "local" || s.provider === "mlx"); }).catch(() => {});
    (async () => {
      try {
        const list = await invoke<ConvMeta[]>("conv_list", { folder });
        setConvs(list);
        if (list.length > 0) { setTabs([list[0].id]); await openConv(list[0].id); }
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
    setTabs((t) => (t.includes(id) ? t : [...t, id]));
    // CONTEXT MODE (2026-08-03): "isolated" (default) starts a truly fresh
    // session — empty provider history, zero cross-chat token cost.
    // "continuous" carries the CURRENT chat's provider history into the new
    // one, so the agent picks up mid-thought (the user opted into the token
    // cost in the Agents pane). The visible transcript always starts clean
    // either way — only the model-facing context differs.
    const carry = agent?.context_mode === "continuous" ? historyRef.current : [];
    setConv(id); setMessages([]); historyRef.current = carry;
  }

  async function openConv(id: string) {
    setTabs((t) => (t.includes(id) ? t : [...t, id]));
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
    setTabs((t) => t.filter((x) => x !== id));
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

  // Rename a chat by id via the dedicated conv_rename command (a targeted
  // UPDATE of title only). The old load+conv_save round-trip was the revert
  // bug (Mason 08-04): every turn-save re-derived the title from the first
  // user message and clobbered the rename; the backend now preserves stored
  // titles on save, so renames MUST go through conv_rename. Used by both the
  // sidebar pencil and the inline header field.
  async function saveConvTitle(id: string, next: string) {
    if (!folder) return;
    const title = next.trim();
    if (!title) return;
    try {
      await invoke("conv_rename", { id, title });
      await refreshList();
    } catch { /* ignore */ }
  }
  // Which sidebar row is being renamed inline. window.prompt() looks like the
  // obvious tool here and is what this used to call — but it is a NO-OP in
  // Tauri's macOS WKWebView (it returns null immediately), so the pencil in the
  // history sidebar silently did nothing while the header field worked fine.
  // Same bug class as the ghost folder: two paths to one action, only one of
  // them exercised. Inline editing works in every webview.
  const [renamingId, setRenamingId] = useState<string | null>(null);
  function renameConv(id: string) { setRenamingId(id); }
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
    const gateSlot = agentId ? turnSlotKey(agentId, convIdRef.current || "fresh") : "";
    if (!prompt || !agentId || (gateSlot && isRunning(gateSlot))) return;
    setInput("");

    const myAgent = agentId;
    const myConvId = convIdRef.current || `agent://${Date.now()}`;
    const channel = myConvId; // per-conversation channel = the stable session id

    // Optimistic user bubble ONLY. Do NOT append a static assistant placeholder:
    // live tokens land in the store (turns.ts) and render via the trailing
    // streaming bubble below — a placeholder here matched that bubble's
    // suppression condition and blocked live streaming entirely (bug: responses
    // appeared all-at-once). Cleo 2026-07-31.
    const withUser: Msg[] = [...msgsRef.current, { role: "user", text: prompt, at: Date.now() }];
    msgsRef.current = withUser;
    setMsgs(withUser);
    void persist(withUser); // thread appears in the sidebar immediately

    // Seed the store's per-agent history from this conversation so a follow-up
    // continues the thread.
    const mySlot = turnSlotKey(myAgent, myConvId);
    setHistory(mySlot, historyRef.current);

    // Per-thread model override (B1) wins; else the agent default.
    const override = getThreadModel(myConvId);
    if (override) {
      providerRef.current = override.provider; modelRef.current = override.model;
      setIsLocal(override.provider === "local" || override.provider === "mlx");
    } else if (folder) {
      try {
        const s = await invoke<{ provider: string; model: string }>("get_selection", { folder });
        providerRef.current = s.provider; modelRef.current = s.model; setIsLocal(s.provider === "local" || s.provider === "mlx");
      } catch { /* keep last */ }
    }

    runningChannelRef.current = channel;
    try {
      // runTurn OWNS the listener + accumulator in the App-level store, so the
      // stream keeps landing even if you navigate away. It resolves with the
      // final history. The live text/tools render via the subscribed `turn`
      // slice below (see the streaming bubble), so no local mirror needed.
      const updated = await runTurn({
        agentId: myAgent, channel, prompt, slot: mySlot,
        model: modelRef.current || null,
        provider: providerRef.current || null,
        folder: folder || null,
        sessionId: myConvId,
        attachments: attRels,
      });
      historyRef.current = updated;
      // Compose the final saved msgs from the store's completed live slice.
      const done = getAgentTurnSnapshot(mySlot);
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
        { role: "assistant", text: done.liveText || emptyReplyText, tools: done.liveTools as ToolLine[], timeline: done.timeline, streaming: false, at: Date.now(), usage: done.usage },
      ];
      // Only overwrite the visible pane if we're STILL viewing this agent+conv.
      if (agentId === myAgent && convIdRef.current === myConvId) {
        msgsRef.current = finalMsgs; setMsgs(finalMsgs);
      }
      void persistFor(myConvId, finalMsgs, historyRef.current);
      runningChannelRef.current = null;
    } catch (err) {
      const errMsgs: Msg[] = [...withUser, { role: "assistant", text: `✗ ${String(err)}`, tools: [], streaming: false, at: Date.now() }];
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

  // COMMAND PRO threads: live state is keyed per (agent, conv) so N threads on
  // the SAME agent stream independently (backend lanes are already per-session).
  const slotKey = agentId && convId ? turnSlotKey(agentId, convId) : (agentId ?? "");
  const inbound = (slotKey && getInbound(slotKey)) || (agentId && getInbound(agentId)) || undefined;

  // Per-agent live turn (from the App-level store). This is what makes switching
  // TO a running agent show its live stream + thinking dots — the store never
  // unmounts, so the stream is always captured and any pane can reattach.
  const turn = useAgentTurn(slotKey || null);
  const running = turn.status === "running";
  // Follow the live stream: msgs is static mid-turn now, so scroll on liveText + tools + timeline.
  // Respects user scroll: if they've scrolled up to read, don't yank them to bottom.
  useEffect(() => { if (running && isNearBottom(160)) pinToBottom(); }, [turn.liveText, turn.liveTools, turn.timeline, running]);
  // Keep pinned while streaming — any height change (new tool card, expanding body, markdown) pins to true bottom so the rounded frame never cuts off.
  // Guarded so a collapsed card / independent resize while scrolled up doesn't yank.
  useEffect(() => {
    const el = scrollRef.current; if (!el) return;
    const target = el.firstElementChild as Element | null; if (!target) return;
    const ro = new ResizeObserver(() => { if (running && isNearBottom(160)) pinToBottom(); });
    ro.observe(target);
    return () => ro.disconnect();
  }, [running]);
  // UI task #1: reload the viewed conv when a headless/continuation turn
  // persists, so the report STAYS on screen instead of vanishing.
  const convVersion = useConvVersion();
  useEffect(() => {
    if (convVersion > 0 && convIdRef.current && agentId && !isRunning(turnSlotKey(agentId, convIdRef.current))) { void openConv(convIdRef.current); }
    // eslint-disable-next-line
  }, [convVersion]);
  // Palette events: new thread / compact active thread.
  useEffect(() => {
    function onNew() { newConv(); }
    function onCompact() { void compactContext(); }
    window.addEventListener("aygent-new-thread", onNew);
    window.addEventListener("aygent-compact", onCompact);
    return () => { window.removeEventListener("aygent-new-thread", onNew); window.removeEventListener("aygent-compact", onCompact); };
    // eslint-disable-next-line
  }, [folder, agent]);
  // Tab status dots: re-render cheaply so background-thread activity shows.
  useEffect(() => {
    const t = setInterval(() => forceTabs((n) => n + 1), 2000);
    return () => clearInterval(t);
  }, []);
  // Herdr-style done/needs-input (B3): active thread + background tabs.
  const prevActiveRunning = useRef(false);
  useEffect(() => {
    const was = prevActiveRunning.current;
    prevActiveRunning.current = running;
    if (was && !running && agent) {
      const snap = slotKey ? getAgentTurnSnapshot(slotKey) : null;
      const text = snap?.liveText || "";
      const kind = kindFor(text);
      notifyThread(agent.name || "Agent", threadTitle(convId), kind, text);
    }
    // eslint-disable-next-line
  }, [running]);
  useEffect(() => {
    if (!agentId) return;
    const prev = prevRunningRef.current;
    let changed = false;
    for (const id of tabs) {
      if (id === convId) continue;
      const r = isRunning(turnSlotKey(agentId, id));
      if (prev[id] && !r) {
        const snap = getAgentTurnSnapshot(turnSlotKey(agentId, id));
        const text = snap?.liveText || "";
        notifyThread(agent?.name || "Agent", threadTitle(id), kindFor(text), text);
        changed = true;
      }
      prev[id] = r;
    }
    if (agentId && convId) prev[`${agentId}:${convId}`] = running;
    if (changed) forceTabs((n) => n + 1);
    // eslint-disable-next-line
  }, [convVersion, tabs.join(",")]);
  // In-app toasts for thread notifications.
  useEffect(() => {
    function onNotify(e: Event) {
      const d = (e as CustomEvent).detail;
      const id = Date.now() + Math.random();
      setToasts((t) => [...t.slice(-3), { id, agentName: d.agentName, threadTitle: d.threadTitle, kind: d.kind, body: d.body }]);
      setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 6000);
    }
    window.addEventListener("aygent-thread-notify", onNotify);
    return () => window.removeEventListener("aygent-thread-notify", onNotify);
  }, []);

  // ---- CONTEXT METER + $ COST (Mason, this session) -----------------------
  // The model's context window + price, fetched Rust-side (pricing.rs). Refetch
  // when the selected model changes so the % + cost track the real model.
  type ModelInfo = { context_tokens: number; known: boolean; price: ModelPrice };
  const [modelInfo, setModelInfo] = useState<ModelInfo | null>(null);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (!folder) { setModelInfo(null); return; }
      try {
        const sel = await invoke<{ provider: string; model: string }>("get_selection", { folder });
        const name = sel.model || "";
        // Local (GGUF) models price at 0 and report their real window via Usage;
        // cloud models resolve the REAL window dynamically from the provider API
        // (Anthropic beta models / OpenRouter /models) — pass the provider so the
        // backend hits the right endpoint instead of guessing from the id.
        const info = await invoke<ModelInfo>("chat_model_info", { provider: sel.provider || "", model: name });
        if (!cancelled) setModelInfo(info);
      } catch { if (!cancelled) setModelInfo(null); }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line
  }, [folder, convId, running]);

  // Per-conversation totals from the persisted per-message usage: SUM the
  // output/cache-write costs across turns; the CONTEXT FILL is the LATEST
  // assistant turn's input tokens (each turn re-sends the whole history, so the
  // last turn's input IS the current window occupancy). Live turn usage is
  // folded in so the meter moves DURING a turn, not only after it saves.
  const liveUsage = running ? turn.usage : undefined;
  const convUsage = (() => {
    let cost = 0, lastContextInput = 0;
    const price = modelInfo?.price;
    const add = (u?: TurnUsage) => {
      if (!u) return;
      // Context fill = the LATEST turn's total input (fresh + cached). Older
      // saved msgs may predate contextInput; fall back to input for those.
      const ci = (u.contextInput ?? 0) || (u.input + u.cacheRead + u.cacheWrite) || u.input || 0;
      if (ci > 0) lastContextInput = ci;
      // Cost bills each component at its own rate (see turnCost) — per-TURN,
      // summed across turns. The 5m/1h split wins over the folded total.
      if (price) cost += turnCost(u, price);
    };
    for (const m of msgs) if (m.role === "assistant" && m.usage) add(m.usage);
    if (liveUsage) { add(liveUsage); }
    return { cost, lastContextInput };
  })();
  // Context fill %: latest turn's input tokens over the model window. During a
  // live turn, prefer the live input count so the bar climbs as work happens.
  // Prefer a backend-reported window (local GGUF models report their real,
  // memory-capped window via Usage) over the pricing-table default.
  const reportedWindow = (() => {
    if (liveUsage?.contextWindow) return liveUsage.contextWindow;
    for (let i = msgs.length - 1; i >= 0; i--) {
      const m = msgs[i];
      if (m.role === "assistant" && m.usage?.contextWindow) return m.usage.contextWindow;
    }
    return 0;
  })();
  const ctxWindow = reportedWindow || modelInfo?.context_tokens || 0;
  // Context fill = latest turn's TOTAL input (fresh + cache read + cache create),
  // NOT fresh input alone. With prompt caching, fresh input is tiny (the "2/1M"
  // bug); the real prompt size is in the cache fields.
  const liveContextInput = liveUsage ? ((liveUsage.contextInput ?? 0) || (liveUsage.input + liveUsage.cacheRead + liveUsage.cacheWrite)) : 0;
  const ctxTokens = liveContextInput || convUsage.lastContextInput || 0;
  // Precise fraction for the BAR width (rounding to an int % made a real 0.4%
  // fill render as 0 and vanish); ctxPct (rounded) drives the color thresholds.
  const ctxFrac = ctxWindow > 0 ? Math.min(1, ctxTokens / ctxWindow) : 0;
  const ctxPct = Math.round(ctxFrac * 100);
  // Bar width: show at least a 4% sliver once there's ANY usage, so a small fill
  // is visibly "a little" rather than an empty (broken-looking) bar.
  const ctxBarWidth = ctxTokens > 0 ? Math.max(4, ctxFrac * 100) : 0;
  // ACCENT PILL (Mason 09-08): the meter is an outline pill in the accent
  // color that fills with the accent as context climbs. Above 90% it turns
  // red — unless the accent IS red, in which case it turns yellow so the
  // warning stays visible against a red UI.
  const accentIsRed = (() => {
    try {
      const cs = getComputedStyle(document.documentElement);
      const raw = (cs.getPropertyValue("--accent").trim() || "").toLowerCase();
      const m = /^#([0-9a-f]{6})$/.exec(raw);
      if (!m) return /red|e0533d|d64545|ff5c5c|c14b3f/.test(raw);
      const n = parseInt(m[1], 16);
      const r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255;
      return r > 150 && r - g > 60 && r - b > 60;
    } catch { return false; }
  })();
  const ctxOver = ctxPct >= 90;
  const ctxWarn = !ctxOver && ctxPct >= 75;
  const ctxFill = ctxOver ? (accentIsRed ? "#eab308" : "var(--danger)") : "var(--accent)";
  const ctxColor = ctxOver ? (accentIsRed ? "#eab308" : "var(--danger)") : ctxWarn ? "#d98a1f" : "var(--text-muted)";

  // COMPACT: summarize the model-facing history so a long chat can keep going.
  const [compacting, setCompacting] = useState(false);
  async function compactContext() {
    if (!convId || compacting || running) return;
    setCompacting(true);
    try {
      const seed = await invoke<unknown[]>("conv_compact", { id: convId });
      historyRef.current = seed;
      if (slotKey) setHistory(slotKey, seed);
      // Mark it in the transcript so the user sees it happened, then persist the
      // shrunk history against the (unchanged) visible transcript.
      const note: Msg = { role: "assistant", text: "\u{1F5DC}\uFE0F Context compacted \u2014 earlier turns summarized to free up the window. The visible chat is unchanged; I kept the gist.", tools: [], streaming: false, at: Date.now() };
      setMessages([...msgsRef.current, note]);
      await persistFor(convId, msgsRef.current, seed);
    } catch (e) {
      alert("Compact failed: " + String(e));
    } finally { setCompacting(false); }
  }

  return (
    <div
      onDragEnter={onDropZoneDragEnter}
      onDragOver={onDropZoneDragOver}
      onDragLeave={onDropZoneDragLeave}
      onDrop={onDropZoneDrop}
      style={{ display: "flex", height: "100%", minHeight: 0, gap: "var(--space-4)", position: "relative" }}>
      {dragOver && (
        <div style={{
          position: "absolute", inset: 0, zIndex: 30, pointerEvents: "none",
          display: "flex", alignItems: "center", justifyContent: "center",
          background: "color-mix(in srgb, var(--accent) 12%, transparent)",
          border: "2px dashed var(--accent)", borderRadius: "var(--radius-card)",
        }}>
          <span style={{
            fontSize: 15, fontWeight: 700, color: "var(--text)",
            background: "var(--surface)", border: "var(--border-width) solid var(--line)",
            borderRadius: 999, padding: "8px 16px", boxShadow: "var(--elevation)",
          }}>Drop files to attach</span>
        </div>
      )}
      {/* MAIN CHAT COLUMN. In multi-pane mode it flexes to share width; solo it
         stays centered. height:100% + flex so the input pins to the bottom. */}
      {/* WIDTH (Mason 08-04): the old `maxWidth: 720` left ~25% dead space on
          each side of a wide window. Now the column is fluid — it fills the
          available space and only reins in on very large displays, leaving a
          5-10% breathing margin instead of 50%. The messages, the input row and
          the buttons all live inside this column, so they share one measurement
          and stay aligned by construction. */}
      <div style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 0, minHeight: 0, width: "100%", maxWidth: multi ? "none" : 1600, margin: multi ? 0 : "0 auto" }}>
        {/* Header: agent name (multi) or "Chat" + editable chat name underneath.
           In multi-pane, each pane is labeled with its AGENT so you always know
           who you're talking to; a close button removes just this pane. */}
        <div style={{ margin: "0 0 var(--space-3)", flexShrink: 0, display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 8 }}>
          <div style={{ minWidth: 0 }}>
            <h2 style={{ fontSize: "var(--text-h1)", fontWeight: "var(--weight-heading)", margin: 0, display: "flex", alignItems: "center", gap: 8, overflow: "hidden" }}>
              <span style={{ color: "var(--accent)", display: "flex" }}><Icon name={(agent?.icon as IconName) || "sparkles"} size={20} /></span>
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{agent?.name || "Agent"}</span>
            </h2>
            <ChatTitle
              title={convs.find((c) => c.id === convId)?.title || ""}
              disabled={!folder || !convId}
              onRename={(next) => renameCurrent(next)}
            />
            {/* COMMAND PRO per-thread model override (B1): null = agent default. */}
            {!!folder && !!convId && (
              <ThreadModelPicker
                agentDefault={{ provider: providerRef.current || agent?.provider || "anthropic", model: modelRef.current || agent?.model || "" }}
                value={getThreadModel(convId)}
                onChange={(v) => { if (convId) setThreadModel(convId, v); }}
              />
            )}
            {/* CONTEXT METER + $ COST (Mason, this session): a compact row under
               the chat title showing how full the model's context window is and
               the running cost of this session, so you SEE the wall coming and
               can Compact before you hit it. Only shows once we have a window. */}
            {!!folder && !!convId && ctxWindow > 0 && (
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginTop: 6, flexWrap: "wrap" }}>
                <div title={`${ctxTokens.toLocaleString()} / ${ctxWindow.toLocaleString()} tokens in context`}
                  style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  {/* accent outline pill: fills with the accent as context climbs */}
                  <div style={{
                    width: 64, height: 14, borderRadius: 999, overflow: "hidden", position: "relative",
                    border: `var(--border-width) solid ${ctxOver ? ctxFill : "var(--accent)"}`,
                    background: "transparent",
                  }}>
                    <div style={{ width: `${ctxBarWidth}%`, height: "100%", background: ctxFill, transition: "width 200ms" }} />
                  </div>
                  <span style={{ fontSize: 12, color: ctxColor, fontVariantNumeric: "tabular-nums", fontWeight: ctxPct >= 75 ? 600 : 400 }}>
                    {fmtTokens(ctxTokens)} / {fmtTokens(ctxWindow)} tokens
                  </span>
                </div>
                {convUsage.cost > 0 && (
                  <span title="Running cost of this chat, based on the model's price" style={{ fontSize: 12, color: "var(--text-muted)", fontVariantNumeric: "tabular-nums" }}>
                    · {fmtCost(convUsage.cost)}
                  </span>
                )}
                <button
                  onClick={() => void compactContext()}
                  disabled={compacting || running || !convId}
                  title="Save context by summarizing chat history with fewer tokens."
                  style={{
                    fontSize: 11.5, cursor: compacting || running ? "default" : "pointer",
                    padding: "2px 8px", borderRadius: 999, fontFamily: "inherit",
                    border: "var(--border-width) solid var(--line)", background: "var(--bg)",
                    color: ctxPct >= 75 ? ctxColor : "var(--text-muted)", opacity: compacting ? 0.6 : 1,
                  }}
                >{compacting ? "Compacting…" : "Compact Context"}</button>
              </div>
            )}
            {/* At-the-wall warning: if context is nearly full, say so plainly. */}
            {!!folder && !!convId && ctxWindow > 0 && ctxPct >= 85 && (
              <div style={{ marginTop: 6, fontSize: 12, color: ctxOver ? ctxFill : "var(--danger)", maxWidth: 420 }}>
                Context is nearly full ({fmtTokens(ctxTokens)} / {fmtTokens(ctxWindow)} tokens) — Compact now to avoid losing your next long reply.
              </div>
            )}
          </div>
          <div style={{ display: "flex", gap: 6, flexShrink: 0 }}>
          {!multi && !blocked && (
            <button
              onClick={() => setHistoryCollapsed((v) => !v)}
              title={historyCollapsed ? "Show history" : "Hide history"}
              style={{ background: historyCollapsed ? "var(--surface)" : "none", border: "1px solid var(--line)", cursor: "pointer", padding: 4, display: "flex", color: "var(--text-muted)", borderRadius: 7 }}
            ><Icon name="chat" size={14} /></button>
          )}
          {multi && !blocked && (
            <button
              onClick={() => setHistoryOpen((o) => !o)}
              title={historyOpen ? "Hide chats" : "Show chats"}
              style={{ background: historyOpen ? "var(--surface)" : "none", border: "var(--border-width) solid var(--line)", cursor: "pointer", padding: 4, display: "flex", color: "var(--text-muted)", borderRadius: "var(--radius-control)" }}
            ><Icon name="chat" size={16} /></button>
          )}
          {closable && (
            <button
              onClick={onClose}
              title="Close this pane"
              style={{ background: "none", border: "none", cursor: "pointer", padding: 4, display: "flex", color: "var(--text-muted)", flexShrink: 0 }}
            ><Icon name="close" size={16} /></button>
          )}
          </div>
        </div>

        {/* COMMAND PRO threads: parallel sessions, one tab per conv. */}
        {!blocked && tabs.length > 0 && (
          <div className="aygent-scroll" style={{ display: "flex", gap: 6, overflowX: "auto", padding: "2px 0 8px", flexShrink: 0 }}>
            {tabs.map((id) => {
              const active = id === convId;
              const title = threadTitle(id);
              const r = agentId ? isRunning(turnSlotKey(agentId, id)) : false;
              const om = getThreadModel(id);
              return (
                <span key={id} onClick={() => { if (id !== convId) void openConv(id); }}
                  title={title + (om ? " - " + om.model : "")}
                  style={{ display: "inline-flex", alignItems: "center", gap: 6, maxWidth: 220, padding: "5px 6px 5px 10px", borderRadius: 999, fontSize: 12.5, cursor: "pointer", whiteSpace: "nowrap", flexShrink: 0, border: active ? "1px solid var(--accent)" : "1px solid var(--line)", background: active ? "var(--surface)" : "transparent", color: "var(--text)", fontWeight: active ? 700 : 500 }}>
                  <span style={{ width: 7, height: 7, borderRadius: "50%", flexShrink: 0, background: r ? "var(--accent)" : "var(--text-faint)", animation: r ? "aygentPulse 1.1s ease-in-out infinite" : "none" }} />
                  <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{title}</span>
                  {tabs.length > 1 && (
                    <button onClick={(e) => { e.stopPropagation(); closeTab(id); }} title="Close thread"
                      style={{ background: "none", border: "none", cursor: "pointer", padding: "0 2px", color: "var(--text-faint)", display: "flex" }}><Icon name="close" size={12} /></button>
                  )}
                </span>
              );
            })}
            <button onClick={() => newConv()} title="New thread (parallel session)"
              style={{ flexShrink: 0, borderRadius: 999, fontSize: 12.5, fontWeight: 600, padding: "5px 12px", cursor: "pointer", border: "1px dashed var(--line)", background: "transparent", color: "var(--text-muted)" }}>+ New</button>
          </div>
        )}
        {blocked && (
          <p style={{ ...hint, marginBottom: 12 }}>
            {!folder ? "Pick an Agent Folder in Settings, " : ""}{!keySet ? `add ${providerLabel(providerRef.current)} key in Settings (or switch this agent to a local model)` : ""} to start.
          </p>
        )}

        {/* Messages bottom-align: newest sits just above the input, older scroll
           up (justifyContent flex-end + margin-top auto on the list wrapper). */}
        <div ref={scrollRef} className="aygent-scroll" style={{ flex: 1, overflowY: "auto", overflowX: "hidden", display: "flex", flexDirection: "column", padding: "4px 6px 24px 2px" }}>
          <div style={{ marginTop: "auto", display: "flex", flexDirection: "column", gap: 10 }}>
          {msgs.length === 0 && !running && !blocked && (
            <p style={hint}>Say hello, or ask your agent to work with files in your folder.</p>
          )}
          {msgs.map((m, i) => <Bubble key={i} m={m} agentId={agentId} price={modelInfo?.price} local={isLocal} />)}
          {/* LIVE inter-agent inbound message: when a peer dispatches a message
              to the agent you're viewing, show it as a user bubble immediately
              (before the reply streams) so you WATCH the conversation arrive. */}
          {running && inbound && (
            <Bubble agentId={agentId} m={{ role: "user", text: `from ${inbound.fromName}: ${inbound.text}` }} />
          )}
          {/* LIVE turn for the agent being viewed: render a trailing streaming
              bubble fed by the store, so switching to a running agent shows its
              tokens + tool cards arriving mid-flight (Atlas #2), for BOTH human
              turns and headless inter-agent turns (same store slot). */}
          {running && !(msgs.length > 0 && msgs[msgs.length - 1].role === "assistant" && (msgs[msgs.length - 1] as { streaming?: boolean }).streaming) && (
            <Bubble agentId={agentId} local={isLocal} m={{
              role: "assistant",
              text: turn.liveText,
              tools: turn.liveTools.map((t) => ({ name: t.name, path: t.path ?? "", ok: t.ok, detail: t.detail, summary: t.summary, body: t.body, running: t.running, spark: t.spark, before: t.before ?? null })),
              timeline: turn.timeline,
              streaming: true,
            }} />
          )}
          </div>
          <div ref={bottomRef} aria-hidden style={{ height: 8, flexShrink: 0 }} />
        </div>

        {/* COMMAND PRO toasts: background-thread done / needs-input. */}
        {toasts.length > 0 && (
          <div style={{ position: "absolute", right: 12, bottom: 76, display: "flex", flexDirection: "column", gap: 8, zIndex: 40, maxWidth: 340 }}>
            {toasts.map((t) => (
              <div key={t.id} style={{ background: "var(--surface)", border: t.kind === "needs-input" ? "1px solid var(--warn)" : "1px solid var(--line)", borderRadius: 10, padding: "9px 12px", boxShadow: "0 8px 28px rgba(0,0,0,0.3)", fontSize: 12.5 }}>
                <div style={{ fontWeight: 700 }}>{t.agentName} - {t.threadTitle} - {t.kind === "needs-input" ? "needs you" : "done"}</div>
                <div style={{ color: "var(--text-muted)", marginTop: 2, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{t.body}</div>
              </div>
            ))}
          </div>
        )}
        {/* Task #5: attachment chips above the input */}
        {attachments.length > 0 && (
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 8, padding: "0 8px" }}>
            {attachments.map((a) => (
              <span key={a.name} style={{
                display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12,
                padding: "4px 10px", borderRadius: 999, border: "var(--border-width) solid var(--line)",
                background: "var(--surface)", color: a.pending ? "var(--text-faint)" : "var(--text)",
              }}>
                {a.name}{a.pending ? "…" : ""}
                {!a.pending && (
                  <button onClick={() => setAttachments((x) => x.filter((y) => y.name !== a.name))}
                    style={{ background: "none", border: "none", cursor: "pointer", padding: 0, color: "var(--text-muted)" }}>✕</button>
                )}
              </span>
            ))}
          </div>
        )}
        <div style={{ display: "flex", gap: 8, marginTop: "var(--space-3)", flexShrink: 0, alignItems: "flex-end", position: "relative", padding: "0 8px" }}>
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
              width: 36, height: 38, flexShrink: 0, cursor: "pointer",
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
              width: 36, height: 38, flexShrink: 0, cursor: "pointer",
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
              height: 38, maxHeight: 200, lineHeight: 1.45,
              boxSizing: "border-box",
              background: "var(--bg)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", color: "var(--text)", padding: "10px 12px",
              fontSize: 15, fontFamily: "inherit",
            }} />
          <Button onClick={running ? stop : send} disabled={blocked || (!running && !input.trim())}>{running ? "Stop" : "Send"}</Button>
        </div>
      </div>

      {/* HISTORY SIDEBAR — in multi-pane, behind hamburger to save space; solo, always visible */}
      {!blocked && !multi && !historyCollapsed && (
        <HistorySidebar
          multi={multi}
          convs={convs} activeId={convId} busy={running} dragId={dragId} overId={overId}
          listElRef={listElRef}
          renamingId={renamingId}
          onCommitRename={(id, title) => { setRenamingId(null); void saveConvTitle(id, title); }}
          onNew={newConv} onOpen={openConv} onDelete={deleteConv} onRename={renameConv}
          onPin={togglePin} onPointerDragStart={startPointerDrag}
        />
      )}
      {!blocked && multi && historyOpen && (
        <div style={{ position: "absolute", top: 0, right: 0, bottom: 0, width: 216, background: "var(--bg)", borderLeft: "var(--border-width) solid var(--line)", zIndex: 5, padding: "12px 0 12px 14px", display: "flex", flexDirection: "column" }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8, paddingRight: 8 }}>
            <span style={{ fontSize: 12, fontWeight: 800, color: "var(--text-faint)" }}>CHATS</span>
            <button onClick={() => setHistoryOpen(false)} style={{ background: "none", border: "none", cursor: "pointer", color: "var(--text-muted)" }}><Icon name="close" size={14} /></button>
          </div>
          <HistorySidebar
            multi={multi}
            convs={convs} activeId={convId} busy={running} dragId={dragId} overId={overId}
            listElRef={listElRef}
            renamingId={renamingId}
            onCommitRename={(id, title) => { setRenamingId(null); void saveConvTitle(id, title); }}
            onNew={() => { newConv(); setHistoryOpen(false); }} onOpen={(id) => { openConv(id); setHistoryOpen(false); }} onDelete={deleteConv} onRename={renameConv}
            onPin={togglePin} onPointerDragStart={startPointerDrag}
          />
        </div>
      )}
    </div>
  );
}

function HistorySidebar({
  multi,
  convs, activeId, busy, dragId, overId, listElRef, renamingId, onCommitRename,
  onNew, onOpen, onDelete, onRename, onPin, onPointerDragStart,
}: {
  multi: boolean;
  convs: ConvMeta[]; activeId: string | null; busy: boolean;
  dragId: string | null; overId: string | null;
  listElRef: React.RefObject<HTMLDivElement>;
  renamingId: string | null;
  onCommitRename: (id: string, title: string) => void;
  onNew: () => void; onOpen: (id: string) => void; onDelete: (id: string) => void;
  onRename: (id: string) => void;
  onPin: (id: string) => void;
  onPointerDragStart: (id: string, e: React.PointerEvent) => void;
}) {
  return (
    <div style={{
      display: "flex", flexDirection: "column", gap: 8,
      // SINGLE pane: fixed 230px column with its own divider; bleed past
      // App.tsx's 28px top/bottom padding so the divider reaches the window
      // edges (grow height by the bled amount — a negative margin SHIFTS, it
      // doesn't stretch). MULTI pane: the hamburger OVERLAY already provides
      // the width, left divider and padding — duplicating them here overflowed
      // the overlay and drew a second, misaligned line (Mason 08-19, bug #3).
      ...(multi
        ? { height: "100%", minHeight: 0, width: "100%", paddingRight: 8 }
        : {
            width: 216, flexShrink: 0,
            height: "calc(100% + 56px)",
            marginTop: -28, marginBottom: -28, paddingTop: 28, paddingBottom: 28,
            borderLeft: "var(--border-width) solid var(--line)", paddingLeft: 14,
          }),
    }}>
      <Button onClick={onNew} disabled={false}>+ New thread</Button>
      <div ref={listElRef} className="aygent-scroll" style={{ overflowY: "auto", display: "flex", flexDirection: "column", gap: 4, marginTop: 4, flex: 1, minHeight: 0 }}>
        {convs.length === 0 && (
          <p style={{ ...hint, fontSize: 13, color: "var(--text-faint)" }}>No chats yet.</p>
        )}
        {convs.map((c) => (
          <HistoryItem
            key={c.id} c={c} active={c.id === activeId}
            dragging={dragId === c.id} isOver={overId === c.id && dragId !== null && dragId !== c.id}
            renaming={renamingId === c.id}
            onCommitRename={(t) => onCommitRename(c.id, t)}
            onOpen={() => onOpen(c.id)} onDelete={() => onDelete(c.id)} onRename={() => onRename(c.id)} onPin={() => onPin(c.id)}
            onPointerDown={(e) => onPointerDragStart(c.id, e)}
          />
        ))}
      </div>
    </div>
  );
}

function HistoryItem({
  c, active, dragging, isOver, renaming, onCommitRename, onOpen, onDelete, onRename, onPin, onPointerDown,
}: {
  c: ConvMeta; active: boolean; dragging: boolean; isOver: boolean;
  renaming: boolean; onCommitRename: (title: string) => void;
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
        padding: "6px 9px", borderRadius: 7, fontSize: 12.5,
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
      {renaming ? (
        <input
          autoFocus
          defaultValue={c.title || ""}
          onPointerDown={(e) => e.stopPropagation()}
          onClick={(e) => e.stopPropagation()}
          onBlur={(e) => onCommitRename(e.currentTarget.value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") { e.preventDefault(); onCommitRename(e.currentTarget.value); }
            if (e.key === "Escape") { e.preventDefault(); onCommitRename(c.title || ""); }
          }}
          style={{
            flex: 1, minWidth: 0, background: "var(--bg)", color: "var(--text)",
            border: "var(--border-width) solid var(--accent)", borderRadius: 4,
            padding: "2px 6px", fontSize: 13, fontFamily: "inherit",
          }}
        />
      ) : (
        <span style={{ flex: 1, minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", color: "var(--text)" }}>
          {c.title || "Untitled"}
        </span>
      )}
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

const THREAD_PROVIDERS = ["anthropic", "openai", "openrouter", "meta", "local", "mlx"];
function ThreadModelPicker({ agentDefault, value, onChange }: {
  agentDefault: { provider: string; model: string };
  value: { provider: string; model: string } | null;
  onChange: (v: { provider: string; model: string } | null) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [prov, setProv] = useState(value?.provider || agentDefault.provider);
  const [mod, setMod] = useState(value?.model || agentDefault.model);
  useEffect(() => { setProv(value?.provider || agentDefault.provider); setMod(value?.model || agentDefault.model); }, [value?.provider, value?.model, agentDefault.provider, agentDefault.model]);
  if (!editing) {
    const label = value ? (value.model || value.provider) + " (thread)" : (agentDefault.model || agentDefault.provider) + " (agent)";
    return (
      <button onClick={() => setEditing(true)} title="Override model for this thread only — other threads keep the agent default"
        style={{ marginTop: 4, display: "inline-flex", alignItems: "center", gap: 6, background: "transparent", border: "1px solid var(--line)", borderRadius: 999, cursor: "pointer", padding: "3px 10px", color: "var(--text-muted)", fontSize: 12, fontFamily: "ui-monospace, monospace", maxWidth: 360 }}>
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{label}</span>
        <span style={{ opacity: 0.6 }}>▾</span>
      </button>
    );
  }
  return (
    <span style={{ marginTop: 4, display: "inline-flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
      <select value={prov} onChange={(e) => setProv(e.target.value)}
        style={{ background: "var(--bg)", border: "1px solid var(--line)", borderRadius: 7, color: "var(--text)", padding: "3px 6px", fontSize: 12 }}>
        {THREAD_PROVIDERS.map((x) => <option key={x} value={x}>{x}</option>)}
      </select>
      <input value={mod} onChange={(e) => setMod(e.target.value)} placeholder="model id (blank = auto)"
        spellCheck={false}
        style={{ background: "var(--bg)", border: "1px solid var(--line)", borderRadius: 7, color: "var(--text)", padding: "3px 8px", fontSize: 12, fontFamily: "ui-monospace, monospace", width: 200 }} />
      <button onClick={() => { onChange(mod.trim() || prov !== agentDefault.provider ? { provider: prov, model: mod.trim() } : null); setEditing(false); }}
        style={{ background: "var(--accent)", color: "var(--bg)", border: "1px solid var(--line)", borderRadius: 7, padding: "3px 10px", fontSize: 12, fontWeight: 700, cursor: "pointer" }}>Set</button>
      {value && <button onClick={() => { onChange(null); setEditing(false); }} title="Back to agent default"
        style={{ background: "transparent", border: "1px solid var(--line)", borderRadius: 7, padding: "3px 10px", fontSize: 12, cursor: "pointer", color: "var(--text-muted)" }}>Reset</button>}
      <button onClick={() => setEditing(false)}
        style={{ background: "none", border: "none", cursor: "pointer", color: "var(--text-faint)", fontSize: 12 }}>✕</button>
    </span>
  );
}

/** Format a $ cost compactly: sub-cent shows more digits so it isn't just $0.00. */
function fmtCost(usd: number): string {
  if (!usd || usd <= 0) return "$0.00";
  if (usd < 0.01) return "$" + usd.toFixed(4);
  if (usd < 1) return "$" + usd.toFixed(3);
  return "$" + usd.toFixed(2);
}

/** Compact token count (DECIMAL thousands, matching how context windows are
 *  advertised): 1234 -> "1.2k", 1_000_000 -> "1M", 1_500_000 -> "1.5M". */
function fmtTokens(n: number): string {
  if (!n) return "0";
  if (n < 1000) return String(n);
  if (n < 1_000_000) return (n / 1000).toFixed(n < 10000 ? 1 : 0) + "k";
  const m = n / 1_000_000;
  return (m < 10 ? m.toFixed(m % 1 === 0 ? 0 : 1) : m.toFixed(0)) + "M";
}

export type ModelPrice = { input: number; output: number; cache_read: number; cache_write: number; cache_write_5m?: number; cache_write_1h?: number };

/** Per-turn $ cost from a usage record + the model's price ($/Mtok). 0 if no price.
 *  OpenCode parity: five separate buckets (fresh in / out / cache read /
 *  cache write / reasoning-at-out-rate — here reasoning rides inside output).
 *  When the 5m/1h write split is present it wins over the folded total —
 *  never bill both (that would double-count the writes). */
export function turnCost(u: TurnUsage, price?: ModelPrice): number {
  if (!price) return 0;
  const w5 = u.cacheWrite5m ?? 0, w1 = u.cacheWrite1h ?? 0;
  const cw = (w5 + w1 > 0)
    ? w5 * (price.cache_write_5m ?? price.cache_write) + w1 * (price.cache_write_1h ?? price.cache_write)
    : u.cacheWrite * price.cache_write;
  return (u.input * price.input + u.output * price.output + u.cacheRead * price.cache_read + cw) / 1_000_000;
}

/** HH:MM:SS in the user's locale, 24h so it's a fixed width in the margin. */
function fmtClock(ms?: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** One-line meta stamp UNDER the message: clock · tokens · cost. Renders
 *  nothing when there is nothing to show, so rows never shift. */
function MetaStamp({ at, usage, price, align }: { at?: number; usage?: TurnUsage; price?: ModelPrice; align: "left" | "right" }) {
  const cost = usage ? turnCost(usage, price) : 0;
  const ctxIn = usage ? ((usage.contextInput ?? 0) || (usage.input + usage.cacheRead + usage.cacheWrite)) : 0;
  const toks = usage ? ctxIn + usage.output : 0;
  const parts: string[] = [];
  const clock = fmtClock(at);
  if (clock) parts.push(clock);
  if (usage && toks > 0) parts.push(fmtTokens(toks) + " tok");
  if (cost > 0) parts.push(fmtCost(cost));
  if (parts.length === 0) return null;
  return (
    <span
      title={usage ? ctxIn.toLocaleString() + " in (incl. cache) + " + (usage.output).toLocaleString() + " out tokens" : undefined}
      style={{ fontSize: 11, lineHeight: "16px", color: "var(--text-faint)", fontVariantNumeric: "tabular-nums", userSelect: "none", textAlign: align }}>
      {parts.join(" · ")}
    </span>
  );
}

function Bubble({ m, agentId, price, local }: { m: Msg; agentId?: string | null; price?: ModelPrice; local?: boolean }) {
  const isUser = m.role === "user";
  const memory = isUser && m.role === "user" ? m.memory : undefined;
  // Full-bleed rows: no margin gutters. Clock/tokens/cost ride as one faint
  // line UNDER the message (MetaStamp, rendered in BubbleBody).
  return (
    <div style={{ display: "flex", alignItems: "flex-start", width: "100%" }}>
      <BubbleBody m={m} isUser={isUser} local={local} memory={memory} agentId={agentId} price={price} />
    </div>
  );
}

function BubbleBody({ m, isUser, memory, agentId, local, price }: { m: Msg; isUser: boolean; memory?: string; agentId?: string | null; local?: boolean; price?: ModelPrice }) {
  const usage = !isUser && m.role === "assistant" ? m.usage : undefined;
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "stretch", minWidth: 0, flex: 1 }}>
      <div style={{
        width: "100%",
        minWidth: 0,
        overflowWrap: "anywhere",
        margin: "0 2px",
        background: isUser
          ? "color-mix(in srgb, var(--accent) 12%, transparent)"
          : "color-mix(in srgb, var(--text) 5%, transparent)",
        color: "var(--text)",
        border: "none",
        ...(isUser
          ? {
              borderRight: "2px solid var(--accent)",
              borderRadius: "8px 0 0 8px",
              padding: "8px 12px 8px 24px",
            }
          : {
              borderLeft: "2px solid color-mix(in srgb, var(--text) 25%, transparent)",
              borderRadius: "0 8px 8px 0",
              padding: "8px 48px 8px 12px",
            }),
        display: "flex", flexDirection: "column", gap: 6,
      }}>
        {/* ORDERED RENDER (Mason 08-04): when a timeline exists, draw tool cards
            and prose in the order they actually happened, so each note sits with
            the calls it describes instead of every card being hoisted to the top.
            Consecutive tool calls stay visually stacked (tight gap); a text
            segment gets breathing room above it. Falls back to the old
            all-cards-then-all-text layout for conversations saved before this. */}
        {!isUser && m.role === "assistant" && m.timeline && m.timeline.length > 0 ? (
          m.timeline.map((item, i) => {
            if (item.kind === "tool") {
              const prevWasTool = i > 0 && m.timeline![i - 1].kind === "tool";
              return (
                <div key={i} style={{ marginTop: prevWasTool ? 4 : 10 }}>
                  <ToolCard t={item.tool as ToolLine} agentId={agentId} />
                </div>
              );
            }
            const text = item.text.trim();
            if (!text) return null;
            return (
              <div key={i} style={{ marginTop: i === 0 ? 0 : 10 }}>
                <TextWithThoughts text={text} streaming={m.streaming} enabled={local} />
              </div>
            );
          })
        ) : (
          <>
            {!isUser && m.role === "assistant" && m.tools.map((t, i) => <ToolCard key={i} t={t} agentId={agentId} />)}
            {m.text && (isUser
              ? <span style={{ display: "block", textAlign: "left", whiteSpace: "pre-wrap", lineHeight: 1.55, fontSize: 14 }}>{m.text}</span>
              : <TextWithThoughts text={m.text} streaming={(m as { streaming?: boolean }).streaming} enabled={local} />)}
          </>
        )}
        {!isUser && m.role === "assistant" && m.streaming && !m.text && <Thinking />}
      </div>
      <MetaStamp at={m.at} usage={usage} price={price} align={isUser ? "right" : "left"} />
      {memory && (
        <span style={{ marginTop: 3, alignSelf: isUser ? "flex-end" : "flex-start", fontSize: 12, color: "#3fa46a" }}>{memory}</span>
      )}
    </div>
  );
}

function ToolCard({ t, agentId }: { t: ToolLine; agentId?: string | null }) {
  // SPARKS: a spark_preview call renders as a live inline mini-app, not a
  // collapsed code card.
  if (t.spark && t.spark.html) return <SparkCard spark={t.spark} agentId={agentId} />;
  // DIFF VIEW (Mason 09-09): code-editing calls render a streaming side-by-side
  // diff (old left/red, new right/green) instead of the plain code dump. The
  // before-snapshot lands async via tool_file_before; until then the card falls
  // back to the plain body view, then swaps to the diff live.
  const diffable = isDiffable(t.name, t.before ?? null);
  const showDiff = diffable && !!t.body;
  const counts = useMemo(
    () => (showDiff ? countDiff(t.before!, t.body!) : null),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [showDiff, t.before?.content, t.body],
  );
  const [userOpen, setUserOpen] = useState<boolean | null>(null); // null = no manual toggle yet
  const pending = t.ok === undefined;
  // Diff cards default OPEN (streams as the model writes); plain cards keep the
  // old behavior (auto-expand while running, collapse when done). Manual wins.
  const open = userOpen !== null ? userOpen : showDiff ? true : (!!t.running && !!t.body);
  const setOpen = (f: (o: boolean) => boolean) => setUserOpen(f(open));
  const paneRef = useRef<HTMLDivElement | null>(null);
  // Follow the stream: keep the pane pinned to the bottom while content grows
  // during a live call. Finished cards never yank the reader's scroll. Diff
  // panes are excluded — pinning to the bottom would hide the changed lines.
  useEffect(() => {
    if (open && t.running && !showDiff && paneRef.current) paneRef.current.scrollTop = paneRef.current.scrollHeight;
  }, [open, t.running, t.body, showDiff]);
  const color = pending ? "var(--text-muted)" : t.ok ? "var(--ok)" : "var(--danger)";
  // A path is "revealable" once the call succeeded and points at a real file
  // (list_files on '.' or a refused call has nothing useful to reveal).
  const revealable = t.ok === true && !!t.path && t.path !== ".";
  const expandable = !!(t.body || t.detail || showDiff);

  async function reveal() {
    try { await invoke("reveal_in_finder", { path: t.path }); } catch { /* jail refused — ignore */ }
  }

  // Collapsed row: real context, not just the tool name (Mason 08-03: "every
  // command just said 'shell run'"). summary is built in turns.ts from the
  // full tool input; older history rows without one fall back to name(path).
  const label = t.summary || `⚙ ${t.name}(${t.path || ""})`;

  return (
    <div style={{
      fontFamily: "ui-monospace, monospace", fontSize: 12.5,
      border: `var(--border-width) solid ${color}`, color,
      borderRadius: "var(--radius-control)",
      background: "var(--bg)", overflow: "hidden",
    }}>
      <div
        onClick={() => { if (expandable) setOpen((o) => !o); }}
        style={{
          display: "flex", alignItems: "center", gap: 8, padding: "6px 10px",
          cursor: expandable ? "pointer" : "default", userSelect: "none",
        }}
        title={expandable ? (open ? "Collapse" : "Expand") : undefined}
      >
        {expandable && (
          <span style={{ fontSize: 10, opacity: 0.7, transform: open ? "rotate(90deg)" : "none", transition: "transform 120ms" }}>▶</span>
        )}
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>{label}</span>
        {revealable && (
          <button
            onClick={(e) => { e.stopPropagation(); void reveal(); }}
            title="Reveal in Finder"
            style={{
              font: "inherit", color: "inherit", background: "none", border: "none",
              padding: 0, cursor: "pointer", textDecoration: "underline", textUnderlineOffset: 2,
              maxWidth: 220, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
            }}
          >{t.path}</button>
        )}
        {counts && <DiffCounts add={counts.add} del={counts.del} />}
        <span style={{ flexShrink: 0 }}>{pending ? "…" : t.ok ? "✓" : "✗"}</span>
      </div>
      {/* Collapsed error hint: the first line of a failure is visible WITHOUT
          expanding — failures shouldn't hide. */}
      {!open && t.ok === false && t.detail && (
        <div style={{ padding: "0 10px 6px 10px", opacity: 0.85, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {t.detail.split("\n")[0].slice(0, 160)}
        </div>
      )}
      {/* Expanded pane: bounded height, scrolls internally — watch code/output
          without the transcript growing by thousands of lines. */}
      {open && (
        <div ref={paneRef} style={{
          borderTop: `var(--border-width) solid ${color}`,
          maxHeight: 300, overflow: "auto", padding: "8px 10px",
          color: "var(--text)", background: "var(--surface)",
        }}>
          {showDiff ? (
            <DiffView before={t.before!} after={t.body!} running={t.running} />
          ) : (t.body && (
            <pre style={{ margin: 0, whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 12 }}>{t.body}</pre>
          ))}
          {t.body && t.detail && <div style={{ height: 8 }} />}
          {t.detail && (
            <pre style={{ margin: 0, whiteSpace: "pre-wrap", wordBreak: "break-word", fontSize: 12, opacity: 0.85 }}>{t.detail}</pre>
          )}
        </div>
      )}
    </div>
  );
}

// SPARKS: an interactive mini-app previewed LIVE inline in the chat. Renders the
// agent's html in a SANDBOXED iframe (allow-scripts, NO same-origin) — it runs
// its own JS + embedded data but can't reach the app, files, or the agent. The
// user iterates by asking for changes (the agent re-emits spark_preview, same
// slug, and this card hot-swaps), then clicks Save to write it to the library.
function SparkCard({ spark, agentId }: { spark: { slug: string; title: string; html: string }; agentId?: string | null }) {
  const [expanded, setExpanded] = useState(true);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [seedState, setSeedState] = useState<Record<string, unknown>>({});
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const blobUrl = useSparkBlobUrl(spark.html, seedState);
  useSparkThemeSync(iframeRef);
  useEffect(() => {
    if (!agentId || !spark.slug) return;
    invoke<Record<string, unknown>>("spark_state_get", { agentId, slug: spark.slug })
      .then((saved) => { if (saved && Object.keys(saved).length) setSeedState(saved); })
      .catch(() => {});
  }, [agentId, spark.slug]);
  useEffect(() => {
    function onMessage(ev: MessageEvent) {
      if (!agentId || !spark.slug) return;
      if (!iframeRef.current || ev.source !== iframeRef.current.contentWindow) return;
      if (!isSparkStateMsg(ev.data)) return;
      invoke("spark_state_set_key", { agentId, slug: spark.slug, key: ev.data.key, value: ev.data.value ?? null }).catch(() => {});
    }
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [agentId, spark.slug]);

  async function save() {
    if (!agentId) { setErr("no agent"); return; }
    setSaving(true); setErr(null);
    try {
      await invoke("spark_save", { agentId, slug: spark.slug, title: spark.title, html: spark.html, description: "" });
      setSaved(true);
      window.dispatchEvent(new Event("aygent-tools-changed"));
    } catch (e) { setErr(String(e)); }
    finally { setSaving(false); }
  }

  return (
    <div style={{
      border: "var(--border-width) solid var(--accent, var(--line))",
      borderRadius: "var(--radius-control)", overflow: "hidden",
      background: "var(--bg)", boxShadow: "var(--elevation)", margin: "2px 0",
    }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "7px 10px", borderBottom: expanded ? "var(--border-width) solid var(--line)" : "none" }}>
        <span style={{ fontSize: 13 }}>⚡</span>
        <span style={{ fontWeight: 700, fontSize: 13, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{spark.title || spark.slug}</span>
        <button onClick={() => setExpanded((e) => !e)} title={expanded ? "Collapse" : "Expand"}
          style={{ font: "inherit", fontSize: 12, background: "none", border: "none", color: "var(--text-muted)", cursor: "pointer" }}>
          {expanded ? "Hide" : "Show"}
        </button>
        <button onClick={() => void save()} disabled={saving || saved}
          style={{
            font: "inherit", fontSize: 12, fontWeight: 600, cursor: saved ? "default" : "pointer",
            padding: "4px 10px", borderRadius: 6, border: "var(--border-width) solid var(--line)",
            background: saved ? "var(--surface)" : "var(--accent)", color: saved ? "var(--text-muted)" : "var(--bg)",
            opacity: saving ? 0.6 : 1,
          }}>
          {saved ? "✓ Saved" : saving ? "Saving…" : "Save to Library"}
        </button>
      </div>
      {err && <div style={{ padding: "4px 10px", fontSize: 12, color: "var(--danger)" }}>✗ {err}</div>}
      {expanded && (
        blobUrl ? (
        <iframe
          ref={iframeRef}
          title={spark.slug}
          src={blobUrl}
          sandbox="allow-scripts allow-popups allow-forms allow-modals"
          style={{ width: "100%", height: 420, border: "none", background: "#fff", display: "block" }}
        />
        ) : null
      )}
    </div>
  );
}

// LOCAL REASONING MODELS (Mason 08-19): Qwen3-style models emit <think>...
// </think> blocks before their answer. Raw, that reads as the agent dumping its
// inner monologue into chat. Split them out and render each as a collapsed
// "Thoughts" bar (same chrome as a tool card); the visible answer stays
// normal Markdown. An UNCLOSED <think> while streaming shows as a live
// "Thinking..." bar so tokens still visibly arrive.
type TextSeg = { kind: "text" | "think"; text: string; open?: boolean };

function splitThinkSegments(text: string): TextSeg[] {
  const segs: TextSeg[] = [];
  let rest = text;
  for (;;) {
    const s = rest.indexOf("<think>");
    if (s < 0) { if (rest) segs.push({ kind: "text", text: rest }); break; }
    if (s > 0) segs.push({ kind: "text", text: rest.slice(0, s) });
    const after = rest.slice(s + 7);
    const e = after.indexOf("</think>");
    if (e < 0) { segs.push({ kind: "think", text: after, open: true }); break; }
    segs.push({ kind: "think", text: after.slice(0, e) });
    rest = after.slice(e + 8);
  }
  return segs;
}

function ThoughtBar({ text, live }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(false);
  const words = text.trim() ? text.trim().split(/\s+/).length : 0;
  return (
    <div style={{
      fontSize: 12.5, fontFamily: "ui-monospace, SFMono-Regular, monospace",
      border: "var(--border-width) solid var(--line)",
      borderRadius: "var(--radius-control)", color: "var(--text-muted)",
      background: "var(--bg)", overflow: "hidden", margin: "2px 0",
    }}>
      <div
        onClick={() => setOpen((o) => !o)}
        title={open ? "Collapse" : "Expand"}
        style={{ display: "flex", alignItems: "center", gap: 8, padding: "6px 10px", cursor: "pointer", userSelect: "none" }}
      >
        <span style={{ fontSize: 10, opacity: 0.7, transform: open ? "rotate(90deg)" : "none", transition: "transform 120ms" }}>▶</span>
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>
          💭 {live ? "Thinking…" : `Thoughts · ${words} word${words === 1 ? "" : "s"}`}
        </span>
        {live && <span style={{ flexShrink: 0 }}>…</span>}
      </div>
      {open && (
        <div style={{
          borderTop: "var(--border-width) solid var(--line)",
          maxHeight: 300, overflow: "auto", padding: "8px 10px",
          whiteSpace: "pre-wrap", lineHeight: 1.5,
        }}>{text.trim()}</div>
      )}
    </div>
  );
}

function TextWithThoughts({ text, streaming, enabled }: { text: string; streaming?: boolean; enabled?: boolean }) {
  if (!enabled || !text.includes("<think>")) return <Markdown text={text} />;
  const segs = splitThinkSegments(text);
  return (
    <>
      {segs.map((seg, i) =>
        seg.kind === "think"
          ? <ThoughtBar key={i} text={seg.text} live={!!seg.open && !!streaming} />
          : (seg.text.trim() ? <Markdown key={i} text={seg.text.trim()} /> : null),
      )}
    </>
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
