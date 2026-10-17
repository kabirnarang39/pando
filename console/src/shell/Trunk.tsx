import { useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { title, type Change } from "../api/client";
import { ago, useNow } from "../ui";

const WINDOW = 30 * 60_000;

/**
 * One notch per landing over the last 30 minutes, newest (the tip, in gold) on the right. How
 * densely the notches crowd is how busy the fleet is.
 */
export function Trunk({ changes, refName }: { changes: Change[] | undefined; refName: string }) {
  const navigate = useNavigate();
  const now = Math.max(useNow(), Date.now());
  const [hover, setHover] = useState<{ c: Change; x: number } | null>(null);
  const seen = useRef<Set<string> | null>(null);
  const recent = (changes ?? []).filter((c) => c.time > now - WINDOW);
  const tenMin = recent.filter((c) => c.time > now - 10 * 60_000).length;

  // Notches that arrive after the first render get one landing pulse.
  const fresh = new Set<string>();
  if (seen.current) for (const c of recent) if (!seen.current.has(c.id)) fresh.add(c.id);
  useEffect(() => {
    seen.current = new Set((changes ?? []).map((c) => c.id));
  }, [changes]);

  const open = (c: Change) => navigate({ to: "/changes", search: (s) => ({ ...s, c: c.id }) });

  return (
    <div className="trunk" onMouseLeave={() => setHover(null)}>
      <span className="trunk-rate" aria-live="off">
        {changes === undefined ? "Connecting…" : `${tenMin} landed in 10 min`}
      </span>
      <div className="trunk-track" role="img" aria-label={`${recent.length} changes landed on ${refName} in the last 30 minutes`}>
        {recent.map((c, i) => {
          const x = 100 - ((now - c.time) / WINDOW) * 100;
          return (
            <button
              key={c.id}
              type="button"
              tabIndex={-1}
              aria-hidden="true"
              className="notch"
              data-tip={i === 0 || undefined}
              data-fresh={fresh.has(c.id) || undefined}
              style={{ left: `${Math.min(99.6, Math.max(0, x))}%`, "--age": 0.35 + 0.55 * (x / 100) } as React.CSSProperties}
              onMouseEnter={() => setHover({ c, x })}
              onFocus={() => setHover({ c, x })}
              onClick={() => open(c)}
            />
          );
        })}
        {hover && (
          <div className="trunk-tip" style={{ left: `${Math.min(88, Math.max(12, hover.x))}%` }}>
            <strong>{title(hover.c.message)}</strong>
            <div className="muted">
              {hover.c.author}, {ago(hover.c.time, now)}
            </div>
          </div>
        )}
      </div>
      <span className="trunk-axis">30 min</span>
    </div>
  );
}
