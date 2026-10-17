import { describe, expect, it } from "vitest";
import { fold, lineDiff, stats, toSplit } from "./diff";

describe("lineDiff", () => {
  it("numbers lines on both sides and counts changes", () => {
    const lines = lineDiff("a\nb\nc\n", "a\nB\nc\nd\n");
    expect(lines.map((l) => `${l.op}:${l.old ?? ""}:${l.new ?? ""}:${l.text}`)).toEqual([
      "ctx:1:1:a",
      "del:2::b",
      "add::2:B",
      "ctx:3:3:c",
      "add::4:d",
    ]);
    expect(stats(lines)).toEqual({ added: 2, deleted: 1 });
  });

  it("marks only the changed words of a rewritten line", () => {
    const [, del, add] = lineDiff("x\nconst LIMIT = 30;\n", "x\nconst LIMIT = 60;\n");
    expect(del!.pieces!.filter((p) => p.changed).map((p) => p.text)).toEqual(["30"]);
    expect(add!.pieces!.filter((p) => p.changed).map((p) => p.text)).toEqual(["60"]);
  });

  it("does not word-diff lines that are mostly different", () => {
    const [del] = lineDiff("alpha beta gamma\n", "one two three\n");
    expect(del!.pieces).toBeUndefined();
  });

  it("treats a missing final newline like a present one", () => {
    expect(lineDiff("a\nb", "a\nb\n").every((l) => l.op === "ctx")).toBe(true);
  });
});

describe("fold", () => {
  it("keeps three lines of context around a change and folds the rest", () => {
    const a = Array.from({ length: 30 }, (_, i) => `line ${i}`).join("\n");
    const b = a.replace("line 15", "line fifteen");
    const rows = fold(lineDiff(a, b));
    const folds = rows.filter((r) => r.kind === "fold");
    expect(folds.map((f) => (f.kind === "fold" ? f.lines.length : 0))).toEqual([12, 11]);
    expect(rows.filter((r) => r.kind === "line")).toHaveLength(3 + 2 + 3);
  });

  it("keeps commented lines visible", () => {
    const a = Array.from({ length: 30 }, (_, i) => `line ${i}`).join("\n");
    const b = a.replace("line 25", "line twenty-five");
    const rows = fold(lineDiff(a, b), 3, new Set([5]));
    expect(rows.some((r) => r.kind === "line" && r.line.new === 5)).toBe(true);
  });

  it("never folds a single line", () => {
    const rows = fold(lineDiff("a\nb\nc\nd\ne\nf\ng\nh\n", "a\nb\nc\nd\nE\nf\ng\nh\n"));
    expect(rows.every((r) => r.kind === "line")).toBe(true);
  });
});

describe("toSplit", () => {
  it("puts a replacement beside what it replaced", () => {
    const rows = toSplit(lineDiff("a\nb\nc\n", "a\nB1\nB2\nc\n"));
    expect(rows.map((r) => `${r.left?.text ?? "-"}|${r.right?.text ?? "-"}`)).toEqual(["a|a", "b|B1", "-|B2", "c|c"]);
  });
});
