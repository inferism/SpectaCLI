import { useEffect, useState } from "react";
import { type DetectedAgent, detectAgents, writeAgent } from "../../lib/ipc";
import { type BroadcastTarget, broadcast } from "../../lib/broadcast";
import { type Conversation, useWorkspace } from "../../store/workspace";

/**
 * One prompt to the running agents of this conversation (all by default).
 * Enter sends, Shift+Enter adds a line.
 */
export default function BroadcastBar({ conv }: { conv: Conversation }) {
  const runtime = useWorkspace((s) => s.runtime);
  const [defs, setDefs] = useState<DetectedAgent[]>([]);
  const [text, setText] = useState("");
  // Unticked agents; a new agent is ticked by default.
  const [off, setOff] = useState<Set<string>>(new Set());
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void detectAgents().then(setDefs).catch(() => {});
  }, []);

  const running = conv.agents.filter((a) => runtime[a.id]?.ptyId && !runtime[a.id].exited);
  const chosen = running.filter((a) => !off.has(a.id));

  const send = async () => {
    if (!text.trim() || chosen.length === 0 || busy) return;
    const targets: (BroadcastTarget & { name: string })[] = chosen.map((a) => {
      const defId = a.launch.kind === "def" ? a.launch.agentDefId : null;
      return {
        name: a.name,
        ptyId: runtime[a.id].ptyId!,
        submit: (defId && defs.find((d) => d.def.id === defId)?.def.submit) || "\r",
        // Plain shells would print the paste markers.
        bracketed: defId !== null,
      };
    });
    setBusy(true);
    setNote(null);
    const failed = await broadcast(text, targets, writeAgent);
    setBusy(false);
    if (failed.length === 0) {
      setText("");
    } else {
      setNote(`Not delivered to ${targets.filter((t) => failed.includes(t.ptyId)).map((t) => t.name).join(", ")}`);
    }
  };

  return (
    <div className="broadcast-bar">
      <div className="broadcast-targets">
        {running.map((a) => (
          <label key={a.id}>
            <input
              type="checkbox"
              checked={!off.has(a.id)}
              onChange={(e) =>
                setOff((prev) => {
                  const next = new Set(prev);
                  if (e.target.checked) next.delete(a.id);
                  else next.add(a.id);
                  return next;
                })
              }
            />
            {a.name}
          </label>
        ))}
        {running.length === 0 && <small>No running agents.</small>}
      </div>
      <div className="broadcast-row">
        <textarea
          rows={2}
          value={text}
          placeholder={`Send to ${chosen.length} agent${chosen.length === 1 ? "" : "s"} (Enter sends, Shift+Enter for a new line)`}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button disabled={!text.trim() || chosen.length === 0 || busy} onClick={() => void send()}>
          Send
        </button>
      </div>
      {note && (
        <small className="broadcast-note" role="alert">
          {note}
        </small>
      )}
    </div>
  );
}
