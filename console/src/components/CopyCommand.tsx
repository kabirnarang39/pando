import { useState } from "react";
import { Icon } from "../ui";

/** A command to run, with a copy button and a line on what it does. */
export function CopyCommand({ label, cmd, note }: { label: string; cmd: string; note: string }) {
  const [done, setDone] = useState(false);
  return (
    <div style={{ display: "grid", gap: 4 }}>
      <h2>{label}</h2>
      <div className="cmd">
        <code>{cmd}</code>
        <button
          type="button"
          className="icon-button"
          aria-label={`Copy ${label.toLowerCase()} command`}
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(cmd);
              setDone(true);
              setTimeout(() => setDone(false), 1400);
            } catch {
              // Clipboard refused: the command is selectable text.
            }
          }}
        >
          <Icon name={done ? "check" : "copy"} size={14} />
        </button>
      </div>
      <p>{note}</p>
    </div>
  );
}
