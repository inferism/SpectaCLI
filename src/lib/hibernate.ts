import { useStatus } from "../store/status";
import { useWorkspace } from "../store/workspace";
import type { AgentStatus } from "./ipc";

const CHECK_MS = 30_000;

/** Auto-hibernate only an agent known to be idle (not one waiting on the user) for the whole delay. */
export function dueForHibernate(autoHibernateMin: number | undefined, status: AgentStatus | undefined, now: number): boolean {
  if (!autoHibernateMin || status?.status !== "idle") return false;
  return now - status.at >= autoHibernateMin * 60_000;
}

/** Hibernate agents past their idle limit, checking every 30 s. Returns a stop function. */
export function startAutoHibernate(): () => void {
  const timer = window.setInterval(() => {
    const { projects, runtime, hibernate } = useWorkspace.getState();
    const status = useStatus.getState().byAgent;
    const now = Date.now();
    for (const p of projects)
      for (const c of p.conversations)
        for (const a of c.agents) {
          const rt = runtime[a.id];
          if (!rt?.ptyId || rt.exited || !rt.resumable || a.hibernated) continue;
          if (dueForHibernate(a.autoHibernateMin, status[rt.ptyId], now)) void hibernate(a.id);
        }
  }, CHECK_MS);
  return () => window.clearInterval(timer);
}
