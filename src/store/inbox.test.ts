import { beforeEach, describe, expect, it } from "vitest";
import { inboxKind, useInbox } from "./inbox";

const item = (ptyId: string, kind: "needs_input" | "finished") => ({
  kind,
  ptyId,
  projectId: "p",
  instanceId: "i",
  projectName: "P",
  agentName: "A",
  at: 0,
});

describe("inboxKind", () => {
  it("flags a request for input once, and a finished turn", () => {
    expect(inboxKind("working", "needs_input")).toBe("needs_input");
    expect(inboxKind(undefined, "needs_input")).toBe("needs_input");
    expect(inboxKind("needs_input", "needs_input")).toBeNull();
    expect(inboxKind("working", "idle")).toBe("finished");
  });
  it("ignores startup and exit", () => {
    expect(inboxKind("starting", "idle")).toBeNull();
    expect(inboxKind(undefined, "starting")).toBeNull();
    expect(inboxKind("working", "exited")).toBeNull();
    expect(inboxKind("needs_input", "idle")).toBeNull();
  });
});

describe("useInbox", () => {
  beforeEach(() => useInbox.getState().clear());
  it("lists newest first and caps the list", () => {
    for (let n = 0; n < 60; n++) useInbox.getState().add(item(`a${n}`, "finished"));
    const items = useInbox.getState().items;
    expect(items).toHaveLength(50);
    expect(items[0].ptyId).toBe("a59");
  });
  it("resolve drops only that agent's needs-input entries", () => {
    const { add, resolve } = useInbox.getState();
    add(item("a", "needs_input"));
    add(item("a", "finished"));
    add(item("b", "needs_input"));
    resolve("a");
    expect(useInbox.getState().items.map((i) => `${i.ptyId}:${i.kind}`)).toEqual(["b:needs_input", "a:finished"]);
  });
});
