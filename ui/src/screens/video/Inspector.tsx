// AYGENT — VIDEO v0.3 inspector: the selected clip's numbers. Frame-accurate
// fields; multi-select edits the shared props.
import { Send } from "lucide-react";
import { useVideo, patchClip, patchClips, mutate, set } from "./store";
import { useAgentTurn, turnSlotKey } from "../../lib/turns";
import { sendVideoPrompt, APPLY_FEEDBACK_PROMPT } from "./AgentDock";
import { type Clip, TRACK_KIND, fmtTime, clipDur } from "./model";
import { Field, Num, Slider, Check, Seg, Section } from "./Panels";

export function Inspector() {
  const s = useVideo();
  const slotKey = s.agentId ? turnSlotKey(s.agentId, "video") : null;
  const turn = useAgentTurn(slotKey);
  const running = turn.status === "running";
  const openNotes = s.comp.clips.filter((k) => k.type === "review" && !k.hidden && k.text.content.trim());
  const applyFooter = openNotes.length > 0 ? (
    <div className="ve-apply">
      <button className="ve-btn primary" disabled={running} onClick={() => { void sendVideoPrompt(APPLY_FEEDBACK_PROMPT); set({ dockTab: "agent", dockOpen: true }); }}>
        <Send size={13} /> {running ? "Working…" : `Apply Feedback${openNotes.length > 1 ? ` (${openNotes.length})` : ""}`}
      </button>
      <p className="ve-hint">Sends your open R1 notes to the agent to action.</p>
    </div>
  ) : null;
  const sel = s.comp.clips.filter((c) => s.selection.includes(c.id));
  if (!sel.length) {
    return (
      <div className="ve-insp">
        <Section title="Scene">
          <div className="ve-row3">
            <Num label="Width" value={s.comp.scene.width} step={2} min={16} max={8192} onChange={(v) => mutate((c) => { c.scene.width = Math.max(16, Math.round(v / 2) * 2); })} />
            <Num label="Height" value={s.comp.scene.height} step={2} min={16} max={8192} onChange={(v) => mutate((c) => { c.scene.height = Math.max(16, Math.round(v / 2) * 2); })} />
            <Num label="FPS" value={s.comp.scene.fps} step={1} min={1} max={120} onChange={(v) => mutate((c) => { c.scene.fps = v; })} />
          </div>
          <Seg value={`${s.comp.scene.width}x${s.comp.scene.height}`} options={[{ v: "1920x1080", l: "16:9" }, { v: "1080x1920", l: "9:16" }, { v: "1080x1080", l: "1:1" }, { v: "1080x1350", l: "4:5" }]} onChange={(v) => { const [w, h] = v.split("x").map(Number); mutate((c) => { c.scene.width = w; c.scene.height = h; }); }} />
          <Field label="Background"><div style={{ display: "flex", gap: 6 }}><input type="color" value={s.comp.scene.background} onChange={(e) => mutate((c) => { c.scene.background = e.target.value; })} /><input value={s.comp.scene.background} onChange={(e) => mutate((c) => { c.scene.background = e.target.value; })} /></div></Field>
        </Section>
        {applyFooter}
        <p className="ve-hint">Select a clip to edit its timing, transform, text and audio. <kbd className="ve-kbd">⇧</kbd>-click for multi-select, drag on an empty lane for marquee.</p>
      </div>
    );
  }
  const c = sel[0];
  const multi = sel.length > 1;
  const ids = sel.map((k) => k.id);
  const fps = s.comp.scene.fps || 30;
  const asset = s.assets.find((a) => a.id === c.asset);
  const kind = TRACK_KIND(c.track);
  const p = (patch: Partial<Clip>) => (multi ? patchClips(ids, () => patch) : patchClip(c.id, patch));
  const pt = (fn: (k: Clip) => void) => mutate((comp) => { for (const k of comp.clips) if (ids.includes(k.id)) fn(k); });

  return (
    <div className="ve-insp">
      <div className="title">
        <span className="ve-pill">{multi ? `${sel.length} clips` : c.type}</span>
        {!multi && (c.type === "review"
          ? <span className="ve-faint" style={{ fontWeight: 700 }}>Review note · {fmtTime(c.start, fps)} → {fmtTime(c.end, fps)}</span>
          : <input value={c.type === "text" ? c.text.content.split("\n")[0] : c.name} onChange={(e) => (c.type === "text" ? pt((k) => { k.text.content = e.target.value; }) : p({ name: e.target.value }))} />)}
      </div>

      <Section title="Timing">
        <div className="ve-row2">
          <Num label="Start" value={c.start} step={1 / fps} min={0} suffix="s" onChange={(v) => pt((k) => { const d = clipDur(k); k.start = v; k.end = v + d; })} />
          <Num label="End" value={c.end} step={1 / fps} min={c.start + 1 / fps} suffix="s" onChange={(v) => pt((k) => { const dv = v - k.end; k.end = v; if (k.type !== "text" && k.type !== "review") k.out += dv * k.speed; })} />
        </div>
        {(c.type !== "text" && c.type !== "review") && (
          <div className="ve-row2">
            <Num label="Source in" value={c.in} step={1 / fps} min={0} suffix="s" onChange={(v) => pt((k) => { if (k.type === "review") { k.in = 0; k.out = clipDur(k); } else { k.in = v; k.out = v + clipDur(k) * k.speed; } })} />
            <Num label="Speed" value={c.speed} step={0.05} min={0.1} max={8} suffix="×" onChange={(v) => pt((k) => { k.speed = v; k.out = k.in + clipDur(k) * v; })} />
          </div>
        )}
        <div className="ve-row2">
          <Field label="Track"><input value={c.track} onChange={(e) => { const t = e.target.value.toUpperCase().trim(); if (/^[VTAR]\d{1,2}$/.test(t)) p({ track: t }); }} /></Field>
          <Field label="Duration"><input readOnly value={`${fmtTime(clipDur(c), fps)} · ${Math.round(clipDur(c) * fps)} f`} className="ve-mono" /></Field>
        </div>
        <div style={{ display: "flex", gap: 14 }}>
          <Check label="Hidden" checked={c.hidden} onChange={(v) => p({ hidden: v })} />
          {kind !== "text" && <Check label="Muted" checked={c.muted} onChange={(v) => p({ muted: v })} />}
          {kind !== "audio" && kind !== "review" && <Check label="Behind subject" checked={c.behindSubject} onChange={(v) => p({ behindSubject: v })} />}
        </div>
        {asset && !multi && <p className="ve-hint">{asset.name} · {asset.width}×{asset.height} · {asset.fps ? `${Math.round(asset.fps)}fps · ` : ""}{fmtTime(asset.duration, fps)} · source {c.in.toFixed(2)}–{c.out.toFixed(2)}s</p>}
        {c.link && !multi && (() => { const partners = s.comp.clips.filter((k) => k.link === c.link && k.id !== c.id); return (
          <p className="ve-hint">🔗 Linked with {partners.length ? partners.map((k) => `${k.track} · ${k.name || k.id}`).join(", ") : "(missing partner)"}. <button className="ve-btn sm" onClick={() => mutate((comp) => { for (const k of comp.clips) if (k.link === c.link) k.link = ""; })}>Unlink</button></p>
        ); })()}
      </Section>

      {c.type === "review" && !multi && (
        <Section title="Feedback for the agent">
          <textarea rows={4} autoFocus={!c.text.content} placeholder="What should change here? The agent reads this with the timestamps." value={c.text.content} onChange={(e) => pt((k) => { k.text.content = e.target.value; })} />
          <div style={{ display: "flex", gap: 14, alignItems: "center" }}>
            <Check label="Resolved" checked={c.hidden} onChange={(v) => p({ hidden: v })} />
            <span className="ve-faint" style={{ fontSize: 11 }}>{fmtTime(c.start, fps)} → {fmtTime(c.end, fps)}</span>
          </div>
          <p className="ve-hint">Stays pinned to this time range. Hidden = resolved (render + agent skip it; reopen from the R1 lane).</p>
        </Section>
      )}

      {c.type === "text" && !multi && (
        <Section title="Text">
          <textarea rows={3} value={c.text.content} onChange={(e) => pt((k) => { k.text.content = e.target.value; })} />
          <div className="ve-row2">
            <Field label="Font"><input value={c.text.font} onChange={(e) => pt((k) => { k.text.font = e.target.value; })} /></Field>
            <Num label="Weight" value={c.text.weight} step={100} min={100} max={900} onChange={(v) => pt((k) => { k.text.weight = v; })} />
          </div>
          <Slider label="Size" value={c.text.size} min={16} max={300} step={1} fmt={(v) => `${v}px`} onChange={(v) => pt((k) => { k.text.size = v; })} />
          <div className="ve-row2">
            <Slider label="X" value={c.text.x} min={0} max={1} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => pt((k) => { k.text.x = v; })} />
            <Slider label="Y" value={c.text.y} min={0} max={1} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => pt((k) => { k.text.y = v; })} />
          </div>
          <Seg value={c.text.align} options={[{ v: "left", l: "Left" }, { v: "center", l: "Center" }, { v: "right", l: "Right" }]} onChange={(v) => pt((k) => { k.text.align = v; })} />
          <div className="ve-row2">
            <Field label="Color"><div style={{ display: "flex", gap: 6 }}><input type="color" value={c.text.color} onChange={(e) => pt((k) => { k.text.color = e.target.value; })} /><input value={c.text.color} onChange={(e) => pt((k) => { k.text.color = e.target.value; })} /></div></Field>
            <Field label="Box"><div style={{ display: "flex", gap: 6 }}><input type="color" value={c.text.bg || "#000000"} onChange={(e) => pt((k) => { k.text.bg = e.target.value; })} /><button className="ve-btn sm" onClick={() => pt((k) => { k.text.bg = ""; })}>none</button></div></Field>
          </div>
          {c.text.bg && <div className="ve-row2"><Slider label="Box opacity" value={c.text.bgOpacity} min={0} max={1} onChange={(v) => pt((k) => { k.text.bgOpacity = v; })} /><Num label="Padding" value={c.text.padding} min={0} max={200} onChange={(v) => pt((k) => { k.text.padding = v; })} /></div>}
          <Slider label="Wrap width" value={c.text.maxWidth} min={0} max={1} fmt={(v) => (v ? `${Math.round(v * 100)}%` : "off")} onChange={(v) => pt((k) => { k.text.maxWidth = v; })} />
          <div style={{ display: "flex", gap: 14 }}>
            <Check label="Fade in/out" checked={c.text.animation === "fade"} onChange={(v) => pt((k) => { k.text.animation = v ? "fade" : "none"; })} />
            <Check label="Shadow" checked={c.text.shadow} onChange={(v) => pt((k) => { k.text.shadow = v; })} />
          </div>
        </Section>
      )}

      {kind === "video" && c.type !== "text" && (
        <Section title="Transform">
          <Seg value={c.fit} options={[{ v: "cover", l: "Cover" }, { v: "contain", l: "Contain" }, { v: "none", l: "None" }]} onChange={(v) => p({ fit: v })} />
          <div className="ve-row2">
            <Num label="X" value={c.transform.x} step={1} suffix="px" onChange={(v) => pt((k) => { k.transform.x = v; })} />
            <Num label="Y" value={c.transform.y} step={1} suffix="px" onChange={(v) => pt((k) => { k.transform.y = v; })} />
          </div>
          <Slider label="Scale" value={c.transform.scale} min={0.1} max={4} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => pt((k) => { k.transform.scale = v; })} />
          <Slider label="Rotation" value={c.transform.rotation} min={-180} max={180} step={0.5} fmt={(v) => `${v}°`} onChange={(v) => pt((k) => { k.transform.rotation = v; })} />
          <Slider label="Opacity" value={c.transform.opacity} min={0} max={1} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => pt((k) => { k.transform.opacity = v; })} />
          <div className="ve-row2">
            <Num label="Fade in" value={c.transitionIn.duration} step={0.1} min={0} max={5} suffix="s" onChange={(v) => pt((k) => { k.transitionIn = { kind: v > 0 ? "fade" : "", duration: v }; })} />
            <Num label="Fade out" value={c.transitionOut.duration} step={0.1} min={0} max={5} suffix="s" onChange={(v) => pt((k) => { k.transitionOut = { kind: v > 0 ? "fade" : "", duration: v }; })} />
          </div>
          <button className="ve-btn sm" onClick={() => pt((k) => { k.transform = { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1 }; })}>Reset transform</button>
        </Section>
      )}

      {(kind === "audio" || c.type === "video") && (
        <Section title="Audio" right={!multi && <button className="ve-btn sm" onClick={() => set({ kfTrack: s.kfTrack === c.track ? null : c.track })}>Keyframes…</button>}>
          <Slider label="Gain" value={c.volume} min={-40} max={12} step={0.5} fmt={(v) => `${v} dB`} onChange={(v) => p({ volume: v })} />
          <div className="ve-row2">
            <Num label="Fade in" value={c.audio.fadeIn} step={0.1} min={0} max={10} suffix="s" onChange={(v) => pt((k) => { k.audio.fadeIn = v; })} />
            <Num label="Fade out" value={c.audio.fadeOut} step={0.1} min={0} max={10} suffix="s" onChange={(v) => pt((k) => { k.audio.fadeOut = v; })} />
          </div>
          {c.type === "video" && !multi && (
            <Field label="Audio asset (replacement, e.g. enhanced)">
              <select value={c.audioAsset} onChange={(e) => p({ audioAsset: e.target.value })}>
                <option value="">original</option>
                {s.assets.filter((a) => a.hasAudio && a.id !== c.asset).map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
              </select>
            </Field>
          )}
          {c.audio.keyframes.length > 0 && <p className="ve-hint">{c.audio.keyframes.length} volume keyframes (timeline seconds — drag them on the lane, or let the agent set them). <button className="ve-btn sm" onClick={() => pt((k) => { k.audio.keyframes = []; })}>Clear</button></p>}
        </Section>
      )}

      <div style={{ display: "flex", gap: 8 }}>
        <button className="ve-btn sm" onClick={() => set({ panel: "color", panelOpen: true })}>Clip color…</button>
        <span className="ve-faint ve-mono" style={{ fontSize: 10.5, alignSelf: "center", marginLeft: "auto" }}>{multi ? ids.length + " ids" : c.id}</span>
      </div>
      {applyFooter}
    </div>
  );
}
