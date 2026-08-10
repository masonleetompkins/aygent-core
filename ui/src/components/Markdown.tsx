// AYGENT — Markdown renderer (dependency-free).
//
// The agent replies in markdown; we render it so **bold**, *italic*, `code`,
// code fences, headers, and lists actually format. This is a small, SAFE
// subset parser: we never use dangerouslySetInnerHTML — every node is a real
// React element, so there's no HTML-injection surface from model output.
//
// Shared across surfaces (Chat now; any future transcript/preview reuses it).
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

// FILE PATHS ARE CLICKABLE (Mason 08-02): any file path the agent mentions in
// a reply -- in prose OR inside `code` -- opens Finder at that location, the
// same reveal_in_finder command the tool-call cards already use. Resolved
// THROUGH THE JAIL BROKER on the Rust side (Mode::Read, "default" scope, same
// as ToolCard) -- an out-of-scope path is refused there, not here; this is
// just the UI trigger.
async function revealPath(p: string) {
  try { await invoke("reveal_in_finder", { path: p }); }
  catch (err) { alert("Couldn't open in Finder: " + String(err)); }
}

function PathLink({ path, mono }: { path: string; mono?: boolean }) {
  return (
    <button
      onClick={() => void revealPath(path)}
      title={`Reveal "${path}" in Finder`}
      style={{
        font: "inherit", fontFamily: mono ? inlineCode.fontFamily : "inherit",
        fontSize: mono ? inlineCode.fontSize : "inherit",
        background: mono ? inlineCode.background : "none",
        padding: mono ? inlineCode.padding : 0,
        borderRadius: mono ? inlineCode.borderRadius : 0,
        border: mono ? inlineCode.border : "none",
        color: "var(--accent)", cursor: "pointer", textDecoration: "underline",
        textUnderlineOffset: 2, textAlign: "left",
      }}
    >{path}</button>
  );
}

// A token "looks like a file path" if it has a directory separator + a
// plausible extension, OR is a bare filename with a recognized extension.
// Deliberately conservative (an allowlist of extensions, not "any dot") so we
// don't misfire on version numbers, sentence-ending abbreviations, etc.
const PATH_EXT = "md|txt|json|jsonc|ts|tsx|js|jsx|rs|py|toml|yaml|yml|css|scss|html|pdf|png|jpe?g|gif|webp|svg|csv|log|sh|lock|app|zip|db|sqlite|mp3|mp4|wav|m4a|webm|plist|conf|cfg|ini";
const PATH_WITH_SLASH_RE = new RegExp(`^\.{0,2}\/?(?:[\w.-]+\/)+[\w.-]+\.(?:${PATH_EXT})\b`, "i");
const BARE_FILE_RE = new RegExp(`^[\w-]+\.(?:${PATH_EXT})\b`, "i");

