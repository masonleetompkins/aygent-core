// AYGENT — VIDEO v0.3 editor store. One external store (like lib/turns.ts) so
// the player, timeline, inspector, panels and agent dock all share the same
// composition + selection + playhead without prop-drilling, and so a headless
// agent turn (video-project-changed event) can reload the edit while any panel
// is mounted. Undo/redo is a plain snapshot stack over the composition.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { type Asset, type Clip, type Composition, type Transcript, blankComposition, normalize, duration, clipDur, uid } from "./model";

export type Status = { ffmpeg: string | null; hyperframes: boolean; whisper: boolean; uv: boolean; proMode: boolean };
export type Project = { name: string; modified: number; assets: number };
export type Panel = "media" | "graphics" | "captions" | "color" | "audio" | "export";

export type State = {
  agentId: string | null;
  folder: string | null;
  status: Status | null;
  projects: Project[];
  project: string | null;
  comp: Composition;
  assets: Asset[];
  transcript: Transcript | null;
  dirty: boolean;
  saving: boolean;
  playhead: number;
  playing: boolean;
  loop: boolean;
  zoom: number;            // px per second
  scrollX: number;
  selection: string[];     // clip ids
  tool: "select" | "razor";
  snap: boolean;
  panel: Panel;
  dockOpen: boolean;
  dockTab: "agent" | "inspector";
  panelOpen: boolean;
  dockW: number;           // px, user-resizable (persisted)
  toast: { text: string; tone: "ok" | "err" | "info"; at: number } | null;
  render: { pct: number; running: boolean; path?: string; error?: string } | null;
  toolProgress: string | null;
  frame: { path: string; time: number; at: number } | null;
  captionLines: { s: number; e: number; words: { w: string; s: number; e: number }[] }[];
  luts: string[];
  renders: { path: string; bytes: number; modified: number }[];
  cacheBust: number;
};

let state: State = {
  agentId: null, folder: null, status: null, projects: [], project: null, comp: blankComposition(), assets: [], transcript: null,
  dirty: false, saving: false, playhead: 0, playing: false, loop: false, zoom: 60, scrollX: 0, selection: [], tool: "select", snap: true,
  panel: "media", dockOpen: true, dockTab: "agent", panelOpen: true, dockW: Math.max(280, Math.min(640, Number(localStorage.getItem("aygent.video.dockW")) || 360)), toast: null, render: null, toolProgress: null, frame: null, captionLines: [], luts: [], renders: [], cacheBust: 0,
};

const listeners = new Set<() => void>();
function emit() { listeners.forEach((l) => l()); }
function subscribe(l: () => void) { listeners.add(l); return () => { listeners.delete(l); }; }
export function useVideo(): State { return useSyncExternalStore(subscribe, () => state); }
export function useVideoSel<T>(sel: (s: State) => T): T { return useSyncExternalStore(subscribe, () => sel(state)); }
export function get(): State { return state; }
export function set(patch: Partial<State> | ((s: State) => Partial<State>)) {
  const p = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...p };
  emit();
  if ("playhead" in p) emitPh();
}

// ---- playhead channel ------------------------------------------------------
// Playback advances the playhead ~60×/s. Waking every store subscriber at that
// rate (timeline, inspector, dock…) is what made playback stutter, so the
// transport tick publishes on this narrow channel instead; only the canvas,
// timecode and playhead lines subscribe. Seeks/pauses still go through set()
// so the rest of the UI catches up at interaction boundaries.
const phListeners = new Set<() => void>();
function emitPh() { phListeners.forEach((l) => l()); }
function subscribePh(l: () => void) { phListeners.add(l); return () => { phListeners.delete(l); }; }
export function usePlayhead(): number { return useSyncExternalStore(subscribePh, () => state.playhead); }
export function tickPlayhead(t: number) { state = { ...state, playhead: t }; emitPh(); }

// ---- undo/redo ------------------------------------------------------------
const past: Composition[] = [];
const future: Composition[] = [];
const MAX = 120;
function snapshot() { past.push(state.comp); if (past.length > MAX) past.shift(); future.length = 0; }
export function undo() { const c = past.pop(); if (!c) return; future.push(state.comp); set({ comp: c, dirty: true }); scheduleSave(); }
export function redo() { const c = future.pop(); if (!c) return; past.push(state.comp); set({ comp: c, dirty: true }); scheduleSave(); }
export const canUndo = () => past.length > 0;
export const canRedo = () => future.length > 0;

