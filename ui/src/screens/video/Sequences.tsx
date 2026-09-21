// AYGENT — VIDEO Sequences sidebar: multiple edits, one footage pool.
// Each sequence is its own composition file (main = composition.json, the rest
// sequences/<name>.json). Media, transcript, bins, chat are project-level and
// shared — only the timeline differs. Bins here organize SEQUENCES the same
// way MediaBins organizes assets (create/rename/delete + drag-to-nest).
import { useState } from "react";
import { Clapperboard, Plus, Pencil, Trash2, Copy, ChevronRight, ChevronDown, FolderPlus, Folder } from "lucide-react";
import { useVideo, openSequence, createSequence, renameSequence, deleteSequence, moveMediaFolder } from "./store";
import { BIN_MIME } from "./MediaBins";

type SeqBin = { id: string; name: string; parent: string };

function binKey(project: string) {
  return `aygent.video.seqBins.${project}`;
}
function loadBins(project: string): SeqBin[] {
  try {
    const raw = JSON.parse(localStorage.getItem(binKey(project)) ?? "[]");
    if (!Array.isArray(raw)) return [];
    return raw
      .filter((b) => b && typeof b.id === "string")
      .map((b: any) => ({ id: String(b.id), name: String(b.name ?? "bin"), parent: String(b.parent ?? "") }));
  } catch {
    return [];
  }
}
function saveBins(project: string, bins: SeqBin[]) {
  try {
    localStorage.setItem(binKey(project), JSON.stringify(bins));
  } catch {
    /* ignore */
  }
}
const binId = () => `sb${Date.now().toString(36)}${Math.floor(Math.random() * 1296).toString(36)}`;

function isInBin(seqName: string, binId: string, bins: SeqBin[], assign: Record<string, string>): boolean {
  return (assign[seqName] ?? "") === binId;
}

