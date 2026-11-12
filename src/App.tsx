import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import PaneGrid from "./components/PaneGrid/PaneGrid";
import Sidebar from "./components/Sidebar/Sidebar";
import TitleBar from "./components/TitleBar/TitleBar";
import { onAgentExit, onAgentStatus, onStatsTick } from "./lib/ipc";
import { flushSave, startPersistence } from "./store/persist";
import { useStats } from "./store/stats";
import { recordStatus, startWaitingTitle } from "./lib/attention";
import { startAutoHibernate } from "./lib/hibernate";
import { useWorkspace } from "./store/workspace";

export default function App() {
  useEffect(() => {
    const un = onAgentExit((e) => useWorkspace.getState().markExited(e.agentId, e.code));
    const unStats = onStatsTick((tick) => useStats.setState({ tick }));
    const unStatus = onAgentStatus(recordStatus);
    const unPersist = startPersistence();
    const stopAutoHibernate = startAutoHibernate();
    const stopWaitingTitle = startWaitingTitle();
    // The window closes once this resolves, so the last change is on disk.
    const unClose = getCurrentWindow().onCloseRequested(() => flushSave());
    return () => {
      stopAutoHibernate();
      stopWaitingTitle();
      void un.then((f) => f());
      void unStats.then((f) => f());
      void unStatus.then((f) => f());
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
