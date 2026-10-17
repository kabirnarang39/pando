// The demo as a whole platform: one organisation with four repositories, people and agents,
// change requests with reviews, checks and comments, protected branches and an inbox. It answers
// the same RPCs as pandod, with repository-scoped calls routed by the `x-pando-repo` header.

import { create } from "@bufbuild/protobuf";
import { Code, ConnectError, createRouterTransport, type HandlerContext, type Interceptor, type Transport } from "@connectrpc/connect";
import { lineDiff, stats } from "../diff";
import { Access, Browse, Changesets, Diff, History, Locks, Refs } from "../gen/pando_pb";
import {
  AccountKind,
  ChangeRequestState,
  CheckState,
  EventKind,
  Platform,
  Reviews,
  Role,
  Verdict,
  Visibility,
  ChangeRequestSchema,
  CommentSchema,
  EventSchema,
  ProtectionSchema,
  ReviewSummarySchema,
  type ChangeRequest,
  type Comment,
  type Event,
  type Protection,
} from "../gen/pando_platform_pb";
import { DemoRepo, repoServices } from "./demo";
import { fleetSeed, infraSeed, mlSeed, webSeed, type RepoSeed } from "./demo-repo";
import { hex } from "./client";

export const ORG = "lumen";
export const VIEWER = "kabir";
export const REPO_HEADER = "x-pando-repo";
const dec = new TextDecoder();

type CR = ChangeRequest;

interface Hosted {
  seed: RepoSeed;
  repo: DemoRepo;
  created: number;
  visibility: Visibility;
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

const people: Record<string, string> = {
  kabir: "Kabir Narang",
  mira: "Mira Okafor",
  dev: "Dev Raman",
  sana: "Sana Iqbal",
};
const agentOwners: Record<string, string> = {
  "agent/refactor-07": "kabir",
  "agent/tests-03": "mira",
  "agent/docs-01": "kabir",
  "agent/deps-02": "dev",
  "agent/perf-05": "kabir",
  "agent/migrate-04": "dev",
  "agent/lint-09": "mira",
  "agent/security-06": "dev",
  "agent/ui-11": "mira",
  "agent/a11y-02": "mira",
  "agent/infra-08": "dev",
  "agent/cost-04": "dev",
  "agent/train-01": "sana",
  "agent/eval-02": "sana",
};

const reviewNotes = [
  "This changes a value other services read at startup; can we roll it out behind the flag first?",
  "Looks right. The load test numbers in the description match what I see on the dashboard.",
  "Please add a test that fails without this change.",
  "Nit: the comment above still mentions the old value.",
  "Is staging already on this setting? If so, approve from me.",
  "Can this wait until after the release cut tomorrow?",
];
const agentReplies = [
  "Added a test that fails on the old value; the new tip includes it.",
  "Staging has run with this for 26 hours with no alerts.",
  "Updated the comment in the same change.",
];

export class DemoPlatform {
  private hosted = new Map<string, Hosted>();
  private crs = new Map<string, CR[]>();
  private comments = new Map<string, Comment[]>();
  private events: Event[] = [];
  private protections = new Map<string, Protection[]>();
  private random = rng(77);
  private nextComment = 1;
  private listeners = new Set<() => void>();

  /** Builds the demo one repository per task, so the page stays responsive while it does. */
  static async create(now = Date.now()): Promise<DemoPlatform> {
    const p = new DemoPlatform(now, true);
    for (const [i, seed] of DemoPlatform.seeds.entries()) {
      await new Promise((r) => setTimeout(r, 0));
      p.host(seed, DemoPlatform.ages[i]!, now);
    }
    await new Promise((r) => setTimeout(r, 0));
    p.seedChangeRequests(now);
    p.advanceChecks(now);
    return p;
  }

  private static seeds = [fleetSeed, webSeed, infraSeed, mlSeed];
  private static ages = [140, 96, 300, 61];

  constructor(now = Date.now(), empty = false) {
    if (empty) return;
    DemoPlatform.seeds.forEach((seed, i) => this.host(seed, DemoPlatform.ages[i]!, now));
    this.seedChangeRequests(now);
    this.advanceChecks(now);
  }

