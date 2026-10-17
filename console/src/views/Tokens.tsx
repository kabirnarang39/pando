import { useConnection } from "../connection";
import { Role } from "../gen/pando_platform_pb";
import { useTokens } from "../platform-data";
import { ago, Author, ErrorState, Icon, Skeleton, useNow } from "../ui";

const roles: Record<number, string> = { [Role.READ]: "Read", [Role.TRIAGE]: "Triage", [Role.WRITE]: "Write", [Role.MAINTAIN]: "Maintain", [Role.ADMIN]: "Admin" };

/** Your tokens, and the agent tokens you issued: what each may do and when it was last used. */
export function TokensView() {
  const tokens = useTokens();
  const { target } = useConnection();
  const now = Math.max(useNow(), Date.now());
  return (
    <div className="page page-narrow">
      <div className="page-head">
        <div>
          <h1>Tokens</h1>
          <p>A token lets a tool or an agent act for you. Give an agent its own token that acts as the agent, limited to the repositories it works in.</p>
        </div>
        <div className="actions">
          <button type="button" className="button" data-primary="" disabled={target.kind === "demo"} title={target.kind === "demo" ? "Connect your server to issue tokens" : undefined}>
            <Icon name="plus" size={15} /> New token
          </button>
        </div>
      </div>
      <div className="panel">
        {tokens.isPending ? (
          <Skeleton />
        ) : tokens.error ? (
          <ErrorState error={tokens.error} />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Acts as</th>
                <th>Up to</th>
                <th className="hide-small">Expires</th>
                <th>Last used</th>
              </tr>
            </thead>
            <tbody>
              {tokens.data.map((t) => (
                <tr key={t.id}>
                  <td>
                    <strong>{t.name}</strong>
                    <div className="muted" style={{ fontSize: "var(--t-xs)" }}>
                      {t.repositories.length ? t.repositories.join(", ") : "All your repositories"}
                    </div>
                  </td>
                  <td>{t.actsAs ? <Author author={t.actsAs} /> : <span className="muted">You</span>}</td>
                  <td>{roles[t.maxRole]}</td>
                  <td className="muted hide-small">in {Math.round((Number(t.expiresAtMs) - now) / 86_400_000)} days</td>
                  <td className="muted">{ago(Number(t.lastUsedAtMs), now)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
