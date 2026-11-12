import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { attachAgent, detachAgent, newAttachId, resizeAgent, writeAgent } from "../../lib/ipc";
import "./TermPane.css";

interface Props {
  agentId: string;
  name: string;
  maximized: boolean;
  onToggleMaximize: () => void;
}

const RESIZE_DEBOUNCE_MS = 50;

export default function TermPane({ agentId, name, maximized, onToggleMaximize }: Props) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = host.current!;
    const term = new Terminal({
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
      theme: { background: "#0f1115", foreground: "#e6e8ee" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    // WebGL when available; the DOM renderer is the fallback. Contexts are
    // capped (~16) and can be lost, so drop the addon instead of failing.
    try {
      const gl = new WebglAddon();
      gl.onContextLoss(() => gl.dispose());
      term.loadAddon(gl);
    } catch {
      /* stay on the DOM renderer */
    }

    let disposed = false;
    let timer: number | undefined;
    const sync = () => {
      fit.fit();
      void resizeAgent(agentId, term.cols, term.rows).catch(() => {});
    };
    const onResize = () => {
      clearTimeout(timer);
      timer = window.setTimeout(sync, RESIZE_DEBOUNCE_MS);
    };

    fit.fit();
    const input = term.onData((d) => void writeAgent(agentId, d).catch(() => {}));
    // Size the PTY before replaying scrollback so output wraps correctly.
    const attachId = newAttachId();
    void resizeAgent(agentId, term.cols, term.rows)
      .catch(() => {})
      .then(() => (disposed ? undefined : attachAgent(agentId, attachId, (b) => term.write(b))));

    const ro = new ResizeObserver(onResize);
    ro.observe(el);

    return () => {
      disposed = true;
      clearTimeout(timer);
      ro.disconnect();
      input.dispose();
      void detachAgent(agentId, attachId).catch(() => {});
      term.dispose();
    };
  }, [agentId]);

  return (
    <section className="termpane">
      <header className="termpane-header">
        <span className="termpane-dot" title="Status" />
        <span className="termpane-name">{name}</span>
        <button
          aria-label={maximized ? "Restore pane" : "Maximize pane"}
          onClick={onToggleMaximize}
        >
          {maximized ? "❐" : "▢"}
        </button>
      </header>
      <div className="termpane-term" ref={host} />
    </section>
  );
}
