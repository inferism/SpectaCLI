import { create } from "zustand";
import { killAgent, spawnAgent, spawnCustom } from "../lib/ipc";

// --- Persisted shape (projects.json, PLAN.md section 4) ---
// Objects are only ever updated by spreading, so fields written by a newer
// version of the app survive a load/save round trip.

export type AgentLaunch = { kind: "def"; agentDefId: string } | { kind: "custom"; command: string };

export interface AgentInstance {
  /** Stable across restarts. Not the PTY id, which changes every launch. */
  id: string;
  launch: AgentLaunch;
  name: string;
  cwd: string;
}

export interface Layout {
  mode: "grid" | "focus";
  /** Focus mode shows this agent; falls back to the first when unset or gone. */
  focusedAgentId: string | null;
  maximizedAgentId: string | null;
}

/** Phase 3 adds more; until then every project has exactly one. */
export interface Conversation {
  id: string;
  title: string;
  createdAt: string;
  /** Array order is pane order. */
  agents: AgentInstance[];
  layout: Layout;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  createdAt: string;
  lastOpenedAt: string;
  conversations: Conversation[];
  activeConversationId: string;
}

export interface ProjectsFile {
  schemaVersion: 1;
  activeProjectId: string | null;
  projects: Project[];
}

// --- Runtime only (never saved) ---

export interface AgentRuntime {
  /** null until launched in this app session. */
  ptyId: string | null;
  exited: boolean;
}

export type LaunchWhat = { defId: string; name: string } | { command: string };

interface WorkspaceState {
  /** False until `hydrate` ran; nothing may be saved before that. */
  hydrated: boolean;
  /** Top-level fields of the loaded file this version doesn't know. */
  fileExtra: Record<string, unknown>;
  projects: Project[];
  activeProjectId: string | null;
  runtime: Record<string, AgentRuntime>;

  hydrate: (file: ProjectsFile | null) => void;
  addProject: (path: string) => void;
  selectProject: (id: string) => void;
  removeProject: (id: string) => void;
  /** Start an agent in `projectId`. Resolves to the new instance id, or null if the project went away meanwhile. */
  launch: (projectId: string, what: LaunchWhat) => Promise<string | null>;
  closeAgent: (instanceId: string) => void;
  /** Relaunch the saved agents of a project that have no process yet (once per app session each). */
  restoreProject: (projectId: string) => Promise<void>;
  /** From `agent:exit`, which reports the PTY id. */
  markExited: (ptyId: string) => void;
  toggleMaximize: (projectId: string, instanceId: string) => void;
  setLayoutMode: (projectId: string, mode: Layout["mode"]) => void;
  focusAgent: (projectId: string, instanceId: string) => void;
}

const now = () => new Date().toISOString();
const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;
/** Windows paths: case-insensitive, either slash, no trailing slash. */
const samePath = (a: string, b: string) => {
  const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return norm(a) === norm(b);
};

export const activeConversation = (p: Project): Conversation =>
  p.conversations.find((c) => c.id === p.activeConversationId) ?? p.conversations[0];

function newConversation(): Conversation {
  return { id: crypto.randomUUID(), title: "Default", createdAt: now(), agents: [], layout: { mode: "grid", focusedAgentId: null, maximizedAgentId: null } };
}

/** Repair what a hand-edited or partial file might lack. */
function normalizeProject(p: Project): Project {
  const conversations = p.conversations?.length ? p.conversations : [newConversation()];
  const activeConversationId = conversations.some((c) => c.id === p.activeConversationId)
    ? p.activeConversationId
    : conversations[0].id;
  return {
    ...p,
    name: p.name ?? baseName(p.path),
    conversations: conversations.map((c) => ({
      ...c,
      agents: c.agents ?? [],
      layout: {
        ...c.layout,
        mode: c.layout?.mode === "focus" ? "focus" : "grid",
        focusedAgentId: c.layout?.focusedAgentId ?? null,
        maximizedAgentId: c.layout?.maximizedAgentId ?? null,
      },
    })),
    activeConversationId,
  };
}

function updateConversation(projects: Project[], projectId: string, fn: (c: Conversation) => Conversation): Project[] {
  return projects.map((p) =>
    p.id !== projectId
      ? p
      : { ...p, conversations: p.conversations.map((c) => (c.id === p.activeConversationId ? fn(c) : c)) },
  );
}

const projectOf = (projects: Project[], instanceId: string) =>
  projects.find((p) => p.conversations.some((c) => c.agents.some((a) => a.id === instanceId)));

/** PTYs that exited before `launch` recorded them (a command that fails at once). */
const earlyExits = new Set<string>();
/** Only while a launch is in flight can an unknown exit be an early one. */
let launching = 0;
/** Saved agents already tried this session, so a failing one is not retried in a loop. */
const restoreTried = new Set<string>();

export function toFile(s: Pick<WorkspaceState, "fileExtra" | "projects" | "activeProjectId">): ProjectsFile {
  return { ...s.fileExtra, schemaVersion: 1, activeProjectId: s.activeProjectId, projects: s.projects };
}

