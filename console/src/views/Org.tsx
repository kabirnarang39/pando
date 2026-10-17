import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { AccountKind, Role } from "../gen/pando_platform_pb";
import { AddPrincipal, RoleSelect } from "../components/Access";
import { useMembers, useOrganization, useRepositories, useSetMember, useTeams } from "../platform-data";
import { ago, Author, LoadError, Skeleton, useNow } from "../ui";

const roles: Record<number, string> = { [Role.READ]: "Read", [Role.TRIAGE]: "Triage", [Role.WRITE]: "Write", [Role.MAINTAIN]: "Maintain", [Role.ADMIN]: "Admin" };

export function OrgView() {
  const { owner } = useParams({ from: "/$owner" });
  const { tab = "repositories" } = useSearch({ from: "/$owner" });
  const navigate = useNavigate({ from: "/$owner" });
  const org = useOrganization(owner);
  const repos = useRepositories(owner);
  const members = useMembers(owner);
  const teams = useTeams(owner);
  const setMember = useSetMember(owner);
  const now = Math.max(useNow(), Date.now());
  const admin = (org.data?.viewerRole ?? Role.UNSPECIFIED) >= Role.ADMIN;
  if (org.error) return <LoadError error={org.error} what={`organisation called “${owner}”`} title={`${owner} didn't load`} retry={() => org.refetch()} />;
  const people = members.data?.filter((m) => m.account?.kind === AccountKind.PERSON) ?? [];
  const agents = members.data?.filter((m) => m.account?.kind === AccountKind.AGENT) ?? [];
  const tabs = [
    ["repositories", "Repositories", repos.data?.length],
    ["people", "People", people.length],
    ["agents", "Agents", agents.length],
    ["teams", "Teams", teams.data?.length],
  ] as const;

  return (
    <div className="page">
      <div className="page-head">
        <span className="org-avatar" style={{ width: 52, height: 52, borderRadius: 14, fontSize: 22 }} aria-hidden="true">
          {org.data?.displayName.slice(0, 1) ?? ""}
        </span>
        <div>
          <h1>{org.data?.displayName ?? owner}</h1>
          <p>{org.data?.description}</p>
        </div>
      </div>
      <div className="segmented" role="group" aria-label="Organisation" style={{ marginBottom: 16 }}>
        {tabs.map(([id, label, n]) => (
          <button key={id} type="button" aria-pressed={tab === id} onClick={() => navigate({ search: { tab: id === "repositories" ? undefined : id }, replace: true })}>
            {label} {n !== undefined && <span className="count">{n}</span>}
          </button>
        ))}
      </div>
      <div className="panel">
        {tab === "repositories" &&
          (repos.isPending ? (
            <Skeleton />
          ) : (
            <ul className="list">
              {repos.data?.map((r) => (
                <li key={r.name}>
                  <Link to="/$owner/$repo" params={{ owner: r.owner, repo: r.name }} className="row" style={{ gridTemplateColumns: "auto minmax(0,1fr) auto" }}>
                    <span className="pulse" data-live={now - Number(r.tipAtMs) < 90_000 || undefined} />
                    <div className="row-main">
                      <div className="row-title">{r.name}</div>
                      <div className="row-meta">
                        <span>{r.description}</span>
                      </div>
                    </div>
                    <div className="row-side">
                      <span className="num">{r.lands24h} landed today</span>
                      <span className="num">{r.openChangeRequests} open</span>
                      <span className="hide-small">{ago(Number(r.tipAtMs), now)}</span>
                    </div>
                  </Link>
                </li>
              ))}
            </ul>
          ))}
        {(tab === "people" || tab === "agents") && (
          <table className="table">
            <thead>
              <tr>
                <th>{tab === "people" ? "Person" : "Agent"}</th>
                {tab === "agents" && <th>Answers to</th>}
                <th>Role</th>
                <th className="hide-small">Teams</th>
                {admin && (
                  <th>
                    <span className="sr-only">Remove</span>
                  </th>
                )}
              </tr>
            </thead>
            <tbody>
              {(tab === "people" ? people : agents).map((m) => (
                <tr key={m.account!.login}>
                  <td>
                    <span className="row-side" style={{ gap: 10, color: "inherit", fontSize: "var(--t-sm)" }}>
                      <Author author={m.account!.login} />
                      {tab === "people" && <span className="muted">{m.account!.displayName}</span>}
                    </span>
                  </td>
                  {tab === "agents" && (
                    <td>
                      <Author author={m.account!.owner} />
                    </td>
                  )}
                  <td>
                    {admin ? (
                      <RoleSelect value={m.role} label={`Role for ${m.account!.login}`} disabled={setMember.isPending} onChange={(role) => setMember.mutate({ login: m.account!.login, role })} />
                    ) : (
                      roles[m.role]
                    )}
                  </td>
                  <td className="muted hide-small">{m.teams.join(", ")}</td>
                  {admin && (
                    <td className="cell-action">
                      <button type="button" className="button button-quiet" disabled={setMember.isPending} onClick={() => setMember.mutate({ login: m.account!.login, role: Role.UNSPECIFIED })}>
                        Remove
                      </button>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {tab === "people" && admin && (
          <AddPrincipal
            label="Login of the person to add"
            placeholder="Login or email, e.g. mira@lumen.dev"
            initial={Role.READ}
            pending={setMember.isPending}
            error={setMember.error ? (setMember.error as Error).message : null}
            onAdd={(login, role, done) => setMember.mutate({ login, role }, { onSuccess: done })}
          />
        )}
        {tab === "agents" && <p className="panel-note muted">An agent works with the role of the person it answers to. Issue it a token on the Tokens page; list it here only to give it a role of its own.</p>}
        {tab === "teams" && (
          <ul className="list">
            {teams.data?.map((t) => (
              <li key={t.slug} className="row" style={{ gridTemplateColumns: "minmax(0,1fr) auto" }}>
                <div className="row-main">
                  <div className="row-title">{t.name}</div>
                  <div className="row-meta">{t.description}</div>
                </div>
                <span className="count">{t.members.length}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