  private host(seed: RepoSeed, age: number, now: number) {
    const i = DemoPlatform.seeds.indexOf(seed);
    {
      const key = `${ORG}/${seed.name}`;
      this.hosted.set(key, { seed, repo: new DemoRepo(seed, now), created: now - age * 86_400_000, visibility: i === 2 ? Visibility.PRIVATE : Visibility.INTERNAL });
      this.crs.set(key, []);
      this.protections.set(key, [
        create(ProtectionSchema, {
          repository: { owner: ORG, name: seed.name },
          pattern: "main",
          requiredApprovals: seed.name === "infra" ? 2 : 1,
          requiredChecks: ["build", "test"],
          dismissStale: true,
          agentsLandDirectly: seed.name !== "infra",
          bypass: ["kabir"],
        }),
      ]);
      this.events.push(create(EventSchema, { id: `e${this.events.length}`, kind: EventKind.REPOSITORY_CREATED, actor: seed.importer, repository: { owner: ORG, name: seed.name }, atMs: BigInt(now - age * 86_400_000), summary: `created ${ORG}/${seed.name}` }));
    }
  }

  start(): () => void {
    const stops = [...this.hosted.values()].map((h) => h.repo.start());
    // Agents keep proposing, checks keep finishing, reviewers keep reviewing.
    const timer = setInterval(() => this.tick(), 4000);
    return () => {
      stops.forEach((s) => s());
      clearInterval(timer);
    };
  }

