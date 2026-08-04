// DRAG + RESIZE interaction (M4).
//
// Pointer-events based (works for mouse and trackpad, and gives us implicit
// capture so a fast drag that leaves the card doesn't strand the gesture).
//
// The model: during a gesture we keep a LIVE preview in React state and never
// touch the database. On release we diff against the starting layout and
// persist only what moved — one `dashboard_arrange` call, which is one undo
// step. Writing per-frame would spam the revision log and make undo useless.
import { useCallback, useEffect, useRef, useState } from "react";
import {
  clamp, clampResize, compact, resolve, pxToCells, changed,
  type Placed,
} from "./gridDrag";

type Mode = "move" | "resize";

interface Gesture {
  id: string;
  mode: Mode;
  startX: number;
  startY: number;
  origin: Placed[];      // layout snapshot when the gesture began
  containerW: number;
}

export interface DragState {
  /** Layouts to RENDER (live preview during a gesture, else the real ones). */
  preview: Placed[] | null;
  /** id currently being dragged, for styling. */
  activeId: string | null;
  mode: Mode | null;
  onPointerDown: (e: React.PointerEvent, id: string, mode: Mode) => void;
  gridRef: React.RefObject<HTMLDivElement>;
}

export function useGridDrag(
  items: Placed[],
  onCommit: (moves: Placed[]) => void,
): DragState {
  const gridRef = useRef<HTMLDivElement>(null);
  const [preview, setPreview] = useState<Placed[] | null>(null);
  const gesture = useRef<Gesture | null>(null);
  // Keep the latest preview in a ref so the pointerup handler (registered once)
  // always commits what's actually on screen, not a stale closure value.
  const previewRef = useRef<Placed[] | null>(null);
  previewRef.current = preview;

  const onPointerDown = useCallback((e: React.PointerEvent, id: string, mode: Mode) => {
    // Left button only; ignore modifier-clicks so right-click menus still work.
    if (e.button !== 0) return;
    const grid = gridRef.current;
    if (!grid) return;
    e.preventDefault();
    e.stopPropagation();
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    gesture.current = {
      id, mode,
      startX: e.clientX,
      startY: e.clientY,
      origin: items.map((i) => ({ ...i })),
      containerW: grid.clientWidth,
    };
    setPreview(items.map((i) => ({ ...i })));
  }, [items]);

  useEffect(() => {
    function move(e: PointerEvent) {
      const g = gesture.current;
      if (!g) return;
      const { cx, cy } = pxToCells(e.clientX - g.startX, e.clientY - g.startY, g.containerW);
      const start = g.origin.find((i) => i.id === g.id);
      if (!start) return;

      const next = g.origin.map((i) => ({ ...i }));
      const target = next.find((i) => i.id === g.id)!;
      if (g.mode === "move") {
        Object.assign(target, clamp({ ...start, x: start.x + cx, y: start.y + cy }));
      } else {
        Object.assign(target, clampResize({ ...start, w: start.w + cx, h: start.h + cy }));
      }
      // Push collisions down, then float everything up into the gaps. Doing
      // this LIVE is what makes the layout feel physical instead of a snap at
      // the end that surprises you.
      setPreview(compact(resolve(next, g.id)));
    }

    function up() {
      const g = gesture.current;
      if (!g) return;
      gesture.current = null;
      const final = previewRef.current;
      setPreview(null);
      if (!final) return;
      const moves = changed(g.origin, final);
      if (moves.length > 0) onCommit(moves);
    }

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
  }, [onCommit]);

  return {
    preview,
    activeId: gesture.current?.id ?? null,
    mode: gesture.current?.mode ?? null,
    onPointerDown,
    gridRef,
  };
}

/** The bottom-right resize grip. Drawn with borders only — no image, no icon
 *  font — so it inherits the accent/line tokens like everything else. */
export function ResizeGrip({ onPointerDown }: { onPointerDown: (e: React.PointerEvent) => void }) {
  return (
    <div
      onPointerDown={onPointerDown}
      title="Drag to resize"
      style={{
        position: "absolute", right: 3, bottom: 3, width: 16, height: 16,
        cursor: "nwse-resize", opacity: 0.45,
        borderRight: "2px solid var(--line)", borderBottom: "2px solid var(--line)",
        borderBottomRightRadius: 4,
      }}
    />
  );
}
