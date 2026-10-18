import { useState } from "react";
import { explain } from "../../api/client";
import { AddPrincipal, RoleSelect, roleNames } from "../../components/Access";
import { ProtectionForm } from "../../components/ProtectionForm";
import { Webhooks } from "../../components/Webhooks";
import { useConnection } from "../../connection";
import { Role } from "../../gen/pando_platform_pb";
import { useDeleteProtection, useGrants, useProtections, useRepository, useSetGrant } from "../../platform-data";
import { useRepo } from "../../repo";
import { Author, Empty, ErrorState, Icon, Skeleton } from "../../ui";

/** Who may do what, and which branches need review before anything lands on them. */
export function RepoSettingsView() {
  const { key } = useRepo();
  const { target } = useConnection();
  const protections = useProtections(key);
  const grants = useGrants(key);
  const repo = useRepository(key);
  const setGrant = useSetGrant(key);
  const admin = (repo.data?.viewerRole ?? Role.UNSPECIFIED) >= Role.ADMIN;
  const removeProtection = useDeleteProtection(key);
  // The branch whose protection is being edited; "" for a new one.
  const [editing, setEditing] = useState<string | null>(null);
  return (
    <div className="page page-narrow">
      {target.kind === "demo" && <p className="status" data-kind="info">In the demo, changes here stay in this browser tab.</p>}
      <section className="panel" aria-labelledby="protect-title" style={{ margin: "20px 0" }}>
        <div className="panel-head">
          <Icon name="shield" />
          <h2 id="protect-title">Protected branches</h2>
          {admin && editing === null && (
            <button type="button" className="button" style={{ marginLeft: "auto" }} onClick={() => setEditing("")}>
              Protect a branch
            </button>
          )}
        </div>
        {editing === "" && <ProtectionForm repo={key} onDone={() => setEditing(null)} />}
        {protections.isPending ? (
          <Skeleton />
        ) : protections.error ? (
          <ErrorState error={protections.error} />
        ) : !protections.data?.length ? (
          <Empty title="No branch is protected">Anyone with write access can land on any branch.</Empty>
        ) : (
          <ul className="list">
            {protections.data.map((p) => (
              <li key={p.pattern} className="row" style={{ gridTemplateColumns: "auto minmax(0,1fr) auto", alignItems: "start" }}>
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
                  {editing === p.pattern && <ProtectionForm repo={key} existing={p} onDone={() => setEditing(null)} />}
                </div>
                {admin && editing !== p.pattern && (
                  <span className="confirm">
                    <button type="button" className="button button-quiet" onClick={() => setEditing(p.pattern)} aria-label={`Edit protection of ${p.pattern}`}>
                      Edit
                    </button>
                    <button type="button" className="button button-quiet" disabled={removeProtection.isPending} onClick={() => removeProtection.mutate(p.pattern)} aria-label={`Stop protecting ${p.pattern}`}>
                      Remove
                    </button>
                  </span>
                )}
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
        <p className="panel-note muted">
          Members of {key.split("/")[0]} have their organisation role here too. Add people, agents or teams (<span className="mono">org/team</span>) for more.
        </p>
        {grants.isPending ? (
          <Skeleton />
        ) : grants.error ? (
          <ErrorState error={grants.error} />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Who</th>
                <th>Role</th>
                {admin && (
                  <th>
                    <span className="sr-only">Remove</span>
                  </th>
                )}
              </tr>
            </thead>
            <tbody>
              {grants.data?.length === 0 && (
                <tr>
                  <td colSpan={3} className="muted">
                    No one beyond the organisation's members.
                  </td>
                </tr>
              )}
              {grants.data?.map((g) => (
                <tr key={g.principal}>
                  <td>{g.principal.includes("/") && !g.principal.startsWith("agent/") && !g.principal.startsWith("bot/") ? <span className="mono">{g.principal}</span> : <Author author={g.principal} />}</td>
                  <td>
                    {admin ? (
                      <RoleSelect value={g.role} label={`Role for ${g.principal}`} disabled={setGrant.isPending} onChange={(role) => setGrant.mutate({ principal: g.principal, role })} />
                    ) : (
                      (roleNames[g.role] ?? "None")
                    )}
                  </td>
                  {admin && (
                    <td className="cell-action">
                      <button type="button" className="button button-quiet" onClick={() => setGrant.mutate({ principal: g.principal, role: Role.UNSPECIFIED })} disabled={setGrant.isPending}>
                        Remove
                      </button>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {admin && (
          <AddPrincipal
            label="Person, agent or team"
            placeholder="mira, agent/perf-05 or lumen/platform"
            pending={setGrant.isPending}
            error={setGrant.error ? explain(setGrant.error) : null}
            onAdd={(principal, role, done) => setGrant.mutate({ principal, role }, { onSuccess: done })}
          />
        )}
      </section>
      {admin && <Webhooks repo={key} />}
    </div>
  );
}
