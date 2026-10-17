import { useEffect, useState } from "react";
import type { Request, Response, Token } from "./worker";

export type { Token };

/** Above this, files show uncoloured: tokenizing would take longer than reading. */
export const HIGHLIGHT_LIMIT = 1_000_000;

let worker: Worker | null = null;
let next = 0;
const pending = new Map<number, (lines: Token[][] | null) => void>();
const cache = new Map<string, Token[][] | null>();

function highlight(path: string, text: string): Promise<Token[][] | null> {
  worker ??= (() => {
    const w = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    w.onmessage = (e: MessageEvent<Response>) => {
      pending.get(e.data.id)?.(e.data.lines);
      pending.delete(e.data.id);
    };
    return w;
  })();
  const id = ++next;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    worker!.postMessage({ id, path, text } satisfies Request);
  });
}

/** Highlighted lines for `text`, or null until ready (show plain text meanwhile). */
export function useHighlight(key: string | undefined, path: string, text: string | null): Token[][] | null {
  const cacheKey = key && `${key}:${path}`;
  const [lines, setLines] = useState<Token[][] | null>(() => (cacheKey ? (cache.get(cacheKey) ?? null) : null));
  useEffect(() => {
    if (!cacheKey || text === null || text.length > HIGHLIGHT_LIMIT) return setLines(null);
    if (cache.has(cacheKey)) return setLines(cache.get(cacheKey)!);
    let live = true;
    setLines(null);
    highlight(path, text).then((l) => {
      if (cache.size > 200) cache.delete(cache.keys().next().value!);
      cache.set(cacheKey, l);
      if (live) setLines(l);
    });
    return () => {
      live = false;
    };
  }, [cacheKey, path, text]);
  return lines;
}
