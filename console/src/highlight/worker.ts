// Syntax highlighting off the main thread. Colours are CSS variables, so the console's own
// light and dark tokens theme the code without re-highlighting.

import { createCssVariablesTheme, createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { languageFor } from "./languages";

export type Token = [text: string, color: string | undefined];
export interface Request { id: number; path: string; text: string }
export interface Response { id: number; lines: Token[][] | null }

const theme = createCssVariablesTheme({ name: "pando", variablePrefix: "--code-", fontStyle: true });
let highlighter: Promise<HighlighterCore> | null = null;
const loaded = new Set<string>();

async function tokens(path: string, text: string): Promise<Token[][] | null> {
  const lang = languageFor(path);
  if (!lang) return null;
  highlighter ??= createHighlighterCore({ themes: [theme], langs: [], engine: createJavaScriptRegexEngine() });
  const h = await highlighter;
  if (!loaded.has(lang.id)) {
    await h.loadLanguage(...(await lang.load()));
    loaded.add(lang.id);
  }
  const { tokens } = h.codeToTokens(text, { lang: lang.id, theme: "pando" });
  return tokens.map((line) => line.map((t) => [t.content, t.color] as Token));
}

self.onmessage = async (e: MessageEvent<Request>) => {
  const { id, path, text } = e.data;
  let lines: Token[][] | null = null;
  try {
    lines = await tokens(path, text);
  } catch {
    // An unsupported construct in a grammar: show the file uncoloured rather than not at all.
  }
  self.postMessage({ id, lines } satisfies Response);
};
