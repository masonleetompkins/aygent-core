// DASHBOARD ACTIONS (M4) — what a button is allowed to do.
//
// A dashboard button NEVER carries code. It carries one of these tagged
// variants, and this file is the only place that can turn one into an effect.
// That keeps the blast radius of a model-authored dashboard equal to the
// blast radius of the app's own UI — no more.
//
// SAFETY RULE: `run_prompt` spends money. It therefore only ever fires from a
// real click, and `confirm` on a destructive `run_tool` is honored here rather
// than trusted to the caller.
import { invoke } from "@tauri-apps/api/core";

export type DashAction =
  | { kind: "run_prompt"; prompt: string; open_chat?: boolean }
  | { kind: "run_tool"; tool: string; args?: unknown; confirm?: boolean }
  | { kind: "refresh_module"; module_id: string }
  | { kind: "navigate"; screen: string }
  | { kind: "open_url"; url: string }
  | { kind: "reveal_in_finder"; path: string }
  | { kind: "set_var"; name: string; value: unknown }
  | { kind: "run_schedule"; schedule_id: string };

export interface ActionCtx {
  agentId: string;
  /** Fire a prompt as a real turn (the dashboard's builder path). */
  runPrompt: (prompt: string, openChat: boolean) => void;
  refreshModule: (moduleId: string) => void;
  navigate: (screen: string) => void;
  setVar: (name: string, value: unknown) => void;
  notify: (msg: string) => void;
}

/// Execute one action. Returns nothing — every effect is a side effect on the
/// app, and failures surface through `notify` rather than throwing into React.
export async function runAction(a: DashAction, ctx: ActionCtx): Promise<void> {
  try {
    switch (a.kind) {
      case "run_prompt":
        if (!a.prompt?.trim()) { ctx.notify("That button has no prompt."); return; }
        ctx.runPrompt(a.prompt, !!a.open_chat);
        return;

      case "run_tool": {
        if (!a.tool) { ctx.notify("That button has no tool."); return; }
        // Confirmation is enforced HERE so a spec can't opt out by omission on
        // a dangerous tool name.
        const dangerous = /delete|remove|drop|reset|purge|rm\b/i.test(a.tool);
        if ((a.confirm || dangerous) && !window.confirm(`Run ${a.tool}?`)) return;
        const out = await invoke<string>("dashboard_run_tool", {
          agentId: ctx.agentId, tool: a.tool, args: a.args ?? {},
        });
        ctx.notify(typeof out === "string" && out ? out.slice(0, 300) : `${a.tool} finished.`);
        return;
      }

      case "refresh_module":
        ctx.refreshModule(a.module_id);
        return;

      case "navigate":
        ctx.navigate(a.screen);
        return;

      case "open_url": {
        // https only — same posture as the Http data source.
        if (!/^https:\/\//i.test(a.url)) { ctx.notify("Only https links are allowed."); return; }
        window.open(a.url, "_blank", "noopener,noreferrer");
        return;
      }

      case "reveal_in_finder":
        await invoke("reveal_in_finder", { path: a.path });
        return;

      case "set_var":
        ctx.setVar(a.name, a.value);
        return;

      case "run_schedule":
        ctx.notify("Running a schedule from the dashboard isn't wired yet — open the Scheduler.");
        return;

      default:
        ctx.notify(`Unknown action: ${(a as { kind?: string })?.kind ?? "?"}`);
    }
  } catch (e) {
    ctx.notify(String(e));
  }
}

/// Map a spec `tone` onto the Button variants we actually have, so a model
/// writing tone:"danger" gets something sane instead of a crash.
export function toneToVariant(tone?: string): "primary" | "secondary" {
  return tone === "primary" ? "primary" : "secondary";
}