export function SequencesPanel() {
  const s = useVideo();
  const project = s.project ?? "";
  const [bins, setBins] = useState<SeqBin[]>(() => loadBins(project));
  const [assign, setAssign] = useState<Record<string, string>>(() => {
    try {
      return JSON.parse(localStorage.getItem(`aygent.video.seqAssign.${project}`) ?? "{}");
    } catch {
      return {};
    }
  });
  const [creating, setCreating] = useState<string | null>(null); // parent or "" or null
  const [draft, setDraft] = useState("");
  const [newSeq, setNewSeq] = useState(false);
  const [seqDraft, setSeqDraft] = useState("");
  const [dupFrom, setDupFrom] = useState("");
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [over, setOver] = useState("");

  // project switch: reload bins/assign
  const [lastProject, setLastProject] = useState(project);
  if (project !== lastProject) {
    setLastProject(project);
    setBins(loadBins(project));
    try {
      setAssign(JSON.parse(localStorage.getItem(`aygent.video.seqAssign.${project}`) ?? "{}"));
    } catch {
      setAssign({});
    }
    setCreating(null);
    setRenaming(null);
    setNewSeq(false);
  }
  function persistAssign(next: Record<string, string>) {
    setAssign(next);
    try {
      localStorage.setItem(`aygent.video.seqAssign.${project}`, JSON.stringify(next));
    } catch {
      /* ignore */
    }
  }
  function persistBins(next: SeqBin[]) {
    setBins(next);
    saveBins(project, next);
  }

  const seqNames = s.sequences.map((x) => x.name);
  const kids = new Map<string, SeqBin[]>();
  const roots: SeqBin[] = [];
  for (const b of bins) {
    if (b.parent && bins.some((x) => x.id === b.parent)) {
      const arr = kids.get(b.parent) ?? [];
      arr.push(b);
      kids.set(b.parent, arr);
    } else roots.push(b);
  }
  const inBin = (id: string) => seqNames.filter((n) => isInBin(n, id, bins, assign));
  const unfiled = seqNames.filter((n) => {
    const b = assign[n] ?? "";
    return !b || !bins.some((x) => x.id === b);
  });
  const isCollapsed = (id: string) => collapsed[id] ?? false;
  const toggle = (id: string) => setCollapsed((c) => ({ ...c, [id]: !c[id] }));

  async function commitCreate() {
    if (creating === null) return;
    const name = draft.trim().slice(0, 80);
    const parent = creating ?? "";
    setCreating(null);
    setDraft("");
    if (!name) return;
    persistBins([...bins, { id: binId(), name, parent }]);
  }
  async function commitSeq() {
    const name = seqDraft.trim();
    if (!name) {
      setNewSeq(false);
      return;
    }
    const ok = await createSequence(name, dupFrom || undefined);
    if (ok) {
      const clean = name.replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 80);
      persistAssign({ ...assign, [clean]: "" });
    }
    setNewSeq(false);
    setSeqDraft("");
    setDupFrom("");
  }
  async function commitRename(old: string) {
    const next = renameDraft.trim();
    setRenaming(null);
    if (!next || next === old) return;
    const ok = await renameSequence(old, next);
    if (ok) {
      const clean = next.replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 80);
      const a = { ...assign };
      if (a[old] !== undefined) {
        a[clean] = a[old];
        delete a[old];
      }
      persistAssign(a);
    }
  }

  function acceptsBin(e: React.DragEvent): boolean {
    return Array.from(e.dataTransfer.types ?? []).includes(BIN_MIME);
  }
  function dropBinOnto(e: React.DragEvent, target: string) {
    const bid = e.dataTransfer.getData(BIN_MIME);
    setOver("");
    if (!bid || bid === target) return;
    e.preventDefault();
    e.stopPropagation();
    // cycle guard
    let cursor = target;
    while (cursor) {
      if (cursor === bid) {
        return;
      }
      cursor = bins.find((b) => b.id === cursor)?.parent ?? "";
    }
    persistBins(bins.map((b) => (b.id === bid ? { ...b, parent: target } : b)));
  }

  function renderSeqRow(name: string) {
    const active = s.sequence === name;
    const main = name === "main";
    return (
      <div
        key={name}
        className={`ve-seqrow ${active ? "on" : ""}`}
        draggable
        onDragStart={(e) => {
          e.dataTransfer.setData("application/aygent-seq", name);
          e.dataTransfer.effectAllowed = "move";
        }}
        onClick={() => {
          if (!active) void openSequence(name);
        }}
        title={active ? "Current sequence" : `Open ${name}`}
      >
        <Clapperboard size={13} />
        {renaming === name ? (
          <input
            autoFocus
            value={renameDraft}
            aria-label="Rename sequence"
            onClick={(e) => e.stopPropagation()}
            onChange={(e) => setRenameDraft(e.target.value)}
            onBlur={() => void commitRename(name)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void commitRename(name);
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setRenaming(null);
              }
            }}
          />
        ) : (
          <span className="nm" title={name}>
            {name}
            {main ? <span className="ve-faint"> · main</span> : null}
          </span>
        )}
        {!renaming && (
          <span
            className="ops"
            onClick={(e) => e.stopPropagation()}
            onMouseDown={(e) => e.stopPropagation()}
            draggable
            onDragStart={(e) => e.stopPropagation()}
          >
            <button
              className="ve-icon-btn"
              style={{ width: 22, height: 22 }}
              title="Duplicate as new sequence"
              onClick={() => {
                setNewSeq(true);
                setSeqDraft(`${name}-copy`);
                setDupFrom(name);
              }}
            >
              <Copy size={12} />
            </button>
            {!main && (
              <>
                <button
                  className="ve-icon-btn"
                  style={{ width: 22, height: 22 }}
                  title="Rename sequence"
                  onClick={() => {
                    setRenaming(name);
                    setRenameDraft(name);
                  }}
                >
                  <Pencil size={12} />
                </button>
                <button
                  className="ve-icon-btn"
                  style={{ width: 22, height: 22 }}
                  title="Delete sequence (media + bins shared, untouched)"
                  onClick={() => {
                    if (confirm(`Delete sequence "${name}"? Media, bins, transcript are shared and untouched.`))
                      void deleteSequence(name);
                  }}
                >
                  <Trash2 size={12} />
                </button>
              </>
            )}
          </span>
        )}
      </div>
    );
  }

  function renderBin(b: SeqBin, depth: number): React.ReactNode {
    const open = !isCollapsed(b.id);
    const children = kids.get(b.id) ?? [];
    const seqs = inBin(b.id);
    const hov = over === b.id;
    return (
      <div key={b.id} style={depth ? { marginLeft: depth * 14 } : undefined}>
        <div
          className={`ve-bin ${open ? "open" : ""} ${hov ? "over" : ""}`}
          onDragOver={(e) => {
            const t = Array.from(e.dataTransfer.types ?? []);
            if (t.includes("application/aygent-seq")) {
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              setOver(b.id);
            } else if (acceptsBin(e)) {
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              setOver(b.id);
            }
          }}
          onDragLeave={() => setOver("")}
          onDrop={(e) => {
            const t = Array.from(e.dataTransfer.types ?? []);
            if (t.includes("application/aygent-seq")) {
              const name = e.dataTransfer.getData("application/aygent-seq");
              setOver("");
              if (!name) return;
              e.preventDefault();
              e.stopPropagation();
              persistAssign({ ...assign, [name]: b.id });
            } else if (acceptsBin(e)) {
              dropBinOnto(e, b.id);
            }
          }}
        >
          <div className="ve-bin-hd" onClick={() => toggle(b.id)} title={open ? "Collapse" : "Expand"}>
            <span className="caret">{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</span>
            <Folder size={14} />
            <span className="nm" title={hov ? "Drop here" : b.name}>
              {b.name}
            </span>
            <span className="ct">{seqs.length + children.reduce((n, k) => n + inBin(k.id).length, 0)}</span>
            <span className="ops" onClick={(e) => e.stopPropagation()}>
              <button
                className="ve-icon-btn"
                style={{ width: 22, height: 22 }}
                title="Delete bin (sequences move up)"
                onClick={() => {
                  if (!confirm(`Delete bin "${b.name}"? Its sequences move up one level.`)) return;
                  const up = b.parent;
                  const nb = bins.filter((x) => x.id !== b.id).map((x) => (x.parent === b.id ? { ...x, parent: up } : x));
                  persistBins(nb);
                  const a = { ...assign };
                  for (const n of seqs) a[n] = up;
                  persistAssign(a);
                }}
              >
                <Trash2 size={12} />
              </button>
            </span>
          </div>
          {open && (
            <div className="ve-bin-rows">
              {children.map((k) => renderBin(k, depth + 1))}
              {seqs.map(renderSeqRow)}
              {seqs.length === 0 && children.length === 0 && <div className="ve-bin-empty">Empty — drag sequences here.</div>}
            </div>
          )}
        </div>
      </div>
    );
  }

  // accept sequence drops to move between bins incl. Unfiled
  function seqDropZone(id: string, children: React.ReactNode) {
    return (
      <div
        onDragOver={(e) => {
          if (Array.from(e.dataTransfer.types ?? []).includes("application/aygent-seq")) {
            e.preventDefault();
            e.dataTransfer.dropEffect = "move";
            setOver(id);
          }
        }}
        onDragLeave={() => setOver("")}
        onDrop={(e) => {
          if (!Array.from(e.dataTransfer.types ?? []).includes("application/aygent-seq")) return;
          const name = e.dataTransfer.getData("application/aygent-seq");
          setOver("");
          if (!name) return;
          e.preventDefault();
          e.stopPropagation();
          persistAssign({ ...assign, [name]: id === "__unfiled" ? "" : id });
        }}
      >
        {children}
      </div>
    );
  }

  return (
    <div className="ve-seqs">
      <div className="ve-binlist-bar">
        <button className="ve-btn sm" onClick={() => { setNewSeq(true); setSeqDraft(""); setDupFrom(""); }}>
          <Plus size={12} /> Sequence
        </button>
        <button className="ve-btn sm" onClick={() => { setCreating(""); setDraft(""); }}>
          <FolderPlus size={12} /> Bin
        </button>
      </div>
      {newSeq && (
        <form
          className="ve-bin-create"
          onSubmit={(e) => {
            e.preventDefault();
            void commitSeq();
          }}
        >
          <Clapperboard size={14} />
          <input
            autoFocus
            value={seqDraft}
            placeholder="Sequence name"
            onChange={(e) => setSeqDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setNewSeq(false);
            }}
            onBlur={() => void commitSeq()}
            aria-label="New sequence name"
          />
          {seqNames.length > 0 && (
            <select value={dupFrom} onChange={(e) => setDupFrom(e.target.value)} title="Duplicate from (empty = blank)">
              <option value="">Blank</option>
              {seqNames.map((n) => (
                <option key={n} value={n}>
                  ⧉ {n}
                </option>
              ))}
            </select>
          )}
        </form>
      )}
      {creating === "" && (
        <form
          className="ve-bin-create"
          onSubmit={(e) => {
            e.preventDefault();
            void (async () => {
              if (!draft.trim()) {
                setCreating(null);
                return;
              }
              persistBins([...bins, { id: binId(), name: draft.trim().slice(0, 80), parent: "" }]);
              setCreating(null);
              setDraft("");
            })();
          }}
        >
          <Folder size={14} />
          <input
            autoFocus
            value={draft}
            placeholder="Bin name"
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setCreating(null);
            }}
            onBlur={() => {
              if (draft.trim()) persistBins([...bins, { id: binId(), name: draft.trim().slice(0, 80), parent: "" }]);
              setCreating(null);
              setDraft("");
            }}
            aria-label="New sequences bin"
          />
        </form>
      )}
      {roots.map((b) => renderBin(b, 0))}
      {seqDropZone(
        "__unfiled",
        <div className={`ve-bin ${over === "__unfiled" ? "over" : ""}`}>
          <div className="ve-bin-hd" style={{ cursor: "default" }}>
            <Folder size={14} />
            <span className="nm">Sequences</span>
            <span className="ct">{unfiled.length}</span>
          </div>
          <div className="ve-bin-rows">{unfiled.map(renderSeqRow)}</div>
        </div>,
      )}
      <p className="ve-hint">One footage pool, many edits. Bins organize sequences; media + transcript stay shared.</p>
    </div>
  );
}

// Re-exported so moveMediaFolder's BIN_MIME stays the single drag key for bins.
export { moveMediaFolder };
