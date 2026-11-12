import { create } from "zustand";
import { killAgent, spawnAgent, spawnCustom } from "../lib/ipc";

export interface Project {
  id: string;
  path: string;
  name: string;
}

export interface PaneAgent {
  /** PTY id from the backend. */
  id: string;
  projectId: string;
  name: string;
  exited: boolean;
}

interface WorkspaceState {
  projects: Project[];
  activeProjectId: string | null;
  agents: PaneAgent[];
  maximizedId: string | null;

  addProject: (path: string) => void;
  selectProject: (id: string) => void;
  removeProject: (id: string) => void;
  /** Start a detected CLI (`{ defId }`) or a typed command (`{ command }`) in the active project. */
  launch: (what: { defId: string; name: string } | { command: string }) => Promise<void>;
  closeAgent: (id: string) => void;
  markExited: (id: string) => void;
  toggleMaximize: (id: string) => void;
}

const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

export const useWorkspace = create<WorkspaceState>((set, get) => ({
  projects: [],
  activeProjectId: null,
  agents: [],
  maximizedId: null,

  addProject: (path) => {
    const existing = get().projects.find((p) => p.path.toLowerCase() === path.toLowerCase());
    if (existing) return set({ activeProjectId: existing.id, maximizedId: null });
    const project = { id: crypto.randomUUID(), path, name: baseName(path) };
    set((s) => ({ projects: [...s.projects, project], activeProjectId: project.id, maximizedId: null }));
  },

  selectProject: (id) => set({ activeProjectId: id, maximizedId: null }),

  removeProject: (id) => {
    get().agents.filter((a) => a.projectId === id).forEach((a) => void killAgent(a.id).catch(() => {}));
    set((s) => {
      const projects = s.projects.filter((p) => p.id !== id);
      return {
        projects,
        agents: s.agents.filter((a) => a.projectId !== id),
        activeProjectId: s.activeProjectId === id ? (projects[0]?.id ?? null) : s.activeProjectId,
        maximizedId: null,
      };
    });
  },

  launch: async (what) => {
    const project = get().projects.find((p) => p.id === get().activeProjectId);
    if (!project) return;
    const id =
      "defId" in what
        ? await spawnAgent(what.defId, project.path, 80, 24)
        : await spawnCustom(what.command, project.path, 80, 24);
    const name = "defId" in what ? what.name : what.command;
    set((s) => ({ agents: [...s.agents, { id, projectId: project.id, name, exited: false }] }));
  },

  closeAgent: (id) => {
    void killAgent(id).catch(() => {});
    set((s) => ({
      agents: s.agents.filter((a) => a.id !== id),
      maximizedId: s.maximizedId === id ? null : s.maximizedId,
    }));
  },

  markExited: (id) => set((s) => ({ agents: s.agents.map((a) => (a.id === id ? { ...a, exited: true } : a)) })),

  toggleMaximize: (id) => set((s) => ({ maximizedId: s.maximizedId === id ? null : id })),
}));
