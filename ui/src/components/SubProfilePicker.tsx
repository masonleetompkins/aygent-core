// SubProfilePicker — Agents form dropdown (slice 4, UNWIRED).
//
// Shows profiles for the chosen subscription provider + failover order note.
// Props: provider ('claude-code'|'codex'), value (profile id), onChange.
// Agents.tsx wiring: render when provider is subscription kind (2-line diff).
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Profile = { id: string; kind: string; label: string; has_token: boolean };

export function SubProfilePicker({ provider, value, onChange }: {
  provider: string; value: string; onChange: (id: string) => void;
}) {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  useEffect(() => {
    if (provider !== "claude-code" && provider !== "codex") return;
    invoke<Profile[]>("sub_profiles_list", { kind: provider })
      .then((l) => {
        setProfiles(l || []);
        if (!value && (l || []).length === 1 && (l as Profile[])[0].has_token) onChange((l as Profile[])[0].id);
      })
      .catch(() => setProfiles([]));
    /* eslint-disable-next-line */
  }, [provider]);
  if (provider !== "claude-code" && provider !== "codex") return null;
  if (profiles.length === 0) {
    return <span style={{ fontSize: 12, color: "var(--text-faint)" }}>No {provider} profiles yet — add one in Settings → Subscriptions.</span>;
  }
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 6, fontSize: 13, fontWeight: 600, flex: 1 }}>
      Subscription profile
      <select value={value} onChange={(e) => onChange(e.target.value)} style={{
        padding: "9px 12px", borderRadius: "var(--radius-control)",
        border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)",
      }}>
        <option value="">Auto (first healthy — failover in order)</option>
        {profiles.map((p) => (
          <option key={p.id} value={p.id}>{p.label}{p.has_token ? "" : " (no token)"}</option>
        ))}
      </select>
      <span style={{ fontSize: 12, color: "var(--text-faint)" }}>
        Exhausted? Auto-tries the next profile. $0 billed — API value still shown.
      </span>
    </label>
  );
}
