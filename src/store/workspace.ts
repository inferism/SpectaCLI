import { create } from "zustand";
import {
  createWorktree,
  hibernateAgent,
  killAgent,
  removeWorktree as removeWorktreeIpc,
  type SessionOpts,
  spawnAgent,
  spawnCustom,
  spawnWsl,
  worktreeChanges,
} from "../lib/ipc";

// --- Persisted shape (projects.json, PLAN.md section 4) ---
// Objects are only ever updated by spreading, so fields written by a newer
// version of the app survive a load/save round trip.

export type AgentLaunch =
  | { kind: "def"; agentDefId: string }
  | { kind: "custom"; command: string }
  /** A login shell (or `command`) inside a WSL distro. */
  | { kind: "wsl"; distro: string; command?: string };

/** The agent's own git worktree; its `cwd` is `path`. */
export interface AgentWorktree {
  path: string;
  branch: string;
}

export interface AgentInstance {
  /** Stable across restarts. Not the PTY id, which changes every launch. */
  id: string;
  launch: AgentLaunch;
  name: string;
  cwd: string;
  /** Set when the agent was started isolated in its own worktree. */
  worktree?: AgentWorktree;
  /** The CLI's own session id, for CLIs that take one up front (Claude). */
  sessionId?: string;
  /** Stopped to free its memory; resumes its session when clicked. */
  hibernated?: boolean;
  /** Hibernate after this many idle minutes. Unset: never. */
  autoHibernateMin?: number;
}

export interface Layout {
  mode: "grid" | "focus";
  /** Focus mode shows this agent; falls back to the first when unset or gone. */
  focusedAgentId: string | null;
  maximizedAgentId: string | null;
}

/** A named set of agents with its own layout; a project has one or more. */
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
  /** The CLI can resume its session, so it can be hibernated. */
  resumable: boolean;
  /** When this process was started to continue an earlier session; null for a new one. */
  resumedAt: number | null;
  /** Shown next to the name, e.g. when a resume failed and a new session started. */
  note?: string;
}

export type LaunchWhat = { defId: string; name: string } | { command: string } | { wslDistro: string; command?: string };

export interface LaunchOpts {
  /** Run in a new git worktree of the project instead of the project folder. */
  isolate?: boolean;
}

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
  launch: (projectId: string, what: LaunchWhat, opts?: LaunchOpts) => Promise<string | null>;
  /**
   * Close the agent and delete its worktree folder (the branch stays). Rejects
   * with "dirty: ..." when the worktree has changes and `force` is not set,
   * before anything is closed.
   */
  removeWorktree: (instanceId: string, force?: boolean) => Promise<void>;
  closeAgent: (instanceId: string) => void;
  /** "New": add an empty conversation to the project and switch to it. Returns its id. */
  newConversation: (projectId: string) => string | null;
  /** Show another conversation of the project; the one left keeps running in the background. */
  selectConversation: (projectId: string, conversationId: string) => void;
  /** Resume the saved, non-hibernated agents of a project that have no process yet (once per app session each). */
  restoreProject: (projectId: string) => Promise<void>;
  /** From `agent:exit`, which reports the PTY id. */
  markExited: (ptyId: string, code?: number | null) => void;
  /** Stop a resumable agent to free its memory, keeping its session. */
  hibernate: (instanceId: string) => Promise<void>;
  /** Relaunch a hibernated agent with its session. */
  resume: (instanceId: string) => Promise<void>;
  setAutoHibernate: (instanceId: string, minutes: number | null) => void;
  /** Give an agent a name of the user's choosing. A blank name is ignored. */
  renameAgent: (instanceId: string, name: string) => void;
  toggleMaximize: (projectId: string, instanceId: string) => void;
  setLayoutMode: (projectId: string, mode: Layout["mode"]) => void;
  focusAgent: (projectId: string, instanceId: string) => void;
  /** Make sure `instanceId` is on screen in its project (focus tab, or the maximized pane), e.g. from an inbox click. */
  revealAgent: (projectId: string, instanceId: string) => void;
}

/** A resumed CLI that fails within this time had nothing to resume (e.g. Claude's "No conversation found"). */
export const RESUME_FAIL_MS = 15_000;

