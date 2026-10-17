import { useConnection } from "../../connection";
import { Role } from "../../gen/pando_platform_pb";
import { useGrants, useProtections } from "../../platform-data";
import { useRepo } from "../../repo";
import { Author, Empty, ErrorState, Icon, Skeleton } from "../../ui";

const roles: Record<number, string> = {
  [Role.READ]: "Read",
  [Role.TRIAGE]: "Triage",
  [Role.WRITE]: "Write",
  [Role.MAINTAIN]: "Maintain",
  [Role.ADMIN]: "Admin",
};

/** Who may do what, and which branches need review before anything lands on them. */
export function RepoSettingsView() {
  const { key } = useRepo();
  const { target } = useConnection();
  const protections = useProtections(key);
  const grants = useGrants(key);
  return (
    <div className="page page-narrow">
      {target.kind === "demo" && <p className="status" data-kind="info">The demo shows settings read-only. On your server, admins change them here.</p>}
      <section className="panel" aria-labelledby="protect-title" style={{ margin: "20px 0" }}>
        <div className="panel-head">
          <Icon name="shield" />
          <h2 id="protect-title">Protected branches</h2>
        </div>
        {protections.isPending ? (
          <Skeleton />
        ) : protections.error ? (
          <ErrorState error={protections.error} />
        ) : !protections.data?.length ? (
          <Empty title="No branch is protected">Anyone with write access can land on any branch.</Empty>
        ) : (
          <ul className="list">
            {protections.data.map((p) => (
              <li key={p.pattern} className="row" style={{ gridTemplateColumns: "auto minmax(0,1fr)" }}>
                <span className="branch">{p.pattern}</span>
                <div className="row-main">
                  <div className="row-meta" style={{ fontSize: "var(--t-sm)", color: "var(--ink-2)" }}>
                    <span>
                      {p.requiredApprovals} {p.requiredApprovals === 1 ? "approval" : "approvals"} required
                    </span>
                    {p.requiredChecks.length > 0 && <span>Checks must pass: {p.requiredChecks.join(", ")}</span>}
                    {p.dismissStale && <span>A new version needs new approvals</span>}
                    <span>{p.agentsLandDirectly ? "Agents land directly" : "An agent's land opens a change request"}</span>
                    {p.bypass.length > 0 && <span>May land without review: {p.bypass.join(", ")}</span>}
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="panel" aria-labelledby="access-title">
        <div className="panel-head">
          <Icon name="users" />
          <h2 id="access-title">Access</h2>
        </div>
        {grants.isPending ? (
          <Skeleton />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Who</th>
                <th>Role</th>
              </tr>
            </thead>
            <tbody>
              {grants.data?.map((g) => (
                <tr key={g.principal}>
                  <td>{g.principal.includes("/") && !g.principal.startsWith("agent/") ? <span className="mono">{g.principal}</span> : <Author author={g.principal} />}</td>
                  <td>{roles[g.role] ?? "None"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
