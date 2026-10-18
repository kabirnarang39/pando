import { useState } from "react";
import { Role } from "../gen/pando_platform_pb";
import { useCreateInvitation, useInvitations, useRevokeInvitation } from "../platform-data";
import { Time, useNow } from "../ui";
import { RoleSelect, roleNames } from "./Access";

/** A value to copy, with a button that copies it. */
export function CopyValue({ label, value, multiline }: { label: string; value: string; multiline?: boolean }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="token-secret-row">
      {multiline ? (
        <pre className="mono copy-block" aria-label={label}>
          {value}
        </pre>
      ) : (
        <code className="mono" aria-label={label}>
          {value}
        </code>
      )}
      <button
        type="button"
        className="button button-quiet"
        onClick={() => void navigator.clipboard?.writeText(value).then(() => setCopied(true), () => undefined)}
      >
        {copied ? "Copied" : `Copy ${label.toLowerCase()}`}
      </button>
    </div>
  );
}

/** Invite someone by email: the link is emailed if the server sends mail, and shown to copy either way. */
export function InviteForm({ org }: { org: string }) {
  const create = useCreateInvitation(org);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState(Role.WRITE);
  const [sent, setSent] = useState<{ email: string; link: string; emailed: boolean } | null>(null);
  return (
    <>
      <form
        className="add-principal"
        aria-label="Invite someone"
        onSubmit={(e) => {
          e.preventDefault();
          const to = email.trim();
          if (to) create.mutate({ email: to, role }, { onSuccess: (r) => (setSent({ email: to, link: r.link, emailed: r.emailed }), setEmail("")) });
        }}
      >
        <input className="input" type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder="teammate@example.com" aria-label="Email address to invite" autoComplete="off" required />
        <RoleSelect value={role} onChange={setRole} label="Role for the invited person" />
        <button type="submit" className="button" data-primary="" disabled={!email.trim() || create.isPending}>
          {create.isPending ? "Inviting…" : "Invite"}
        </button>
        {create.error && (
          <p className="status" data-kind="error" role="alert">
            {(create.error as Error).message}
          </p>
        )}
      </form>
      {sent && (
        <div className="token-secret" role="status" style={{ margin: 16 }}>
          <p>
            {sent.emailed ? (
              <>
                We emailed an invitation to <strong>{sent.email}</strong>. You can also send them this link yourself.
              </>
            ) : (
              <>
                Send this link to <strong>{sent.email}</strong> (this server doesn't send email). It works once, for 7 days.
              </>
            )}
          </p>
          <CopyValue label="Invitation link" value={sent.link} />
        </div>
      )}
    </>
  );
}

/** Invitations not yet accepted, each revocable. */
export function InvitationList({ org }: { org: string }) {
  const list = useInvitations(org);
  const revoke = useRevokeInvitation(org);
  const now = Math.max(useNow(), Date.now());
  if (!list.data?.length) return null;
  return (
    <table className="table" aria-label="Pending invitations">
      <thead>
        <tr>
          <th>Invited</th>
          <th>Role</th>
          <th className="hide-small">Sent</th>
          <th>
            <span className="sr-only">Revoke</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {list.data.map((i) => (
          <tr key={i.id}>
            <td>
              {i.email} {Number(i.expiresAtMs) <= now && <span className="muted">(expired)</span>}
            </td>
            <td>{roleNames[i.role]}</td>
            <td className="muted hide-small">
              <Time ms={Number(i.createdAtMs)} />
            </td>
            <td className="cell-action">
              <button type="button" className="button button-quiet" aria-label={`Revoke the invitation for ${i.email}`} disabled={revoke.isPending} onClick={() => revoke.mutate(i.id)}>
                Revoke
              </button>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
