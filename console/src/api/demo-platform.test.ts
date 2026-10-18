import { describe, expect, it } from "vitest";
import { ChangeRequestState, CheckState, Role, Verdict } from "../gen/pando_platform_pb";
import { DemoPlatform } from "./demo-platform";

describe("demo platform", () => {
  const p = new DemoPlatform();

  it("hosts four repositories with history", () => {
    const repos = p.repositories();
    expect(repos.map((r) => r.name)).toEqual(["fleet", "storefront", "infra", "ranking"]);
    expect(repos.every((r) => r.lands24h > 0)).toBe(true);
  });

  it("explains what blocks a change request, and lands it once nothing does", () => {
    const open = p.changeRequests("lumen/fleet").find((c) => c.state === ChangeRequestState.OPEN && c.reviews.length === 0 && c.conflicts.length === 0)!;
    expect(open).toBeDefined();
    expect(open.blockers.join(" ")).toMatch(/approval/);
    expect(() => p.land("lumen/fleet", open.number)).toThrow(/can't land yet/);

    open.checks.forEach((c) => (c.state = CheckState.PASSED));
    const before = p.repository("lumen/fleet").tip;
    p.review("lumen/fleet", open.number, Verdict.APPROVED, "");
    expect(open.blockers).toEqual([]);
    const { landedAs } = p.land("lumen/fleet", open.number);
    expect(open.state).toBe(ChangeRequestState.LANDED);
    expect(p.repository("lumen/fleet").tip).not.toEqual(before);
    expect(p.repository("lumen/fleet").tip).toEqual(landedAs);
  });

  it("reports paths changed on main since a change request's base as conflicts", () => {
    const all = p.changeRequests("lumen/fleet");
    const withConflict = all.find((c) => c.conflicts.length > 0);
    if (withConflict) expect(withConflict.blockers.join(" ")).toMatch(/Conflicts in/);
    expect(all.some((c) => c.state === ChangeRequestState.OPEN)).toBe(true);
  });

  it("rebases onto main: a clean re-apply clears the conflict and makes old approvals stale", () => {
    const repo = "lumen/fleet";
    // Make a conflict on purpose: land something on main that touches a change request's file.
    const cr = p.changeRequests(repo).find((c) => c.state === ChangeRequestState.OPEN && c.conflicts.length === 0)!;
    p.review(repo, cr.number, Verdict.APPROVED, "");
    const r = p.repoFor(repo);
    const path = r.resolve(cr.source).changes[0]!.path;
    const text = new TextDecoder().decode(r.file("main", path).bytes);
    // An edit far from the agent's (a new first line) merges cleanly.
    r.landWith(path, "// header added on main\n" + text, "kabir", "Add a header");
    p.close(repo, cr.number, ChangeRequestState.OPEN);
    expect(cr.conflicts).toEqual([path]);
    p.rebase(repo, cr.number);
    expect(cr.conflicts).toEqual([]);
    expect(cr.blockers.join(" ")).toMatch(/approval/);
    const after = new TextDecoder().decode(r.file(cr.source, path).bytes);
    expect(after).toContain("// header added on main");
  });

  it("refuses a rebase whose edits overlap main's, naming the file", () => {
    const repo = "lumen/fleet";
    const cr = p.changeRequests(repo).find((c) => c.state === ChangeRequestState.OPEN && c.conflicts.length === 0)!;
    const r = p.repoFor(repo);
    const path = r.resolve(cr.source).changes[0]!.path;
    // Rewrite every line on main: the agent's edit can't apply anywhere.
    const text = new TextDecoder().decode(r.file("main", path).bytes);
    r.landWith(path, text.split("\n").map((l) => `${l} // rewritten`).join("\n"), "kabir", "Rewrite");
    expect(() => p.rebase(repo, cr.number)).toThrow(path);
  });

  it("puts requested reviews in the viewer's inbox", () => {
    expect(p.inbox().length).toBeGreaterThan(0);
    expect(p.inbox().every((i) => (i.changeRequest ? i.changeRequest.state === ChangeRequestState.OPEN : i.issue?.assignees.includes("kabir")))).toBe(true);
  });

  it("creates organisations and invitations, and refuses bad or taken names", () => {
    expect(() => p.createOrganization("bad name", "")).toThrow(/not a valid organisation name/);
    expect(() => p.createOrganization("lumen", "")).toThrow(/taken/);
    expect(p.createOrganization("acme", "Acme Labs").displayName).toBe("Acme Labs");
    expect(() => p.createInvitation("acme", "nope", Role.READ)).toThrow(/not an email/);
    const { invitation, link } = p.createInvitation("acme", "Ana@Example.com", Role.WRITE);
    expect(invitation.email).toBe("ana@example.com");
    expect(link).toMatch(/\/login\/invite\/[0-9a-f]{64}$/);
    expect(p.invitations("acme")).toHaveLength(1);
    p.revokeInvitation("acme", invitation.id);
    expect(p.invitations("acme")).toHaveLength(0);
  });
});
