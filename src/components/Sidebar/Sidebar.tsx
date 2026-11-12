import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { type DetectedAgent, detectAgents, listWslDistros, onRegistryChanged } from "../../lib/ipc";
import { fmtBytes } from "../../lib/format";
import { useStats } from "../../store/stats";
import { activeConversation, type LaunchWhat, useWorkspace } from "../../store/workspace";
import InlineName from "../InlineName";
import "../InlineName.css";
import "./Sidebar.css";

export default function Sidebar() {
  const {
    projects,
    activeProjectId,
    runtime,
    addProject,
    selectProject,
    removeProject,
    launch,
    closeAgent,
    renameAgent,
    newConversation,
    selectConversation,
  } = useWorkspace();
  const tick = useStats((s) => s.tick);
  const [detected, setDetected] = useState<DetectedAgent[]>([]);
  const [picker, setPicker] = useState(false);
  const [custom, setCustom] = useState("");
  const [isolate, setIsolate] = useState(false);
  const [distros, setDistros] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);

  useEffect(() => {
    void detectAgents().then(setDetected).catch(() => {});
    const un = onRegistryChanged(setDetected);
    void listWslDistros().then(setDistros).catch(() => {});
    return () => void un.then((f) => f());
  }, []);

  const pickFolder = async () => {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir === "string") addProject(dir);
  };

  const start = async (what: LaunchWhat) => {
    if (!activeProjectId) return;
    setPicker(false);
    setCustom("");
    setError(null);
    try {
      await launch(activeProjectId, what, { isolate });
    } catch (e) {
      // Tauri command errors arrive as plain strings.
      setError(typeof e === "string" ? e : String(e));
    }
  };

  const active = projects.find((p) => p.id === activeProjectId);
  const mine = active ? activeConversation(active).agents : [];

  return (
    <aside className="sidebar">
      <div className="sb-head">
        <span>Projects</span>
        <button aria-label="Add project folder" onClick={() => void pickFolder()}>+</button>
      </div>
      <ul className="sb-list">
        {projects.map((p) => (
          <li key={p.id} className={p.id === activeProjectId ? "active" : ""} title={p.path}>
            <button className="sb-main" onClick={() => selectProject(p.id)}>{p.name}</button>
            {(() => {
              const bytes = activeConversation(p).agents.reduce(
                (n, a) => n + (tick?.agents[runtime[a.id]?.ptyId ?? ""]?.privateBytes ?? 0),
                0,
              );
              return bytes > 0 && <small className="sb-stats">{fmtBytes(bytes)}</small>;
            })()}
            <button aria-label={`Remove ${p.name}`} onClick={() => removeProject(p.id)}>×</button>
          </li>
        ))}
      </ul>

      <div className="sb-head">
        <span>Conversations</span>
        <button disabled={!activeProjectId} onClick={() => activeProjectId && newConversation(activeProjectId)}>
          New
        </button>
      </div>
      <ul className="sb-list">
        {active?.conversations.map((c) => (
          <li key={c.id} className={c.id === activeConversation(active).id ? "active" : ""}>
            <button className="sb-main" onClick={() => selectConversation(active.id, c.id)}>{c.title}</button>
            <small className="sb-stats">{c.agents.length}</small>
          </li>
        ))}
      </ul>

      <div className="sb-head">
        <span>Agents</span>
        <button aria-label="Add agent" disabled={!activeProjectId} onClick={() => setPicker((v) => !v)}>+</button>
      </div>
      {picker && (
        <div className="sb-picker">
          {detected.map((d) => (
            <button key={d.def.id} onClick={() => void start({ defId: d.def.id, name: d.def.name })}>
              {d.def.name}
              {d.version && <small> {d.version}</small>}
            </button>
          ))}
          {detected.length === 0 && <small>No CLIs detected.</small>}
          {distros.map((d) => (
            <button key={d} title="A shell inside this WSL distro, in the project folder" onClick={() => void start({ wslDistro: d })}>
              WSL · {d}
            </button>
          ))}
          <label className="sb-check" title="Run the next agent in its own git worktree and branch (needs a git project)">
            <input type="checkbox" checked={isolate} onChange={(e) => setIsolate(e.target.checked)} />
            Isolate in worktree
          </label>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (custom.trim()) void start({ command: custom.trim() });
            }}
          >
            <input placeholder="Custom command" value={custom} onChange={(e) => setCustom(e.target.value)} />
          </form>
        </div>
      )}
      {error && (
        <div className="sb-error" role="alert">
          <span title={error}>{error}</span>
          <button aria-label="Dismiss error" onClick={() => setError(null)}>×</button>
        </div>
      )}
      <ul className="sb-list">
        {mine.map((a) => (
          <li key={a.id} className={runtime[a.id]?.ptyId && !runtime[a.id].exited ? "" : "exited"}>
            <span className="sb-main">
              <InlineName
                value={a.name}
                onCommit={(n) => renameAgent(a.id, n)}
                editing={renamingId === a.id}
                onEditingChange={(v) => setRenamingId(v ? a.id : null)}
              />
              {a.worktree && <small title={a.worktree.path}> ⎇</small>}
              {a.hibernated && <small> hibernated</small>}
            </span>
            {(() => {
              const u = runtime[a.id]?.ptyId ? tick?.agents[runtime[a.id].ptyId!] : undefined;
              return u && <small className="sb-stats" title={`${u.processes} processes`}>{fmtBytes(u.privateBytes)}</small>;
            })()}
            <button aria-label={`Rename ${a.name}`} title="Rename" onClick={() => setRenamingId(a.id)}>
              ✎
            </button>
            <button aria-label={`Close ${a.name}`} onClick={() => closeAgent(a.id)}>×</button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
