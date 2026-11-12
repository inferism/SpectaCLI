import { loadProjects, saveProjects } from "../lib/ipc";
import { type ProjectsFile, toFile, useWorkspace } from "./workspace";

const DEBOUNCE_MS = 500;

let timer: number | undefined;
let pending: string | null = null;
let lastSaved = "";
let chain: Promise<void> = Promise.resolve();

function write(json: string): Promise<void> {
  // One save at a time, in order.
  chain = chain
    .then(() => saveProjects(JSON.parse(json)))
    .then(() => {
      lastSaved = json;
    })
    .catch((e) => console.error("save projects.json:", e));
  return chain;
}

/**
 * Load projects.json into the store, then save on every persisted change.
 * If the load fails nothing is ever saved, so a file we couldn't read (or one
 * from a newer version) is never overwritten. Returns an unsubscribe.
 */
export async function startPersistence(): Promise<() => void> {
  let file: ProjectsFile | null;
  try {
    file = (await loadProjects()) as ProjectsFile | null;
  } catch (e) {
    console.error("load projects.json; changes will not be saved:", e);
    return () => {};
  }
  useWorkspace.getState().hydrate(file);
  lastSaved = file ? JSON.stringify(toFile(useWorkspace.getState())) : "";

  return useWorkspace.subscribe((s) => {
    if (!s.hydrated) return;
    // Runtime-only changes (exits, PTY ids) produce the same JSON and are skipped.
    const json = JSON.stringify(toFile(s));
    if (json === lastSaved || json === pending) return;
    pending = json;
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      const j = pending;
      pending = null;
      if (j) void write(j);
    }, DEBOUNCE_MS);
  });
}

/** Write any debounced change now. Call before the window closes. */
export function flushSave(): Promise<void> {
  clearTimeout(timer);
  const j = pending;
  pending = null;
  return j ? write(j) : chain;
}
