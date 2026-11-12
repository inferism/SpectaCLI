import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { type DetectedAgent, detectAgents, onRegistryChanged } from "../../lib/ipc";
import { useWorkspace } from "../../store/workspace";
import "./Sidebar.css";

export default function Sidebar() {
  const { projects, activeProjectId, agents, addProject, selectProject, removeProject, launch, closeAgent } =
    useWorkspace();
  const [detected, setDetected] = useState<DetectedAgent[]>([]);
  const [picker, setPicker] = useState(false);
  const [custom, setCustom] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void detectAgents().then(setDetected).catch(() => {});
    const un = onRegistryChanged(setDetected);
    return () => void un.then((f) => f());
  }, []);

  const pickFolder = async () => {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir === "string") addProject(dir);
  };

  const start = async (what: Parameters<typeof launch>[0]) => {
    setPicker(false);
    setCustom("");
    setError(null);
    try {
      await launch(what);
    } catch (e) {
      setError(String(e));
    }
  };

  const mine = agents.filter((a) => a.projectId === activeProjectId);

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
            <button aria-label={`Remove ${p.name}`} onClick={() => removeProject(p.id)}>×</button>
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
      {error && <div className="sb-error">{error}</div>}
      <ul className="sb-list">
        {mine.map((a) => (
          <li key={a.id} className={a.exited ? "exited" : ""}>
            <span className="sb-main">{a.name}</span>
            <button aria-label={`Close ${a.name}`} onClick={() => closeAgent(a.id)}>×</button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
