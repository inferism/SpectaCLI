import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { type RefObject, useEffect, useLayoutEffect, useRef } from "react";
import {
  type AgentStatus,
  type AgentStatusKind,
  attachAgent,
  detachAgent,
  newAttachId,
  resizeAgent,
  writeAgent,
} from "../../lib/ipc";
import { fmtUsage, usageTitle } from "../../lib/format";
import type { AgentWorktree } from "../../store/workspace";
import { useStats } from "../../store/stats";
import { useStatus } from "../../store/status";
import "./TermPane.css";
import InlineName from "../InlineName";
import "../InlineName.css";
import WorktreeMenu from "./WorktreeMenu";

const STATUS_LABEL: Record<AgentStatusKind, string> = {
  starting: "Starting",
  working: "Working",
  idle: "Idle",
  needs_input: "Needs input",
  exited: "Exited",
};

const SOURCE_LABEL: Record<AgentStatus["source"], string> = {
  launch: "since launch",
  hook: "reported by the CLI's hook",
  terminal: "from a bell or notification in its output",
  heuristic: "guessed from its output, may be wrong",
  exit: "process ended",
};

function statusTitle(s: AgentStatus | undefined): string {
  if (!s) return "No status signal for this agent";
  return `${STATUS_LABEL[s.status]} · ${SOURCE_LABEL[s.source]} · ${new Date(s.at).toLocaleTimeString()}`;
}

interface Props {
  ptyId: string;
  /** False while its project is in the background, another pane is maximized, or the window is minimized. */
  visible: boolean;
  name: string;
  /** The agent's own name (without notes like "(exited)"), for renaming. */
  rawName: string;
  onRename: (name: string) => void;
  maximized: boolean;
  onToggleMaximize: () => void;
  /** Set when the agent runs in its own git worktree. */
  worktree?: { instanceId: string; projectPath: string; info: AgentWorktree };
  /** Set when the agent can be hibernated (its CLI can resume). */
  hibernation?: {
    autoMin: number | null;
    onHibernate: () => void;
    onAutoChange: (minutes: number | null) => void;
  };
}

const RESIZE_DEBOUNCE_MS = 50;
const AUTO_HIBERNATE_CHOICES = [5, 15, 30, 60];

/**
 * A hidden pane has no terminal at all (PLAN.md feature 3): Rust keeps
 * reading and buffering the agent's output, and showing the pane again
 * mounts a fresh xterm that replays the buffer. A hidden pane costs no
 * parsing, rendering, IPC or WebGL context.
 */
export default function TermPane({ ptyId, visible, name, rawName, onRename, maximized, onToggleMaximize, hibernation, worktree }: Props) {
  const usage = useStats((s) => s.tick?.agents[ptyId]);
  const status = useStatus((s) => s.byAgent[ptyId]);
  // Survives the terminal, so focus comes back with it.
  const hadFocus = useRef(false);

  return (
    <section className="termpane" data-pty={ptyId}>
      <header className="termpane-header">
        <span className={`termpane-dot${status ? ` is-${status.status}` : ""}`} title={statusTitle(status)} />
        <InlineName className="termpane-name" value={rawName} display={name} onCommit={onRename} />
        {usage && (
          <span className="termpane-stats" title={usageTitle(usage)}>
            {fmtUsage(usage)}
          </span>
        )}
        {worktree && <WorktreeMenu instanceId={worktree.instanceId} projectPath={worktree.projectPath} worktree={worktree.info} />}
        {hibernation && (
          <>
            <select
              className="termpane-auto"
              aria-label="Auto-hibernate when idle"
              title="Hibernate automatically after this long idle (needs a status signal from the CLI)"
              value={hibernation.autoMin ?? ""}
              onChange={(e) => hibernation.onAutoChange(e.target.value ? Number(e.target.value) : null)}
            >
              <option value="">Auto off</option>
              {AUTO_HIBERNATE_CHOICES.map((m) => (
                <option key={m} value={m}>
                  Idle {m} min
                </option>
              ))}
            </select>
            <button
              aria-label="Hibernate"
              title="Hibernate: stop the CLI to free its memory and resume the session later"
              onClick={hibernation.onHibernate}
            >
              ⏾
            </button>
          </>
        )}
        <button
          aria-label={maximized ? "Restore pane" : "Maximize pane"}
          title={`${maximized ? "Restore" : "Maximize"} (Ctrl+Shift+Enter)`}
          onClick={onToggleMaximize}
        >
          {maximized ? "❐" : "▢"}
        </button>
      </header>
      <div className="termpane-term">{visible && <Xterm key={ptyId} ptyId={ptyId} hadFocus={hadFocus} />}</div>
    </section>
  );
}