/** Mutate the composition (records undo, marks dirty, autosaves). */
export function mutate(fn: (c: Composition) => Composition | void, opts: { undo?: boolean } = {}) {
  if (opts.undo !== false) snapshot();
  const draft = JSON.parse(JSON.stringify(state.comp)) as Composition;
  const out = fn(draft) ?? draft;
  set({ comp: out, dirty: true });
  scheduleSave();
}
export function patchClip(id: string, patch: Partial<Clip>, opts?: { undo?: boolean }) {
  mutate((c) => { c.clips = c.clips.map((k) => (k.id === id ? { ...k, ...patch } : k)); }, opts);
}
export function patchClips(ids: string[], fn: (k: Clip) => Partial<Clip>, opts?: { undo?: boolean }) {
  mutate((c) => { c.clips = c.clips.map((k) => (ids.includes(k.id) ? { ...k, ...fn(k) } : k)); }, opts);
}

// A drag is many patches but ONE undo step: call beginGesture once, then patch with undo:false.
export function beginGesture() { snapshot(); }

// ---- persistence ----------------------------------------------------------
let saveTimer: number | null = null;
export function scheduleSave(delay = 600) {
  if (saveTimer) window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => { saveTimer = null; void save(); }, delay);
}
export async function save(): Promise<boolean> {
  const { agentId, project, comp } = state;
  if (!agentId || !project) return false;
  set({ saving: true });
  try {
    await invoke("video_save", { agentId, project, composition: comp });
    set({ dirty: false, saving: false });
    void refreshCaptionLines();
    return true;
  } catch (e) { set({ saving: false }); toast(`save failed: ${String(e)}`, "err"); return false; }
}
export async function flushSave() { if (saveTimer) { window.clearTimeout(saveTimer); saveTimer = null; } if (state.dirty) await save(); }

export function toast(text: string, tone: "ok" | "err" | "info" = "info") {
  set({ toast: { text, tone, at: Date.now() } });
  const at = state.toast!.at;
  window.setTimeout(() => { if (state.toast?.at === at) set({ toast: null }); }, tone === "err" ? 7000 : 3200);
}

export async function init(agentId: string | null, folder: string | null) {
  const changed = agentId !== state.agentId;
  set({ agentId, folder });
  try { set({ status: await invoke<Status>("video_status", { folder }) }); } catch { /* keep */ }
  if (!agentId) { set({ projects: [], project: null, comp: blankComposition(), assets: [] }); return; }
  await refreshProjects();
  if (changed || !state.project) {
    const last = localStorage.getItem(`aygent.video.last.${agentId}`);
    const pick = state.projects.find((p) => p.name === last)?.name ?? state.projects[0]?.name ?? null;
    if (pick) await open(pick); else set({ project: null, comp: blankComposition(), assets: [], transcript: null, selection: [] });
  }
}

export async function refreshProjects() {
  const { agentId } = state; if (!agentId) return;
  try { set({ projects: await invoke<Project[]>("video_projects", { agentId }) }); } catch (e) { toast(String(e), "err"); }
}

export async function open(name: string) {
  const { agentId } = state; if (!agentId) return;
  await flushSave();
  try {
    const r = await invoke<{ project: string; composition: unknown; assets: Asset[]; transcript: Transcript | null; chat: unknown }>("video_load", { agentId, project: name });
    past.length = 0; future.length = 0;
    const assets = Array.isArray(r.assets) ? r.assets : [];
    set({ project: r.project, comp: normalize(r.composition, assets), assets, transcript: r.transcript ?? null, dirty: false, playhead: 0, playing: false, selection: [], render: null, frame: null, cacheBust: Date.now() });
    localStorage.setItem(`aygent.video.last.${agentId}`, r.project);
    chatLoaded(r.chat);
    void refreshLuts(); void refreshRenders(); void refreshCaptionLines();
    // repair any thumbs missing after an interrupted import
    if (assets.some((a) => !a.thumbs)) void refreshThumbs();
  } catch (e) { toast(String(e), "err"); }
}

