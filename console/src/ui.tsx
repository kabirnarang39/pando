import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { explain } from "./api/client";
import { useConnection } from "./connection";

const paths = {
  code: "M5.5 4 1.5 8l4 4M10.5 4l4 4-4 4",
  changes: "M8 1.5v3M8 11.5v3M8 4.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z",
  lock: "M4 7V5a4 4 0 0 1 8 0v2M3 7h10v7H3z",
  plug: "M6 1.5V5M10 1.5V5M3.5 5h9v2a4.5 4.5 0 0 1-9 0V5ZM8 11.5v3",
  search: "M7 12.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11ZM11 11l3.5 3.5",
  chevron: "M6 3.5 10.5 8 6 12.5",
  branch: "M4.5 2v12M4.5 6.5c0 3 7 1.5 7-2.5M11.5 4V2",
  copy: "M5.5 5.5h8v8h-8zM2.5 10.5v-8h8",
  check: "M2.5 8.5 6 12l7.5-8",
  sun: "M8 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM8 1v1.5M8 13.5V15M1 8h1.5M13.5 8H15M3 3l1 1M12 12l1 1M3 13l1-1M12 4l1-1",
  moon: "M13.5 9.5A5.5 5.5 0 0 1 6.5 2.5a5.5 5.5 0 1 0 7 7Z",
  back: "M10 3.5 5.5 8l4.5 4.5",
  history: "M2 8a6 6 0 1 0 1.8-4.3M2 2v3.5h3.5M8 5v3.5l2.5 1.5",
  file: "M3.5 1.5h6l3 3v10h-9zM9.5 1.5v3h3",
  keyboard: "M1.5 4h13v8h-13zM4 6.5h1M7.5 6.5h1M11 6.5h1M4.5 9.5h7",
} as const;
export type IconName = keyof typeof paths;

export function Icon({ name, size = 16, label }: { name: IconName; size?: number; label?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <path d={paths[name]} />
    </svg>
  );
}

export function Mark({ size = 24 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
      <rect width="64" height="64" rx="14" fill="#16324F" />
      <g fill="#EEF1EC">
        <path d="M8.9 50.5q3 0 3-3l.6-16.5a2 2 0 0 1 4 0l.6 16.5q0 3 3 3Z" />
        <path d="M16.8 50.5q3 0 3-3l.8-30.1a2.4 2.4 0 0 1 4.8 0l.8 30.1q0 3 3 3Z" />
        <path d="M26.2 50.5q3 0 3-3l.7-23.4a2.1 2.1 0 0 1 4.2 0l.7 23.4q0 3 3 3Z" />
        <path d="M34.7 50.5q3 0 3-3l.8-36a2.5 2.5 0 0 1 5 0l.8 36q0 3 3 3Z" />
        <path d="M44.5 50.5q3 0 3-3l.6-18.6a1.9 1.9 0 0 1 3.8 0l.6 18.6q0 3 3 3Z" />
        <rect x="8" y="50" width="48" height="4.5" rx="2.25" />
      </g>
      <path d="M38.5 16.5 38.5 11.5a2.5 2.5 0 0 1 5 0V16.5Z" fill="#F0B429" />
    </svg>
  );
}

// ---- time ----

let now = Date.now();
const subscribers = new Set<() => void>();
setInterval(() => {
  now = Date.now();
  subscribers.forEach((s) => s());
}, 15_000);

/** The current time, updated every 15 seconds for every component that shows an age. */
export function useNow() {
  return useSyncExternalStore(
    (s) => (subscribers.add(s), () => subscribers.delete(s)),
    () => now,
  );
}

export function ago(ms: number, at: number): string {
  const s = Math.max(0, Math.round((at - ms) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d}d ago`;
  return new Date(ms).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

const full = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" });

export function Time({ ms }: { ms: number }) {
  const at = useNow();
  if (!ms) return <span className="muted">no time</span>;
  return (
    <time dateTime={new Date(ms).toISOString()} title={full.format(ms)} className="num">
      {ago(ms, Math.max(at, Date.now() - 15_000))}
    </time>
  );
}

// ---- people and agents ----

function hash(s: string) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

/** Agents are named `agent/<role>-<n>` by convention; they get square marks, people round. */
export function parseAuthor(author: string) {
  const agent = /^(agent|bot)s?\//i.test(author) || /\[bot\]$/.test(author);
  const slash = author.lastIndexOf("/");
  const ns = slash >= 0 ? author.slice(0, slash + 1) : "";
  const name = author.slice(slash + 1) || author || "unknown";
  const words = name.split(/[-_.\s@]+/).filter(Boolean);
  const digits = name.match(/\d+$/)?.[0];
  const initials = digits && words[0] ? words[0][0]! + String(Number(digits)).slice(-1) : words.slice(0, 2).map((w) => w[0]).join("");
  return { agent, ns, name, initials: initials.toUpperCase().slice(0, 2), color: `var(--who-${hash(author) % 8})` };
}

export function Author({ author, compact }: { author: string; compact?: boolean }) {
  const a = parseAuthor(author);
  return (
    <span className="who" title={author}>
      <span className="who-mark" data-agent={a.agent || undefined} style={{ "--c": a.color } as React.CSSProperties} aria-hidden="true">
        {a.initials}
      </span>
      {!compact && (
        <span className="who-name">
          {a.ns && <span className="ns">{a.ns}</span>}
          {a.name}
        </span>
      )}
      {compact && <span className="sr-only">{author}</span>}
    </span>
  );
}

// ---- ids ----

export function CopyId({ id, label = "Copy id" }: { id: string; label?: string }) {
  const [done, setDone] = useState(false);
  useEffect(() => {
    if (!done) return;
    const t = setTimeout(() => setDone(false), 1400);
    return () => clearTimeout(t);
  }, [done]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(id);
      setDone(true);
    } catch {
      window.getSelection()?.selectAllChildren(document.getElementById(`id-${id}`)!);
    }
  };
  return (
    <button type="button" className="copy" onClick={copy} aria-label={`${label} ${id}`} title={id}>
      <span id={`id-${id}`}>{id.slice(0, 12)}</span>
      <Icon name={done ? "check" : "copy"} size={12} />
      <span className="sr-only" aria-live="polite">
        {done ? "Copied" : ""}
      </span>
    </button>
  );
}

// ---- states ----

export function Skeleton({ rows = 6 }: { rows?: number }) {
  return (
    <div className="skeleton" aria-busy="true" aria-label="Loading">
      {Array.from({ length: rows }, (_, i) => (
        <i key={i} style={{ width: `${55 + ((i * 37) % 40)}%` }} />
      ))}
    </div>
  );
}

export function ErrorState({ error, retry, title = "This didn't load" }: { error: unknown; retry?: () => void; title?: string }) {
  const { target } = useConnection();
  return (
    <div className="state" role="alert">
      <h2>{title}</h2>
      <p>{explain(error, target.kind === "server" ? target.url : undefined)}</p>
      {retry && (
        <button type="button" className="button" onClick={retry}>
          Try again
        </button>
      )}
    </div>
  );
}

export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="state">
      <h2>{title}</h2>
      {children && <p>{children}</p>}
    </div>
  );
}

export function formatBytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 ** 2).toFixed(1)} MB`;
}

/** Text if the bytes look like text (no NUL in the first 8 KB and valid UTF-8), else null. */
export function asText(bytes: Uint8Array): string | null {
  if (bytes.subarray(0, 8192).includes(0)) return null;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}
