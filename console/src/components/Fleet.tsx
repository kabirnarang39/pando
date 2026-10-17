import { useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { title, type Change } from "../api/client";
import { useRepo } from "../repo";
import { ago, Author, parseAuthor, useNow } from "../ui";

interface Hover {
  c: Change;
  x: number;
  y: number;
}

/** Changes that arrived after the first render get one landing pulse. */
function useFresh(changes: Change[] | undefined) {
  const seen = useRef<Set<string> | null>(null);
  const fresh = new Set<string>();
  if (seen.current && changes) for (const c of changes) if (!seen.current.has(c.id)) fresh.add(c.id);
  useEffect(() => {
    seen.current = new Set((changes ?? []).map((c) => c.id));
  }, [changes]);
  return fresh;
}

function useOpen() {
  const navigate = useNavigate();
  const { owner, name } = useRepo();
  return (c: Change) => navigate({ to: "/$owner/$repo/changes", params: { owner, repo: name }, search: (s) => ({ ...s, c: c.id }) });
}

/**
 * The fleet at work: one lane per author over the last `minutes`, one tick per landing, the
 * newest in gold. Who is busy, who stalled and how the work is spread, at a glance.
 */
export function FleetLanes({ changes, minutes = 60, max = 10 }: { changes: Change[] | undefined; minutes?: number; max?: number }) {
  const now = Math.max(useNow(), Date.now());
  const window = minutes * 60_000;
  const [hover, setHover] = useState<Hover | null>(null);
  const fresh = useFresh(changes);
  const open = useOpen();
  const box = useRef<HTMLDivElement>(null);
  const recent = (changes ?? []).filter((c) => c.time > now - window);
  const byAuthor = new Map<string, Change[]>();
  for (const c of recent) byAuthor.set(c.author, [...(byAuthor.get(c.author) ?? []), c]);
  const lanes = [...byAuthor.entries()].sort((a, b) => b[1].length - a[1].length).slice(0, max);
  const latest = recent[0]?.id;

  if (!changes) return <div className="fleet" aria-busy="true" style={{ minHeight: 120 }} />;
  if (lanes.length === 0) return <p className="notice">Nothing landed in the last {minutes} minutes.</p>;

  return (
    <div className="fleet" ref={box} onMouseLeave={() => setHover(null)} role="img" aria-label={`${recent.length} changes from ${byAuthor.size} authors in the last ${minutes} minutes`}>
      {lanes.map(([author, cs]) => (
        <div className="lane" key={author}>
          <Author author={author} />
          <div className="lane-track">
            {cs.map((c) => {
              const x = 100 - ((now - c.time) / window) * 100;
              return (
                <button
                  key={c.id}
                  type="button"
                  tabIndex={-1}
                  aria-hidden="true"
                  className="tick"
                  data-latest={c.id === latest || undefined}
                  data-fresh={fresh.has(c.id) || undefined}
                  style={{ left: `${Math.min(99.5, Math.max(0.5, x))}%`, "--c": parseAuthor(author).color, "--age": 0.35 + 0.65 * (x / 100) } as React.CSSProperties}
                  onMouseEnter={(e) => {
                    const r = box.current!.getBoundingClientRect();
                    const t = e.currentTarget.getBoundingClientRect();
                    setHover({ c, x: t.left - r.left, y: t.top - r.top });
                  }}
                  onClick={() => open(c)}
                />
              );
            })}
          </div>
          <span className="n num">{cs.length}</span>
        </div>
      ))}
      <div className="fleet-axis" aria-hidden="true">
        <span />
        <div>
          <span>{minutes} min ago</span>
          <span>{minutes / 2} min</span>
          <span>now</span>
        </div>
        <span />
      </div>
      {hover && (
        <div className="fleet-tip" style={{ left: hover.x, top: hover.y }}>
          <strong>{title(hover.c.message)}</strong>
          <div className="muted">
            {hover.c.author}, {ago(hover.c.time, now)}
          </div>
        </div>
      )}
    </div>
  );
}

/** A one-line version for headers: every landing in the last 30 minutes on one track. */
export function Trunk({ changes }: { changes: Change[] | undefined }) {
  const now = Math.max(useNow(), Date.now());
  const window = 30 * 60_000;
  const fresh = useFresh(changes);
  const open = useOpen();
  const recent = (changes ?? []).filter((c) => c.time > now - window);
  const tenMin = recent.filter((c) => c.time > now - 10 * 60_000).length;
  return (
    <div className="trunk">
      <span className="num" style={{ minWidth: "12ch" }}>
        {changes === undefined ? "Connecting…" : `${tenMin} landed in 10 min`}
      </span>
      <div className="trunk-track" role="img" aria-label={`${recent.length} changes landed in the last 30 minutes`}>
        {recent.map((c, i) => {
          const x = 100 - ((now - c.time) / window) * 100;
          return (
            <button
              key={c.id}
              type="button"
              tabIndex={-1}
              aria-hidden="true"
              className="tick"
              title={`${title(c.message)} (${c.author})`}
              data-latest={i === 0 || undefined}
              data-fresh={fresh.has(c.id) || undefined}
              style={{ left: `${Math.min(99.6, Math.max(0, x))}%`, "--c": parseAuthor(c.author).color, "--age": 0.3 + 0.7 * (x / 100) } as React.CSSProperties}
              onClick={() => open(c)}
            />
          );
        })}
      </div>
      <span>30 min</span>
    </div>
  );
}
