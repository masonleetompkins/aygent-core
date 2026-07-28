// Scheduler — where AYGENT becomes proactive. List an agent's schedules
// (next/last/status), build new ones with NO cron string in sight (the $20-buyer
// path), pause/resume individually or globally, and drill into run history.
//
// Backs onto the Rust scheduler (M1.8): the ticker fires each due schedule
// headless via the drainer. Two kinds a user builds here:
//   • Scheduled job  — daily/weekly at a wall-clock time (fresh turn)
//   • Heartbeat      — every N minutes, keeps recent conversation context
// Cost ceiling + fire caps live under "Advanced" (sensible defaults prefilled).
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

type Schedule = {
  id: number;
  agent_id: string;
  name: string;
  kind: string;                 // 'cron' | 'heartbeat' | 'interval'
  spec_json: string;
  action_json: string;
  enabled: boolean;
  next_fire_at: number | null;  // UTC ms
  last_fired_at: number | null;
  daily_fire_count: number;
  last_status: string | null;
  last_result: string | null;
};
type Run = {
  id: number; fired_at: number; started_at: number | null; finished_at: number | null;
  state: string; skip_reason: string | null; result_snippet: string | null;
};

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function fmtWhen(ms: number | null): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const t = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return sameDay ? `today ${t}` : `${d.toLocaleDateString([], { month: "short", day: "numeric" })} ${t}`;
}

// Turn the stored spec JSON into a human timing summary for the list.
function specSummary(kind: string, specJson: string): string {
  try {
    const s = JSON.parse(specJson);
    if (s.kind === "interval") {
      const mins = Math.round((s.every_secs ?? 0) / 60);
      return kind === "heartbeat" ? `Heartbeat · every ${mins} min` : `Every ${mins} min`;
    }
    if (s.kind === "daily_at") return `Daily ${fmtHHMM(s.hh, s.mm)}`;
    if (s.kind === "weekly_at") {
      const days = (s.days ?? []).map((d: number) => DAYS[d]).join(", ");
      return `Weekly (${days}) ${fmtHHMM(s.hh, s.mm)}`;
    }
  } catch { /* fall through */ }
  return kind;
}
function fmtHHMM(hh: number, mm: number): string {
  const d = new Date(); d.setHours(hh, mm, 0, 0);
  return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}
function statusTone(s: string | null): "ok" | "danger" | "muted" {
  if (s === "ok") return "ok";
  if (s === "error") return "danger";
  return "muted";
}

