import { Link, Outlet, useLocation, useNavigate, useParams } from "@tanstack/react-router";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { Mark, Wordmark } from "../brand";
import { pendingServer, useConnection } from "../connection";
import { isSingle, useCapabilities, useInbox, useRepositories, useViewer } from "../platform-data";
import { RepoProvider } from "../repo";
import { Author, Icon } from "../ui";
import type { MenuMode } from "./CommandMenu";
import { modKey, typing, useTheme } from "./hooks";

// The command menu is its own chunk, fetched when idle or on first open.
const CommandMenu = lazy(() => import("./CommandMenu").then((m) => ({ default: m.CommandMenu })));
const AccountMenu = lazy(() => import("./AccountMenu").then((m) => ({ default: m.AccountMenu })));
const Shortcuts = lazy(() => import("./AccountMenu").then((m) => ({ default: m.Shortcuts })));

export function AppShell() {
  const navigate = useNavigate();
  const params = useParams({ strict: false }) as { owner?: string; repo?: string; number?: string };
  const [theme, setTheme] = useTheme();
  const [menu, setMenu] = useState<MenuMode | null>(null);
  const [keys, setKeys] = useState(false);
  const [drawer, setDrawer] = useState(false);
  const chord = useRef(0);
  // What was typed before the menu could take focus; handed to it.
  const [early, setEarly] = useState("");
  const menuOpen = useRef(false);
  menuOpen.current = menu !== null || keys;
  const location = useLocation();
  // A `?server=` link opens the connection screen first: nothing is sent until the person confirms.
  useEffect(() => {
    if (pendingServer() && location.pathname !== "/connect") navigate({ to: "/connect", replace: true });
  }, [location.pathname, navigate]);
  useEffect(() => {
    setDrawer(false);
  }, [location.pathname]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setMenu((m) => (m ? null : "all"));
        return;
      }
      // While a menu is open every key belongs to it. Printable keys that arrive before its input
      // has focus are collected and handed to it, so nothing typed is lost.
      if (menuOpen.current && !typing(e) && e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        setEarly((q) => q + e.key);
        return;
      }
      if (menuOpen.current || typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (Date.now() - chord.current < 1200) {
        chord.current = 0;
        const repo = params.owner && params.repo ? { owner: params.owner, repo: params.repo } : null;
        if (e.key === "h") navigate({ to: "/" });
        else if (e.key === "i") navigate({ to: "/inbox" });
        else if (repo && e.key === "c") navigate({ to: "/$owner/$repo/code/$", params: { ...repo, _splat: "" } });
        else if (repo && e.key === "l") navigate({ to: "/$owner/$repo/changes", params: repo });
        else if (repo && e.key === "r") navigate({ to: "/$owner/$repo/reviews", params: repo });
        else if (repo && e.key === "k") navigate({ to: "/$owner/$repo/locks", params: repo });
        return;
      }
      if (e.key === "g") chord.current = Date.now();
      else if (e.key === "/") (e.preventDefault(), setMenu("all"));
      else if (e.key === "t" && params.repo) (e.preventDefault(), setMenu("files"));
      else if (e.key === "?") setKeys(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate, params.owner, params.repo]);

  const [menuLoaded, setMenuLoaded] = useState(false);
  useEffect(() => {
    if (menu) setMenuLoaded(true);
  }, [menu]);
  useEffect(() => {
    // Warm the command menu once the first screen is up, so it never competes with it.
    const id = setTimeout(() => void import("./CommandMenu"), 3000);
    return () => clearTimeout(id);
  }, []);
  useEffect(() => {
    if (!menu) setEarly("");
  }, [menu]);
  const commandMenu = menuLoaded || menu ? (
    <Suspense
      fallback={
        menu && (
          <div className="overlay">
            <div className="palette">
              <input className="palette-early" autoFocus value={early} onChange={(e) => setEarly(e.target.value)} placeholder="Search files, repositories, reviews and actions…" aria-label="Command menu" />
            </div>
          </div>
        )
      }
    >
      <CommandMenu open={menu} initialQuery={early} onClose={() => setMenu(null)} theme={theme} setTheme={setTheme} showShortcuts={() => setKeys(true)} />
    </Suspense>
  ) : null;

  return (
    <div className="app" data-drawer={drawer || undefined}>
      <button type="button" className="skip" onClick={() => document.getElementById("main")?.focus()}>
        Skip to content
      </button>
      <Sidebar onSearch={() => setMenu("all")} theme={theme} setTheme={setTheme} showShortcuts={() => setKeys(true)} />
      {drawer && <div className="scrim" onClick={() => setDrawer(false)} aria-hidden="true" />}
      <div className="main">
        <TopBar onMenu={() => setDrawer(true)} />
        <main className="content" id="main" tabIndex={-1}>
          <Outlet />
        </main>
      </div>
      {params.owner && params.repo ? (
        <RepoProvider owner={params.owner} name={params.repo}>
          {commandMenu}
        </RepoProvider>
      ) : (
        commandMenu
      )}
      {keys && (
        <Suspense>
          <Shortcuts open onClose={() => setKeys(false)} />
        </Suspense>
      )}
    </div>
  );
}

