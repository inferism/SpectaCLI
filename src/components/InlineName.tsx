import { useState } from "react";

interface Props {
  /** The name to start editing from. */
  value: string;
  /** What to show instead of `value` while not editing (e.g. with a status note). */
  display?: string;
  onCommit: (name: string) => void;
  className?: string;
}

/** A name that turns into a text box on double-click. Enter or blur saves, Escape cancels. */
export default function InlineName({ value, display, onCommit, className }: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);

  if (!editing) {
    return (
      <span
        className={className}
        title="Double-click to rename"
        onDoubleClick={() => {
          setDraft(value);
          setEditing(true);
        }}
      >
        {display ?? value}
      </span>
    );
  }
  const finish = (save: boolean) => {
    setEditing(false);
    if (save && draft.trim() && draft.trim() !== value) onCommit(draft);
  };
  return (
    <input
      className="inline-name"
      autoFocus
      value={draft}
      maxLength={60}
      aria-label="Agent name"
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => finish(true)}
      onKeyDown={(e) => {
        if (e.key === "Enter") finish(true);
        if (e.key === "Escape") finish(false);
      }}
    />
  );
}
