import { Link, Outlet, useLocation, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { Button, Dialog, DialogTrigger, Menu, MenuItem, MenuTrigger, Popover } from "react-aria-components";
import { Trunk } from "../../components/Fleet";
import { useConnection } from "../../connection";
import { useLive, useLocks, useRefs } from "../../data";
import { Visibility } from "../../gen/pando_platform_pb";
import { useRepository } from "../../platform-data";
import { RepoProvider, useRepo } from "../../repo";
import { useRefName } from "../../shell/hooks";
import { ago, ErrorState, Icon, useNow, type IconName } from "../../ui";

export function RepoLayout() {
  const { owner, repo } = useParams({ from: "/$owner/$repo" });
  return (
    <RepoProvider owner={owner} name={repo}>
      <Layout />
    </RepoProvider>
  );
}

const visibility: Record<number, string> = { [Visibility.PRIVATE]: "Private", [Visibility.INTERNAL]: "Internal", [Visibility.PUBLIC]: "Public" };

function Layout() {
  const { owner, name, key } = useRepo();
  const meta = useRepository(key);
  const refName = useRefName();
  const live = useLive(refName);
  const locks = useLocks();
  const { pathname } = useLocation();
  const section = pathname.split("/")[3] ?? "";
  const compact = section === "code" || section === "changes" || (section === "reviews" && !!pathname.split("/")[4]);
  const now = useNow();
  const rp = { owner, repo: name };

  if (meta.error && !meta.data) return <ErrorState error={meta.error} title={`${owner}/${name} didn't load`} retry={() => meta.refetch()} />;
  const r = meta.data;

  const tab = (to: string, label: string, icon: IconName, count?: number) => (
    <Link to={to} params={rp} search={(s: { ref?: string }) => ({ ref: s.ref })} activeOptions={{ exact: to === "/$owner/$repo", includeSearch: false }}>
      <Icon name={icon} size={15} />
      {label}
      {count ? <span className="count">{count}</span> : null}
    </Link>
  );

  return (
    <div className="repo-layout">
      <header className="repo-head" data-compact={compact || undefined}>
        <div className="repo-title">
          <h1>
            <Link to="/$owner" params={{ owner }} className="owner" style={{ color: "inherit" }}>
              {owner}
            </Link>
            <span className="owner"> / </span>
            <Link to="/$owner/$repo" params={rp} style={{ color: "inherit" }}>
              {name}
            </Link>
          </h1>
          {r && <span className="vis">{visibility[r.visibility] ?? "Private"}</span>}
          <div className="repo-actions">
            <RefPicker refName={refName} />
            <CloneButton />
          </div>
        </div>
        {!compact && r && (
          <>
            {r.description && <p className="repo-desc">{r.description}</p>}
            <div className="repo-stats">
              <span>
                <strong className="num">{r.lands24h.toLocaleString()}</strong> landed today
              </span>
              <span>
                <strong className="num">{r.openChangeRequests}</strong> open reviews
              </span>
              <span>Last landing {r.tipAtMs ? ago(Number(r.tipAtMs), now) : "never"}</span>
              {r.topics.length > 0 && (
                <span>
                  {r.topics.map((t) => (
                    <span key={t} className="topic">
                      {t}
                    </span>
                  ))}
                </span>
              )}
            </div>
            <Trunk changes={live.changes} />
          </>
        )}
        <nav className="tabs" aria-label="Repository">
          {tab("/$owner/$repo", "Overview", "repo")}
          {tab("/$owner/$repo/code/$", "Code", "code")}
          {tab("/$owner/$repo/changes", "Changes", "changes")}
          {tab("/$owner/$repo/reviews", "Reviews", "review", r?.openChangeRequests)}
          {tab("/$owner/$repo/locks", "Locks", "lock", locks.data?.length)}
          {tab("/$owner/$repo/agents", "Agents", "bot")}
          {tab("/$owner/$repo/settings", "Settings", "settings")}
        </nav>
      </header>
      <div className="repo-body">
        <Outlet />
      </div>
    </div>
  );
}

function RefPicker({ refName }: { refName: string }) {
  const refs = useRefs();
  const navigate = useNavigate();
  return (
    <MenuTrigger>
      <Button className="ref-button" aria-label={`Branch: ${refName}. Switch branch`}>
        <Icon name="branch" size={14} />
        <span>{refName}</span>
        <Icon name="down" size={13} />
      </Button>
      <Popover placement="bottom end" offset={6}>
        <Menu
          className="menu"
          aria-label="Branches"
          selectionMode="single"
          selectedKeys={[refName]}
          items={refs.data ?? []}
          onAction={(key) => navigate({ to: ".", search: (s: Record<string, unknown>) => ({ ...s, ref: key === "main" ? undefined : String(key) }) })}
          renderEmptyState={() => <div className="menu-item muted">{refs.isPending ? "Loading branches…" : "No branches"}</div>}
        >
          {(r) => (
            <MenuItem id={r.name} className="menu-item" textValue={r.name}>
              <span className="mono">{r.name}</span>
            </MenuItem>
          )}
        </Menu>
      </Popover>
    </MenuTrigger>
  );
}

function CloneButton() {
  const { owner, name } = useRepo();
  const { target } = useConnection();
  const server = target.kind === "server" ? target.url : "https://pando.example.com";
  const commands = [
    { label: "Clone", cmd: `pando clone ${server}/${owner}/${name}`, note: "Fetches one tree, lazily: seconds, even for huge histories." },
    { label: "Mount", cmd: `pando mount ~/${name} --server ${server}/${owner}/${name}`, note: "Read-only view; files download on first read." },
  ];
  return (
    <DialogTrigger>
      <Button className="button" data-primary="">
        <Icon name="code" size={15} /> Clone
      </Button>
      <Popover placement="bottom end" offset={6}>
        <Dialog className="clone" aria-label="Clone or mount">
          {target.kind === "demo" && <p>This is the demo; with your server the address below is real.</p>}
          {commands.map((c) => (
            <CopyCommand key={c.label} {...c} />
          ))}
        </Dialog>
      </Popover>
    </DialogTrigger>
  );
}

function CopyCommand({ label, cmd, note }: { label: string; cmd: string; note: string }) {
  const [done, setDone] = useState(false);
  return (
    <div style={{ display: "grid", gap: 4 }}>
      <h2>{label}</h2>
      <div className="cmd">
        <code>{cmd}</code>
        <button
          type="button"
          className="icon-button"
          aria-label={`Copy ${label.toLowerCase()} command`}
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(cmd);
              setDone(true);
              setTimeout(() => setDone(false), 1400);
            } catch {
              // Clipboard refused: the command is selectable text.
            }
          }}
        >
          <Icon name={done ? "check" : "copy"} size={14} />
        </button>
      </div>
      <p>{note}</p>
    </div>
  );
}
