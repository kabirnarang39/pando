import { useMemo, useState } from "react";
import { isNotFound, type FileChange } from "../api/client";
import { useFile } from "../data";
import { fold, lineDiff, stats, toSplit, type Line, type Piece } from "../diff";
import { asText, formatBytes, Icon, Skeleton } from "../ui";

export type Layout = "unified" | "split";
/** Diffs above this many lines start collapsed: rendering them is a choice, not a surprise. */
const LARGE = 1500;

/** One file's change, between the change's first parent (`from`) and the change itself (`to`). */
export function FileDiff({ file, from, to, layout, open, onToggle }: { file: FileChange; from: string | undefined; to: string; layout: Layout; open: boolean; onToggle(open: boolean): void }) {
  const deleted = file.blob === "";
  const before = useFile(from, file.path, open && !!from);
  const after = useFile(to, file.path, open && !deleted);
  const added = !from || (before.error !== null && isNotFound(before.error));
  const [forced, setForced] = useState(false);

  const result = useMemo(() => {
    if (!open) return null;
    const a = added ? new Uint8Array() : before.data?.bytes;
    const b = deleted ? new Uint8Array() : after.data?.bytes;
    if (!a || !b) return null;
    const ta = asText(a);
    const tb = asText(b);
    if (ta === null || tb === null) return { binary: true as const, sizes: [a.length, b.length] };
    const lines = lineDiff(ta, tb);
    return { binary: false as const, lines, ...stats(lines) };
  }, [open, added, deleted, before.data, after.data]);

  const kind = deleted ? "deleted" : added && (before.isFetched || !from) ? "added" : null;
  const pending = open && !result && (before.isPending || after.isPending);
  const failed = open && !result && !pending ? (after.error ?? before.error) : null;

  return (
    <details className="file-diff" open={open} onToggle={(e) => onToggle((e.target as HTMLDetailsElement).open)} id={`f-${file.path}`}>
      <summary>
        <span className="chevron" aria-hidden="true">
          <Icon name="chevron" size={14} />
        </span>
        <span className="file-path">{file.path}</span>
        {kind && <span className="badge" data-kind={kind}>{kind}</span>}
        {result && !result.binary && (
          <span className="num" style={{ fontSize: "var(--t-xs)" }}>
            <span className="stat-add">+{result.added}</span> <span className="stat-del">−{result.deleted}</span>
          </span>
        )}
      </summary>
      {pending && <Skeleton rows={4} />}
      {failed && <p className="notice">This file didn't load: {String((failed as Error).message ?? failed)}</p>}
      {result?.binary && (
        <p className="notice">
          Binary file{kind === "added" ? `, ${formatBytes(result.sizes[1]!)}` : kind === "deleted" ? `, was ${formatBytes(result.sizes[0]!)}` : `: ${formatBytes(result.sizes[0]!)} before, ${formatBytes(result.sizes[1]!)} after`}.
        </p>
      )}
      {result && !result.binary && result.lines.length > LARGE && !forced ? (
        <p className="notice">
          This diff has {result.lines.length.toLocaleString()} lines.{" "}
          <button type="button" className="button" onClick={() => setForced(true)}>
            Show the diff
          </button>
        </p>
      ) : (
        result && !result.binary && <Lines lines={result.lines} layout={layout} />
      )}
    </details>
  );
}

function Lines({ lines, layout }: { lines: Line[]; layout: Layout }) {
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const rows = useMemo(() => fold(lines), [lines]);
  if (lines.length === 0) return <p className="notice">The file is empty.</p>;
  const groups: (Line[] | { fold: number; count: number })[] = [];
  for (const r of rows) {
    if (r.kind === "fold" && !expanded.has(r.id)) groups.push({ fold: r.id, count: r.lines.length });
    else {
      const ls = r.kind === "fold" ? r.lines : [r.line];
      const last = groups[groups.length - 1];
      if (Array.isArray(last)) last.push(...ls);
      else groups.push([...ls]);
    }
  }
  return (
    <div className="diff-scroll">
      <div className="diff" data-layout={layout} role="table" aria-label="Changed lines">
        {groups.map((g, i) =>
          Array.isArray(g) ? (
            layout === "split" ? (
              toSplit(g).map((r, k) => <SplitLine key={`${i}-${k}`} left={r.left} right={r.right} />)
            ) : (
              g.map((l, k) => <UnifiedLine key={`${i}-${k}`} line={l} />)
            )
          ) : (
            <button key={`fold-${g.fold}`} type="button" className="fold" onClick={() => setExpanded((s) => new Set(s).add(g.fold))}>
              Show {g.count} unchanged {g.count === 1 ? "line" : "lines"}
            </button>
          ),
        )}
      </div>
    </div>
  );
}

const marker = { add: "+", del: "−", ctx: "" } as const;
const opLabel = { add: "added", del: "removed", ctx: "" } as const;

function Text({ line }: { line: Line }) {
  if (!line.pieces) return <>{line.text || " "}</>;
  const Tag = line.op === "add" ? "ins" : "del";
  return (
    <>
      {line.pieces.map((p: Piece, i) => (p.changed ? <Tag key={i}>{p.text}</Tag> : <span key={i}>{p.text}</span>))}
    </>
  );
}

function UnifiedLine({ line }: { line: Line }) {
  const op = line.op === "ctx" ? undefined : line.op;
  return (
    <div className="drow" data-op={op} role="row">
      <span className="dn num" role="cell">{line.old ?? ""}</span>
      <span className="dn num" role="cell">{line.new ?? ""}</span>
      <span className="dm" role="cell" aria-label={opLabel[line.op] || undefined}>
        {marker[line.op]}
      </span>
      <span className="dsrc" role="cell">
        <Text line={line} />
      </span>
    </div>
  );
}

function SplitLine({ left, right }: { left?: Line; right?: Line }) {
  const lo = left && left.op !== "ctx" ? left.op : undefined;
  const ro = right && right.op !== "ctx" ? right.op : undefined;
  return (
    <div className="drow" role="row">
      <span className="dn num" data-op={lo} role="cell">{left?.old ?? ""}</span>
      <span className="dm" data-op={lo} role="cell" aria-label={lo ? opLabel[lo] : undefined}>
        {lo ? marker[lo] : ""}
      </span>
      <span className="dsrc" data-op={lo} role="cell">
        {left ? <Text line={left} /> : ""}
      </span>
      <span className="dn num" data-op={ro} role="cell">{right?.new ?? ""}</span>
      <span className="dm" data-op={ro} role="cell" aria-label={ro ? opLabel[ro] : undefined}>
        {ro ? marker[ro] : ""}
      </span>
      <span className="dsrc" data-op={ro} role="cell">
        {right ? <Text line={right} /> : ""}
      </span>
    </div>
  );
}
