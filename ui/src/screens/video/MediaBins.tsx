// AYGENT — VIDEO media panel: grid + Premiere-style NESTED bins list.
// Bins nest (parent id), import folders as bin trees, batch import shows a
// progress bar driven by video-import-progress events, and both assets and
// bins drag between bins (separate drag mimes so headers stay drop targets).
import { useState } from "react";
import { Upload, X, FolderOpen, Folder, FolderPlus, Music, Film, Image as ImageIcon, RefreshCw, Link2, Unplug, ChevronRight, ChevronDown, LayoutGrid, List, Pencil, Trash2, FolderInput } from "lucide-react";
import { useVideo, importPick, importFolder, removeAsset, relinkAsset, reload, addAssetToTimeline, refreshThumbs, createMediaFolder, renameMediaFolder, deleteMediaFolder, moveMediaAssets, moveMediaFolder, setDropBin } from "./store";
import { type Asset, type MediaFolder, fmtDur, fmtBytes, mediaUrl } from "./model";
import { Section } from "./Panels";

type MediaView = "grid" | "list";
export const isGeneratedAsset = (a: Asset) =>
  a.name.includes("(alpha)") || a.name.includes("(enhanced)") || a.name.includes("(cleaned)") || a.name.includes("(HF)") || a.name.includes("(overlay)");

export function MediaPanel() {
  const s = useVideo();
  const [sel, setSel] = useState<string | null>(null);
  const [view, setView] = useState<MediaView>(() => {
    try { return localStorage.getItem("aygent.video.mediaView") === "list" ? "list" : "grid"; } catch { return "grid"; }
  });
  const files = s.assets.filter((a) => !isGeneratedAsset(a));
  const generated = s.assets.filter(isGeneratedAsset);
  const offline = s.assets.filter((a) => a.online === false);
  const prog = s.importProgress;
  const pct = prog && prog.total > 0 ? Math.round((prog.done / prog.total) * 100) : 0;
  function pickView(v: MediaView) { setView(v); try { localStorage.setItem("aygent.video.mediaView", v); } catch { /* ignore */ } }
  const volumeOf = (p: string) => { const m = /^\/Volumes\/([^/]+)/.exec(p); return m ? m[1] : "Macintosh HD"; };
  return (
    <>
      <div className="ve-panel-head"><span className="ve-title">Media</span><span className="spacer" />
        <button className={`ve-icon-btn ${view === "grid" ? "on" : ""}`} title="Grid view" onClick={() => pickView("grid")}><LayoutGrid size={14} /></button>
        <button className={`ve-icon-btn ${view === "list" ? "on" : ""}`} title="List view with bins" onClick={() => pickView("list")}><List size={14} /></button>
        <button className="ve-icon-btn" title="Rebuild thumbnails" onClick={() => void refreshThumbs()}><RefreshCw size={14} /></button>
        <ImportButton />
      </div>
      <div className="ve-panel-body">
        <MediaDropTarget onPick={() => void importPick()} />
        {prog && (
          <div className="ve-importprog" role="status" aria-label="Import progress">
            <div className="ve-progress"><div className="bar"><i style={{ width: `${pct}%` }} /></div><span className="ve-mono">{prog.total > 0 ? `${prog.done}/${prog.total}` : "…"}</span></div>
            <div className="nm">{prog.total > 0 ? (prog.name || "finishing…") : "scanning…"}</div>
          </div>
        )}
        {offline.length > 0 && (
          <div className="ve-offline">
            <div className="hd"><Unplug size={14} /> {offline.length === 1 ? "1 file is offline" : `${offline.length} files are offline`}</div>
            {offline.map((a) => (
              <div key={a.id} className="row">
                <div className="nm">{a.name}</div>
                <div className="pth" title={a.path}>{a.path}</div>
                <div className="acts">
                  <span className="ve-faint">{a.linked ? "hardlink missing" : `plug in ${volumeOf(a.path)}`}</span>
                  <button className="ve-btn sm" onClick={() => void relinkAsset(a.id)}><Link2 size={12} /> Relink…</button>
                </div>
              </div>
            ))}
            <button className="ve-btn sm" style={{ alignSelf: "flex-start" }} onClick={() => void reload()}><RefreshCw size={12} /> Check again</button>
          </div>
        )}
        {view === "grid" ? (
          <>
            {files.length > 0 && <AssetGrid assets={files} sel={sel} setSel={setSel} />}
            {generated.length > 0 && <Section title="Generated"><AssetGrid assets={generated} sel={sel} setSel={setSel} /></Section>}
          </>
        ) : (
          <AssetList files={files} generated={generated} sel={sel} setSel={setSel} />
        )}
        <p className="ve-hint">Drag a clip onto a lane, or double-click to append at the end. <kbd className="ve-kbd">⌫</kbd> on a card unlinks it.</p>
      </div>
    </>
  );
}
/** Single Import button: files AND folders. Click opens a menu — the native
 *  file picker only selects files, the folder picker only folders, so one
 *  button offers both (no separate cluttering Folder button in the header). */
