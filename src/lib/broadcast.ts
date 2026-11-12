// Broadcast bar (PLAN.md phase 4): one prompt to several agents, sent as a
// bracketed paste followed by each CLI's registry `submit` key.

const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";

/** Gap between the paste and the submit key: a TUI that sees them together can take Enter as part of the paste. */
export const SUBMIT_DELAY_MS = 150;

export interface BroadcastTarget {
  ptyId: string;
  /** The registry's submit bytes (default Enter). */
  submit: string;
  /** Wrap the text as a bracketed paste. False for plain shells, which would show the markers. */
  bracketed: boolean;
}

/** The bytes that put `text` into a prompt. Escape characters are dropped so the text cannot end the paste early. */
export function pasteBytes(text: string, bracketed: boolean): string {
  const clean = text.replace(/\x1b/g, "").replace(/\r\n?/g, "\n");
  return bracketed ? PASTE_START + clean + PASTE_END : clean.replace(/\n/g, " ");
}

/**
 * Paste `text` into every target, wait once, then submit in each. Returns the
 * pty ids whose write failed (an agent that exited meanwhile).
 */
export async function broadcast(
  text: string,
  targets: BroadcastTarget[],
  write: (ptyId: string, data: string) => Promise<void>,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): Promise<string[]> {
  const failed = new Set<string>();
  const send = (t: BroadcastTarget, data: string) => write(t.ptyId, data).catch(() => void failed.add(t.ptyId));
  await Promise.all(targets.map((t) => send(t, pasteBytes(text, t.bracketed))));
  await sleep(SUBMIT_DELAY_MS);
  await Promise.all(targets.filter((t) => !failed.has(t.ptyId)).map((t) => send(t, t.submit)));
  return [...failed];
}
