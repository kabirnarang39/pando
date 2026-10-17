import { Link, Outlet, useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { Button, Dialog, Menu, MenuItem, MenuTrigger, Modal, ModalOverlay, Popover } from "react-aria-components";
import { useConnection } from "../connection";
import { useLive, useRefs, useWhoAmI } from "../data";
import { Icon, Mark } from "../ui";
import { CommandMenu, type MenuMode } from "./CommandMenu";
import { modKey, typing, useRefName, useTheme } from "./hooks";
import { Trunk } from "./Trunk";

export function Shell() {
  const { target, label, connect } = useConnection();
  const refName = useRefName();
  const navigate = useNavigate();
  const live = useLive(refName);
  const who = useWhoAmI();
  const [theme, setTheme] = useTheme();
  const [menu, setMenu] = useState<MenuMode | null>(null);
  const [keys, setKeys] = useState(false);
  const chord = useRef(0);
  const menuOpen = useRef(false);
  menuOpen.current = menu !== null || keys;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setMenu((m) => (m ? null : "all"));
        return;
      }
      // While a menu is open every key belongs to it, even before its input has focus.
      if (menuOpen.current || typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (Date.now() - chord.current < 1200) {
        chord.current = 0;
        const to = { c: "/code/$", l: "/changes", k: "/locks" }[e.key] as "/changes" | "/locks" | "/code/$" | undefined;
        if (to === "/code/$") navigate({ to, params: { _splat: "" }, search: (s) => ({ ref: s.ref }) });
        else if (to) navigate({ to, search: (s) => ({ ref: s.ref }) });
        return;
      }
      if (e.key === "g") chord.current = Date.now();
      else if (e.key === "/") (e.preventDefault(), setMenu("all"));
      else if (e.key === "t") (e.preventDefault(), setMenu("files"));
      else if (e.key === "?") setKeys(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  const state = live.error ? "offline" : live.changes ? "live" : "connecting";

  return (
    <div className="app">
      {target.kind === "demo" && (
        <div className="banner" role="note">
          <span>You are looking at a demo repository with simulated agents. Nothing here is real.</span>
          <button type="button" onClick={() => navigate({ to: "/connect" })}>
            Connect your server
          </button>
        </div>
      )}

      <header className="header">
        <Link to="/changes" search={(s) => ({ ref: s.ref })} className="brand" aria-label="Pando console home">
          <Mark size={26} />
          <span className="hide-small">Pando</span>
        </Link>
        <span className="repo" title={target.kind === "server" ? target.url : undefined}>
          {label}
        </span>
        <RefPicker refName={refName} />

        <button type="button" className="header-search" onClick={() => setMenu("all")} aria-label="Search files, changes and actions">
          <Icon name="search" />
          <span>Search files, changes and actions</span>
          <kbd className="hide-small">{modKey} K</kbd>
        </button>

        <div className="header-tools">
          <span className="live" data-state={state} title={live.error ? "Can't reach the server; retrying" : `Watching ${refName}`}>
            <span className="live-dot" aria-hidden="true" />
            <span className="label">{state === "offline" ? "Offline, retrying" : state === "live" ? "Live" : "Connecting"}</span>
          </span>
          {who.data && (
            <span className="live hide-small" title={who.data.fullRead ? "Full read access" : `May read: ${who.data.readPrefixes.join(", ")}`}>
              {who.data.principal}
            </span>
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
      </header>

      <Trunk changes={live.changes} refName={refName} />

      <nav className="rail" aria-label="Sections">
        <Link to="/changes" search={(s) => ({ ref: s.ref })} activeOptions={{ includeSearch: false }}>
          <Icon name="changes" size={18} />
          Changes
        </Link>
        <Link to="/code/$" params={{ _splat: "" }} search={(s) => ({ ref: s.ref })} activeOptions={{ includeSearch: false }}>
          <Icon name="code" size={18} />
          Code
        </Link>
        <Link to="/locks" search={(s) => ({ ref: s.ref })} activeOptions={{ includeSearch: false }}>
          <Icon name="lock" size={18} />
          Locks
        </Link>
        <span className="spacer" />
        <Link to="/connect" search={(s) => ({ ref: s.ref })} activeOptions={{ includeSearch: false }}>
          <Icon name="plug" size={18} />
          Connect
        </Link>
      </nav>

      <main id="main">
        <Outlet />
      </main>

      <CommandMenu
        open={menu}
        onClose={() => setMenu(null)}
        tip={live.tip.data}
        refName={refName}
        recent={live.changes}
        theme={theme}
        setTheme={setTheme}
        showShortcuts={() => setKeys(true)}
      />
      <Shortcuts open={keys} onClose={() => setKeys(false)} />
      {target.kind === "server" && live.error && who.error && <StaleConnection onDemo={() => connect({ kind: "demo" })} />}
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
      </Button>
      <Popover placement="bottom start" offset={6}>
        <Menu
          className="menu"
          aria-label="Branches"
          selectionMode="single"
          selectedKeys={[refName]}
          items={refs.data ?? []}
          onAction={(key) => navigate({ to: ".", search: (s) => ({ ...s, ref: key === "main" ? undefined : String(key) }) })}
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

const shortcuts: [string[], string][] = [
  [[modKey, "K"], "Search files, changes and actions"],
  [["/"], "Search"],
  [["t"], "Go to a file"],
  [["g", "l"], "Changes"],
  [["g", "c"], "Code"],
  [["g", "k"], "Locks"],
  [["j", "k"], "Next and previous change"],
  [["b"], "Show or hide blame"],
  [["h"], "Show a file's history"],
  [["u"], "Unified or split diff"],
  [["?"], "This list"],
];

function Shortcuts({ open, onClose }: { open: boolean; onClose(): void }) {
  return (
    <ModalOverlay className="overlay" isDismissable isOpen={open} onOpenChange={(o) => !o && onClose()}>
      <Modal>
        <Dialog className="dialog" aria-labelledby="keys-title">
          <h2 id="keys-title">Keyboard shortcuts</h2>
          <dl className="keys">
            {shortcuts.map(([k, what]) => (
              <div key={what} style={{ display: "contents" }}>
                <dt>
                  {k.map((x) => (
                    <kbd key={x}>{x}</kbd>
                  ))}
                </dt>
                <dd>{what}</dd>
              </div>
            ))}
          </dl>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

function StaleConnection({ onDemo }: { onDemo(): void }) {
  const navigate = useNavigate();
  return (
    <div className="toast" role="status">
      The server isn't answering.{" "}
      <button type="button" className="copy" onClick={() => navigate({ to: "/connect" })}>
        Check connection
      </button>{" "}
      <button type="button" className="copy" onClick={onDemo}>
        Use the demo
      </button>
    </div>
  );
}
