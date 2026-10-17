import {
  Activity, Bot, Check, ChevronDown, ChevronLeft, ChevronRight, CircleCheck, CircleDot, CircleX, Clock, Code2, Copy,
  ExternalLink, Eye, File, Folder, FolderGit2, GitBranch, GitMerge, GitPullRequest, History, House, Inbox, KeyRound,
  Keyboard, Lock, MessageSquare, Moon, PanelLeft, Plug, Plus, Search, Settings, ShieldCheck, Sun, TriangleAlert, Users, X,
} from "lucide-react";
import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { explain, isNotFound } from "./api/client";
import { Link } from "@tanstack/react-router";
import { useConnection } from "./connection";

const icons = {
  code: Code2, changes: History, lock: Lock, plug: Plug, search: Search, chevron: ChevronRight, down: ChevronDown,
  branch: GitBranch, copy: Copy, check: Check, sun: Sun, moon: Moon, back: ChevronLeft, history: History, file: File,
  folder: Folder, keyboard: Keyboard, home: House, inbox: Inbox, review: GitPullRequest, merge: GitMerge, bot: Bot,
  settings: Settings, repo: FolderGit2, users: Users, key: KeyRound, shield: ShieldCheck, comment: MessageSquare,
  ok: CircleCheck, fail: CircleX, pending: Clock, warn: TriangleAlert, x: X, menu: PanelLeft, activity: Activity,
  dot: CircleDot, eye: Eye, plus: Plus, external: ExternalLink,
} as const;
export type IconName = keyof typeof icons;

export function Icon({ name, size = 16, label }: { name: IconName; size?: number; label?: string }) {
  const C = icons[name];
  return <C size={size} strokeWidth={1.75} className="icon" role={label ? "img" : undefined} aria-label={label} aria-hidden={label ? undefined : true} />;
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

export function Author({ author, compact, large }: { author: string; compact?: boolean; large?: boolean }) {
  const a = parseAuthor(author);
  return (
    <span className="who" title={a.agent ? `${author} (agent)` : author}>
      <span className="who-mark" data-agent={a.agent || undefined} data-size={large ? "lg" : undefined} style={{ "--c": a.color } as React.CSSProperties} aria-hidden="true">
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

/** A page for something that isn't on this server: say what was looked for and where to go. */
export function NotFound({ what }: { what: string }) {
  return (
    <div className="state">
      <h2>There's no {what} here</h2>
      <p>
        It may have been renamed, or you may not have access to it. <Link to="/">Go to Home</Link>
      </p>
    </div>
  );
}

/** An error, or a "not found" page when the error says so. */
export function LoadError({ error, what, title, retry }: { error: unknown; what: string; title: string; retry?(): void }) {
  if (isNotFound(error)) return <NotFound what={what} />;
  return <ErrorState error={error} title={title} retry={retry} />;
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