export const useWorkspace = create<WorkspaceState>((set, get) => ({
  hydrated: false,
  fileExtra: {},
  projects: [],
  activeProjectId: null,
  runtime: {},

  hydrate: (file) => {
    if (!file) return set({ hydrated: true });
    const { schemaVersion: _v, activeProjectId, projects, ...fileExtra } = file;
    const ps = (projects ?? []).map(normalizeProject);
    const active = ps.some((p) => p.id === activeProjectId) ? activeProjectId : (ps[0]?.id ?? null);
    set({ hydrated: true, fileExtra, projects: ps, activeProjectId: active });
    // Only the last project wakes up; the others stay hibernated until selected.
    if (active) void get().restoreProject(active);
  },

  addProject: (path) => {
    const existing = get().projects.find((p) => samePath(p.path, path));
    if (existing) return get().selectProject(existing.id);
    const conv = newConversation();
    const project: Project = {
      id: crypto.randomUUID(),
      name: baseName(path),
      path,
      createdAt: now(),
      lastOpenedAt: now(),
      conversations: [conv],
      activeConversationId: conv.id,
    };
    set((s) => ({ projects: [...s.projects, project], activeProjectId: project.id }));
  },

  selectProject: (id) => {
    set((s) => ({
      activeProjectId: id,
      projects: s.projects.map((p) => (p.id === id ? { ...p, lastOpenedAt: now() } : p)),
    }));
    void get().restoreProject(id);
  },

  removeProject: (id) => {
    const project = get().projects.find((p) => p.id === id);
    if (!project) return;
    const ids = project.conversations.flatMap((c) => c.agents.map((a) => a.id));
    for (const i of ids) {
      const pty = get().runtime[i]?.ptyId;
      if (pty) void killAgent(pty).catch(() => {});
    }
    set((s) => {
      const projects = s.projects.filter((p) => p.id !== id);
      const runtime = { ...s.runtime };
      ids.forEach((i) => delete runtime[i]);
      return {
        projects,
        runtime,
        activeProjectId: s.activeProjectId === id ? (projects[0]?.id ?? null) : s.activeProjectId,
      };
    });
  },

  launch: async (projectId, what) => {
    const project = get().projects.find((p) => p.id === projectId);
    if (!project) return null;
    let ptyId: string;
    launching++;
    try {
      ptyId =
        "defId" in what
          ? await spawnAgent(what.defId, project.path, 80, 24)
          : await spawnCustom(what.command, project.path, 80, 24);
    } finally {
      launching--;
    }
    // The project may have been removed while the process was starting.
    if (!get().projects.some((p) => p.id === projectId)) {
      earlyExits.delete(ptyId);
      void killAgent(ptyId).catch(() => {});
      return null;
    }
    const instance: AgentInstance = {
      id: crypto.randomUUID(),
      launch: "defId" in what ? { kind: "def", agentDefId: what.defId } : { kind: "custom", command: what.command },
      name: "defId" in what ? what.name : what.command,
      cwd: project.path,
    };
    const exited = earlyExits.delete(ptyId);
    set((s) => ({
      projects: updateConversation(s.projects, projectId, (c) => ({ ...c, agents: [...c.agents, instance] })),
      runtime: { ...s.runtime, [instance.id]: { ptyId, exited } },
    }));
    return instance.id;
  },

  restoreProject: async (projectId) => {
    const project = get().projects.find((p) => p.id === projectId);
    if (!project) return;
    const todo = activeConversation(project).agents.filter((a) => !get().runtime[a.id] && !restoreTried.has(a.id));
    await Promise.all(
      todo.map(async (a) => {
        restoreTried.add(a.id);
        let ptyId: string;
        launching++;
        try {
          ptyId =
            a.launch.kind === "def"
              ? await spawnAgent(a.launch.agentDefId, a.cwd, 80, 24)
              : await spawnCustom(a.launch.command, a.cwd, 80, 24);
        } catch (e) {
          console.error(`restore ${a.name}:`, e);
          return;
        } finally {
          launching--;
        }
        // Closed or removed while starting.
        if (!projectOf(get().projects, a.id)) {
          earlyExits.delete(ptyId);
          void killAgent(ptyId).catch(() => {});
          return;
        }
        const exited = earlyExits.delete(ptyId);
        set((s) => ({ runtime: { ...s.runtime, [a.id]: { ptyId, exited } } }));
      }),
    );
  },

  closeAgent: (instanceId) => {
    const pty = get().runtime[instanceId]?.ptyId;
    if (pty) void killAgent(pty).catch(() => {});
    const project = projectOf(get().projects, instanceId);
    set((s) => {
      const runtime = { ...s.runtime };
      delete runtime[instanceId];
      return {
        runtime,
        projects: project
          ? updateConversation(s.projects, project.id, (c) => ({
              ...c,
              agents: c.agents.filter((a) => a.id !== instanceId),
              layout: {
                ...c.layout,
                maximizedAgentId: c.layout.maximizedAgentId === instanceId ? null : c.layout.maximizedAgentId,
                focusedAgentId: c.layout.focusedAgentId === instanceId ? null : c.layout.focusedAgentId,
              },
            }))
          : s.projects,
      };
    });
  },

  markExited: (ptyId) => {
    const entry = Object.entries(get().runtime).find(([, r]) => r.ptyId === ptyId);
    if (!entry) {
      if (launching > 0) earlyExits.add(ptyId);
      return;
    }
    set((s) => ({ runtime: { ...s.runtime, [entry[0]]: { ...entry[1], exited: true } } }));
  },

  toggleMaximize: (projectId, instanceId) =>
    set((s) => ({
      projects: updateConversation(s.projects, projectId, (c) => ({
        ...c,
        layout: { ...c.layout, maximizedAgentId: c.layout.maximizedAgentId === instanceId ? null : instanceId },
      })),
    })),

  setLayoutMode: (projectId, mode) =>
    set((s) => ({
      projects: updateConversation(s.projects, projectId, (c) => ({ ...c, layout: { ...c.layout, mode } })),
    })),

  focusAgent: (projectId, instanceId) =>
    set((s) => ({
      projects: updateConversation(s.projects, projectId, (c) => ({
        ...c,
        layout: { ...c.layout, focusedAgentId: instanceId },
      })),
    })),
}));