export function Scheduler({ agentId }: { agentId: string | null }) {
  const [list, setList] = useState<Schedule[]>([]);
  const [paused, setPaused] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [openRuns, setOpenRuns] = useState<number | null>(null);
  const [runs, setRuns] = useState<Run[]>([]);

  async function refresh() {
    try {
      setList(await invoke<Schedule[]>("scheduler_list", { agentId: agentId ?? null }));
      setPaused(await invoke<boolean>("scheduler_get_paused"));
    } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { refresh(); const t = setInterval(refresh, 5000); return () => clearInterval(t); }, [agentId]);

  async function togglePauseAll() {
    try { const p = await invoke<boolean>("scheduler_set_paused", { paused: !paused }); setPaused(p); refresh(); }
    catch (e) { setErr(String(e)); }
  }
  async function toggleOne(s: Schedule) {
    try { await invoke("scheduler_set_enabled", { id: s.id, enabled: !s.enabled }); refresh(); }
    catch (e) { setErr(String(e)); }
  }
  async function remove(s: Schedule) {
    try { await invoke("scheduler_delete", { id: s.id }); refresh(); }
    catch (e) { setErr(String(e)); }
  }
  async function runNow(s: Schedule) {
    setErr(null);
    try {
      const enq = await invoke<boolean>("scheduler_run_now", { id: s.id });
      setErr(enq ? `Fired “${s.name}” now — check the agent’s chat + History in a few seconds.` : `“${s.name}” ran but enqueued nothing (a guard blocked it, or it’s a system job).`);
      setTimeout(refresh, 1500);
    } catch (e) { setErr(String(e)); }
  }
  async function showRuns(id: number) {
    if (openRuns === id) { setOpenRuns(null); return; }
    setOpenRuns(id);
    try { setRuns(await invoke<Run[]>("scheduler_runs", { scheduleId: id, limit: 10 })); }
    catch (e) { setErr(String(e)); }
  }


  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 720 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Scheduler</h2>
        <div style={{ display: "flex", gap: 8 }}>
          <Button variant="secondary" onClick={refresh}>↻ Refresh</Button>
          <Button variant="secondary" onClick={togglePauseAll}>
            {paused ? "▶ Resume all" : "⏸ Pause all"}
          </Button>
          <Button onClick={() => setAdding(true)} disabled={!agentId}>+ New schedule</Button>
        </div>
      </div>
      <p style={{ ...hint, marginTop: -8 }}>
        AYGENT runs these on its own — daily briefings, periodic checks, nightly jobs — even while you’re away.
      </p>
      {paused && <Pill tone="danger">All schedules paused — nothing will fire until you resume.</Pill>}
      {!agentId && <Pill tone="muted">Pick an agent in the rail to add a schedule.</Pill>}
      {err && <Pill tone="danger">{err}</Pill>}

      {adding && agentId && (
        <AddSchedule agentId={agentId} onClose={() => setAdding(false)} onSaved={() => { setAdding(false); refresh(); }} />
      )}

      {list.length === 0 && !adding && (
        <Card title="No schedules yet">
          <p style={hint}>Create one — e.g. “every morning at 8:00, give me a briefing.” It’ll run on its own.</p>
        </Card>
      )}

      {list.map((s) => (
        <Card key={s.id} title={undefined}>
          <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
            <span style={{ fontSize: 16 }}>{s.enabled ? "🟢" : "⚪"}</span>
            <b style={{ fontSize: 15 }}>{s.name}</b>
            <Pill tone="muted">{specSummary(s.kind, s.spec_json)}</Pill>
            <span style={{ ...hint, fontSize: 13 }}>next: {s.enabled ? fmtWhen(s.next_fire_at) : "—"}</span>
            {s.last_status && <Pill tone={statusTone(s.last_status)}>{s.last_status}</Pill>}
            <div style={{ marginLeft: "auto", display: "flex", gap: 6 }}>
              <Button onClick={() => runNow(s)}>▶ Run now</Button>
              <Button variant="secondary" onClick={() => toggleOne(s)}>{s.enabled ? "Pause" : "Resume"}</Button>
              <Button variant="secondary" onClick={() => showRuns(s.id)}>History</Button>
              <Button variant="secondary" onClick={() => remove(s)}>Delete</Button>
            </div>
          </div>
          {s.last_result && <p style={{ ...hint, marginTop: 8 }}>Last: {s.last_result}</p>}
          {openRuns === s.id && (
            <div style={{ marginTop: 10, borderTop: "var(--border-width) solid var(--line)", paddingTop: 10, display: "flex", flexDirection: "column", gap: 4 }}>
              {runs.length === 0 && <p style={hint}>No runs yet.</p>}
              {runs.map((r) => (
                <div key={r.id} style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 13 }}>
                  <Pill tone={r.state === "ok" ? "ok" : r.state === "error" ? "danger" : "muted"}>{r.state}{r.skip_reason ? `:${r.skip_reason}` : ""}</Pill>
                  <span style={hint}>{fmtWhen(r.fired_at)}</span>
                  {r.result_snippet && <span style={{ ...hint, opacity: 0.8 }}>· {r.result_snippet}</span>}
                </div>
              ))}
            </div>
          )}
        </Card>
      ))}
    </div>
  );
}

