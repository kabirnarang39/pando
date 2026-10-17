import { useState } from "react";
import type { Webhook } from "../gen/pando_platform_pb";
import { useCreateWebhook, useDeleteWebhook, useDeliveries, usePingWebhook, useWebhooks } from "../platform-data";
import { ago, Icon, Skeleton, useNow } from "../ui";

const EVENTS = [
  ["push", "Pushes and lands"],
  ["change_request", "Change requests opened, landed, closed"],
  ["review", "Reviews"],
  ["comment", "Comments"],
  ["check", "Checks reported"],
] as const;

const status = (s: number) => (s === 0 ? "Not sent yet" : s === -1 ? "No answer" : `HTTP ${s}`);

/** Webhooks: call a URL when something happens here. Administrators only. */
export function Webhooks({ repo }: { repo: string }) {
  const hooks = useWebhooks(repo, true);
  const create = useCreateWebhook(repo);
  const remove = useDeleteWebhook(repo);
  const ping = usePingWebhook(repo);
  const [adding, setAdding] = useState(false);
  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<string[]>([]);
  const [secret, setSecret] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  return (
    <section className="panel" aria-labelledby="hooks-title" style={{ marginTop: 20 }}>
      <div className="panel-head">
        <Icon name="external" />
        <h2 id="hooks-title">Webhooks</h2>
        {!adding && (
          <button type="button" className="button" style={{ marginLeft: "auto" }} onClick={() => (setAdding(true), setSecret(null))}>
            Add webhook
          </button>
        )}
      </div>
      <p className="panel-note muted">
        Pando posts JSON to these URLs when something happens, signed with <span className="mono">X-Pando-Signature-256</span> the way GitHub signs, so CI, chat and trackers can react.
      </p>
      {adding && (
        <form
          className="protection-form"
          aria-label="Add webhook"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate({ url: url.trim(), events }, { onSuccess: (r) => (setAdding(false), setUrl(""), setEvents([]), setSecret(r.secret)) });
          }}
        >
          <label>
            <span>Payload URL</span>
            <input className="input" type="url" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://ci.example.com/hooks/pando" required autoFocus />
          </label>
          <fieldset className="events">
            <legend>Events (none checked: every event)</legend>
            {EVENTS.map(([id, label]) => (
              <label key={id} className="check">
                <input type="checkbox" checked={events.includes(id)} onChange={(e) => setEvents((v) => (e.target.checked ? [...v, id] : v.filter((x) => x !== id)))} /> {label}
              </label>
            ))}
          </fieldset>
          {create.error && (
            <p className="status" data-kind="error" role="alert">
              {(create.error as Error).message}
            </p>
          )}
          <div className="token-form-actions">
            <button type="button" className="button button-quiet" onClick={() => setAdding(false)}>
              Cancel
            </button>
            <button type="submit" className="button" data-primary="" disabled={!url.trim() || create.isPending}>
              {create.isPending ? "Adding…" : "Add webhook"}
            </button>
          </div>
        </form>
      )}
      {secret && (
        <div className="token-secret" role="status" style={{ margin: 16 }}>
          <p>
            <strong>Copy the signing secret now.</strong> It isn't shown again. Check each request's <span className="mono">X-Pando-Signature-256</span> against it.
          </p>
          <div className="token-secret-row">
            <code className="mono" aria-label="Signing secret">
              {secret}
            </code>
            <button type="button" className="button button-quiet" onClick={() => setSecret(null)}>
              Done
            </button>
          </div>
        </div>
      )}
      {hooks.isPending ? (
        <Skeleton />
      ) : !hooks.data?.length ? (
        !adding && <p className="panel-note muted">No webhooks yet.</p>
      ) : (
        <ul className="list">
          {hooks.data.map((h) => (
            <li key={h.id}>
              <div className="row" style={{ gridTemplateColumns: "minmax(0,1fr) auto" }}>
                <div className="row-main">
                  <div className="row-title mono">{h.url}</div>
                  <div className="row-meta">
                    <span>{h.events.length ? h.events.join(", ") : "Every event"}</span>
                    <span data-kind={h.lastStatus >= 200 && h.lastStatus < 300 ? "ok" : h.lastStatus === 0 ? undefined : "bad"}>Last delivery: {status(h.lastStatus)}</span>
                  </div>
                </div>
                <span className="confirm">
                  <button type="button" className="button button-quiet" disabled={ping.isPending} onClick={() => ping.mutate(h.id, { onSuccess: () => setOpen(h.id) })}>
                    Ping
                  </button>
                  <button type="button" className="button button-quiet" aria-expanded={open === h.id} onClick={() => setOpen(open === h.id ? null : h.id)}>
                    Deliveries
                  </button>
                  <button type="button" className="button button-quiet" aria-label={`Remove webhook ${h.url}`} onClick={() => remove.mutate(h.id)}>
                    Remove
                  </button>
                </span>
              </div>
              {open === h.id && <Deliveries hook={h} />}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Deliveries({ hook }: { hook: Webhook }) {
  const d = useDeliveries(hook.id);
  const now = Math.max(useNow(), Date.now());
  if (d.isPending) return <Skeleton rows={2} />;
  if (!d.data?.length) return <p className="panel-note muted">Nothing sent yet. Ping it to check the URL.</p>;
  return (
    <table className="table deliveries" aria-label={`Deliveries to ${hook.url}`}>
      <tbody>
        {d.data.map((x) => (
          <tr key={x.id}>
            <td className="mono">{x.event}</td>
            <td>{x.delivered ? "Delivered" : x.attempts >= 8 ? "Gave up" : x.attempts ? "Retrying" : "Queued"}</td>
            <td>
              {status(x.status)}
              {x.error && <span className="muted"> · {x.error}</span>}
            </td>
            <td className="muted">{ago(Number(x.createdAtMs), now)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
