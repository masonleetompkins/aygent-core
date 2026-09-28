// DASHBOARD GRID GEOMETRY (M4) — pure functions, no React, no DOM.
//
// Kept separate from the screen on purpose: drag math is the part that's easy
// to get subtly wrong (off-by-one columns, negative rows, cards eating each
// other), and pure functions can be reasoned about and tested directly.
//
// The grid is 12 columns wide, rows are a fixed pixel height, and every module
// owns an {x,y,w,h} in CELL units — never pixels. Pixels only exist during a
// drag, and get converted back to cells before anything is persisted.

export const GRID_COLS = 12;
export const ROW_H = 44;
export const GAP = 16;
export const MIN_W = 2;
export const MIN_H = 2;

export interface Layout { x: number; y: number; w: number; h: number }
export interface Placed extends Layout { id: string }

/** Width of one column in px, given the grid's measured content width. */
export function colWidth(containerW: number): number {
  return (containerW - GAP * (GRID_COLS - 1)) / GRID_COLS;
}

/** Convert a pixel delta into a whole-cell delta (rounded, so it snaps). */
export function pxToCells(dx: number, dy: number, containerW: number): { cx: number; cy: number } {
  const cw = colWidth(containerW);
  return {
    cx: Math.round(dx / (cw + GAP)),
    cy: Math.round(dy / (ROW_H + GAP)),
  };
}

/** Keep a layout inside the grid and above the minimum usable size. */
export function clamp(l: Layout): Layout {
  const w = Math.max(MIN_W, Math.min(GRID_COLS, Math.round(l.w)));
  const h = Math.max(MIN_H, Math.round(l.h));
  const x = Math.max(0, Math.min(GRID_COLS - w, Math.round(l.x)));
  const y = Math.max(0, Math.round(l.y));
  return { x, y, w, h };
}

/** Clamp a RESIZE: the left/top edge is fixed, so width is bounded by what's
 *  left of the grid rather than by sliding the card back inward. */
export function clampResize(l: Layout): Layout {
  const x = Math.max(0, Math.min(GRID_COLS - MIN_W, Math.round(l.x)));
  const y = Math.max(0, Math.round(l.y));
  const w = Math.max(MIN_W, Math.min(GRID_COLS - x, Math.round(l.w)));
  const h = Math.max(MIN_H, Math.round(l.h));
  return { x, y, w, h };
}

export function overlaps(a: Layout, b: Layout): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/**
 * Resolve overlaps after `movedId` was placed, by pushing colliding cards DOWN.
 *
 * Why down-only: it's the one direction that always has room, so the operation
 * can't fail or bounce a card off-grid. Sideways shuffling looks clever and
 * feels random — you drop a card and something unrelated jumps across the
 * screen. Downward displacement is predictable, which matters more.
 *
 * Iterative rather than recursive so a cascade (A pushes B pushes C) settles,
 * with a hard iteration cap as a guard against pathological input.
 */
export function resolve(items: Placed[], movedId: string): Placed[] {
  const out = items.map((i) => ({ ...i }));
  const moved = out.find((i) => i.id === movedId);
  if (!moved) return out;

  for (let pass = 0; pass < 40; pass++) {
    let collided = false;
    // Stable order: settle nearer-the-top cards first so results are
    // deterministic regardless of the array's original ordering.
    const order = [...out].sort((a, b) => a.y - b.y || a.x - b.x);
    for (const item of order) {
      if (item.id === movedId) continue;
      if (overlaps(item, moved)) {
        item.y = moved.y + moved.h;
        collided = true;
      }
      for (const other of order) {
        if (other.id === item.id || other.id === movedId) continue;
        if (overlaps(item, other) && item.y >= other.y) {
          item.y = other.y + other.h;
          collided = true;
        }
      }
    }
    if (!collided) break;
  }
  return out;
}

/**
 * Close vertical gaps so cards float up into empty space.
 *
 * Without this, dragging a card out of the top row leaves a permanent hole and
 * the dashboard slowly drifts downward with every edit.
 */
export function compact(items: Placed[]): Placed[] {
  const sorted = [...items].sort((a, b) => a.y - b.y || a.x - b.x);
  const settled: Placed[] = [];
  for (const item of sorted) {
    const next = { ...item };
    while (next.y > 0) {
      const probe = { ...next, y: next.y - 1 };
      if (settled.some((s) => overlaps(probe, s))) break;
      next.y = probe.y;
    }
    settled.push(next);
  }
  return settled;
}

/** Only the cards whose geometry actually changed — keeps the write small and
 *  makes a no-op drag write nothing at all. */
export function changed(before: Placed[], after: Placed[]): Placed[] {
  const prev = new Map(before.map((b) => [b.id, b]));
  return after.filter((a) => {
    const p = prev.get(a.id);
    return !p || p.x !== a.x || p.y !== a.y || p.w !== a.w || p.h !== a.h;
  });
}