export async function create(name: string, size?: { w: number; h: number }) {
  const { agentId } = state; if (!agentId) return;
  const comp = blankComposition();
  if (size) { comp.scene.width = size.w; comp.scene.height = size.h; }
  try {
    await invoke("video_create", { agentId, project: name, composition: comp });
    await refreshProjects();
    await open(name);
    toast(`created ${name}`, "ok");
  } catch (e) { toast(String(e), "err"); }
}

/** Reload from disk (after an agent tool ran) WITHOUT losing UI state. */
export async function reload() {
  const { agentId, project } = state; if (!agentId || !project) return;
  try {
    const r = await invoke<{ composition: unknown; assets: Asset[]; transcript: Transcript | null }>("video_load", { agentId, project });
    const assets = Array.isArray(r.assets) ? r.assets : [];
    snapshot();
    set({ comp: normalize(r.composition, assets), assets, transcript: r.transcript ?? null, dirty: false, cacheBust: Date.now() });
    void refreshLuts(); void refreshRenders(); void refreshCaptionLines();
  } catch (e) { toast(String(e), "err"); }
}

export async function importPick() {
  const { agentId, project } = state; if (!agentId || !project) { toast("open a project first"); return; }
  try {
    const r = await invoke<{ assets: any[]; errors: { path: string; error: string }[] }>("video_pick_media", { agentId, project });
    afterImport(r);
  } catch (e) { toast(String(e), "err"); }
}
export async function importPaths(paths: string[]) {
  const { agentId, project } = state; if (!agentId || !project) { toast("open a project first"); return; }
  toast(`linking ${paths.length} file${paths.length === 1 ? "" : "s"}…`);
  try {
    const r = await invoke<{ assets: any[]; errors: { path: string; error: string }[] }>("video_import_paths", { agentId, project, paths });
    afterImport(r);
  } catch (e) { toast(String(e), "err"); }
}
function afterImport(r: { assets: any[]; errors: { path: string; error: string }[] }) {
  const media = r.assets.filter((a) => a && a.id);
  const luts = r.assets.filter((a) => a && a.lut);
  if (media.length) {
    const ids = new Set(media.map((a) => a.id));
    set((s) => ({ assets: [...s.assets.filter((a) => !ids.has(a.id)), ...media], cacheBust: Date.now() }));
    // First video into an empty timeline → lay it on V1 automatically.
    if (state.comp.clips.length === 0) {
      const a = media.find((x) => x.kind === "video") ?? media[0];
      if (a) addAssetToTimeline(a, 0);
    }
    toast(`linked ${media.length} file${media.length === 1 ? "" : "s"}`, "ok");
  }
  if (luts.length) { void refreshLuts(); toast(`LUT added: ${luts[0].lut}`, "ok"); }
  for (const e of r.errors ?? []) toast(`${e.path.split("/").pop()}: ${e.error}`, "err");
}

export async function refreshThumbs() {
  const { agentId, project } = state; if (!agentId || !project) return;
  try { const assets = await invoke<Asset[]>("video_refresh_thumbs", { agentId, project }); set({ assets, cacheBust: Date.now() }); } catch { /* ignore */ }
}
export async function relinkAsset(id: string) {
  const { agentId, project } = state; if (!agentId || !project) return;
  try {
    const r = await invoke<{ relinked: boolean; linked?: boolean }>("video_relink_asset", { agentId, project, assetId: id });
    if (r.relinked) { toast(r.linked ? "relinked (hardlinked)" : "relinked (reference)", "ok"); await reload(); }
  } catch (e) { toast(String(e), "err"); }
}
export async function removeAsset(id: string) {
  const { agentId, project } = state; if (!agentId || !project) return;
  try {
    await invoke("video_remove_asset", { agentId, project, assetId: id });
    set((s) => ({ assets: s.assets.filter((a) => a.id !== id) }));
    mutate((c) => { c.clips = c.clips.filter((k) => k.asset !== id); });
  } catch (e) { toast(String(e), "err"); }
}
export async function refreshLuts() {
  const { agentId, project } = state; if (!agentId || !project) return;
  try { set({ luts: await invoke<string[]>("video_list_luts", { agentId, project }) }); } catch { /* ignore */ }
}
export async function pickLut(): Promise<string | null> {
  const { agentId, project } = state; if (!agentId || !project) return null;
  try { const r = await invoke<string | null>("video_pick_lut", { agentId, project }); await refreshLuts(); return r; } catch (e) { toast(String(e), "err"); return null; }
}
export async function refreshRenders() {
  const { agentId, project } = state; if (!agentId || !project) return;
  try { set({ renders: await invoke<State["renders"]>("video_list_renders", { agentId, project }) }); } catch { /* ignore */ }
}
export async function refreshCaptionLines() {
  // Removed: the old ASS/DOM caption pipeline is gone. Hyperframes renders all captions + graphics.
  if (state.captionLines.length) set({ captionLines: [] });
}

