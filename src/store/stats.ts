import { create } from "zustand";
import type { StatsTick } from "../lib/ipc";

/** Latest `stats:tick`. Runtime only. */
export const useStats = create<{ tick: StatsTick | null }>(() => ({ tick: null }));
