// AYGENT — Markdown renderer (dependency-free).
//
// The agent replies in markdown; we render it so **bold**, *italic*, `code`,
// code fences, headers, and lists actually format. This is a small, SAFE
// subset parser: we never use dangerouslySetInnerHTML — every node is a real
// React element, so there's no HTML-injection surface from model output.
//
// Shared across surfaces (Chat now; any future transcript/preview reuses it).
import type { ReactNode } from "react";

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
    { re: /^`([^`]+)`/, render: (m) => <code style={inlineCode}>{m[1]}</code> },
    {
      re: /^\[([^\]]+)\]\(([^)]+)\)/,
      render: (m) => <a href={m[2]} style={link} target="_blank" rel="noreferrer">{m[1]}</a>,
    },
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

// Block-level: split into paragraphs, code fences, headings, and list groups.
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
      !/^\s*([-*+]|\d+\.)\s+/.test(lines[i])
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
