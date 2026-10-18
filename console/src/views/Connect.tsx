import { useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { Checkbox, FieldError, Form, Input, Label, Text, TextField } from "react-aria-components";
import { explain, isUnimplemented, makeApi, serverTransport } from "../api/client";
import { pendingServer, useConnection } from "../connection";
import { Icon } from "../ui";

type Check = { kind: "ok"; text: string } | { kind: "error"; text: string } | null;

/** What a server's sign-in pages offer (`GET /login/providers`). */
export type Providers = { providers: { id: string; name: string; url: string }[]; email: boolean; turnstile_site_key?: string };

const TURNSTILE = "https://challenges.cloudflare.com/turnstile/v0/api.js";

/** The origin of an address, or null if it isn't an http(s) URL. */
function originOf(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "https:" || u.protocol === "http:" ? u.origin : null;
  } catch {
    return null;
  }
}

/** The sign-in options of the server at `origin`, if it has sign-in pages. */
function useProviders(origin: string | null) {
  const [found, setFound] = useState<{ origin: string; providers: Providers } | null>(null);
  useEffect(() => {
    if (!origin) return;
    const stop = new AbortController();
    fetch(`${origin}/login/providers`, { signal: stop.signal, credentials: "omit" })
      .then((r) => (r.ok ? (r.json() as Promise<Providers>) : null))
      .then((p) => p && Array.isArray(p.providers) && setFound({ origin, providers: p }))
      .catch(() => undefined);
    return () => stop.abort();
  }, [origin]);
  return found?.origin === origin ? found.providers : null;
}

/** Sign in through the server's own pages: one button per provider, and an email link. */
function SignIn({ origin, providers, onToken }: { origin: string; providers: Providers; onToken(): void }) {
  const key = providers.turnstile_site_key;
  const widget = useRef<HTMLDivElement>(null);
  // Cloudflare's script loads only when the server asks for the check, and only once.
  useEffect(() => {
    if (!key || !providers.email) return;
    const w = window as unknown as { turnstile?: { render(el: HTMLElement, o: { sitekey: string }): void } };
    if (w.turnstile && widget.current) return w.turnstile.render(widget.current, { sitekey: key });
    if (document.querySelector(`script[src="${TURNSTILE}"]`)) return;
    const s = document.createElement("script");
    s.src = TURNSTILE;
    s.async = true;
    document.head.append(s);
  }, [key, providers.email]);
  return (
    <>
      <div className="signin-buttons">
        {providers.providers.map((p) => (
          <a key={p.id} className="button" data-primary="" href={`${origin}${p.url}?next=/`}>
            Sign in with {p.name}
          </a>
        ))}
      </div>
      {providers.email && (
        <>
          {providers.providers.length > 0 && <p className="or">or</p>}
          <form method="post" action={`${origin}/login/email`} className="stack">
            <label className="field">
              <span>Email address</span>
              <input className="input" type="email" name="email" required autoComplete="email" placeholder="you@example.com" />
            </label>
            <input type="hidden" name="next" value="/" />
            {key && <div ref={widget} className="cf-turnstile" data-sitekey={key} />}
            <button type="submit" className="button">
              Email me a sign-in link
            </button>
          </form>
        </>
      )}
      <p className="muted">
        New here? Signing in creates your account.{" "}
        <button type="button" className="link-button" onClick={onToken}>
          Use a token instead
        </button>
      </p>
    </>
  );
}

