// Starts a real pandod with browser access, imports a small git repository into it, clones it
// and locks a file, so the tests exercise the console against the actual server.

import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { connect } from "node:net";
import { join, resolve } from "node:path";

const bin = resolve(process.env.PANDO_BIN ?? "../target/release");
export const PORT = 7611;
export const SERVER = `http://127.0.0.1:${PORT}`;

function git(cwd: string, ...args: string[]) {
  execFileSync("git", ["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "-c", "commit.gpgsign=false", ...args], { cwd, stdio: "pipe" });
}

/** Resolves once something accepts connections on the port (pandod is up). */
async function up(port: number) {
  for (let i = 0; i < 100; i++) {
    const ok = await new Promise<boolean>((done) => {
      const s = connect(port, "127.0.0.1", () => (s.end(), done(true)));
      s.on("error", () => done(false));
    });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`pandod did not start on port ${port}`);
}

export default async function setup() {
  const dir = mkdtempSync(join(tmpdir(), "pando-console-e2e-"));
  const src = join(dir, "src");
  mkdirSync(join(src, "services/billing"), { recursive: true });
  writeFileSync(join(src, "README.md"), "# Orchard\n\nA small repository for the console's end-to-end tests.\n");
  writeFileSync(join(src, "services/billing/tax.rs"), "pub const RATE_BPS: u32 = 1900;\n\npub fn tax(cents: u64) -> u64 {\n    cents * RATE_BPS as u64 / 10_000\n}\n");
  writeFileSync(join(src, "design.psd"), Buffer.from([0x38, 0x42, 0x50, 0x53, 0, 1, 2, 3]));
  git(src, "init", "-q", "-b", "main");
  git(src, "add", "-A");
  git(src, "commit", "-qm", "Start the orchard");
  writeFileSync(join(src, "services/billing/tax.rs"), "pub const RATE_BPS: u32 = 2000;\n\npub fn tax(cents: u64) -> u64 {\n    cents * RATE_BPS as u64 / 10_000\n}\n");
  git(src, "commit", "-qam", "Raise the tax rate to 20%");

  const server = spawn(join(bin, "pandod"), ["server", "--listen", `127.0.0.1:${PORT}`, "--metadata", `sqlite://${join(dir, "meta.db")}`, "--web-origins", "http://127.0.0.1:4173"], {
    stdio: ["ignore", "ignore", "inherit"],
  });
  try {
    await up(PORT);
    const run = (args: string[], cwd?: string) => execFileSync(join(bin, "pando"), args, { cwd, stdio: ["ignore", "pipe", "pipe"], timeout: 30_000 });
    run(["import-git", src, "--server", SERVER]);
    const clone = join(dir, "clone");
    run(["clone", SERVER, clone]);
    run(["lock", "design.psd"], clone);
  } catch (e) {
    server.kill();
    throw e;
  }

  return () => {
    server.kill();
  };
}
