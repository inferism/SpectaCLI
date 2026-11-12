/** At most one toast per agent in this window (PLAN.md feature 4). */
export const TOAST_DEBOUNCE_MS = 10_000;

/**
 * An OS toast is for an agent the user is not looking at: the window is not
 * focused, or the pane is not on screen. Debounced per agent.
 */
export function shouldToast(o: {
  windowFocused: boolean;
  paneShown: boolean;
  lastToastAt: number | undefined;
  now: number;
}): boolean {
  if (o.windowFocused && o.paneShown) return false;
  return o.lastToastAt === undefined || o.now - o.lastToastAt >= TOAST_DEBOUNCE_MS;
}
