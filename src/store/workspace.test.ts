import { beforeEach, expect, test, vi } from "vitest";

let nextPty = 0;
let spawnGate: Promise<void> = Promise.resolve();
const killed: string[] = [];
const wslSpawns: { distro: string; command: string | null; cwd: string }[] = [];
const removedTrees: { path: string; force: boolean }[] = [];
let dirtyFiles = 0;
const hibernated: string[] = [];
/** Every spawnAgent call's session options. */
const spawns: { sessionId?: string | null; resume?: boolean }[] = [];
vi.mock("../lib/ipc", () => ({
  // Like the Claude registry entry: an id up front, resume by it.
  spawnAgent: async (_def: string, _cwd: string, _c: number, _r: number, s: { sessionId?: string | null; resume?: boolean } = {}) => {
    spawns.push(s);
    await spawnGate;
    const resumed = !!(s.resume && s.sessionId);
    return { agentId: `pty-${++nextPty}`, sessionId: s.sessionId ?? `sess-${nextPty}`, resumed, resumable: true };
  },
  spawnCustom: async () => {
    await spawnGate;
    return `pty-${++nextPty}`;
  },
  spawnWsl: async (distro: string, command: string | null, cwd: string) => {
    wslSpawns.push({ distro, command, cwd });
    return `pty-${++nextPty}`;
  },
  createWorktree: async (project: string, name: string) => {
    const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-");
    return { path: `${project}\.spectacli\worktrees\${slug}`, branch: `spectacli/${slug}` };
  },
  worktreeChanges: async () => dirtyFiles,
  removeWorktree: async (path: string, force: boolean) => void removedTrees.push({ path, force }),
  killAgent: async (id: string) => void killed.push(id),
  hibernateAgent: async (id: string) => void hibernated.push(id),
}));

const { activeConversation, soloAgentId, toFile, useWorkspace } = await import("./workspace");
const ws = () => useWorkspace.getState();
const agentsOf = (projectId: string) => activeConversation(ws().projects.find((p) => p.id === projectId)!).agents;

beforeEach(() => {
  useWorkspace.setState({ hydrated: false, fileExtra: {}, projects: [], activeProjectId: null, runtime: {} });
  killed.length = 0;
  wslSpawns.length = 0;
  removedTrees.length = 0;
  dirtyFiles = 0;
  hibernated.length = 0;
  spawns.length = 0;
  spawnGate = Promise.resolve();
});

test("adding the same folder twice selects the existing project", () => {
  ws().addProject("C:\\code\\a");
  ws().addProject("C:\\code\\b");
  ws().addProject("c:/code/A/");
  expect(ws().projects).toHaveLength(2);
  expect(ws().activeProjectId).toBe(ws().projects[0].id);
});

test("switching projects keeps each project's agents and maximize", async () => {
  ws().addProject("C:\\a");
  const a = ws().activeProjectId!;
  const agent = (await ws().launch(a, { command: "x" }))!;
  ws().toggleMaximize(a, agent);
  ws().addProject("C:\\b");
  await ws().launch(ws().activeProjectId!, { command: "y" });
  ws().selectProject(a);
  expect(agentsOf(a).map((x) => x.id)).toEqual([agent]);
  expect(activeConversation(ws().projects[0]).layout.maximizedAgentId).toBe(agent);
  expect(killed).toEqual([]);
});

test("focus mode keeps the focused agent, cleared when it closes", async () => {
  ws().addProject("C:\a");
  const a = ws().activeProjectId!;
  const x = (await ws().launch(a, { command: "x" }))!;
  ws().setLayoutMode(a, "focus");
  ws().focusAgent(a, x);
  const layout = () => activeConversation(ws().projects[0]).layout;
  expect(layout()).toMatchObject({ mode: "focus", focusedAgentId: x });
  ws().closeAgent(x);
  expect(layout().focusedAgentId).toBeNull();
});

test("persisted ids are instance ids, PTY ids stay runtime-only", async () => {
  ws().addProject("C:\\a");
  const id = (await ws().launch(ws().activeProjectId!, { defId: "claude", name: "Claude Code" }))!;
  const json = JSON.stringify(toFile(ws()));
  expect(json).toContain(id);
  expect(json).not.toContain(ws().runtime[id].ptyId!);
});

test("exits change runtime only, not the saved file", async () => {
  ws().addProject("C:\\a");
  const id = (await ws().launch(ws().activeProjectId!, { command: "x" }))!;
  const before = JSON.stringify(toFile(ws()));
  ws().markExited(ws().runtime[id].ptyId!);
  expect(ws().runtime[id].exited).toBe(true);
  expect(JSON.stringify(toFile(ws()))).toBe(before);
});

