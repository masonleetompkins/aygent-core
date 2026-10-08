// Shared model-list helpers (moved out of Agents.tsx, Mason 10-08 — the chat
// thread picker uses the same ranking + labels so both surfaces agree).

// Rank a model id most-powerful-first. Higher score = more capable = higher in
// the dropdown. Family tier dominates; version bumps break ties (opus-5 > opus-4-8).
// Provider-agnostic heuristic; unknown ids fall to the bottom but stay listed.
export function modelRank(id: string): number {
  const s = id.toLowerCase();
  let base = 0;
  if (s.includes("fable") || s.includes("mythos")) base = 900;      // next-gen top tier
  else if (s.includes("opus")) base = 800;
  else if (s.includes("gpt-5") || s.includes("o3") || s.includes("o1")) base = 780; // OpenAI reasoning/top
  else if (s.includes("sonnet")) base = 700;
  else if (s.includes("gpt-4")) base = 680;
  else if (s.includes("haiku")) base = 500;
  else if (s.includes("mini") || s.includes("small")) base = 400;
  else base = 300;
  // version nudge: pull a trailing version like "-5", "-4-8", "4.6" out of the id.
  const m = s.match(/(\d+)(?:[.-](\d+))?/g);
  let ver = 0;
  if (m) { const last = m[m.length - 1].replace(/[.-]/g, "."); const parts = last.split("."); ver = (parseInt(parts[0] || "0") * 10) + parseInt(parts[1] || "0"); }
  return base + ver;
}

// A short, human label for a model id (family + version), so the dropdown reads
// nicely instead of showing raw ids.
export function modelLabel(id: string): string {
  const s = id.toLowerCase();
  const fam =
    s.includes("fable") ? "Fable" : s.includes("mythos") ? "Mythos" :
    s.includes("opus") ? "Opus" : s.includes("sonnet") ? "Sonnet" : s.includes("haiku") ? "Haiku" :
    null;
  return fam ? `${fam} — ${id}` : id;
}