function ImportButton() {
  const [menu, setMenu] = useState(false);
  return (
    <div style={{ position: "relative", flexShrink: 0 }}>
      <button className="ve-btn sm primary" onClick={() => setMenu((v) => !v)} title="Import files or a whole folder as bins"><Upload size={13} /> Import</button>
      {menu && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 40 }} onClick={() => setMenu(false)} />
          <div className="ve-menu" role="menu">
            <button role="menuitem" onClick={() => { setMenu(false); void importPick(); }}>Files&hellip;</button>
            <button role="menuitem" onClick={() => { setMenu(false); void importFolder(); }}>Folder as bins&hellip;</button>
          </div>
        </>
      )}
    </div>
  );
}
/** The big drop zone. HTML5 dragover only PAINTS + records the hover target
 *  (setDropBin) — the native Tauri drop event in Editor.tsx carries the real
 *  OS paths and performs the import. preventDefault on dragover is what lets
 *  the drop land instead of navigating. */
function MediaDropTarget({ onPick }: { onPick: () => void }) {
  const [over, setOver] = useState(false);
  return (
    <div className={`ve-drop${over ? " over" : ""}`} onClick={onPick}
      onDragOver={(e) => { if (Array.from(e.dataTransfer.types ?? []).includes("Files")) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; setDropBin(""); setOver(true); } }}
      onDragLeave={() => { setOver(false); setDropBin(""); }}
      onDrop={(e) => { setOver(false); if (Array.from(e.dataTransfer.types ?? []).includes("Files")) e.preventDefault(); }}>
      <Link2 size={18} />
      <div><b>Drop footage here</b> or click to pick</div>
      <div className="ve-faint" style={{ fontSize: 11 }}>Drag files <b>or folders</b> straight in — a folder becomes a bin, subfolders nest. Same-volume hardlinked, never copied.</div>
    </div>
  );
}
type MediaSel = { sel: string | null; setSel: (v: string | null) => void };
function kindIcon(kind: Asset["kind"], size = 14) {
  return kind === "audio" ? <Music size={size} /> : kind === "image" ? <ImageIcon size={size} /> : <Film size={size} />;
}
function AssetGrid({ assets, sel, setSel }: { assets: Asset[] } & MediaSel) {
  const s = useVideo();
  return (
    <div className="ve-media">
      {assets.map((a) => {
        const thumb = a.thumbs?.strip && s.agentId && s.project ? mediaUrl(s.agentId, s.project, "cache", a.thumbs.strip, s.cacheBust) : null;
        const fw = a.thumbs?.frameW ?? 114, fh = a.thumbs?.frameH ?? 64;
        return (
          <div key={a.id} className={`item ${sel === a.id ? "sel" : ""} ${a.online === false ? "offline" : ""}`} draggable onDragStart={(e) => { e.dataTransfer.setData("application/aygent-asset", a.id); e.dataTransfer.effectAllowed = "copy"; }}
            onClick={() => setSel(a.id)} onDoubleClick={() => addAssetToTimeline(a)} tabIndex={0}
            onKeyDown={(e) => { if ((e.key === "Backspace" || e.key === "Delete") && sel === a.id) { e.preventDefault(); if (confirm(`Unlink ${a.name}? Clips using it are removed.`)) void removeAsset(a.id); } }}>
            <div className={`thumb ${a.kind}`} style={thumb && a.kind !== "audio" ? { backgroundImage: `url(${thumb})`, backgroundSize: `${(fw / fh) * 100 * (a.thumbs?.stripFrames ?? 1)}% 100%`, backgroundPosition: "left center" } : undefined}>
              {a.kind === "audio" ? <Music size={22} /> : !thumb ? (a.kind === "image" ? <ImageIcon size={22} /> : <Film size={22} />) : null}
            </div>
            <span className="badge">{a.kind === "video" ? `${a.height}p` : a.kind.toUpperCase()}{!a.linked ? " · REF" : ""}</span>
              {a.online === false && <span className="badge off" title={a.path}>OFFLINE</span>}
            <button className="x" title="Unlink" onClick={(e) => { e.stopPropagation(); if (confirm(`Unlink ${a.name}? Clips using it are removed.`)) void removeAsset(a.id); }}><X size={12} /></button>
            <div className="meta"><div className="n" title={a.path}>{a.name}</div><div className="d"><span>{a.kind === "image" ? `${a.width}×${a.height}` : fmtDur(a.duration)}</span><span>{a.fps ? `${Math.round(a.fps)}fps` : fmtBytes(a.size)}</span></div></div>
          </div>
        );
      })}
    </div>
  );
}

