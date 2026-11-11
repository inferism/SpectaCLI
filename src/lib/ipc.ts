import { invoke } from "@tauri-apps/api/core";
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
