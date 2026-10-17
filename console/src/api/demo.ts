// An in-browser Pando server for the demo: a repository with a history, refs and locks, and a
// fleet of simulated agents that keep landing small, real edits. It answers the same RPCs as
// pandod, so every screen of the console runs against it unchanged.

import { create, toBinary } from "@bufbuild/protobuf";
import { Code, ConnectError, createRouterTransport, type Transport } from "@connectrpc/connect";
import { diffArrays } from "diff";
import { ChangesetSchema, FileChangeSchema, Mode } from "../gen/pando_objects_pb";
import { Access, Browse, Changesets, Diff, History, Locks, Refs } from "../gen/pando_pb";
import { binaryFiles, people, seedFiles } from "./demo-repo";
import { hex } from "./client";

interface FileState {
  bytes: Uint8Array;
  mode: Mode;
  blob: Uint8Array;
  /** Changeset id that last changed each line (text files only). */
  blame: Uint8Array[];
}
type Snapshot = Map<string, FileState>;

interface Stored {
  id: Uint8Array;
  encoded: Uint8Array;
  parent: string | null;
  generation: bigint;
  changes: { path: string; blob: Uint8Array; mode: Mode }[];
  snapshot: Snapshot;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

/** A deterministic 32-byte digest (four FNV-1a lanes). Ids only need to be distinct here. */
function digest(bytes: Uint8Array): Uint8Array {
  const out = new Uint8Array(32);
  const view = new DataView(out.buffer);
  for (let lane = 0; lane < 8; lane++) {
    let h = (0x811c9dc5 ^ (lane * 0x9e3779b1)) >>> 0;
    for (let i = 0; i < bytes.length; i++) h = Math.imul(h ^ bytes[i]!, 0x01000193) >>> 0;
    h = Math.imul(h ^ bytes.length ^ (h >>> 15), 0x2c1b3c6d) >>> 0;
    view.setUint32(lane * 4, h);
  }
  return out;
}

function rng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return (s >>> 0) / 4294967296;
  };
}

const lines = (b: Uint8Array) => dec.decode(b).split("\n");
const isText = (path: string) => !(path in binaryFiles);
const overlaps = (path: string, filter: string) =>
  !filter || path === filter || path.startsWith(filter + "/") || filter.startsWith(path + "/");

/** One edit an agent makes: the new file text and the message it lands with. */
type Edit = { path: string; text: string | null; message: string };

export class DemoRepo {
  private changesets = new Map<string, Stored>();
  private refs = new Map<string, string>();
  private locks = new Map<string, { owner: string; at: number }>();
  private random = rng(20261017);
  private listeners = new Set<() => void>();
  private counter = 0;
  private home: Record<string, number> = {};

  constructor(now = Date.now()) {
    const snapshot: Snapshot = new Map();
    const seedTime = now - 6 * 3600_000;
    const files: { path: string; bytes: Uint8Array }[] = [
      ...Object.entries(seedFiles).map(([path, text]) => ({ path, bytes: enc.encode(text) })),
      ...Object.entries(binaryFiles).map(([path, size]) => ({ path, bytes: this.binary(size) })),
    ];
    const root = this.commit(null, snapshot, files.map((f) => ({ path: f.path, bytes: f.bytes })), "kabir", "Import the Fleet monorepo", seedTime);
    this.refs.set("main", root);
    // A few hours of history, landing faster as the fleet scales up.
    let t = seedTime;
    while (t < now - 60_000) {
      t += 4_000 + this.random() * 70_000 * (1 - (t - seedTime) / (now - seedTime));
      this.landRandom(Math.min(t, now - 60_000));
    }
    this.refs.set("release/2.4", this.ancestor(this.refs.get("main")!, 40));
    this.refs.set("agent/perf-05/batching", this.ancestor(this.refs.get("main")!, 3));
    this.locks.set("design/checkout.fig", { owner: "kabir", at: now - 47 * 60_000 });
    this.locks.set("assets/brand/hero.psd", { owner: "agent/docs-01", at: now - 6 * 60_000 });
  }

  /** Lands one change every couple of seconds until stopped. */
  start(): () => void {
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => {
      this.landRandom(Date.now());
      this.listeners.forEach((l) => l());
      timer = setTimeout(tick, 900 + this.random() * 2600);
    };
    timer = setTimeout(tick, 1500);
    return () => clearTimeout(timer);
  }

