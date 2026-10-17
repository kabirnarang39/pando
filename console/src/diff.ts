import { diffArrays, diffWordsWithSpace } from "diff";

export type Op = "ctx" | "add" | "del";
/** A piece of a changed line; `changed` pieces are what differs from its counterpart. */
export type Piece = { text: string; changed: boolean };
export interface Line {
  op: Op;
  old?: number;
  new?: number;
  text: string;
  pieces?: Piece[];
}
export type Row = { kind: "line"; line: Line } | { kind: "fold"; id: number; lines: Line[] };
export interface SplitRow {
  left?: Line;
  right?: Line;
}

const WORD_DIFF_MAX = 400;

export const splitLines = (text: string) => {
  const ls = text.split("\n");
  if (ls.length > 1 && ls[ls.length - 1] === "") ls.pop();
  return ls;
};

/** Every line of `a` → `b` as context, deletion or addition, with word-level pieces. */
export function lineDiff(a: string, b: string): Line[] {
  const out: Line[] = [];
  let o = 1;
  let n = 1;
  for (const part of diffArrays(splitLines(a), splitLines(b))) {
    for (const text of part.value) {
      if (part.added) out.push({ op: "add", new: n++, text });
      else if (part.removed) out.push({ op: "del", old: o++, text });
      else out.push({ op: "ctx", old: o++, new: n++, text });
    }
  }
  pairWords(out);
  return out;
}

/** A run of deletions followed by additions is a rewrite: mark which words changed. */
function pairWords(lines: Line[]) {
  for (let i = 0; i < lines.length; ) {
    if (lines[i]!.op !== "del") {
      i++;
      continue;
    }
    let d = i;
    while (d < lines.length && lines[d]!.op === "del") d++;
    let a = d;
    while (a < lines.length && lines[a]!.op === "add") a++;
    const pairs = Math.min(d - i, a - d);
    for (let k = 0; k < pairs; k++) {
      const del = lines[i + k]!;
      const add = lines[d + k]!;
      if (del.text.length > WORD_DIFF_MAX || add.text.length > WORD_DIFF_MAX) continue;
      const parts = diffWordsWithSpace(del.text, add.text);
      // Mostly different lines read better whole than as confetti.
      const same = parts.filter((p) => !p.added && !p.removed).reduce((s, p) => s + p.value.trim().length, 0);
      if (same < Math.min(del.text.trim().length, add.text.trim().length) * 0.4) continue;
      del.pieces = parts.filter((p) => !p.added).map((p) => ({ text: p.value, changed: !!p.removed }));
      add.pieces = parts.filter((p) => !p.removed).map((p) => ({ text: p.value, changed: !!p.added }));
    }
    i = a;
  }
}

/**
 * Unchanged runs longer than twice `context` fold away, keeping `context` lines around changes.
 * Lines in `keep` (by new line number, e.g. lines with comments) never fold.
 */
export function fold(lines: Line[], context = 3, keep?: Set<number>): Row[] {
  const rows: Row[] = [];
  let id = 0;
  const folds = (l: Line) => l.op === "ctx" && !keep?.has(l.new!);
  for (let i = 0; i < lines.length; ) {
    if (!folds(lines[i]!)) {
      rows.push({ kind: "line", line: lines[i++]! });
      continue;
    }
    let j = i;
    while (j < lines.length && folds(lines[j]!)) j++;
    const head = i === 0 ? 0 : context;
    const tail = j === lines.length ? 0 : context;
    if (j - i > head + tail + 1) {
      for (let k = i; k < i + head; k++) rows.push({ kind: "line", line: lines[k]! });
      rows.push({ kind: "fold", id: id++, lines: lines.slice(i + head, j - tail) });
      for (let k = j - tail; k < j; k++) rows.push({ kind: "line", line: lines[k]! });
    } else for (let k = i; k < j; k++) rows.push({ kind: "line", line: lines[k]! });
    i = j;
  }
  return rows;
}

/** Side by side: context on both sides, a deletion run beside the addition run that replaced it. */
export function toSplit(lines: Line[]): SplitRow[] {
  const out: SplitRow[] = [];
  for (let i = 0; i < lines.length; ) {
    const l = lines[i]!;
    if (l.op === "ctx") {
      out.push({ left: l, right: l });
      i++;
      continue;
    }
    const dels: Line[] = [];
    const adds: Line[] = [];
    while (i < lines.length && lines[i]!.op === "del") dels.push(lines[i++]!);
    while (i < lines.length && lines[i]!.op === "add") adds.push(lines[i++]!);
    for (let k = 0; k < Math.max(dels.length, adds.length); k++) out.push({ left: dels[k], right: adds[k] });
  }
  return out;
}

export const stats = (lines: Line[]) => ({
  added: lines.filter((l) => l.op === "add").length,
  deleted: lines.filter((l) => l.op === "del").length,
});
