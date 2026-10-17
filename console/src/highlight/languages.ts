import type { LanguageRegistration } from "shiki/core";

type Loader = () => Promise<LanguageRegistration[]>;
const lang = (m: Promise<{ default: LanguageRegistration[] }>) => m.then((x) => x.default);

// Each grammar is its own chunk, fetched the first time a file needs it.
const byId: Record<string, Loader> = {
  c: () => lang(import("shiki/langs/c.mjs")),
  cpp: () => lang(import("shiki/langs/cpp.mjs")),
  css: () => lang(import("shiki/langs/css.mjs")),
  dockerfile: () => lang(import("shiki/langs/dockerfile.mjs")),
  go: () => lang(import("shiki/langs/go.mjs")),
  html: () => lang(import("shiki/langs/html.mjs")),
  java: () => lang(import("shiki/langs/java.mjs")),
  javascript: () => lang(import("shiki/langs/javascript.mjs")),
  json: () => lang(import("shiki/langs/json.mjs")),
  jsx: () => lang(import("shiki/langs/jsx.mjs")),
  kotlin: () => lang(import("shiki/langs/kotlin.mjs")),
  markdown: () => lang(import("shiki/langs/markdown.mjs")),
  proto: () => lang(import("shiki/langs/proto.mjs")),
  python: () => lang(import("shiki/langs/python.mjs")),
  ruby: () => lang(import("shiki/langs/ruby.mjs")),
  rust: () => lang(import("shiki/langs/rust.mjs")),
  shellscript: () => lang(import("shiki/langs/shellscript.mjs")),
  sql: () => lang(import("shiki/langs/sql.mjs")),
  swift: () => lang(import("shiki/langs/swift.mjs")),
  terraform: () => lang(import("shiki/langs/terraform.mjs")),
  toml: () => lang(import("shiki/langs/toml.mjs")),
  tsx: () => lang(import("shiki/langs/tsx.mjs")),
  typescript: () => lang(import("shiki/langs/typescript.mjs")),
  yaml: () => lang(import("shiki/langs/yaml.mjs")),
};

const byExt: Record<string, string> = {
  c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", css: "css", go: "go", html: "html", java: "java",
  js: "javascript", mjs: "javascript", cjs: "javascript", json: "json", jsx: "jsx", kt: "kotlin",
  md: "markdown", proto: "proto", py: "python", rb: "ruby", rs: "rust", sh: "shellscript",
  bash: "shellscript", zsh: "shellscript", sql: "sql", swift: "swift", tf: "terraform",
  toml: "toml", tsx: "tsx", ts: "typescript", mts: "typescript", yaml: "yaml", yml: "yaml",
};
const byName: Record<string, string> = { Dockerfile: "dockerfile", "Cargo.lock": "toml", Makefile: "shellscript" };

export function languageFor(path: string): { id: string; load: Loader } | null {
  const name = path.split("/").pop() ?? "";
  const id = byName[name] ?? byExt[name.split(".").pop()?.toLowerCase() ?? ""];
  return id && byId[id] ? { id, load: byId[id] } : null;
}
