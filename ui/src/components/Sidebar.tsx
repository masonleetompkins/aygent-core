// AYGENT sidebar nav (icon + label). SF-Symbol-style line glyphs (no emoji).
import type { ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

export type ScreenId =
  | "chat" | "agents" | "tools" | "settings" | "scheduler" | "connections" | "savepoints";

export interface NavItem { id: ScreenId; label: string; icon: IconName; enabled: boolean; }

// Phase-1 order. Only what's built is enabled; the rest show as "soon" so the
// product shape is visible without pretending features exist.
export const NAV: NavItem[] = [
  { id: "chat", label: "Chat", icon: "chat", enabled: true },
  { id: "agents", label: "Agents", icon: "agents", enabled: true },
  { id: "tools", label: "Tools", icon: "tools", enabled: true },
  { id: "scheduler", label: "Scheduler", icon: "scheduler", enabled: true },
  { id: "connections", label: "Connections", icon: "connections", enabled: true },
  { id: "savepoints", label: "Save Points", icon: "savepoints", enabled: true },
  { id: "settings", label: "Settings", icon: "settings", enabled: true },
];

export function Sidebar({ active, onSelect }: { active: ScreenId; onSelect: (id: ScreenId) => void }) {
  return (
    <nav style={{
      width: 220, flexShrink: 0, height: "100vh", boxSizing: "border-box",
      borderRight: "var(--border-width) solid var(--line)",
      background: "var(--surface)", boxShadow: "var(--elevation)",
      display: "flex", flexDirection: "column", padding: "18px 12px", gap: 4,
    }}>
      <div style={{ padding: "6px 10px 16px", fontWeight: 800, letterSpacing: "0.14em", fontSize: 18 }}>
        AYGENT
      </div>
      {NAV.map((item) => (
        <NavButton key={item.id} item={item} active={active === item.id} onSelect={onSelect} />
      ))}
      <div style={{ marginTop: "auto", padding: "10px", fontSize: 11, color: "var(--text-faint)" }}>
        v0.0.1 · Phase 1
      </div>
    </nav>
  );
}

function NavButton({ item, active, onSelect }: { item: NavItem; active: boolean; onSelect: (id: ScreenId) => void }): ReactNode {
  const dim = !item.enabled;
  return (
    <button
      onClick={() => item.enabled && onSelect(item.id)}
      disabled={dim}
      style={{
        display: "flex", alignItems: "center", gap: 10, width: "100%",
        padding: "9px 12px", borderRadius: "var(--radius-control)",
        border: `var(--border-width) solid ${active ? "var(--line)" : "transparent"}`,
        background: active ? "var(--bg)" : "transparent",
        boxShadow: active ? "var(--elevation)" : "none",
        color: dim ? "var(--text-faint)" : "var(--text)",
        cursor: dim ? "default" : "pointer",
        fontSize: 14, fontWeight: active ? 700 : 500, textAlign: "left",
        transition: "background 120ms ease, box-shadow 120ms ease",
      }}
    >
      <span style={{ width: 18, display: "flex", justifyContent: "center" }}><Icon name={item.icon as IconName} size={17} /></span>
      <span>{item.label}</span>
      {dim && <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--text-faint)" }}>soon</span>}
    </button>
  );
}
