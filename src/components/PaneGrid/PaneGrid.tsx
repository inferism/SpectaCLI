import { activeConversation, type Project, useWorkspace } from "../../store/workspace";
import TermPane from "../TermPane/TermPane";
import { useWindowShown } from "../../lib/windowShown";
import { gridDims } from "./gridDims";
import "./PaneGrid.css";

/**
 * One grid per project with running agents, all mounted: switching projects
 * hides a grid instead of unmounting it. Hidden panes drop their terminals
 * (see TermPane) and replay the agent's scrollback when shown again.
 */
export default function PaneGrid() {
  const projects = useWorkspace((s) => s.projects);
  const activeProjectId = useWorkspace((s) => s.activeProjectId);
  const runtime = useWorkspace((s) => s.runtime);
  const windowShown = useWindowShown();

  const active = projects.find((p) => p.id === activeProjectId);
  const hasRunning = (p: Project) => activeConversation(p).agents.some((a) => runtime[a.id]?.ptyId);
  const mounted = projects.filter((p) => p.id === activeProjectId || hasRunning(p));

  return (
    <>
      {mounted.map((p) => (
        <ProjectGrid key={p.id} project={p} visible={p.id === activeProjectId} windowShown={windowShown} />
      ))}
      {(!active || activeConversation(active).agents.length === 0) && (
        <div className="grid-empty">{active ? "Add an agent with + in the sidebar." : "Add a project folder to start."}</div>
      )}
    </>
  );
}

function ProjectGrid({ project, visible, windowShown }: { project: Project; visible: boolean; windowShown: boolean }) {
  const runtime = useWorkspace((s) => s.runtime);
  const toggleMaximize = useWorkspace((s) => s.toggleMaximize);
  const conv = activeConversation(project);
  const agents = conv.agents;
  if (agents.length === 0) return null;

  const { cols, rows } = gridDims(agents.length);
  const maximized = agents.some((a) => a.id === conv.layout.maximizedAgentId) ? conv.layout.maximizedAgentId : null;
  // Past 3x3 the rows stay at a third of the viewport and the grid scrolls.
  const rowSize = `calc(100% / ${Math.min(rows, 3)})`;

  return (
    <div
      className="grid"
      style={{
        ...(maximized
          ? { gridTemplateColumns: "1fr", gridAutoRows: "100%" }
          : { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gridAutoRows: rowSize }),
        ...(visible ? {} : { display: "none" }),
      }}
    >
      {agents.map((a) => {
        const rt = runtime[a.id];
        const shown = !maximized || maximized === a.id;
        const name = rt?.exited ? `${a.name} (exited)` : a.name;
        return (
          <div key={a.id} className="grid-cell" style={shown ? undefined : { display: "none" }}>
            {rt?.ptyId ? (
              <TermPane
                ptyId={rt.ptyId}
                visible={visible && shown && windowShown}
                name={name}
                maximized={maximized === a.id}
                onToggleMaximize={() => toggleMaximize(project.id, a.id)}
              />
            ) : (
              <section className="termpane">
                <header className="termpane-header">
                  <span className="termpane-name">{a.name}</span>
                </header>
                <div className="grid-empty">Not running</div>
              </section>
            )}
          </div>
        );
      })}
    </div>
  );
}
