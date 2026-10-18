import { Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { Code, ConnectError } from "@connectrpc/connect";
import { Role } from "../gen/pando_platform_pb";
import { CopyValue, InviteForm } from "../components/Invitations";
import { useConnection } from "../connection";
import { useCreateOrganization, useCreateToken, useViewer } from "../platform-data";

type Step = "org" | "invite" | "agent";
const steps: [Step, string][] = [
  ["org", "Name your organisation"],
  ["invite", "Invite teammates"],
  ["agent", "Connect an agent"],
];

/** What went wrong creating an organisation, in plain words. */
function orgError(e: unknown, name: string): string {
  const message = e instanceof ConnectError ? e.rawMessage : String(e);
  if (!(e instanceof ConnectError)) return message;
  switch (e.code) {
    case Code.AlreadyExists:
      return `“${name}” is already taken. Try another name.`;
    case Code.InvalidArgument:
      return `${message}. Names use letters, digits, “.”, “_” and “-”, like acme or acme-labs.`;
    case Code.ResourceExhausted:
      return `${message}. Ask a site administrator if you need more.`;
    case Code.PermissionDenied:
      return `You can't create organisations on this server: ${message}.`;
    default:
      return message;
  }
}

/**
 * Setting up a new organisation, start to finish: name it, invite teammates (optional), and
 * connect an agent with a token and the MCP address. Every step says what happens next.
 */
export function WelcomeView() {
  const viewer = useViewer();
  const first = viewer.data?.platform && viewer.data.organizations.length === 0;
  const [step, setStep] = useState<Step>("org");
  const [org, setOrg] = useState("");
  const at = steps.findIndex(([s]) => s === step);
  return (
    <div className="page page-narrow">
      <div>
        <h1>{first ? "Welcome to Pando" : "New organisation"}</h1>
        <p className="lede">
          {first
            ? "Three short steps and your team and agents can start landing code. You can skip the last two and come back later."
            : "An organisation holds repositories, people and agents. Three short steps; the last two are optional."}
        </p>
        <ol className="steps" aria-label="Steps">
          {steps.map(([s, label], i) => (
            <li key={s} aria-current={s === step ? "step" : undefined} data-done={i < at || undefined}>
              {i + 1}. {label}
            </li>
          ))}
        </ol>
        <section className="panel panel-body" aria-labelledby="step-title">
          {step === "org" && <NameStep onDone={(login) => (setOrg(login), setStep("invite"))} />}
          {step === "invite" && <InviteStep org={org} onNext={() => setStep("agent")} />}
          {step === "agent" && <AgentStep org={org} />}
        </section>
      </div>
    </div>
  );
}

function NameStep({ onDone }: { onDone(login: string): void }) {
  const create = useCreateOrganization();
  const [login, setLogin] = useState("");
  const [name, setName] = useState("");
  return (
    <form
      className="stack"
      onSubmit={(e) => {
        e.preventDefault();
        const l = login.trim();
        if (l) create.mutate({ login: l, displayName: name.trim() }, { onSuccess: () => onDone(l) });
      }}
    >
      <h2 id="step-title">Name your organisation</h2>
      <label className="field">
        <span>Short name, used in addresses</span>
        <input className="input" value={login} onChange={(e) => setLogin(e.target.value)} placeholder="acme" required autoFocus autoComplete="off" spellCheck={false} aria-describedby="org-hint" />
      </label>
      <p id="org-hint" className="muted" style={{ margin: 0 }}>
        Repositories live at <span className="mono">{login.trim() || "acme"}/REPOSITORY</span>. Letters, digits, “.”, “_” and “-”.
      </p>
      <label className="field">
        <span>Display name (optional)</span>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Acme Labs" autoComplete="organization" />
      </label>
      {create.error && (
        <p className="status" data-kind="error" role="alert">
          {orgError(create.error, login.trim())}
        </p>
      )}
      <div className="row-actions">
        <button type="submit" className="button" data-primary="" disabled={!login.trim() || create.isPending}>
          {create.isPending ? "Creating…" : "Create organisation"}
        </button>
      </div>
    </form>
  );
}

function InviteStep({ org, onNext }: { org: string; onNext(): void }) {
  return (
    <div className="stack">
      <h2 id="step-title">Invite teammates</h2>
      <p style={{ margin: 0 }}>
        <strong>{org}</strong> is ready. Invite people by email: each gets a link that works once, for 7 days. You can do this later from the organisation's People tab.
      </p>
      <InviteForm org={org} />
      <div className="row-actions">
        <button type="button" className="button" data-primary="" onClick={onNext}>
          Continue
        </button>
        <button type="button" className="button button-quiet" onClick={onNext}>
          Skip for now
        </button>
      </div>
    </div>
  );
}

function AgentStep({ org }: { org: string }) {
  const { target } = useConnection();
  const navigate = useNavigate();
  const create = useCreateToken();
  const [who, setWho] = useState<"agent" | "me">("agent");
  const [agent, setAgent] = useState("claude");
  const server = target.kind === "server" ? target.url.replace(/\/+$/, "") : "https://pando.example.com";
  const [mcp, setMcp] = useState(`${server}/mcp`);
  const [token, setToken] = useState<string | null>(null);
  const actsAs = who === "agent" ? `agent/${agent.trim()}` : "";
  const snippet = token && `claude mcp add --transport http pando ${mcp} \\\n  --header "Authorization: Bearer ${token}"`;
  return (
    <div className="stack">
      <h2 id="step-title">Connect an agent</h2>
      <p style={{ margin: 0 }}>
        Agents reach Pando over MCP with a URL and a token. Give the agent a name of its own, so its work is easy to tell apart from yours; it answers to you.
      </p>
      {!token ? (
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            create.mutate(
              { name: who === "agent" ? `${actsAs} (${org})` : `MCP (${org})`, actsAs, repositories: [], maxRole: Role.WRITE, days: 90 },
              { onSuccess: (r) => setToken(r.secret) },
            );
          }}
        >
          <fieldset className="events">
            <legend>The token acts as</legend>
            <label className="check">
              <input type="radio" name="who" checked={who === "agent"} onChange={() => setWho("agent")} /> An agent
            </label>
            <label className="check">
              <input type="radio" name="who" checked={who === "me"} onChange={() => setWho("me")} /> Me
            </label>
          </fieldset>
          {who === "agent" && (
            <label className="field">
              <span>Agent name</span>
              <input className="input" value={agent} onChange={(e) => setAgent(e.target.value)} required pattern="[A-Za-z0-9][A-Za-z0-9._\-]*" autoComplete="off" spellCheck={false} />
            </label>
          )}
          {create.error && (
            <p className="status" data-kind="error" role="alert">
              {(create.error as Error).message}
            </p>
          )}
          <div className="row-actions">
            <button type="submit" className="button" data-primary="" disabled={create.isPending || (who === "agent" && !agent.trim())}>
              {create.isPending ? "Creating…" : "Create token"}
            </button>
            <Link to="/$owner" params={{ owner: org }} className="button button-quiet">
              Skip for now
            </Link>
          </div>
        </form>
      ) : (
        <>
          <div className="token-secret" role="status">
            <p>
              <strong>Copy the token now.</strong> It isn't shown again. It can write to {org}'s repositories for 90 days; revoke it any time on the Tokens page.
            </p>
            <CopyValue label="Token" value={token} />
          </div>
          <label className="field">
            <span>MCP address</span>
            <input className="input mono" type="url" value={mcp} onChange={(e) => setMcp(e.target.value)} aria-describedby="mcp-hint" />
          </label>
          <p id="mcp-hint" className="muted" style={{ margin: 0 }}>
            Where this server's <span className="mono">pando mcp --http</span> gateway answers. Change it if your administrator gave you another address.
          </p>
          <div>
            <p style={{ margin: "0 0 6px" }}>Paste this into a terminal to add it to Claude Code. Cursor, Claude Desktop and other MCP clients take the same address and header.</p>
            <CopyValue label="Command" value={snippet!} multiline />
          </div>
          <div className="row-actions">
            <button type="button" className="button" data-primary="" onClick={() => navigate({ to: "/$owner", params: { owner: org } })}>
              Go to {org}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
