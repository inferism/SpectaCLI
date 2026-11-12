import { create } from "zustand";
import type { AgentStatusKind } from "../lib/ipc";

export type InboxKind = "needs_input" | "finished";

export interface InboxItem {
  id: number;
  kind: InboxKind;
  /** The PTY id, as in `agent:status`. */
  ptyId: string;
  projectId: string;
  instanceId: string;
  projectName: string;
  agentName: string;
  /** Unix time in ms. */
  at: number;
}

/** What a status change means for the inbox: asking for input, or finishing a turn. */
export function inboxKind(prev: AgentStatusKind | undefined, next: AgentStatusKind): InboxKind | null {
  if (next === "needs_input" && prev !== "needs_input") return "needs_input";
  if (next === "idle" && prev === "working") return "finished";
  return null;
}

const MAX_ITEMS = 50;
let nextId = 1;

interface InboxState {
  items: InboxItem[];
  add: (item: Omit<InboxItem, "id">) => void;
  remove: (id: number) => void;
  clear: () => void;
  /** The agent is no longer waiting: its "needs input" entries are stale. */
  resolve: (ptyId: string) => void;
}

/** Newest first. Runtime only. */
export const useInbox = create<InboxState>((set) => ({
  items: [],
  add: (item) => set((s) => ({ items: [{ ...item, id: nextId++ }, ...s.items].slice(0, MAX_ITEMS) })),
  remove: (id) => set((s) => ({ items: s.items.filter((i) => i.id !== id) })),
  clear: () => set({ items: [] }),
  resolve: (ptyId) =>
    set((s) => ({ items: s.items.filter((i) => !(i.ptyId === ptyId && i.kind === "needs_input")) })),
}));