const now = () => new Date().toISOString();
const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;
/** Windows paths: case-insensitive, either slash, no trailing slash. */
const samePath = (a: string, b: string) => {
  const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return norm(a) === norm(b);
};

export const activeConversation = (p: Project): Conversation =>
  p.conversations.find((c) => c.id === p.activeConversationId) ?? p.conversations[0];

/**
 * The one agent a conversation shows alone: the focused tab in focus mode
 * (first agent when unset), else the maximized one. Null in a plain grid.
 */
export function soloAgentId(c: Conversation): string | null {
  const has = (id: string | null) => c.agents.some((a) => a.id === id);
  if (c.layout.mode === "focus") return has(c.layout.focusedAgentId) ? c.layout.focusedAgentId : (c.agents[0]?.id ?? null);
  return has(c.layout.maximizedAgentId) ? c.layout.maximizedAgentId : null;
}

function newConversation(title = "Default"): Conversation {
  return { id: crypto.randomUUID(), title, createdAt: now(), agents: [], layout: { mode: "grid", focusedAgentId: null, maximizedAgentId: null } };
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

/** Update one agent wherever it is. */
function updateInstance(projects: Project[], instanceId: string, fn: (a: AgentInstance) => AgentInstance): Project[] {
  return projects.map((p) =>
    p.conversations.some((c) => c.agents.some((a) => a.id === instanceId))
      ? {
          ...p,
          conversations: p.conversations.map((c) => ({
            ...c,
            agents: c.agents.map((a) => (a.id === instanceId ? fn(a) : a)),
          })),
        }
      : p,
  );
}

const projectOf = (projects: Project[], instanceId: string) =>
  projects.find((p) => p.conversations.some((c) => c.agents.some((a) => a.id === instanceId)));

export const findInstance = (projects: Project[], instanceId: string): AgentInstance | undefined => {
  for (const p of projects) for (const c of p.conversations) for (const a of c.agents) if (a.id === instanceId) return a;
  return undefined;
};

interface Started {
  ptyId: string;
  sessionId: string | null;
  resumed: boolean;
  resumable: boolean;
}

async function spawnFor(launch: AgentLaunch, cwd: string, session: SessionOpts): Promise<Started> {
  if (launch.kind === "custom") {
    const ptyId = await spawnCustom(launch.command, cwd, 80, 24);
    return { ptyId, sessionId: null, resumed: false, resumable: false };
  }
  if (launch.kind === "wsl") {
    const ptyId = await spawnWsl(launch.distro, launch.command ?? null, cwd, 80, 24);
    return { ptyId, sessionId: null, resumed: false, resumable: false };
  }
  const s = await spawnAgent(launch.agentDefId, cwd, 80, 24, session);
  return { ptyId: s.agentId, sessionId: s.sessionId, resumed: s.resumed, resumable: s.resumable };
}

/** PTYs that exited before a launch recorded them (a command that fails at once), with their exit code. */
const earlyExits = new Map<string, number | null>();
/** Only while a launch is in flight can an unknown exit be an early one. */
let launching = 0;
/** Saved agents already tried this session, so a failing one is not retried in a loop. */
const restoreTried = new Set<string>();
/** Agents whose failed resume already fell back to a new session. */
const resumeFellBack = new Set<string>();

export function toFile(s: Pick<WorkspaceState, "fileExtra" | "projects" | "activeProjectId">): ProjectsFile {
  return { ...s.fileExtra, schemaVersion: 1, activeProjectId: s.activeProjectId, projects: s.projects };
}

export const useWorkspace = create<WorkspaceState>((set, get) => {
  /**
   * Start a saved agent's process and record it. Started agents that were
   * closed or hibernated meanwhile are killed again.
   */
  async function startSaved(instanceId: string, session: { resume: boolean }, note?: string): Promise<void> {
    const a = findInstance(get().projects, instanceId);
    if (!a) return;
    let r: Started;
    launching++;
    try {
      r = await spawnFor(a.launch, a.cwd, { sessionId: a.sessionId, resume: session.resume });
    } catch (e) {
      console.error(`start ${a.name}:`, e);
      return;
    } finally {
      launching--;
    }
    const still = findInstance(get().projects, instanceId);
    if (!still || still.hibernated) {
      earlyExits.delete(r.ptyId);
      void killAgent(r.ptyId).catch(() => {});
      return;
    }
    const early = earlyExits.has(r.ptyId);
    const code = earlyExits.get(r.ptyId);
    earlyExits.delete(r.ptyId);
    set((s) => ({
      projects:
        r.sessionId && r.sessionId !== still.sessionId
          ? updateInstance(s.projects, instanceId, (x) => ({ ...x, sessionId: r.sessionId! }))
          : s.projects,
      runtime: {
        ...s.runtime,
        [instanceId]: { ptyId: r.ptyId, exited: false, resumable: r.resumable, resumedAt: r.resumed ? Date.now() : null, note },
      },
    }));
    if (early) get().markExited(r.ptyId, code);
  }

  return {
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

    launch: async (projectId, what, opts = {}) => {
      const project = get().projects.find((p) => p.id === projectId);
      if (!project) return null;
      const launch: AgentLaunch =
        "defId" in what
          ? { kind: "def", agentDefId: what.defId }
          : "wslDistro" in what
            ? { kind: "wsl", distro: what.wslDistro, ...(what.command ? { command: what.command } : {}) }
            : { kind: "custom", command: what.command };
      const name = "defId" in what ? what.name : "wslDistro" in what ? `WSL · ${what.wslDistro}` : what.command;
      // The worktree is made first: the agent starts inside it.
      let worktree: AgentWorktree | undefined;
      if (opts.isolate) {
        worktree = await createWorktree(project.path, `${name}-${crypto.randomUUID().slice(0, 4)}`);
      }
      const cwd = worktree?.path ?? project.path;
      const discardWorktree = () => {
        if (worktree) void removeWorktreeIpc(worktree.path, true).catch(() => {});
      };
      let r: Started;
      launching++;
      try {
        r = await spawnFor(launch, cwd, {});
      } catch (e) {
        discardWorktree();
        throw e;
      } finally {
        launching--;
      }
      // The project may have been removed while the process was starting.
      if (!get().projects.some((p) => p.id === projectId)) {
        earlyExits.delete(r.ptyId);
        void killAgent(r.ptyId).catch(() => {});
        discardWorktree();
        return null;
      }
      const instance: AgentInstance = {
        id: crypto.randomUUID(),
        launch,
        name,
        cwd,
        ...(worktree ? { worktree } : {}),
        ...(r.sessionId ? { sessionId: r.sessionId } : {}),
      };
      const exited = earlyExits.delete(r.ptyId);
      set((s) => ({
        projects: updateConversation(s.projects, projectId, (c) => ({ ...c, agents: [...c.agents, instance] })),
        runtime: { ...s.runtime, [instance.id]: { ptyId: r.ptyId, exited, resumable: r.resumable, resumedAt: null } },
      }));
      return instance.id;
    },

    removeWorktree: async (instanceId, force = false) => {
      const a = findInstance(get().projects, instanceId);
      if (!a?.worktree) return;
      const { path } = a.worktree;
      if (!force) {
        const n = await worktreeChanges(path);
        if (n > 0) throw `dirty: ${n} changed file${n === 1 ? "" : "s"}`;
      }
      // The process holds the folder open on Windows: end it first.
      const pty = get().runtime[instanceId]?.ptyId;
      if (pty) await killAgent(pty).catch(() => {});
      get().closeAgent(instanceId);
      await removeWorktreeIpc(path, true);
    },

    restoreProject: async (projectId) => {
      const project = get().projects.find((p) => p.id === projectId);
      if (!project) return;
      const todo = activeConversation(project).agents.filter(
        (a) => !a.hibernated && !get().runtime[a.id] && !restoreTried.has(a.id),
      );
      await Promise.all(
        todo.map((a) => {
          restoreTried.add(a.id);
          return startSaved(a.id, { resume: true });
        }),
      );
    },

    newConversation: (projectId) => {
      const project = get().projects.find((p) => p.id === projectId);
      if (!project) return null;
      // Next free "Conversation N", so a deleted or renamed title never collides.
      const titles = new Set(project.conversations.map((c) => c.title));
      let n = project.conversations.length + 1;
      while (titles.has(`Conversation ${n}`)) n++;
      const conv = newConversation(`Conversation ${n}`);
      set((s) => ({
        projects: s.projects.map((p) =>
          p.id === projectId ? { ...p, conversations: [...p.conversations, conv], activeConversationId: conv.id } : p,
        ),
      }));
      return conv.id;
    },

    selectConversation: (projectId, conversationId) => {
      const project = get().projects.find((p) => p.id === projectId);
      if (!project || !project.conversations.some((c) => c.id === conversationId)) return;
      set((s) => ({
        projects: s.projects.map((p) => (p.id === projectId ? { ...p, activeConversationId: conversationId } : p)),
      }));
      void get().restoreProject(projectId);
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

    markExited: (ptyId, code = null) => {
      const entry = Object.entries(get().runtime).find(([, r]) => r.ptyId === ptyId);
      if (!entry) {
        if (launching > 0) earlyExits.set(ptyId, code);
        return;
      }
      const [instanceId, rt] = entry;
      set((s) => ({ runtime: { ...s.runtime, [instanceId]: { ...rt, exited: true } } }));
      // A resume that fails at once had nothing to resume: start a new session, once.
      const failedResume = rt.resumedAt !== null && Date.now() - rt.resumedAt < RESUME_FAIL_MS && code !== 0;
      if (failedResume && !resumeFellBack.has(instanceId)) {
        resumeFellBack.add(instanceId);
        void startSaved(instanceId, { resume: false }, "new session");
      }
    },

    hibernate: async (instanceId) => {
      const a = findInstance(get().projects, instanceId);
      const rt = get().runtime[instanceId];
      if (!a || a.launch.kind !== "def" || !rt?.ptyId || !rt.resumable) return;
      const { ptyId } = rt;
      const agentDefId = a.launch.agentDefId;
      // The pane switches to "hibernated" at once; the process stops in the background.
      set((s) => {
        const runtime = { ...s.runtime };
        delete runtime[instanceId];
        return { runtime, projects: updateInstance(s.projects, instanceId, (x) => ({ ...x, hibernated: true })) };
      });
      try {
        await hibernateAgent(ptyId, agentDefId);
      } catch {
        await killAgent(ptyId).catch(() => {});
      }
    },

    resume: async (instanceId) => {
      const a = findInstance(get().projects, instanceId);
      if (!a?.hibernated) return;
      set((s) => ({ projects: updateInstance(s.projects, instanceId, (x) => ({ ...x, hibernated: false })) }));
      resumeFellBack.delete(instanceId);
      await startSaved(instanceId, { resume: true });
    },

    setAutoHibernate: (instanceId, minutes) =>
      set((s) => ({
        projects: updateInstance(s.projects, instanceId, (x) => {
          const { autoHibernateMin: _old, ...rest } = x;
          return minutes ? { ...rest, autoHibernateMin: minutes } : rest;
        }),
      })),

    renameAgent: (instanceId, name) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      set((s) => ({ projects: updateInstance(s.projects, instanceId, (x) => ({ ...x, name: trimmed })) }));
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

    revealAgent: (projectId, instanceId) => {
      set((s) => ({
        projects: s.projects.map((p) => {
          if (p.id !== projectId) return p;
          // The agent may live in a conversation that is not showing: switch to it.
          const owner = p.conversations.find((c) => c.agents.some((a) => a.id === instanceId));
          const activeConversationId = owner?.id ?? p.activeConversationId;
          return {
            ...p,
            activeConversationId,
            conversations: p.conversations.map((c) =>
              c.id !== activeConversationId
                ? c
                : {
                    ...c,
                    layout: {
                      ...c.layout,
                      focusedAgentId: instanceId,
                      maximizedAgentId: c.layout.maximizedAgentId ? instanceId : null,
                    },
                  },
            ),
          };
        }),
      }));
      // Start the other saved agents of the conversation it switched to.
      void get().restoreProject(projectId);
    },
  };
});