  onChange(l: () => void) {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  repoFor(header: string | null): DemoRepo {
    const key = header || `${ORG}/fleet`;
    const h = this.hosted.get(key);
    if (!h) throw new ConnectError(`no repository \`${key}\``, Code.NotFound);
    return h.repo;
  }

  // ---- seeding ----

  private seedChangeRequests(now: number) {
    const plan: [string, number][] = [
      ["fleet", 16],
      ["storefront", 6],
      ["infra", 4],
      ["ranking", 3],
    ];
    for (const [name, count] of plan) {
      for (let i = 0; i < count; i++) {
        const minutesAgo = (count - i) * (8 + this.random() * 40);
        this.open(`${ORG}/${name}`, now - minutesAgo * 60_000, i < count - 3);
      }
    }
  }

  /** An agent proposes a change; on a protected branch it becomes a change request. */
  private open(key: string, at: number, settle: boolean) {
    const h = this.hosted.get(key)!;
    const list = this.crs.get(key)!;
    const number = BigInt(list.length + 1 + (h.seed.name === "fleet" ? 3100 : 200));
    const agents = h.seed.people.filter((p) => p.startsWith("agent/"));
    const author = agents[Math.floor(this.random() * agents.length)]!;
    const source = `${author}/cr-${number}`;
    const proposal = h.repo.propose(author, source, at);
    if (!proposal) return;
    const [title, ...rest] = proposal.message.split("\n");
    let additions = 0;
    let deletions = 0;
    for (const p of proposal.paths) {
      const before = this.text(h.repo, proposal.base, p);
      const after = this.text(h.repo, proposal.tip, p);
      const s = stats(lineDiff(before, after));
      additions += s.added;
      deletions += s.deleted;
    }
    const owner = agentOwners[author] ?? VIEWER;
    const cr: CR = create(ChangeRequestSchema, {
      repository: { owner: ORG, name: h.seed.name },
      number,
      title: title ?? "",
      body: `${rest.join("\n").trim() || "Small change proposed by an agent."}\n\nOpened automatically: \`${author}\` landed on protected \`main\`.`,
      author,
      state: ChangeRequestState.OPEN,
      source,
      target: "main",
      sourceTip: this.bytes(proposal.tip),
      base: this.bytes(proposal.base),
      createdAtMs: BigInt(Math.round(at)),
      updatedAtMs: BigInt(Math.round(at)),
      requestedReviewers: [owner === author ? VIEWER : owner],
      reviews: [],
      checks: [
        { name: "build", state: CheckState.PENDING, summary: "", detailsUrl: "", startedAtMs: BigInt(Math.round(at)), finishedAtMs: 0n },
        { name: "test", state: CheckState.PENDING, summary: "", detailsUrl: "", startedAtMs: BigInt(Math.round(at)), finishedAtMs: 0n },
        { name: "lint", state: CheckState.PENDING, summary: "", detailsUrl: "", startedAtMs: BigInt(Math.round(at)), finishedAtMs: 0n },
      ],
      comments: 0,
      files: proposal.paths.length,
      additions,
      deletions,
      conflicts: [],
      blockers: [],
      landedAs: new Uint8Array(),
      labels: [],
      fromProtectedLand: true,
    });
    list.push(cr);
    this.event(EventKind.CHANGE_REQUEST_OPENED, author, cr, `opened #${number} ${cr.title}`, at);
    if (settle) this.settle(key, cr, at);
    this.refresh(key, cr);
  }

  /** Move an older change request along: checks finish, reviews and comments arrive, some land. */
  private settle(key: string, cr: CR, at: number) {
    const roll = this.random();
    const after = (m: number) => BigInt(Math.round(at + m * 60_000));
    for (const c of cr.checks) {
      const failed = roll < 0.12 && c.name === "test";
      c.state = failed ? CheckState.FAILED : CheckState.PASSED;
      c.summary = failed ? "1 test failed: handles_retry_exhaustion" : "";
      c.finishedAtMs = after(3 + this.random() * 4);
    }
    const reviewer = cr.requestedReviewers[0] ?? VIEWER;
    if (roll < 0.3) {
      this.comment(key, cr, reviewer, reviewNotes[Math.floor(this.random() * reviewNotes.length)]!, at + 9 * 60_000, true);
      cr.reviews.push(create(ReviewSummarySchema, { reviewer, verdict: Verdict.CHANGES_REQUESTED, atMs: after(9), reviewedTip: cr.sourceTip }));
    } else if (roll < 0.7) {
      cr.reviews.push(create(ReviewSummarySchema, { reviewer, verdict: Verdict.APPROVED, atMs: after(12), reviewedTip: cr.sourceTip }));
      if (roll < 0.55) {
        const h = this.hosted.get(key)!;
        const landed = h.repo.landBranch(cr.source, cr.author, cr.title);
        cr.state = ChangeRequestState.LANDED;
        cr.landedAs = this.bytes(landed);
        cr.updatedAtMs = after(14);
        this.event(EventKind.CHANGE_REQUEST_LANDED, reviewer, cr, `landed #${cr.number}`, at + 14 * 60_000);
      }
    } else if (roll < 0.76) {
      cr.state = ChangeRequestState.CLOSED;
      this.comment(key, cr, reviewer, "Superseded by a later change to the same setting.", at + 20 * 60_000, false);
    }
  }

  /** Checks that have run long enough finish, mostly green. */
  private advanceChecks(now: number) {
    for (const [key, list] of this.crs) {
      for (const cr of list) {
        if (cr.state !== ChangeRequestState.OPEN) continue;
        for (const c of cr.checks) {
          if (c.state === CheckState.PENDING && now - Number(c.startedAtMs) > 6000 + this.random() * 9000) {
            c.state = this.random() < 0.08 && c.name === "test" ? CheckState.FAILED : CheckState.PASSED;
            c.finishedAtMs = BigInt(Math.min(now, Number(c.startedAtMs) + 180_000));
            if (c.state === CheckState.FAILED) c.summary = "1 test failed: handles_zero_quantity";
          }
        }
        this.refresh(key, cr);
      }
    }
  }

  private tick() {
    const now = Date.now();
    this.advanceChecks(now);
    // Now and then an agent lands on a protected branch: a new change request appears.
    if (this.random() < 0.35) this.open(`${ORG}/fleet`, now, false);
    this.listeners.forEach((l) => l());
  }

  // ---- helpers ----

  private text(repo: DemoRepo, rev: string, path: string): string {
    try {
      return dec.decode(repo.file(rev, path).bytes);
    } catch {
      return "";
    }
  }

  private bytes(hexId: string) {
    return Uint8Array.from(hexId.match(/../g)!.map((b) => parseInt(b, 16)));
  }

  private comment(key: string, cr: CR, author: string, body: string, at: number, onLine: boolean) {
    const ck = `${key}#${cr.number}`;
    const list = this.comments.get(ck) ?? [];
    const path = onLine ? this.hosted.get(key)!.repo.resolve(cr.source).changes[0]?.path ?? "" : "";
    const id = `c${this.nextComment++}`;
    const c = create(CommentSchema, {
      id,
      author,
      body,
      createdAtMs: BigInt(Math.round(at)),
      editedAtMs: 0n,
      path,
      line: onLine ? 1 + Math.floor(this.random() * 6) : 0,
      onBase: false,
      atTip: cr.sourceTip,
      thread: id,
      resolved: false,
      suggestion: "",
      endLine: 0,
    });
    list.push(c);
    if (onLine && this.random() < 0.6) {
      list.push(create(CommentSchema, { ...c, id: `c${this.nextComment++}`, author: cr.author, body: agentReplies[Math.floor(this.random() * agentReplies.length)]!, createdAtMs: BigInt(Math.round(at + 4 * 60_000)) }));
    }
    this.comments.set(ck, list);
    cr.comments = list.length;
    this.event(EventKind.COMMENTED_ON, author, cr, `commented on #${cr.number}`, at);
  }

  private event(kind: EventKind, actor: string, cr: CR, summary: string, at: number) {
    this.events.push(create(EventSchema, { id: `e${this.events.length}`, kind, actor, repository: cr.repository, atMs: BigInt(Math.round(at)), changeRequest: cr.number, summary }));
  }

  /** Recompute what stands between a change request and landing. */
  private refresh(key: string, cr: CR) {
    if (cr.state !== ChangeRequestState.OPEN) {
      cr.blockers = [];
      return;
    }
    const h = this.hosted.get(key)!;
    const rule = this.protections.get(key)!.find((p) => p.pattern === cr.target);
    cr.conflicts = h.repo.conflicts(cr.source, hex(cr.base));
    const blockers: string[] = [];
    const approvals = new Set(cr.reviews.filter((r) => r.verdict === Verdict.APPROVED && hex(r.reviewedTip) === hex(cr.sourceTip)).map((r) => r.reviewer)).size;
    const changes = cr.reviews.filter((r) => r.verdict === Verdict.CHANGES_REQUESTED);
    if (rule && approvals < rule.requiredApprovals) {
      const n = rule.requiredApprovals - approvals;
      blockers.push(`Needs ${n} more ${n === 1 ? "approval" : "approvals"}`);
    }
    if (changes.length) blockers.push(`${changes[0]!.reviewer} requested changes`);
    const failed = cr.checks.filter((c) => c.state === CheckState.FAILED).map((c) => c.name);
    const pending = cr.checks.filter((c) => c.state === CheckState.PENDING).map((c) => c.name);
    if (failed.length) blockers.push(`${failed.join(", ")} failed`);
    else if (pending.length) blockers.push(`Waiting for ${pending.join(", ")}`);
    if (cr.conflicts.length) blockers.push(`Conflicts in ${cr.conflicts.length} ${cr.conflicts.length === 1 ? "file" : "files"}`);
    cr.blockers = blockers;
  }

  // ---- reads and writes, as the server answers them ----

  account(login: string) {
    if (login === ORG) return { login: ORG, displayName: "Lumen Labs", kind: AccountKind.ORGANIZATION, owner: "", avatarUrl: "", createdAtMs: 0n, siteAdmin: false };
    if (people[login]) return { login, displayName: people[login]!, kind: AccountKind.PERSON, owner: "", avatarUrl: "", createdAtMs: 0n, siteAdmin: login === VIEWER };
    if (agentOwners[login]) return { login, displayName: login.split("/")[1]!, kind: AccountKind.AGENT, owner: agentOwners[login]!, avatarUrl: "", createdAtMs: 0n, siteAdmin: false };
    throw new ConnectError(`no account \`${login}\``, Code.NotFound);
  }

  organization() {
    return {
      login: ORG,
      displayName: "Lumen Labs",
      description: "Search and checkout for 40 million shoppers, built by a small team and a large fleet of agents.",
      createdAtMs: BigInt(Date.now() - 400 * 86_400_000),
      viewerRole: Role.ADMIN,
      repositories: this.hosted.size,
      members: Object.keys(people).length,
      agents: Object.keys(agentOwners).length,
    };
  }

  repositories() {
    return [...this.hosted.entries()].map(([key, h]) => this.repository(key, h));
  }

  repository(key: string, h = this.hosted.get(key)) {
    if (!h) throw new ConnectError(`no repository \`${key}\``, Code.NotFound);
    const s = h.repo.stats();
    return {
      owner: ORG,
      name: h.seed.name,
      description: h.seed.description,
      visibility: h.visibility,
      defaultBranch: "main",
      createdAtMs: BigInt(h.created),
      viewerRole: Role.ADMIN,
      archived: false,
      lands24h: s.lands,
      openChangeRequests: this.crs.get(key)!.filter((c) => c.state === ChangeRequestState.OPEN).length,
      tip: s.tip,
      tipAtMs: BigInt(s.at),
      topics: h.seed.topics,
    };
  }

  members() {
    return [
      ...Object.keys(people).map((login) => ({ account: this.account(login), role: login === VIEWER ? Role.ADMIN : Role.MAINTAIN, teams: login === "dev" ? ["platform"] : ["product"] })),
      ...Object.keys(agentOwners).map((login) => ({ account: this.account(login), role: Role.WRITE, teams: ["agents"] })),
    ];
  }

  changeRequests(key: string) {
    return [...(this.crs.get(key) ?? [])].sort((a, b) => Number(b.number - a.number));
  }

  changeRequest(key: string, number: bigint): CR {
    const cr = this.crs.get(key)?.find((c) => c.number === number);
    if (!cr) throw new ConnectError(`no change request #${number}`, Code.NotFound);
    return cr;
  }

  commentsOf(key: string, number: bigint) {
    return this.comments.get(`${key}#${number}`) ?? [];
  }

  addComment(key: string, number: bigint, body: string, path: string, line: number, thread: string, suggestion: string) {
    const cr = this.changeRequest(key, number);
    if (!body.trim()) throw new ConnectError("a comment needs some text", Code.InvalidArgument);
    const id = `c${this.nextComment++}`;
    const c = create(CommentSchema, { id, author: VIEWER, body, createdAtMs: BigInt(Date.now()), path, line, atTip: cr.sourceTip, thread: thread || id, suggestion });
    const list = this.comments.get(`${key}#${number}`) ?? [];
    list.push(c);
    this.comments.set(`${key}#${number}`, list);
    cr.comments = list.length;
    cr.updatedAtMs = BigInt(Date.now());
    this.event(EventKind.COMMENTED_ON, VIEWER, cr, `commented on #${number}`, Date.now());
    this.listeners.forEach((l) => l());
    return c;
  }

  resolve(key: string, number: bigint, thread: string, resolved: boolean) {
    const list = this.commentsOf(key, number);
    const head = list.find((c) => c.id === thread);
    if (!head) throw new ConnectError("no such thread", Code.NotFound);
    list.filter((c) => c.thread === thread).forEach((c) => (c.resolved = resolved));
    return head;
  }

  review(key: string, number: bigint, verdict: Verdict, body: string) {
    const cr = this.changeRequest(key, number);
    if (cr.state !== ChangeRequestState.OPEN) throw new ConnectError("this change request is not open", Code.FailedPrecondition);
    if (body.trim()) this.addComment(key, number, body, "", 0, "", "");
    cr.reviews = [...cr.reviews.filter((r) => r.reviewer !== VIEWER), create(ReviewSummarySchema, { reviewer: VIEWER, verdict, atMs: BigInt(Date.now()), reviewedTip: cr.sourceTip })];
    cr.requestedReviewers = cr.requestedReviewers.filter((r) => r !== VIEWER);
    this.event(EventKind.REVIEWED, VIEWER, cr, `${verdict === Verdict.APPROVED ? "approved" : verdict === Verdict.CHANGES_REQUESTED ? "requested changes on" : "reviewed"} #${number}`, Date.now());
    this.refresh(key, cr);
    this.listeners.forEach((l) => l());
    return cr;
  }

  land(key: string, number: bigint) {
    const cr = this.changeRequest(key, number);
    this.refresh(key, cr);
    if (cr.state !== ChangeRequestState.OPEN) throw new ConnectError("this change request is not open", Code.FailedPrecondition);
    if (cr.blockers.length) throw new ConnectError(`can't land yet: ${cr.blockers.join("; ")}`, Code.FailedPrecondition);
    const h = this.hosted.get(key)!;
    const id = h.repo.landBranch(cr.source, cr.author, cr.title);
    cr.state = ChangeRequestState.LANDED;
    cr.landedAs = this.bytes(id);
    cr.updatedAtMs = BigInt(Date.now());
    cr.blockers = [];
    this.event(EventKind.CHANGE_REQUEST_LANDED, VIEWER, cr, `landed #${number}`, Date.now());
    this.listeners.forEach((l) => l());
    return { changeRequest: cr, landedAs: cr.landedAs };
  }

  rebase(key: string, number: bigint) {
    const cr = this.changeRequest(key, number);
    if (cr.state !== ChangeRequestState.OPEN) throw new ConnectError("this change request is not open", Code.FailedPrecondition);
    const h = this.hosted.get(key)!;
    const r = h.repo.rebaseBranch(cr.source, hex(cr.base));
    if ("conflicts" in r) throw new ConnectError(`these files changed in the same place on ${cr.target}: ${r.conflicts.join(", ")}`, Code.FailedPrecondition);
    cr.base = this.bytes(r.base);
    cr.sourceTip = this.bytes(r.tip);
    cr.updatedAtMs = BigInt(Date.now());
    // Fresh checks for the new version.
    for (const c of cr.checks) {
      c.state = CheckState.PENDING;
      c.startedAtMs = BigInt(Date.now());
      c.summary = "";
    }
    this.event(EventKind.COMMENTED_ON, VIEWER, cr, `rebased #${number} onto ${cr.target}`, Date.now());
    this.refresh(key, cr);
    this.listeners.forEach((l) => l());
    return cr;
  }

  close(key: string, number: bigint, state: ChangeRequestState) {
    const cr = this.changeRequest(key, number);
    cr.state = state;
    cr.updatedAtMs = BigInt(Date.now());
    if (state === ChangeRequestState.CLOSED) this.event(EventKind.CHANGE_REQUEST_CLOSED, VIEWER, cr, `closed #${number}`, Date.now());
    this.refresh(key, cr);
    return cr;
  }

  inbox() {
    const items: { changeRequest: CR; reason: string; unread: boolean }[] = [];
    for (const list of this.crs.values()) {
      for (const cr of list) {
        if (cr.state !== ChangeRequestState.OPEN) continue;
        const mine = cr.reviews.find((r) => r.reviewer === VIEWER);
        if (cr.requestedReviewers.includes(VIEWER) && !mine) items.push({ changeRequest: cr, reason: "Review requested", unread: Date.now() - Number(cr.createdAtMs) < 30 * 60_000 });
        else if (agentOwners[cr.author] === VIEWER && cr.blockers.length === 0) items.push({ changeRequest: cr, reason: "Your agent is ready to land", unread: false });
      }
    }
    return items.sort((a, b) => Number(b.changeRequest.updatedAtMs - a.changeRequest.updatedAtMs));
  }

  eventsOf(key: string | null, limit: number) {
    return this.events
      .filter((e) => !key || `${e.repository?.owner}/${e.repository?.name}` === key)
      .sort((a, b) => Number(b.atMs - a.atMs))
      .slice(0, limit || 50);
  }

  protectionsOf(key: string) {
    return this.protections.get(key) ?? [];
  }
}

const keyOf = (r: { owner: string; name: string } | undefined | null) => (r ? `${r.owner}/${r.name}` : "");
const wait = () => new Promise((r) => setTimeout(r, 12 + Math.random() * 40));

/** A transport answered by the demo platform; repository calls are routed by header. */
export function platformTransport(p: DemoPlatform, options?: { interceptors?: Interceptor[] }): Transport {
  const repo = (ctx: HandlerContext) => p.repoFor(ctx.requestHeader.get(REPO_HEADER));
  return createRouterTransport((router) => {
    repoServices(router, repo, [Refs, Changesets, History, Diff, Browse, Locks, Access]);
    router.service(Platform, {
      async viewer() {
        await wait();
        return { account: p.account(VIEWER), organizations: [p.organization()] };
      },
      async getAccount(r) {
        return p.account(r.login);
      },
      async listOrganizations() {
        return { organizations: [p.organization()] };
      },
      async getOrganization(r) {
        if (r.login !== ORG) throw new ConnectError(`no organisation \`${r.login}\``, Code.NotFound);
        return p.organization();
      },
      createOrganization() {
        throw new ConnectError("the demo is read-only for organisations", Code.PermissionDenied);
      },
      async listMembers() {
        await wait();
        return { members: p.members() };
      },
      setMember() {
        throw new ConnectError("the demo can't change members", Code.PermissionDenied);
      },
      async listTeams() {
        return {
          teams: [
            { org: ORG, slug: "product", name: "Product", description: "Storefront and search", members: ["kabir", "mira", "sana"] },
            { org: ORG, slug: "platform", name: "Platform", description: "Infrastructure and tooling", members: ["dev"] },
            { org: ORG, slug: "agents", name: "Agents", description: "Every agent account", members: Object.keys(agentOwners) },
          ],
        };
      },
      async listRepositories(r) {
        await wait();
        const q = r.query.toLowerCase();
        return { repositories: p.repositories().filter((x) => !q || x.name.includes(q) || x.description.toLowerCase().includes(q)), nextPageToken: "" };
      },
      async getRepository(r) {
        await wait();
        return p.repository(keyOf(r));
      },
      createRepository() {
        throw new ConnectError("the demo can't create repositories; connect a server to try it", Code.PermissionDenied);
      },
      updateRepository() {
        throw new ConnectError("the demo can't change repository settings", Code.PermissionDenied);
      },
      async listGrants() {
        return { grants: [{ principal: `${ORG}/product`, role: Role.WRITE }, { principal: `${ORG}/agents`, role: Role.WRITE }, { principal: `${ORG}/platform`, role: Role.ADMIN }] };
      },
      setGrant() {
        throw new ConnectError("the demo can't change access", Code.PermissionDenied);
      },
      async listTokens() {
        const now = Date.now();
        return {
          tokens: [
            { id: "t1", name: "laptop", actsAs: "", repositories: [], maxRole: Role.ADMIN, createdAtMs: BigInt(now - 40 * 86_400_000), expiresAtMs: BigInt(now + 50 * 86_400_000), lastUsedAtMs: BigInt(now - 3_600_000) },
            { id: "t2", name: "perf fleet", actsAs: "agent/perf-05", repositories: [`${ORG}/fleet`], maxRole: Role.WRITE, createdAtMs: BigInt(now - 9 * 86_400_000), expiresAtMs: BigInt(now + 21 * 86_400_000), lastUsedAtMs: BigInt(now - 20_000) },
          ],
        };
      },
      createToken() {
        throw new ConnectError("the demo can't issue tokens", Code.PermissionDenied);
      },
      revokeToken() {
        throw new ConnectError("the demo can't revoke tokens", Code.PermissionDenied);
      },
    });
    router.service(Reviews, {
      async listChangeRequests(r) {
        await wait();
        const states = new Set(r.states);
        return { changeRequests: p.changeRequests(keyOf(r.repository)).filter((c) => (states.size === 0 || states.has(c.state)) && (!r.author || c.author === r.author)), nextPageToken: "" };
      },
      async getChangeRequest(r) {
        await wait();
        return p.changeRequest(keyOf(r.repository), r.number);
      },
      createChangeRequest() {
        throw new ConnectError("open change requests from the command line in the demo: agents do it for you here", Code.Unimplemented);
      },
      async updateChangeRequest(r) {
        const key = keyOf(r.ref?.repository);
        if (r.fields.includes("state")) return p.close(key, r.ref!.number, r.state);
        return p.changeRequest(key, r.ref!.number);
      },
      async listComments(r) {
        await wait();
        return { comments: p.commentsOf(keyOf(r.repository), r.number) };
      },
      async addComment(r) {
        return p.addComment(keyOf(r.ref?.repository), r.ref!.number, r.body, r.path, r.line, r.thread, r.suggestion);
      },
      async resolveThread(r) {
        return p.resolve(keyOf(r.ref?.repository), r.ref!.number, r.thread, r.resolved);
      },
      async submitReview(r) {
        return p.review(keyOf(r.ref?.repository), r.ref!.number, r.verdict, r.body);
      },
      async land(r) {
        await wait();
        return p.land(keyOf(r.repository), r.number);
      },
      async rebase(r) {
        await wait();
        return p.rebase(keyOf(r.repository), r.number);
      },
      async listProtections(r) {
        return { protections: p.protectionsOf(keyOf(r)) };
      },
      setProtection() {
        throw new ConnectError("the demo can't change protection", Code.PermissionDenied);
      },
      async inbox() {
        await wait();
        return { items: p.inbox() };
      },
      async listEvents(r) {
        await wait();
        return { events: p.eventsOf(r.repository ? keyOf(r.repository) : null, r.limit), nextPageToken: "" };
      },
    });
  }, { transport: { interceptors: options?.interceptors ?? [] } });
}