function Sidebar({ onSearch, theme, setTheme, showShortcuts }: { onSearch(): void; theme: string; setTheme(t: "system" | "light" | "dark"): void; showShortcuts(): void }) {
  const viewer = useViewer();
  const repos = useRepositories();
  const inbox = useInbox();
  const reviews = useCapabilities().data?.reviews ?? false;
  const { label, target } = useConnection();
  const org = viewer.data?.organizations[0];
  const waiting = inbox.data?.length ?? 0;
  const unread = inbox.data?.filter((i) => i.unread).length ?? 0;
  const now = Date.now();
  const me = (
    <>
      {viewer.data?.principal ? <Author author={viewer.data.principal} compact /> : <Icon name="users" />}
      <span style={{ minWidth: 0 }}>
        <span className="who-name">{viewer.data?.account?.displayName ?? (viewer.data?.principal || "Anonymous")}</span>
        <span className="conn">{target.kind === "demo" ? "Demo data" : label}</span>
      </span>
    </>
  );
  // Swapping the button would drop keyboard focus, so a focused one waits for its click.
  const [account, setAccount] = useState<"later" | "now" | "open">("later");
  const plain = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const id = setTimeout(() => document.activeElement !== plain.current && setAccount((a) => (a === "later" ? "now" : a)), 3000);
    return () => clearTimeout(id);
  }, []);
  const plainMe = (
    <button ref={plain} type="button" className="me" aria-label="Account and settings" aria-haspopup="menu" onClick={() => setAccount("open")}>
      {me}
    </button>
  );

  return (
    <nav className="sidebar" aria-label="Main">
      <div className="workspace">
        <Link to="/" className="lockup" aria-label="Pando home">
          <Mark size={22} />
          <Wordmark height={15} />
        </Link>
      </div>
      {org && (
        <Link to="/$owner" params={{ owner: org.login }} className="org-switch" style={{ color: "inherit", textDecoration: "none" }}>
          <span className="org-avatar" aria-hidden="true">
            {org.displayName.slice(0, 1).toUpperCase()}
          </span>
          <span style={{ minWidth: 0 }}>
            <span className="name">{org.displayName}</span>
            <span className="sub">
              {org.repositories} repositories, {org.agents} agents
            </span>
          </span>
        </Link>
      )}
      <button type="button" className="side-search" onClick={onSearch}>
        <Icon name="search" size={15} />
        <span>Search or jump to</span>
        <kbd>{modKey}K</kbd>
      </button>
      <div className="side-scroll">
        <Link to="/" className="side-link" activeOptions={{ exact: true }}>
          <Icon name="home" />
          <span className="label">Home</span>
        </Link>
        {reviews && (
        <Link to="/inbox" className="side-link">
          <Icon name="inbox" />
          <span className="label">Inbox</span>
          {waiting > 0 && (
            <span className="count" data-hot={unread > 0 || undefined} aria-label={`${waiting} waiting, ${unread} new`}>
              {waiting}
            </span>
          )}
        </Link>
        )}
        <div className="side-section">Repositories</div>
        {repos.data?.map((r) => {
          const live = now - Number(r.tipAtMs) < 90_000;
          return (
            <Link key={r.name} to="/$owner/$repo" params={{ owner: r.owner, repo: r.name }} className="side-link" activeOptions={{ exact: false }}>
              <span className="pulse" data-live={live || undefined} aria-label={live ? "landing now" : undefined} />
              <span className="label">{isSingle(`${r.owner}/${r.name}`) ? label : r.name}</span>
              {r.openChangeRequests > 0 && <span className="count" title={`${r.openChangeRequests} open change requests`}>{r.openChangeRequests}</span>}
            </Link>
          );
        })}
        {repos.isPending && <div className="side-link muted">Loading…</div>}
      </div>
      <div className="side-foot">
        {/* The menu (and the accessibility library under it) loads after the first screen, or on first use. */}
        {account === "later" ? (
          plainMe
        ) : (
          <Suspense fallback={plainMe}>
            <AccountMenu showShortcuts={showShortcuts} open={account === "open"}>
              {me}
            </AccountMenu>
          </Suspense>
        )}
        <button
          type="button"
          className="icon-button"
          onClick={() => setTheme(theme === "dark" ? "light" : theme === "light" ? "system" : "dark")}
          aria-label={`Theme: ${theme}. Change theme`}
          title={`Theme: ${theme}`}
        >
          <Icon name={theme === "dark" ? "moon" : "sun"} />
        </button>
      </div>
    </nav>
  );
}

