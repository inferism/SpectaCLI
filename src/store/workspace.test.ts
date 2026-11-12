import { beforeEach, expect, test, vi } from "vitest";

let nextPty = 0;
let spawnGate: Promise<void> = Promise.resolve();
const killed: string[] = [];
vi.mock("../lib/ipc", () => ({
  spawnAgent: async () => {
    await spawnGate;
    return `pty-${++nextPty}`;
  },
  spawnCustom: async () => {
    await spawnGate;
    return `pty-${++nextPty}`;
  },
  killAgent: async (id: string) => void killed.push(id),
}));

const { activeConversation, soloAgentId, toFile, useWorkspace } = await import("./workspace");
const ws = () => useWorkspace.getState();
const agentsOf = (projectId: string) => activeConversation(ws().projects.find((p) => p.id === projectId)!).agents;

beforeEach(() => {
  useWorkspace.setState({ hydrated: false, fileExtra: {}, projects: [], activeProjectId: null, runtime: {} });
  killed.length = 0;
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