// Inline formatting: **bold**, *italic* / _italic_, `code`, [text](url).
// Parsed with a single tokenizer pass so nesting like **bold `code`** works.
function renderInline(text: string, keyBase: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let i = 0;
  let k = 0;
  const push = (n: ReactNode) => nodes.push(<span key={`${keyBase}-${k++}`}>{n}</span>);

  // Regex for the next inline token from position i.
  const patterns: { re: RegExp; render: (m: RegExpExecArray) => ReactNode }[] = [
    { re: /^\*\*([^*]+)\*\*/, render: (m) => <strong>{renderInline(m[1], `${keyBase}-b${k}`)}</strong> },
    { re: /^__([^_]+)__/, render: (m) => <strong>{renderInline(m[1], `${keyBase}-b2${k}`)}</strong> },
    { re: /^\*([^*]+)\*/, render: (m) => <em>{renderInline(m[1], `${keyBase}-i${k}`)}</em> },
    { re: /^_([^_]+)_/, render: (m) => <em>{renderInline(m[1], `${keyBase}-i2${k}`)}</em> },
    // `code` spans: if the content LOOKS LIKE A FILE PATH, make it a clickable
    // Finder-reveal link (monospaced, same look as before) instead of inert code.
    {
      re: /^`([^`]+)`/,
      render: (m) => (PATH_WITH_SLASH_RE.test(m[1]) || BARE_FILE_RE.test(m[1]))
        ? <PathLink path={m[1]} mono />
        : <code style={inlineCode}>{m[1]}</code>,
    },
    {
      re: /^\[([^\]]+)\]\(([^)]+)\)/,
      render: (m) => <a href={m[2]} style={link} target="_blank" rel="noreferrer">{m[1]}</a>,
    },
    // Bare paths mentioned in PLAIN PROSE (not backtick-wrapped): a directory-
    // style path (has a `/`) OR a bare filename with a recognized extension.
    // Tried near the end of the pattern list so it never steals a token that a
    // more specific rule (bold/italic/code/link) should have claimed first.
    { re: PATH_WITH_SLASH_RE, render: (m) => <PathLink path={m[0]} /> },
    { re: BARE_FILE_RE, render: (m) => <PathLink path={m[0]} /> },
  ];

  let buf = "";
  const flush = () => { if (buf) { push(buf); buf = ""; } };

  while (i < text.length) {
    const rest = text.slice(i);
    let matched = false;
    for (const { re, render } of patterns) {
      const m = re.exec(rest);
      if (m) {
        flush();
        push(render(m));
        i += m[0].length;
        matched = true;
        break;
      }
    }
    if (!matched) { buf += text[i]; i += 1; }
  }
  flush();
  return nodes;
}

// GFM TABLE support. The agent (esp. whoami / connected-account summaries)
// emits pipe tables; without this they render as a paragraph of raw `|` pipes
// (Mason 08-10: the connected-accounts table was unreadable). A table is a
// header row of `| a | b |`, a SEPARATOR row (`|---|:--:|---|` — dashes with
// optional leading/trailing colons for alignment), then zero+ data rows. The
// separator is the signature that tells a real table apart from a paragraph
// that merely contains pipes, so we require it.

/// Is this line a GFM table separator row? Each cell is dashes with optional
/// alignment colons, e.g. `---`, `:--`, `--:`, `:-:`. Must have >=1 cell.
function isTableSeparator(line: string): boolean {
  const t = line.trim();
  if (!t.includes("-") || !t.includes("|")) return false;
  const cells = splitRow(t);
  return cells.length > 0 && cells.every((c) => /^:?-+:?$/.test(c.trim()));
}

/// Split a pipe row into cell strings. Tolerates optional leading/trailing `|`
/// and escaped pipes (`\|`) inside a cell. Trims each cell.
function splitRow(line: string): string[] {
  let t = line.trim();
  if (t.startsWith("|")) t = t.slice(1);
  if (t.endsWith("|")) t = t.slice(0, -1);
  const cells: string[] = [];
  let buf = "";
  for (let j = 0; j < t.length; j++) {
    const ch = t[j];
    if (ch === "\\" && t[j + 1] === "|") { buf += "|"; j++; continue; }
    if (ch === "|") { cells.push(buf.trim()); buf = ""; continue; }
    buf += ch;
  }
  cells.push(buf.trim());
  return cells;
}

/// Column alignments parsed from the separator row (`:--`=left, `--:`=right,
/// `:-:`=center, else undefined).
function parseAligns(sep: string): (("left" | "right" | "center") | undefined)[] {
  return splitRow(sep).map((c) => {
    const t = c.trim();
    const l = t.startsWith(":"), r = t.endsWith(":");
    if (l && r) return "center";
    if (r) return "right";
    if (l) return "left";
    return undefined;
  });
}

// Block-level: split into paragraphs, code fences, headings, list groups, and
// GFM tables.
export function Markdown({ text }: { text: string }) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let key = 0;

  while (i < lines.length) {
    const line = lines[i];

    // Fenced code block ``` ... ```
    if (line.trim().startsWith("```")) {
      const buf: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trim().startsWith("```")) { buf.push(lines[i]); i += 1; }
      i += 1; // consume closing fence
      blocks.push(<pre key={`c${key++}`} style={codeBlock}>{buf.join("\n")}</pre>);
      continue;
    }

    // Heading  #..###### 
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) {
      const level = h[1].length;
      const size = [22, 19, 17, 16, 15, 14][level - 1];
      blocks.push(
        <div key={`h${key++}`} style={{ fontWeight: 800, fontSize: size, margin: "6px 0 2px" }}>
          {renderInline(h[2], `h${key}`)}
        </div>
      );
      i += 1;
      continue;
    }

    // GFM table: a `|`-bearing header line whose NEXT line is a separator row.
    // (The separator requirement is what keeps a plain paragraph-with-pipes
    // from being mistaken for a table.)
    if (
      line.includes("|") &&
      i + 1 < lines.length &&
      isTableSeparator(lines[i + 1])
    ) {
      const header = splitRow(line);
      const aligns = parseAligns(lines[i + 1]);
      i += 2; // consume header + separator
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim() !== "") {
        // Stop if we hit another block starter (heading/fence/list) defensively.
        if (lines[i].trim().startsWith("```") || /^(#{1,6})\s+/.test(lines[i])) break;
        rows.push(splitRow(lines[i]));
        i += 1;
      }
      const alignOf = (idx: number) => aligns[idx] ?? undefined;
      blocks.push(
        <div key={`tw${key++}`} style={{ overflowX: "auto", margin: "6px 0" }}>
          <table style={tableStyle}>
            <thead>
              <tr>
                {header.map((cell, idx) => (
                  <th key={idx} style={{ ...thStyle, textAlign: alignOf(idx) }}>
                    {renderInline(cell, `th${key}-${idx}`)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((cells, r) => (
                <tr key={r}>
                  {header.map((_, c) => (
                    <td key={c} style={{ ...tdStyle, textAlign: alignOf(c) }}>
                      {renderInline(cells[c] ?? "", `td${key}-${r}-${c}`)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
      continue;
    }

    // List group (bulleted - / * / +, or numbered 1.)
    if (/^\s*([-*+]|\d+\.)\s+/.test(line)) {
      const items: { ordered: boolean; content: string }[] = [];
      while (i < lines.length && /^\s*([-*+]|\d+\.)\s+/.test(lines[i])) {
        const ordered = /^\s*\d+\.\s+/.test(lines[i]);
        const content = lines[i].replace(/^\s*([-*+]|\d+\.)\s+/, "");
        items.push({ ordered, content });
        i += 1;
      }
      const ordered = items[0]?.ordered;
      const Tag = ordered ? "ol" : "ul";
      blocks.push(
        <Tag key={`l${key++}`} style={{ margin: "4px 0", paddingLeft: 22, display: "flex", flexDirection: "column", gap: 3 }}>
          {items.map((it, idx) => (
            <li key={idx} style={{ lineHeight: 1.5 }}>{renderInline(it.content, `l${key}-${idx}`)}</li>
          ))}
        </Tag>
      );
      continue;
    }

    // Blank line → spacer (paragraph break)
    if (line.trim() === "") { i += 1; continue; }

    // Otherwise: gather a paragraph (consecutive non-blank, non-special lines)
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !lines[i].trim().startsWith("```") &&
      !/^(#{1,6})\s+/.test(lines[i]) &&
      !/^\s*([-*+]|\d+\.)\s+/.test(lines[i]) &&
      !(lines[i].includes("|") && i + 1 < lines.length && isTableSeparator(lines[i + 1]))
    ) {
      para.push(lines[i]);
      i += 1;
    }
    blocks.push(
      <p key={`p${key++}`} style={{ margin: "0 0 2px", lineHeight: 1.55 }}>
        {renderInline(para.join("\n"), `p${key}`)}
      </p>
    );
  }

  return <div style={{ display: "flex", flexDirection: "column", gap: 6, fontSize: 15 }}>{blocks}</div>;
}

const inlineCode: React.CSSProperties = {
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
  fontSize: "0.9em", background: "var(--bg)", padding: "1px 5px",
  borderRadius: 5, border: "var(--border-width) solid var(--line)",
};
const codeBlock: React.CSSProperties = {
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
  fontSize: 13, lineHeight: 1.5, margin: "4px 0", padding: "12px 14px",
  background: "var(--bg)", border: "var(--border-width) solid var(--line)",
  // Task #3 (Mason 08-01): whiteSpace:"pre" let long lines widen the whole
  // chat pane (horizontal page scroll). Wrap instead; code stays monospaced.
  borderRadius: "var(--radius-control)", whiteSpace: "pre-wrap", overflowWrap: "anywhere",
};
const link: React.CSSProperties = { color: "var(--accent)", textDecoration: "underline" };
const tableStyle: React.CSSProperties = {
  borderCollapse: "collapse", width: "100%", fontSize: 14, lineHeight: 1.45,
};
const thStyle: React.CSSProperties = {
  textAlign: "left", fontWeight: 700, padding: "6px 10px",
  borderBottom: "var(--border-width) solid var(--line)",
  background: "var(--bg)", whiteSpace: "nowrap",
};
const tdStyle: React.CSSProperties = {
  textAlign: "left", padding: "6px 10px", verticalAlign: "top",
  borderBottom: "var(--border-width) solid var(--line)",
};
