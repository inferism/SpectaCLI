import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import PaneGrid from "./components/PaneGrid/PaneGrid";
import Sidebar from "./components/Sidebar/Sidebar";
import TitleBar from "./components/TitleBar/TitleBar";
import { onAgentExit } from "./lib/ipc";
import { flushSave, startPersistence } from "./store/persist";
import { useWorkspace } from "./store/workspace";

export default function App() {
  useEffect(() => {
    const un = onAgentExit((e) => useWorkspace.getState().markExited(e.agentId));
    const unPersist = startPersistence();
    // The window closes once this resolves, so the last change is on disk.
    const unClose = getCurrentWindow().onCloseRequested(() => flushSave());
    return () => {
      void un.then((f) => f());
      void unPersist.then((f) => f());
      void unClose.then((f) => f());
    };
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
