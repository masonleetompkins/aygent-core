// AYGENT — VIDEO transcript editor (Captions panel).
// Full scrollable transcript: one row per caption line, each word editable with
// its own timecode. Merge lines (combine two sections into one) and split lines
// (set cursor, split into two). Edits persist to transcript.json (per-word times
// + text); section arrangement persists to composition.captions.lines. Caption
// builds map everything through the A-roll cuts.
import { useEffect, useMemo, useRef, useState } from "react";
import { Play, Merge, Scissors, Plus, Trash2, RotateCcw, Pencil } from "lucide-react";
import { useVideo, mutate, runTool, saveTranscript, toast, seek, set } from "./store";
import { type Word, type CaptionLineEdit, fmtTime } from "./model";
import { Section } from "./Panels";

type Line = { key: string; s: number; e: number; words: { w: string; s: number; e: number }[]; custom: boolean };

const tc = (t: number, fps: number) => fmtTime(t, fps).replace(/^00:/, "");

/** Resolve display lines: user sections if any, else auto-group (mirrors backend). */
function resolveLines(words: Word[], sections: CaptionLineEdit[], maxWords: number): Line[] {
  if (!words.length) return [];
  const mw = Math.max(1, Math.min(8, maxWords || 3));
  const groups: Word[][] = [];
  if (sections.length) {
    const secs = [...sections].sort((a, b) => a.s - b.s);
    const containing = (t: number) => secs.findIndex((l) => t >= l.s - 1e-6 && t <= l.e + 0.35);
    let cur: Word[] = []; let curLi: number | null | undefined = undefined;
    let chars = 0;
    for (const w of words) {
      const li = containing(w.s);
      let brk = false;
      if (curLi !== undefined) {
        if (curLi !== li) brk = true;
        else if (li === -1) {
          const gap = cur.length ? w.s - cur[cur.length - 1].e : 0;
          const ends = cur.length ? /[.?!]$/.test(cur[cur.length - 1].w) : false;
          brk = cur.length >= mw || chars + w.w.length + 1 > 24 || gap > 0.7 || ends;
        }
      }
      if (brk) { groups.push(cur); cur = []; chars = 0; }
      chars += w.w.length + 1; cur.push(w); curLi = li;
    }
    if (cur.length) groups.push(cur);
  } else {
    let cur: Word[] = []; let chars = 0;
    for (const w of words) {
      const gap = cur.length ? w.s - cur[cur.length - 1].e : 0;
      const ends = cur.length ? /[.?!]$/.test(cur[cur.length - 1].w) : false;
      if (cur.length && (cur.length >= mw || chars + w.w.length + 1 > 24 || gap > 0.7 || ends)) { groups.push(cur); cur = []; chars = 0; }
      chars += w.w.length + 1; cur.push(w);
    }
    if (cur.length) groups.push(cur);
  }
  return groups.map((g, i) => {
    const s = g[0].s; const e = g[g.length - 1].e;
    return { key: `L${i}-${Math.round(s * 1000)}`, s, e, words: g.map((w) => ({ ...w })), custom: sections.length > 0 };
  });
}

