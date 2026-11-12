import { useEffect, useRef, useState } from "react";
import { openInboxItem } from "../../lib/attention";
import { type InboxKind, useInbox } from "../../store/inbox";
import "./Inbox.css";

const LABEL: Record<InboxKind, string> = { needs_input: "needs input", finished: "finished" };

/** The title-bar bell: agents that asked for input or finished. Click an entry to jump to its pane. */
export default function Inbox() {
  const items = useInbox((s) => s.items);
  const clear = useInbox((s) => s.clear);
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div className="inbox" ref={root}>
      <button
        className="inbox-bell"
        aria-label={`Inbox, ${items.length} waiting`}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3">
          <path d="M8 1.5a4 4 0 0 0-4 4v2.6L2.6 11h10.8L12 8.1V5.5a4 4 0 0 0-4-4zM6.4 13a1.7 1.7 0 0 0 3.2 0" />
        </svg>
        {items.length > 0 && <span className="inbox-badge">{items.length > 9 ? "9+" : items.length}</span>}
      </button>
      {open && (
        <div className="inbox-panel" role="dialog" aria-label="Inbox">
          <div className="inbox-head">
            <span>Inbox</span>
            {items.length > 0 && <button onClick={clear}>Clear</button>}
          </div>
          {items.length === 0 ? (
            <p className="inbox-empty">Nothing waiting. Agents that need input or finish show up here.</p>
          ) : (
            <ul>
              {items.map((i) => (
                <li key={i.id}>
                  <button
                    onClick={() => {
                      setOpen(false);
                      openInboxItem(i);
                    }}
                  >
                    <span className={`inbox-dot is-${i.kind}`} />
                    <span className="inbox-text">
                      <b>{i.agentName}</b> {LABEL[i.kind]}
                      <small>{i.projectName}</small>
                    </span>
                    <time>{new Date(i.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