/** Run a video_* tool from the UI (same core as the agent). */
export async function runTool<T = any>(name: string, input: Record<string, unknown>, label?: string): Promise<T | null> {
  const { agentId, project } = state; if (!agentId || !project) { toast("open a project first"); return null; }
  await flushSave();
  set({ toolProgress: label ?? name.replace("video_", "").replace("_", " ") + "…" });
  try {
    const r = await invoke<T>("video_tool", { agentId, name, input: { project, ...input } });
    set({ toolProgress: null });
    await reload();
    return r;
  } catch (e) { set({ toolProgress: null }); toast(String(e), "err"); return null; }
}

export async function renderFrame(time = state.playhead) {
  const { agentId, project, comp } = state; if (!agentId || !project) return;
  set({ toolProgress: "rendering frame…" });
  try {
    const path = await invoke<string>("video_frame", { agentId, project, time, composition: comp });
    set({ frame: { path, time, at: Date.now() }, toolProgress: null });
  } catch (e) { set({ toolProgress: null }); toast(String(e), "err"); }
}

export async function startRender(preset: Composition["exports"][number]) {
  const { agentId, project } = state; if (!agentId || !project) return;
  await flushSave();
  set({ render: { pct: 0, running: true } });
  try {
    const r = await invoke<{ path: string; bytes: number }>("video_render", { agentId, project, preset });
    set({ render: { pct: 100, running: false, path: r.path } });
    toast(`exported ${r.path.split("/").pop()}`, "ok");
    void refreshRenders();
  } catch (e) { set({ render: { pct: 0, running: false, error: String(e) } }); toast(String(e), "err"); }
}
export async function cancelRender() { const { agentId, project } = state; if (agentId && project) await invoke("video_render_cancel", { agentId, project }); }
export async function reveal(tail?: string) { const { agentId, project } = state; if (agentId && project) { try { await invoke("video_reveal", { agentId, project, tail }); } catch (e) { toast(String(e), "err"); } } }