export function TranscriptEditor() {
  const s = useVideo();
  const tx = s.transcript;
  const fps = s.comp.scene.fps || 30;
  const [words, setWords] = useState<Word[]>([]);
  const [dirty, setDirty] = useState(false);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [editWord, setEditWord] = useState<{ li: number; wi: number; txt: string } | null>(null);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [search, setSearch] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const txId = tx ? `${tx.asset}:${tx.words.length}:${tx.created}` : "";

  // reload local copy when the backend transcript changes (transcribe / save / reload)
  useEffect(() => {
    if (tx) { setWords(tx.words.map((w) => ({ ...w }))); setDirty(false); setSel(new Set()); setEditWord(null); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [txId]);

  const sections = s.comp.captions.lines;
  const maxWords = s.comp.graphics.captionMaxWords || 3;
  const lines = useMemo(() => resolveLines(words, sections, maxWords), [words, sections, maxWords]);

  // map line index → word index ranges for selection ops
  const lineWords = useMemo(() => {
    const out: { start: number; end: number }[] = [];
    let k = 0;
    for (const l of lines) { out.push({ start: k, end: k + l.words.length }); k += l.words.length; }
    return out;
  }, [lines]);

  if (!tx) return null;
  const q = search.trim().toLowerCase();
  const visible = q ? lines.map((l, i) => ({ l, i })).filter(({ l }) => l.words.some((w) => w.w.toLowerCase().includes(q))) : lines.map((l, i) => ({ l, i }));

  const markDirty = (w: Word[]) => { setWords(w); setDirty(true); };

  function commitWordEdit() {
    if (!editWord) return;
    const { li, wi, txt } = editWord;
    const t = txt.trim();
    if (!t) { setEditWord(null); return; }
    const r = lineWords[li]; if (!r) { setEditWord(null); return; }
    const gi = r.start + wi;
    markDirty(words.map((w, i) => (i === gi ? { ...w, w: t } : w)));
    setEditWord(null);
  }

  /** Replace the word-level selection with one merged section (one line, words appear at once). */
  function mergeSelected() {
    const idx = [...sel].sort((a, b) => a - b);
    if (idx.length < 2) return;
    // merge = drop internal section boundaries: sections covering the range collapse to one
    const lo = words[idx[0]].s, hi = words[idx[idx.length - 1]].e;
    mutate((c) => {
      const kept = c.captions.lines.filter((l) => l.e < lo - 1e-6 || l.s > hi + 1e-6);
      kept.push({ s: lo, e: hi, words: [] });
      kept.sort((a, b) => a.s - b.s);
      c.captions.lines = kept;
    });
    setSel(new Set());
    toast("merged into one section", "ok");
  }

  /** Split line li after word wi (cursor between words): two sections. */
  function splitLine(li: number, wi: number) {
    const l = lines[li]; if (!l || wi < 0 || wi >= l.words.length - 1) return;
    const a = l.words[wi], b = l.words[wi + 1];
    const cut = (a.e + b.s) / 2;
    mutate((c) => {
      const kept = c.captions.lines.filter((x) => !(x.s <= l.s + 0.01 && x.e >= l.e - 0.01));
      kept.push({ s: l.s, e: cut, words: [] }, { s: cut, e: l.e, words: [] });
      kept.sort((x, y) => x.s - y.s);
      c.captions.lines = kept;
    });
  }

  /** Drop all section arrangement → back to auto-group. */
  function resetSections() { mutate((c) => { c.captions.lines = []; }); setSel(new Set()); }

  /** Persist word edits to transcript.json (validates + renormalizes server-side). */
  async function save() {
    if (!tx) return;
    const ok = await saveTranscript({ asset: tx.asset, language: tx.language, words });
    if (ok) setDirty(false);
  }

  function playLine(l: Line) { seek(l.s); set({ playing: true }); }

  return (
    <Section title="Transcript editor" right={<span className="ve-pill">{words.length} words · {lines.length} lines</span>}>
      <div style={{ display: "flex", gap: 6 }}>
        <input value={search} placeholder="find a word…" onChange={(e) => setSearch(e.target.value)} style={{ flex: 1 }} />
        {dirty && <button className="ve-btn sm primary" onClick={() => void save()}>Save words</button>}
      </div>
      {dirty && <p className="ve-hint" style={{ color: "var(--warn)" }}>Word edits unsaved — Save writes transcript.json (takes, auto-cut + caption builds use it).</p>}
      <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
        <button className="ve-btn sm" disabled={sel.size < 2} onClick={mergeSelected} title="Combine the selected words' lines into one section"><Merge size={12} /> Merge ({sel.size})</button>
        <button className="ve-btn sm" disabled={!sections.length} onClick={resetSections} title="Drop all sections, back to auto-group"><RotateCcw size={12} /> Auto</button>
      </div>
      <div ref={listRef} className="aygent-scroll" style={{ display: "flex", flexDirection: "column", gap: 4, maxHeight: 320, overflowY: "auto", paddingRight: 2 }}>
        {visible.map(({ l, i: li }) => {
          const r = lineWords[li];
          const allSel = r && Array.from({ length: r.end - r.start }, (_, k) => r.start + k).every((k) => sel.has(k));
          const open = expanded.has(li);
          return (
            <div key={l.key} style={{ border: "1px solid rgba(var(--accent-rgb),.12)", borderRadius: 8, padding: "6px 8px", background: allSel ? "rgba(var(--accent-rgb),.07)" : undefined }}>
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <input type="checkbox" checked={!!allSel} title="Select line" onChange={() => {
                  const n = new Set(sel);
                  const idxs = Array.from({ length: r.end - r.start }, (_, k) => r.start + k);
                  if (allSel) idxs.forEach((k) => n.delete(k)); else idxs.forEach((k) => n.add(k));
                  setSel(n);
                }} />
                <button className="ve-mono" style={{ fontSize: 10.5, color: "var(--accent)", fontWeight: 700 }} title="Play from here" onClick={() => playLine(l)}>{tc(l.s, fps)}</button>
                <span className="ve-faint ve-mono" style={{ fontSize: 10 }}>→{tc(l.e, fps)}</span>
                <span className="spacer" style={{ flex: 1 }} />
                <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="Play line" onClick={() => playLine(l)}><Play size={11} /></button>
                <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title={open ? "Hide word times" : "Show per-word times"} onClick={() => { const n = new Set(expanded); if (open) n.delete(li); else n.add(li); setExpanded(n); }}><Pencil size={11} /></button>
              </div>
              <div style={{ fontSize: 12.5, lineHeight: 1.5, marginTop: 3, userSelect: "text" }}>
                {l.words.map((w, wi) => {
                  const gi = r.start + wi;
                  const isSel = sel.has(gi);
                  const editing = editWord && editWord.li === li && editWord.wi === wi;
                  return (
                    <span key={wi}>
                      {editing ? (
                        <input autoFocus value={editWord.txt} onChange={(e) => setEditWord({ ...editWord, txt: e.target.value })}
                          onBlur={commitWordEdit} onKeyDown={(e) => { if (e.key === "Enter") commitWordEdit(); if (e.key === "Escape") setEditWord(null); }}
                          style={{ width: `${Math.max(3, editWord.txt.length + 1)}ch`, fontSize: 12.5, padding: "0 3px" }} />
                      ) : (
                        <span onDoubleClick={() => setEditWord({ li, wi, txt: w.w })} title="Double-click to edit"
                          onClick={(e) => { if (e.shiftKey || e.metaKey) { const n = new Set(sel); if (n.has(gi)) n.delete(gi); else n.add(gi); setSel(n); } }}
                          style={{ cursor: "text", borderRadius: 3, padding: "0 1px", background: isSel ? "rgba(var(--accent-rgb),.25)" : undefined }}>{w.w}</span>
                      )}
                      {wi < l.words.length - 1 && (
                        <button title="Split line here" onClick={() => splitLine(li, wi)}
                          style={{ display: "inline-flex", width: 14, height: 14, alignItems: "center", justifyContent: "center", color: "var(--text-faint)", verticalAlign: 1 }}><Scissors size={9} /></button>
                      )}
                      {wi < l.words.length - 1 && " "}
                    </span>
                  );
                })}
              </div>
              {open && (
                <div style={{ marginTop: 4, display: "flex", flexDirection: "column", gap: 2 }}>
                  {l.words.map((w, wi) => {
                    const gi = r.start + wi;
                    return (
                      <div key={wi} style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 11 }}>
                        <input value={w.w} onChange={(e) => markDirty(words.map((x, i) => (i === gi ? { ...x, w: e.target.value } : x)))} style={{ flex: 1, fontSize: 11, padding: "2px 6px" }} />
                        <input className="ve-mono" value={w.s.toFixed(2)} title="Word start (s)" onChange={(e) => { const v = Number(e.target.value); if (isFinite(v)) markDirty(words.map((x, i) => (i === gi ? { ...x, s: Math.max(0, v) } : x))); }} style={{ width: 64, fontSize: 10.5, padding: "2px 4px" }} />
                        <input className="ve-mono" value={w.e.toFixed(2)} title="Word end (s)" onChange={(e) => { const v = Number(e.target.value); if (isFinite(v)) markDirty(words.map((x, i) => (i === gi ? { ...x, e: v } : x))); }} style={{ width: 64, fontSize: 10.5, padding: "2px 4px" }} />
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
        {visible.length === 0 && <p className="ve-hint">No lines match.</p>}
      </div>
      <p className="ve-hint">Double-click a word to fix it · <b>✂</b> between words splits the line · checkbox-select 2+ lines then <b>Merge</b> combines · pencil shows per-word times · click a timecode to play from there.</p>
      <TranscriptAdd words={words} onAdd={(w) => markDirty([...words, w])} />
      <TranscriptDelete sel={sel} onDelete={(idxs) => { markDirty(words.filter((_, i) => !idxs.has(i))); setSel(new Set()); }} />
    </Section>
  );
}

function TranscriptAdd({ words, onAdd }: { words: Word[]; onAdd: (w: Word) => void }) {
  const [open, setOpen] = useState(false);
  const [txt, setTxt] = useState("");
  const [at, setAt] = useState("");
  if (!open) return <button className="ve-btn sm" onClick={() => setOpen(true)}><Plus size={12} /> Add word</button>;
  return (
    <div style={{ display: "flex", gap: 6 }}>
      <input value={txt} placeholder="word" onChange={(e) => setTxt(e.target.value)} style={{ flex: 1 }} />
      <input value={at} placeholder="at (s)" title="Insert at source seconds" onChange={(e) => setAt(e.target.value)} style={{ width: 76 }} />
      <button className="ve-btn sm primary" onClick={() => {
        const t = txt.trim(); if (!t) return;
        const v = Number(at);
        const s = isFinite(v) && v >= 0 ? v : (words.length ? words[words.length - 1].e + 0.05 : 0);
        onAdd({ w: t, s, e: s + 0.3 }); setTxt(""); setAt(""); setOpen(false);
      }}>Add</button>
      <button className="ve-btn sm" onClick={() => setOpen(false)}>Cancel</button>
    </div>
  );
}

function TranscriptDelete({ sel, onDelete }: { sel: Set<number>; onDelete: (idxs: Set<number>) => void }) {
  if (!sel.size) return null;
  return (
    <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
      <span className="ve-faint" style={{ fontSize: 11 }}>{sel.size} word{sel.size === 1 ? "" : "s"} selected</span>
      <button className="ve-btn sm" onClick={() => { if (confirm(`Delete ${sel.size} word${sel.size === 1 ? "" : "s"} from the transcript?`)) onDelete(sel); }}><Trash2 size={12} /> Delete</button>
    </div>
  );
}
