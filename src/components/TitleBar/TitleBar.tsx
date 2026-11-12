import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { fmtBytes, fmtCpu } from "../../lib/format";
import { useStats } from "../../store/stats";
import Inbox from "../Inbox/Inbox";
import "./TitleBar.css";

export default function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    const sync = () => void win.isMaximized().then(setMaximized);
    sync();
    const unlisten = win.onResized(sync);
    return () => void unlisten.then((f) => f());
  }, []);

  const tick = useStats((s) => s.tick);
  const agents = tick ? Object.values(tick.agents) : [];
  const agentBytes = agents.reduce((n, u) => n + u.privateBytes, 0);
  const agentCpu = agents.reduce((n, u) => n + u.cpu, 0);

  const win = getCurrentWindow();

  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar-title" data-tauri-drag-region>
        SpectaCLI
      </span>
      {tick && (
        <span
          className="titlebar-stats"
          data-tauri-drag-region
          title="Private bytes. Agents are the CLI process trees; Spectacli is the app, its WebView2 and the ConPTY hosts."
        >
          Agents {fmtBytes(agentBytes)} · {fmtCpu(agentCpu)} · Spectacli {fmtBytes(tick.spectacli.privateBytes)}
        </span>
      )}
      <div className="titlebar-buttons">
        <Inbox />
        <button aria-label="Minimize" onClick={() => void win.minimize()}>
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 5h10" stroke="currentColor" /></svg>
        </button>
        <button
          aria-label={maximized ? "Restore" : "Maximize"}
          onClick={() => void win.toggleMaximize()}
        >
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor">
              <path d="M2.5 2.5V.5h7v7h-2" /><rect x=".5" y="2.5" width="7" height="7" />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor">
              <rect x=".5" y=".5" width="9" height="9" />
            </svg>
          )}
        </button>
        <button className="close" aria-label="Close" onClick={() => void win.close()}>
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" /></svg>
        </button>
      </div>
    </header>
  );
}