// ---- Add flow — no cron string in sight (the simple path) -----------------
function AddSchedule({ agentId, onClose, onSaved }: { agentId: string; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState("Morning briefing");
  const [whenKind, setWhenKind] = useState<"daily" | "weekly" | "interval">("daily");
  const [hh, setHh] = useState(8);
  const [mm, setMm] = useState(0);
  const [days, setDays] = useState<number[]>([1, 2, 3, 4, 5]);
  const [everyMin, setEveryMin] = useState(30);
  const [doWhat, setDoWhat] = useState<"prompt" | "job">("prompt");
  const [prompt, setPrompt] = useState("Give me my briefing.");
  const [keepContext, setKeepContext] = useState(false);
  const [job, setJob] = useState("distill");
  const [advanced, setAdvanced] = useState(false);
  const [maxFires, setMaxFires] = useState(2);
  const [costCeiling, setCostCeiling] = useState<string>("");
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  function toggleDay(d: number) {
    setDays((prev) => prev.includes(d) ? prev.filter((x) => x !== d) : [...prev, d].sort());
  }

  async function save() {
    setSaving(true); setErr(null);
    // Build the typed `when` spec the Rust enum expects.
    let when: any;
    if (whenKind === "interval") when = { kind: "interval", every_secs: Math.max(1, everyMin) * 60 };
    else if (whenKind === "daily") when = { kind: "daily_at", hh, mm };
    else when = { kind: "weekly_at", days, hh, mm };
    // Build the action.
    const action = doWhat === "prompt"
      ? { kind: "agent_turn", prompt_template: prompt, context: keepContext ? "continue" : "fresh" }
      : { kind: "system_job", job };
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone || "local";
    try {
      await invoke("scheduler_create", {
        agentId, name, when, action, tz,
        maxFiresPerDay: maxFires,
        maxCostUnitsPerDay: costCeiling.trim() === "" ? null : Math.max(0, parseInt(costCeiling, 10) || 0),
      });
      onSaved();
    } catch (e) { setErr(String(e)); } finally { setSaving(false); }
  }

  const seg = (active: boolean): React.CSSProperties => ({
    padding: "7px 14px", borderRadius: "var(--radius-control)", cursor: "pointer", fontSize: 14, fontWeight: 600,
    border: "var(--border-width) solid var(--line)",
    background: active ? "var(--accent)" : "var(--surface)", color: active ? "var(--bg)" : "var(--text)",
  });

  return (
    <Card title="New schedule">
      <label style={hint}>Name</label>
      <Input value={name} onChange={(e) => setName(e.target.value)} />

      <label style={{ ...hint, marginTop: 6 }}>When</label>
      <div style={{ display: "flex", gap: 8 }}>
        <span style={seg(whenKind === "interval")} onClick={() => setWhenKind("interval")}>Every N min</span>
        <span style={seg(whenKind === "daily")} onClick={() => setWhenKind("daily")}>Daily</span>
        <span style={seg(whenKind === "weekly")} onClick={() => setWhenKind("weekly")}>Weekly</span>
      </div>

      {whenKind === "interval" && (
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span style={hint}>every</span>
          <Input value={String(everyMin)} onChange={(e) => setEveryMin(parseInt(e.target.value, 10) || 0)} style={{ maxWidth: 90 }} />
          <span style={hint}>minutes</span>
        </div>
      )}
      {(whenKind === "daily" || whenKind === "weekly") && (
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <span style={hint}>at</span>
          <Input value={String(hh)} onChange={(e) => setHh(Math.min(23, Math.max(0, parseInt(e.target.value, 10) || 0)))} style={{ maxWidth: 64 }} />
          <span style={hint}>:</span>
          <Input value={String(mm).padStart(2, "0")} onChange={(e) => setMm(Math.min(59, Math.max(0, parseInt(e.target.value, 10) || 0)))} style={{ maxWidth: 64 }} />
          <span style={{ ...hint, fontSize: 12 }}>(24h, your local time)</span>
        </div>
      )}
      {whenKind === "weekly" && (
        <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
          {DAYS.map((d, i) => (
            <span key={i} style={seg(days.includes(i))} onClick={() => toggleDay(i)}>{d}</span>
          ))}
        </div>
      )}

      <label style={{ ...hint, marginTop: 6 }}>Do what</label>
      <div style={{ display: "flex", gap: 8 }}>
        <span style={seg(doWhat === "prompt")} onClick={() => setDoWhat("prompt")}>Send a prompt</span>
        <span style={seg(doWhat === "job")} onClick={() => setDoWhat("job")}>Run a built-in job</span>
      </div>
      {doWhat === "prompt" && (
        <>
          <Input value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="e.g. Give me my morning briefing" />
          <label style={{ display: "flex", gap: 8, alignItems: "center", ...hint }}>
            <input type="checkbox" checked={keepContext} onChange={(e) => setKeepContext(e.target.checked)} />
            Keep conversation context (heartbeat style — it sees recent chat)
          </label>
        </>
      )}
      {doWhat === "job" && (
        <select value={job} onChange={(e) => setJob(e.target.value)}
          style={{ background: "var(--bg)", color: "var(--text)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "9px 12px", fontSize: 14 }}>
          <option value="distill">Nightly memory distill (Slice 5)</option>
          <option value="reconcile">Weekly memory cleanup (Slice 5)</option>
        </select>
      )}

      <button onClick={() => setAdvanced(!advanced)} style={{ ...hint, background: "none", border: "none", cursor: "pointer", textAlign: "left", padding: 0, marginTop: 6 }}>
        {advanced ? "▾" : "▸"} Advanced (limits & cost ceiling)
      </button>
      {advanced && (
        <div style={{ display: "flex", flexDirection: "column", gap: 8, paddingLeft: 8 }}>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <span style={hint}>Max fires per day</span>
            <Input value={String(maxFires)} onChange={(e) => setMaxFires(parseInt(e.target.value, 10) || 1)} style={{ maxWidth: 80 }} />
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <span style={hint}>Daily cost ceiling (blank = none)</span>
            <Input value={costCeiling} onChange={(e) => setCostCeiling(e.target.value)} style={{ maxWidth: 120 }} placeholder="units/day" />
          </div>
          <p style={{ ...hint, fontSize: 12 }}>Hitting the ceiling auto-pauses this schedule for the day — the surprise-bill circuit breaker.</p>
        </div>
      )}

      {err && <Pill tone="danger">{err}</Pill>}
      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <Button onClick={save} disabled={saving}>{saving ? "Saving…" : "Create schedule"}</Button>
        <Button variant="secondary" onClick={onClose}>Cancel</Button>
      </div>
    </Card>
  );
}
