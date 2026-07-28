// Playground — temporary home for the working Phase-0 proofs (jail probe, daemon
// self-test, agent tool-use run) so we don't lose them while the real screens
// (Chat, Agents, etc.) get built. Themed via tokens.
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill, Transcript } from "../components/ui";

type ProbeResult = { ok: boolean; resolved?: string; error?: string } | null;
const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

// M1.7 Slice 1 — vault memory read-path proof types (mirror memory.rs).
type IngestReport = {
  scanned: number; parsed: number; embedded: number;
  skipped_unchanged: number; links: number; errors: string[];
};
type RetrievedNote = {
  path: string; title: string; ntype: string;
  score: number; via: string; snippet: string;
};

export function Playground({ folder, ws, agentId }: { folder: string | null; ws: WebSocket | null; agentId: string | null }) {
  const [probePath, setProbePath] = useState("notes/hello.md");
  const [probe, setProbe] = useState<ProbeResult>(null);
  const [selftest, setSelftest] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("Create a file called hello.md with a short greeting, then read it back.");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // --- Memory test panel state (Slice 1) ---
  const [vaultPath, setVaultPath] = useState("");
  const [ingest, setIngest] = useState<IngestReport | null>(null);
  const [ingestErr, setIngestErr] = useState<string | null>(null);
  const [ingesting, setIngesting] = useState(false);
  const [query, setQuery] = useState("why am I so tired lately?");
  const [hits, setHits] = useState<RetrievedNote[] | null>(null);
  const [retrieveErr, setRetrieveErr] = useState<string | null>(null);
  const [retrieving, setRetrieving] = useState(false);

  // --- Slice 2: L1 episodic write (gate + append) ---
  const [gate, setGate] = useState<{ all_pass: boolean; files: { file: string; pass: boolean; why: string }[] } | null>(null);
  const [gateBusy, setGateBusy] = useState(false);

  // --- Slice 3: explicit L2 write + novelty dedup ---
  const [fact, setFact] = useState("Mason takes a break from weed for fertility reasons");
  const [ftype, setFtype] = useState("fact");
  const [remembered, setRemembered] = useState<{ action: string; path: string; title: string; similarity: number; matched_path: string; confidence: number }[]>([]);
  const [rememberErr, setRememberErr] = useState<string | null>(null);
  const [rememberBusy, setRememberBusy] = useState(false);

  // --- Slice 4: auto-capture + salience (the killer loop) ---
  const [turn, setTurn] = useState("I prefer dark roast coffee, no sugar. I switched to decaf a few weeks ago to sleep better. Also I decided to ship AYGENT at $20 one-time. Can you help me with the billing page?");
  const [capture, setCapture] = useState<{ candidates: number; created: number; reinforced: number; results: { action: string; path: string; title: string; similarity: number; matched_path: string; confidence: number }[] } | null>(null);
  const [captureErr, setCaptureErr] = useState<string | null>(null);
  const [captureBusy, setCaptureBusy] = useState(false);

  async function runCapture() {
    if (!agentId) { setCaptureErr("No active agent — pick one in the rail first."); return; }
    setCaptureBusy(true); setCaptureErr(null); setCapture(null);
    try { setCapture(await invoke("memory_auto_capture", { agentId, userText: turn })); }
    catch (e) { setCaptureErr(String(e)); }
    finally { setCaptureBusy(false); }
  }

  async function runRemember() {
    if (!agentId) { setRememberErr("No active agent — pick one in the rail first."); return; }
    setRememberBusy(true); setRememberErr(null);
    try {
      const r = await invoke<{ action: string; path: string; title: string; similarity: number; matched_path: string; confidence: number }>(
        "memory_remember", { agentId, text: fact, ntype: ftype });
      setRemembered((prev) => [r, ...prev].slice(0, 8));
    } catch (e) { setRememberErr(String(e)); }
    finally { setRememberBusy(false); }
  }
  const [entry, setEntry] = useState("10:24 shipped Slice 2 — the vault write path is real");
  const [appendMsg, setAppendMsg] = useState<string | null>(null);
  const [appendErr, setAppendErr] = useState<string | null>(null);
  const [appendBusy, setAppendBusy] = useState(false);

  async function runGate() {
    setGateBusy(true); setGate(null);
    try { setGate(await invoke("memory_gate_check")); }
    catch (e) { setGate({ all_pass: false, files: [{ file: "gate", pass: false, why: String(e) }] }); }
    finally { setGateBusy(false); }
  }
  async function runAppend() {
    if (!agentId) { setAppendErr("No active agent — pick one in the rail first."); return; }
    setAppendBusy(true); setAppendMsg(null); setAppendErr(null);
    const today = new Date();
    const date = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
    try {
      const r = await invoke<{ path: string; created: boolean; bytes_before: number; bytes_added: number; bytes_after: number }>(
        "memory_append_daily", { agentId, date, entry });
      setAppendMsg(`✓ ${r.created ? "created" : "appended to"} ${r.path} · +${r.bytes_added} bytes (${r.bytes_before}→${r.bytes_after})`);
    } catch (e) { setAppendErr(String(e)); }
    finally { setAppendBusy(false); }
  }

  async function runIngest() {
    if (!agentId) { setIngestErr("No active agent — pick one in the rail first."); return; }
    setIngesting(true); setIngest(null); setIngestErr(null);
    try { setIngest(await invoke<IngestReport>("memory_ingest", { agentId, vaultPath })); }
    catch (e) { setIngestErr(String(e)); }
    finally { setIngesting(false); }
  }
  async function runRetrieve() {
    if (!agentId) { setRetrieveErr("No active agent — pick one in the rail first."); return; }
    setRetrieving(true); setHits(null); setRetrieveErr(null);
    try { setHits(await invoke<RetrievedNote[]>("memory_retrieve", { agentId, query, topK: 3, expandHops: 1 })); }
    catch (e) { setRetrieveErr(String(e)); }
    finally { setRetrieving(false); }
  }

  async function runProbe() { setProbe(await invoke<ProbeResult>("broker_probe", { requested: probePath })); }
  function runDaemonSelftest() {
    setSelftest("running…");
    if (!ws) { setSelftest("daemon not connected"); return; }
    const handler = (ev: MessageEvent) => {
      let m: any; try { m = JSON.parse(ev.data); } catch { return; }
      if (m.type === "selftest:result") {
        const i = m.inside?.ok ? "ADMIT" : "refuse:" + m.inside?.error;
        const o = m.outside?.ok ? "ADMIT(!!)" : "refuse:" + m.outside?.error;
        setSelftest(`inside=${i}  ·  outside=${o}`);
        ws.removeEventListener("message", handler);
      }
    };
    ws.addEventListener("message", handler);
    ws.send(JSON.stringify({ type: "selftest" }));
  }
  async function call(cmd: string) {
    setBusy(true); setReply(null);
    try { setReply(await invoke<string>(cmd, { prompt })); }
    catch (e) { setReply("✗ " + String(e)); }
    finally { setBusy(false); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 620 }}>
      <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Playground</h2>
      <p style={{ ...hint, marginTop: -8 }}>Temporary — the Phase-0 security + agent proofs. These move into Chat/Agents as those screens land.</p>

      {!folder && <Pill tone="muted">Pick an Agent Folder in Settings first.</Pill>}

      {folder && (
        <Card title="Probe the jail">
          <p style={hint}>Type a path; the broker admits or refuses it.</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input mono value={probePath} onChange={(e) => setProbePath(e.target.value)} placeholder="notes/hello.md  or  ../../etc/passwd" />
            <Button onClick={runProbe}>Probe</Button>
          </div>
          {probe && <Pill tone={probe.ok ? "ok" : "danger"}>{probe.ok ? `✓ admitted → ${probe.resolved}` : `✗ refused → ${probe.error}`}</Pill>}
          <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 10 }}>
            <p style={hint}>Test the jail from the <b>daemon's</b> side:</p>
            <div><Button variant="secondary" onClick={runDaemonSelftest} disabled={!ws}>Test jail from daemon</Button></div>
            {selftest && <Transcript text={selftest} />}
          </div>
        </Card>
      )}

      {folder && (
        <Card title="Run the agent">
          <p style={hint}>The agent can use jailed file tools (read/write/list) inside your folder.</p>
          <Input value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          <div style={{ display: "flex", gap: 8 }}>
            <Button onClick={() => call("agent_run")} disabled={busy}>{busy ? "…" : "Run agent (tool use)"}</Button>
            <Button variant="secondary" onClick={() => call("anthropic_test")} disabled={busy}>{busy ? "…" : "Plain completion"}</Button>
          </div>
          {reply && <Transcript text={reply} error={reply.startsWith("✗")} />}
        </Card>
      )}

      {/* M1.7 Slice 1 — Vault Memory read path (ingest + graph-expansion retrieval). */}
      <Card title="Vault memory (Slice 1 — read path)">
        <p style={hint}>
          Point at a vault folder, ingest it (parse links + embed <b>in-process</b> via the
          built-in engine — AYGENT auto-downloads the tiny embedding model on first use,
          no install, no Ollama), then ask a question. Success = a query whose literal
          words match almost nothing still surfaces the right notes — some <code>semantic</code>,
          some pulled in via <code>graph:</code> expansion. Zero vault writes.
        </p>
        <p style={{ ...hint, fontSize: 12 }}>
          First ingest downloads the ~85MB embedding model into AYGENT’s own folder — that one-time
          fetch can take a moment; after that it’s instant and fully offline.
        </p>
        {!agentId && <Pill tone="muted">Pick an agent in the rail first (memory is scoped to it).</Pill>}

        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <p style={hint}>Vault folder (absolute path — e.g. the bundled <code>aygent/test-vault</code>):</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input mono value={vaultPath} onChange={(e) => setVaultPath(e.target.value)}
              placeholder="/Users/you/Documents/aygent/test-vault" />
            <Button variant="secondary" onClick={async () => {
              const p = await invoke<string | null>("pick_vault_folder");
              if (p) setVaultPath(p);
            }}>Browse…</Button>
            <Button onClick={runIngest} disabled={ingesting || !agentId || !vaultPath}>
              {ingesting ? "Ingesting…" : "Ingest"}
            </Button>
          </div>
          {ingest && (
            <Pill tone={ingest.errors.length ? "danger" : "ok"}>
              {`✓ scanned ${ingest.scanned} · parsed ${ingest.parsed} · embedded ${ingest.embedded} · skipped ${ingest.skipped_unchanged} · links ${ingest.links}`}
              {ingest.errors.length ? ` · ${ingest.errors.length} error(s)` : ""}
            </Pill>
          )}
          {ingest && ingest.errors.length > 0 && <Transcript text={ingest.errors.join("\n")} error />}
          {ingestErr && <Transcript text={"✗ " + ingestErr} error />}
        </div>

        <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          <p style={hint}>Ask the memory:</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="why am I so tired lately?" />
            <Button onClick={runRetrieve} disabled={retrieving || !agentId}>
              {retrieving ? "…" : "Retrieve"}
            </Button>
          </div>
          {retrieveErr && <Transcript text={"✗ " + retrieveErr} error />}
          {hits && hits.length === 0 && <Pill tone="muted">No hits — ingest a vault first.</Pill>}
          {hits && hits.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {hits.map((h, i) => (
                <div key={i} style={{
                  border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
                  padding: "10px 12px", background: "var(--bg)", boxShadow: "var(--elevation)",
                }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                    <b style={{ fontSize: 14 }}>{h.title || h.path}</b>
                    <Pill tone={h.via === "semantic" ? "ok" : "muted"}>
                      {h.via === "semantic" ? `semantic ${h.score.toFixed(2)}` : `↳ ${h.via}`}
                    </Pill>
                    <span style={{ ...hint, fontSize: 12 }}>{h.ntype} · {h.path}</span>
                  </div>
                  <p style={{ ...hint, marginTop: 6 }}>{h.snippet}</p>
                </div>
              ))}
            </div>
          )}
        </div>
      </Card>

      {/* M1.7 Slice 2 — L1 episodic WRITE (append to daily note). Gate-guarded. */}
      <Card title="Vault memory (Slice 2 — episodic write)">
        <p style={hint}>
          The first real write to a vault: <b>append</b> a dated line to today’s daily note.
          Guarded by a <b>byte-stability gate</b> — the append can only ever add to the tail;
          it can never modify a byte of what’s already there (the &quot;never corrupt your vault&quot;
          invariant). Run the gate first; it must be all-green before you trust a write.
        </p>

        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <div><Button variant="secondary" onClick={runGate} disabled={gateBusy}>
            {gateBusy ? "Checking…" : "Run byte-stability gate"}
          </Button></div>
          {gate && (
            <>
              <Pill tone={gate.all_pass ? "ok" : "danger"}>
                {gate.all_pass ? "✓ GATE GREEN — all corpus files preserve their prefix" : "✗ GATE FAILED — do NOT enable writes"}
              </Pill>
              <Transcript error={!gate.all_pass}
                text={gate.files.map((f) => `${f.pass ? "✓" : "✗"} ${f.file} — ${f.why}`).join("\n")} />
            </>
          )}
        </div>

        <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          <p style={hint}>Append an episodic line to today’s <code>Daily/YYYY-MM-DD.md</code> (created if missing):</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input value={entry} onChange={(e) => setEntry(e.target.value)} placeholder="what happened…" />
            <Button onClick={runAppend} disabled={appendBusy || !agentId}>
              {appendBusy ? "…" : "Append"}
            </Button>
          </div>
          {appendMsg && <Pill tone="ok">{appendMsg}</Pill>}
          {appendErr && <Transcript text={"✗ " + appendErr} error />}
        </div>
      </Card>

      {/* M1.7 Slice 3 — explicit L2 write ("remember this") + novelty dedup. */}
      <Card title="Vault memory (Slice 3 — remember + dedup)">
        <p style={hint}>
          “Remember this” → a durable single-idea note in <code>Memory/</code> with typed frontmatter,
          embedded + linked. The <b>novelty gate</b>: say the SAME fact twice and you get
          <b> one</b> memory, <i>reinforced</i> (confidence bumped) — not a duplicate. Try clicking
          <b> Remember</b> twice on the same text, then tweak the wording and try again.
        </p>
        {!agentId && <Pill tone="muted">Pick an agent in the rail first.</Pill>}
        <div style={{ display: "flex", gap: 8 }}>
          <Input value={fact} onChange={(e) => setFact(e.target.value)} placeholder="a durable fact / preference / decision…" />
          <select value={ftype} onChange={(e) => setFtype(e.target.value)}
            style={{ background: "var(--bg)", color: "var(--text)", border: "var(--border-width) solid var(--line)",
                     borderRadius: "var(--radius-control)", padding: "0 10px", fontSize: 13 }}>
            {["fact", "preference", "decision", "person", "project", "note"].map((t) => <option key={t} value={t}>{t}</option>)}
          </select>
          <Button onClick={runRemember} disabled={rememberBusy || !agentId}>{rememberBusy ? "…" : "Remember"}</Button>
        </div>
        {rememberErr && <Transcript text={"✗ " + rememberErr} error />}
        {remembered.length > 0 && (
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {remembered.map((r, i) => (
              <Pill key={i} tone={r.action === "created" ? "ok" : "muted"}>
                {r.action === "created"
                  ? `➕ created ${r.path} · conf ${r.confidence.toFixed(2)}${r.similarity > 0 ? ` (nearest ${r.similarity.toFixed(2)})` : ""}`
                  : `♻ reinforced ${r.matched_path} · sim ${r.similarity.toFixed(2)} → conf ${r.confidence.toFixed(2)}`}
              </Pill>
            ))}
          </div>
        )}
      </Card>

      {/* M1.7 Slice 4 — AUTO-CAPTURE + salience. The Self-Gardening killer loop. */}
      <Card title="Vault memory (Slice 4 — auto-capture)">
        <p style={hint}>
          Paste a chunk of conversation as if you just said it. The agent extracts the
          <b> durable facts</b> on its own — salience-gated (it ignores questions + transient
          states) and novelty-deduped (never a dupe). This is the unprompted Self-Gardening loop:
          you talk, it quietly files what matters, into files you own.
        </p>
        {!agentId && <Pill tone="muted">Pick an agent in the rail first.</Pill>}
        <textarea value={turn} onChange={(e) => setTurn(e.target.value)} rows={4}
          style={{ background: "var(--bg)", color: "var(--text)", border: "var(--border-width) solid var(--line)",
                   borderRadius: "var(--radius-control)", padding: "9px 12px", fontSize: 14, resize: "vertical",
                   fontFamily: "inherit", boxShadow: "var(--elevation)" }} />
        <div><Button onClick={runCapture} disabled={captureBusy || !agentId}>{captureBusy ? "Capturing…" : "Auto-capture"}</Button></div>
        {captureErr && <Transcript text={"✗ " + captureErr} error />}
        {capture && (
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            <Pill tone={capture.created + capture.reinforced > 0 ? "ok" : "muted"}>
              {`${capture.candidates} candidate(s) → ➕ ${capture.created} created · ♻ ${capture.reinforced} reinforced`}
            </Pill>
            {capture.results.map((r, i) => (
              <div key={i} style={{ border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
                                    padding: "8px 12px", background: "var(--bg)", boxShadow: "var(--elevation)" }}>
                <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                  <Pill tone={r.action === "created" ? "ok" : "muted"}>{r.action === "created" ? "➕ created" : "♻ reinforced"}</Pill>
                  <span style={{ ...hint, fontSize: 12 }}>{r.action === "created" ? r.path : r.matched_path} · conf {r.confidence.toFixed(2)}</span>
                </div>
                <p style={{ ...hint, marginTop: 4 }}>{r.title}</p>
              </div>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
