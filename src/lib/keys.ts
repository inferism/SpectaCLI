import { activeConversation, useWorkspace } from "../store/workspace";

/** Put the cursor in an agent's terminal; it may mount on the next render. */
export function focusPty(ptyId: string, delay = 0) {
  const go = () => document.querySelector<HTMLElement>(`[data-pty="${ptyId}"] .xterm-helper-textarea`)?.focus();
  if (delay) window.setTimeout(go, delay);
  else go();
}

/**
 * App-wide pane shortcuts, caught before xterm sees them:
 * - Ctrl+1..9 puts the cursor in that pane of the current conversation
 * - Ctrl+Shift+Enter maximizes or restores the pane that has the cursor
 * Returns an unsubscribe.
 */
export function startPaneKeys(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (!e.ctrlKey || e.altKey || e.metaKey) return;
    const ws = useWorkspace.getState();
    const project = ws.projects.find((p) => p.id === ws.activeProjectId);
    if (!project) return;
    const agents = activeConversation(project).agents;

    if (!e.shiftKey && /^Digit[1-9]$/.test(e.code)) {
      const agent = agents[Number(e.code.slice(5)) - 1];
      const ptyId = agent && ws.runtime[agent.id]?.ptyId;
      if (!agent) return;
      e.preventDefault();
      e.stopPropagation();
      ws.revealAgent(project.id, agent.id);
      if (ptyId) focusPty(ptyId, 50);
      return;
    }

    if (e.shiftKey && e.key === "Enter") {
      const ptyId = (document.activeElement?.closest("[data-pty]") as HTMLElement | null)?.dataset.pty;
      const agent = ptyId && agents.find((a) => ws.runtime[a.id]?.ptyId === ptyId);
      if (!agent) return;
      e.preventDefault();
      e.stopPropagation();
      ws.toggleMaximize(project.id, agent.id);
      focusPty(ptyId, 50);
    }
  };
  window.addEventListener("keydown", onKey, true);
  return () => window.removeEventListener("keydown", onKey, true);
}