// ---- LIST VIEW (Premiere-style nested bins) ----------------------------------
// Recursive bin tree: sub-bins indent under their parent, Unfiled + Generated
// stay flat at the bottom. Assets AND bins both drag between bins (bin drag
// sets a second mime so headers stay drop targets for both). Folder import
// parks a whole directory here as a bin tree.
export const ASSET_MIME = "application/aygent-asset";
export const BIN_MIME = "application/aygent-bin";

function AssetList({ files, generated, sel, setSel }: { files: Asset[]; generated: Asset[] } & MediaSel) {
  const s = useVideo();
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => {
    try { return JSON.parse(localStorage.getItem("aygent.video.binsCollapsed") ?? "{}"); } catch { return {}; }
  });
  const [creating, setCreating] = useState<string | null>(null); // parent bin id ("" = top level) or null
  const [draft, setDraft] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [targetBin, setTargetBin] = useState("");
  const [moveIds, setMoveIds] = useState<string[] | null>(null);
  const isCollapsed = (id: string) => collapsed[id] ?? (id === "__gen");
  function toggle(id: string) {
    setCollapsed((c) => {
      const next = { ...c, [id]: !(c[id] ?? (id === "__gen")) };
      try { localStorage.setItem("aygent.video.binsCollapsed", JSON.stringify(next)); } catch { /* ignore */ }
      return next;
    });
  }
  const folders: MediaFolder[] = s.folders;
  // tree: parent id → children; orphans (dangling parent) surface as roots
  const kids = new Map<string, MediaFolder[]>();
  const roots: MediaFolder[] = [];
  for (const f of folders) {
    const p = f.parent ?? "";
    if (p && folders.some((x) => x.id === p)) {
      const arr = kids.get(p) ?? [];
      arr.push(f); kids.set(p, arr);
    } else roots.push(f);
  }
  const byFolder = new Map<string, Asset[]>();
  const unfiled: Asset[] = [];
  for (const a of files) {
    if (a.folder && folders.some((f) => f.id === a.folder)) {
      const arr = byFolder.get(a.folder) ?? [];
      arr.push(a); byFolder.set(a.folder, arr);
    } else unfiled.push(a);
  }
  // flat list with depth for the Move-to menu
  const flat: { f: MediaFolder; depth: number }[] = [];
  (function walk(list: MediaFolder[], d: number) {
    for (const f of list) { flat.push({ f, depth: d }); walk(kids.get(f.id) ?? [], d + 1); }
  })(roots, 0);
  function countTotal(id: string): number {
    let n = (byFolder.get(id) ?? []).length;
    for (const k of kids.get(id) ?? []) n += countTotal(k.id);
    return n;
  }
  async function commitCreate() {
    if (creating === null) return; // cancelled via Escape
    const name = draft.trim();
    const parent = creating ?? "";
    setCreating(null); setDraft("");
    if (!name) return;
    const r = await createMediaFolder(name, parent);
    if (r) toggleOff(r.id);
  }
  function toggleOff(id: string) {
    setCollapsed((c) => {
      if (c[id] === false) return c;
      const next = { ...c, [id]: false };
      try { localStorage.setItem("aygent.video.binsCollapsed", JSON.stringify(next)); } catch { /* ignore */ }
      return next;
    });
  }
  async function commitRename(id: string) {
    const name = renameDraft.trim();
    setRenaming(null);
    if (name) await renameMediaFolder(id, name);
  }
  async function commitMove() {
    if (moveIds && moveIds.length) await moveMediaAssets(moveIds, targetBin);
    setMoveIds(null); setTargetBin("");
  }
  const openMove = (ids: string[]) => { setMoveIds(ids); setTargetBin(""); };
  return (
    <div className="ve-binlist">
      <div className="ve-binlist-bar">
        <button className="ve-btn sm" onClick={() => { setCreating(""); setDraft(""); }}><FolderPlus size={12} /> New bin</button>
      </div>
      {creating === "" && (
        <form className="ve-bin-create" onSubmit={(e) => { e.preventDefault(); void commitCreate(); }}>
          <Folder size={14} />
          <input autoFocus value={draft} placeholder="Bin name" onChange={(e) => setDraft(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setCreating(null)} onBlur={() => void commitCreate()} aria-label="New bin name" />
        </form>
      )}
      {roots.map((f) => (
        <BinNode key={f.id} folder={f} depth={0}
          sel={sel} setSel={setSel} folders={folders} byFolder={byFolder} kids={kids} countTotal={countTotal}
          isCollapsed={isCollapsed} toggle={toggle}
          renaming={renaming} renameDraft={renameDraft} setRenameDraft={setRenameDraft}
          onStartRename={(fid, cur) => { setRenaming(fid); setRenameDraft(cur); }}
          onCommitRename={(fid) => void commitRename(fid)} onCancelRename={() => setRenaming(null)}
          onOpenMove={openMove} creating={creating}
          onStartCreate={(pid) => { setCreating(pid); setDraft(""); }}
          onCancelCreate={() => { setCreating(null); setDraft(""); }}
          draft={draft} setDraft={setDraft} commitCreate={() => void commitCreate()} />
      ))}
      <BinGroupBare id="__unfiled" name="Unfiled" assets={unfiled} open={!isCollapsed("__unfiled")} onToggle={() => toggle("__unfiled")}
        sel={sel} setSel={setSel} folders={folders} onOpenMove={openMove} />
      {generated.length > 0 && (
        <BinGroupBare id="__gen" name="Generated" assets={generated} open={!isCollapsed("__gen")} onToggle={() => toggle("__gen")}
          sel={sel} setSel={setSel} folders={folders} onOpenMove={openMove} />
      )}
      {moveIds && (
        <div className="ve-move-pop" role="dialog" aria-label="Move media to bin">
          <h4>Move {moveIds.length} item{moveIds.length === 1 ? "" : "s"} to…</h4>
          <select value={targetBin} onChange={(e) => setTargetBin(e.target.value)} autoFocus>
            <option value="">Unfiled</option>
            {flat.map(({ f, depth }) => <option key={f.id} value={f.id}>{`${"  ".repeat(depth)}📁 ${f.name}`}</option>)}
          </select>
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
            <button className="ve-btn sm" onClick={() => { setMoveIds(null); setTargetBin(""); }}>Cancel</button>
            <button className="ve-btn sm primary" onClick={() => void commitMove()}>Move</button>
          </div>
        </div>
      )}
    </div>
  );
}

