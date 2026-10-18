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
  Issues,
  IssueSchema,
  IssueState,
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
  type Issue,
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

  // Access changes made in the demo: kept in memory for this tab only.
  private memberRoles = new Map<string, Role>();
  private grantLists = new Map<string, { principal: string; role: Role }[]>();
  private tokenList: { id: string; name: string; actsAs: string; repositories: string[]; maxRole: Role; createdAtMs: bigint; expiresAtMs: bigint; lastUsedAtMs: bigint }[] = [];

  members() {
    const base = [
      ...Object.keys(people).map((login) => ({ account: this.account(login), role: login === VIEWER ? Role.ADMIN : Role.MAINTAIN, teams: login === "dev" ? ["platform"] : ["product"] })),
      ...Object.keys(agentOwners).map((login) => ({ account: this.account(login), role: Role.WRITE, teams: ["agents"] })),
    ];
    for (const [login, role] of this.memberRoles)
      if (!base.some((m) => m.account.login === login))
        base.push({ account: { login, displayName: login.split("@")[0]!, kind: AccountKind.PERSON, owner: "", avatarUrl: "", createdAtMs: BigInt(Date.now()), siteAdmin: false }, role, teams: [] });
    return base.map((m) => ({ ...m, role: this.memberRoles.get(m.account.login) ?? m.role })).filter((m) => m.role !== Role.UNSPECIFIED);
  }

  setMember(login: string, role: Role) {
    if (!/^[A-Za-z0-9._@+/-]{1,100}$/.test(login)) throw new ConnectError(`\`${login}\` is not a valid login`, Code.InvalidArgument);
    if (login === VIEWER && role !== Role.ADMIN) throw new ConnectError(`\`${login}\` is the last administrator of ${ORG}; make someone else one first`, Code.FailedPrecondition);
    this.memberRoles.set(login, role);
    return { account: this.members().find((m) => m.account.login === login)?.account, role, teams: [] };
  }

  grants(key: string) {
    if (!this.grantLists.has(key))
      this.grantLists.set(key, [
        { principal: `${ORG}/product`, role: Role.WRITE },
        { principal: `${ORG}/agents`, role: Role.WRITE },
        { principal: `${ORG}/platform`, role: Role.ADMIN },
      ]);
    return this.grantLists.get(key)!;
  }

  setGrant(key: string, principal: string, role: Role) {
    if (!/^[A-Za-z0-9._@+/-]{1,100}$/.test(principal)) throw new ConnectError(`\`${principal}\` is not a valid login or team`, Code.InvalidArgument);
    const rest = this.grants(key).filter((g) => g.principal !== principal);
    this.grantLists.set(key, role === Role.UNSPECIFIED ? rest : [...rest, { principal, role }].sort((a, b) => a.principal.localeCompare(b.principal)));
    return { principal, role };
  }

  tokens() {
    if (!this.tokenList.length && !this.tokensTouched) {
      const now = Date.now();
      this.tokenList = [
        { id: "t1", name: "laptop", actsAs: "", repositories: [], maxRole: Role.ADMIN, createdAtMs: BigInt(now - 40 * 86_400_000), expiresAtMs: BigInt(now + 50 * 86_400_000), lastUsedAtMs: BigInt(now - 3_600_000) },
        { id: "t2", name: "perf fleet", actsAs: "agent/perf-05", repositories: [`${ORG}/fleet`], maxRole: Role.WRITE, createdAtMs: BigInt(now - 9 * 86_400_000), expiresAtMs: BigInt(now + 21 * 86_400_000), lastUsedAtMs: BigInt(now - 20_000) },
      ];
    }
    return this.tokenList;
  }
  private tokensTouched = false;

  createToken(t: { name: string; actsAs: string; repositories: string[]; maxRole: Role; expiresAtMs: bigint }) {
    if (!t.name.trim()) throw new ConnectError("give the token a name of up to 100 characters", Code.InvalidArgument);
    if (t.actsAs && !t.actsAs.startsWith("agent/") && t.actsAs !== VIEWER)
      throw new ConnectError("a token can act only as you or as an agent (`agent/NAME`) you answer for", Code.PermissionDenied);
    for (const r of t.repositories) if (!this.hosted.has(r)) throw new ConnectError(`no repository \`${r}\` on this server`, Code.NotFound);
    const bytes = crypto.getRandomValues(new Uint8Array(32));
    // Marked as a demo token so it can never be mistaken for a real one.
    const secret = `pando_demo_${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
    const token = { id: `t${Date.now().toString(36)}`, name: t.name.trim(), actsAs: t.actsAs || VIEWER, repositories: t.repositories, maxRole: t.maxRole || Role.ADMIN, createdAtMs: BigInt(Date.now()), expiresAtMs: t.expiresAtMs, lastUsedAtMs: 0n };
    this.tokens().unshift(token);
    return { token, secret };
  }

  private hooks: { id: string; repo: string; url: string; events: string[]; deliveries: { id: string; event: string; createdAtMs: bigint }[] }[] = [];

  webhooksOf(repo: string) {
    return this.hooks.filter((h) => h.repo === repo).map((h) => this.hookView(h));
  }

  private hookView(h: (typeof this.hooks)[number]) {
    const [owner, name] = h.repo.split("/");
    return { id: h.id, repository: { owner: owner!, name: name! }, url: h.url, events: h.events, active: true, createdAtMs: 0n, createdBy: VIEWER, lastStatus: 0, lastDeliveryAtMs: 0n };
  }

  createWebhook(repo: string, url: string, events: string[]) {
    if (!/^https?:\/\/[^/\s]+/.test(url)) throw new ConnectError("a webhook calls an http:// or https:// URL", Code.InvalidArgument);
    const h = { id: `h${Date.now().toString(36)}`, repo, url, events, deliveries: [] };
    this.hooks.push(h);
    const secret = [...crypto.getRandomValues(new Uint8Array(32))].map((b) => b.toString(16).padStart(2, "0")).join("");
    return { webhook: this.hookView(h), secret };
  }

  private hook(id: string) {
    const h = this.hooks.find((x) => x.id === id);
    if (!h) throw new ConnectError(`no webhook \`${id}\``, Code.NotFound);
    return h;
  }

  pingWebhook(id: string) {
    const d = { id: `d${Date.now().toString(36)}`, event: "ping", createdAtMs: BigInt(Date.now()) };
    this.hook(id).deliveries.unshift(d);
    return this.deliveryView(d);
  }

  private deliveryView(d: { id: string; event: string; createdAtMs: bigint }) {
    return { ...d, status: 0, error: "the demo doesn't call URLs; on your server this is sent now", attempts: 0, delivered: false, payload: "{}" };
  }

  deliveriesOf(id: string) {
    return this.hook(id).deliveries.map((d) => this.deliveryView(d));
  }

  deleteWebhook(id: string) {
    this.hook(id);
    this.hooks = this.hooks.filter((h) => h.id !== id);
  }

  // Organisations and invitations made in the demo: kept in memory for this tab only.
  private madeOrgs: { login: string; displayName: string; createdAtMs: bigint }[] = [];
  private invites: { id: string; org: string; email: string; role: Role; invitedBy: string; createdAtMs: bigint; expiresAtMs: bigint }[] = [];

  organizations() {
    return [this.organization(), ...this.madeOrgs.map((o) => ({ ...o, description: "", viewerRole: Role.ADMIN, repositories: 0, members: 1, agents: 0 }))];
  }

  createOrganization(login: string, displayName: string) {
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,38}$/.test(login)) throw new ConnectError(`\`${login}\` is not a valid organisation name: use letters, digits, \`.\`, \`_\` and \`-\``, Code.InvalidArgument);
    if (login === ORG || people[login] || this.madeOrgs.some((o) => o.login === login)) throw new ConnectError(`\`${login}\` is taken`, Code.AlreadyExists);
    this.madeOrgs.push({ login, displayName: displayName.trim() || login, createdAtMs: BigInt(Date.now()) });
    return this.organizations().at(-1)!;
  }

  invitations(org: string) {
    return this.invites.filter((i) => i.org === org);
  }

  createInvitation(org: string, email: string, role: Role) {
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email.trim())) throw new ConnectError(`\`${email}\` is not an email address`, Code.InvalidArgument);
    const id = Math.random().toString(16).slice(2, 18).padEnd(16, "0");
    const now = Date.now();
    const invitation = { id, org, email: email.trim().toLowerCase(), role: role || Role.READ, invitedBy: VIEWER, createdAtMs: BigInt(now), expiresAtMs: BigInt(now + 7 * 86_400_000) };
    this.invites.push(invitation);
    const secret = Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join("");
    return { invitation, link: `https://pando.example.com/login/invite/${secret}`, emailed: false };
  }

  revokeInvitation(org: string, id: string) {
    const before = this.invites.length;
    this.invites = this.invites.filter((i) => !(i.org === org && i.id === id));
    if (this.invites.length === before) throw new ConnectError(`no invitation \`${id}\``, Code.NotFound);
  }

  revokeToken(id: string) {
    const before = this.tokens().length;
    this.tokenList = this.tokens().filter((t) => t.id !== id);
    this.tokensTouched = true;
    if (this.tokenList.length === before) throw new ConnectError(`no token \`${id}\``, Code.NotFound);
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

  addComment(key: string, number: bigint, body: string, path: string, line: number, thread: string, suggestion: string, endLine = 0) {
    const cr = this.changeRequest(key, number);
    if (!body.trim()) throw new ConnectError("a comment needs some text", Code.InvalidArgument);
    const id = `c${this.nextComment++}`;
    const c = create(CommentSchema, { id, author: VIEWER, body, createdAtMs: BigInt(Date.now()), path, line, endLine, atTip: cr.sourceTip, thread: thread || id, suggestion });
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

  // Issues, kept in memory; numbered after the change requests, as on a server.
  private issueLists = new Map<string, Issue[]>();
  private issueComments = new Map<string, Comment[]>();

  issuesOf(key: string) {
    if (!this.issueLists.has(key)) {
      const [owner, name] = key.split("/");
      const at = (h: number) => BigInt(Date.now() - h * 3_600_000);
      const base = Math.max(0, ...(this.crs.get(key) ?? []).map((c) => Number(c.number))) - 40;
      const make = (n: number, i: Omit<Partial<Issue>, "$typeName" | "$unknown">) =>
        create(IssueSchema, { repository: { owner, name }, number: BigInt(base + n), state: IssueState.OPEN, createdAtMs: at(30), updatedAtMs: at(2), ...i });
      this.issueLists.set(
        key,
        key.endsWith("/fleet")
          ? [
              make(1, { title: "Refunds over 30 days need finance approval", author: "mira", body: "Enterprise customers ask for 45 days. Keep 30 for everyone else, and route longer ones to finance.", labels: ["billing"], assignees: [VIEWER] }),
              make(2, { title: "Search ranking ignores stock levels", author: "sana", body: "Out-of-stock items still rank first for popular queries.", labels: ["search"], assignees: ["agent/perf-05"], updatedAtMs: at(1) }),
              make(3, { title: "Flaky checkout end-to-end test", author: "dev", labels: ["tests"], state: IssueState.COMPLETED, closedAtMs: at(20), updatedAtMs: at(20) }),
            ]
          : [],
      );
    }
    return this.issueLists.get(key)!;
  }

  issue(key: string, number: bigint) {
    const i = this.issuesOf(key).find((x) => x.number === number);
    if (!i) throw new ConnectError(`no issue #${number} in ${key}`, Code.NotFound);
    return { ...i, comments: (this.issueComments.get(`${key}#${number}`) ?? []).length };
  }

  createIssue(key: string, r: { title: string; body: string; labels: string[]; assignees: string[] }) {
    if (!r.title.trim()) throw new ConnectError("give the issue a title of up to 300 characters", Code.InvalidArgument);
    const [owner, name] = key.split("/");
    const numbers = [...(this.crs.get(key) ?? []).map((c) => Number(c.number)), ...this.issuesOf(key).map((i) => Number(i.number))];
    const now = BigInt(Date.now());
    const i = create(IssueSchema, { repository: { owner, name }, number: BigInt(Math.max(0, ...numbers) + 1), title: r.title.trim(), body: r.body, author: VIEWER, state: IssueState.OPEN, labels: r.labels, assignees: r.assignees, createdAtMs: now, updatedAtMs: now });
    this.issuesOf(key).unshift(i);
    return i;
  }

  updateIssue(key: string, number: bigint, r: { fields: string[]; title: string; body: string; state: IssueState; labels: string[]; assignees: string[] }) {
    const i = this.issuesOf(key).find((x) => x.number === number);
    if (!i) throw new ConnectError(`no issue #${number} in ${key}`, Code.NotFound);
    for (const f of r.fields) {
      if (f === "title" && r.title.trim()) i.title = r.title.trim();
      if (f === "body") i.body = r.body;
      if (f === "labels") i.labels = r.labels;
      if (f === "assignees") i.assignees = r.assignees;
      if (f === "state") {
        i.state = r.state;
        i.closedAtMs = r.state === IssueState.OPEN ? 0n : BigInt(Date.now());
      }
    }
    i.updatedAtMs = BigInt(Date.now());
    return this.issue(key, number);
  }

  issueCommentsOf(key: string, number: bigint) {
    return this.issueComments.get(`${key}#${number}`) ?? [];
  }

  addIssueComment(key: string, number: bigint, body: string) {
    this.issue(key, number);
    if (!body.trim()) throw new ConnectError("a comment needs some text", Code.InvalidArgument);
    const id = `ic${Date.now().toString(36)}`;
    const c = create(CommentSchema, { id, author: VIEWER, body: body.trim(), createdAtMs: BigInt(Date.now()), thread: id });
    this.issueComments.set(`${key}#${number}`, [...this.issueCommentsOf(key, number), c]);
    return c;
  }

  inbox() {
    const items: { changeRequest?: CR; issue?: Issue; reason: string; unread: boolean }[] = [];
    for (const key of this.hosted.keys())
      for (const i of this.issuesOf(key)) if (i.state === IssueState.OPEN && i.assignees.includes(VIEWER)) items.push({ issue: i, reason: "Assigned to you", unread: false });
    for (const list of this.crs.values()) {
      for (const cr of list) {
        if (cr.state !== ChangeRequestState.OPEN) continue;
        const mine = cr.reviews.find((r) => r.reviewer === VIEWER);
        if (cr.requestedReviewers.includes(VIEWER) && !mine) items.push({ changeRequest: cr, reason: "Review requested", unread: Date.now() - Number(cr.createdAtMs) < 30 * 60_000 });
        else if (agentOwners[cr.author] === VIEWER && cr.blockers.length === 0) items.push({ changeRequest: cr, reason: "Your agent is ready to land", unread: false });
      }
    }
    const when = (x: (typeof items)[number]) => (x.changeRequest ?? x.issue)!.updatedAtMs;
    return items.sort((a, b) => Number(when(b) - when(a)));
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

  setProtectionOf(key: string, p: Protection) {
    const pattern = p.pattern.trim();
    if (!pattern || pattern.slice(0, -1).includes("*")) throw new ConnectError("protect a branch (`main`) or a prefix (`release/*`, `*`)", Code.InvalidArgument);
    const saved = create(ProtectionSchema, { ...p, pattern });
    this.protections.set(key, [...this.protectionsOf(key).filter((x) => x.pattern !== pattern), saved].sort((a, b) => a.pattern.localeCompare(b.pattern)));
    return saved;
  }

  deleteProtectionOf(key: string, pattern: string) {
    const before = this.protectionsOf(key);
    if (!before.some((x) => x.pattern === pattern)) throw new ConnectError(`\`${pattern}\` is not protected`, Code.NotFound);
    this.protections.set(key, before.filter((x) => x.pattern !== pattern));
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
        return { account: p.account(VIEWER), organizations: p.organizations(), tokenId: "" };
      },
      async getAccount(r) {
        return p.account(r.login);
      },
      async listOrganizations() {
        return { organizations: p.organizations() };
      },
      async getOrganization(r) {
        const o = p.organizations().find((x) => x.login === r.login);
        if (!o) throw new ConnectError(`no organisation \`${r.login}\``, Code.NotFound);
        return o;
      },
      async createOrganization(r) {
        return p.createOrganization(r.login, r.displayName);
      },
      async listMembers(r) {
        await wait();
        return { members: r.login === ORG ? p.members() : [{ account: p.account(VIEWER), role: Role.ADMIN, teams: [] }] };
      },
      async createInvitation(r) {
        return p.createInvitation(r.org, r.email, r.role);
      },
      async listInvitations(r) {
        return { invitations: p.invitations(r.login) };
      },
      async revokeInvitation(r) {
        p.revokeInvitation(r.org, r.id);
        return {};
      },
      async setMember(r) {
        return p.setMember(r.login, r.role);
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
      async listGrants(r) {
        return { grants: p.grants(keyOf(r)) };
      },
      async setGrant(r) {
        return p.setGrant(keyOf(r.repository), r.principal, r.role);
      },
      async listTokens() {
        return { tokens: p.tokens() };
      },
      async createToken(r) {
        return p.createToken(r);
      },
      async revokeToken(r) {
        p.revokeToken(r.id);
        return {};
      },
      async listWebhooks(r) {
        return { webhooks: p.webhooksOf(keyOf(r)) };
      },
      async createWebhook(r) {
        return p.createWebhook(keyOf(r.repository), r.url, r.events);
      },
      async deleteWebhook(r) {
        p.deleteWebhook(r.id);
        return {};
      },
      async listWebhookDeliveries(r) {
        return { deliveries: p.deliveriesOf(r.id) };
      },
      async pingWebhook(r) {
        return p.pingWebhook(r.id);
      },
    });
    router.service(Issues, {
      async listIssues(r) {
        await wait();
        const states = r.states.length ? r.states : [IssueState.OPEN];
        return { issues: p.issuesOf(keyOf(r.repository)).filter((i) => states.includes(i.state)).map((i) => p.issue(keyOf(r.repository), i.number)) };
      },
      async getIssue(r) {
        await wait();
        return p.issue(keyOf(r.repository), r.number);
      },
      async createIssue(r) {
        return p.createIssue(keyOf(r.repository), r);
      },
      async updateIssue(r) {
        return p.updateIssue(keyOf(r.ref?.repository), r.ref!.number, r);
      },
      async listIssueComments(r) {
        return { comments: p.issueCommentsOf(keyOf(r.repository), r.number) };
      },
      async addIssueComment(r) {
        return p.addIssueComment(keyOf(r.ref?.repository), r.ref!.number, r.body);
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
        return p.addComment(keyOf(r.ref?.repository), r.ref!.number, r.body, r.path, r.line, r.thread, r.suggestion, r.endLine);
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
      async setProtection(r) {
        return p.setProtectionOf(keyOf(r.repository), r);
      },
      async deleteProtection(r) {
        p.deleteProtectionOf(keyOf(r.repository), r.pattern);
        return {};
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