test("an exit that arrives before launch finishes is not lost", async () => {
  ws().addProject("C:\\a");
  let open!: () => void;
  spawnGate = new Promise((r) => (open = r));
  const p = ws().launch(ws().activeProjectId!, { command: "exit 1" });
  ws().markExited(`pty-${nextPty + 1}`);
  open();
  const id = (await p)!;
  expect(ws().runtime[id].exited).toBe(true);
});

test("removing a project during launch kills the new process", async () => {
  ws().addProject("C:\\a");
  const projectId = ws().activeProjectId!;
  let open!: () => void;
  spawnGate = new Promise((r) => (open = r));
  const p = ws().launch(projectId, { command: "x" });
  ws().removeProject(projectId);
  open();
  expect(await p).toBeNull();
  expect(killed).toEqual([`pty-${nextPty}`]);
});

test("closing an agent kills it and clears its maximize", async () => {
  ws().addProject("C:\\a");
  const projectId = ws().activeProjectId!;
  const id = (await ws().launch(projectId, { command: "x" }))!;
  const pty = ws().runtime[id].ptyId!;
  ws().toggleMaximize(projectId, id);
  ws().closeAgent(id);
  expect(killed).toEqual([pty]);
  expect(agentsOf(projectId)).toEqual([]);
  expect(activeConversation(ws().projects[0]).layout.maximizedAgentId).toBeNull();
  expect(ws().runtime[id]).toBeUndefined();
});

test("hydrate keeps unknown fields and repairs a bad active project", () => {
  const conv = { id: "c1", title: "Default", createdAt: "t", agents: [], layout: { maximizedAgentId: null }, future: 1 };
  ws().hydrate({
    schemaVersion: 1,
    activeProjectId: "gone",
    projects: [
      { id: "p1", name: "a", path: "C:\\a", createdAt: "t", lastOpenedAt: "t", conversations: [conv], activeConversationId: "c1", extra: "kept" } as never,
    ],
    topLevelExtra: true,
  } as never);
  expect(ws().hydrated).toBe(true);
  expect(ws().activeProjectId).toBe("p1");
  const json = toFile(ws()) as unknown as Record<string, unknown>;
  expect(json.topLevelExtra).toBe(true);
  expect(JSON.stringify(json)).toContain('"extra":"kept"');
  expect(JSON.stringify(json)).toContain('"future":1');
});

test("hydrate gives a project without conversations a default one", () => {
  ws().hydrate({ schemaVersion: 1, activeProjectId: "p1", projects: [{ id: "p1", path: "C:\\x\\proj" } as never] });
  const p = ws().projects[0];
  expect(p.name).toBe("proj");
  expect(activeConversation(p).agents).toEqual([]);
});

test("revealAgent focuses the agent and moves a maximize onto it", async () => {
  ws().addProject("C:\code\a");
  const p = ws().projects[0].id;
  const a = (await ws().launch(p, { command: "x" }))!;
  const b = (await ws().launch(p, { command: "y" }))!;
  const conv = () => activeConversation(ws().projects[0]);

  ws().revealAgent(p, b);
  expect(conv().layout.focusedAgentId).toBe(b);
  expect(conv().layout.maximizedAgentId).toBeNull();
  expect(soloAgentId(conv())).toBeNull();

  ws().toggleMaximize(p, a);
  ws().revealAgent(p, b);
  expect(conv().layout.maximizedAgentId).toBe(b);
  expect(soloAgentId(conv())).toBe(b);

  ws().setLayoutMode(p, "focus");
  ws().revealAgent(p, a);
  expect(soloAgentId(conv())).toBe(a);
});

const claude = { defId: "claude", name: "Claude Code" };
const tick = () => new Promise((r) => setTimeout(r, 0));

function savedProject(agents: object[]) {
  ws().hydrate({
    schemaVersion: 1,
    activeProjectId: "p1",
    projects: [
      {
        id: "p1",
        name: "a",
        path: "C:\a",
        createdAt: "t",
        lastOpenedAt: "t",
        activeConversationId: "c1",
        conversations: [{ id: "c1", title: "Default", createdAt: "t", agents, layout: { mode: "grid", focusedAgentId: null, maximizedAgentId: null } }],
      },
    ],
  } as never);
}

const saved = (id: string, extra: object = {}) => ({ id, launch: { kind: "def", agentDefId: "claude" }, name: "Claude Code", cwd: "C:\a", ...extra });

