// An organisation's plan and what it uses this month (ADR-035). Its own chunk: only
// administrators of servers with plans ever load it.

import { useCapabilities, useUsage } from "../platform-data";
import { Author, ErrorState, formatBytes, Skeleton, Time } from "../ui";

/** One limit: "4 of 10" with a bar, or "4 of unlimited" without one. */
function Limit({ label, used, max, format = String, atLimit }: { label: string; used: number; max?: bigint; format?: (n: number) => string; atLimit: string }) {
  const limit = max === undefined ? undefined : Number(max);
  const id = `limit-${label.toLowerCase().replace(/\W+/g, "-")}`;
  return (
    <div className="usage-card">
      <h3 id={id}>{label}</h3>
      <p className="num">
        {format(used)} <span className="muted">of {limit === undefined ? "unlimited" : format(limit)}</span>
      </p>
      {limit !== undefined && (
        <>
          <meter aria-labelledby={id} min={0} max={Math.max(limit, 1)} high={limit * 0.8} optimum={0} value={Math.min(used, limit)} />
          <p className="muted usage-note">{atLimit}</p>
        </>
      )}
    </div>
  );
}

export function UsagePanel({ org }: { org: string }) {
  const usage = useUsage(org, true);
  const billingUrl = useCapabilities().data?.billingUrl ?? "";
  if (usage.error) return <ErrorState error={usage.error} retry={() => usage.refetch()} title="Usage didn't load" />;
  if (!usage.data) return <Skeleton rows={4} />;
  const u = usage.data;
  const l = u.limits;
  const changePlan = billingUrl ? `${billingUrl}${billingUrl.includes("?") ? "&" : "?"}org=${encodeURIComponent(org)}` : "";
  const agents = l?.activeAgents === undefined ? "" : String(l.activeAgents);
  return (
    <div className="usage">
      <div className="usage-head">
        <div>
          <h2>Usage &amp; plan</h2>
          {l?.planLabel && <p>Plan: {l.planLabel}</p>}
          {!u.enforced && <p className="muted">This server doesn't enforce limits.</p>}
        </div>
        {changePlan && (
          <a className="button" data-primary="" href={changePlan} rel="noopener">
            Change plan
          </a>
        )}
      </div>
      {changePlan && <p className="muted usage-note">Change plan opens the billing site, where you choose people and agents and see the price.</p>}
      <div className="usage-grid">
        <Limit label="People" used={u.seats} max={l?.seats} atLimit="At the limit, nobody new can join until you change plan or remove someone." />
        <Limit
          label={`Active agents in ${u.month}`}
          used={u.activeAgents.length}
          max={l?.activeAgents}
          atLimit={`At ${agents} active agents, new agents can't land until next month or until you change plan. Agents already active keep landing.`}
        />
        <Limit label="Storage" used={Number(u.storageBytes)} max={l?.storageBytes} format={formatBytes} atLimit="Over the limit, nothing new can be pushed; everything can still be read." />
        <Limit label="Private repositories" used={u.privateRepos} max={l?.privateRepos} atLimit="At the limit, new repositories must be public." />
      </div>
      <h3 className="usage-sub">Agents active this month</h3>
      {u.activeAgents.length === 0 ? (
        <p className="muted usage-note">No agent has landed a change this month. An agent counts once it lands its first change in a calendar month (UTC).</p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>Agent</th>
              <th>First landed</th>
            </tr>
          </thead>
          <tbody>
            {u.activeAgents.map((a) => (
              <tr key={a.login}>
                <td>
                  <Author author={a.login} />
                </td>
                <td>
                  <Time ms={Number(a.firstAtMs)} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
