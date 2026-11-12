import { create } from "zustand";
import type { AgentStatus } from "../lib/ipc";

/** Latest `agent:status` per agent id. Runtime only. */
export const useStatus = create<{ byAgent: Record<string, AgentStatus> }>(() => ({ byAgent: {} }));

export const setStatus = (s: AgentStatus) =>
  useStatus.setState((st) => ({ byAgent: { ...st.byAgent, [s.agentId]: s } }));