function Xterm({ ptyId, hadFocus }: { ptyId: string; hadFocus: RefObject<boolean> }) {
  const host = useRef<HTMLDivElement>(null);

  // A layout cleanup still sees the DOM, so it can tell whether we had focus.
  useLayoutEffect(() => {
    const el = host.current!;
    return () => {
      hadFocus.current = el.contains(document.activeElement);
    };
  }, [hadFocus]);

  useEffect(() => {
    const el = host.current!;
    const term = new Terminal({
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
      theme: { background: "#070708", foreground: "#f1f1f2" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);

    let disposed = false;
    let opened = false;
    let timer: number | undefined;
    // While the scrollback replays, xterm answers the queries in it (cursor
    // position, colours, device attributes). The CLI asked those long ago:
    // keep the answers out of its input. Keys typed in that instant drop too.
    let muted = false;
    const attachId = newAttachId();
    const input = term.onData((d) => {
      if (!muted) void writeAgent(ptyId, d).catch(() => {});
    });
    const write = (b: Uint8Array) => {
      if (!disposed) term.write(b);
    };
    const replay = (b: Uint8Array) => {
      if (disposed) return;
      muted = true;
      term.write(b, () => (muted = false));
    };

    // xterm can't measure a 0x0 element (a hidden or minimized window), so it
    // opens on the first real size, and a 0x0 size is never sent to the PTY.
    const open = () => {
      opened = true;
      term.open(el);
      // WebGL when available; the DOM renderer is the fallback.
      try {
        const gl = new WebglAddon();
        gl.onContextLoss(() => gl.dispose());
        term.loadAddon(gl);
      } catch {
        /* stay on the DOM renderer */
      }
      fit.fit();
      if (hadFocus.current) term.focus();
      // Size the PTY before replaying scrollback so output wraps correctly.
      void resizeAgent(ptyId, term.cols, term.rows)
        .catch(() => {})
        .then(() => (disposed ? undefined : attachAgent(ptyId, attachId, write, replay)))
        // Unmounted while attaching: the detach may have reached Rust first.
        .then(() => (disposed ? detachAgent(ptyId, attachId) : undefined))
        .catch(() => {});
    };
    const sync = () => {
      if (!el.clientWidth || !el.clientHeight) return;
      if (!opened) return open();
      fit.fit();
      void resizeAgent(ptyId, term.cols, term.rows).catch(() => {});
    };
    const ro = new ResizeObserver(() => {
      clearTimeout(timer);
      if (opened) timer = window.setTimeout(sync, RESIZE_DEBOUNCE_MS);
      else sync();
    });
    ro.observe(el);

    // Dragging the window to a monitor with another scale factor changes the
    // cell size in device pixels without resizing the element, so the
    // ResizeObserver stays silent. Refit when the pixel ratio changes.
    let dprQuery: MediaQueryList | undefined;
    const onDpr = () => {
      if (disposed) return;
      sync();
      watchDpr();
    };
    const watchDpr = () => {
      dprQuery?.removeEventListener("change", onDpr);
      dprQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      dprQuery.addEventListener("change", onDpr, { once: true });
    };
    watchDpr();

    return () => {
      disposed = true;
      dprQuery?.removeEventListener("change", onDpr);
      clearTimeout(timer);
      ro.disconnect();
      input.dispose();
      if (opened) void detachAgent(ptyId, attachId).catch(() => {});
      term.dispose();
    };
  }, [ptyId, hadFocus]);

  return <div className="termpane-xterm" ref={host} />;
}