type BinProps = {
  folder: MediaFolder; depth: number;
  sel: string | null; setSel: (v: string | null) => void;
  folders: MediaFolder[]; byFolder: Map<string, Asset[]>; kids: Map<string, MediaFolder[]>;
  countTotal: (id: string) => number;
  isCollapsed: (id: string) => boolean; toggle: (id: string) => void;
  renaming: string | null; renameDraft: string; setRenameDraft: (v: string) => void;
  onStartRename: (id: string, cur: string) => void; onCommitRename: (id: string) => void; onCancelRename: () => void;
  onOpenMove: (ids: string[]) => void;
  creating: string | null; onStartCreate: (parent: string) => void; onCancelCreate: () => void;
  draft: string; setDraft: (v: string) => void; commitCreate: () => void;
};

/** One bin in the tree. Accepts BOTH asset drops (move into bin) and bin drops
 *  (nest under bin). Dragging the header moves the bin itself. */
function BinNode(props: BinProps) {
  const { folder, depth, sel, setSel, folders, byFolder, kids, countTotal, isCollapsed, toggle } = props;
  const id = folder.id;
  const open = !isCollapsed(id);
  const assets = byFolder.get(id) ?? [];
  const children = kids.get(id) ?? [];
  const total = countTotal(id);
  const renaming = props.renaming === id;
  const creatingHere = props.creating === id;
  const [over, setOver] = useState<"" | "asset" | "bin">("");

  function accepts(e: React.DragEvent): "" | "asset" | "bin" | "files" {
    const types = Array.from(e.dataTransfer.types ?? []);
    if (types.includes(ASSET_MIME)) return "asset";
    if (types.includes(BIN_MIME)) return "bin";
    if (types.includes("Files")) return "files";
    return "";
  }
  function onDrop(e: React.DragEvent) {
    const kind = accepts(e);
    setOver(""); setDropBin("");
    if (!kind) return;
    if (kind === "files") { e.preventDefault(); return; } // native drop owns OS files
    e.preventDefault(); e.stopPropagation();
    if (kind === "asset") {
      const aid = e.dataTransfer.getData(ASSET_MIME);
      if (aid) void moveMediaAssets([aid], id);
    } else {
      const bid = e.dataTransfer.getData(BIN_MIME);
      if (bid && bid !== id) void moveMediaFolder(bid, id);
    }
  }
  function onDelete() {
    if (confirm(`Delete bin "${folder.name}"?${children.length ? ` Its ${children.length} sub-bin${children.length === 1 ? "" : "s"} move up one level.` : ""} Its media moves up too.`)) void deleteMediaFolder(id);
  }
  return (
    <div style={depth ? { marginLeft: depth * 14 } : undefined}>
      <div className={`ve-bin ${open ? "open" : ""} ${over ? "over" : ""}`}
        onDragOver={(e) => { const k = accepts(e); if (k) { e.preventDefault(); e.dataTransfer.dropEffect = k === "files" ? "copy" : "move"; setOver(k === "files" ? "asset" : k); if (k === "files") setDropBin(id); } }}
        onDragLeave={() => { setOver(""); setDropBin(""); }} onDrop={onDrop}>
        <div className="ve-bin-hd" onClick={() => toggle(id)} title={open ? "Collapse" : "Expand"}
          draggable={!renaming}
          onDragStart={!renaming ? (e) => { e.dataTransfer.setData(BIN_MIME, id); e.dataTransfer.effectAllowed = "move"; e.stopPropagation(); } : undefined}>
          <span className="caret">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
          <Folder size={14} />
          {renaming ? (
            <input autoFocus value={props.renameDraft} aria-label="Rename bin"
              onClick={(e) => e.stopPropagation()} onChange={(e) => props.setRenameDraft(e.target.value)}
              onBlur={() => props.onCommitRename(id)} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); props.onCommitRename(id); } if (e.key === "Escape") { e.preventDefault(); props.onCancelRename(); } }} />
          ) : (
            <span className="nm" title={over === "bin" ? "Drop to nest bin here" : over === "asset" ? "Drop to move media here" : folder.name}>{folder.name}</span>
          )}
          <span className="ct" title={`${assets.length} here · ${total} with sub-bins`}>{assets.length}{total !== assets.length ? ` (${total})` : ""}</span>
          {!renaming && (
            <span className="ops" onClick={(e) => e.stopPropagation()}>
              <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="New sub-bin" onClick={() => props.onStartCreate(id)}><FolderPlus size={12} /></button>
              <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="Import folder here as nested bins" onClick={() => void importFolder(id)}><FolderInput size={12} /></button>
              <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="Rename bin" onClick={() => props.onStartRename(id, folder.name)}><Pencil size={12} /></button>
              <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="Delete bin (contents move up)" onClick={onDelete}><Trash2 size={12} /></button>
            </span>
          )}
        </div>
        {open && (
          <div className="ve-bin-rows">
            {creatingHere && (
              <form className="ve-bin-create" onSubmit={(e) => { e.preventDefault(); props.commitCreate(); }}>
                <Folder size={14} />
                <input autoFocus value={props.draft} placeholder="Sub-bin name" onChange={(e) => props.setDraft(e.target.value)} onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); props.onCancelCreate(); } }} onBlur={() => props.commitCreate()} aria-label="New sub-bin name" />
              </form>
            )}
            {children.map((k) => (
              <BinNode key={k.id} {...props} folder={k} depth={depth + 1} />
            ))}
            {assets.length === 0 && children.length === 0 && !creatingHere && <div className="ve-bin-empty">Empty — drag media here.</div>}
            {assets.map((a) => <AssetRow key={a.id} a={a} sel={sel === a.id} setSel={setSel} onOpenMove={props.onOpenMove} folders={folders} />)}
          </div>
        )}
      </div>
    </div>
  );
}

