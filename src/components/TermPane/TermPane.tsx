import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { attachAgent, detachAgent, newAttachId, resizeAgent, writeAgent } from "../../lib/ipc";
import "./TermPane.css";

interface Props {
  ptyId: string;
  /** False while its project is in the background or another pane is maximized. */
  visible: boolean;
  name: string;
  maximized: boolean;
  onToggleMaximize: () => void;
}

const RESIZE_DEBOUNCE_MS = 50;

interface Live {
  term: Terminal;
  /** Opened and attached on first show: xterm can't measure inside display:none. */
  open: () => void;
  opened: boolean;
  sync: () => void;
  gl: WebglAddon | null;
  hadFocus: boolean;
}

export default function TermPane({ ptyId, visible, name, maximized, onToggleMaximize }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const live = useRef<Live | null>(null);

  // The terminal lives as long as the agent, shown or not, so switching
  // projects never loses its screen or scroll position.
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

    let disposed = false;
    let timer: number | undefined;
    const attachId = newAttachId();
    const input = term.onData((d) => void writeAgent(ptyId, d).catch(() => {}));
    const ro = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = window.setTimeout(state.sync, RESIZE_DEBOUNCE_MS);
    });

    const state: Live = {
      term,
      opened: false,
      gl: null,
      hadFocus: false,
      // A hidden pane measures 0x0: never send that to the PTY.
      sync: () => {
        if (!state.opened || !el.clientWidth || !el.clientHeight) return;
        fit.fit();
        void resizeAgent(ptyId, term.cols, term.rows).catch(() => {});
      },
      open: () => {
        state.opened = true;
        term.open(el);
        fit.fit();
        // Size the PTY before replaying scrollback so output wraps correctly.
        void resizeAgent(ptyId, term.cols, term.rows)
          .catch(() => {})
          .then(() => (disposed ? undefined : attachAgent(ptyId, attachId, (b) => term.write(b))));
        ro.observe(el);
      },
    };
    live.current = state;

    return () => {
      disposed = true;
      live.current = null;
      clearTimeout(timer);
      ro.disconnect();
      input.dispose();
      if (state.opened) void detachAgent(ptyId, attachId).catch(() => {});
      term.dispose();
    };
  }, [ptyId]);

  useEffect(() => {
    const state = live.current;
    if (!state) return;
    const { term } = state;
    if (!visible) {
      // WebGL contexts are capped (~16) across all projects: only shown panes keep one.
      state.gl?.dispose();
      state.gl = null;
      state.hadFocus = !!term.textarea && term.textarea === document.activeElement;
      term.blur();
      return;
    }
    if (!state.opened) state.open();
    // WebGL when available; the DOM renderer is the fallback.
    if (!state.gl) {
      try {
        const gl = new WebglAddon();
        gl.onContextLoss(() => {
          gl.dispose();
          if (state.gl === gl) state.gl = null;
        });
        term.loadAddon(gl);
        state.gl = gl;
      } catch {
        /* stay on the DOM renderer */
      }
    }
    state.sync();
    if (state.hadFocus) term.focus();
  }, [visible, ptyId]);

  return (
    <section className="termpane">
      <header className="termpane-header">
        <span className="termpane-dot" title="Status" />
        <span className="termpane-name">{name}</span>
        <button aria-label={maximized ? "Restore pane" : "Maximize pane"} onClick={onToggleMaximize}>
          {maximized ? "❐" : "▢"}
        </button>
      </header>
      <div className="termpane-term" ref={host} />
    </section>
  );
}
