import { useState } from "react";
import { RoleSelect, roleNames } from "../components/Access";
import { Role } from "../gen/pando_platform_pb";
import { useCreateToken, useRevokeToken, useTokens } from "../platform-data";
import { ago, Author, ErrorState, Icon, Skeleton, useNow } from "../ui";

/** Your tokens, and the agent tokens you issued: what each may do and when it was last used. */
export function TokensView() {
  const tokens = useTokens();
  const revoke = useRevokeToken();
  const [creating, setCreating] = useState(false);
  const [secret, setSecret] = useState<{ id: string; value: string } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const now = Math.max(useNow(), Date.now());
  return (
    <div className="page page-narrow">
      <div className="page-head">
        <div>
          <h1>Tokens</h1>
          <p>A token lets a tool or an agent act for you. Give an agent its own token that acts as the agent, limited to the repositories it works in.</p>
        </div>
        <div className="actions">
          {!creating && (
            <button type="button" className="button" data-primary="" onClick={() => (setCreating(true), setSecret(null))}>
              <Icon name="plus" size={15} /> New token
            </button>
          )}
        </div>
      </div>
      {creating && (
        <NewToken
          onCancel={() => setCreating(false)}
          onCreated={(id, value) => {
            setCreating(false);
            setSecret({ id, value });
          }}
        />
      )}
      {secret && <Secret value={secret.value} onDone={() => setSecret(null)} />}
      <div className="panel">
        {tokens.isPending ? (
          <Skeleton />
        ) : tokens.error ? (
          <ErrorState error={tokens.error} />
        ) : tokens.data.length === 0 ? (
          <p className="panel-note muted">No tokens yet. Create one for each tool or agent, so you can revoke one without the others.</p>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Acts as</th>
                <th>Up to</th>
                <th className="hide-small">Expires</th>
                <th className="hide-small">Last used</th>
                <th>
                  <span className="sr-only">Revoke</span>
                </th>
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
                  <td>{roleNames[t.maxRole]}</td>
                  <td className="muted hide-small">in {Math.ceil((Number(t.expiresAtMs) - now) / 86_400_000)} days</td>
                  <td className="muted hide-small">{Number(t.lastUsedAtMs) ? ago(Number(t.lastUsedAtMs), now) : "Never"}</td>
                  <td className="cell-action">
                    {confirm === t.id ? (
                      <span className="confirm">
                        <button type="button" className="button" data-danger="" disabled={revoke.isPending} onClick={() => revoke.mutate(t.id, { onSuccess: () => setConfirm(null) })}>
                          Revoke
                        </button>
                        <button type="button" className="button button-quiet" onClick={() => setConfirm(null)}>
                          Keep
                        </button>
                      </span>
                    ) : (
                      <button type="button" className="button button-quiet" onClick={() => setConfirm(t.id)} aria-label={`Revoke ${t.name}`}>
                        Revoke
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {revoke.error && (
          <p className="status" data-kind="error" role="alert">
            {(revoke.error as Error).message}
          </p>
        )}
      </div>
    </div>
  );
}

function NewToken({ onCancel, onCreated }: { onCancel(): void; onCreated(id: string, secret: string): void }) {
  const create = useCreateToken();
  const [name, setName] = useState("");
  const [actsAs, setActsAs] = useState("");
  const [repos, setRepos] = useState("");
  const [role, setRole] = useState(Role.WRITE);
  const [days, setDays] = useState(90);
  return (
    <form
      className="panel token-form"
      aria-label="New token"
      onSubmit={(e) => {
        e.preventDefault();
        create.mutate(
          { name: name.trim(), actsAs: actsAs.trim(), repositories: repos.split(/[\s,]+/).filter(Boolean), maxRole: role, days },
          { onSuccess: (r) => onCreated(r.token?.id ?? "", r.secret) },
        );
      }}
    >
      <label>
        <span>Name</span>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="What uses it, e.g. laptop or perf fleet" autoFocus required maxLength={100} />
      </label>
      <label>
        <span>Acts as</span>
        <input className="input" value={actsAs} onChange={(e) => setActsAs(e.target.value)} placeholder="You, or an agent such as agent/perf-05" spellCheck={false} />
      </label>
      <label>
        <span>Repositories</span>
        <input className="input" value={repos} onChange={(e) => setRepos(e.target.value)} placeholder="All you can reach, or lumen/fleet lumen/web" spellCheck={false} />
      </label>
      <div className="token-form-row">
        <label>
          <span>Up to</span>
          <RoleSelect value={role} onChange={setRole} label="Highest role" />
        </label>
        <label>
          <span>Expires in</span>
          <select className="input" value={days} onChange={(e) => setDays(Number(e.target.value))}>
            {[7, 30, 90, 180, 366].map((d) => (
              <option key={d} value={d}>
                {d === 366 ? "1 year" : `${d} days`}
              </option>
            ))}
          </select>
        </label>
      </div>
      {create.error && (
        <p className="status" data-kind="error" role="alert">
          {(create.error as Error).message}
        </p>
      )}
      <div className="token-form-actions">
        <button type="button" className="button button-quiet" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="button" data-primary="" disabled={!name.trim() || create.isPending}>
          {create.isPending ? "Creating…" : "Create token"}
        </button>
      </div>
    </form>
  );
}

/** The new token, shown once. */
function Secret({ value, onDone }: { value: string; onDone(): void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="panel token-secret" role="status">
      <p>
        <strong>Copy the token now.</strong> It isn't shown again; the server keeps only a hash. Use it as <span className="mono">PANDO_TOKEN</span>.
      </p>
      <div className="token-secret-row">
        <code className="mono" aria-label="New token">
          {value}
        </code>
        <button
          type="button"
          className="button"
          onClick={() =>
            navigator.clipboard.writeText(value).then(
              () => setCopied(true),
              () => setCopied(false),
            )
          }
        >
          <Icon name={copied ? "check" : "copy"} size={14} /> {copied ? "Copied" : "Copy"}
        </button>
        <button type="button" className="button button-quiet" onClick={onDone}>
          Done
        </button>
      </div>
    </div>
  );
}