export function ConnectView() {
  const { target, connect } = useConnection();
  const navigate = useNavigate();
  const suggested = pendingServer();
  const [url, setUrl] = useState(suggested ?? (target.kind === "server" ? target.url : ""));
  const [token, setToken] = useState(target.kind === "server" ? (target.token ?? "") : "");
  const [remember, setRemember] = useState(target.kind === "server" && target.remember);
  const [busy, setBusy] = useState(false);
  const [check, setCheck] = useState<Check>(null);
  const [useToken, setUseToken] = useState(false);
  // The server that serves this console usually has the sign-in pages too.
  const origin = originOf(url.trim()) ?? location.origin;
  const providers = useProviders(origin);

  /** Ask the server who we are and what it holds before switching to it. */
  const test = async () => {
    const api = makeApi(serverTransport(url, token || null));
    const who = await api.access.whoAmI({});
    // A server that hosts many repositories answers for them by name, not for a default one.
    const viewer = await api.platform.viewer({}).catch((e) => {
      if (isUnimplemented(e)) return null;
      throw e;
    });
    if (viewer) {
      const n = viewer.organizations.length;
      return `Connected as ${who.principal}. ${n} ${n === 1 ? "organisation" : "organisations"}.`;
    }
    const refs = await api.refs.list({});
    return `Connected as ${who.principal}. ${refs.refs.length} ${refs.refs.length === 1 ? "branch" : "branches"}; ${who.fullRead ? "full read access" : `may read ${who.readPrefixes.join(", ") || "nothing"}`}.`;
  };

  const submit = async () => {
    setBusy(true);
    setCheck(null);
    try {
      const text = await test();
      setCheck({ kind: "ok", text });
      connect({ kind: "server", url: url.trim(), token: token || null, remember });
      // Drop ?server= so a reload does not ask again.
      history.replaceState(null, "", location.pathname + location.hash.replace(/^#\/connect.*/, "#/"));
      navigate({ to: "/" });
    } catch (e) {
      setCheck({ kind: "error", text: explain(e, url) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page page-narrow">
      <div>
        <h1>Connection</h1>
        <p className="lede">
          The console reads one Pando server. It runs in your browser and talks to the server directly; nothing passes through anyone
          else.
        </p>
        {suggested && (
          <p className="status" data-kind="ok" role="status">
            A link asked to connect to <strong className="mono">{suggested}</strong>. Connect only if you trust it: the console sends it your
            token.
          </p>
        )}
        <div className="choices">
          <section className="panel panel-body" aria-labelledby="server-title">
            <h2 id="server-title">{providers && !useToken ? "Sign in" : "Your server"}</h2>
            {providers && !useToken ? (
              <SignIn origin={origin} providers={providers} onToken={() => setUseToken(true)} />
            ) : (
            <>
            <p>
              Start pandod with this site allowed: <code>--web-origins {location.origin}</code>
            </p>
            <Form
              onSubmit={(e) => {
                e.preventDefault();
                void submit();
              }}
            >
              <TextField className="field" isRequired type="url" value={url} onChange={setUrl} autoFocus>
                <Label>Server address</Label>
                <Input placeholder="https://pando.example.com:7440" />
                <FieldError />
              </TextField>
              <TextField className="field" type="password" value={token} onChange={setToken}>
                <Label>Token</Label>
                <Input autoComplete="off" placeholder="Leave empty if the server allows anonymous reads" />
                <Text slot="description">Sent only to this server, as a bearer token.</Text>
              </TextField>
              <Checkbox className="check" isSelected={remember} onChange={setRemember}>
                {({ isSelected }) => (
                  <>
                    <span
                      aria-hidden="true"
                      style={{
                        display: "inline-grid",
                        placeItems: "center",
                        width: 18,
                        height: 18,
                        border: "1px solid var(--line-strong)",
                        borderRadius: 4,
                        background: isSelected ? "var(--accent)" : "var(--raised)",
                        color: "var(--accent-fg)",
                      }}
                    >
                      {isSelected && <Icon name="check" size={12} />}
                    </span>
                    Remember the token on this device
                  </>
                )}
              </Checkbox>
              <div className="row-actions">
                <button type="submit" className="button" data-primary disabled={busy || !url}>
                  {busy ? "Connecting…" : "Connect"}
                </button>
                {target.kind === "server" && <span className="muted">Connected to {target.url}</span>}
              </div>
              {check && (
                <p className="status" data-kind={check.kind} role={check.kind === "error" ? "alert" : "status"}>
                  {check.text}
                </p>
              )}
            </Form>
            </>
            )}
          </section>
          <section className="panel panel-body" aria-labelledby="demo-title">
            <h2 id="demo-title">The demo</h2>
            <p>
              A sample monorepo where eight agents and one person land changes every few seconds. It runs entirely in this tab and
              resets when you reload.
            </p>
            <button
              type="button"
              className="button"
              disabled={target.kind === "demo"}
              onClick={() => {
                connect({ kind: "demo" });
                navigate({ to: "/" });
              }}
            >
              {target.kind === "demo" ? "Using the demo" : "Use the demo"}
            </button>
          </section>
        </div>
      </div>
    </div>
  );
}
