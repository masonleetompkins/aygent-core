// AYGENT — Browser screen (BROWSER-ARCH Slice 1).
// The first time the in-app browser DOES something you can see: type a URL, hit
// Go, and Chromium (launched headless behind the scenes) navigates + returns a
// screenshot rendered right here. Slice 2 upgrades this still image to a live
// screencast; Slice 3 lets you click INTO it. For now: address bar + Go + the
// rendered page.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

type NavResult = { screenshot: string; url: string; title: string };

export function Browser() {
  const [installed, setInstalled] = useState<boolean | null>(null);
  const [addr, setAddr] = useState("");          // what's in the address bar
  const [page, setPage] = useState<NavResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    invoke<{ installed: boolean }>("browser_status")
      .then((s) => setInstalled(s.installed))
      .catch(() => setInstalled(false));
  }, []);

  async function go(target?: string) {
    const url = (target ?? addr).trim();
    if (!url) return;
    setLoading(true); setErr(null);
    try {
      const res = await invoke<NavResult>("browser_navigate", { url });
      setPage(res);
      setAddr(res.url); // reflect the final (post-redirect) URL
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoading(false);
    }
  }

  if (installed === false) {
    return (
      <Card title="Browser">
        <p style={{ color: "var(--text-muted)", fontSize: 14, margin: 0 }}>
          The in-app browser isn’t enabled yet. Turn it on in Settings — AYGENT downloads Chromium
          into its own space, just like a local model. Then come back here to browse.
        </p>
      </Card>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0, gap: 12 }}>
      <div>
        <div style={{ fontSize: "var(--text-h1)", fontWeight: 800, letterSpacing: "-0.01em" }}>Browser</div>
        <div style={{ color: "var(--text-faint)", fontSize: "var(--text-caption)" }}>
          A real browser inside AYGENT. Type a URL and go — your agents can use this too.
        </div>
      </div>

      {/* Address bar */}
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <div style={{ flex: 1 }}>
          <Input
            value={addr}
            onChange={(e: any) => setAddr(e.target.value)}
            onKeyDown={(e: any) => { if (e.key === "Enter") go(); }}
            placeholder="Enter a URL or search…"
          />
        </div>
        <Button onClick={() => go()} disabled={loading || !addr.trim()}>
          {loading ? "Loading…" : "Go"}
        </Button>
      </div>

      {err && <Pill tone="danger">✗ {err}</Pill>}

      {/* The rendered page (screenshot). Fills remaining height. */}
      <div style={{
        flex: 1, minHeight: 0, border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-card)", background: "var(--bg)", overflow: "auto",
        display: "flex", flexDirection: "column",
      }}>
        {page ? (
          <>
            {page.title && (
              <div style={{
                padding: "8px 12px", borderBottom: "var(--border-width) solid var(--line)",
                fontSize: 13, fontWeight: 600, color: "var(--text-muted)",
                whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis",
              }}>{page.title}</div>
            )}
            <img
              src={page.screenshot}
              alt={page.title || page.url}
              style={{ width: "100%", display: "block" }}
            />
          </>
        ) : (
          <div style={{
            flex: 1, display: "flex", alignItems: "center", justifyContent: "center",
            color: "var(--text-faint)", fontSize: 14,
          }}>
            {loading ? "Loading the page…" : "Enter a URL above to load a page."}
          </div>
        )}
      </div>
    </div>
  );
}
