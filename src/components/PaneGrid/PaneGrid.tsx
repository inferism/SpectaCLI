import { useWorkspace } from "../../store/workspace";
import TermPane from "../TermPane/TermPane";
import { gridDims } from "./gridDims";
import "./PaneGrid.css";

export default function PaneGrid() {
  const { agents, activeProjectId, maximizedId, toggleMaximize } = useWorkspace();
  const shown = agents.filter((a) => a.projectId === activeProjectId);
  const { cols, rows } = gridDims(shown.length);
  const maximized = shown.some((a) => a.id === maximizedId) ? maximizedId : null;

  if (shown.length === 0) {
    return (
      <div className="grid-empty">
        {activeProjectId ? "Add an agent with + in the sidebar." : "Add a project folder to start."}
      </div>
    );
  }

  // Past 3x3 the rows stay at a third of the viewport and the grid scrolls.
  const rowSize = `calc(100% / ${Math.min(rows, 3)})`;
  return (
    <div
      className="grid"
      style={
        maximized
          ? { gridTemplateColumns: "1fr", gridAutoRows: "100%" }
          : { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gridAutoRows: rowSize }
      }
    >
      {shown.map((a) => (
        // Hidden panes stay mounted so their terminals keep state.
        <div key={a.id} className="grid-cell" style={maximized && maximized !== a.id ? { display: "none" } : undefined}>
          <TermPane
            agentId={a.id}
            name={a.exited ? `${a.name} (exited)` : a.name}
            maximized={maximized === a.id}
            onToggleMaximize={() => toggleMaximize(a.id)}
          />
        </div>
      ))}
    </div>
  );
}