/** The synthetic Unfiled / Generated groups: flat, no rename/delete/children.
 *  Asset drops unfile here; bin drops move the bin to top level. */
function BinGroupBare({ id, name, assets, open, onToggle, sel, setSel, folders, onOpenMove }: {
  id: string; name: string; assets: Asset[]; open: boolean; onToggle: () => void;
  sel: string | null; setSel: (v: string | null) => void;
  folders: MediaFolder[]; onOpenMove: (ids: string[]) => void;
}) {
  const [over, setOver] = useState(false);
  function onDrop(e: React.DragEvent) {
    const types = Array.from(e.dataTransfer.types ?? []);
    setOver(false); setDropBin("");
    if (types.includes("Files")) { e.preventDefault(); return; } // native owns OS files
    if (types.includes(ASSET_MIME)) {
      const aid = e.dataTransfer.getData(ASSET_MIME);
      if (!aid) return;
      e.preventDefault(); e.stopPropagation();
      void moveMediaAssets([aid], "");
    } else if (types.includes(BIN_MIME)) {
      const bid = e.dataTransfer.getData(BIN_MIME);
      if (!bid) return;
      e.preventDefault(); e.stopPropagation();
      void moveMediaFolder(bid, "");
    }
  }
  return (
    <div className={`ve-bin ${open ? "open" : ""} ${over ? "over" : ""}`}
      onDragOver={(e) => { const t = Array.from(e.dataTransfer.types ?? []); if (t.includes(ASSET_MIME) || t.includes(BIN_MIME)) { e.preventDefault(); e.dataTransfer.dropEffect = "move"; setOver(true); } else if (t.includes("Files")) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; setDropBin(""); setOver(true); } }}
      onDragLeave={() => { setOver(false); setDropBin(""); }} onDrop={onDrop}>
      <div className="ve-bin-hd" onClick={onToggle} title={open ? "Collapse" : "Expand"}>
        <span className="caret">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
        <Folder size={14} />
        <span className="nm" title={name}>{name}</span>
        <span className="ct">{assets.length}</span>
      </div>
      {open && (
        <div className="ve-bin-rows">
          {assets.length === 0 && <div className="ve-bin-empty">Empty — drag media here.</div>}
          {assets.map((a) => <AssetRow key={a.id} a={a} sel={sel === a.id} setSel={setSel} onOpenMove={onOpenMove} folders={folders} />)}
        </div>
      )}
    </div>
  );
}

