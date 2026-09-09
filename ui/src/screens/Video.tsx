// AYGENT — VIDEO tab (v0.3): the agentic NLE. Thin entry; the editor lives in
// ./video/ (store, model, Player, Timeline, Inspector, AgentDock, Panels).
// Full-bleed: App.tsx renders this screen without the standard content padding.
import { Editor } from "./video/Editor";

export function Video({ agentId, agentName, folder }: { agentId: string | null; agentName?: string; folder: string | null }) {
  return <Editor agentId={agentId} agentName={agentName} folder={folder} />;
}
