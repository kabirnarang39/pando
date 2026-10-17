// Fails the build when the JavaScript needed for the first screen grows past the budget.
// The floor is React DOM, the router and the gRPC-Web client (about 107 KB gzipped); the rest
// is the app shell. Lower BUDGET_KB when the bundle shrinks; never raise it without a reason.
import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
const BUDGET_KB = 225;
const html = readFileSync("dist/index.html", "utf8");
const files = [...new Set(html.match(/assets\/[^"]+\.js/g) ?? [])];
const kb = files.reduce((sum, f) => sum + gzipSync(readFileSync(`dist/${f}`)).length, 0) / 1024;
console.log(`first-screen JavaScript: ${kb.toFixed(0)} KB gzipped across ${files.length} files (budget ${BUDGET_KB} KB)`);
if (kb > BUDGET_KB) {
  console.error(`over budget by ${(kb - BUDGET_KB).toFixed(0)} KB`);
  process.exit(1);
}