// ---- timeline helpers -------------------------------------------------------
export function addAssetToTimeline(a: Asset, at?: number, track?: string) {
  const t = track ?? (a.kind === "audio" ? (state.comp.clips.some((c) => c.track === "A1") ? "A2" : "A1") : state.comp.clips.some((c) => c.track === "V1" && c.type === "video") && state.comp.clips.length ? "V2" : "V1");
  const start = at ?? duration(state.comp);
  const d = a.kind === "image" ? 5 : Math.max(0.04, a.duration);
  const clip: Clip = { ...(newClipFor(a)), track: t, start, end: start + d, in: 0, out: d };
  const extra: Clip[] = [];
  // Video with an audio stream lays a LINKED audio clip on A1 so the timeline
  // shows the waveform under the picture (same asset, same timing, link id shared).
  if (a.kind === "video" && a.hasAudio && (t === "V1" || t === "V2")) {
    const link = uid("l");
    clip.link = link;
    const ac: Clip = { ...(newClipFor(a)), type: "audio", track: "A1", name: `${a.name} · audio`, start, end: start + d, in: 0, out: d, link };
    extra.push(ac);
  }
  mutate((c) => { c.clips.push(clip); for (const x of extra) c.clips.push(x); });
  set({ selection: [clip.id, ...extra.map((x) => x.id)] });
  return clip;
}
function newClipFor(a: Asset): Clip {
  const base = normalize({ clips: [{ id: uid(), track: "V1", type: a.kind, asset: a.id, name: a.name }] }).clips[0];
  return base;
}
/** Expand ids to include linked partners (V+A pairs move/split/delete as one). */
export function withLinked(ids: string[]): string[] {
  const links = new Set(state.comp.clips.filter((c) => ids.includes(c.id) && c.link).map((c) => c.link));
  if (!links.size) return ids;
  const out = new Set(ids);
  for (const c of state.comp.clips) if (c.link && links.has(c.link)) out.add(c.id);
  return [...out];
}
export function splitAt(time: number, ids?: string[]) {
  const targets = withLinked(ids ?? (state.selection.length ? state.selection : state.comp.clips.filter((c) => time > c.start + 0.02 && time < c.end - 0.02).map((c) => c.id)));
  mutate((c) => {
    const out: Clip[] = [];
    for (const k of c.clips) {
      if (!targets.includes(k.id) || time <= k.start + 0.02 || time >= k.end - 0.02) { out.push(k); continue; }
      const srcAt = k.in + (time - k.start) * k.speed;
      const nl = k.link ? uid("l") : "";
      out.push({ ...k, end: time, out: srcAt, transitionOut: { kind: "", duration: 0 }, link: nl });
      out.push({ ...k, id: uid(), start: time, in: srcAt, transitionIn: { kind: "", duration: 0 }, link: nl });
    }
    c.clips = out;
  });
}
export function deleteSelected(ripple = false) {
  const ids = withLinked(state.selection); if (!ids.length) return;
  mutate((c) => {
    const removed = c.clips.filter((k) => ids.includes(k.id));
    c.clips = c.clips.filter((k) => !ids.includes(k.id));
    if (ripple) for (const r of removed.sort((a, b) => b.start - a.start)) {
      const d = clipDur(r);
      for (const k of c.clips) if (k.track === r.track && k.start >= r.end - 1e-6) { k.start -= d; k.end -= d; }
    }
  });
  set({ selection: [] });
}
export function duplicateSelected() {
  const ids = withLinked(state.selection); if (!ids.length) return;
  const made: string[] = [];
  mutate((c) => {
    for (const k of c.clips.filter((x) => ids.includes(x.id))) { const d = clipDur(k); const n = { ...k, id: uid(), start: k.end, end: k.end + d }; c.clips.push(n); made.push(n.id); }
  });
  set({ selection: made });
}
export function closeGaps(track?: string) {
  mutate((c) => {
    const tracks = track ? [track] : Array.from(new Set(c.clips.map((k) => k.track)));
    for (const t of tracks) {
      const ks = c.clips.filter((k) => k.track === t).sort((a, b) => a.start - b.start);
      let cursor = 0;
      for (const k of ks) { const d = clipDur(k); k.start = cursor; k.end = cursor + d; cursor += d; }
    }
  });
}

// ---- transport -------------------------------------------------------------
export function seek(t: number) { set({ playhead: Math.max(0, Math.min(t, Math.max(duration(state.comp), 0.001))) }); }
export function togglePlay() { set((s) => ({ playing: !s.playing })); }
export function stepFrames(n: number) { const fps = state.comp.scene.fps || 30; set({ playing: false }); seek(Math.round(state.playhead * fps + n) / fps); }

// ---- agent dock chat (persisted in Video/<project>/chat.json) ---------------
export type ChatMsg = { role: "user" | "assistant"; text: string; at: number; tools?: { name: string; summary?: string; ok?: boolean; detail?: string }[] };
let chat: ChatMsg[] = [];
let chatHistory: unknown[] = [];
const chatListeners = new Set<() => void>();
export function useChat(): ChatMsg[] { return useSyncExternalStore((l) => { chatListeners.add(l); return () => { chatListeners.delete(l); }; }, () => chat); }
function chatLoaded(raw: unknown) {
  const r = (raw ?? {}) as { msgs?: ChatMsg[]; history?: unknown[] };
  chat = Array.isArray(r.msgs) ? r.msgs : [];
  chatHistory = Array.isArray(r.history) ? r.history : [];
  chatListeners.forEach((l) => l());
}
export function chatGetHistory() { return chatHistory; }
export function chatPush(m: ChatMsg) { chat = [...chat, m]; chatListeners.forEach((l) => l()); }
export function chatReplaceLast(m: ChatMsg) { chat = [...chat.slice(0, -1), m]; chatListeners.forEach((l) => l()); }
export async function chatPersist(history: unknown[]) {
  chatHistory = history;
  const { agentId, project } = state; if (!agentId || !project) return;
  try { await invoke("video_chat_save", { agentId, project, chat: { msgs: chat.slice(-200), history: history.slice(-60) } }); } catch { /* non-fatal */ }
}
export function chatClear() { chat = []; chatHistory = []; chatListeners.forEach((l) => l()); void chatPersist([]); }