test("a new agent saves the session id its CLI was given", async () => {
  ws().addProject("C:\a");
  const id = (await ws().launch(ws().activeProjectId!, claude))!;
  expect(spawns).toEqual([{}]);
  expect(agentsOf(ws().activeProjectId!)[0].sessionId).toBe(`sess-${nextPty}`);
  expect(JSON.stringify(toFile(ws()))).toContain(`sess-${nextPty}`);
  expect(ws().runtime[id]).toMatchObject({ resumable: true, resumedAt: null });
});

test("restore resumes saved sessions and leaves hibernated agents alone", async () => {
  savedProject([saved("a1", { sessionId: "s-a" }), saved("a2", { sessionId: "s-b", hibernated: true })]);
  await tick();
  expect(spawns).toEqual([{ sessionId: "s-a", resume: true }]);
  expect(ws().runtime.a1.resumedAt).not.toBeNull();
  expect(ws().runtime.a2).toBeUndefined();
});

test("a resume that fails at once starts a new session with the same id, once", async () => {
  savedProject([saved("f1", { sessionId: "s-a" })]);
  await tick();
  ws().markExited(ws().runtime.f1.ptyId!, 1);
  await tick();
  expect(spawns).toEqual([
    { sessionId: "s-a", resume: true },
    { sessionId: "s-a", resume: false },
  ]);
  expect(ws().runtime.f1).toMatchObject({ exited: false, note: "new session", resumedAt: null });
  // A later failure is not retried.
  ws().markExited(ws().runtime.f1.ptyId!, 1);
  await tick();
  expect(spawns).toHaveLength(2);
});

test("a clean exit after resuming is not treated as a failed resume", async () => {
  savedProject([saved("k1", { sessionId: "s-a" })]);
  await tick();
  ws().markExited(ws().runtime.k1.ptyId!, 0);
  await tick();
  expect(spawns).toHaveLength(1);
  expect(ws().runtime.k1.exited).toBe(true);
});

test("hibernate stops the process and resume continues the session", async () => {
  ws().addProject("C:\a");
  const id = (await ws().launch(ws().activeProjectId!, claude))!;
  const pty = ws().runtime[id].ptyId!;
  const session = agentsOf(ws().activeProjectId!)[0].sessionId;
  await ws().hibernate(id);
  expect(hibernated).toEqual([pty]);
  expect(ws().runtime[id]).toBeUndefined();
  expect(agentsOf(ws().activeProjectId!)[0].hibernated).toBe(true);
  // Its exit event arrives later and changes nothing.
  ws().markExited(pty, 1);
  expect(spawns).toHaveLength(1);

  await ws().resume(id);
  expect(spawns[1]).toEqual({ sessionId: session, resume: true });
  expect(agentsOf(ws().activeProjectId!)[0].hibernated).toBe(false);
  expect(ws().runtime[id].ptyId).not.toBe(pty);
});

test("custom commands cannot be hibernated", async () => {
  ws().addProject("C:\a");
  const id = (await ws().launch(ws().activeProjectId!, { command: "x" }))!;
  await ws().hibernate(id);
  expect(hibernated).toEqual([]);
  expect(ws().runtime[id].ptyId).not.toBeNull();
});

test("auto-hibernate minutes are saved and cleared", async () => {
  ws().addProject("C:\a");
  const id = (await ws().launch(ws().activeProjectId!, claude))!;
  ws().setAutoHibernate(id, 15);
  expect(agentsOf(ws().activeProjectId!)[0].autoHibernateMin).toBe(15);
  ws().setAutoHibernate(id, null);
  expect("autoHibernateMin" in agentsOf(ws().activeProjectId!)[0]).toBe(false);
});

test("New adds an empty conversation and switches to it; the old one keeps its agents", async () => {
  ws().addProject("C:\a");
  const p = ws().activeProjectId!;
  const first = (await ws().launch(p, { command: "x" }))!;
  const firstConv = ws().projects[0].activeConversationId;
  const id = ws().newConversation(p)!;
  const project = ws().projects[0];
  expect(project.conversations.map((c) => c.title)).toEqual(["Default", "Conversation 2"]);
  expect(project.activeConversationId).toBe(id);
  expect(activeConversation(project).agents).toEqual([]);
  // New agents go to the new conversation; nothing is killed.
  const second = (await ws().launch(p, { command: "y" }))!;
  expect(agentsOf(p).map((a) => a.id)).toEqual([second]);
  expect(killed).toEqual([]);
  ws().selectConversation(p, firstConv);
  expect(agentsOf(p).map((a) => a.id)).toEqual([first]);
  expect(ws().runtime[second]?.ptyId).toBeTruthy();
});

