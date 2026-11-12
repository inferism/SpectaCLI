import { expect, test } from "vitest";
import { dueForHibernate } from "./hibernate";

const at = 1_000_000;
const status = (s: "idle" | "working" | "needs_input") => ({ agentId: "p", status: s, source: "hook" as const, at });

test("only an agent idle for the whole delay is due", () => {
  expect(dueForHibernate(5, status("idle"), at + 5 * 60_000)).toBe(true);
  expect(dueForHibernate(5, status("idle"), at + 5 * 60_000 - 1)).toBe(false);
  expect(dueForHibernate(5, status("needs_input"), at + 60 * 60_000)).toBe(false);
  expect(dueForHibernate(5, status("working"), at + 60 * 60_000)).toBe(false);
  expect(dueForHibernate(5, undefined, at + 60 * 60_000)).toBe(false);
  expect(dueForHibernate(undefined, status("idle"), at + 60 * 60_000)).toBe(false);
});