function AssetRow({ a, sel, setSel, onOpenMove, folders }: { a: Asset; sel: boolean; setSel: (v: string | null) => void; onOpenMove: (ids: string[]) => void; folders: MediaFolder[] }) {
  return (
    <div className={`ve-brow ${sel ? "sel" : ""} ${a.online === false ? "offline" : ""}`} draggable
      onDragStart={(e) => { e.dataTransfer.setData(ASSET_MIME, a.id); e.dataTransfer.effectAllowed = "move"; }}
      onClick={() => setSel(a.id)} onDoubleClick={() => addAssetToTimeline(a)} tabIndex={0}
      onKeyDown={(e) => { if ((e.key === "Backspace" || e.key === "Delete") && sel) { e.preventDefault(); if (confirm(`Unlink ${a.name}? Clips using it are removed.`)) void removeAsset(a.id); } }}>
      <span className="ic">{kindIcon(a.kind)}</span>
      <span className="n" title={a.path}>{a.name}</span>
      <span className="d">{a.kind === "image" ? `${a.width}×${a.height}` : fmtDur(a.duration)}</span>
      <span className="d sz">{a.fps ? `${Math.round(a.fps)}fps` : fmtBytes(a.size)}</span>
      {a.online === false && <span className="ve-pill err" style={{ height: 18, fontSize: 10 }}>offline</span>}
      {!a.linked && <span className="ve-faint" style={{ fontSize: 10 }}>REF</span>}
      <span className="ops">
        {folders.length > 0 && <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="Move to bin…" onClick={(e) => { e.stopPropagation(); onOpenMove([a.id]); }}><FolderOpen size={12} /></button>}
        <button className="ve-icon-btn" style={{ width: 22, height: 22 }} title="Unlink" onClick={(e) => { e.stopPropagation(); if (confirm(`Unlink ${a.name}? Clips using it are removed.`)) void removeAsset(a.id); }}><X size={12} /></button>
      </span>
    </div>
  );
}

export const _testBins = { isGeneratedAsset };
