import { useRef, useState } from "react";

interface Props {
  /** The name to start editing from. */
  value: string;
  /** What to show instead of `value` while not editing (e.g. with a status note). */
  display?: string;
  onCommit: (name: string) => void;
  className?: string;
  /** Set both to let the parent start editing (e.g. from a pencil button). */
  editing?: boolean;
  onEditingChange?: (editing: boolean) => void;
  /** Accessible name of the text box. */
  label?: string;
}

/** A name that turns into a text box on double-click. Enter or blur saves, Escape cancels. */
export default function InlineName({ value, display, onCommit, className, editing: controlled, onEditingChange, label }: Props) {
  const [own, setOwn] = useState(false);
  const editing = controlled ?? own;
  const setEditing = (v: boolean) => (onEditingChange ? onEditingChange(v) : setOwn(v));

  if (!editing) {
    return (
      <span className={className} title="Double-click to rename" onDoubleClick={() => setEditing(true)}>
        {display ?? value}
      </span>
    );
  }
  return (
    <EditBox
      initial={value}
      label={label ?? "Agent name"}
      onDone={(name) => {
        setEditing(false);
        if (name !== null && name.trim() && name.trim() !== value) onCommit(name);
      }}
    />
  );
}

/** Mounted fresh for each edit, so the draft always starts from the current name. */
function EditBox({ initial, label, onDone }: { initial: string; label: string; onDone: (name: string | null) => void }) {
  const [draft, setDraft] = useState(initial);
  // Unmounting blurs the box; without this, Escape would also save on that blur.
  const finished = useRef(false);
  const finish = (name: string | null) => {
    if (finished.current) return;
    finished.current = true;
    onDone(name);
  };
  return (
    <input
      className="inline-name"
      autoFocus
      value={draft}
      maxLength={60}
      aria-label={label}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => finish(draft)}
      onKeyDown={(e) => {
        if (e.key === "Enter") finish(draft);
        if (e.key === "Escape") finish(null);
      }}
    />
  );
}