  onLand(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private binary(size: number): Uint8Array {
    const b = new Uint8Array(size);
    b.set([0x38, 0x42, 0x50, 0x53]);
    for (let i = 4; i < size; i++) b[i] = (this.random() * 256) | 0;
    return b;
  }

  private ancestor(id: string, n: number): string {
    let cur = id;
    for (let i = 0; i < n; i++) cur = this.changesets.get(cur)?.parent ?? cur;
    return cur;
  }

  private commit(
    parent: string | null,
    base: Snapshot,
    edits: { path: string; bytes: Uint8Array | null }[],
    author: string,
    message: string,
    time: number,
  ): string {
    const changes = edits.map((e) => {
      const mode = e.path.endsWith(".sh") ? Mode.EXEC : Mode.FILE;
      return { path: e.path, blob: e.bytes ? digest(e.bytes) : new Uint8Array(), mode };
    });
    const encoded = toBinary(
      ChangesetSchema,
      create(ChangesetSchema, {
        parents: parent ? [this.changesets.get(parent)!.id] : [],
        changes: changes.map((c) => create(FileChangeSchema, c)),
        author,
        message,
        timestampMs: BigInt(Math.round(time)),
      }),
    );
    const id = digest(encoded);
    const snapshot: Snapshot = new Map(base);
    for (const e of edits) {
      if (!e.bytes) {
        snapshot.delete(e.path);
        continue;
      }
      const old = base.get(e.path);
      let blame: Uint8Array[] = [];
      if (isText(e.path)) {
        const next = lines(e.bytes);
        blame = old ? this.remapBlame(old, next, id) : next.map(() => id);
      }
      snapshot.set(e.path, { bytes: e.bytes, mode: Mode.FILE, blob: digest(e.bytes), blame });
    }
    const key = hex(id);
    const generation = parent ? this.changesets.get(parent)!.generation + 1n : 1n;
    this.changesets.set(key, { id, encoded, parent, generation, changes, snapshot });
    return key;
  }

  private remapBlame(old: FileState, next: string[], id: Uint8Array): Uint8Array[] {
    const out: Uint8Array[] = [];
    let oldLine = 0;
    for (const part of diffArrays(lines(old.bytes), next)) {
      if (part.removed) oldLine += part.count!;
      else if (part.added) for (let i = 0; i < part.count!; i++) out.push(id);
      else for (let i = 0; i < part.count!; i++) out.push(old.blame[oldLine++] ?? id);
    }
    return out;
  }

  private landRandom(time: number) {
    const tip = this.refs.get("main")!;
    const snap = this.changesets.get(tip)!.snapshot;
    const author = people[Math.floor(this.random() * people.length)]!;
    const edit = this.edit(snap, author);
    if (!edit) return;
    const id = this.commit(tip, snap, [{ path: edit.path, bytes: edit.text === null ? null : enc.encode(edit.text) }], author, edit.message, time);
    this.refs.set("main", id);
  }

  /** A small, plausible change in the style of the agent making it. */
  private edit(snap: Snapshot, author: string): Edit | null {
    const pick = <T,>(xs: T[]) => xs[Math.floor(this.random() * xs.length)]!;
    const textPaths = [...snap.keys()].filter(isText);
    const n = ++this.counter;
    const kind = author.includes("docs")
      ? "doc"
      : author.includes("tests")
        ? "test"
        : author.includes("perf") || author.includes("deps")
          ? "tune"
          : pick(["tune", "doc", "test", "tune", "adr"]);

    if (kind === "adr" && this.random() < 0.3) {
      const slug = pick(["retry-budget", "shard-count", "free-shipping", "refund-window", "cache-ttl"]);
      const path = `docs/decisions/${String(n).padStart(4, "0")}-${slug}.md`;
      return {
        path,
        text: `# ${n}. ${slug.replace(/-/g, " ")}\n\nStatus: accepted\n\nWe keep this value in one constant and change it with a measured reason.\n`,
        message: `Record decision on ${slug.replace(/-/g, " ")}`,
      };
    }
    if (kind === "tune") {
      // Only named settings, and never further than 3x from where they started.
      const tunable = /(const [A-Z_]+(?:: \w+)? = |^\s*[a-z_]+\s+=\s+|timeout_minutes: |batchSize: )(\d{1,5})\b/;
      const candidates = textPaths.filter((p) => !p.endsWith(".md") && lines(snap.get(p)!.bytes).some((l) => tunable.test(l)));
      if (!candidates.length) return null;
      const path = pick(candidates);
      const ls = lines(snap.get(path)!.bytes);
      const i = pick(ls.map((l, i) => (tunable.test(l) ? i : -1)).filter((i) => i >= 0));
      const line = ls[i]!;
      const m = line.match(tunable)!;
      const name = line.match(/([A-Za-z_]+)\s*(?::\s*\w+\s*)?[:=]/)?.[1] ?? "value";
      const key = `${path}:${name}`;
      const old = Number(m[2]);
      const home = (this.home[key] ??= old);
      const round = (v: number) => (v < 10 ? Math.round(v) : v < 100 ? Math.max(5, Math.round(v / 5) * 5) : Number(v.toPrecision(2)));
      const next = round(Math.min(home * 3, Math.max(Math.ceil(home / 3), old * pick([0.5, 0.8, 1.25, 1.5, 2]))));
      if (next === old) return null;
      ls[i] = line.replace(tunable, `$1${next}`);
      const why = pick(["Measured under the nightly load test.", "p99 latency stayed flat in staging.", "Matches what production has needed for a week.", "Cuts cost without missing the SLO."]);
      return { path, text: ls.join("\n"), message: `${next > old ? "Raise" : "Lower"} ${name} to ${next} (was ${old})\n\n${why}` };
    }
    if (kind === "test") {
      // At most three generated tests per file: a fleet that only ever adds tests is not believable.
      const open = textPaths.filter((p) => (p.endsWith(".rs") || p.endsWith(".go")) && (dec.decode(snap.get(p)!.bytes).match(/[Hh]andles/g)?.length ?? 0) < 3);
      if (!open.length) return null;
      const path = pick(open);
      const text = dec.decode(snap.get(path)!.bytes);
      const thing = pick(["empty input", "zero quantity", "maximum price", "unicode titles", "duplicate ids", "retry exhaustion"]);
      const fn = thing.replace(/ /g, "_");
      const added = path.endsWith(".rs")
        ? `\n#[cfg(test)]\nmod ${fn}_${n} {\n    #[test]\n    fn handles_${fn}() {\n        assert!(true, "${thing} is accepted");\n    }\n}\n`
        : `\nfunc TestHandles${fn.replace(/(^|_)(\w)/g, (_, __, c: string) => c.toUpperCase())}${n}(t *testing.T) {\n\tt.Log("${thing}")\n}\n`;
      return { path, text: text.trimEnd() + "\n" + added, message: `Add test for ${thing} in ${path.split("/").pop()}` };
    }
    // Documentation: reword or add a line to a Markdown file or a doc comment.
    const path = pick(textPaths.filter((p) => p.endsWith(".md")));
    const ls = lines(snap.get(path)!.bytes);
    const fresh = [
      "Link the dashboard from the first step.",
      "Mention the staging environment explicitly.",
      "Say who to page outside office hours.",
      "Explain why the retry limit is what it is.",
      "Add the command to check shard health.",
      "Note that refunds over 30 days need approval.",
      "Point to the load test that set these numbers.",
      "List the regions this applies to.",
    ].filter((n) => !ls.includes(n));
    if (!fresh.length) return null;
    const note = pick(fresh);
    const at = Math.max(1, Math.floor(this.random() * ls.length));
    ls.splice(at, 0, `${note}`);
    return { path, text: ls.join("\n"), message: `Docs: ${note.charAt(0).toLowerCase()}${note.slice(1, -1)}` };
  }

  // ---- reads, as the server answers them ----

  resolve(rev: string): Stored {
    const id = this.refs.get(rev) ?? rev;
    const cs = this.changesets.get(id) ?? [...this.changesets.entries()].find(([k]) => rev.length >= 6 && k.startsWith(rev))?.[1];
    if (!cs) throw new ConnectError(`unknown revision \`${rev}\``, Code.NotFound);
    return cs;
  }

  refList() {
    return [...this.refs.entries()].map(([name, id]) => ({ name, id: this.changesets.get(id)!.id }));
  }

  ref(name: string) {
    const id = this.refs.get(name);
    return id ? { exists: true, id: this.changesets.get(id)!.id } : { exists: false, id: new Uint8Array() };
  }

  info(cs: Stored) {
    return { id: cs.id, rootTree: digest(cs.encoded), generation: cs.generation, encoded: cs.encoded, redacted: false };
  }

  *log(rev: string, path: string, limit: number) {
    let cur: Stored | undefined = this.resolve(rev);
    let out = 0;
    const max = limit || 1000;
    while (cur && out < max) {
      if (cur.changes.some((c) => overlaps(c.path, path))) {
        out++;
        yield this.info(cur);
      }
      cur = cur.parent ? this.changesets.get(cur.parent) : undefined;
    }
  }

  file(rev: string, path: string): FileState {
    const f = this.resolve(rev).snapshot.get(path);
    if (!f) throw new ConnectError(`no file \`${path}\` at ${rev}`, Code.NotFound);
    return f;
  }

  listDir(rev: string, path: string) {
    const snap = this.resolve(rev).snapshot;
    const prefix = path ? path.replace(/\/$/, "") + "/" : "";
    const entries = new Map<string, { name: string; mode: Mode; id: Uint8Array }>();
    for (const [p, f] of snap) {
      if (!p.startsWith(prefix)) continue;
      const rest = p.slice(prefix.length);
      const slash = rest.indexOf("/");
      if (slash < 0) entries.set(rest, { name: rest, mode: f.mode, id: f.blob });
      else {
        const name = rest.slice(0, slash);
        if (!entries.has(name)) entries.set(name, { name, mode: Mode.DIR, id: digest(enc.encode(prefix + name)) });
      }
    }
    if (path && entries.size === 0) throw new ConnectError(`no directory \`${path}\``, Code.NotFound);
    return [...entries.values()].sort((a, b) => (a.name < b.name ? -1 : 1));
  }

  find(rev: string, pattern: string, limit: number) {
    const all = [...this.resolve(rev).snapshot.keys()].filter((p) => p.includes(pattern)).sort();
    const max = limit || 100;
    return { paths: all.slice(0, max), truncated: all.length > max };
  }

  diff(from: string, to: string) {
    const a = this.resolve(from).snapshot;
    const b = this.resolve(to).snapshot;
    const out: { path: string; blob: Uint8Array; mode: Mode }[] = [];
    for (const [p, f] of b) if (hex(a.get(p)?.blob ?? new Uint8Array()) !== hex(f.blob)) out.push({ path: p, blob: f.blob, mode: f.mode });
    for (const p of a.keys()) if (!b.has(p)) out.push({ path: p, blob: new Uint8Array(), mode: Mode.FILE });
    return out.sort((x, y) => (x.path < y.path ? -1 : 1));
  }

  blame(rev: string, path: string) {
    const f = this.file(rev, path);
    if (!isText(path)) throw new ConnectError(`\`${path}\` is not a text file`, Code.FailedPrecondition);
    return lines(f.bytes).map((text, i) => ({ changeset: f.blame[i]!, text }));
  }

  lockList(prefix: string) {
    return [...this.locks.entries()]
      .filter(([p]) => p.startsWith(prefix))
      .map(([path, l]) => ({ path, owner: l.owner, lockedAtMs: BigInt(l.at) }));
  }
}

/** A transport answered by `repo`, with a little latency so loading states are real. */
export function demoTransport(repo: DemoRepo): Transport {
  const wait = () => new Promise((r) => setTimeout(r, 15 + Math.random() * 45));
  return createRouterTransport(({ service }) => {
    service(Refs, {
      async get(r) {
        await wait();
        return repo.ref(r.name);
      },
      async list() {
        await wait();
        return { refs: repo.refList() };
      },
      compareAndSwap() {
        throw new ConnectError("the demo is read-only", Code.PermissionDenied);
      },
    });
    service(Changesets, {
      async get(r) {
        await wait();
        return repo.info(repo.resolve(r.rev));
      },
      create() {
        throw new ConnectError("the demo is read-only", Code.PermissionDenied);
      },
    });
    service(History, {
      async *log(r) {
        await wait();
        yield* repo.log(r.rev, r.path, r.limit);
      },
      async blame(r) {
        await wait();
        return { lines: repo.blame(r.rev, r.path) };
      },
    });
    service(Diff, {
      async diff(r) {
        await wait();
        return { changes: repo.diff(r.from, r.to) };
      },
    });
    service(Browse, {
      async listDir(r) {
        await wait();
        return { entries: repo.listDir(r.rev, r.path) };
      },
      async readFile(r) {
        await wait();
        const f = repo.file(r.rev, r.path);
        return { content: f.bytes, mode: f.mode, id: f.blob };
      },
      async find(r) {
        await wait();
        return repo.find(r.rev, r.pattern, r.limit);
      },
      // eslint-disable-next-line require-yield
      async *checkout() {
        throw new ConnectError("use `pando clone` to check out", Code.Unimplemented);
      },
    });
    service(Locks, {
      async list(r) {
        await wait();
        return { locks: repo.lockList(r.prefix) };
      },
      lock() {
        throw new ConnectError("the demo is read-only", Code.PermissionDenied);
      },
      unlock() {
        throw new ConnectError("the demo is read-only", Code.PermissionDenied);
      },
    });
    service(Access, {
      async whoAmI() {
        await wait();
        return { principal: "demo", fullRead: true, readPrefixes: [] };
      },
    });
  });
}
