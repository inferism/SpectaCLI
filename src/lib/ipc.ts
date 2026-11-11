import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// Mirrors src-tauri/src/registry (serialized as camelCase).
export interface AgentDef {
  id: string;
  name: string;
  commands: string[];
  versionArgs: string[];
  versionRegex: string | null;
  newSessionArgs: string[];
  resumeArgs: string[];
  continueArgs: string[];
  hook: { kind: string } | null;
  configDirEnv: string | null;
  imagePaste: "path" | "native" | null;
  submit: string;
}

export interface LaunchSpec {
  kind: "exe" | "node" | "cmdShim";
  program: string;
  args: string[];
}

export interface DetectedAgent {
  def: AgentDef;
  path: string;
  launch: LaunchSpec;
  version: string | null;
}

export const detectAgents = () => invoke<DetectedAgent[]>("detect_agents");

/** Fired once startup detection finishes (and later when the registry changes). */
export const onRegistryChanged = (cb: (agents: DetectedAgent[]) => void): Promise<UnlistenFn> =>
  listen<DetectedAgent[]>("registry:changed", (e) => cb(e.payload));

// --- Agents in PTYs (src-tauri/src/pty) ---

export const spawnAgent = (agentDefId: string, cwd: string, cols: number, rows: number) =>
  invoke<string>("spawn_agent", { agentDefId, cwd, cols, rows });

/**
 * Stream an agent's output to `onData`: the scrollback first, then live
 * batches, in order. Replaces any pane already attached to this agent.
 */
export async function attachAgent(agentId: string, onData: (bytes: Uint8Array) => void): Promise<void> {
  // Live batches can arrive before the scrollback response; hold them until
  // the scrollback has been written. The two never overlap (Rust guarantees it).
  let early: Uint8Array[] | null = [];
  const onOutput = new Channel<ArrayBuffer>();
  onOutput.onmessage = (buf) => {
    const bytes = new Uint8Array(buf);
    if (early) early.push(bytes);
    else onData(bytes);
  };
  const scrollback = await invoke<ArrayBuffer>("attach_agent", { agentId, onOutput });
  onData(new Uint8Array(scrollback));
  early.forEach(onData);
  early = null;
}

/** Stop streaming (pane hidden). The agent keeps running and buffering. */
export const detachAgent = (agentId: string) => invoke<void>("detach_agent", { agentId });
export const writeAgent = (agentId: string, data: string) => invoke<void>("write", { agentId, data });
export const resizeAgent = (agentId: string, cols: number, rows: number) =>
  invoke<void>("resize", { agentId, cols, rows });
/** Kills the agent's whole process tree. `agent:exit` still fires. */
export const killAgent = (agentId: string) => invoke<void>("kill_agent", { agentId });

export interface AgentExit {
  agentId: string;
  /** null when the exit code could not be read. */
  code: number | null;
}

/** Fired after an agent's process exits and its last output was sent. */
export const onAgentExit = (cb: (e: AgentExit) => void): Promise<UnlistenFn> =>
  listen<AgentExit>("agent:exit", (e) => cb(e.payload));
