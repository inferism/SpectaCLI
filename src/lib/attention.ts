import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { type InboxItem, inboxKind, useInbox } from "../store/inbox";
import { useStatus } from "../store/status";
import { activeConversation, soloAgentId, useWorkspace } from "../store/workspace";
import type { AgentStatus } from "./ipc";
import { shouldToast } from "./toast";

const lastToast = new Map<string, number>();

function findAgent(ptyId: string) {
  const { projects, runtime } = useWorkspace.getState();
  for (const project of projects) {
    for (const conv of project.conversations) {
      const agent = conv.agents.find((a) => runtime[a.id]?.ptyId === ptyId);
      if (agent) return { project, agent };
    }
  }
  return null;
}

/** Whether this agent's pane is on screen (not counting window focus). */
function paneShown(projectId: string, instanceId: string): boolean {
  const { projects, activeProjectId } = useWorkspace.getState();
  const project = projects.find((p) => p.id === projectId);
  if (!project || projectId !== activeProjectId) return false;
  const solo = soloAgentId(activeConversation(project));
  return solo === null || solo === instanceId;
}

/** An OS notification; asks for permission the first time. */
export async function toast(title: string, body: string) {
  try {
    if (!(await isPermissionGranted()) && (await requestPermission()) !== "granted") return;
    sendNotification({ title, body });
  } catch (e) {
    console.error("toast:", e);
  }
}

const TEXT = {
  needs_input: (agent: string) => `${agent} needs your input`,
  finished: (agent: string) => `${agent} finished`,
};

/** Feed every `agent:status` here: it updates the status store, the inbox and the OS toasts. */
export function recordStatus(s: AgentStatus) {
  const prev = useStatus.getState().byAgent[s.agentId]?.status;
  useStatus.setState((st) => ({ byAgent: { ...st.byAgent, [s.agentId]: s } }));
  if (s.status !== "needs_input") useInbox.getState().resolve(s.agentId);
  const kind = inboxKind(prev, s.status);
  if (!kind) return;
  const found = findAgent(s.agentId);
  if (!found) return;
  const { project, agent } = found;
  useInbox.getState().add({
    kind,
    ptyId: s.agentId,
    projectId: project.id,
    instanceId: agent.id,
    projectName: project.name,
    agentName: agent.name,
    at: s.at,
  });
  const now = Date.now();
  const show = shouldToast({
    windowFocused: document.hasFocus(),
    paneShown: paneShown(project.id, agent.id),
    lastToastAt: lastToast.get(s.agentId),
    now,
  });
  if (show) {
    lastToast.set(s.agentId, now);
    void toast(TEXT[kind](agent.name), project.name);
  }
}

/** Bring the item's pane on screen and put the cursor in it. */
export function openInboxItem(item: InboxItem) {
  const ws = useWorkspace.getState();
  ws.selectProject(item.projectId);
  ws.revealAgent(item.projectId, item.instanceId);
  useInbox.getState().remove(item.id);
  // The pane's terminal mounts on the next render.
  window.setTimeout(() => {
    document.querySelector<HTMLElement>(`[data-pty="${item.ptyId}"] .xterm-helper-textarea`)?.focus();
  }, 150);
}
