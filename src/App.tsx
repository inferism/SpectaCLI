import { useEffect } from "react";
import PaneGrid from "./components/PaneGrid/PaneGrid";
import Sidebar from "./components/Sidebar/Sidebar";
import TitleBar from "./components/TitleBar/TitleBar";
import { onAgentExit } from "./lib/ipc";
import { useWorkspace } from "./store/workspace";

export default function App() {
  useEffect(() => {
    const un = onAgentExit((e) => useWorkspace.getState().markExited(e.agentId));
    return () => void un.then((f) => f());
  }, []);

  return (
    <div className="app">
      <TitleBar />
      <div className="app-body">
        <Sidebar />
        <main>
          <PaneGrid />
        </main>
      </div>
    </div>
  );
}