const sections: Record<string, string> = {
  code: "Code",
  changes: "Changes",
  reviews: "Reviews",
  locks: "Locks",
  agents: "Agents",
  settings: "Settings",
};

function TopBar({ onMenu }: { onMenu(): void }) {
  const params = useParams({ strict: false }) as { owner?: string; repo?: string; number?: string };
  const { pathname } = useLocation();
  const { target, label } = useConnection();
  const navigate = useNavigate();
  const section = params.repo ? pathname.split("/")[3] : undefined;
  const single = !!params.owner && !!params.repo && isSingle(`${params.owner}/${params.repo}`);
  const title = pathname === "/" ? "Home" : pathname === "/inbox" ? "Inbox" : pathname === "/connect" ? "Connection" : pathname.startsWith("/settings") ? "Tokens" : null;

  return (
    <header className="topbar">
      <button type="button" className="icon-button mobile-nav" onClick={onMenu} aria-label="Open navigation">
        <Icon name="menu" />
      </button>
      <nav className="crumbs-top" aria-label="Breadcrumbs">
        {title && <span aria-current="page">{title}</span>}
        {params.owner && !single && (
          <Link to="/$owner" params={{ owner: params.owner }}>
            {params.owner}
          </Link>
        )}
        {params.repo && (
          <>
            {!single && <span className="sep">/</span>}
            <Link to="/$owner/$repo" params={{ owner: params.owner!, repo: params.repo }} activeOptions={{ exact: true }}>
              {single ? label : params.repo}
            </Link>
          </>
        )}
        {section && sections[section] && (
          <>
            <span className="sep">/</span>
            {params.number ? (
              <Link to="/$owner/$repo/reviews" params={{ owner: params.owner!, repo: params.repo! }}>
                {sections[section]}
              </Link>
            ) : (
              <span aria-current="page">{sections[section]}</span>
            )}
          </>
        )}
        {params.number && (
          <>
            <span className="sep">/</span>
            <span aria-current="page">#{params.number}</span>
          </>
        )}
      </nav>
      <span className="spacer" />
      {target.kind === "demo" && (
        <button type="button" className="demo-chip" onClick={() => navigate({ to: "/connect" })} title="Everything here is simulated in your browser">
          <span className="pulse" data-live />
          Demo data
          <span className="hide-small" style={{ textDecoration: "underline", textUnderlineOffset: 2, fontWeight: 500 }}>
            Connect your server
          </span>
        </button>
      )}
    </header>
  );
}