test("selectConversation ignores an unknown id, and revealAgent switches to the agent's conversation", async () => {
  ws().addProject("C:\a");
  const p = ws().activeProjectId!;
  const first = (await ws().launch(p, { command: "x" }))!;
  const firstConv = ws().projects[0].activeConversationId;
  ws().newConversation(p);
  const current = ws().projects[0].activeConversationId;
  ws().selectConversation(p, "nope");
  expect(ws().projects[0].activeConversationId).toBe(current);
  ws().revealAgent(p, first);
  expect(ws().projects[0].activeConversationId).toBe(firstConv);
  expect(activeConversation(ws().projects[0]).layout.focusedAgentId).toBe(first);
});

test("revealing an agent in another conversation starts that conversation's saved agents", async () => {
  ws().hydrate({
    schemaVersion: 1,
    activeProjectId: "p1",
    projects: [
      {
        id: "p1",
        name: "a",
        path: "C:\a",
        createdAt: "t",
        lastOpenedAt: "t",
        activeConversationId: "c1",
        conversations: [
          { id: "c1", title: "One", createdAt: "t", agents: [], layout: { mode: "grid", focusedAgentId: null, maximizedAgentId: null } },
          { id: "c2", title: "Two", createdAt: "t", agents: [saved("x1", { sessionId: "s-x" }), saved("x2", { sessionId: "s-y" })], layout: { mode: "grid", focusedAgentId: null, maximizedAgentId: null } },
        ],
      },
    ],
  } as never);
  await tick();
  expect(spawns).toEqual([]);
  ws().revealAgent("p1", "x1");
  await tick();
  expect(ws().projects[0].activeConversationId).toBe("c2");
  expect(spawns).toEqual([{ sessionId: "s-x", resume: true }, { sessionId: "s-y", resume: true }]);
});

test("an isolated agent starts in its own worktree and records it", async () => {
  ws().addProject("C:\code\a");
  const id = (await ws().launch(ws().activeProjectId!, { command: "x" }, { isolate: true }))!;
  const a = findAgent(id);
  expect(a.worktree?.branch).toMatch(/^spectacli\/x-/);
  expect(a.cwd).toBe(a.worktree!.path);
  expect(a.cwd.startsWith("C:\code\a\.spectacli\worktrees\\")).toBe(true);
});

test("a launch that fails discards the worktree it made", async () => {
  ws().addProject("C:\code\a");
  spawnGate = Promise.reject(new Error("boom"));
  spawnGate.catch(() => {});
  await expect(ws().launch(ws().activeProjectId!, { command: "x" }, { isolate: true })).rejects.toThrow("boom");
  expect(removedTrees).toHaveLength(1);
  expect(removedTrees[0].force).toBe(true);
});

test("removing a clean worktree closes the agent and deletes the folder", async () => {
  ws().addProject("C:\code\a");
  const id = (await ws().launch(ws().activeProjectId!, { command: "x" }, { isolate: true }))!;
  const path = findAgent(id).worktree!.path;
  await ws().removeWorktree(id);
  expect(killed.length).toBeGreaterThan(0);
  expect(removedTrees).toEqual([{ path, force: true }]);
  expect(agentsOf(ws().activeProjectId!)).toHaveLength(0);
});

test("a dirty worktree is refused until forced, and nothing is closed meanwhile", async () => {
  ws().addProject("C:\code\a");
  const id = (await ws().launch(ws().activeProjectId!, { command: "x" }, { isolate: true }))!;
  dirtyFiles = 2;
  await expect(ws().removeWorktree(id)).rejects.toMatch(/^dirty: 2 changed files/);
  expect(killed).toHaveLength(0);
  expect(removedTrees).toHaveLength(0);
  await ws().removeWorktree(id, true);
  expect(removedTrees).toHaveLength(1);
});

test("a WSL shell launches in the project folder and is saved as a wsl launch", async () => {
  ws().addProject("C:\code\a");
  const id = (await ws().launch(ws().activeProjectId!, { wslDistro: "Ubuntu" }))!;
  expect(wslSpawns).toEqual([{ distro: "Ubuntu", command: null, cwd: "C:\code\a" }]);
  expect(findAgent(id).launch).toEqual({ kind: "wsl", distro: "Ubuntu" });
  expect(findAgent(id).name).toBe("WSL · Ubuntu");
});

const findAgent = (id: string) => agentsOf(ws().activeProjectId!).find((a) => a.id === id)!;
