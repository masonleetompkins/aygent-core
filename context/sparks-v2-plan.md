# Sparks v2 — inline-in-chat, iterate, save-when-ready, default UI

Mason feedback (2026-08-07) on Sparks v1:
1. Agent produced ugly plain HTML — no default design system.
2. Had to leave Chat to see it — wants inline render in the conversation.
3. Wants to iterate in chat, then Save to Library when ready.

## New model: a `spark_preview` TOOL (not file writes)
Agent calls spark_preview({slug, title, html}). It:
- returns ok + echoes the slug/title (no file written yet).
- Chat renders a SPECIAL inline card: a sandboxed iframe (srcdoc=html,
  sandbox="allow-scripts allow-popups allow-forms") + a "Save to Library" button
  + "open in Sparks tab" after save.
- Iterate: agent calls spark_preview again (same slug) → card hot-swaps html.
- Save: the card's button calls a new `spark_save` command → writes
  Sparks/<slug>/index.html + spark.json (jailed). Then it shows in the Sparks tab.

The old file-write path in the Sparks skill is replaced by spark_preview; the
Sparks tab (sparks_list/read/delete) stays as the LIBRARY of saved ones.

## Default design system (the #1 fix)
Rewrite SPARKS_INSTRUCTIONS: unless the user asks for a specific look, the Spark
MUST use AYGENT's aesthetic — a <style> block with the same tokens as the app:
- bg #ffffff, text #0a0a0a, muted #5c5c5c, line, accent, radius 14/8,
  system font stack, card surfaces with soft shadow, generous spacing,
  a type scale. Provide a starter template in the instructions so the model
  always starts from something clean.
- Support dark too if asked. Charts: use Chart.js via CDN. Data: embed as
  const DATA = {...} (unchanged rule — Spark never calls back).

## Pieces
- Rust: spark_preview tool schema + dispatch (returns ok, carries html back to
  the UI via the ToolResult detail so the card can render it). spark_save
  command (write index.html + spark.json). Keep sparks_list/read/delete.
- turns.ts: recognize spark_preview → a SparkCard slot type carrying {slug,
  title, html}.
- Chat.tsx: render SparkCard = inline iframe + Save button (invokes spark_save).
- SPARKS_INSTRUCTIONS rewrite with the default template.

## Wire detail
spark_preview's html can be large. The tool RESULT detail is capped at 2000
chars in the activity emit — so DON'T rely on ToolResult detail for the html.
Instead: the ToolUse `input` carries {slug,title,html} in full (args stream).
turns.ts reads html from the ToolUse input, not the result. Verify the input
isn't truncated in the store (turns.ts caps write_file content at N — must NOT
cap spark_preview html, or preview truncates). Handle in describeToolUse.
